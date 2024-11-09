use std::{fs, io, path, sync, thread, time};

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
    pub(crate) args: Vec<&'a String>,
    pub(crate) arg_map: Option<&'a String>,
    pub(crate) arg_map_file: Option<&'a String>,
    pub(crate) include_deps: bool,
    pub(crate) fail_on_undefined: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct OutRunFiles {
    stdout: String,
    stderr: String,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct OutRun {
    path: String,
    files: OutRunFiles,
    targets: HashMap<String, String>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Out {
    run: OutRun,
}
impl Out {
    fn new(run_path: &path::Path) -> Self {
        Self {
            run: OutRun {
                path: run_path.display().to_string(),
                files: OutRunFiles {
                    stdout: "stdout.zst".to_string(),
                    stderr: "stderr.zst".to_string(),
                },
                targets: HashMap::new(),
            },
        }
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct RunOutput {
    pub(crate) failed: bool,
    invocation: String,
    out: Out,
    results: Vec<CommandRunResult>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CommandRunResult {
    command: String,
    successes: Vec<TargetRunResult>,
    failures: Vec<TargetRunResult>,
    unknowns: Vec<TargetRunResult>,
}
impl CommandRunResult {
    fn new(command: &str) -> Self {
        Self {
            command: command.to_string(),
            successes: vec![],
            failures: vec![],
            unknowns: vec![],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct TargetRunResult {
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_secs: Option<f32>,
}

#[derive(Deserialize, Default)]
struct ArgMap {
    table: HashMap<String, HashMap<String, Vec<String>>>,
}
impl ArgMap {
    fn new(input: &HandleRunInput<'_>) -> Result<Self, MonorailError> {
        if !input.args.is_empty() {
            if input.commands.len() != 1 {
                return Err(MonorailError::from(
                    "When providing --arg, only one command may be specified",
                ));
            }
            if input.targets.len() != 1 {
                return Err(MonorailError::from(
                    "When providing --arg, only one target may be specified",
                ));
            }
            Ok(Self {
                table: HashMap::from([(
                    input
                        .targets
                        .iter()
                        .next()
                        .ok_or(MonorailError::from("Could not extract target"))?
                        .to_string(),
                    HashMap::from([(
                        input.commands[0].to_string(),
                        input.args.iter().map(|s| s.to_string()).collect(),
                    )]),
                )]),
            })
        } else if let Some(am) = input.arg_map {
            let table: HashMap<String, HashMap<String, Vec<String>>> = serde_json::from_str(am)
                .map_err(|e| {
                    MonorailError::Generic(format!("Inline arg map contains invalid JSON; {}", e))
                })?;
            Ok(Self { table })
        } else if let Some(amf) = input.arg_map_file {
            let p = path::Path::new(amf);
            let f = fs::File::open(p).map_err(MonorailError::from)?;
            let br = io::BufReader::new(f);
            let table: HashMap<String, HashMap<String, Vec<String>>> = serde_json::from_reader(br)
                .map_err(|e| {
                    MonorailError::Generic(format!(
                        "File arg map at {} contains invalid JSON; {}",
                        p.display(),
                        e
                    ))
                })?;
            Ok(Self { table })
        } else {
            Ok(Default::default())
        }
    }
    fn get_args<'a>(&'a self, target: &'a str, command: &'a str) -> Option<&'a [String]> {
        if let Some(cmd_map) = &self.table.get(target) {
            if let Some(args) = &cmd_map.get(command) {
                return Some(args);
            }
        }
        None
    }
}

#[instrument]
pub(crate) async fn handle_run<'a>(
    cfg: &'a core::Config,
    input: &'a HandleRunInput<'a>,
    invocation: &'a str,
    work_path: &'a path::Path,
) -> Result<RunOutput, MonorailError> {
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    if cfg.targets.is_empty() {
        return Err(MonorailError::from("No configured targets"));
    }
    let mut tracking_run = get_next_tracking_run(cfg, &tracking_table)?;
    let run_path = setup_run_path(cfg, tracking_run.id, work_path)?;
    let commands = get_all_commands(cfg, &input.commands, &input.sequences)?;
    let arg_map = ArgMap::new(input)?;

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
        &run_path,
        &arg_map,
    )?;
    let run_output = run_internal(
        cfg,
        run_data_groups,
        &commands,
        &run_path,
        input.fail_on_undefined,
        invocation,
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
    out: Out,
}

// Builds the RunData for each target group that will be used when spawning tasks.
fn get_run_data_groups<'a>(
    index: &core::Index<'_>,
    commands: &'a [&'a String],
    targets: &[Target],
    target_groups: &[Vec<String>],
    work_path: &path::Path,
    run_path: &path::Path,
    arg_map: &ArgMap,
) -> Result<RunDataGroups, MonorailError> {
    let mut out = Out::new(run_path);

    // for converting potentially deep nested paths into a single directory string
    let mut hasher = sha2::Sha256::new();

    let mut groups = Vec::with_capacity(target_groups.len());
    for (i, cmd) in commands.iter().enumerate() {
        for group in target_groups {
            let mut run_data = Vec::with_capacity(group.len());
            let mut unknowns = Vec::with_capacity(group.len());
            for target_path in group {
                hasher.update(target_path);
                let target_hash = format!("{:x}", hasher.finalize_reset());
                out.run
                    .targets
                    .insert(target_path.to_string(), target_hash.clone());
                let logs = Logs::new(run_path, cmd.as_str(), &target_hash)?;
                unknowns.push(TargetRunResult {
                    target: target_path.to_owned(),
                    code: None,
                    error: None,
                    runtime_secs: None,
                });
                let target_index = index
                    .dag
                    .label2node
                    .get(target_path.as_str())
                    .copied()
                    .ok_or(MonorailError::DependencyGraph(
                        GraphError::LabelNodeNotFound(target_path.to_owned()),
                    ))?;
                let tar = targets
                    .get(target_index)
                    .ok_or(MonorailError::from("Target not found"))?;
                let commands_path = work_path.join(target_path).join(&tar.commands.path);
                let mut command_args = None;
                let command_path = match &tar.commands.definitions {
                    Some(definitions) => match definitions.get(cmd.as_str()) {
                        Some(def) => {
                            let app_target_command = target::AppTargetCommand::new(
                                cmd,
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
                        None => file::find_file_by_stem(cmd, &commands_path),
                    },
                    None => file::find_file_by_stem(cmd, &commands_path),
                };
                // now, append any runtime args to existing config args, if present
                let arg_map_args = arg_map.get_args(&tar.path, cmd);
                if let Some(ref mut args) = command_args {
                    if let Some(am_args) = arg_map_args {
                        args.extend_from_slice(am_args);
                    }
                } else if let Some(am_args) = arg_map_args {
                    command_args = Some(am_args.to_owned());
                }
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

    Ok(RunDataGroups { groups, out })
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

struct CommandTaskFinishInfo {
    id: usize,
    status: std::process::ExitStatus,
    elapsed: time::Duration,
}
struct CommandTaskCancelInfo {
    id: usize,
    elapsed: time::Duration,
    error: MonorailError,
}

#[derive(Debug)]
struct CommandTask {
    id: usize,
    start_time: time::Instant,
    token: sync::Arc<tokio_util::sync::CancellationToken>,
    target: sync::Arc<String>,
    command: sync::Arc<String>,
    log_config: sync::Arc<core::LogConfig>,
    stdout_client: log::CompressorClient,
    stderr_client: log::CompressorClient,
    log_stream_client: Option<log::StreamClient>,
}
impl CommandTask {
    #[allow(clippy::too_many_arguments)]
    #[instrument]
    async fn run(
        &mut self,
        mut child: tokio::process::Child,
    ) -> Result<CommandTaskFinishInfo, CommandTaskCancelInfo> {
        let (stdout_log_stream_client, stderr_log_stream_client) = match &self.log_stream_client {
            Some(lsc) => {
                let allowed = log::is_log_allowed(
                    &lsc.args.targets,
                    &lsc.args.commands,
                    &self.target,
                    &self.command,
                );
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
        let stdout_header = log::get_header(
            &self.stdout_client.file_name,
            &self.target,
            &self.command,
            true,
        );
        let stdout_fut = log::process_reader(
            &self.log_config,
            tokio::io::BufReader::new(
                child
                    .stdout
                    .take()
                    .ok_or(MonorailError::from("Missing stdout task stream"))
                    .map_err(|e| CommandTaskCancelInfo {
                        id: self.id,
                        elapsed: self.start_time.elapsed(),
                        error: e,
                    })?,
            ),
            self.stdout_client.clone(),
            stdout_header,
            stdout_log_stream_client,
            self.token.clone(),
        );
        let stderr_header = log::get_header(
            &self.stderr_client.file_name,
            &self.target,
            &self.command,
            true,
        );
        let stderr_fut = log::process_reader(
            &self.log_config,
            tokio::io::BufReader::new(
                child
                    .stderr
                    .take()
                    .ok_or(MonorailError::from("Missing stderr task stream"))
                    .map_err(|e| CommandTaskCancelInfo {
                        id: self.id,
                        elapsed: self.start_time.elapsed(),
                        error: e,
                    })?,
            ),
            self.stderr_client.clone(),
            stderr_header,
            stderr_log_stream_client,
            self.token.clone(),
        );
        let child_fut = async { child.wait().await.map_err(MonorailError::from) };

        let (_stdout_result, _stderr_result, child_result) =
            tokio::try_join!(stdout_fut, stderr_fut, child_fut).map_err(|e| {
                CommandTaskCancelInfo {
                    id: self.id,
                    elapsed: self.start_time.elapsed(),
                    error: e,
                }
            })?;
        Ok(CommandTaskFinishInfo {
            id: self.id,
            status: child_result,
            elapsed: self.start_time.elapsed(),
        })
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
fn setup_run_path(
    cfg: &core::Config,
    run_id: usize,
    work_path: &path::Path,
) -> Result<path::PathBuf, MonorailError> {
    let run_path = cfg.get_run_path(work_path).join(format!("{}", run_id));
    // remove the run_path path if it exists, and create a new one
    std::fs::remove_dir_all(&run_path).unwrap_or(());
    std::fs::create_dir_all(&run_path)?;
    Ok(run_path)
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
    match log::StreamClient::connect(addr).await {
        Ok(client) => Some(client),
        Err(e) => {
            debug!(error = e.to_string(), "Log streaming disabled");
            None
        }
    }
}

fn create_aborted_unknowns_result(command: &str, run_data: &[RunData]) -> CommandRunResult {
    let mut unknowns = vec![];
    for rd in run_data.iter() {
        let target = rd.target_path.to_owned();
        error!(
            status = "aborted",
            command = command,
            target = &rd.target_path
        );
        unknowns.push(TargetRunResult {
            target,
            code: None,
            error: Some("command task cancelled".to_string()),
            runtime_secs: None,
        });
    }
    CommandRunResult {
        command: command.to_string(),
        successes: vec![],
        failures: vec![],
        unknowns,
    }
}

async fn process_task_results(
    mut js: tokio::task::JoinSet<Result<CommandTaskFinishInfo, CommandTaskCancelInfo>>,
    run_data_group: &RunDataGroup,
    token: &sync::Arc<tokio_util::sync::CancellationToken>,
    abort_table: HashMap<tokio::task::Id, usize>,
    command: &str,
    crr: &mut CommandRunResult,
) -> Result<bool, MonorailError> {
    let mut cancelled = false;
    while let Some(join_res) = js.join_next().await {
        match join_res {
            Ok(task_res) => {
                match task_res {
                    Ok(info) => {
                        let rd = &run_data_group.datas[info.id];
                        let mut trr = TargetRunResult {
                            target: rd.target_path.to_owned(),
                            code: info.status.code(),
                            error: None,
                            runtime_secs: Some(info.elapsed.as_secs_f32()),
                        };
                        if info.status.success() {
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
                    Err(info) => {
                        // TODO: --cancel-on-error option
                        token.cancel();
                        cancelled = true;

                        let (status, message) = match info.error {
                            MonorailError::TaskCancelled => {
                                ("cancelled", "command task cancelled".to_string())
                            }
                            _ => ("error", format!("command task failed: {}", info.error)),
                        };

                        let rd = &run_data_group.datas[info.id];
                        error!(
                            status = status,
                            command = command,
                            target = &rd.target_path,
                            "task"
                        );

                        crr.failures.push(TargetRunResult {
                            target: rd.target_path.to_owned(),
                            code: None,
                            error: Some(message),
                            runtime_secs: Some(info.elapsed.as_secs_f32()),
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
                        error: Some("command task cancelled".to_string()),
                        runtime_secs: None,
                    })
                }
            }
        }
    }
    Ok(cancelled)
}

// Prepare the compressor ahead of time so that we can run it before spawning tasks.
// While this involves a second iteration of the RunData slice, this is necessary to
// avoid potentially blocking chatty tasks behind running the compressor.
fn initialize_compressor(
    run_datas: &[RunData],
    num_threads: usize,
) -> Result<
    (
        log::Compressor,
        Vec<(log::CompressorClient, log::CompressorClient)>,
    ),
    MonorailError,
> {
    let mut compressor = log::Compressor::new(
        num_threads,
        sync::Arc::new(sync::atomic::AtomicBool::new(false)),
    );
    let mut clients = Vec::new();

    for run_data in run_datas.iter() {
        let stdout_client = compressor.register(&run_data.logs.stdout_path)?;
        let stderr_client = compressor.register(&run_data.logs.stderr_path)?;
        clients.push((stdout_client, stderr_client));
    }
    Ok((compressor, clients))
}

async fn schedule_task(
    mut task: CommandTask,
    rd: &RunData,
    join_set: &mut tokio::task::JoinSet<Result<CommandTaskFinishInfo, CommandTaskCancelInfo>>,
    abort_table: &mut HashMap<tokio::task::Id, usize>,
    crr: &mut CommandRunResult,
    fail_on_undefined: bool,
) -> Result<(), MonorailError> {
    if let Some(command_path) = &rd.command_path {
        // check that the command path is executable before proceeding
        if file::is_executable(command_path) {
            info!(
                status = "scheduled",
                command = *task.command,
                target = &rd.target_path,
                "task"
            );
            let task_id = task.id;
            let child = spawn_task(&rd.command_work_path, command_path, &rd.command_args)?;
            let handle = join_set.spawn(async move { task.run(child).await });
            abort_table.insert(handle.id(), task_id);
        } else {
            info!(
                status = "non_executable",
                command = *task.command,
                target = &rd.target_path,
                "task"
            );
            crr.unknowns.push(TargetRunResult {
                target: task.target.to_string(),
                code: None,
                error: Some("command not executable".to_string()),
                runtime_secs: None,
            });
        }
    } else {
        info!(
            status = "undefined",
            command = *task.command,
            target = &rd.target_path,
            "task"
        );
        let trr = TargetRunResult {
            target: task.target.to_string(),
            code: None,
            error: Some("command not found".to_string()),
            runtime_secs: None,
        };
        if fail_on_undefined {
            crr.failures.push(trr);
        } else {
            crr.unknowns.push(trr);
        }
    }
    Ok(())
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
        info!(
            num = run_data_group.datas.len(),
            command = command,
            "processing targets",
        );
        if cancelled {
            results.push(create_aborted_unknowns_result(
                command,
                &run_data_group.datas,
            ));
            continue;
        }
        let mut crr = CommandRunResult::new(command);

        let mut js = tokio::task::JoinSet::new();
        let token = sync::Arc::new(tokio_util::sync::CancellationToken::new());
        let mut abort_table = HashMap::new();
        let log_config = sync::Arc::new(cfg.log.clone());

        let (mut compressor, compressor_clients) = initialize_compressor(&run_data_group.datas, 2)?;
        let compressor_handle = thread::spawn(move || compressor.run());

        for (id, rd) in run_data_group.datas.iter().enumerate() {
            let target = sync::Arc::new(rd.target_path.clone());
            let command = sync::Arc::new(command.to_string());
            let clients = &compressor_clients[id];
            let ct = CommandTask {
                id,
                token: token.clone(),
                target,
                command,
                log_config: log_config.clone(),
                stdout_client: clients.0.clone(),
                stderr_client: clients.1.clone(),
                log_stream_client: log_stream_client.clone(),
                start_time: time::Instant::now(),
            };

            schedule_task(
                ct,
                rd,
                &mut js,
                &mut abort_table,
                &mut crr,
                fail_on_undefined,
            )
            .await?;
        }

        cancelled =
            process_task_results(js, run_data_group, &token, abort_table, command, &mut crr)
                .await?;

        for client in compressor_clients {
            client.0.shutdown()?;
            client.1.shutdown()?;
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
    run_data_groups: RunDataGroups,
    commands: &'a [&'a String],
    run_path: &path::Path,
    fail_on_undefined: bool,
    invocation: &'a str,
) -> Result<RunOutput, MonorailError> {
    info!(num = run_data_groups.groups.len(), "processing groups");

    let (results, failed) =
        process_run_data_groups(cfg, &run_data_groups, commands, fail_on_undefined).await?;

    let o = RunOutput {
        failed,
        invocation: invocation.to_owned(),
        results,
        out: run_data_groups.out,
    };

    store_run_output(&o, run_path)?;

    Ok(o)
}

// Serialize and store the compressed results of the given RunOutput.
fn store_run_output(run_output: &RunOutput, run_path: &path::Path) -> Result<(), MonorailError> {
    let run_result_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(run_path.join(result::RESULT_OUTPUT_FILE_NAME))
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
    fn new(run_path: &path::Path, command: &str, target_hash: &str) -> Result<Self, MonorailError> {
        let dir_path = run_path.join(command).join(target_hash);
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

    fn setup_handle_run_input<'a>(
        commands: Vec<&'a String>,
        targets: HashSet<&'a String>,
        args: Vec<&'a String>,
        arg_map: Option<&'a String>,
        arg_map_file: Option<&'a String>,
    ) -> HandleRunInput<'a> {
        HandleRunInput {
            git_opts: git::GitOptions::default(),
            commands,
            sequences: vec![],
            targets,
            args,
            arg_map,
            arg_map_file,
            include_deps: false,
            fail_on_undefined: false,
        }
    }

    const ARG_MAP_JSON: &str = r#"{
      "rust/crate1": {
        "build": [
          "--release"
        ]
      }
    }"#;

    #[test]
    fn test_arg_map_single_command_target() {
        let command = "build".to_string();
        let target = "rust/crate1".to_string();
        let arg1 = "--release".to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target]),
            vec![&arg1],
            None,
            None,
        );

        let arg_map = ArgMap::new(&input).expect("Expected valid ArgMap");
        let args = arg_map
            .get_args("rust/crate1", "build")
            .expect("Args not found");

        assert_eq!(args, &vec!["--release".to_string()]);
    }

    #[test]
    fn test_arg_map_args_multiple_commands_error() {
        let command1 = "build".to_string();
        let command2 = "test".to_string();
        let target = "rust/crate1".to_string();
        let arg = "--release".to_string();

        let input = setup_handle_run_input(
            vec![&command1, &command2],
            HashSet::from([&target]),
            vec![&arg],
            None,
            None,
        );

        let result = ArgMap::new(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_arg_map_args_multiple_targets_error() {
        let command = "build".to_string();
        let target1 = "rust/crate1".to_string();
        let target2 = "rust/crate2".to_string();
        let arg = "--release".to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target1, &target2]),
            vec![&arg],
            None,
            None,
        );

        let result = ArgMap::new(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_arg_map_valid_inline_json() {
        let json = ARG_MAP_JSON.to_string();
        let input = setup_handle_run_input(vec![], HashSet::new(), vec![], Some(&json), None);

        let arg_map = ArgMap::new(&input).expect("Expected valid ArgMap");
        let args = arg_map
            .get_args("rust/crate1", "build")
            .expect("Args not found");

        assert_eq!(args, &vec!["--release".to_string()]);
    }

    #[test]
    fn test_arg_map_invalid_inline_json() {
        let invalid_json = r#"{lkjsdf"#.to_string();
        let input =
            setup_handle_run_input(vec![], HashSet::new(), vec![], Some(&invalid_json), None);

        let result = ArgMap::new(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_arg_map_valid_file() {
        let td = tempdir().unwrap();
        let command = "build".to_string();
        let target = "rust/crate1".to_string();

        let path = td.path().join("test_arg_map.json");
        std::fs::write(&path, ARG_MAP_JSON).expect("Failed to write test file");
        let path_str = path.display().to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target]),
            vec![],
            None,
            Some(&path_str),
        );
        let arg_map = ArgMap::new(&input).expect("Expected valid ArgMap");

        let args = arg_map
            .get_args("rust/crate1", "build")
            .expect("Args not found");
        assert_eq!(args, &vec!["--release".to_string()]);
    }

    #[test]
    fn test_arg_map_invalid_file() {
        let td = tempdir().unwrap();
        let path = td.path().join("invalid_arg_map.json");
        std::fs::write(&path, r#"{lksjdfklj"#).expect("Failed to write test file");
        let path_str = path.display().to_string();

        let input = setup_handle_run_input(vec![], HashSet::new(), vec![], None, Some(&path_str));
        let result = ArgMap::new(&input);

        assert!(result.is_err());
    }

    #[test]
    fn test_arg_map_get_args_nonexistent() {
        let json = ARG_MAP_JSON.to_string();
        let input = setup_handle_run_input(vec![], HashSet::new(), vec![], Some(&json), None);

        let arg_map = ArgMap::new(&input).expect("Expected valid ArgMap");
        let args = arg_map.get_args("nonexistent_target", "nonexistent_command");

        assert!(args.is_none());
    }

    async fn prep_process_run_data_groups_test(
        cfg: &core::Config,
        work_path: &path::Path,
        commands: &[&String],
        target_groups: &[Vec<String>],
    ) -> RunDataGroups {
        let index = core::Index::new(cfg, &cfg.get_target_path_set(), work_path).unwrap();
        let run_path = work_path.join("run_path");
        let arg_map: ArgMap = ArgMap {
            table: HashMap::new(),
        };

        get_run_data_groups(
            &index,
            commands,
            &cfg.targets,
            target_groups,
            work_path,
            &run_path,
            &arg_map,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_process_run_data_groups() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let cfg = new_test_repo(work_path).await;
        let cmd1 = "cmd1".to_string();
        let target1 = "target1".to_string();
        let commands = vec![&cmd1];
        let target_groups = vec![vec![target1.clone()]];
        let run_data_groups =
            prep_process_run_data_groups_test(&cfg, work_path, &commands, &target_groups).await;
        let res = process_run_data_groups(&cfg, &run_data_groups, &commands, false).await;
        assert!(res.is_ok(), "Expected no error from processing groups");

        let rdg1 = &run_data_groups.groups[0];
        assert_eq!(commands[rdg1.command_index], &cmd1);

        let rd1 = &rdg1.datas[0];

        let (results, cancelled) = res.unwrap();
        assert!(!cancelled);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results,
            vec![CommandRunResult {
                command: cmd1,
                successes: vec![TargetRunResult {
                    target: rd1.target_path.clone(),
                    code: Some(0),
                    error: None,
                    runtime_secs: results[0].successes[0].runtime_secs
                }],
                failures: vec![],
                unknowns: vec![]
            }]
        );
    }

    #[tokio::test]
    async fn test_get_run_data_groups() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let cmd1 = "cmd4".to_string();
        let commands = vec![&cmd1];
        let target_groups = vec![vec!["target1".to_string()]];
        let run_path = work_path.join("run_path");
        let arg_map: ArgMap = ArgMap {
            table: HashMap::new(),
        };

        let result = get_run_data_groups(
            &index,
            &commands,
            &c.targets,
            &target_groups,
            work_path,
            &run_path,
            &arg_map,
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
        let run_path = temp_dir.path();

        let run_output = RunOutput {
            failed: false,
            invocation: "example args".to_string(),
            out: Out::new(&run_path),
            results: vec![
                CommandRunResult {
                    command: "build".to_string(),
                    successes: vec![TargetRunResult {
                        target: "target1".to_string(),
                        code: Some(0),
                        error: None,
                        runtime_secs: Some(12.3),
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
                        error: Some("Compilation error".to_string()),
                        runtime_secs: Some(5.5),
                    }],
                    unknowns: vec![],
                },
            ],
        };

        let result = store_run_output(&run_output, run_path);
        assert!(result.is_ok(), "store_run_output should succeed");

        let result_file_path = run_path.join(result::RESULT_OUTPUT_FILE_NAME);
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
