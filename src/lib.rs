pub mod common;
mod error;
mod graph;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::{env, fs, path};

use std::io::Write;

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use trie_rs::{Trie, TrieBuilder};

use crate::error::{GraphError, MonorailError};

const CHECKPOINT_PREFIX: &str = "monorail-";
pub const HANDLE_OK: i32 = 0;
pub const HANDLE_ERR: i32 = 1;
pub const HANDLE_FATAL: i32 = 2;

#[inline(always)]
fn get_code(is_err: bool) -> i32 {
    if is_err {
        return HANDLE_ERR;
    }
    HANDLE_OK
}

pub fn handle(matches: &clap::ArgMatches, output_format: &str) -> Result<i32, MonorailError> {
    let wd: String = match matches.get_one::<String>("working-directory") {
        Some(wd) => wd.into(),
        None => {
            let pb = env::current_dir()?;
            let s = pb.to_str();
            match s {
                Some(s) => s.to_string(),
                None => {
                    return Err(MonorailError::from("Failed to get current dir"));
                }
            }
        }
    };

    match matches.get_one::<String>("config-file") {
        Some(cfg_path) => {
            let mut cfg = Config::new(Path::new(&wd).join(cfg_path).to_str().unwrap_or(cfg_path))?;
            cfg.workdir.clone_from(&wd);
            cfg.validate()?;

            if let Some(_config) = matches.subcommand_matches("config") {
                write_result(&Ok(cfg), output_format)?;
                return Ok(HANDLE_OK);
            }
            if let Some(checkpoint) = matches.subcommand_matches("checkpoint") {
                if let Some(create) = checkpoint.subcommand_matches("create") {
                    let res = handle_checkpoint_create(
                        &cfg,
                        HandleCheckpointCreateInput {
                            dry_run: create.get_flag("dry-run"),
                            git_path: create.get_one::<String>("git-path").unwrap(),
                            use_libgit2_status: create.get_flag("use-libgit2-status"),
                        },
                        &wd,
                    );
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
                if let Some(update) = checkpoint.subcommand_matches("update") {
                    let i = HandleCheckpointUpdateInput::try_from(update)?;
                    let res = handle_checkpoint_update(&cfg, &i, &wd);
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(target) = matches.subcommand_matches("target") {
                if let Some(list) = target.subcommand_matches("list") {
                    let res = handle_target_list(
                        &cfg,
                        HandleTargetListInput {
                            show_target_groups: list.get_flag("show-target-groups"),
                        },
                    );
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(analyze) = matches.subcommand_matches("analyze") {
                let i = AnalyzeInput::from(analyze);
                let res = handle_analyze(&cfg, &i, &wd);
                write_result(&res, output_format)?;
                return Ok(get_code(res.is_err()));
            }

            if let Some(run) = matches.subcommand_matches("run") {
                let i = RunInput::try_from(run).unwrap();
                match handle_run2(&cfg, &i, &wd) {
                    Ok(o) => {
                        let mut code = HANDLE_OK;
                        if o.failed {
                            code = HANDLE_ERR;
                        }
                        write_result(&Ok(o), output_format)?;
                        return Ok(code);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            Err(MonorailError::from("Command not recognized"))
        }
        None => Err(MonorailError::from("No configuration specified")),
    }
}

pub fn write_result<T>(
    value: &Result<T, MonorailError>,
    output_format: &str,
) -> Result<(), MonorailError>
where
    T: Serialize,
{
    match output_format {
        "json" => {
            match value {
                Ok(t) => {
                    let mut writer = std::io::stdout();
                    serde_json::to_writer(&mut writer, &t)?;
                    writeln!(writer)?;
                }
                Err(e) => {
                    let mut writer = std::io::stderr();
                    serde_json::to_writer(&mut writer, &e)?;
                    writeln!(writer)?;
                }
            }
            Ok(())
        }
        _ => Err(MonorailError::Generic(format!(
            "Unrecognized output format {}",
            output_format
        ))),
    }
}

#[derive(Debug)]
struct GitChangeOptions<'a> {
    start: Option<&'a str>,
    end: Option<&'a str>,
    git_path: &'a str,
    use_libgit2_status: bool,
}

#[derive(Debug)]
struct RunInput<'a> {
    git_change_options: GitChangeOptions<'a>,
    functions: Vec<&'a String>,
}

/*

{
    "results:": [
        {
            "function": "prep",
            "successes": [
                {
                    "target": "api/rust",
                    "code": 0,
                    "stdout_log_file": "/Users/.../stdout"
                    "stderr_log_file": "/Users/.../stderr"
                }
            ],
            "failures": []
        }
    ]
}
*/
#[derive(Debug, Serialize)]
struct RunOutput<'a> {
    failed: bool,
    results: Vec<FunctionRunResult<'a>>,
}
#[derive(Debug, Serialize)]
struct FunctionRunResult<'a> {
    function: &'a str,
    successes: Vec<TargetRunSuccess>,
    failures: Vec<TargetRunFailure>,
}
#[derive(Debug, Serialize)]
struct TargetRunSuccess {
    target: String,
    code: Option<i32>,
    stdout_log_path: path::PathBuf,
    stderr_log_path: path::PathBuf,
}
impl TargetRunSuccess {
    fn new(
        target: String,
        code: Option<i32>,
        stdout_log_path: path::PathBuf,
        stderr_log_path: path::PathBuf,
    ) -> Self {
        Self {
            target,
            code,
            stdout_log_path,
            stderr_log_path,
        }
    }
}
#[derive(Debug, Serialize)]
struct TargetRunFailure {
    target: String,
    code: Option<i32>,
    stdout_log_path: Option<path::PathBuf>,
    stderr_log_path: Option<path::PathBuf>,
    error: MonorailError,
}
impl TargetRunFailure {
    fn new(
        target: String,
        code: Option<i32>,
        stdout_log_path: Option<path::PathBuf>,
        stderr_log_path: Option<path::PathBuf>,
        error: MonorailError,
    ) -> Self {
        Self {
            target,
            code,
            stdout_log_path,
            stderr_log_path,
            error,
        }
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for RunInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_change_options: GitChangeOptions {
                start: cmd.get_one::<String>("start").map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>("end").map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>("git-path").unwrap(),
                use_libgit2_status: cmd.get_flag("use-libgit2-status"),
            },
            functions: cmd
                .get_many::<String>("function")
                .ok_or_else(|| MonorailError::from("No functions provided"))
                .into_iter()
                .flatten()
                .collect(),
        })
    }
}

// <workdir>/<monorail-out>/tracking
fn get_tracking_path(workdir: &str) -> path::PathBuf {
    Path::new(workdir).join("monorail-out").join("tracking")
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

    Ok(format!("{}", hex::encode(hasher.finalize())))
}

async fn compare_checksum(pending: &HashMap<String, String>, workdir: &str, name: &str) -> bool {
    if let Some(checksum) = pending.get(name) {
        // compute checksum of x.name and check not equal
        match get_file_checksum(&Path::new(workdir).join(name)).await {
            Ok(x_checksum) => checksum == &x_checksum,
            Err(e) => false,
        }
    } else {
        false
    }
}

async fn get_filtered_changes(
    changes: Vec<RawChange>,
    pending: &HashMap<String, String>,
    workdir: &str,
) -> Vec<RawChange> {
    tokio_stream::iter(changes)
        .then(|x| async {
            if compare_checksum(&pending, workdir, &x.name).await {
                None
            } else {
                Some(x)
            }
        })
        .filter_map(|x| x)
        .collect()
        .await
}

// Fetch status changes, filter them using the tracking checkpoint if present.
async fn get_git_status_changes(
    repo: &git2::Repository,
    git_opts: &GitChangeOptions<'_>,
    checkpoint: &Option<TrackingCheckpoint<'_>>,
    workdir: &str,
) -> Result<Vec<RawChange>, MonorailError> {
    let mut status_changes = match git_opts.use_libgit2_status {
        true => libgit2_status_changes(repo)?,
        false => git_cmd_status_changes(git_cmd_status(git_opts.git_path, repo.workdir())?),
    };

    // use checkpoint pending to filter status changes, if present
    if let Some(checkpoint) = checkpoint {
        if let Some(pending) = &checkpoint.pending {
            if !pending.is_empty() {
                return Ok(get_filtered_changes(status_changes, &pending, workdir).await);
            }
        }
    }
    Ok(status_changes)
}

// Fetch diff changes, using the tracking checkpoint commit if present.
async fn get_git_diff_changes<'a>(
    repo: &'a git2::Repository,
    git_opts: &'a GitChangeOptions<'a>,
    checkpoint: &'a Option<TrackingCheckpoint<'a>>,
    workdir: &str,
) -> Result<Option<Vec<RawChange>>, MonorailError> {
    // first check git_opts.start; if provided, use that
    let start_oid_opt = match git_opts.start {
        Some(start) => Some(repo.revparse_single(start)?.id()),
        None => {
            // otherwise, check checkpoint.commit; if provided, use that
            match checkpoint {
                Some(checkpoint) => {
                    if checkpoint.commit != "" {
                        Some(repo.revparse_single(&checkpoint.commit)?.id())
                    } else {
                        None
                    }
                }
                // if not then there's nowhere to start from, and we're done
                None => {
                    return Ok(None);
                }
            }
        }
    };

    match start_oid_opt {
        Some(start_oid) => {
            let end_oid = repo.revparse_single(git_opts.end.unwrap_or("HEAD"))?.id();
            let diff_changes = libgit2_start_end_diff_changes(repo, start_oid, end_oid)?;
            if let Some(checkpoint) = checkpoint {
                if let Some(pending) = &checkpoint.pending {
                    if !pending.is_empty() {
                        return Ok(Some(
                            get_filtered_changes(diff_changes, &pending, workdir).await,
                        ));
                    }
                }
            }
            Ok(Some(diff_changes))
        }
        None => Ok(None),
    }
}

async fn get_git_all_changes<'a>(
    repo: &'a git2::Repository,
    git_change_opts: &'a GitChangeOptions<'a>,
    checkpoint: &'a Option<TrackingCheckpoint<'a>>,
    workdir: &str,
) -> Result<Option<Vec<RawChange>>, MonorailError> {
    match get_git_diff_changes(repo, git_change_opts, checkpoint, workdir).await? {
        Some(mut diff_changes) => {
            // append status changes
            let mut status_changes =
                get_git_status_changes(repo, git_change_opts, checkpoint, workdir).await?;
            status_changes.append(&mut diff_changes);
            status_changes.sort();
            status_changes.dedup();
            Ok(Some(status_changes))
        }
        None => {
            // no provided start or checkpoint start; we will analyze all targets
            Ok(None)
        }
    }
}

fn handle_run2<'a>(
    config: &'a Config,
    input: &'a RunInput<'a>,
    workdir: &'a str,
) -> Result<RunOutput<'a>, MonorailError> {
    let targets = config
        .targets
        .as_ref()
        .ok_or(MonorailError::from("No configured targets"))?;
    let mut lookups = Lookups::new(config)?;
    // todo no runtime here, make outer fn async
    // TODO: not needed if targeting directly with --targets
    let rt = tokio::runtime::Runtime::new()?;
    let tracking = rt.block_on(Tracking::open(&get_tracking_path(workdir)))?;
    let checkpoint = Some(match rt.block_on(tracking.open_checkpoint()) {
        Ok(checkpoint) => checkpoint,
        Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
        Err(e) => {
            return Err(e);
        }
    });

    let changes = match config.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(workdir)?;
            rt.block_on(get_git_all_changes(
                &repo,
                &input.git_change_options,
                &checkpoint,
                workdir,
            ))?
        }
    };
    // END TODO
    let ao = analyze(&mut lookups, changes, false, false, true)?;
    let target_groups = ao
        .target_groups
        .ok_or(MonorailError::from("No target groups found"))?;
    run(
        &lookups,
        workdir,
        config.checksum,
        "stateid",
        "todochangeme",
        &input.functions,
        targets.clone(),
        &target_groups,
    )
}

#[derive(Debug)]
struct TrackingCheckpoint<'a> {
    path: &'a path::Path,
    commit: String,
    pending: Option<HashMap<String, String>>,
}
impl<'a> TrackingCheckpoint<'a> {
    fn new(file_path: &'a path::Path) -> Self {
        Self {
            path: file_path,
            commit: String::new(),
            pending: None,
        }
    }
    // Open the internal file and read its contents.
    async fn open(file_path: &'a path::Path) -> Result<Self, MonorailError> {
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(file_path)
            .await
            .map_err(|e| MonorailError::CheckpointNotFound(e))?;
        // Read the contents of the existing file into runtime structures.
        let mut br = tokio::io::BufReader::new(file).lines();

        // The first line contains the sha of the latest tracked commit
        let commit: String = br.next_line().await?.ok_or(MonorailError::Generic(format!(
            "Tracking checkpoint {} is missing a commit",
            file_path.display()
        )))?;

        // The remaining lines contain pairs of <path> <content sha>
        let mut pending = HashMap::new();
        while let Some(line) = br.next_line().await? {
            let parts: Vec<&str> = line.split(" ").collect();
            if parts.len() != 2 {
                return Err(MonorailError::Generic(format!(
                    "Tracking checkpoint 'pending' is malformed: {}",
                    line
                )));
            }
            pending.insert(parts[0].to_owned(), parts[1].to_owned());
        }
        Ok(Self {
            path: file_path,
            commit,
            pending: if !pending.is_empty() {
                Some(pending)
            } else {
                None
            },
        })
    }

    // Copy all current state into the file.
    async fn save(&mut self) -> Result<(), MonorailError> {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)
            .await?;
        // write commit and pending
        let mut bw = tokio::io::BufWriter::new(file);
        bw.write(self.commit.as_bytes()).await?;
        bw.write(&[10_u8]).await?;

        if let Some(pending) = &self.pending {
            for (key, val) in pending.iter() {
                let mut line = key.to_owned();
                line.push_str(" ");
                line.push_str(val);
                bw.write(line.as_bytes()).await?;
                bw.write(&[10_u8]).await?;
            }
        }

        bw.flush().await.map_err(|e| MonorailError::Io(e))
    }
}
#[derive(Debug)]
struct Tracking {
    checkpoint_path: path::PathBuf,
    lock_path: path::PathBuf,
    lock_file: tokio::fs::File,
}
impl<'a> Tracking {
    // Open a lockfile and return the table ready for use.
    async fn open(dir_path: &'a path::Path) -> Result<Self, MonorailError> {
        std::fs::create_dir_all(dir_path)?;
        let lock_path = dir_path.join("LOCKFILE");
        let lock_file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await?;
        Ok(Self {
            checkpoint_path: dir_path.join("CHECKPOINT"),
            lock_path,
            lock_file,
        })
    }
    fn new_checkpoint(&'a self) -> TrackingCheckpoint {
        TrackingCheckpoint::new(&self.checkpoint_path)
    }
    async fn open_checkpoint(&'a self) -> Result<TrackingCheckpoint, MonorailError> {
        TrackingCheckpoint::open(&self.checkpoint_path).await
    }
}
impl Drop for Tracking {
    fn drop(&mut self) {
        std::fs::remove_file(&self.lock_path).expect(&format!(
            "Error removing lockfile {}",
            self.lock_path.display()
        ));
    }
}

fn run<'a>(
    lookups: &Lookups,
    workdir: &str,
    config_checksum: u32,
    state_id: &str,
    log_id: &str,
    functions: &'a [&'a String],
    targets: Vec<Target>,
    target_groups: &Vec<Vec<String>>,
) -> Result<RunOutput<'a>, MonorailError> {
    let mut o = RunOutput {
        failed: false,
        results: vec![],
    };
    let rt = tokio::runtime::Runtime::new()?;
    for f in functions.iter() {
        for group in target_groups {
            let mut tuples: Vec<(String, String, String)> = vec![];
            for target_path in group {
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
                let script_path = std::path::Path::new(&target_path).join(&target.run);
                // TODO: check file exists
                tuples.push((
                    target_path.to_owned(),
                    script_path
                        .to_str()
                        .ok_or(MonorailError::from("Run file not found"))?
                        .to_owned(),
                    f.to_string(),
                ));
            }
            let res: Result<FunctionRunResult<'_>, MonorailError> = rt.block_on(async move {
                let mut js = tokio::task::JoinSet::new();
                for (target_path, script_path, func) in tuples {
                    // TODO: check file type to pick command
                    let lp = LogPaths::new(
                        workdir,
                        "monorail-out/logs",
                        log_id,
                        func.as_str(),
                        &target_path,
                    );
                    js.spawn(run_bash_command(
                        workdir.to_owned(),
                        target_path.to_owned(),
                        script_path,
                        func.to_owned(),
                        lp,
                    ));
                }
                let mut frr = FunctionRunResult {
                    function: f.as_str(),
                    successes: vec![],
                    failures: vec![],
                };
                while let Some(join_res) = js.join_next().await {
                    let res = join_res?;
                    match res {
                        Ok((status, target_path, lp)) => {
                            if status.success() {
                                frr.successes.push(TargetRunSuccess::new(
                                    target_path,
                                    status.code(),
                                    lp.stdout_log_path,
                                    lp.stderr_log_path,
                                ));
                            } else {
                                // TODO: --halt-on-first-failure
                                let error = MonorailError::from("Function returned an error");
                                frr.failures.push(TargetRunFailure::new(
                                    target_path,
                                    status.code(),
                                    Some(lp.stdout_log_path),
                                    Some(lp.stderr_log_path),
                                    error,
                                ));
                            }
                        }
                        Err((target_path, e)) => {
                            // TODO: --halt-on-first-failure
                            let error = MonorailError::Generic(format!(
                                "Function could not execute: {}",
                                e
                            ));
                            frr.failures.push(TargetRunFailure::new(
                                target_path,
                                None,
                                None,
                                None,
                                error,
                            ));
                        }
                    }
                }
                Ok(frr)
            });
            let frr = res?;
            if !frr.failures.is_empty() {
                o.failed = true;
            }
            o.results.push(frr);
        }
    }
    Ok(o)
}

struct LogPaths {
    dir_path: path::PathBuf,
    stdout_log_path: path::PathBuf,
    stderr_log_path: path::PathBuf,
}
impl LogPaths {
    fn new(workdir: &str, logdir: &str, id: &str, function: &str, target: &str) -> Self {
        let dir_path = path::Path::new(workdir)
            .join(logdir)
            .join(id)
            .join(function)
            .join(target);
        let stdout_log_path = dir_path.clone().join("stdout");
        let stderr_log_path = dir_path.clone().join("stderr");
        Self {
            dir_path,
            stdout_log_path,
            stderr_log_path,
        }
    }
    fn open(&self) -> Result<(fs::File, fs::File), MonorailError> {
        std::fs::create_dir_all(&self.dir_path)?;
        let stdout_log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.stdout_log_path)
            .map_err(|e| MonorailError::Generic(e.to_string()))?;

        let stderr_log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.stderr_log_path)
            .map_err(|e| MonorailError::Generic(e.to_string()))?;
        Ok((stdout_log_file, stderr_log_file))
    }
}

// TODO: LogPaths -> TargetPaths and include all paths in it
// TODO: clean up this returning of the string target_path
async fn run_bash_command(
    workdir: String,
    target_path: String,
    script: String,
    function: String,
    lp: LogPaths,
) -> Result<(std::process::ExitStatus, String, LogPaths), (String, MonorailError)> {
    let (stdout_log_file, stderr_log_file) = lp
        .open()
        .map_err(|e| (target_path.clone(), MonorailError::from(e)))?;
    let mut cmd = tokio::process::Command::new("bash")
        .arg("-c")
        .arg(format!("source $(pwd)/{} && {}", script, function))
        .current_dir(workdir)
        .stdout(stdout_log_file)
        .stderr(stderr_log_file)
        // parallel execution makes use of stdin impractical
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| (target_path.clone(), MonorailError::from(e)))?;
    Ok((
        cmd.wait()
            .await
            .map_err(|e| (target_path.clone(), MonorailError::from(e)))?,
        target_path,
        lp,
    ))
}

#[derive(Debug)]
struct AnalyzeInput<'a> {
    git_change_options: GitChangeOptions<'a>,
    show_changes: bool,
    show_change_targets: bool,
    show_target_groups: bool,
}

impl<'a> From<&'a clap::ArgMatches> for AnalyzeInput<'a> {
    fn from(cmd: &'a clap::ArgMatches) -> Self {
        Self {
            git_change_options: GitChangeOptions {
                start: cmd.get_one::<String>("start").map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>("end").map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>("git-path").unwrap(),
                use_libgit2_status: cmd.get_flag("use-libgit2-status"),
            },
            show_changes: cmd.get_flag("show-changes"),
            show_change_targets: cmd.get_flag("show-change-targets"),
            show_target_groups: cmd.get_flag("show-target-groups"),
        }
    }
}
#[derive(Serialize, Debug)]
struct AnalyzeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<AnalyzedChange>>,
    targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_groups: Option<Vec<Vec<String>>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
struct AnalyzedChange {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Vec<AnalyzedChangeTarget>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
struct AnalyzedChangeTarget {
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

// TODO: use checkpoint
fn handle_analyze<'a>(
    config: &'a Config,
    input: &AnalyzeInput<'a>,
    workdir: &'a str,
) -> Result<AnalyzeOutput, MonorailError> {
    let mut lookups = Lookups::new(config)?;
    match config.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(workdir)?;
            let changes = git_get_raw_changes(&repo, &input.git_change_options)?;
            let mut lookups = Lookups::new(config)?;
            // todo no runtime here, make outer fn async
            let rt = tokio::runtime::Runtime::new()?;
            let tracking = rt.block_on(Tracking::open(&get_tracking_path(workdir)))?;
            let checkpoint = Some(match rt.block_on(tracking.open_checkpoint()) {
                Ok(checkpoint) => checkpoint,
                Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
                Err(e) => {
                    return Err(e);
                }
            });

            let changes = match config.vcs.r#use {
                VcsKind::Git => {
                    let repo = git2::Repository::open(workdir)?;
                    rt.block_on(get_git_all_changes(
                        &repo,
                        &input.git_change_options,
                        &checkpoint,
                        workdir,
                    ))?
                }
            };
            analyze(
                &mut lookups,
                changes,
                input.show_changes,
                input.show_change_targets,
                input.show_target_groups,
            )
        }
    }
}

fn analyze<'a>(
    lookups: &mut Lookups<'_>,
    changes: Option<Vec<RawChange>>,
    show_changes: bool,
    show_change_targets: bool,
    show_target_groups: bool,
) -> Result<AnalyzeOutput, MonorailError> {
    let mut output = AnalyzeOutput {
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
            output.targets.push(t.to_owned());
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
        output.targets = lookups.targets.clone();
        if show_target_groups {
            output.target_groups = Some(lookups.dag.get_labeled_groups()?);
        }
    }
    output.targets.sort();

    Ok(output)
}

fn git_get_raw_changes(
    repo: &git2::Repository,
    input: &GitChangeOptions,
) -> Result<Vec<RawChange>, MonorailError> {
    let start = match input.start {
        Some(s) => repo.revparse_single(s)?.id(),
        None => {
            match libgit2_latest_monorail_tag(repo, repo.revparse_single("HEAD")?.id())? {
                Some(lr) => lr.target_id(),
                None => {
                    // no tags, use first commit of the repo
                    match libgit2_first_commit(repo)? {
                        Some(fc) => fc,
                        None => {
                            return Err(
                                "couldn't find a starting point; commit something first".into()
                            )
                        }
                    }
                }
            }
        }
    };

    let end = repo.revparse_single(input.end.unwrap_or("HEAD"))?.id();
    git_all_changes(repo, start, end, input.use_libgit2_status, input.git_path)
}

#[derive(Debug, Serialize)]
struct HandleTargetListInput {
    show_target_groups: bool,
}
#[derive(Debug, Serialize)]
struct TargetListOutput {
    targets: Option<Vec<Target>>,
    target_groups: Option<Vec<Vec<String>>>,
}

fn handle_target_list(
    cfg: &Config,
    input: HandleTargetListInput,
) -> Result<TargetListOutput, MonorailError> {
    let mut o = TargetListOutput {
        targets: cfg.targets.clone(),
        target_groups: None,
    };
    if input.show_target_groups {
        let mut lookups = Lookups::new(cfg)?;
        o.target_groups = Some(lookups.dag.get_labeled_groups()?);
    }
    Ok(o)
}

#[derive(Debug)]
struct HandleCheckpointUpdateInput<'a> {
    pending: bool,
    git_change_options: GitChangeOptions<'a>,
}
impl<'a> TryFrom<&'a clap::ArgMatches> for HandleCheckpointUpdateInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_change_options: GitChangeOptions {
                start: None,
                end: None,
                git_path: cmd
                    .get_one::<String>("git-path")
                    .ok_or(MonorailError::from("Missing 'git-path'"))?,
                use_libgit2_status: cmd.get_flag("use-libgit2-status"),
            },
            pending: cmd.get_flag("pending"),
        })
    }
}
#[derive(Debug, Serialize)]
struct CheckpointUpdateOutput {
    commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<HashMap<String, String>>,
}

fn handle_checkpoint_update(
    cfg: &Config,
    input: &HandleCheckpointUpdateInput,
    workdir: &str,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    match cfg.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(workdir)?;
            let rt = tokio::runtime::Runtime::new()?;
            let tracking = rt.block_on(Tracking::open(&get_tracking_path(workdir)))?;
            let mut checkpoint = match rt.block_on(tracking.open_checkpoint()) {
                Ok(cp) => cp,
                Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
                // TODO: need to set path on checkpoint tho; don't use default
                Err(e) => {
                    return Err(e);
                }
            };
            // let mut checkpoint = Some(rt.block_on(tracking.open_checkpoint())?);
            checkpoint.commit = repo.revparse_single("HEAD")?.id().to_string();
            if input.pending {
                // TODO: add uncommitted/unstaged files and checksums to the checkpoint
                let mut status_changes = rt.block_on(get_git_status_changes(
                    &repo,
                    &input.git_change_options,
                    &None,
                    workdir,
                ))?;
                if !status_changes.is_empty() {
                    let mut pending = HashMap::new();
                    for change in status_changes.iter() {
                        dbg!(&change);
                        let p = Path::new(workdir).join(&change.name);
                        pending.insert(change.name.clone(), rt.block_on(get_file_checksum(&p))?);
                    }
                    checkpoint.pending = Some(pending);
                }
            }
            rt.block_on(checkpoint.save())?;
            Ok(CheckpointUpdateOutput {
                commit: checkpoint.commit,
                pending: checkpoint.pending,
            })
        }
    }
}

#[derive(Debug, Serialize)]
struct HandleCheckpointCreateInput<'a> {
    dry_run: bool,
    git_path: &'a str,
    use_libgit2_status: bool,
}
#[derive(Debug, Serialize)]
struct CheckpointCreateOutput {
    id: String,
    targets: Vec<String>,
    dry_run: bool,
}

fn handle_checkpoint_create(
    cfg: &Config,
    input: HandleCheckpointCreateInput,
    wd: &str,
) -> Result<CheckpointCreateOutput, MonorailError> {
    match cfg.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(wd)?;

            // require that HEAD point to the configured trunk branch
            let trunk_branch = repo.find_branch(&cfg.vcs.git.trunk, git2::BranchType::Local)?;

            if !trunk_branch.is_head() {
                return Err(format!(
                    "HEAD points to {} expected vcs.git.trunk branch {}",
                    repo.head()?.name().unwrap_or(""),
                    &cfg.vcs.git.trunk
                )
                .into());
            }

            let end_oid = repo.revparse_single("HEAD")?.id();
            let latest_tag = libgit2_latest_monorail_tag(&repo, end_oid)?;
            let start_oid = match &latest_tag {
                Some(lt) => lt.target_id(),
                None => {
                    // no tags, use first commit of the repo
                    match libgit2_first_commit(&repo)? {
                        Some(fc) => fc,
                        None => {
                            return Err(
                                "couldn't find a starting point; commit something first".into()
                            )
                        }
                    }
                }
            };

            if end_oid == start_oid {
                return Err(format!(
                    "HEAD and the last checkpoint are the same commit {}, nothing to do",
                    start_oid
                )
                .into());
            }

            // TODO: error on [ci skip]

            // TODO: error if checkpoint would include unpushed changes

            // fetch changed targets
            let changes = git_all_changes(
                &repo,
                start_oid,
                end_oid,
                input.use_libgit2_status,
                input.git_path,
            )?;
            let mut lookups = Lookups::new(cfg)?;
            let o = analyze(&mut lookups, Some(changes), true, false, false)?;

            if let Some(ref changes) = o.changes {
                if changes.is_empty() {
                    return Err("No changes detected, aborting checkpoint creation".into());
                }
            }

            // get new tag name
            let tag_name = match latest_tag {
                Some(latest_tag) => match latest_tag.name() {
                    Some(id) => increment_checkpoint_id(id)?,
                    None => return Err("checkpoint tag has no name".into()),
                },
                None => format!("{}1", CHECKPOINT_PREFIX),
            };

            if !input.dry_run {
                // generate the checkpoint message body
                let checkpoint_msg = CheckpointMessage {
                    num_changes: o.changes.map_or_else(|| 0, |v| v.len()),
                    targets: &o.targets,
                };
                let mut checkpoint_msg_str = serde_json::to_string_pretty(&checkpoint_msg)?;
                checkpoint_msg_str.push('\n');

                // create tag and push
                repo.tag(
                    &tag_name,
                    &repo.find_object(end_oid, None)?,
                    &repo.signature()?,
                    &checkpoint_msg_str,
                    false,
                )?;
                // NOTE: shelling out to `git` to avoid having to do a full remote/auth/ssh integration with libgit, for now
                let output = std::process::Command::new(input.git_path)
                    .arg("-C")
                    .arg(wd)
                    .arg("push")
                    .arg(&cfg.vcs.git.remote)
                    .arg(&format!(
                        "{}/{}",
                        cfg.vcs.git.tags_refspec_prefix, &tag_name
                    ))
                    .output()
                    .expect("failed to push tags");
                if !output.status.success() {
                    // clean up the unpushable local tag
                    match repo.tag_delete(&tag_name) {
                        Ok(()) => {
                            return Err(format!(
                                "failed to push tags: {}",
                                std::str::from_utf8(&output.stderr)?
                            )
                            .into());
                        }
                        Err(e) => {
                            return Err(format!(
                                "failed to push tags: {}, and failed to delete local tag: {}",
                                std::str::from_utf8(&output.stderr)?,
                                e
                            )
                            .into());
                        }
                    }
                }
            }

            Ok(CheckpointCreateOutput {
                id: tag_name,
                targets: o.targets,
                dry_run: input.dry_run,
            })
        }
    }
}

// Generates a new checkpoint id given an existing one
fn increment_checkpoint_id(id: &str) -> Result<String, MonorailError> {
    match id.strip_prefix(CHECKPOINT_PREFIX) {
        Some(s) => Ok(format!(
            "{}{}",
            CHECKPOINT_PREFIX,
            s.parse::<usize>()
                .map_err(MonorailError::ParseInt)?
                .checked_add(1)
                .ok_or(MonorailError::Generic(format!(
                    "checkpoint id increment would overflow: {}",
                    s
                )))?
        )),
        None => Err(format!(
            "expected checkpoint id with prefix '{}', got {}",
            CHECKPOINT_PREFIX, id
        )
        .into()),
    }
}

#[derive(Serialize)]
struct CheckpointMessage<'a> {
    num_changes: usize,
    targets: &'a [String],
}

fn handle_cmd_output(
    output: std::io::Result<std::process::Output>,
) -> Result<Vec<u8>, MonorailError> {
    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(output.stdout)
            } else {
                Err(String::from_utf8_lossy(&output.stderr).to_string().into())
            }
        }
        Err(e) => Err(e.to_string().into()),
    }
}

fn git_cmd_status(
    git_path: &str,
    workdir: Option<&std::path::Path>,
) -> Result<Vec<u8>, MonorailError> {
    handle_cmd_output(
        std::process::Command::new(git_path)
            .args([
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--renames",
            ])
            .current_dir(workdir.unwrap_or(&std::env::current_dir()?))
            .output(),
    )
}

fn git_cmd_status_changes(s: Vec<u8>) -> Vec<RawChange> {
    let mut v: Vec<RawChange> = Vec::new();
    if !s.is_empty() {
        let iter = s.split(|x| char::from(*x) == '\n');
        for w in iter {
            let mut parts: Vec<Cow<str>> = w
                .split(|z| char::from(*z) == ' ')
                .map(String::from_utf8_lossy)
                .collect();
            parts.retain(|z| z != "");
            // not interested in anything else spurious from
            // our parsing of line format
            if parts.len() == 2 {
                v.push(RawChange {
                    name: parts[1].to_string(),
                });
            }
        }
        return v;
    }
    v
}

// given a commit, find the earliest annotated monorail tag behind it
// NOTE: this is pub for now, since it's used in integration tests;
// once the `checkpoint list` command is done, make this private
// and move integration tests to the command
pub fn libgit2_latest_monorail_tag(
    repo: &git2::Repository,
    oid: git2::Oid,
) -> Result<Option<git2::Tag>, git2::Error> {
    let o = repo.find_object(oid, None)?;

    let mut dopts = git2::DescribeOptions::new();
    dopts.pattern(format!("{}*[0-9]", CHECKPOINT_PREFIX).as_str());
    let d = o.describe(&dopts);
    match d {
        Ok(d) => {
            let mut fo = git2::DescribeFormatOptions::new();
            fo.abbreviated_size(0);
            let s = d.format(Some(&fo))?;

            let r = repo.resolve_reference_from_short_name(&s)?;
            Ok(Some(r.peel_to_tag()?))
        }
        Err(_) => Ok(None),
    }
}

// git rev-list --max-parents=0 HEAD
fn libgit2_first_commit(repo: &git2::Repository) -> Result<Option<git2::Oid>, MonorailError> {
    let mut rw = repo.revwalk()?;
    match rw.push_head() {
        Ok(_) => (),
        Err(_e) => return Ok(None),
    }
    match rw.last() {
        Some(rw) => Ok(Some(rw?)),
        None => Ok(None),
    }
}

fn libgit2_start_end_diff_changes(
    repo: &git2::Repository,
    start_oid: git2::Oid,
    end_oid: git2::Oid,
) -> Result<Vec<RawChange>, git2::Error> {
    let start_tree = repo.find_object(start_oid, None)?.peel_to_tree()?;
    let end_tree = repo.find_object(end_oid, None)?.peel_to_tree()?;

    let mut opts = git2::DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&start_tree), Some(&end_tree), Some(&mut opts))?;
    let mut v: Vec<RawChange> = Vec::new();
    let diffres = diff.foreach(
        &mut |dd: git2::DiffDelta, _num: f32| -> bool {
            // TODO: prevent double pushing for new files
            if let Some(path) = dd.old_file().path() {
                if let Some(s) = path.to_str() {
                    v.push(RawChange {
                        name: s.to_string(),
                    });
                }
            }
            if let Some(path) = dd.new_file().path() {
                if let Some(s) = path.to_str() {
                    v.push(RawChange {
                        name: s.to_string(),
                    });
                }
            }
            true
        },
        None,
        None,
        None,
    );
    match diffres {
        Ok(()) => Ok(v),
        Err(e) => Err(e),
    }
}

fn libgit2_status_changes(repo: &git2::Repository) -> Result<Vec<RawChange>, git2::Error> {
    let mut v: Vec<RawChange> = Vec::new();
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true);
    opts.renames_from_rewrites(true);
    opts.recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    for s in statuses.iter() {
        if let Some(path) = s.path() {
            v.push(RawChange {
                name: path.to_string(),
            });
        }
    }
    Ok(v)
}

fn git_all_changes(
    repo: &git2::Repository,
    start_oid: git2::Oid,
    end_oid: git2::Oid,
    use_libgit2: bool,
    git_path: &str,
) -> Result<Vec<RawChange>, MonorailError> {
    let mut changes = libgit2_start_end_diff_changes(repo, start_oid, end_oid)?;

    let mut status = match use_libgit2 {
        true => libgit2_status_changes(repo)?,
        false => git_cmd_status_changes(git_cmd_status(git_path, repo.workdir())?),
    };
    changes.append(&mut status);

    changes.sort();
    changes.dedup();
    Ok(changes)
}

#[inline(always)]
fn require_existence(workdir: &str, path: &str) -> Result<(), MonorailError> {
    let p = Path::new(workdir).join(path);
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

struct Lookups<'a> {
    targets: Vec<String>,
    targets_trie: Trie<u8>,
    ignores: Trie<u8>,
    uses: Trie<u8>,
    use2targets: HashMap<&'a str, Vec<&'a str>>,
    ignore2targets: HashMap<&'a str, Vec<&'a str>>,
    dag: graph::Dag,
}
impl<'a> Lookups<'a> {
    fn new(cfg: &'a Config) -> Result<Self, MonorailError> {
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
                require_existence(cfg.workdir.as_str(), target_path_str)?;
                if dag.label2node.contains_key(target_path_str) {
                    return Err(MonorailError::DependencyGraph(GraphError::DuplicateLabel(
                        target.path.to_owned(),
                    )));
                }
                dag.set_label(target.path.to_owned(), i);
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
struct RawChange {
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
struct Config {
    #[serde(default)]
    vcs: Vcs,
    targets: Option<Vec<Target>>,

    // Additional CLI config
    #[serde(skip)]
    workdir: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct Vcs {
    #[serde(default)]
    r#use: VcsKind,
    #[serde(default)]
    git: Git,
}
#[derive(Serialize, Deserialize, Debug)]
struct Git {
    #[serde(default = "Git::default_trunk")]
    trunk: String,
    #[serde(default = "Git::default_remote")]
    remote: String,
    #[serde(default = "Git::default_tags_refspec_prefix")]
    tags_refspec_prefix: String,
}
impl Git {
    fn default_trunk() -> String {
        "master".into()
    }
    fn default_remote() -> String {
        "origin".into()
    }
    fn default_tags_refspec_prefix() -> String {
        "refs/tags".into()
    }
}
impl Default for Git {
    fn default() -> Self {
        Git {
            trunk: Git::default_trunk(),
            remote: Git::default_remote(),
            tags_refspec_prefix: Git::default_tags_refspec_prefix(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Target {
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

impl Config {
    fn new(file_path: &str) -> Result<Config, MonorailError> {
        let path = Path::new(file_path);

        let file = File::open(path)?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf()?;
        let mut config: Config = toml::from_str(std::str::from_utf8(buf)?)?;
        Ok(config)
    }
    fn validate(&self) -> Result<(), MonorailError> {
        Ok(())
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

    #[test]
    fn test_libgit2_latest_monorail_tag() {
        let (repo, repo_path) = get_repo(false);
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let lt = libgit2_latest_monorail_tag(&repo, oid1).unwrap();
        assert!(lt.is_none());

        let tag_oid = repo
            .tag(
                format!("{}1", CHECKPOINT_PREFIX).as_str(),
                &repo.find_object(oid1, None).unwrap(),
                &get_signature(),
                "",
                false,
            )
            .unwrap();
        let oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &[&get_commit(&repo, oid1)],
        );
        assert_eq!(
            libgit2_latest_monorail_tag(&repo, oid2)
                .unwrap()
                .unwrap()
                .id(),
            tag_oid
        );

        purge_repo(&repo_path);
    }
    #[test]
    fn test_libgit2_first_commit() {
        let (repo, repo_path) = get_repo(false);

        assert_eq!(libgit2_first_commit(&repo).unwrap(), None);
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let _oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &[&get_commit(&repo, oid1)],
        );
        assert_eq!(libgit2_first_commit(&repo).unwrap(), Some(oid1));

        purge_repo(&repo_path);
    }

    #[test]
    fn test_libgit2_start_end_diff_changes() {
        let (repo, repo_path) = get_repo(false);

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let _f1 = create_file(&repo_path, "", "foo.txt", b"x");
        let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
        let _f2 = create_file(&repo_path, "", "bar.txt", b"y");
        let oid3 = commit_file(&repo, "bar.txt", Some("HEAD"), &[&get_commit(&repo, oid2)]);

        assert_eq!(
            libgit2_start_end_diff_changes(&repo, oid1, oid3)
                .unwrap()
                .len(),
            4
        );

        purge_repo(&repo_path);
    }
    #[test]
    fn test_git_all_changes() {
        let (repo, repo_path) = get_repo(false);

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);

        // no changes
        const USE_LIBGIT2: bool = true;
        assert_eq!(
            git_all_changes(&repo, oid1, oid1, !USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            git_all_changes(&repo, oid1, oid1, USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            0
        );

        // committed changes
        let _f1 = create_file(&repo_path, "", "foo.txt", b"x");
        let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
        let _f2 = create_file(&repo_path, "", "bar.txt", b"y");
        let oid3 = commit_file(&repo, "bar.txt", Some("HEAD"), &[&get_commit(&repo, oid2)]);
        assert_eq!(
            git_all_changes(&repo, oid1, oid3, !USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            git_all_changes(&repo, oid1, oid3, USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            2
        );

        // committed + unstaged changes
        let _f3 = create_file(&repo_path, "", "baz.txt", b"z");
        assert_eq!(
            git_all_changes(&repo, oid1, oid3, !USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            git_all_changes(&repo, oid1, oid3, USE_LIBGIT2, "git")
                .unwrap()
                .len(),
            3
        );

        purge_repo(&repo_path);
    }
    #[test]
    fn test_libgit2_status_changes() {
        let (repo, repo_path) = get_repo(false);

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);

        assert_eq!(libgit2_status_changes(&repo).unwrap().len(), 0);

        // check that unstaged changes are detected
        let fname = "foo.txt";
        let fpath = std::path::Path::new(&repo_path).join(fname);
        let mut file = File::create(fpath).unwrap();
        file.write_all(b"x").unwrap();
        assert_eq!(libgit2_status_changes(&repo).unwrap().len(), 1);

        // check that staged changes are detected
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(fname)).unwrap();
        index.write_tree().unwrap();
        let _oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &[&get_commit(&repo, oid1)],
        );
        assert_eq!(libgit2_status_changes(&repo).unwrap().len(), 0);

        // TODO: check that renames are detected

        // TODO: check that untracked directories are recursed

        purge_repo(&repo_path);
    }

    #[test]
    fn test_increment_checkpoint_id() {
        assert_eq!(
            increment_checkpoint_id(format!("{}1", CHECKPOINT_PREFIX).as_str()).unwrap(),
            format!("{}2", CHECKPOINT_PREFIX)
        );
        assert!(increment_checkpoint_id("foo").is_err());
        assert!(increment_checkpoint_id(format!("{}foo", CHECKPOINT_PREFIX).as_str()).is_err());
        assert!(increment_checkpoint_id(
            format!("{}{}", CHECKPOINT_PREFIX, std::usize::MAX).as_str()
        )
        .is_err());
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

    fn prep_raw_config_repo() -> Config {
        let (_repo, repo_path) = get_repo(false);
        let mut c: Config = toml::from_str(RAW_CONFIG).unwrap();
        c.workdir = repo_path.clone();

        create_file(
            &repo_path,
            "rust",
            "monorail.sh",
            b"function whoami { echo 'rust' }",
        );
        create_file(
            &repo_path,
            "rust/target",
            "monorail.sh",
            b"function whoami { echo 'rust/target' }",
        );
        c
    }

    #[test]
    fn test_analyze_empty() {
        let changes = vec![];
        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, false, false).unwrap();

        assert!(o.changes.unwrap().is_empty());
        assert!(o.targets.is_empty());
    }

    #[test]
    fn test_analyze_unknown() {
        let change1 = "foo.txt";
        let changes = vec![RawChange {
            name: change1.to_string(),
        }];
        let expected_targets: Vec<String> = vec![];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![]),
        }];

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![]));
    }

    #[test]
    fn test_analyze_target_file() {
        let change1 = "rust/lib.rs";
        let changes = vec![RawChange {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }
    #[test]
    fn test_analyze_target() {
        let change1 = "rust";
        let changes = vec![RawChange {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_ancestors() {
        let change1 = "rust/target/foo.txt";
        let changes = vec![RawChange {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_uses() {
        let change1 = "common/foo.txt";
        let changes = vec![RawChange {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_ignores() {
        let change1 = "rust/target/ignoreme.txt";
        let changes = vec![RawChange {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c).unwrap();
        let o = analyze(&mut lookups, changes, true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![vec![target1.to_string()]]));
    }

    #[test]
    fn test_lookups() {
        let c = prep_raw_config_repo();
        let l = Lookups::new(&c).unwrap();

        assert_eq!(
            l.targets
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
    fn test_git_cmd_status_changes() {
        let raw = r#" M .circleci/config.yml
 M Monorail.toml
 M rust/support/script/command.sh
?? go/project/tator/
?? out.json
"#;
        let changes = git_cmd_status_changes(Vec::from(raw));
        assert_eq!(
            changes,
            vec![
                RawChange {
                    name: ".circleci/config.yml".to_string()
                },
                RawChange {
                    name: "Monorail.toml".to_string()
                },
                RawChange {
                    name: "rust/support/script/command.sh".to_string()
                },
                RawChange {
                    name: "go/project/tator/".to_string()
                },
                RawChange {
                    name: "out.json".to_string()
                },
            ]
        );

        assert_eq!(git_cmd_status_changes(Vec::from("")), vec![]);
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
        assert!(Lookups::new(&c).is_err());
    }
}
