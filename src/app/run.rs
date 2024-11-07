use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::OpenOptions;
use std::io::BufWriter;

use std::result::Result;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tracing::{debug, error, info, instrument};

use crate::app::{analyze, log, result, target};
use crate::core::error::{GraphError, MonorailError};
use crate::core::{self, file, git, tracking, ChangeProviderKind, Target};

#[derive(Debug)]
pub(crate) struct HandleRunInput<'a> {
    pub(crate) git_opts: git::GitOptions<'a>,
    pub(crate) commands: Vec<&'a String>,
    pub(crate) sequences: Vec<&'a String>,
    pub(crate) targets: HashSet<&'a String>,
    pub(crate) include_deps: bool,
    pub(crate) fail_on_undefined: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct RunOutput {
    pub(crate) failed: bool,
    invocation_args: String,
    results: Vec<CommandRunResult>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CommandRunResult {
    command: String,
    successes: Vec<TargetRunResult>,
    failures: Vec<TargetRunResult>,
    unknowns: Vec<TargetRunResult>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct TargetRunResult {
    target: String,
    code: Option<i32>,
    stdout_path: Option<path::PathBuf>,
    stderr_path: Option<path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    runtime_secs: f32,
}

#[instrument]
pub(crate) async fn handle_run<'a>(
    cfg: &'a core::Config,
    input: &'a HandleRunInput<'a>,
    invocation_args: &'a str,
    work_path: &'a path::Path,
) -> Result<RunOutput, MonorailError> {
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    if cfg.targets.is_empty() {
        return Err(MonorailError::from("No configured targets"));
    }
    let mut tracking_run = get_next_tracking_run(cfg, &tracking_table)?;
    let log_dir = setup_log_dir(cfg, tracking_run.id, work_path)?;
    let commands = get_all_commands(cfg, &input.commands, &input.sequences)?;

    let (index, target_groups) = match input.targets.len() {
        0 => {
            let ths = cfg.get_target_path_set();
            let mut index = core::Index::new(cfg, &ths, work_path)?;
            let checkpoint = match tracking_table.open_checkpoint() {
                Ok(checkpoint) => Some(checkpoint),
                Err(MonorailError::TrackingCheckpointNotFound(_)) => None,
                Err(e) => {
                    return Err(e);
                }
            };

            let changes = match cfg.change_provider.r#use {
                ChangeProviderKind::Git => {
                    git::get_git_all_changes(&input.git_opts, &checkpoint, work_path).await?
                }
            };
            let ai = analyze::AnalyzeInput::new(false, false, true);
            let ao = analyze::analyze(&ai, &mut index, changes)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            (index, target_groups)
        }
        _ => {
            let mut index = core::Index::new(cfg, &input.targets, work_path)?;
            let target_groups = if input.include_deps {
                let ai = analyze::AnalyzeInput::new(false, false, true);
                let ao = analyze::analyze(&ai, &mut index, None)?;
                ao.target_groups
                    .ok_or(MonorailError::from("No target groups found"))?
            } else {
                // since the user specified the targets they want, without deps,
                // we will make synthetic serialized length 1 groups that ignore the graph
                let mut tg = vec![];
                for t in input.targets.iter() {
                    tg.push(vec![t.to_string()]);
                }
                debug!("Synthesizing target groups");
                tg
            };
            (index, target_groups)
        }
    };

    let run_data_groups = get_run_data_groups(
        &index,
        &commands,
        &cfg.targets,
        &target_groups,
        work_path,
        &log_dir,
    )?;
    let run_output = run_internal(
        cfg,
        &run_data_groups,
        &commands,
        &log_dir,
        input.fail_on_undefined,
        invocation_args,
    )
    .await?;

    // Update the run counter
    tracking_run.save()?;
    Ok(run_output)
}
#[derive(Debug, Serialize)]
struct RunData {
    target_path: String,
    command_work_path: path::PathBuf,
    command_path: Option<path::PathBuf>,
    command_args: Option<Vec<String>>,
    #[serde(skip)]
    logs: Logs,
}
#[derive(Debug, Serialize)]
struct RunDataGroup {
    #[serde(skip)]
    command_index: usize,
    datas: Vec<RunData>,
}
#[derive(Debug, Serialize)]
struct RunDataGroups {
    groups: Vec<RunDataGroup>,
}

// Builds the RunData for each target group that will be used when spawning tasks.
fn get_run_data_groups<'a>(
    index: &core::Index<'_>,
    commands: &'a [&'a String],
    targets: &[Target],
    target_groups: &[Vec<String>],
    work_path: &path::Path,
    log_dir: &path::Path,
) -> Result<RunDataGroups, MonorailError> {
    // for converting potentially deep nested paths into a single directory string
    let mut path_hasher = sha2::Sha256::new();

    let mut groups = Vec::with_capacity(target_groups.len());
    for (i, c) in commands.iter().enumerate() {
        for group in target_groups {
            let mut run_data = Vec::with_capacity(group.len());
            let mut unknowns = Vec::with_capacity(group.len());
            for target_path in group {
                let logs = Logs::new(log_dir, c.as_str(), target_path, &mut path_hasher)?;
                unknowns.push(TargetRunResult {
                    target: target_path.to_owned(),
                    code: None,
                    stdout_path: Some(logs.stdout_path.to_owned()),
                    stderr_path: Some(logs.stderr_path.to_owned()),
                    error: None,
                    runtime_secs: 0.0,
                });
                let target_index = index
                    .dag
                    .label2node
                    .get(target_path.as_str())
                    .copied()
                    .ok_or(MonorailError::DependencyGraph(
                        GraphError::LabelNodeNotFound(target_path.to_owned()),
                    ))?;
                let target = targets
                    .get(target_index)
                    .ok_or(MonorailError::from("Target not found"))?;
                let commands_path = work_path.join(target_path).join(&target.commands.path);
                let mut command_args = None;
                let command_path = match &target.commands.definitions {
                    Some(definitions) => match definitions.get(c.as_str()) {
                        Some(def) => {
                            let app_target_command = target::AppTargetCommand::new(
                                c,
                                Some(def),
                                &commands_path,
                                work_path,
                            );
                            debug!(
                                command_path = &app_target_command
                                    .path
                                    .as_ref()
                                    .unwrap_or(&path::PathBuf::new())
                                    .display()
                                    .to_string(),
                                command_args = &app_target_command
                                    .args
                                    .as_ref()
                                    .unwrap_or(&vec![])
                                    .join(" "),
                                "Using defined command"
                            );
                            command_args = app_target_command.args;
                            app_target_command.path
                        }
                        None => file::find_file_by_stem(c, &commands_path),
                    },
                    None => file::find_file_by_stem(c, &commands_path),
                };
                run_data.push(RunData {
                    target_path: target_path.to_owned(),
                    command_work_path: work_path.join(target_path),
                    command_path,
                    command_args,
                    logs,
                });
            }
            groups.push(RunDataGroup {
                command_index: i,
                datas: run_data,
            });
        }
    }

    Ok(RunDataGroups { groups })
}

pub(crate) fn spawn_task(
    command_work_path: &path::Path,
    command_path: &path::Path,
    command_args: &Option<Vec<String>>,
) -> Result<tokio::process::Child, MonorailError> {
    debug!(
        command_path = command_path.display().to_string(),
        command_args = command_args.clone().unwrap_or_default().join(", "),
        "Spawn task"
    );
    let mut cmd = tokio::process::Command::new(command_path);
    cmd.current_dir(command_work_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // parallel execution makes use of stdin impractical
        .stdin(std::process::Stdio::null());
    if let Some(ca) = command_args {
        cmd.args(ca);
    }
    cmd.spawn().map_err(MonorailError::from)
}

#[derive(Debug)]
struct CommandTask {
    id: usize,
    start_time: time::Instant,
}
impl CommandTask {
    #[allow(clippy::too_many_arguments)]
    #[instrument]
    async fn run(
        &mut self,
        mut child: tokio::process::Child,
        token: sync::Arc<tokio_util::sync::CancellationToken>,
        target: String,
        command: String,
        log_config: core::LogConfig,
        stdout_client: log::CompressorClient,
        stderr_client: log::CompressorClient,
        log_stream_client: Option<log::StreamClient>,
    ) -> Result<
        (usize, std::process::ExitStatus, time::Duration),
        (usize, time::Duration, MonorailError),
    > {
        let (stdout_log_stream_client, stderr_log_stream_client) = match log_stream_client {
            Some(lsc) => {
                let allowed =
                    log::is_log_allowed(&lsc.args.targets, &lsc.args.commands, &target, &command);
                let stdout_lsc = if allowed && lsc.args.include_stdout {
                    Some(lsc.clone())
                } else {
                    None
                };
                let stderr_lsc = if allowed && lsc.args.include_stderr {
                    Some(lsc.clone())
                } else {
                    None
                };
                (stdout_lsc, stderr_lsc)
            }
            None => (None, None),
        };
        let stdout_header = log::get_header(&stdout_client.file_name, &target, &command, true);
        let stdout_fut = log::process_reader(
            &log_config,
            tokio::io::BufReader::new(child.stdout.take().unwrap()), // todo unwrap
            stdout_client,
            stdout_header,
            stdout_log_stream_client,
            token.clone(),
        );
        let stderr_header = log::get_header(&stderr_client.file_name, &target, &command, true);
        let stderr_fut = log::process_reader(
            &log_config,
            tokio::io::BufReader::new(child.stderr.take().unwrap()), // todo unwrap
            stderr_client,
            stderr_header,
            stderr_log_stream_client,
            token.clone(),
        );
        let child_fut = async { child.wait().await.map_err(MonorailError::from) };

        let (_stdout_result, _stderr_result, child_result) =
            tokio::try_join!(stdout_fut, stderr_fut, child_fut)
                .map_err(|e| (self.id, self.start_time.elapsed(), e))?;
        Ok((self.id, child_result, self.start_time.elapsed()))
    }
}

fn get_next_tracking_run(
    cfg: &core::Config,
    tracking_table: &tracking::Table,
) -> Result<tracking::Run, MonorailError> {
    // obtain current log info counter and increment it before using
    let mut run = match tracking_table.open_run() {
        Ok(run) => run,
        Err(MonorailError::TrackingRunNotFound(_)) => tracking_table.new_run(),
        Err(e) => {
            return Err(e);
        }
    };
    if run.id >= cfg.max_retained_runs {
        run.id = 0;
    }
    run.id += 1;
    Ok(run)
}

#[instrument]
fn setup_log_dir(
    cfg: &core::Config,
    run_id: usize,
    work_path: &path::Path,
) -> Result<path::PathBuf, MonorailError> {
    let log_dir = cfg.get_log_path(work_path).join(format!("{}", run_id));
    // remove the log_dir path if it exists, and create a new one
    std::fs::remove_dir_all(&log_dir).unwrap_or(());
    std::fs::create_dir_all(&log_dir)?;
    Ok(log_dir)
}

// Expands sequences into commands and appends any explicit commands.
fn get_all_commands<'a>(
    cfg: &'a core::Config,
    commands: &'a [&'a String],
    sequences: &'a [&'a String],
) -> Result<Vec<&'a String>, MonorailError> {
    // append provided commands to any expanded sequences provided
    let mut all_commands = vec![];
    if !sequences.is_empty() {
        let cfg_sequences = cfg.sequences.as_ref().ok_or(MonorailError::from(
            "No sequences are defined in configuration",
        ))?;
        for seq in sequences {
            all_commands.extend(
                cfg_sequences
                    .get(seq.as_str())
                    .ok_or(MonorailError::Generic(format!(
                        "Sequence {} is not defined",
                        seq
                    )))?,
            );
        }
    }

    all_commands.extend_from_slice(commands);
    Ok(all_commands)
}

async fn initialize_log_stream(addr: &str) -> Option<log::StreamClient> {
    match log::StreamClient::connect().await {
        Ok(client) => Some(client),
        Err(e) => {
            debug!(error = e.to_string(), "Log streaming disabled");
            None
        }
    }
}

async fn process_run_data_groups(
    cfg: &core::Config,
    run_data_groups: &RunDataGroups,
    all_commands: &[&String],
    fail_on_undefined: bool,
) -> Result<(Vec<CommandRunResult>, bool), MonorailError> {
    // TODO: parameterize addr from cfg
    let log_stream_client = initialize_log_stream("127.0.0.1:9201").await;

    let mut results = Vec::new();
    let mut cancelled = false;
    let mut failed = false;

    for run_data_group in &run_data_groups.groups {
        let command = &all_commands[run_data_group.command_index];
        let mut crr = CommandRunResult::new(command);
        let mut crr = CommandRunResult {
            command: command.to_string(),
            successes: vec![],
            failures: vec![],
            unknowns: vec![],
        };

        info!(
            num = run_data_group.datas.len(),
            command = command,
            "processing targets",
        );
        if cancelled {
            let mut unknowns = vec![];
            for rd in run_data_group.datas.iter() {
                let target = rd.target_path.to_owned();
                error!(
                    status = "aborted",
                    command = command,
                    target = &rd.target_path
                );
                unknowns.push(TargetRunResult {
                    target,
                    code: None,
                    stdout_path: None,
                    stderr_path: None,
                    error: Some("command task cancelled".to_string()),
                    runtime_secs: 0.0,
                });
            }
            results.push(CommandRunResult {
                command: command.to_string(),
                successes: vec![],
                failures: vec![],
                unknowns,
            });
            continue;
        }

        let mut js = tokio::task::JoinSet::new();
        let token = sync::Arc::new(tokio_util::sync::CancellationToken::new());
        let mut abort_table = HashMap::new();
        let compressor_shutdown = sync::Arc::new(sync::atomic::AtomicBool::new(false));
        let mut compressor = log::Compressor::new(2, compressor_shutdown.clone());
        let mut compressor_clients = vec![];
        for id in 0..run_data_group.datas.len() {
            let rd = &run_data_group.datas[id];
            let stdout_client = compressor.register(&rd.logs.stdout_path)?;
            let stderr_client = compressor.register(&rd.logs.stderr_path)?;
            compressor_clients.push(stdout_client.clone());
            compressor_clients.push(stderr_client.clone());
            let mut ft = CommandTask {
                id,
                start_time: time::Instant::now(),
            };
            let target2 = rd.target_path.clone();
            let command2 = command.to_string();

            if let Some(command_path) = &rd.command_path {
                // check that the command path is executable before proceeding
                if file::is_executable(command_path) {
                    info!(
                        status = "scheduled",
                        command = command,
                        target = &rd.target_path,
                        "task"
                    );
                    let child = spawn_task(&rd.command_work_path, command_path, &rd.command_args)?;
                    let token2 = token.clone();
                    let log_stream_client2 = log_stream_client.clone();
                    let log_config = cfg.log.clone();
                    let handle = js.spawn(async move {
                        ft.run(
                            child,
                            token2,
                            target2,
                            command2,
                            log_config,
                            stdout_client,
                            stderr_client,
                            log_stream_client2,
                        )
                        .await
                    });
                    abort_table.insert(handle.id(), id);
                } else {
                    info!(
                        status = "non_executable",
                        command = command,
                        target = &rd.target_path,
                        "task"
                    );
                    crr.unknowns.push(TargetRunResult {
                        target: target2,
                        code: None,
                        stdout_path: None,
                        stderr_path: None,
                        error: Some("command not executable".to_string()),
                        runtime_secs: 0.0,
                    });
                }
            } else {
                info!(
                    status = "undefined",
                    command = command,
                    target = &rd.target_path,
                    "task"
                );
                let trr = TargetRunResult {
                    target: target2,
                    code: None,
                    stdout_path: None,
                    stderr_path: None,
                    error: Some("command not found".to_string()),
                    runtime_secs: 0.0,
                };
                if fail_on_undefined {
                    crr.failures.push(trr);
                } else {
                    crr.unknowns.push(trr);
                }
            }
        }
        let compressor_handle = std::thread::spawn(move || compressor.run());
        while let Some(join_res) = js.join_next().await {
            match join_res {
                Ok(task_res) => {
                    match task_res {
                        Ok((id, status, duration)) => {
                            let rd = &run_data_group.datas[id];
                            let mut trr = TargetRunResult {
                                target: rd.target_path.to_owned(),
                                code: status.code(),
                                stdout_path: Some(rd.logs.stdout_path.to_owned()),
                                stderr_path: Some(rd.logs.stderr_path.to_owned()),
                                error: None,
                                runtime_secs: duration.as_secs_f32(),
                            };
                            if status.success() {
                                info!(
                                    status = "success",
                                    command = command,
                                    target = &rd.target_path,
                                    "task"
                                );

                                crr.successes.push(trr);
                            } else {
                                // TODO: --cancel-on-error option
                                token.cancel();
                                cancelled = true;
                                error!(
                                    status = "failed",
                                    command = command,
                                    target = &rd.target_path,
                                    "task"
                                );

                                trr.error = Some("command returned an error".to_string());
                                crr.failures.push(trr);
                            }
                        }
                        Err((id, duration, e)) => {
                            // TODO: --cancel-on-error option
                            token.cancel();
                            cancelled = true;

                            let (status, message) = match e {
                                MonorailError::TaskCancelled => {
                                    ("cancelled", "command task cancelled".to_string())
                                }
                                _ => ("error", format!("command task failed: {}", e)),
                            };

                            let rd = &run_data_group.datas[id];
                            error!(
                                status = status,
                                command = command,
                                target = &rd.target_path,
                                "task"
                            );

                            crr.failures.push(TargetRunResult {
                                target: rd.target_path.to_owned(),
                                code: None,
                                stdout_path: Some(rd.logs.stdout_path.to_owned()),
                                stderr_path: Some(rd.logs.stderr_path.to_owned()),
                                error: Some(message),
                                runtime_secs: duration.as_secs_f32(),
                            });
                        }
                    }
                }
                Err(e) => {
                    if e.is_cancelled() {
                        let run_data_index = abort_table
                            .get(&e.id())
                            .ok_or(MonorailError::from("Task id missing from abort table"))?;
                        let rd = &run_data_group.datas[*run_data_index];
                        error!(
                            status = "aborted",
                            command = command,
                            target = &rd.target_path,
                            "task"
                        );

                        crr.unknowns.push(TargetRunResult {
                            target: rd.target_path.to_owned(),
                            code: None,
                            stdout_path: None,
                            stderr_path: None,
                            error: Some("command task cancelled".to_string()),
                            runtime_secs: 0.0,
                        })
                    }
                }
            }
        }
        for client in compressor_clients.iter() {
            client.shutdown()?;
        }

        // Unwrap for thread dyn Any panic contents, which isn't easily mapped to a MonorailError
        // because it doesn't impl Error; however, the internals of this handle do, so they
        // will get propagated.
        compressor_handle.join().unwrap()?;
        if !crr.failures.is_empty() {
            failed = true;
        }

        results.push(crr);
    }

    Ok((results, failed))
}

#[allow(clippy::too_many_arguments)]
#[instrument]
async fn run_internal<'a>(
    cfg: &'a core::Config,
    run_data_groups: &'a RunDataGroups,
    commands: &'a [&'a String],
    log_dir: &path::Path,
    fail_on_undefined: bool,
    invocation_args: &'a str,
) -> Result<RunOutput, MonorailError> {
    info!(num = run_data_groups.groups.len(), "processing groups");

    let (results, failed) =
        process_run_data_groups(cfg, run_data_groups, commands, fail_on_undefined).await?;

    let o = RunOutput {
        failed,
        invocation_args: invocation_args.to_owned(),
        results,
    };

    store_run_output(&o, log_dir)?;

    Ok(o)
}

// Serialize and store the compressed results of the given RunOutput.
fn store_run_output(run_output: &RunOutput, log_dir: &path::Path) -> Result<(), MonorailError> {
    let run_result_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_dir.join(result::RESULT_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let bw = BufWriter::new(run_result_file);
    let mut encoder = zstd::stream::write::Encoder::new(bw, 3)?;
    serde_json::to_writer(&mut encoder, run_output)?;
    encoder.finish()?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct Logs {
    stdout_path: path::PathBuf,
    stderr_path: path::PathBuf,
}
impl Logs {
    fn new(
        log_dir: &path::Path,
        command: &str,
        target_path: &str,
        hasher: &mut sha2::Sha256,
    ) -> Result<Self, MonorailError> {
        hasher.update(target_path);
        let dir_path = log_dir
            .join(command)
            .join(format!("{:x}", hasher.finalize_reset()));
        std::fs::create_dir_all(&dir_path)?;
        Ok(Self {
            stdout_path: dir_path.clone().join(log::STDOUT_FILE),
            stderr_path: dir_path.clone().join(log::STDERR_FILE),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;
    use std::fs::File;
    use std::io::Read;
    use tempfile::tempdir;
    use zstd::stream::read::Decoder;

    use sha2::Sha256;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn test_get_run_data_groups() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let cmd1 = "cmd4".to_string();
        let commands = vec![&cmd1];
        let target_groups = vec![vec!["target1".to_string()]];
        let log_dir = work_path.join("log_dir");

        let result = get_run_data_groups(
            &index,
            &commands,
            &c.targets,
            &target_groups,
            work_path,
            &log_dir,
        );
        assert!(result.is_ok(), "get_run_data_groups returned an error");

        let run_data_groups = result.unwrap();
        assert_eq!(run_data_groups.groups.len(), 1);
        assert_eq!(run_data_groups.groups[0].datas.len(), 1);

        let run_data = &run_data_groups.groups[0].datas[0];
        assert_eq!(run_data.target_path, "target1");
        assert!(run_data.command_path.is_some());
        assert_eq!(run_data.command_args, Some(vec!["arg1".to_string()]));
    }

    #[test]
    fn test_store_run_output_success() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let log_dir = temp_dir.path();

        let run_output = RunOutput {
            failed: false,
            invocation_args: "example args".to_string(),
            results: vec![
                CommandRunResult {
                    command: "build".to_string(),
                    successes: vec![TargetRunResult {
                        target: "target1".to_string(),
                        code: Some(0),
                        stdout_path: Some(log_dir.join("stdout_target1.log")),
                        stderr_path: Some(log_dir.join("stderr_target1.log")),
                        error: None,
                        runtime_secs: 12.3,
                    }],
                    failures: vec![],
                    unknowns: vec![],
                },
                CommandRunResult {
                    command: "test".to_string(),
                    successes: vec![],
                    failures: vec![TargetRunResult {
                        target: "target2".to_string(),
                        code: Some(1),
                        stdout_path: Some(log_dir.join("stdout_target2.log")),
                        stderr_path: Some(log_dir.join("stderr_target2.log")),
                        error: Some("Compilation error".to_string()),
                        runtime_secs: 5.5,
                    }],
                    unknowns: vec![],
                },
            ],
        };

        let result = store_run_output(&run_output, log_dir);
        assert!(result.is_ok(), "store_run_output should succeed");

        let result_file_path = log_dir.join(result::RESULT_OUTPUT_FILE_NAME);
        assert!(result_file_path.exists(), "Result file should be created");

        let compressed_file = File::open(&result_file_path).expect("Failed to open result file");
        let mut decoder = Decoder::new(compressed_file).expect("Failed to create decoder");
        let mut decompressed_data = String::new();
        decoder
            .read_to_string(&mut decompressed_data)
            .expect("Failed to read decompressed data");

        let deserialized_output: RunOutput =
            serde_json::from_str(&decompressed_data).expect("Failed to deserialize data");

        assert_eq!(
            run_output, deserialized_output,
            "Deserialized data should match the original"
        );
    }

    #[tokio::test]
    async fn test_spawn_task_success() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let work_path = temp_dir.path();

        let command_path = path::Path::new("echo");
        let command_args = Some(vec!["Hello, world!".to_string()]);

        let child_result = spawn_task(work_path, command_path, &command_args);
        assert!(
            child_result.is_ok(),
            "spawn_task should succeed with valid command"
        );

        let child = child_result.unwrap();
        let output = child
            .wait_with_output()
            .await
            .expect("Failed to read child output");
        assert!(
            output.status.success(),
            "The command should exit successfully"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Hello, world!"),
            "Output should contain the expected message"
        );
    }

    #[tokio::test]
    async fn test_spawn_task_failure() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let work_path = temp_dir.path();

        let command_path = path::Path::new("invalid_command");
        let command_args = Some(vec!["arg1".to_string(), "arg2".to_string()]);

        let child_result = spawn_task(work_path, command_path, &command_args);
        assert!(
            child_result.is_err(),
            "spawn_task should fail with an invalid command"
        );
    }
}
