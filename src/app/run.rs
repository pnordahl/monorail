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
    pub(crate) argmap: Vec<&'a String>,
    pub(crate) argmap_file: Vec<&'a String>,
    pub(crate) include_deps: bool,
    pub(crate) fail_on_undefined: bool,
    pub(crate) use_base_argmaps: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OutRunFiles {
    result: String,
    stdout: String,
    stderr: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OutRun {
    path: String,
    files: OutRunFiles,
    targets: HashMap<String, String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Out {
    run: OutRun,
}
impl Out {
    fn new(run_path: &path::Path) -> Self {
        Self {
            run: OutRun {
                path: run_path.display().to_string(),
                files: OutRunFiles {
                    result: result::RESULT_OUTPUT_FILE_NAME.to_string(),
                    stdout: log::STDOUT_FILE.to_string(),
                    stderr: log::STDERR_FILE.to_string(),
                },
                targets: HashMap::new(),
            },
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) enum RunStatus {
    // task created
    #[serde(rename = "scheduled")]
    Scheduled,
    // non-zero exit code, successful completion
    #[serde(rename = "success")]
    Success,
    // command lacks executable permission
    #[serde(rename = "not_executable")]
    NotExecutable,
    // command definition not found on disk
    #[serde(rename = "undefined")]
    Undefined,
    // command return non-zero exit code
    #[serde(rename = "error")]
    Error,
    // command task was cancelled
    #[serde(rename = "cancelled")]
    Cancelled,
    // command task panicked
    #[serde(rename = "panicked")]
    Panicked,
    // command task was not run
    #[serde(rename = "skipped")]
    Skipped,
}
impl RunStatus {
    fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Scheduled => "scheduled",
            RunStatus::Success => "success",
            RunStatus::NotExecutable => "not_executable",
            RunStatus::Undefined => "undefined",
            RunStatus::Error => "error",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Panicked => "panicked",
            RunStatus::Skipped => "skipped",
        }
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct TargetRunResult {
    status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_secs: Option<f32>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CommandRunResult {
    command: String,
    target_groups: Vec<HashMap<String, TargetRunResult>>,
}
impl CommandRunResult {
    fn new(command: &str) -> Self {
        Self {
            command: command.to_string(),
            target_groups: vec![],
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RunOutput {
    pub(crate) failed: bool,
    invocation: String,
    out: Out,
    results: Vec<CommandRunResult>,
}

#[derive(Debug, Deserialize, Default)]
struct ArgMap {
    table: HashMap<String, HashMap<String, Vec<String>>>,
}
impl ArgMap {
    fn new() -> Self {
        Self {
            table: HashMap::new(),
        }
    }
    fn merge(&mut self, src: HashMap<String, HashMap<String, Vec<String>>>) {
        for (src_target, src_commands) in src {
            let self_commands = self.table.entry(src_target).or_default();
            for (src_command, mut src_args) in src_commands {
                self_commands
                    .entry(src_command)
                    .or_default()
                    .append(&mut src_args);
            }
        }
    }
    // Loads an argmap containing only commands. Specifically useful for a target's
    // base argmap, which defines only command -> [arg].
    fn merge_target_commands(&mut self, target: &str, p: &path::Path) -> Result<(), MonorailError> {
        let f = fs::File::open(p).map_err(MonorailError::from)?;
        let br = io::BufReader::new(f);
        let src = serde_json::from_reader(br).map_err(|e| {
            MonorailError::Generic(format!(
                "File arg map at {} contains invalid JSON; {}",
                p.display(),
                e
            ))
        })?;
        self.merge(HashMap::from([(target.to_string(), src)]));
        Ok(())
    }
    fn merge_file(&mut self, p: &path::Path) -> Result<(), MonorailError> {
        let f = fs::File::open(p).map_err(MonorailError::from)?;
        let br = io::BufReader::new(f);
        let src = serde_json::from_reader(br).map_err(|e| {
            MonorailError::Generic(format!(
                "File arg map at {} contains invalid JSON; {}",
                p.display(),
                e
            ))
        })?;
        self.merge(src);
        Ok(())
    }
    fn merge_run_input(&mut self, input: &HandleRunInput) -> Result<(), MonorailError> {
        // first process files
        for f in &input.argmap_file {
            self.merge_file(path::Path::new(f))?;
        }

        // next, argmap literals
        for s in &input.argmap {
            let src: HashMap<String, HashMap<String, Vec<String>>> = serde_json::from_str(s)
                .map_err(|e| {
                    MonorailError::Generic(format!("Inline arg map contains invalid JSON; {}", e))
                })?;
            self.merge(src);
        }

        // finally, args
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
            let src = HashMap::from([(
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
            )]);
            self.merge(src)
        }
        Ok(())
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
    let mut argmap = ArgMap::new();

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
            if input.use_base_argmaps {
                for t in ao.targets.iter() {
                    let base =
                        cfg.targets[*index.get_target_index(t)?].get_argmap_base_path(work_path);
                    if file::exists(&base).await {
                        argmap.merge_target_commands(t, &base)?;
                    }
                }
            }
            (index, target_groups)
        }
        _ => {
            let mut index = core::Index::new(cfg, &input.targets, work_path)?;
            let target_groups = if input.include_deps {
                let ai = analyze::AnalyzeInput::new(false, false, true);
                let ao = analyze::analyze(&ai, &mut index, None)?;
                if input.use_base_argmaps {
                    for t in ao.targets.iter() {
                        let base = cfg.targets[*index.get_target_index(t)?]
                            .get_argmap_base_path(work_path);
                        if file::exists(&base).await {
                            argmap.merge_target_commands(t, &base)?;
                        }
                    }
                }

                ao.target_groups
                    .ok_or(MonorailError::from("No target groups found"))?
            } else {
                // since the user specified the targets they want, without deps,
                // we will make synthetic serialized length 1 groups that ignore the graph
                let mut tg = vec![];
                for t in input.targets.iter() {
                    if input.use_base_argmaps {
                        let base = cfg.targets[*index.get_target_index(t)?]
                            .get_argmap_base_path(work_path);
                        if file::exists(&base).await {
                            argmap.merge_target_commands(t, &base)?;
                        }
                    }
                    tg.push(vec![t.to_string()]);
                }
                debug!("Synthesized target groups");
                tg
            };
            (index, target_groups)
        }
    };

    argmap.merge_run_input(input)?;

    let plan = get_plan(
        &index,
        &commands,
        &cfg.targets,
        &target_groups,
        work_path,
        &run_path,
        &argmap,
    )?;

    let run_output = run_internal(
        cfg,
        plan,
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
struct PlanTarget {
    path: String,
    command_work_path: path::PathBuf,
    command_path: Option<path::PathBuf>,
    command_args: Option<Vec<String>>,
    #[serde(skip)]
    logs: Logs,
}
#[derive(Debug, Serialize)]
struct PlanCommandTargetGroup {
    #[serde(skip)]
    command_index: usize,
    target_groups: Vec<Vec<PlanTarget>>,
}

#[derive(Debug, Serialize)]
struct Plan {
    command_target_groups: Vec<PlanCommandTargetGroup>,
    out: Out,
}

// Builds the execution plan that will be used when spawning tasks.
fn get_plan<'a>(
    index: &core::Index<'_>,
    commands: &'a [&'a String],
    targets: &[Target],
    target_groups: &[Vec<String>],
    work_path: &path::Path,
    run_path: &path::Path,
    argmap: &ArgMap,
) -> Result<Plan, MonorailError> {
    let mut out = Out::new(run_path);

    // for converting potentially deep nested paths into a single directory string
    let mut hasher = sha2::Sha256::new();
    let mut command_target_groups = Vec::with_capacity(commands.len());
    for (i, cmd) in commands.iter().enumerate() {
        let mut plan_target_groups = Vec::with_capacity(target_groups.len());
        for group in target_groups {
            let mut plan_targets = Vec::with_capacity(group.len());
            for target_path in group {
                hasher.update(target_path);
                let target_hash = format!("{:x}", hasher.finalize_reset());
                out.run
                    .targets
                    .insert(target_path.to_string(), target_hash.clone());
                let logs = Logs::new(run_path, cmd.as_str(), &target_hash)?;
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
                let command_path = match &tar.commands.definitions {
                    Some(definitions) => match definitions.get(cmd.as_str()) {
                        Some(def) => {
                            let app_target_command = target::AppTargetCommand::new(
                                cmd,
                                Some(def),
                                &commands_path,
                                work_path,
                            );
                            app_target_command.path
                        }
                        None => file::find_file_by_stem(cmd, &commands_path),
                    },
                    None => file::find_file_by_stem(cmd, &commands_path),
                };
                plan_targets.push(PlanTarget {
                    path: target_path.to_owned(),
                    command_work_path: work_path.join(target_path),
                    command_path,
                    command_args: argmap.get_args(&tar.path, cmd).map(|x| x.to_vec()),
                    logs,
                });
            }
            plan_target_groups.push(plan_targets);
        }
        command_target_groups.push(PlanCommandTargetGroup {
            command_index: i,
            target_groups: plan_target_groups,
        });
    }

    Ok(Plan {
        command_target_groups,
        out,
    })
}

pub(crate) fn spawn_task(
    command_work_path: &path::Path,
    command_path: &path::Path,
    command_args: &Option<Vec<String>>,
) -> Result<tokio::process::Child, MonorailError> {
    debug!(
        command_path = command_path.display().to_string(),
        command_args = command_args.clone().unwrap_or_default().join(" "),
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
    status: RunStatus,
    error: Option<String>,
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
                        status: RunStatus::Error,
                        error: Some(e.to_string()),
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
                        status: RunStatus::Error,
                        error: Some(e.to_string()),
                    })?,
            ),
            self.stderr_client.clone(),
            stderr_header,
            stderr_log_stream_client,
            self.token.clone(),
        );
        let child_fut = async { child.wait().await.map_err(MonorailError::from) };

        // todo; cancellation future
        let (_stdout_result, _stderr_result, child_result) =
            tokio::try_join!(stdout_fut, stderr_fut, child_fut).map_err(|e| {
                CommandTaskCancelInfo {
                    id: self.id,
                    elapsed: self.start_time.elapsed(),
                    status: RunStatus::Error,
                    error: Some(e.to_string()),
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

fn create_skipped_result(command: &str, target_groups: &[Vec<PlanTarget>]) -> CommandRunResult {
    let mut crr = CommandRunResult::new(command);
    for plan_targets in target_groups.iter() {
        let mut target_group = HashMap::new();
        for plan_target in plan_targets {
            let status = RunStatus::Skipped;
            error!(
                status = status.as_str(),
                command = command,
                target = &plan_target.path
            );
            target_group.insert(
                plan_target.path.to_string(),
                TargetRunResult {
                    status,
                    code: None,
                    runtime_secs: None,
                },
            );
        }
        crr.target_groups.push(target_group);
    }
    crr
}

async fn process_task_results(
    mut js: tokio::task::JoinSet<Result<CommandTaskFinishInfo, CommandTaskCancelInfo>>,
    target_group: &[PlanTarget],
    token: &sync::Arc<tokio_util::sync::CancellationToken>,
    abort_table: HashMap<tokio::task::Id, usize>,
    command: &str,
    result_target_group: &mut HashMap<String, TargetRunResult>,
) -> Result<bool, MonorailError> {
    let mut failed = false;
    while let Some(join_res) = js.join_next().await {
        match join_res {
            Ok(task_res) => {
                match task_res {
                    Ok(info) => {
                        let plan_target = &target_group[info.id];
                        if info.status.success() {
                            let status = RunStatus::Success;
                            info!(
                                status = status.as_str(),
                                command = command,
                                target = &plan_target.path,
                                "Task"
                            );
                            result_target_group.insert(
                                plan_target.path.to_string(),
                                TargetRunResult {
                                    status,
                                    code: info.status.code(),
                                    runtime_secs: Some(info.elapsed.as_secs_f32()),
                                },
                            );
                        } else {
                            // TODO: --cancel-on-error option
                            token.cancel();
                            failed = true;
                            let status = RunStatus::Error;
                            error!(
                                status = status.as_str(),
                                command = command,
                                target = &plan_target.path,
                                "Task"
                            );
                            result_target_group.insert(
                                plan_target.path.to_string(),
                                TargetRunResult {
                                    status,
                                    code: info.status.code(),
                                    runtime_secs: Some(info.elapsed.as_secs_f32()),
                                },
                            );
                        }
                    }
                    Err(info) => {
                        // TODO: --cancel-on-error option
                        token.cancel();
                        failed = true;

                        let plan_target = &target_group[info.id];
                        error!(
                            status = info.status.as_str(),
                            command = command,
                            target = &plan_target.path,
                            error = &info.error,
                            "Task"
                        );
                        result_target_group.insert(
                            plan_target.path.to_string(),
                            TargetRunResult {
                                status: info.status,
                                code: None,
                                runtime_secs: Some(info.elapsed.as_secs_f32()),
                            },
                        );
                    }
                }
            }
            Err(e) => {
                if e.is_cancelled() {
                    let run_data_index = abort_table
                        .get(&e.id())
                        .ok_or(MonorailError::from("Task id missing from abort table"))?;
                    let plan_target = &target_group[*run_data_index];
                    let status = RunStatus::Cancelled;
                    error!(
                        status = status.as_str(),
                        command = command,
                        target = &plan_target.path,
                        "Task"
                    );

                    result_target_group.insert(
                        plan_target.path.to_string(),
                        TargetRunResult {
                            status,
                            code: None,
                            runtime_secs: None,
                        },
                    );
                }
            }
        }
    }
    Ok(failed)
}

// Prepare the compressor ahead of time so that we can run it before spawning tasks.
// While this involves a second iteration of the RunData slice, this is necessary to
// avoid potentially blocking chatty tasks behind running the compressor.
fn initialize_compressor(
    plan_targets: &[PlanTarget],
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

    for plan_target in plan_targets.iter() {
        let stdout_client = compressor.register(&plan_target.logs.stdout_path)?;
        let stderr_client = compressor.register(&plan_target.logs.stderr_path)?;
        clients.push((stdout_client, stderr_client));
    }
    Ok((compressor, clients))
}

async fn schedule_task(
    mut task: CommandTask,
    plan_target: &PlanTarget,
    join_set: &mut tokio::task::JoinSet<Result<CommandTaskFinishInfo, CommandTaskCancelInfo>>,
    abort_table: &mut HashMap<tokio::task::Id, usize>,
    result_target_group: &mut HashMap<String, TargetRunResult>,
    fail_on_undefined: bool,
) -> Result<bool, MonorailError> {
    let mut failed = false;
    if let Some(command_path) = &plan_target.command_path {
        // check that the command path is executable before proceeding
        if file::is_executable(command_path) {
            let status = RunStatus::Scheduled;
            info!(
                status = status.as_str(),
                command = *task.command,
                target = &plan_target.path,
                "Task"
            );
            let task_id = task.id;
            let child = spawn_task(
                &plan_target.command_work_path,
                command_path,
                &plan_target.command_args,
            )?;
            let handle = join_set.spawn(async move { task.run(child).await });
            abort_table.insert(handle.id(), task_id);
        } else {
            let status = RunStatus::NotExecutable;
            info!(
                status = status.as_str(),
                command = *task.command,
                target = &plan_target.path,
                "Task"
            );
            result_target_group.insert(
                plan_target.path.to_string(),
                TargetRunResult {
                    status,
                    code: None,
                    runtime_secs: None,
                },
            );
            failed = true;
        }
    } else {
        let status = RunStatus::Undefined;
        info!(
            status = status.as_str(),
            command = *task.command,
            target = &plan_target.path,
            "Task"
        );
        result_target_group.insert(
            plan_target.path.to_string(),
            TargetRunResult {
                status,
                code: None,
                runtime_secs: None,
            },
        );
        if fail_on_undefined {
            failed = true;
        }
    }
    Ok(failed)
}

async fn process_plan(
    cfg: &core::Config,
    plan: &Plan,
    all_commands: &[&String],
    fail_on_undefined: bool,
) -> Result<(Vec<CommandRunResult>, bool), MonorailError> {
    // TODO: parameterize addr from cfg
    let log_stream_client = initialize_log_stream("127.0.0.1:9201").await;

    let mut results = Vec::new();
    let mut failed = false;

    info!(
        num = plan.command_target_groups.len(),
        "Processing commands"
    );

    for plan_command_target_group in &plan.command_target_groups {
        let command = &all_commands[plan_command_target_group.command_index];
        info!(
            num = plan_command_target_group.target_groups.len(),
            command = command,
            "Processing target groups",
        );
        if failed {
            results.push(create_skipped_result(
                command,
                &plan_command_target_group.target_groups,
            ));
            continue;
        }

        let log_config = sync::Arc::new(cfg.log.clone());
        let mut crr = CommandRunResult::new(command);

        for plan_targets in plan_command_target_group.target_groups.iter() {
            let token = sync::Arc::new(tokio_util::sync::CancellationToken::new());
            let mut abort_table = HashMap::new();
            let mut result_target_group = HashMap::new();
            let mut js = tokio::task::JoinSet::new();
            let (mut compressor, compressor_clients) = initialize_compressor(plan_targets, 2)?;
            let compressor_handle = thread::spawn(move || compressor.run());
            // schedule all plantargets for this command
            info!(
                num = plan_targets.len(),
                command = command,
                "Processing targets",
            );
            for (id, plan_target) in plan_targets.iter().enumerate() {
                let target = sync::Arc::new(plan_target.path.clone());
                if !failed {
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
                    if schedule_task(
                        ct,
                        plan_target,
                        &mut js,
                        &mut abort_table,
                        &mut result_target_group,
                        fail_on_undefined,
                    )
                    .await?
                    {
                        // prevent any additional tasks from being scheduled
                        failed = true;
                    }
                } else {
                    result_target_group.insert(
                        target.to_string(),
                        TargetRunResult {
                            status: RunStatus::Skipped,
                            code: None,
                            runtime_secs: None,
                        },
                    );
                }
            }
            if process_task_results(
                js,
                plan_targets,
                &token,
                abort_table,
                command,
                &mut result_target_group,
            )
            .await?
            {
                failed = true;
            }

            crr.target_groups.push(result_target_group);

            for client in compressor_clients {
                client.0.shutdown()?;
                client.1.shutdown()?;
            }
            // Unwrap for thread dyn Any panic contents, which isn't easily mapped to a MonorailError
            // because it doesn't impl Error; however, the internals of this handle do, so they
            // will get propagated.
            compressor_handle.join().unwrap()?;
        }
        results.push(crr);
    }

    Ok((results, failed))
}

#[allow(clippy::too_many_arguments)]
#[instrument]
async fn run_internal<'a>(
    cfg: &'a core::Config,
    plan: Plan,
    commands: &'a [&'a String],
    run_path: &path::Path,
    fail_on_undefined: bool,
    invocation: &'a str,
) -> Result<RunOutput, MonorailError> {
    info!("Processing plan");
    let (results, failed) = process_plan(cfg, &plan, commands, fail_on_undefined).await?;
    let o = RunOutput {
        failed,
        invocation: invocation.to_owned(),
        out: plan.out,
        results,
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
        argmap: Vec<&'a String>,
        argmap_file: Vec<&'a String>,
    ) -> HandleRunInput<'a> {
        HandleRunInput {
            git_opts: git::GitOptions::default(),
            commands,
            sequences: vec![],
            targets,
            args,
            argmap,
            argmap_file,
            include_deps: false,
            fail_on_undefined: false,
            use_base_argmaps: true,
        }
    }

    const ARGMAP_JSON: &str = r#"{
      "rust/crate1": {
        "build": [
          "--release"
        ]
      }
    }"#;

    const ARGMAP_JSON2: &str = r#"{
      "rust/crate1": {
        "build": [
          "--force"
        ]
      }
    }"#;

    // test_handle_run_cmd_error (failed == true)

    #[tokio::test]
    async fn test_handle_run_cmd_error() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let command = "cmd1".to_string();
        let t1 = "target1".to_string();
        let t2 = "target2".to_string();
        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&t1, &t2]),
            vec![],
            vec![],
            vec![],
        );

        let o = handle_run(&c, &input, "x", work_path).await.unwrap();
        assert!(o.failed);

        let crr = &o.results[0];
        assert_eq!(crr.command, "cmd1");
        for tg in &crr.target_groups {
            for (target, trr) in tg {
                if *target == t1 {
                    assert_eq!(trr.status, RunStatus::Error);
                    assert_eq!(trr.code, Some(1));
                    assert!(trr.runtime_secs.is_some());
                } else if *target == t2 {
                    assert_eq!(trr.status, RunStatus::Skipped);
                    assert_eq!(trr.code, None);
                    assert!(trr.runtime_secs.is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn test_handle_run() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let command = "cmd0".to_string();
        let input = setup_handle_run_input(vec![&command], HashSet::new(), vec![], vec![], vec![]);

        let o = handle_run(&c, &input, "x", work_path).await.unwrap();
        assert!(!o.failed);
        assert_eq!(o.invocation, "x");
        assert_eq!(
            o.out.run.targets,
            HashMap::from([
                (
                    "target6".to_string(),
                    "3c992f4174a270d2e9787c32535039e2e153e2649bc22d2dfef9dc36ab241d26".to_string()
                ),
                (
                    "target2".to_string(),
                    "241280caf7050c8d7e5ad3efb9760f91355431bb73bb260de2c225a86f634ee4".to_string()
                ),
                (
                    "target4".to_string(),
                    "7e3fb57bd1219911770bc429f87c2af1856c736d0bbb735bbb56736b52f0c35a".to_string()
                ),
                (
                    "target1".to_string(),
                    "348c5ac8a37a1212512d4366a084ba14e38aae3b78eedd4bc4b63c7b0af6c889".to_string()
                ),
                (
                    "target4/target5".to_string(),
                    "13ad70c6d0a981250ab7e6c54147f50f986bca583514c423fe8e610f5a63bf58".to_string()
                ),
                (
                    "target3".to_string(),
                    "89877681fadeeed65cc01a350cee399de305404ea931acf583c2342e9b937c3e".to_string()
                )
            ])
        );
        let crr = &o.results[0];
        assert_eq!(crr.command, "cmd0");
        for tg in &crr.target_groups {
            for (_, trr) in tg.iter() {
                assert_eq!(trr.status, RunStatus::Success);
                assert_eq!(trr.code, Some(0));
                assert!(trr.runtime_secs.is_some());
            }
        }
    }

    #[test]
    fn test_argmap_single_command_target_all() {
        let td = tempdir().unwrap();
        let command = "build".to_string();
        let target = "rust/crate1".to_string();
        let arg1 = "--plorp".to_string();
        let argmap = ARGMAP_JSON.to_string();

        let path = td.path().join("test_argmap.json");
        std::fs::write(&path, r#"{"rust/crate1":{"build":["--burp"]}}"#)
            .expect("Failed to write test file");
        let path_str = path.display().to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target]),
            vec![&arg1],
            vec![&argmap],
            vec![&path_str],
        );

        let mut argmap = ArgMap::new();
        argmap
            .merge_run_input(&input)
            .expect("Expected argmap merge to succeed");
        let args = argmap
            .get_args("rust/crate1", "build")
            .expect("Args not found");

        assert_eq!(
            args,
            &vec!["--burp".to_string(), "--release".to_string(), arg1]
        );
    }

    #[test]
    fn test_argmap_single_command_target() {
        let command = "build".to_string();
        let target = "rust/crate1".to_string();
        let arg1 = "--release".to_string();
        let arg2 = "--force".to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target]),
            vec![&arg1, &arg2],
            vec![],
            vec![],
        );

        let mut argmap = ArgMap::new();
        argmap
            .merge_run_input(&input)
            .expect("Expected argmap merge to succeed");
        let args = argmap
            .get_args("rust/crate1", "build")
            .expect("Args not found");

        assert_eq!(args, &vec![arg1, arg2]);
    }

    #[test]
    fn test_argmap_args_multiple_commands_error() {
        let command1 = "build".to_string();
        let command2 = "test".to_string();
        let target = "rust/crate1".to_string();
        let arg = "--release".to_string();

        let input = setup_handle_run_input(
            vec![&command1, &command2],
            HashSet::from([&target]),
            vec![&arg],
            vec![],
            vec![],
        );

        let mut argmap = ArgMap::new();
        let result = argmap.merge_run_input(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_argmap_args_multiple_targets_error() {
        let command = "build".to_string();
        let target1 = "rust/crate1".to_string();
        let target2 = "rust/crate2".to_string();
        let arg = "--release".to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target1, &target2]),
            vec![&arg],
            vec![],
            vec![],
        );

        let mut argmap = ArgMap::new();
        let result = argmap.merge_run_input(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_argmap_valid_inline_json() {
        let json = ARGMAP_JSON.to_string();
        let json2 = ARGMAP_JSON2.to_string();
        let input =
            setup_handle_run_input(vec![], HashSet::new(), vec![], vec![&json, &json2], vec![]);

        let mut argmap = ArgMap::new();
        argmap
            .merge_run_input(&input)
            .expect("Expected argmap merge to succeed");
        let args = argmap
            .get_args("rust/crate1", "build")
            .expect("Args not found");

        assert_eq!(args, &vec!["--release".to_string(), "--force".to_string()]);
    }

    #[test]
    fn test_argmap_invalid_inline_json() {
        let invalid_json = r#"{lkjsdf"#.to_string();
        let input =
            setup_handle_run_input(vec![], HashSet::new(), vec![], vec![&invalid_json], vec![]);

        let mut argmap = ArgMap::new();
        let result = argmap.merge_run_input(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_argmap_valid_file() {
        let td = tempdir().unwrap();
        let command = "build".to_string();
        let target = "rust/crate1".to_string();

        let path = td.path().join("test_argmap.json");
        std::fs::write(&path, ARGMAP_JSON).expect("Failed to write test file");
        let path_str = path.display().to_string();

        let path2 = td.path().join("test_argmap2.json");
        std::fs::write(&path2, ARGMAP_JSON2).expect("Failed to write test file");
        let path_str2 = path2.display().to_string();

        let input = setup_handle_run_input(
            vec![&command],
            HashSet::from([&target]),
            vec![],
            vec![],
            vec![&path_str, &path_str2],
        );
        let mut argmap = ArgMap::new();
        argmap
            .merge_run_input(&input)
            .expect("Expected argmap merge to succeed");

        let args = argmap
            .get_args("rust/crate1", "build")
            .expect("Args not found");
        assert_eq!(args, &vec!["--release".to_string(), "--force".to_string()]);
    }

    #[test]
    fn test_argmap_invalid_file() {
        let td = tempdir().unwrap();
        let path = td.path().join("invalid_argmap.json");
        std::fs::write(&path, r#"{lksjdfklj"#).expect("Failed to write test file");
        let path_str = path.display().to_string();

        let input = setup_handle_run_input(vec![], HashSet::new(), vec![], vec![], vec![&path_str]);
        let mut argmap = ArgMap::new();
        let result = argmap.merge_run_input(&input);

        assert!(result.is_err());
    }

    #[test]
    fn test_argmap_get_args_nonexistent() {
        let json = ARGMAP_JSON.to_string();
        let input = setup_handle_run_input(vec![], HashSet::new(), vec![], vec![&json], vec![]);

        let mut argmap = ArgMap::new();
        argmap
            .merge_run_input(&input)
            .expect("Expected argmap merge to succeed");
        let args = argmap.get_args("nonexistent_target", "nonexistent_command");

        assert!(args.is_none());
    }

    async fn prep_process_plan_test(
        cfg: &core::Config,
        work_path: &path::Path,
        commands: &[&String],
        target_groups: &[Vec<String>],
    ) -> Plan {
        let index = core::Index::new(cfg, &cfg.get_target_path_set(), work_path).unwrap();
        let run_path = work_path.join("run_path");
        let argmap: ArgMap = ArgMap {
            table: HashMap::new(),
        };

        get_plan(
            &index,
            commands,
            &cfg.targets,
            target_groups,
            work_path,
            &run_path,
            &argmap,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_process_plan() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let cfg = new_test_repo(work_path).await;
        let cmd1 = "cmd2".to_string();
        let target1 = "target1".to_string();
        let commands = vec![&cmd1];
        let target_groups = vec![vec![target1.clone()]];
        let plan = prep_process_plan_test(&cfg, work_path, &commands, &target_groups).await;
        let res = process_plan(&cfg, &plan, &commands, false).await;
        assert!(res.is_ok(), "Expected no error from processing groups");

        let ctg = &plan.command_target_groups[0];
        assert_eq!(commands[ctg.command_index], &cmd1);

        let target_group = &ctg.target_groups[0];

        let (results, cancelled) = res.unwrap();
        assert!(!cancelled);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results,
            vec![CommandRunResult {
                command: cmd1,
                target_groups: vec![HashMap::from([(
                    target_group[0].path.clone(),
                    TargetRunResult {
                        status: RunStatus::Success,
                        code: Some(0),
                        runtime_secs: results[0].target_groups[0]
                            .get("target1")
                            .unwrap()
                            .runtime_secs
                    }
                )])]
            }]
        );
    }

    #[tokio::test]
    async fn test_get_plan() {
        let td = tempdir().unwrap();
        let work_path = &td.path();
        let c = new_test_repo(work_path).await;
        let index = core::Index::new(&c, &c.get_target_path_set(), work_path).unwrap();
        let cmd1 = "cmd4".to_string();
        let commands = vec![&cmd1];
        let target_groups = vec![vec!["target1".to_string()]];
        let run_path = work_path.join("run_path");
        let argmap: ArgMap = ArgMap {
            table: HashMap::new(),
        };

        let result = get_plan(
            &index,
            &commands,
            &c.targets,
            &target_groups,
            work_path,
            &run_path,
            &argmap,
        );
        assert!(result.is_ok(), "get_plan returned an error");

        let plan = result.unwrap();
        assert_eq!(plan.command_target_groups.len(), 1);
        assert_eq!(plan.command_target_groups[0].target_groups.len(), 1);

        let plan_target = &plan.command_target_groups[0].target_groups[0][0];
        assert_eq!(plan_target.path, "target1");
        assert!(plan_target.command_path.is_some());
        assert_eq!(plan_target.command_args, None);
    }

    #[test]
    fn test_store_run_output_success() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let run_path = temp_dir.path();

        let run_output = RunOutput {
            failed: false,
            invocation: "example args".to_string(),
            out: Out::new(run_path),
            results: vec![
                CommandRunResult {
                    command: "build".to_string(),
                    target_groups: vec![HashMap::from([(
                        "target1".to_string(),
                        TargetRunResult {
                            code: Some(0),
                            status: RunStatus::Success,
                            runtime_secs: Some(12.3),
                        },
                    )])],
                },
                CommandRunResult {
                    command: "test".to_string(),
                    target_groups: vec![HashMap::from([(
                        "target2".to_string(),
                        TargetRunResult {
                            code: Some(1),
                            status: RunStatus::Error,
                            runtime_secs: Some(5.5),
                        },
                    )])],
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
            run_output.results.len(),
            deserialized_output.results.len(),
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
