mod graph;
mod tracking;

use std::cmp::Ordering;
use std::{io, path, sync, time};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::PermissionsExt;

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use tracing::{debug, error, info, instrument, trace};
use trie_rs::{Trie, TrieBuilder};

use crate::common::error::{GraphError, MonorailError};

const RUN_OUTPUT_FILE_NAME: &str = "run.json.zst";
const STDOUT_FILE: &str = "stdout.zst";
const STDERR_FILE: &str = "stderr.zst";
const RESET_COLOR: &str = "\x1b[0m";

#[derive(Debug)]
pub struct GitOptions<'a> {
    pub(crate) start: Option<&'a str>,
    pub(crate) end: Option<&'a str>,
    pub(crate) git_path: &'a str,
}

#[derive(Debug)]
pub struct RunInput<'a> {
    pub(crate) git_opts: GitOptions<'a>,
    pub(crate) commands: Vec<&'a String>,
    pub(crate) targets: HashSet<&'a String>,
    pub(crate) include_deps: bool,
    pub(crate) fail_on_undefined: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunOutput {
    pub(crate) failed: bool,
    invocation_args: String,
    results: Vec<CommandRunResult>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct CommandRunResult {
    command: String,
    successes: Vec<TargetRunResult>,
    failures: Vec<TargetRunResult>,
    unknowns: Vec<TargetRunResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TargetRunResult {
    target: String,
    code: Option<i32>,
    stdout_path: Option<path::PathBuf>,
    stderr_path: Option<path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    runtime_secs: f32,
}

// Open a file from the provided path, and compute its checksum
// TODO: allow hasher to be passed in?
// TODO: configure buffer size based on file size?
// TODO: pass in open file instead of path?
async fn get_file_checksum(p: &path::Path) -> Result<String, MonorailError> {
    let mut file = match tokio::fs::File::open(p).await {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok("".to_string()),
        Err(e) => {
            return Err(MonorailError::from(e));
        }
    };
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
        // otherwise, check checkpoint.id; if provided, use that
        if let Some(checkpoint) = checkpoint {
            if checkpoint.id.is_empty() {
                None
            } else {
                Some(&checkpoint.id)
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
    // let diff_changes = get_git_diff_changes(git_opts, checkpoint, work_dir).await?;
    // let mut other_changes = git_cmd_other_changes(git_opts.git_path, work_dir).await?;
    let (diff_changes, mut other_changes) = tokio::try_join!(
        get_git_diff_changes(git_opts, checkpoint, work_dir),
        git_cmd_other_changes(git_opts.git_path, work_dir)
    )?;
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
pub struct LogTailInput {
    pub filter_input: LogFilterInput,
}

pub async fn log_tail<'a>(_cfg: &'a Config, input: &LogTailInput) -> Result<(), MonorailError> {
    let mut lss = LogStreamServer::new("127.0.0.1:9201", &input.filter_input);
    lss.listen().await
}

#[derive(Serialize)]
pub struct LogShowInput<'a> {
    pub id: Option<&'a usize>,
    pub filter_input: LogFilterInput,
}

pub fn log_show<'a>(
    cfg: &'a Config,
    input: &'a LogShowInput<'a>,
    work_dir: &'a path::Path,
) -> Result<(), MonorailError> {
    let log_id = match input.id {
        Some(id) => *id,
        None => {
            let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
            let log_info = tracking_table.open_log_info()?;
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

    let mut stdout = std::io::stdout();
    // open directory at log_dir
    for fn_entry in log_dir.read_dir()? {
        let fn_path = fn_entry?.path();
        if fn_path.is_dir() {
            let command = fn_path.file_name().unwrap().to_str().unwrap();
            for t_entry in fn_path.read_dir()? {
                let t_path = t_entry?.path();
                if t_path.is_dir() {
                    let target_hash = t_path.file_name().unwrap().to_str().unwrap();
                    for e in t_path.read_dir()? {
                        let target =
                            hash2target
                                .get(target_hash)
                                .ok_or(MonorailError::Generic(format!(
                                    "Target not found for {}",
                                    &target_hash
                                )))?;
                        let p = e?.path();
                        let filename = p
                            .file_name()
                            .ok_or(MonorailError::Generic(format!(
                                "Bad path file name: {:?}",
                                &p
                            )))?
                            .to_str()
                            .ok_or(MonorailError::from("Bad file name string"))?;
                        if is_log_allowed(
                            &input.filter_input.targets,
                            &input.filter_input.commands,
                            target,
                            command,
                        ) && (filename == STDOUT_FILE && input.filter_input.include_stdout
                            || filename == STDERR_FILE && input.filter_input.include_stderr)
                        {
                            let header = get_log_header(filename, target, command, true);
                            let header_bytes = header.as_bytes();
                            stream_archive_file_to_stdout(header_bytes, &p, &mut stdout)?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn get_log_header(filename: &str, target: &str, command: &str, color: bool) -> String {
    if color {
        let filename_color = match filename {
            STDOUT_FILE => "\x1b[38;5;81m",
            STDERR_FILE => "\x1b[38;5;214m",
            _ => "",
        };
        format!("[monorail | {filename_color}{filename}{RESET_COLOR} | {target} | {command}]\n")
    } else {
        format!("[monorail | {filename} | {target} | {command}]\n")
    }
}

fn stream_archive_file_to_stdout(
    header: &[u8],
    path: &path::Path,
    stdout: &mut std::io::Stdout,
) -> Result<(), MonorailError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let decoder = zstd::stream::read::Decoder::new(reader)?;
    let mut line_reader = std::io::BufReader::new(decoder);
    let mut line: Vec<u8> = Vec::new();
    let mut wrote_header = false;
    while line_reader.read_until(b'\n', &mut line)? > 0 {
        if !wrote_header {
            stdout.write_all(header)?;
            wrote_header = true;
        }
        stdout.write_all(&line)?;
        line.clear();
    }

    Ok(())
}

#[instrument]
pub async fn handle_run<'a>(
    cfg: &'a Config,
    input: &'a RunInput<'a>,
    invocation_args: &'a str,
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
            let checkpoint = match tracking_table.open_checkpoint() {
                Ok(checkpoint) => Some(checkpoint),
                Err(MonorailError::TrackingCheckpointNotFound(_)) => None,
                Err(e) => {
                    return Err(e);
                }
            };

            let changes = match cfg.change_provider.r#use {
                ChangeProviderKind::Git => {
                    get_git_all_changes(&input.git_opts, &checkpoint, work_dir).await?
                }
            };
            let ai = AnalyzeInput::new(false, false, true);
            let ao = analyze(&ai, &mut lookups, changes)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            run(
                cfg,
                &tracking_table,
                &lookups,
                work_dir,
                &input.commands,
                targets.clone(),
                &target_groups,
                input.fail_on_undefined,
                invocation_args,
            )
            .await
        }
        _ => {
            let mut lookups = Lookups::new(cfg, &input.targets, work_dir)?;
            let target_groups = if input.include_deps {
                let ai = AnalyzeInput::new(false, false, true);
                let ao = analyze(&ai, &mut lookups, None)?;
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

            run(
                cfg,
                &tracking_table,
                &lookups,
                work_dir,
                &input.commands,
                targets.clone(),
                &target_groups,
                input.fail_on_undefined,
                invocation_args,
            )
            .await
        }
    }
}
#[derive(Serialize)]
struct RunData {
    target_path: String,
    command_work_dir: path::PathBuf,
    command_path: Option<path::PathBuf>,
    command_args: Option<Vec<String>>,
    #[serde(skip)]
    logs: Logs,
}
#[derive(Serialize)]
struct RunDataGroup {
    #[serde(skip)]
    command_index: usize,
    datas: Vec<RunData>,
}
#[derive(Serialize)]
struct RunDataGroups {
    groups: Vec<RunDataGroup>,
}

fn find_command_executable(name: &str, dir: &path::Path) -> Option<path::PathBuf> {
    debug!(
        name = name,
        dir = dir.display().to_string(),
        "Command search"
    );
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(stem) = path.file_stem() {
                    if stem == name {
                        return Some(path.to_path_buf());
                    }
                }
            }
        }
    }
    None
}

fn is_executable(p: &path::Path) -> bool {
    if let Ok(metadata) = std::fs::metadata(p) {
        let permissions = metadata.permissions();
        return permissions.mode() & 0o111 != 0;
    }
    false
}

// Create an initial run output with unknowns, and build the RunData for each target group.
fn get_run_data_groups<'a>(
    lookups: &Lookups<'_>,
    commands: &'a [&'a String],
    targets: &[Target],
    target_groups: &[Vec<String>],
    work_dir: &path::Path,
    log_dir: &path::Path,
    invocation_args: &'a str,
) -> Result<(RunOutput, RunDataGroups), MonorailError> {
    // for converting potentially deep nested paths into a single directory string
    let mut path_hasher = sha2::Sha256::new();

    let mut groups = Vec::with_capacity(target_groups.len());
    let mut crrs = Vec::with_capacity(target_groups.len());
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
                let commands_path = work_dir.join(target_path).join(&target.commands.path);
                let mut command_args = None;
                let command_path = match &target.commands.definitions {
                    Some(definitions) => match definitions.get(c.as_str()) {
                        Some(def) => {
                            command_args = Some(def.args.clone());
                            Some(commands_path.join(&def.exec))
                        }
                        None => find_command_executable(c, &commands_path),
                    },
                    None => find_command_executable(c, &commands_path),
                };
                run_data.push(RunData {
                    target_path: target_path.to_owned(),
                    command_work_dir: work_dir.join(target_path),
                    command_path,
                    command_args,
                    logs,
                });
            }
            groups.push(RunDataGroup {
                command_index: i,
                datas: run_data,
            });
            let crr = CommandRunResult {
                command: c.to_string(),
                successes: vec![],
                failures: vec![],
                unknowns,
            };
            crrs.push(crr);
        }
    }

    Ok((
        RunOutput {
            failed: false,
            invocation_args: invocation_args.to_owned(),
            results: crrs,
        },
        RunDataGroups { groups },
    ))
}

pub fn spawn_task(
    command_work_dir: &path::Path,
    command_path: &path::Path,
    command_args: &Option<Vec<String>>,
) -> Result<tokio::process::Child, MonorailError> {
    debug!(
        command_path = command_path.display().to_string(),
        command_args = command_args.clone().unwrap_or_default().join(", "),
        "Spawn task"
    );
    let mut cmd = tokio::process::Command::new(command_path);
    cmd.current_dir(command_work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // parallel execution makes use of stdin impractical
        .stdin(std::process::Stdio::null());
    if let Some(ca) = command_args {
        cmd.args(ca);
    }
    cmd.spawn().map_err(MonorailError::from)
}

fn is_log_allowed(
    targets: &HashSet<String>,
    commands: &HashSet<String>,
    target: &str,
    command: &str,
) -> bool {
    let target_allowed = targets.is_empty() || targets.contains(target);
    let command_allowed = commands.is_empty() || commands.contains(command);
    target_allowed && command_allowed
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
        stdout_client: CompressorClient,
        stderr_client: CompressorClient,
        log_stream_client: Option<LogStreamClient>,
    ) -> Result<
        (usize, std::process::ExitStatus, time::Duration),
        (usize, time::Duration, MonorailError),
    > {
        let (stdout_log_stream_client, stderr_log_stream_client) = match log_stream_client {
            Some(lsc) => {
                let allowed =
                    is_log_allowed(&lsc.args.targets, &lsc.args.commands, &target, &command);
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
        let stdout_header = get_log_header(&stdout_client.file_name, &target, &command, true);
        let stdout_fut = process_reader(
            tokio::io::BufReader::new(child.stdout.take().unwrap()),
            stdout_client,
            stdout_header,
            stdout_log_stream_client,
            token.clone(),
        );
        let stderr_header = get_log_header(&stderr_client.file_name, &target, &command, true);
        let stderr_fut = process_reader(
            tokio::io::BufReader::new(child.stderr.take().unwrap()),
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogFilterInput {
    pub commands: HashSet<String>,
    pub targets: HashSet<String>,
    pub include_stdout: bool,
    pub include_stderr: bool,
}

struct LogStreamServer<'a> {
    address: &'a str,
    args: &'a LogFilterInput,
}
impl<'a> LogStreamServer<'a> {
    fn new(address: &'a str, args: &'a LogFilterInput) -> Self {
        Self { address, args }
    }
    async fn listen(&mut self) -> Result<(), MonorailError> {
        let listener = tokio::net::TcpListener::bind(self.address).await?;
        let args_data = serde_json::to_vec(&self.args)?;
        debug!("Log stream server listening");
        loop {
            let (mut socket, _) = listener.accept().await?;
            debug!("Client connected");
            // first, write to the client what we're interested in receiving
            socket.write_all(&args_data).await?;
            _ = socket.write(b"\n").await?;
            debug!("Sent log stream arguments");
            Self::process(socket).await?;
        }
    }
    #[instrument]
    async fn process(socket: tokio::net::TcpStream) -> Result<(), MonorailError> {
        let br = tokio::io::BufReader::new(socket);
        let mut lines = br.lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            stdout.write_all(line.as_bytes()).await?;
            _ = stdout.write(b"\n").await?;
            stdout.flush().await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct LogStreamClient {
    stream: sync::Arc<tokio::sync::Mutex<tokio::net::TcpStream>>,
    args: LogFilterInput,
}
impl LogStreamClient {
    #[instrument]
    async fn data(
        &mut self,
        data: sync::Arc<Vec<Vec<u8>>>,
        header: &[u8],
    ) -> Result<(), MonorailError> {
        let mut guard = self.stream.lock().await;
        guard.write_all(header).await.map_err(MonorailError::from)?;
        for v in data.iter() {
            guard.write_all(v).await.map_err(MonorailError::from)?;
        }
        Ok(())
    }
    #[instrument]
    async fn connect(addr: &str) -> Result<Self, MonorailError> {
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        info!(address = addr, "Connected to log stream server");
        let mut args_data = Vec::new();
        // pull arg preferences from the server on connect
        let mut br = tokio::io::BufReader::new(&mut stream);
        br.read_until(b'\n', &mut args_data).await?;
        let args: LogFilterInput = serde_json::from_slice(args_data.as_slice())?;
        debug!("Received log stream arguments");
        if args.include_stdout || args.include_stderr {
            let targets = if args.targets.is_empty() {
                String::from("(any target)")
            } else {
                args.targets.iter().cloned().collect::<Vec<_>>().join(", ")
            };
            let commands = if args.commands.is_empty() {
                String::from("(any command)")
            } else {
                args.commands.iter().cloned().collect::<Vec<_>>().join(", ")
            };
            let mut files = vec![];
            if args.include_stdout {
                files.push(STDOUT_FILE);
            }
            if args.include_stderr {
                files.push(STDERR_FILE);
            }
            stream
                .write_all(get_log_header(&files.join(", "), &targets, &commands, false).as_bytes())
                .await
                .map_err(MonorailError::from)?;
        }

        Ok(Self {
            stream: sync::Arc::new(tokio::sync::Mutex::new(stream)),
            args,
        })
    }
}

async fn process_reader<R>(
    mut reader: tokio::io::BufReader<R>,
    compressor_client: CompressorClient,
    header: String,
    mut log_stream_client: Option<LogStreamClient>,
    token: sync::Arc<tokio_util::sync::CancellationToken>,
) -> Result<(), MonorailError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));
    loop {
        let mut bufs = Vec::new();
        loop {
            let mut buf = Vec::new();
            tokio::select! {
                _ = token.cancelled() => {
                    process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                    return Err(MonorailError::TaskCancelled);
                }
                res = reader.read_until(b'\n', &mut buf) => {
                    match res {
                        Ok(0) => {
                            process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                            return Ok(());
                        },
                        Ok(_n) => {
                            bufs.push(buf);
                        }
                        Err(e) => {
                            process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                            return Err(MonorailError::from(e));
                        }
                    }
                }
                _ = interval.tick() => {
                    process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, false).await?;
                    break;
                }
            }
        }
    }
}

#[instrument]
async fn process_bufs(
    header: &str,
    bufs: Vec<Vec<u8>>,
    compressor_client: &CompressorClient,
    log_stream_client: &mut Option<LogStreamClient>,
    should_end: bool,
) -> Result<(), MonorailError> {
    if !bufs.is_empty() {
        let bufs_arc = sync::Arc::new(bufs);
        let bufs_arc2 = bufs_arc.clone();
        let lsc_fut = async {
            if let Some(ref mut lsc) = log_stream_client {
                lsc.data(bufs_arc2, header.as_bytes()).await
            } else {
                Ok(())
            }
        };
        let cc_fut = async { compressor_client.data(bufs_arc).await };
        let (_, _) = tokio::try_join!(lsc_fut, cc_fut)?;
    }
    if should_end {
        compressor_client.end().await?;
    }

    Ok(())
}

#[derive(Debug)]
enum CompressRequest {
    Data(usize, sync::Arc<Vec<Vec<u8>>>),
    End(usize),
    Shutdown,
}
#[derive(Debug)]
struct Compressor {
    index: usize,
    num_threads: usize,
    req_channels: Vec<(
        flume::Sender<CompressRequest>,
        Option<flume::Receiver<CompressRequest>>,
    )>,
    registrations: Vec<Vec<path::PathBuf>>,
    shutdown: sync::Arc<sync::atomic::AtomicBool>,
}
#[derive(Debug, Clone)]
struct CompressorClient {
    file_name: String,
    encoder_index: usize,
    req_tx: flume::Sender<CompressRequest>,
}
impl CompressorClient {
    async fn data(&self, data: sync::Arc<Vec<Vec<u8>>>) -> Result<(), MonorailError> {
        self.req_tx
            .send_async(CompressRequest::Data(self.encoder_index, data))
            .await
            .map_err(MonorailError::from)
    }
    async fn end(&self) -> Result<(), MonorailError> {
        self.req_tx
            .send_async(CompressRequest::End(self.encoder_index))
            .await
            .map_err(MonorailError::from)
    }
    fn shutdown(&self) -> Result<(), MonorailError> {
        self.req_tx
            .send(CompressRequest::Shutdown)
            .map_err(MonorailError::from)
    }
}

impl Compressor {
    fn new(num_threads: usize, shutdown: sync::Arc<sync::atomic::AtomicBool>) -> Self {
        let mut req_channels = vec![];
        let mut registrations = vec![];
        for _ in 0..num_threads {
            let (req_tx, req_rx) = flume::bounded(1000);
            req_channels.push((req_tx, Some(req_rx)));
            registrations.push(vec![]);
        }
        Self {
            index: 0,
            num_threads,
            req_channels,
            registrations,
            shutdown,
        }
    }
    // Register the provided path and return a CompressorClient
    // that can be used to schedule operations on the underlying encoder.
    fn register(&mut self, p: &path::Path) -> Result<CompressorClient, MonorailError> {
        // todo; check path not already seen
        let thread_index = self.index % self.num_threads;
        let encoder_index = self.registrations[thread_index].len();
        self.registrations[thread_index].push(p.to_path_buf());
        self.index += 1;

        let file_name = p.file_name().unwrap().to_str().unwrap().to_string(); // todo; monorailerror

        Ok(CompressorClient {
            file_name,
            encoder_index,
            req_tx: self.req_channels[thread_index].0.clone(),
        })
    }
    fn run(&mut self) -> Result<(), MonorailError> {
        std::thread::scope(|s| {
            for x in 0..self.num_threads {
                let regs = &self.registrations[x];
                let req_rx =
                    self.req_channels[x]
                        .1
                        .take()
                        .ok_or(MonorailError::Generic(format!(
                            "Missing channel for index {}",
                            x
                        )))?;
                let shutdown = &self.shutdown;
                s.spawn(move || {
                    let mut encoders = vec![];
                    for r in regs.iter() {
                        let f = std::fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(r)
                            .map_err(|e| MonorailError::Generic(e.to_string()))?;
                        let bw = std::io::BufWriter::new(f);
                        encoders.push(zstd::stream::write::Encoder::new(bw, 3)?);
                    }
                    loop {
                        if shutdown.load(sync::atomic::Ordering::Relaxed) {
                            break;
                        };
                        match req_rx.recv()? {
                            CompressRequest::End(encoder_index) => {
                                trace!(
                                    encoder_index = encoder_index,
                                    thread_id = x,
                                    "encoder finish"
                                );
                                encoders[encoder_index].do_finish()?;
                            }
                            CompressRequest::Shutdown => {
                                trace!("compressor shutdown");
                                break;
                            }
                            CompressRequest::Data(encoder_index, data) => {
                                trace!(lines = data.len(), thread_id = x, "encoder write");
                                for v in data.iter() {
                                    encoders[encoder_index].write_all(v)?;
                                }
                            }
                        }
                    }
                    for mut enc in encoders {
                        trace!(thread_id = x, "encoder finish");
                        enc.do_finish()?;
                    }
                    Ok::<(), MonorailError>(())
                });
            }
            Ok(())
        })
    }
}

#[allow(clippy::too_many_arguments)]
#[instrument]
async fn run<'a>(
    cfg: &'a Config,
    tracking_table: &tracking::Table,
    lookups: &Lookups<'_>,
    work_dir: &path::Path,
    commands: &'a [&'a String],
    targets: Vec<Target>,
    target_groups: &[Vec<String>],
    fail_on_undefined: bool,
    invocation_args: &'a str,
) -> Result<RunOutput, MonorailError> {
    let log_stream_client = match LogStreamClient::connect("127.0.0.1:9201").await {
        Ok(lsc) => Some(lsc),
        Err(e) => {
            debug!(error = e.to_string(), "Log streaming disabled");
            None
        }
    };

    let mut o = RunOutput {
        failed: false,
        invocation_args: invocation_args.to_owned(),
        results: vec![],
    };
    let log_dir = {
        // obtain current log info counter and increment it before using
        let mut log_info = match tracking_table.open_log_info() {
            Ok(log_info) => log_info,
            Err(MonorailError::TrackingLogInfoNotFound(_)) => tracking_table.new_log_info(),
            Err(e) => {
                return Err(e);
            }
        };
        if log_info.id >= cfg.max_retained_runs {
            log_info.id = 0;
        }
        log_info.id += 1;

        log_info.save()?;
        let log_dir = cfg.get_log_path(work_dir).join(format!("{}", log_info.id));
        // remove the log_dir path if it exists, and create a new one
        std::fs::remove_dir_all(&log_dir).unwrap_or(());
        std::fs::create_dir_all(&log_dir)?;
        log_dir
    };

    let (initial_run_output, run_data_groups) = get_run_data_groups(
        lookups,
        commands,
        &targets,
        target_groups,
        work_dir,
        &log_dir,
        invocation_args,
    )?;

    // Make initial run output available for use by other commands
    store_run_output(&initial_run_output, &log_dir)?;

    let mut cancelled = false;

    info!(num = run_data_groups.groups.len(), "processing groups");

    // Spawn concurrent tasks for each group of rundata
    for mut run_data_group in run_data_groups.groups {
        let command = &commands[run_data_group.command_index];
        info!(
            num = run_data_group.datas.len(),
            command = command,
            "processing targets",
        );
        if cancelled {
            let mut unknowns = vec![];
            for rd in run_data_group.datas.iter_mut() {
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
            o.results.push(CommandRunResult {
                command: command.to_string(),
                successes: vec![],
                failures: vec![],
                unknowns,
            });
            continue;
        }

        let mut crr = CommandRunResult {
            command: command.to_string(),
            successes: vec![],
            failures: vec![],
            unknowns: vec![],
        };
        let mut js = tokio::task::JoinSet::new();
        let token = sync::Arc::new(tokio_util::sync::CancellationToken::new());
        let mut abort_table = HashMap::new();
        let compressor_shutdown = sync::Arc::new(sync::atomic::AtomicBool::new(false));
        let mut compressor = Compressor::new(2, compressor_shutdown.clone());
        let mut compressor_clients = vec![];
        for id in 0..run_data_group.datas.len() {
            let rd = &mut run_data_group.datas[id];
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
                if is_executable(command_path) {
                    info!(
                        status = "scheduled",
                        command = command,
                        target = &rd.target_path,
                        "task"
                    );
                    let child = spawn_task(&rd.command_work_dir, command_path, &rd.command_args)?;
                    let token2 = token.clone();
                    let log_stream_client2 = log_stream_client.clone();
                    let handle = js.spawn(async move {
                        ft.run(
                            child,
                            token2,
                            target2,
                            command2,
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
                        let rd = &mut run_data_group.datas[*run_data_index];
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
            o.failed = true;
        }
        o.results.push(crr);
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
    let mut encoder = zstd::stream::write::Encoder::new(bw, 3).unwrap(); // todo unwrap
    serde_json::to_writer(&mut encoder, run_output)?;
    encoder.finish().unwrap(); // todo unwrap
    Ok(())
}

#[derive(Serialize)]
pub struct Logs {
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
            stdout_path: dir_path.clone().join(STDOUT_FILE),
            stderr_path: dir_path.clone().join(STDERR_FILE),
        })
    }
}

#[derive(Debug)]
pub struct ResultShowInput {}

pub fn result_show<'a>(
    cfg: &'a Config,
    work_path: &'a path::Path,
    _input: &'a ResultShowInput,
) -> Result<RunOutput, MonorailError> {
    // open tracking and get log_info
    let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
    // use log_info to get results.json file in id dir
    let log_info = tracking_table.open_log_info()?;
    let log_dir = cfg.get_log_path(work_path).join(format!("{}", log_info.id));
    let run_output_file = std::fs::OpenOptions::new()
        .read(true)
        .open(log_dir.join(RUN_OUTPUT_FILE_NAME))
        .map_err(|e| MonorailError::Generic(e.to_string()))?;
    let br = BufReader::new(run_output_file);
    let mut decoder = zstd::stream::read::Decoder::new(br)?;

    Ok(serde_json::from_reader(&mut decoder)?)
}

#[derive(Debug)]
pub struct HandleAnalyzeInput<'a> {
    pub(crate) git_opts: GitOptions<'a>,
    pub(crate) analyze_input: AnalyzeInput,
    pub(crate) targets: HashSet<&'a String>,
}

#[derive(Debug)]
pub struct AnalyzeInput {
    pub(crate) show_changes: bool,
    pub(crate) show_change_targets: bool,
    pub(crate) show_target_groups: bool,
}
impl AnalyzeInput {
    fn new(show_changes: bool, show_change_targets: bool, show_target_groups: bool) -> Self {
        Self {
            show_changes,
            show_change_targets,
            show_target_groups,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct AnalyzeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<AnalyzedChange>>,
    targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_groups: Option<Vec<Vec<String>>>,
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

pub async fn handle_analyze<'a>(
    cfg: &'a Config,
    input: &HandleAnalyzeInput<'a>,
    work_dir: &'a path::Path,
) -> Result<AnalysisShowOutput, MonorailError> {
    let (mut lookups, changes) = match input.targets.len() {
        0 => {
            let changes = match cfg.change_provider.r#use {
                ChangeProviderKind::Git => match cfg.change_provider.r#use {
                    ChangeProviderKind::Git => {
                        let tracking = tracking::Table::new(&cfg.get_tracking_path(work_dir))?;
                        let checkpoint = match tracking.open_checkpoint() {
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

    analyze(&input.analyze_input, &mut lookups, changes)
}

// Gets the targets that would be ignored by this change.
fn get_ignore_targets<'a>(lookups: &'a Lookups<'_>, name: &'a str) -> HashSet<&'a str> {
    let mut ignore_targets = HashSet::new();
    lookups
        .ignores
        .common_prefix_search(name)
        .for_each(|m: String| {
            if let Some(v) = lookups.ignore2targets.get(m.as_str()) {
                v.iter().for_each(|target| {
                    ignore_targets.insert(*target);
                });
            }
        });
    ignore_targets
}

// // todo various stuff aroudn porting analysis show to analyze, factor out some of the internals of analyze

// fn analyze<'a>(
//     input: &AnalyzeInput<'a>,
//     lookups: &mut Lookups<'_>,
//     changes: Option<Vec<Change>>,
// ) -> Result<AnalyzeOutput, MonorailError> {
//     let mut output = AnalyzeOutput {
//         changes: if input.show_changes {
//             Some(vec![])
//         } else {
//             None
//         },
//         targets: vec![],
//         target_groups: None,
//     };

//     // determine which targets are affected by this change
//     let change_targets = match changes {
//         Some(changes) => {
//             let mut targets = HashSet::new();
//             changes.iter().for_each(|c| {
//                 let mut change_targets = if input.show_change_targets {
//                     Some(vec![])
//                 } else {
//                     None
//                 };
//                 let ignore_targets = get_ignore_targets(lookups, &c.name);
//                 analyze_change_targets_and_ancestors(
//                     &c.name,
//                     &lookups,
//                     &ignore_targets,
//                     &mut targets,
//                     &mut output,
//                 );

//                 let use_matches = lookups.uses_trie.common_prefix_search(&c.name);
//                 use_matches.for_each(|u: String| {
//                     if !ignore_targets.contains(u.as_str()) {
//                         if let Some(v) = lookups.use2targets.get(u.as_str()) {
//                             v.iter().for_each(|target| {
//                                 analyze_change_targets_and_ancestors(
//                                     target,
//                                     &lookups,
//                                     &ignore_targets,
//                                     &mut targets,
//                                     &mut output,
//                                 );
//                             });
//                         }
//                     }
//                 });
//             });
//             Some(targets)
//         }
//         None => None,
//     };
// }

// fn analyze_change_targets_and_ancestors(
//     change: &Change,
//     lookups: &Lookups<'_>,
//     ignore_targets: &HashSet<&str>,
//     targets: &mut HashSet<&str>,
//     output: &mut AnalyzeOutput,
// ) {
//     let target_matches = lookups.targets_trie.common_prefix_search(&change.name);
//     target_matches.for_each(|target: String| {
//         if !ignore_targets.contains(target.as_str()) {
//             targets.insert(target);
//             trace!(target = &target, "Added");
//             // additionally check any target path ancestors
//             let ancestor_target_matches = lookups.targets_trie.common_prefix_search(&target);
//             ancestor_target_matches.for_each(|target2: String| {
//                 if target2 != target && !ignore_targets.contains(target2.as_str()) {
//                     targets.insert(target2);
//                     trace!(target = &target, ancestor = &target2, "Added ancestor");
//                 } else {
//                     trace!(target = &target, ancestor = &target2, "Ignored ancestor");
//                 }
//             });
//         } else {
//             trace!(target = &target, "Ignored");
//         }
//     });
// }

fn analyze(
    input: &AnalyzeInput,
    lookups: &mut Lookups<'_>,
    changes: Option<Vec<Change>>,
) -> Result<AnalysisShowOutput, MonorailError> {
    let mut output = AnalysisShowOutput {
        changes: if input.show_changes {
            Some(vec![])
        } else {
            None
        },
        targets: vec![],
        target_groups: None,
    };
    let output_targets = match changes {
        Some(changes) => {
            let mut output_targets = HashSet::new();
            changes.iter().for_each(|c| {
                let mut change_targets = if input.show_change_targets {
                    Some(vec![])
                } else {
                    None
                };
                let ignore_targets = get_ignore_targets(lookups, &c.name);
                lookups
                    .targets_trie
                    .common_prefix_search(&c.name)
                    .for_each(|target: String| {
                        if !ignore_targets.contains(target.as_str()) {
                            // additionally, add any targets that lie further up from this target
                            lookups.targets_trie.common_prefix_search(&target).for_each(
                                |target2: String| {
                                    if target2 != target {
                                        if !ignore_targets.contains(target2.as_str()) {
                                            if let Some(change_targets) = change_targets.as_mut() {
                                                change_targets.push(AnalyzedChangeTarget {
                                                    path: target2.to_owned(),
                                                    reason:
                                                        AnalyzedChangeTargetReason::TargetParent,
                                                });
                                            }
                                            output_targets.insert(target2);
                                        } else if let Some(change_targets) = change_targets.as_mut() {
                                            change_targets.push(AnalyzedChangeTarget {
                                                path: target.to_owned(),
                                                reason: AnalyzedChangeTargetReason::Ignores,
                                            });
                                        }
                                    }
                                },
                            );
                            if let Some(change_targets) = change_targets.as_mut() {
                                change_targets.push(AnalyzedChangeTarget {
                                    path: target.to_owned(),
                                    reason: AnalyzedChangeTargetReason::Target,
                                });
                            }
                            output_targets.insert(target);
                        } else if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(AnalyzedChangeTarget {
                                path: target.to_owned(),
                                reason: AnalyzedChangeTargetReason::Ignores,
                            });
                        }
                    });
                lookups
                    .uses
                    .common_prefix_search(&c.name)
                    .for_each(|m: String| {
                        if !ignore_targets.contains(m.as_str()) {
                            if let Some(v) = lookups.use2targets.get(m.as_str()) {
                                v.iter().for_each(|target| {
                                    if !ignore_targets.contains(target) {
                                        // additionally, add any targets that lie further up from this target
                                        lookups.targets_trie.common_prefix_search(target).for_each(
                                            |target2: String| {
                                                if &target2 != target {
                                                    if !ignore_targets.contains(target2.as_str()) {
                                                        if let Some(change_targets) = change_targets.as_mut() {
                                                            change_targets.push(AnalyzedChangeTarget {
                                                                path: target2.to_owned(),
                                                                reason: AnalyzedChangeTargetReason::UsesTargetParent,
                                                            });
                                                        }
                                                        output_targets.insert(target2);
                                                } else if let Some(change_targets) = change_targets.as_mut() {
                                                        change_targets.push(AnalyzedChangeTarget {
                                                            path: target.to_string(),
                                                            reason: AnalyzedChangeTargetReason::Ignores,
                                                        });
                                                    }
                                                }
                                            },
                                        );
                                        if let Some(change_targets) = change_targets.as_mut() {
                                            change_targets.push(AnalyzedChangeTarget {
                                                path: target.to_string(),
                                                reason: AnalyzedChangeTargetReason::Uses,
                                            });
                                        }
                                        output_targets.insert(target.to_string());
                                    } else if let Some(change_targets) = change_targets.as_mut() {
                                        change_targets.push(AnalyzedChangeTarget {
                                            path: target.to_string(),
                                            reason: AnalyzedChangeTargetReason::Ignores,
                                        });
                                    }
                                });
                            }
                        }
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

    // todo; copy this into analyze factored
    if let Some(output_targets) = output_targets {
        // copy the hashmap into the output vector
        for t in output_targets.iter() {
            output.targets.push(t.clone());
        }
        if input.show_target_groups {
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
        if input.show_target_groups {
            output.target_groups = Some(lookups.dag.get_labeled_groups()?);
        }
    }
    output.targets.sort();

    Ok(output)
}

#[derive(Debug, Serialize)]
pub struct TargetShowInput {
    pub show_target_groups: bool,
}
#[derive(Debug, Serialize)]
pub struct TargetShowOutput {
    targets: Option<Vec<Target>>,
    target_groups: Option<Vec<Vec<String>>>,
}

pub fn target_show(
    cfg: &Config,
    input: TargetShowInput,
    work_dir: &path::Path,
) -> Result<TargetShowOutput, MonorailError> {
    let mut o = TargetShowOutput {
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
    let mut checkpoint = tracking.open_checkpoint()?;
    checkpoint.id = "".to_string();
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
        checkpoint: tracking.open_checkpoint()?,
    })
}

#[derive(Debug)]
pub struct CheckpointUpdateInput<'a> {
    pub(crate) pending: bool,
    pub(crate) git_opts: GitOptions<'a>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointUpdateOutput {
    checkpoint: tracking::Checkpoint,
}

pub async fn handle_checkpoint_update(
    cfg: &Config,
    input: &CheckpointUpdateInput<'_>,
    work_dir: &path::Path,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    match cfg.change_provider.r#use {
        ChangeProviderKind::Git => {
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
    let mut checkpoint = match tracking.open_checkpoint() {
        Ok(cp) => cp,
        Err(MonorailError::TrackingCheckpointNotFound(_)) => tracking.new_checkpoint(),
        // TODO: need to set path on checkpoint tho; don't use default
        Err(e) => {
            return Err(e);
        }
    };
    checkpoint.id = "head".to_string();
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
    checkpoint.save()?;

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
#[derive(Debug)]
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
enum ChangeProviderKind {
    #[serde(rename = "git")]
    #[default]
    Git,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct ChangeProvider {
    r#use: ChangeProviderKind,
}

impl FromStr for ChangeProviderKind {
    type Err = MonorailError;
    fn from_str(s: &str) -> Result<ChangeProviderKind, Self::Err> {
        match s {
            "git" => Ok(ChangeProviderKind::Git),
            _ => Err(MonorailError::Generic(format!(
                "Unrecognized change provider kind: {}",
                s
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    #[serde(default = "Config::default_output_path")]
    output_dir: String,
    #[serde(default = "Config::default_max_retained_runs")]
    max_retained_runs: usize,
    #[serde(default)]
    change_provider: ChangeProvider,
    targets: Option<Vec<Target>>,
}
impl Config {
    pub fn new(file_path: &path::Path) -> Result<Config, MonorailError> {
        let file = File::open(file_path)?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf()?;
        Ok(serde_json::from_str(std::str::from_utf8(buf)?)?)
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
    fn default_max_retained_runs() -> usize {
        10
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommandDefinition {
    exec: String,
    args: Vec<String>,
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
    // Configuration and optional overrides for commands.
    #[serde(default)]
    commands: TargetCommands,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TargetCommands {
    // Relative path from this target's `path` to a directory containing
    // commands that can be executed by `monorail run`.
    path: String,
    // Mappings of command names to executable statements; these
    // statements will be used when spawning tasks, and if unspecified
    // monorail will try to use an executable named {{command}}*.
    definitions: Option<HashMap<String, CommandDefinition>>,
}
impl Default for TargetCommands {
    fn default() -> Self {
        Self {
            path: "monorail".into(),
            definitions: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    const RAW_CONFIG: &str = r#"
{
    "targets": [
        {
            "path": "rust"
        },
        {
            "path": "rust/target",
            "ignores": [
                "rust/target/ignoreme.txt"
            ],
            "uses": [
                "rust/vendor",
                "common"
            ]
        }
    ]
}
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
                    id: "".to_string(),
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
                    id: first_head.clone(),
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
                    id: first_head.clone(),
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
                    id: second_head.clone(),
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
                    id: second_head.clone(),
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
                id: "test".to_string(),
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
                    id: start.clone(),
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
                    id: start.clone(),
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
                    id: end.clone(),
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
                    id: end.clone(),
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

        // files that don't exist have an empty checksum
        let p = &repo_path.join("test.txt");
        assert_eq!(get_file_checksum(p).await.unwrap(), "".to_string());

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
        let c: Config = serde_json::from_str(RAW_CONFIG).unwrap();

        create_file(
            &repo_path,
            "rust",
            "monorail.sh",
            b"command whoami { echo 'rust' }",
        )
        .await;
        create_file(
            &repo_path,
            "rust/target",
            "monorail.sh",
            b"command whoami { echo 'rust/target' }",
        )
        .await;
        (c, repo_path)
    }

    #[tokio::test]
    async fn test_analyze_empty() {
        let changes = vec![];
        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let ai = AnalyzeInput::new(true, false, false);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Ignores,
                },
            ]),
        }];

        let (c, work_dir) = prep_raw_config_repo().await;
        let mut lookups = Lookups::new(&c, &c.get_target_path_set(), &work_dir).unwrap();
        let ai = AnalyzeInput::new(true, true, true);
        let o = analyze(&ai, &mut lookups, Some(changes)).unwrap();

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
{
    "targets": [
        { "path": "rust" },
        { "path": "rust" }
    ]
}
"#;
        let c: Config = serde_json::from_str(config_str).unwrap();
        let work_dir = std::env::current_dir().unwrap();
        assert!(Lookups::new(&c, &c.get_target_path_set(), &work_dir).is_err());
    }
}
