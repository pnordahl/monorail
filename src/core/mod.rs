mod graph;
mod tracking;

use std::cmp::Ordering;
use std::{path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use trie_rs::{Trie, TrieBuilder};

use crate::common::error::{GraphError, MonorailError};

const RUN_OUTPUT_FILE_NAME: &str = "run.json.gz";
const STDOUT_FILE: &str = "stdout.gz";
const STDERR_FILE: &str = "stderr.gz";

#[derive(Debug)]
pub struct GitOptions<'a> {
    pub(crate) start: Option<&'a str>,
    pub(crate) end: Option<&'a str>,
    pub(crate) git_path: &'a str,
}

#[derive(Debug)]
pub struct RunInput<'a> {
    pub(crate) git_opts: GitOptions<'a>,
    pub(crate) functions: Vec<&'a String>,
    pub(crate) targets: HashSet<&'a String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunOutput {
    pub(crate) failed: bool,
    results: Vec<FunctionRunResult>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FunctionRunResult {
    function: String,
    successes: Vec<TargetRunResult>,
    failures: Vec<TargetRunResult>,
    unknowns: Vec<TargetRunResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TargetRunResult {
    target: String,
    code: Option<i32>,
    stdout_path: path::PathBuf,
    stderr_path: path::PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    runtime_secs: f32,
}

// Open a file from the provided path, and compute its checksum
// TODO: allow hasher to be passed in?
// TODO: configure buffer size based on file size?
// TODO: pass in open file instead of path?
async fn get_file_checksum(path: &path::Path) -> Result<String, MonorailError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = sha2::Sha256::new();

    let mut buffer = [0u8; 64 * 1024];
    loop {
        let num = file.read(&mut buffer).await?;
        if num == 0 {
            break;
        }
        hasher.update(&buffer[..num]);
    }

    Ok(hex::encode(hasher.finalize()).to_string())
}

async fn checksum_is_equal(
    pending: &HashMap<String, String>,
    work_dir: &path::Path,
    name: &str,
) -> bool {
    match pending.get(name) {
        Some(checksum) => {
            // compute checksum of x.name and check not equal
            get_file_checksum(&work_dir.join(name))
                .await
                // Note that any error here is ignored; the reason for this is that checkpoints
                // can reference stale or deleted paths, and it's not in our best interest to
                // short-circuit change detection when this occurs.
                .map_or(false, |x_checksum| checksum == &x_checksum)
        }
        None => false,
    }
}

async fn get_filtered_changes(
    changes: Vec<Change>,
    pending: &HashMap<String, String>,
    work_dir: &path::Path,
) -> Vec<Change> {
    tokio_stream::iter(changes)
        .then(|x| async {
            if checksum_is_equal(pending, work_dir, &x.name).await {
                None
            } else {
                Some(x)
            }
        })
        .filter_map(|x| x)
        .collect()
        .await
}

// Fetch diff changes, using the tracking checkpoint commit if present.
async fn get_git_diff_changes<'a>(
    git_opts: &'a GitOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    work_dir: &path::Path,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let start = git_opts.start.or_else(|| {
        // otherwise, check checkpoint.commit; if provided, use that
        if let Some(checkpoint) = checkpoint {
            if checkpoint.commit.is_empty() {
                None
            } else {
                Some(&checkpoint.commit)
            }
        } else {
            // if not then there's nowhere to start from, and we're done
            None
        }
    });

    let end = match start {
        Some(_) => git_opts.end,
        None => None,
    };

    let diff_changes = git_cmd_diff_changes(git_opts.git_path, work_dir, start, end).await?;
    if start.is_none() && end.is_none() && diff_changes.is_empty() {
        // no pending changes and diff range is ok, but signficant in that it
        // means change detection is impossible and other processes should consider
        // all targets changed
        Ok(None)
    } else {
        Ok(Some(diff_changes))
    }
}

async fn get_git_all_changes<'a>(
    git_opts: &'a GitOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    work_dir: &path::Path,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let diff_changes = get_git_diff_changes(git_opts, checkpoint, work_dir).await?;
    let mut other_changes = git_cmd_other_changes(git_opts.git_path, work_dir).await?;
    if let Some(diff_changes) = diff_changes {
        other_changes.extend(diff_changes);
    }
    let mut filtered_changes = match checkpoint {
        Some(checkpoint) => {
            if let Some(pending) = &checkpoint.pending {
                if !pending.is_empty() {
                    get_filtered_changes(other_changes, pending, work_dir).await
                } else {
                    other_changes
                }
            } else {
                other_changes
            }
        }
        None => {
            // no changes and no checkpoint means change detection is not possible
            if other_changes.is_empty() {
                return Ok(None);
            }
            other_changes
        }
    };

    filtered_changes.sort();
    Ok(Some(filtered_changes))
}

#[derive(Serialize)]
pub(crate) struct LogShowInput<'a> {
    pub(crate) should_tail: bool,
    pub(crate) id: Option<&'a usize>,
    pub(crate) functions: HashSet<&'a str>,
    pub(crate) targets: HashSet<&'a str>,
    pub(crate) include_stdout: bool,
    pub(crate) include_stderr: bool,
}
// Probably nothing in here, but maybe some analytics about the logs?
#[derive(Serialize)]
pub(crate) struct LogShowOutput {}

pub async fn handle_log_show<'a>(
    cfg: &'a Config,
    input: &'a LogShowInput<'a>,
    work_dir: &'a path::Path,
) -> Result<(), MonorailError> {
    let log_id = match input.id {
        Some(id) => *id,
        None => {
            let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
            let mut log_info = tracking_table.open_log_info().await?;
            log_info.id
        }
    };

    let log_dir = cfg.get_log_path(work_dir).join(format!("{}", log_id));
    if !log_dir.try_exists()? {
        return Err(MonorailError::Generic(format!(
            "Log path {} does not exist",
            &log_dir.display().to_string()
        )));
    }

    // map all targets to their shas for filtering and prefixing log lines
    let targets = cfg.targets.as_ref().ok_or(MonorailError::from(
        "No configured targets, cannot tail logs",
    ))?;
    let mut hasher = sha2::Sha256::new();
    let mut hash2target = HashMap::new();
    for target in targets {
        hasher.update(&target.path);
        hash2target.insert(format!("{:x}", hasher.finalize_reset()), &target.path);
    }

    let stdout = tokio::io::stdout();
    let stdout_mutex = tokio::sync::Mutex::new(stdout);
    let stdout_arc = sync::Arc::new(stdout_mutex);

    // open directory at log_dir
    let mut js = tokio::task::JoinSet::new();
    let filter_functions = !input.functions.is_empty();
    let filter_targets = !input.targets.is_empty();
    for fn_entry in log_dir.read_dir()? {
        let fn_path = fn_entry?.path();
        if fn_path.is_dir() {
            let function = fn_path.file_name().unwrap().to_str().unwrap();
            if !filter_functions || input.functions.contains(&function) {
                for t_entry in fn_path.read_dir()? {
                    let t_path = t_entry?.path();
                    if t_path.is_dir() {
                        let target_hash = t_path.file_name().unwrap().to_str().unwrap();
                        if !filter_targets || input.targets.contains(&target_hash) {
                            for gz in t_path.read_dir()? {
                                let target =
                                    hash2target.get(target_hash).ok_or(MonorailError::Generic(
                                        format!("Target not found for {}", &target_hash),
                                    ))?;
                                let gz_path = gz?.path();
                                let stdout_arc2 = stdout_arc.clone();
                                let function2 = function.to_owned();
                                let target2 = target.to_string();
                                // todo; color requires --ansi-256
                                let color = format!("\x1b[38;5;{}m", target_hash.as_bytes()[0]);
                                // todo; stderr/stdout filter
                                js.spawn(async move {
                                    stream_gzip_file_to_stdout(
                                        target2,
                                        function2,
                                        color,
                                        gz_path.to_path_buf(),
                                        stdout_arc2,
                                    )
                                    .await
                                    .unwrap();
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    while let Some(join_res) = js.join_next().await {
        match join_res {
            Ok(task_res) => {
                // dbg!(task_res);
            }
            Err(e) => {
                dbg!(e);
            }
        }
    }

    Ok(())
}

async fn stream_gzip_file_to_stdout(
    target: String,
    function: String,
    color: String,
    path: path::PathBuf,
    stdout_mutex: sync::Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
) -> tokio::io::Result<()> {
    let filename = path.file_name().unwrap().to_str().unwrap(); // todo; MonorailError
    let file = tokio::fs::File::open(&path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut decoder = async_compression::tokio::bufread::GzipDecoder::new(reader);
    let mut line_reader = tokio::io::BufReader::new(decoder);
    let mut line: Vec<u8> = Vec::new();
    let reset = "\x1b[0m";
    let prefix = format!(
        "{}[monorail | {} | {} | {}]{} ",
        color, filename, target, function, reset
    );
    let prefix_bytes = prefix.as_bytes();
    while line_reader.read_until(b'\n', &mut line).await? > 0 {
        let mut stdout = stdout_mutex.lock().await;
        stdout.write_all(prefix_bytes).await?;
        stdout.write_all(&line).await?;
        line.clear();
    }

    Ok(())
}

// want an async task that can open a path as a tokio file to read, and pipe the file as it gets data to a writer

const OUTPUT_KIND_WORKFLOW: &str = "workflow";
const OUTPUT_KIND_RESULT: &str = "result";
const OUTPUT_KIND_ERROR: &str = "error";

#[derive(Serialize)]
pub(crate) struct Output<T: Serialize> {
    timestamp: String,
    kind: &'static str,
    data: T,
}
impl<T: Serialize> Output<T> {
    pub(crate) fn workflow(data: T) -> Self {
        Self {
            timestamp: Self::utc_now(),
            kind: OUTPUT_KIND_WORKFLOW,
            data,
        }
    }
    pub(crate) fn result(data: T) -> Self {
        Self {
            timestamp: Self::utc_now(),
            kind: OUTPUT_KIND_RESULT,
            data,
        }
    }
    pub(crate) fn error(data: T) -> Self {
        Self {
            timestamp: Self::utc_now(),
            kind: OUTPUT_KIND_ERROR,
            data,
        }
    }
    fn utc_now() -> String {
        chrono::Utc::now().to_rfc3339()
    }
}

struct Workflow<W: Write> {
    writer: W,
}
impl<W: Write> Workflow<W> {
    pub fn log<T: Serialize>(&mut self, data: T) -> Result<(), MonorailError> {
        serde_json::to_writer(&mut self.writer, &data).map_err(MonorailError::from)?;
        writeln!(&mut self.writer).map_err(MonorailError::from)
    }
}

pub async fn handle_run<'a>(
    cfg: &'a Config,
    input: &'a RunInput<'a>,
    work_dir: &'a path::Path,
) -> Result<RunOutput, MonorailError> {
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
    let targets = cfg
        .targets
        .as_ref()
        .ok_or(MonorailError::from("No configured targets"))?;

    match input.targets.len() {
        0 => {
            let ths = cfg.get_target_path_set();
            let mut lookups = Lookups::new(cfg, &ths, work_dir)?;
            let checkpoint = match tracking_table.open_checkpoint().await {
                Ok(checkpoint) => Some(checkpoint),
                Err(MonorailError::TrackingCheckpointNotFound(_)) => None,
                Err(e) => {
                    return Err(e);
                }
            };

            let changes = match cfg.vcs.r#use {
                VcsKind::Git => {
                    get_git_diff_changes(&input.git_opts, &checkpoint, work_dir).await?
                }
            };
            let ao = analyze_show(&mut lookups, changes, false, false, true)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            run(
                cfg,
                &tracking_table,
                &lookups,
                work_dir,
                &input.functions,
                targets.clone(),
                &target_groups,
            )
            .await
        }
        _ => {
            let mut lookups = Lookups::new(cfg, &input.targets, work_dir)?;
            let ao = analyze_show(&mut lookups, None, false, false, true)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            run(
                cfg,
                &tracking_table,
                &lookups,
                work_dir,
                &input.functions,
                targets.clone(),
                &target_groups,
            )
            .await
        }
    }
}

struct RunData {
    target_path: String,
    script_path: String,
    logs: Logs,
}
struct RunDataGroup {
    function_index: usize,
    datas: Vec<RunData>,
}
struct RunDataGroups {
    work_dir: path::PathBuf,
    groups: Vec<RunDataGroup>,
}

// Create an initial run output with unknowns, and build the RunData for each target group.
fn get_run_data_groups<'a>(
    lookups: &Lookups<'_>,
    functions: &'a [&'a String],
    targets: &[Target],
    target_groups: &[Vec<String>],
    work_dir: &path::Path,
    log_dir: &path::Path,
) -> Result<(RunOutput, RunDataGroups), MonorailError> {
    // for converting potentially deep nested paths into a single directory string
    let mut path_hasher = sha2::Sha256::new();

    let mut groups = Vec::with_capacity(target_groups.len());
    let mut frrs = Vec::with_capacity(target_groups.len());
    for (i, f) in functions.iter().enumerate() {
        for group in target_groups {
            let mut run_data = Vec::with_capacity(group.len());
            let mut unknowns = Vec::with_capacity(group.len());
            for target_path in group {
                let logs = Logs::open(log_dir, f.as_str(), &target_path, &mut path_hasher)?;
                unknowns.push(TargetRunResult {
                    target: target_path.to_owned(),
                    code: None,
                    stdout_path: logs.stdout_path.to_owned(),
                    stderr_path: logs.stderr_path.to_owned(),
                    error: None,
                    runtime_secs: 0.0,
                });
                let target_index = lookups
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
                let script_path = std::path::Path::new(&target_path)
                    .join(&target.run)
                    .to_str()
                    .ok_or(MonorailError::from("Run file not found"))?
                    .to_owned();
                run_data.push(RunData {
                    target_path: target_path.to_owned(),
                    script_path: script_path.to_owned(),
                    logs,
                });
            }
            groups.push(RunDataGroup {
                function_index: i,
                datas: run_data,
            });
            let frr = FunctionRunResult {
                function: f.to_string(),
                successes: vec![],
                failures: vec![],
                unknowns,
            };
            frrs.push(frr);
        }
    }

    Ok((
        RunOutput {
            failed: false,
            results: frrs,
        },
        RunDataGroups {
            work_dir: work_dir.to_path_buf(),
            groups,
        },
    ))
}

pub fn spawn_bash_task(
    function: &str,
    work_dir: &path::Path,
    script_path: &str,
) -> Result<tokio::process::Child, MonorailError> {
    tokio::process::Command::new("bash")
        .arg("-c")
        .arg(format!("source $(pwd)/{} && {}", script_path, function))
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // parallel execution makes use of stdin impractical
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(MonorailError::from)
}

struct FunctionTask {
    id: usize,
    // stdout_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    // stderr_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    start_time: time::Instant,
}
impl FunctionTask {
    async fn run(
        &mut self,
        mut child: tokio::process::Child,
        token: sync::Arc<tokio_util::sync::CancellationToken>,
        stdout_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
        stderr_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    ) -> Result<
        (usize, std::process::ExitStatus, time::Duration),
        (usize, time::Duration, MonorailError),
    > {
        let stdout_fut =
            process_reader3(child.stdout.take().unwrap(), stdout_encoder, token.clone());
        let stderr_fut =
            process_reader3(child.stderr.take().unwrap(), stderr_encoder, token.clone());
        let child_fut = async { child.wait().await.map_err(MonorailError::from) };

        let (stdout_result, stderr_result, child_result) =
            tokio::try_join!(stdout_fut, stderr_fut, child_fut)
                .map_err(|e| (self.id, self.start_time.elapsed(), MonorailError::from(e)))?;

        Ok((self.id, child_result, self.start_time.elapsed()))
    }
}

async fn process_reader3<R>(
    mut reader: R,
    mut encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    token: sync::Arc<tokio_util::sync::CancellationToken>,
) -> Result<(), MonorailError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0u8; 1024];
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                dbg!("shutdown log encoder");
                encoder.try_finish()?;
                return Err(MonorailError::TaskCancelled);
            }
            res = reader.read(&mut buffer) => {
                match res {
                    Ok(0) => { break; },
                    Ok(n) => {
                        encoder
                        .write_all(&buffer[..n])
                        .map_err(MonorailError::from)?;
                        encoder.flush()?;
                    }
                    Err(e) => return Err(MonorailError::from(e))
                }
            }
        }
    }

    encoder.try_finish()?;

    Ok(())
}

#[derive(Serialize)]
struct FunctionRunWorkflow<'a> {
    status: &'a str,
    function: &'a str,
    target: &'a str,
}

async fn run<'a>(
    cfg: &'a Config,
    tracking_table: &tracking::Table,
    lookups: &Lookups<'_>,
    work_dir: &path::Path,
    functions: &'a [&'a String],
    targets: Vec<Target>,
    target_groups: &[Vec<String>],
) -> Result<RunOutput, MonorailError> {
    let stdout = std::io::stdout();
    let writer = stdout.lock();
    let mut workflow = Workflow { writer };

    let mut o = RunOutput {
        failed: false,
        results: vec![],
    };
    let log_dir = {
        // obtain current log info counter and increment it before using
        let mut log_info = match tracking_table.open_log_info().await {
            Ok(log_info) => log_info,
            Err(MonorailError::TrackingLogInfoNotFound(_)) => tracking_table.new_log_info(),
            Err(e) => {
                return Err(e);
            }
        };
        if log_info.id >= cfg.max_retained_run_results {
            log_info.id = 0;
        }
        log_info.id += 1;

        log_info.save().await?;
        let log_dir = cfg.get_log_path(work_dir).join(format!("{}", log_info.id));
        // remove the log_dir path if it exists, and create a new one
        std::fs::remove_dir_all(&log_dir).unwrap_or(());
        std::fs::create_dir_all(&log_dir)?;
        log_dir
    };

    let (initial_run_output, run_data_groups) = get_run_data_groups(
        lookups,
        functions,
        &targets,
        target_groups,
        work_dir,
        &log_dir,
    )?;

    // Make initial run output available for use by other commands
    store_run_output(&initial_run_output, &log_dir)?;

    // let work_dir_arc = sync::Arc::new(run_data_groups.work_dir);
    let mut abort = false;

    // Spawn concurrent tasks for each group of rundata
    for mut run_data_group in run_data_groups.groups {
        if abort {
            let mut unknowns = vec![];
            let function = functions[run_data_group.function_index].to_owned();
            for rd in run_data_group.datas.iter_mut() {
                let mut stdout_encoder =
                    rd.logs
                        .stdout_encoder
                        .take()
                        .ok_or(MonorailError::Generic(format!(
                            "Failed to acquire encoder for {}",
                            &rd.logs.stdout_path.display().to_string()
                        )))?;
                let mut stderr_encoder =
                    rd.logs
                        .stderr_encoder
                        .take()
                        .ok_or(MonorailError::Generic(format!(
                            "Failed to acquire encoder for {}",
                            &rd.logs.stderr_path.display().to_string()
                        )))?;
                stdout_encoder.try_finish()?;
                stderr_encoder.try_finish()?;
                let target = rd.target_path.to_owned();
                workflow.log(Output::workflow(FunctionRunWorkflow {
                    status: "aborted",
                    function: &function,
                    target: &rd.target_path,
                }))?;
                unknowns.push(TargetRunResult {
                    target,
                    code: None,
                    stdout_path: rd.logs.stdout_path.to_owned(),
                    stderr_path: rd.logs.stderr_path.to_owned(),
                    error: Some("Function task cancelled".to_string()),
                    runtime_secs: 0.0,
                });
            }
            o.results.push(FunctionRunResult {
                function,
                successes: vec![],
                failures: vec![],
                unknowns,
            });
            continue;
        }
        let function = functions[run_data_group.function_index].to_owned();
        let mut frr = FunctionRunResult {
            function: function.to_owned(),
            successes: vec![],
            failures: vec![],
            unknowns: vec![],
        };
        // let function_arc = sync::Arc::new(function);
        let mut js = tokio::task::JoinSet::new();
        let token = sync::Arc::new(tokio_util::sync::CancellationToken::new());
        let mut abort_table = HashMap::new();
        for id in 0..run_data_group.datas.len() {
            let rd = &mut run_data_group.datas[id];
            let stdout_encoder =
                rd.logs
                    .stdout_encoder
                    .take()
                    .ok_or(MonorailError::Generic(format!(
                        "Failed to acquire encoder for {}",
                        &rd.logs.stdout_path.display().to_string()
                    )))?;
            let stderr_encoder =
                rd.logs
                    .stderr_encoder
                    .take()
                    .ok_or(MonorailError::Generic(format!(
                        "Failed to acquire encoder for {}",
                        &rd.logs.stderr_path.display().to_string()
                    )))?;
            let mut ft = FunctionTask {
                id,
                start_time: time::Instant::now(),
            };
            // TODO: check file type to pick command
            let mut child = spawn_bash_task(&function, &run_data_groups.work_dir, &rd.script_path)?;
            let token2 = token.clone();
            let handle = js
                .spawn(async move { ft.run(child, token2, stdout_encoder, stderr_encoder).await });
            // map this task for reference, in case it's aborted
            abort_table.insert(handle.id(), id);
        }
        while let Some(join_res) = js.join_next().await {
            match join_res {
                Ok(task_res) => {
                    match task_res {
                        Ok((id, status, duration)) => {
                            let rd = &run_data_group.datas[id];
                            let mut trr = TargetRunResult {
                                target: rd.target_path.to_owned(),
                                code: status.code(),
                                stdout_path: rd.logs.stdout_path.to_owned(),
                                stderr_path: rd.logs.stderr_path.to_owned(),
                                error: None,
                                runtime_secs: duration.as_secs_f32(),
                            };
                            if status.success() {
                                workflow.log(Output::workflow(FunctionRunWorkflow {
                                    status: "success",
                                    function: &function,
                                    target: &rd.target_path,
                                }))?;
                                frr.successes.push(trr);
                            } else {
                                // TODO: --abort-on-failure option
                                token.cancel();
                                // js.abort_all();
                                abort = true;
                                workflow.log(Output::workflow(FunctionRunWorkflow {
                                    status: "error",
                                    function: &function,
                                    target: &rd.target_path,
                                }))?;

                                trr.error = Some("Function returned an error".to_string());
                                frr.failures.push(trr);
                            }
                        }
                        Err((id, duration, e)) => {
                            // TODO: --abort-on-failure option
                            token.cancel();
                            // js.abort_all();
                            abort = true;

                            let (status, message) = match e {
                                MonorailError::TaskCancelled => {
                                    ("cancelled", "Function task cancelled".to_string())
                                }
                                _ => ("error", format!("Function task failed: {}", e)),
                            };

                            let rd = &run_data_group.datas[id];
                            workflow.log(Output::workflow(FunctionRunWorkflow {
                                status,
                                function: &function,
                                target: &rd.target_path,
                            }))?;
                            frr.failures.push(TargetRunResult {
                                target: rd.target_path.to_owned(),
                                code: None,
                                stdout_path: rd.logs.stdout_path.to_owned(),
                                stderr_path: rd.logs.stderr_path.to_owned(),
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
                        let rd = &mut run_data_group.datas[*run_data_index];
                        workflow.log(Output::workflow(FunctionRunWorkflow {
                            status: "aborted",
                            function: &function,
                            target: &rd.target_path,
                        }))?;
                        frr.unknowns.push(TargetRunResult {
                            target: rd.target_path.to_owned(),
                            code: None,
                            stdout_path: rd.logs.stdout_path.to_owned(),
                            stderr_path: rd.logs.stderr_path.to_owned(),
                            error: Some("Function task cancelled".to_string()),
                            runtime_secs: 0.0,
                        })
                    }
                }
            }
        }
        if !frr.failures.is_empty() || !frr.unknowns.is_empty() {
            o.failed = true;
        }
        o.results.push(frr);
    }

    // Update the run output with final results
    store_run_output(&o, &log_dir)?;

    Ok(o)
}

// Serialize and store the compressed results of the given RunOutput.
fn store_run_output(run_output: &RunOutput, log_dir: &path::Path) -> Result<(), MonorailError> {
    let run_result_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_dir.join(RUN_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let bw = BufWriter::new(run_result_file);
    let mut encoder = flate2::write::GzEncoder::new(bw, flate2::Compression::default());
    serde_json::to_writer(&mut encoder, run_output)?;
    encoder.try_finish().map_err(MonorailError::from)
}

async fn run_bash_task(
    id: usize,
    function: sync::Arc<String>,
    work_dir: sync::Arc<path::PathBuf>,
    script_path: String,
    stdout_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    stderr_encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
    start_time: time::Instant,
) -> Result<(usize, std::process::ExitStatus, time::Duration), (usize, time::Duration, MonorailError)>
{
    let mut child = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(format!("source $(pwd)/{} && {}", script_path, function))
        .current_dir(work_dir.as_path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // parallel execution makes use of stdin impractical
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| (id, start_time.elapsed(), MonorailError::from(e)))?;

    let stdout_fut = {
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(async move { process_reader(stdout, stdout_encoder).await })
        } else {
            tokio::spawn(async { Ok(()) })
        }
    };

    let stderr_fut = {
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move { process_reader(stderr, stderr_encoder).await })
        } else {
            tokio::spawn(async { Ok(()) })
        }
    };
    let child_fut = tokio::spawn(async move { child.wait().await.map_err(MonorailError::from) });

    let (stdout_result, stderr_result, child_result) =
        tokio::try_join!(stdout_fut, stderr_fut, child_fut)
            .map_err(|e| (id, start_time.elapsed(), MonorailError::from(e)))?;

    stdout_result.map_err(|e| (id, start_time.elapsed(), e))?;
    stderr_result.map_err(|e| (id, start_time.elapsed(), e))?;
    let status = child_result.map_err(|e| (id, start_time.elapsed(), e))?;

    Ok((id, status, start_time.elapsed()))
}

async fn process_reader2<R>(
    mut reader: R,
    encoder: &mut flate2::write::GzEncoder<BufWriter<std::fs::File>>,
) -> Result<(), MonorailError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0u8; 1024];
    while let Ok(bytes_read) = reader.read(&mut buffer).await {
        if bytes_read == 0 {
            break;
        }
        encoder
            .write_all(&buffer[..bytes_read])
            .map_err(MonorailError::from)?;
        encoder.flush()?;
    }

    Ok(())
}

async fn process_reader<R>(
    mut reader: R,
    mut encoder: flate2::write::GzEncoder<BufWriter<std::fs::File>>,
) -> Result<(), MonorailError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0u8; 1024];
    while let Ok(bytes_read) = reader.read(&mut buffer).await {
        if bytes_read == 0 {
            break;
        }
        encoder
            .write_all(&buffer[..bytes_read])
            .map_err(MonorailError::from)?;
        encoder.flush()?;
    }

    Ok(())
}

fn finalize_log(
    encoder: &mut flate2::write::GzEncoder<BufWriter<std::fs::File>>,
) -> Result<(), MonorailError> {
    encoder
        .write_all("[monorail] Log ends\n".as_bytes())
        .map_err(MonorailError::from)?;
    encoder.try_finish()?;
    Ok(())
}

pub struct Logs {
    stdout_path: path::PathBuf,
    stderr_path: path::PathBuf,
    stdout_encoder: Option<flate2::write::GzEncoder<BufWriter<std::fs::File>>>,
    stderr_encoder: Option<flate2::write::GzEncoder<BufWriter<std::fs::File>>>,
}
impl Logs {
    fn open(
        log_dir: &path::Path,
        function: &str,
        target_path: &str,
        hasher: &mut sha2::Sha256,
    ) -> Result<Self, MonorailError> {
        let log_preamble = format!("[monorail] Log begins - {}: {}\n", target_path, function);
        hasher.update(target_path);
        let dir_path = log_dir
            .join(function)
            .join(&format!("{:x}", hasher.finalize_reset()));
        let stdout_path = dir_path.clone().join(STDOUT_FILE);
        let stderr_path = dir_path.clone().join(STDERR_FILE);
        std::fs::create_dir_all(&dir_path)?;
        let stdout_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stdout_path)
            .map_err(|e| MonorailError::Generic(e.to_string()))?;
        let stdout_bw = BufWriter::new(stdout_file);
        let mut stdout_encoder =
            flate2::write::GzEncoder::new(stdout_bw, flate2::Compression::default());
        stdout_encoder
            .write_all(log_preamble.as_bytes())
            .map_err(MonorailError::from)?;
        stdout_encoder.flush()?;
        let stderr_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stderr_path)
            .map_err(|e| MonorailError::Generic(e.to_string()))?;
        let stderr_bw = BufWriter::new(stderr_file);
        let mut stderr_encoder =
            flate2::write::GzEncoder::new(stderr_bw, flate2::Compression::default());
        stderr_encoder
            .write_all(log_preamble.as_bytes())
            .map_err(MonorailError::from)?;
        stderr_encoder.flush()?;
        Ok(Self {
            stdout_path,
            stderr_path,
            stdout_encoder: Some(stdout_encoder),
            stderr_encoder: Some(stderr_encoder),
        })
    }
}

#[derive(Debug)]
pub struct HandleResultShowInput {}

pub async fn handle_result_show<'a>(
    cfg: &'a Config,
    _input: &'a HandleResultShowInput,
    work_dir: &'a path::Path,
) -> Result<RunOutput, MonorailError> {
    // open tracking and get log_info
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
    // use log_info to get results.json file in id dir
    let log_info = tracking_table.open_log_info().await?;
    let log_dir = cfg.get_log_path(work_dir).join(format!("{}", log_info.id));
    let run_output_file = std::fs::OpenOptions::new()
        .read(true)
        .open(log_dir.join(RUN_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let br = BufReader::new(run_output_file);
    let mut decoder = flate2::read::GzDecoder::new(br);
    Ok(serde_json::from_reader(&mut decoder)?)
}

#[derive(Debug)]
pub struct AnalysisShowInput<'a> {
    pub(crate) git_opts: GitOptions<'a>,
    pub(crate) show_changes: bool,
    pub(crate) show_change_targets: bool,
    pub(crate) show_target_groups: bool,
    pub(crate) targets: HashSet<&'a String>,
}

#[derive(Serialize, Debug)]
pub struct AnalysisShowOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<AnalyzedChange>>,
    targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_groups: Option<Vec<Vec<String>>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub struct AnalyzedChange {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Vec<AnalyzedChangeTarget>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub struct AnalyzedChangeTarget {
    path: String,
    reason: AnalyzedChangeTargetReason,
}
impl Ord for AnalyzedChangeTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}
impl PartialOrd for AnalyzedChangeTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Serialize, Debug, Eq, PartialEq)]
enum AnalyzedChangeTargetReason {
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "target_parent")]
    TargetParent,
    #[serde(rename = "uses")]
    Uses,
    #[serde(rename = "uses_target_parent")]
    UsesTargetParent,
    #[serde(rename = "ignores")]
    Ignores,
}

pub async fn handle_analyze_show<'a>(
    cfg: &'a Config,
    input: &AnalysisShowInput<'a>,
    work_dir: &'a path::Path,
) -> Result<AnalysisShowOutput, MonorailError> {
    let (mut lookups, changes) = match input.targets.len() {
        0 => {
            let changes = match cfg.vcs.r#use {
                VcsKind::Git => match cfg.vcs.r#use {
                    VcsKind::Git => {
                        let tracking = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
                        let checkpoint = match tracking.open_checkpoint().await {
                            Ok(checkpoint) => Some(checkpoint),
                            Err(MonorailError::TrackingCheckpointNotFound(_)) => None,
                            Err(e) => {
                                return Err(e);
                            }
                        };
                        get_git_all_changes(&input.git_opts, &checkpoint, work_dir).await?
                    }
                },
            };
            (
                Lookups::new(cfg, &cfg.get_target_path_set(), work_dir)?,
                changes,
            )
        }
        _ => (Lookups::new(cfg, &input.targets, work_dir)?, None),
    };

    analyze_show(
        &mut lookups,
        changes,
        input.show_changes,
        input.show_change_targets,
        input.show_target_groups,
    )
}

fn analyze_show(
    lookups: &mut Lookups<'_>,
    changes: Option<Vec<Change>>,
    show_changes: bool,
    show_change_targets: bool,
    show_target_groups: bool,
) -> Result<AnalysisShowOutput, MonorailError> {
    let mut output = AnalysisShowOutput {
        changes: if show_changes { Some(vec![]) } else { None },
        targets: vec![],
        target_groups: None,
    };

    let output_targets = match changes {
        Some(changes) => {
            let mut output_targets = HashSet::new();
            changes.iter().for_each(|c| {
                let mut change_targets = if show_change_targets {
                    Some(vec![])
                } else {
                    None
                };
                let mut add_targets = HashSet::new();

                lookups
                    .targets_trie
                    .common_prefix_search(&c.name)
                    .for_each(|target: String| {
                        // additionally, add any targets that lie further up from this target
                        lookups.targets_trie.common_prefix_search(&target).for_each(
                            |target2: String| {
                                if target2 != target {
                                    if let Some(change_targets) = change_targets.as_mut() {
                                        change_targets.push(AnalyzedChangeTarget {
                                            path: target2.to_owned(),
                                            reason: AnalyzedChangeTargetReason::TargetParent,
                                        });
                                    }
                                    add_targets.insert(target2);
                                }
                            },
                        );
                        if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(AnalyzedChangeTarget {
                                path: target.to_owned(),
                                reason: AnalyzedChangeTargetReason::Target,
                            });
                        }
                        add_targets.insert(target);
                    });
                lookups
                    .uses
                    .common_prefix_search(&c.name)
                    .for_each(|m: String| {
                        if let Some(v) = lookups.use2targets.get(m.as_str()) {
                            v.iter().for_each(|target| {
                                // additionally, add any targets that lie further up from this target
                                lookups.targets_trie.common_prefix_search(target).for_each(
                                    |target2: String| {
                                        if &target2 != target {
                                            if let Some(change_targets) = change_targets.as_mut() {
                                                change_targets.push(AnalyzedChangeTarget {
                                                path: target2.to_owned(),
                                                reason:
                                                    AnalyzedChangeTargetReason::UsesTargetParent,
                                            });
                                            }
                                            add_targets.insert(target2);
                                        }
                                    },
                                );
                                if let Some(change_targets) = change_targets.as_mut() {
                                    change_targets.push(AnalyzedChangeTarget {
                                        path: target.to_string(),
                                        reason: AnalyzedChangeTargetReason::Uses,
                                    });
                                }
                                add_targets.insert(target.to_string());
                            });
                        }
                    });

                lookups
                    .ignores
                    .common_prefix_search(&c.name)
                    .for_each(|m: String| {
                        if let Some(v) = lookups.ignore2targets.get(m.as_str()) {
                            v.iter().for_each(|target| {
                                add_targets.remove(*target);
                                if let Some(change_targets) = change_targets.as_mut() {
                                    change_targets.push(AnalyzedChangeTarget {
                                        path: target.to_string(),
                                        reason: AnalyzedChangeTargetReason::Ignores,
                                    });
                                }
                            });
                        }
                    });

                add_targets.iter().for_each(|t| {
                    output_targets.insert(t.to_owned());
                });

                if let Some(change_targets) = change_targets.as_mut() {
                    change_targets.sort();
                }

                if let Some(output_changes) = output.changes.as_mut() {
                    output_changes.push(AnalyzedChange {
                        path: c.name.to_owned(),
                        targets: change_targets,
                    });
                }
            });
            Some(output_targets)
        }
        None => None,
    };

    if let Some(output_targets) = output_targets {
        // copy the hashmap into the output vector
        for t in output_targets.iter() {
            output.targets.push(t.clone());
        }
        if show_target_groups {
            let groups = lookups.dag.get_groups()?;

            // prune the groups to contain only affected targets
            let mut pruned_groups: Vec<Vec<String>> = vec![];
            for group in groups.iter().rev() {
                let mut pg: Vec<String> = vec![];
                for id in group {
                    let label = lookups.dag.node2label.get(id).ok_or_else(|| {
                        MonorailError::DependencyGraph(GraphError::LabelNotFound(*id))
                    })?;
                    if output_targets.contains::<String>(label) {
                        pg.push(label.to_owned());
                    }
                }
                if !pg.is_empty() {
                    pruned_groups.push(pg);
                }
            }
            output.target_groups = Some(pruned_groups);
        }
    } else {
        // use config targets and all target groups
        for t in &lookups.targets {
            output.targets.push(t.to_string());
        }
        if show_target_groups {
            output.target_groups = Some(lookups.dag.get_labeled_groups()?);
        }
    }
    output.targets.sort();

    Ok(output)
}

#[derive(Debug, Serialize)]
pub struct HandleTargetShowInput {
    pub show_target_groups: bool,
}
#[derive(Debug, Serialize)]
pub struct TargetListOutput {
    targets: Option<Vec<Target>>,
    target_groups: Option<Vec<Vec<String>>>,
}

pub fn handle_target_show(
    cfg: &Config,
    input: HandleTargetShowInput,
    work_dir: &path::Path,
) -> Result<TargetListOutput, MonorailError> {
    let mut o = TargetListOutput {
        targets: cfg.targets.clone(),
        target_groups: None,
    };
    if input.show_target_groups {
        let mut lookups = Lookups::new(cfg, &cfg.get_target_path_set(), work_dir)?;
        o.target_groups = Some(lookups.dag.get_labeled_groups()?);
    }
    Ok(o)
}

#[derive(Debug, Serialize)]
pub struct CheckpointDeleteOutput {
    checkpoint: tracking::Checkpoint,
}

pub async fn handle_checkpoint_delete(
    cfg: &Config,
    work_dir: &path::Path,
) -> Result<CheckpointDeleteOutput, MonorailError> {
    let tracking = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
    let mut checkpoint = tracking.open_checkpoint().await?;
    checkpoint.commit = "".to_string();
    checkpoint.pending = None;

    tokio::fs::remove_file(&checkpoint.path).await?;

    Ok(CheckpointDeleteOutput { checkpoint })
}

#[derive(Debug, Serialize)]
pub struct CheckpointShowOutput {
    checkpoint: tracking::Checkpoint,
}

pub async fn handle_checkpoint_show(
    cfg: &Config,
    work_dir: &path::Path,
) -> Result<CheckpointShowOutput, MonorailError> {
    let tracking = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
    Ok(CheckpointShowOutput {
        checkpoint: tracking.open_checkpoint().await?,
    })
}

#[derive(Debug)]
pub struct HandleCheckpointUpdateInput<'a> {
    pub(crate) pending: bool,
    pub(crate) git_opts: GitOptions<'a>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointUpdateOutput {
    checkpoint: tracking::Checkpoint,
}

pub async fn handle_checkpoint_update(
    cfg: &Config,
    input: &HandleCheckpointUpdateInput<'_>,
    work_dir: &path::Path,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    match cfg.vcs.r#use {
        VcsKind::Git => {
            checkpoint_update_git(
                input.pending,
                &input.git_opts,
                work_dir,
                &cfg.get_tracking_path(work_dir),
            )
            .await
        }
    }
}

async fn checkpoint_update_git<'a>(
    include_pending: bool,
    git_opts: &'a GitOptions<'a>,
    work_dir: &path::Path,
    tracking_path: &path::Path,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    let tracking = tracking::Table::new(tracking_path)?;
    let mut checkpoint = match tracking.open_checkpoint().await {
        Ok(cp) => cp,
        Err(MonorailError::TrackingCheckpointNotFound(_)) => tracking.new_checkpoint(),
        // TODO: need to set path on checkpoint tho; don't use default
        Err(e) => {
            return Err(e);
        }
    };
    checkpoint.commit = "head".to_string();
    if include_pending {
        // get all changes with no checkpoint, so diff will return [HEAD, staging area]
        let pending_changes = get_git_all_changes(git_opts, &None, work_dir).await?;
        if let Some(pending_changes) = pending_changes {
            if !pending_changes.is_empty() {
                let mut pending = HashMap::new();
                for change in pending_changes.iter() {
                    let p = work_dir.join(&change.name);
                    pending.insert(change.name.clone(), get_file_checksum(&p).await?);
                }
                checkpoint.pending = Some(pending);
            }
        }
    }
    checkpoint.save().await?;
    Ok(CheckpointUpdateOutput { checkpoint })
}

async fn git_cmd_other_changes(
    git_path: &str,
    work_dir: &path::Path,
) -> Result<Vec<Change>, MonorailError> {
    let mut child = tokio::process::Command::new(git_path)
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(MonorailError::from)?;
    let mut out = vec![];
    if let Some(stdout) = child.stdout.take() {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            out.push(Change { name: line });
        }
    }
    let mut stderr_string = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut stderr_string).await?;
    }
    let status = child.wait().await.map_err(|e| {
        MonorailError::Generic(format!(
            "Error getting git diff; error: {}, reason: {}",
            e, &stderr_string
        ))
    })?;
    if status.success() {
        Ok(out)
    } else {
        Err(MonorailError::Generic(format!(
            "Error getting git diff: {}",
            stderr_string
        )))
    }
}

async fn git_cmd_diff_changes(
    git_path: &str,
    work_dir: &path::Path,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<Vec<Change>, MonorailError> {
    let mut args = vec!["diff", "--name-only", "--find-renames"];
    if let Some(start) = start {
        args.push(start);
    }
    if let Some(end) = end {
        args.push(end);
    }
    let mut child = tokio::process::Command::new(git_path)
        .args(&args)
        .current_dir(work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(MonorailError::from)?;
    let mut out = vec![];
    if let Some(stdout) = child.stdout.take() {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            out.push(Change { name: line });
        }
    }
    let mut stderr_string = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_string(&mut stderr_string).await?;
    }
    let status = child.wait().await.map_err(|e| {
        MonorailError::Generic(format!(
            "Error getting git diff; error: {}, reason: {}",
            e, &stderr_string
        ))
    })?;
    if status.success() {
        Ok(out)
    } else {
        Err(MonorailError::Generic(format!(
            "Error getting git diff: {}",
            stderr_string
        )))
    }
}

#[inline(always)]
fn require_existence(work_dir: &path::Path, path: &str) -> Result<(), MonorailError> {
    let p = work_dir.join(path);
    if p.is_file() {
        return Ok(());
    }
    // we also require that there be at least one file in it, because
    // many other processes require non-empty directories to be correct
    if p.is_dir() {
        for entry in p.read_dir()?.flatten() {
            if entry.path().is_file() {
                return Ok(());
            }
        }
        return Err(MonorailError::Generic(format!(
            "Directory {} has no files",
            &p.display()
        )));
    }

    Err(MonorailError::PathDNE(path.to_owned()))
}

pub struct Lookups<'a> {
    targets: Vec<String>,
    targets_trie: Trie<u8>,
    ignores: Trie<u8>,
    uses: Trie<u8>,
    use2targets: HashMap<&'a str, Vec<&'a str>>,
    ignore2targets: HashMap<&'a str, Vec<&'a str>>,
    dag: graph::Dag,
}
impl<'a> Lookups<'a> {
    fn new(
        cfg: &'a Config,
        visible_targets: &HashSet<&String>,
        work_dir: &path::Path,
    ) -> Result<Self, MonorailError> {
        let mut targets = vec![];
        let mut targets_builder = TrieBuilder::new();
        let mut ignores_builder = TrieBuilder::new();
        let mut uses_builder = TrieBuilder::new();
        let mut use2targets = HashMap::<&str, Vec<&str>>::new();
        let mut ignore2targets = HashMap::<&str, Vec<&str>>::new();

        let num_targets = match cfg.targets.as_ref() {
            Some(targets) => targets.len(),
            None => 0,
        };
        let mut dag = graph::Dag::new(num_targets);

        if let Some(cfg_targets) = cfg.targets.as_ref() {
            cfg_targets.iter().enumerate().try_for_each(|(i, target)| {
                targets.push(target.path.to_owned());
                let target_path_str = target.path.as_str();
                require_existence(work_dir, target_path_str)?;
                if dag.label2node.contains_key(target_path_str) {
                    return Err(MonorailError::DependencyGraph(GraphError::DuplicateLabel(
                        target.path.to_owned(),
                    )));
                }
                dag.set_label(&target.path, i);
                targets_builder.push(&target.path);

                if let Some(ignores) = target.ignores.as_ref() {
                    ignores.iter().for_each(|s| {
                        ignores_builder.push(s);
                        ignore2targets
                            .entry(s.as_str())
                            .or_default()
                            .push(target_path_str);
                    });
                }
                Ok(())
            })?;
        }

        let targets_trie = targets_builder.build();

        // process target uses and build up both the dependency graph, and the direct mapping of non-target uses to the affected targets
        if let Some(cfg_targets) = cfg.targets.as_ref() {
            cfg_targets.iter().enumerate().try_for_each(|(i, target)| {
                let target_path_str = target.path.as_str();
                // if this target is under an existing target, add it as a dep
                let mut nodes = targets_trie
                    .common_prefix_search(target_path_str)
                    .filter(|t: &String| t != &target.path)
                    .map(|t| {
                        dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                            MonorailError::DependencyGraph(GraphError::LabelNodeNotFound(t))
                        })
                    })
                    .collect::<Result<Vec<usize>, MonorailError>>()?;

                if let Some(uses) = &target.uses {
                    for s in uses {
                        let uses_path_str = s.as_str();
                        uses_builder.push(uses_path_str);
                        let matching_targets: Vec<String> =
                            targets_trie.common_prefix_search(uses_path_str).collect();
                        use2targets.entry(s).or_default().push(target_path_str);
                        // a dependency has been established between this target and some
                        // number of targets, so we update the graph
                        nodes.extend(
                            matching_targets
                                .iter()
                                .filter(|&t| t != &target.path)
                                .map(|t| {
                                    dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                                        MonorailError::DependencyGraph(
                                            GraphError::LabelNodeNotFound(t.to_owned()),
                                        )
                                    })
                                })
                                .collect::<Result<Vec<usize>, MonorailError>>()?,
                        );
                    }
                }
                nodes.sort();
                nodes.dedup();
                dag.set(i, nodes);
                Ok::<(), MonorailError>(())
            })?;

            // now that the graph is fully constructed, set subtree visibility
            for t in visible_targets {
                let node = dag.label2node.get(t.as_str()).copied().ok_or_else(|| {
                    MonorailError::DependencyGraph(GraphError::LabelNodeNotFound(
                        t.to_owned().clone(),
                    ))
                })?;
                dag.set_subtree_visibility(node, true);
            }
        }

        targets.sort();
        Ok(Self {
            targets,
            targets_trie,
            ignores: ignores_builder.build(),
            uses: uses_builder.build(),
            use2targets,
            ignore2targets,
            dag,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Change {
    name: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
enum VcsKind {
    #[serde(rename = "git")]
    #[default]
    Git,
}
impl FromStr for VcsKind {
    type Err = MonorailError;
    fn from_str(s: &str) -> Result<VcsKind, Self::Err> {
        match s {
            "git" => Ok(VcsKind::Git),
            _ => Err(MonorailError::Generic(format!(
                "Unrecognized vcs kind: {}",
                s
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    #[serde(default = "Config::default_output_path")]
    output_dir: String,
    #[serde(default = "Config::default_max_retained_run_results")]
    max_retained_run_results: usize,
    #[serde(default)]
    vcs: Vcs,
    targets: Option<Vec<Target>>,
}
impl Config {
    pub fn new(file_path: &path::Path) -> Result<Config, MonorailError> {
        let file = File::open(file_path)?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf()?;
        Ok(toml::from_str(std::str::from_utf8(buf)?)?)
    }
    pub fn get_target_path_set(&self) -> HashSet<&String> {
        let mut o = HashSet::new();
        if let Some(targets) = &self.targets {
            for t in targets {
                o.insert(&t.path);
            }
        }
        o
    }
    pub fn get_tracking_path(&self, work_dir: &path::Path) -> path::PathBuf {
        work_dir.join(&self.output_dir).join("tracking")
    }
    pub fn get_log_path(&self, work_dir: &path::Path) -> path::PathBuf {
        work_dir.join(&self.output_dir).join("log")
    }
    fn default_output_path() -> String {
        "monorail-out".to_string()
    }
    fn default_max_retained_run_results() -> usize {
        10
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Vcs {
    #[serde(default)]
    r#use: VcsKind,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Target {
    // The filesystem path, relative to the repository root.
    path: String,
    // Out-of-path directories that should affect this target. If this
    // path lies within a target, then a dependency for this target
    // on the other target.
    uses: Option<Vec<String>>,
    // Paths that should not affect this target; has the highest
    // precedence when evaluating a change.
    ignores: Option<Vec<String>>,
    // Relative path from this target's `path` to a file containing
    // functions that can be executed by `monorail run`.
    #[serde(default = "Target::default_run")]
    run: String,
}
impl Target {
    fn default_run() -> String {
        "monorail.sh".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    const RAW_CONFIG: &str = r#"
[[targets]]
path = "rust"

[[targets]]
path = "rust/target"
ignores = [
    "rust/target/ignoreme.txt"
]
uses = [
    "rust/vendor",
    "common"
]
"#;

    #[tokio::test]
    async fn test_get_git_diff_changes_ok() -> Result<(), Box<dyn std::error::Error>> {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let start = get_head(&repo_path).await;

        // no start/end without a checkpoint or pending changes is ok(none)
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            None
        );

        // start == end is ok
        git_opts.start = Some(&start);
        git_opts.end = Some(&start);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![])
        );

        // start < end with changes is ok
        let foo_path = &repo_path.join("foo.txt");
        let _foo_checksum = write_with_checksum(foo_path, &[1]).await?;
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;
        let end = get_head(&repo_path).await;
        git_opts.start = Some(&start);
        git_opts.end = Some(&end);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        // start > end with changes is ok
        git_opts.start = Some(&end);
        git_opts.end = Some(&start);
        assert_eq!(
            get_git_diff_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_ok_with_checkpoint() -> Result<(), Box<dyn std::error::Error>>
    {
        let repo_path = init(false).await;
        let git_opts = GitOptions {
            start: None,
            end: None,
            git_path: "git",
        };

        // no changes with empty checkpoint commit is ok(none)
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: "".to_string(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            None
        );

        // get initial start of repo
        commit(&repo_path).await;
        let first_head = get_head(&repo_path).await;

        // no changes for valid checkpoint commit is empty vector
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: first_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![])
        );

        // create first file and commit
        let foo_path = &repo_path.join("foo.txt");
        let _ = write_with_checksum(foo_path, &[1]).await?;
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;
        let second_head = get_head(&repo_path).await;

        // foo visible when checkpoint commit is first head
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: first_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );
        // foo invisble when checkpoint commit is updated to second head
        assert_eq!(
            get_git_diff_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: second_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![])
        );
        // foo is visible if user passes start, since it has higher priority over checkpoint commit
        assert_eq!(
            get_git_diff_changes(
                &GitOptions {
                    start: Some(&first_head),
                    end: None,
                    git_path: "git",
                },
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: second_head.clone(),
                    pending: None,
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_err() -> Result<(), Box<dyn std::error::Error>> {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let start = get_head(&repo_path).await;

        // no start/end, with invalid checkpoint commit is err
        assert!(get_git_diff_changes(
            &git_opts,
            &Some(tracking::Checkpoint {
                path: path::Path::new("x").to_path_buf(),
                commit: "test".to_string(),
                pending: Some(HashMap::from([(
                    "foo.txt".to_string(),
                    "blarp".to_string()
                )])),
            }),
            &repo_path
        )
        .await
        .is_err());

        // bad start is err
        git_opts.start = Some("foo");
        assert!(get_git_diff_changes(&git_opts, &None, &repo_path)
            .await
            .is_err());

        // bad end is err
        git_opts.start = Some(&start);
        git_opts.end = Some("foo");
        assert!(get_git_diff_changes(&git_opts, &None, &repo_path)
            .await
            .is_err());
        git_opts.start = None;
        git_opts.end = None;

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok1() {
        let repo_path = init(false).await;
        // no changes, no checkpoint is ok
        assert!(get_git_all_changes(
            &GitOptions {
                start: None,
                end: None,
                git_path: "git",
            },
            &None,
            &repo_path
        )
        .await
        .unwrap()
        .is_none());
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok2() {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let start = get_head(&repo_path).await;
        git_opts.start = Some(&start);

        // no changes, with checkpoint is ok
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: start.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        "dsfksl".to_string()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_get_git_all_changes_ok3() {
        let repo_path = init(false).await;
        let mut git_opts = GitOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        commit(&repo_path).await;
        let start = get_head(&repo_path).await;
        git_opts.start = Some(&start);

        // create a new file and check that it is seen
        let foo_path = &repo_path.join("foo.txt");
        let foo_checksum = write_with_checksum(foo_path, &[1]).await.unwrap();
        add("foo.txt", &repo_path).await;
        commit(&repo_path).await;

        assert_eq!(
            get_git_all_changes(&git_opts, &None, &repo_path)
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );

        // update checkpoint to include file and check that it is no longer seen,
        // even though commit sha lags
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: start.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        foo_checksum.clone()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );

        let end = get_head(&repo_path).await;
        git_opts.end = Some(&end);

        // create another file and check that it is seen, even though checkpoint
        // points to head
        let bar_path = &repo_path.join("bar.txt");
        let bar_checksum = write_with_checksum(bar_path, &[2]).await.unwrap();
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: end.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        foo_checksum.clone()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            1
        );

        // update checkpoint and verify second file is not seen
        assert_eq!(
            get_git_all_changes(
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: path::Path::new("x").to_path_buf(),
                    commit: end.clone(),
                    pending: Some(HashMap::from([
                        ("foo.txt".to_string(), foo_checksum),
                        ("bar.txt".to_string(), bar_checksum)
                    ])),
                }),
                &repo_path
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_get_filtered_changes() {
        let repo_path = init(false).await;
        let root_path = &repo_path;
        let fname1 = "test1.txt";
        let fname2 = "test2.txt";
        let change1 = Change {
            name: fname1.into(),
        };
        let change2 = Change {
            name: fname2.into(),
        };

        // changes, pending with change checksum match
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone()],
                &get_pending(&[(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                        .await
                        .unwrap(),
                )]),
                &repo_path
            )
            .await,
            vec![]
        );

        // changes, pending with change checksum mismatch
        tokio::fs::write(path::Path::new(&repo_path).join(fname1), &[1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone()],
                &get_pending(&[(fname1, "foo".into(),)]),
                &repo_path
            )
            .await,
            vec![change1.clone()]
        );

        // empty changes, empty pending
        assert_eq!(
            get_filtered_changes(vec![], &HashMap::new(), &repo_path).await,
            vec![]
        );

        // changes, empty pending
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone(), change2.clone()],
                &HashMap::new(),
                &repo_path
            )
            .await,
            vec![change1.clone(), change2.clone()]
        );
        // empty changes, pending
        assert_eq!(
            get_filtered_changes(
                vec![],
                &get_pending(&[(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                        .await
                        .unwrap(),
                )]),
                &repo_path
            )
            .await,
            vec![]
        );
    }

    fn get_pending(pairs: &[(&str, String)]) -> HashMap<String, String> {
        let mut pending = HashMap::new();
        for (fname, checksum) in pairs {
            pending.insert(fname.to_string(), checksum.clone());
        }
        pending
    }

    #[tokio::test]
    async fn test_checksum_is_equal() {
        let repo_path = init(false).await;
        let fname1 = "test1.txt";

        let root_path = &repo_path;
        let pending = get_pending(&[(
            fname1,
            write_with_checksum(&root_path.join(fname1), &[1, 2, 3])
                .await
                .unwrap(),
        )]);

        // checksums must match
        assert!(checksum_is_equal(&pending, &repo_path, fname1).await);
        // file error (such as dne) interpreted as checksum mismatch
        assert!(!checksum_is_equal(&pending, &repo_path, "dne.txt").await);

        // write a file and use a pending entry with a mismatched checksum
        let fname2 = "test2.txt";
        tokio::fs::write(root_path.join(fname2), &[1])
            .await
            .unwrap();
        let pending2 = get_pending(&[(fname2, "foobar".into())]);
        // checksums don't match
        assert!(!checksum_is_equal(&pending2, &repo_path, fname2).await);
    }

    #[tokio::test]
    async fn test_get_file_checksum() {
        let repo_path = init(false).await;

        // files that don't exist can't be checksummed
        let p = &repo_path.join("test.txt");
        assert!(get_file_checksum(p).await.is_err());

        // write file and compare checksums
        let checksum = write_with_checksum(p, &[1, 2, 3]).await.unwrap();
        assert_eq!(get_file_checksum(p).await.unwrap(), checksum);
    }

    async fn write_with_checksum(path: &path::Path, data: &[u8]) -> Result<String, MonorailError> {
        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        tokio::fs::write(path, &data).await?;
        Ok(hex::encode(hasher.finalize()).to_string())
    }

    #[test]
    fn test_trie() {
        let mut builder = TrieBuilder::new();
        builder.push("rust/target/project1/README.md");
        builder.push("common/log");
        builder.push("common/error");
        builder.push("rust/foo/log");

        let trie = builder.build();

        assert!(trie.exact_match("rust/target/project1/README.md"));
        let matches = trie
            .common_prefix_search("common/log/bar.rs")
            .collect::<Vec<String>>();
        assert_eq!(String::from_utf8_lossy(matches[0].as_bytes()), "common/log");
    }

    #[test]
    fn test_config_new() {
        // TODO
    }

    async fn prep_raw_config_repo() -> (Config, path::PathBuf) {
        let repo_path = init(false).await;
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();

        create_file(
            &repo_path,
            "rust",
            "monorail.sh",
            b"function whoami { echo 'rust' }",
        )
        .await;
        create_file(
            &repo_path,
            "rust/target",
            "monorail.sh",
            b"function whoami { echo 'rust/target' }",
        )
        .await;
        (c, repo_path)
    }

    #[tokio::test]
    async fn test_analyze_empty() {
        let changes = vec![];
        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, false, false).unwrap();

        assert!(o.changes.unwrap().is_empty());
        assert!(o.targets.is_empty());
    }

    #[tokio::test]
    async fn test_analyze_unknown() {
        let change1 = "foo.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let expected_targets: Vec<String> = vec![];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![]));
    }

    #[tokio::test]
    async fn test_analyze_target_file() {
        let change1 = "rust/lib.rs";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let expected_targets = vec![target1.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }
    #[tokio::test]
    async fn test_analyze_target() {
        let change1 = "rust";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let expected_targets = vec![target1.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[tokio::test]
    async fn test_analyze_target_ancestors() {
        let change1 = "rust/target/foo.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let target2 = "rust/target";
        let expected_targets = vec![target1.to_string(), target2.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()], vec![target2.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::TargetParent,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
            ]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[tokio::test]
    async fn test_analyze_target_uses() {
        let change1 = "common/foo.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let target2 = "rust/target";
        let expected_targets = vec![target1.to_string(), target2.to_string()];
        let expected_target_groups = vec![vec![target1.to_string()], vec![target2.to_string()]];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::UsesTargetParent,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Uses,
                },
            ]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[tokio::test]
    async fn test_analyze_target_ignores() {
        let change1 = "rust/target/ignoreme.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let target2 = "rust/target";
        let expected_targets = vec![target1.to_string()];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::TargetParent,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Ignores,
                },
            ]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let o = analyze_show(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![vec![target1.to_string()]]));
    }

    #[tokio::test]
    async fn test_lookups() {
        let (c, work_dir) = prep_raw_config_repo().await;
        let l = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();

        assert_eq!(
            l.targets_trie
                .common_prefix_search("rust/target/src/foo.rs")
                .collect::<Vec<String>>(),
            vec!["rust".to_string(), "rust/target".to_string()]
        );
        assert_eq!(
            l.uses
                .common_prefix_search("common/foo.txt")
                .collect::<Vec<String>>(),
            vec!["common".to_string()]
        );
        assert_eq!(
            l.ignores
                .common_prefix_search("rust/target/ignoreme.txt")
                .collect::<Vec<String>>(),
            vec!["rust/target/ignoreme.txt".to_string()]
        );
        // lies within `rust` target, so it's in the dag, not the map
        assert_eq!(*l.use2targets.get("common").unwrap(), vec!["rust/target"]);
        assert_eq!(
            *l.ignore2targets.get("rust/target/ignoreme.txt").unwrap(),
            vec!["rust/target"]
        );
    }

    #[test]
    fn test_err_duplicate_target_path() {
        let config_str: &str = r#"
[[targets]]
path = "rust"

[[targets]]
path = "rust"
"#;
        let c: Config = toml::from_str(config_str).unwrap();
        let work_dir = std::env::current_dir().unwrap();
        assert!(Lookups::new(&c, &c.get_target_path_set(), &work_dir).is_err());
    }
}
