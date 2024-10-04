use crate::{graph, tracking};

use std::borrow::Cow;
use std::cmp::Ordering;
use std::{fs, path};

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::Path;

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio_stream::StreamExt;
use trie_rs::{Trie, TrieBuilder};

use crate::error::{GraphError, MonorailError};

#[derive(Debug)]
pub struct GitChangeOptions<'a> {
    pub(crate) start: Option<&'a str>,
    pub(crate) end: Option<&'a str>,
    pub(crate) git_path: &'a str,
}

#[derive(Debug)]
pub struct RunInput<'a> {
    pub(crate) git_change_options: GitChangeOptions<'a>,
    pub(crate) functions: Vec<&'a String>,
    pub(crate) targets: HashSet<&'a String>,
}

#[derive(Debug, Serialize)]
pub struct RunOutput<'a> {
    pub(crate) failed: bool,
    results: Vec<FunctionRunResult<'a>>,
}
#[derive(Debug, Serialize)]
pub struct FunctionRunResult<'a> {
    function: &'a str,
    successes: Vec<TargetRunSuccess>,
    failures: Vec<TargetRunFailure>,
}
#[derive(Debug, Serialize)]
pub struct TargetRunSuccess {
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
pub struct TargetRunFailure {
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

    Ok(hex::encode(hasher.finalize()).to_string())
}

async fn checksum_is_equal(pending: &HashMap<String, String>, workdir: &str, name: &str) -> bool {
    match pending.get(name) {
        Some(checksum) => {
            // compute checksum of x.name and check not equal
            get_file_checksum(&Path::new(workdir).join(name))
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
    workdir: &str,
) -> Vec<Change> {
    tokio_stream::iter(changes)
        .then(|x| async {
            if checksum_is_equal(pending, workdir, &x.name).await {
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
// async fn get_git_status_changes(
//     repo: &git2::Repository,
//     git_opts: &GitChangeOptions<'_>,
//     checkpoint: &Option<tracking::Checkpoint>,
//     workdir: &str,
// ) -> Result<Vec<Change>, MonorailError> {
//     let status_changes = git_cmd_status_changes(git_cmd_status(git_opts.git_path, repo.workdir())?);

//     // use checkpoint pending to filter status changes, if present
//     if let Some(checkpoint) = checkpoint {
//         if let Some(pending) = &checkpoint.pending {
//             if !pending.is_empty() {
//                 return Ok(get_filtered_changes(status_changes, pending, workdir).await);
//             }
//         }
//     }
//     Ok(status_changes)
// }

// Fetch diff changes, using the tracking checkpoint commit if present.
async fn get_git_diff_changes<'a>(
    repo: &'a git2::Repository,
    git_opts: &'a GitChangeOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    workdir: &str,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let start = git_opts.start.map(|start| start).or_else(|| {
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

    let mut diff_changes =
        git_cmd_diff_changes(git_opts.git_path, repo.workdir(), start, end).await?;
    if start.is_none() && end.is_none() && diff_changes.is_empty() {
        // no pending changes and diff range is ok, but signficant enough to be
        // represented differently from an empty vec
        Ok(None)
    } else {
        if let Some(checkpoint) = checkpoint {
            if let Some(pending) = &checkpoint.pending {
                if !pending.is_empty() {
                    return Ok(Some(
                        get_filtered_changes(diff_changes, pending, workdir).await,
                    ));
                }
            }
        }
        Ok(Some(diff_changes))
    }
}

async fn get_git_all_changes<'a>(
    repo: &'a git2::Repository,
    git_opts: &'a GitChangeOptions<'a>,
    checkpoint: &'a Option<tracking::Checkpoint>,
    workdir: &str,
) -> Result<Option<Vec<Change>>, MonorailError> {
    let mut other_changes = git_cmd_other_changes(git_opts.git_path, repo.workdir()).await?;
    match get_git_diff_changes(repo, git_opts, checkpoint, workdir).await? {
        Some(diff_changes) => {
            other_changes.extend(diff_changes);
            other_changes.sort();
            Ok(Some(other_changes))
        }
        None => {
            // no provided start or checkpoint start; we will analyze all targets
            if other_changes.is_empty() {
                Ok(None)
            } else {
                other_changes.sort();
                Ok(Some(other_changes))
            }
        }
    }
}

pub async fn handle_run<'a>(
    config: &'a Config,
    input: &'a RunInput<'a>,
    workdir: &'a str,
) -> Result<RunOutput<'a>, MonorailError> {
    let targets = config
        .targets
        .as_ref()
        .ok_or(MonorailError::from("No configured targets"))?;
    match input.targets.len() {
        0 => {
            let ths = config.get_target_path_set();
            let mut lookups = Lookups::new(config, &ths)?;
            let tracking = tracking::Table::open(&get_tracking_path(workdir)).await?;
            let checkpoint = Some(match tracking.open_checkpoint().await {
                Ok(checkpoint) => checkpoint,
                Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
                Err(e) => {
                    return Err(e);
                }
            });

            let changes = match config.vcs.r#use {
                VcsKind::Git => {
                    let repo = git2::Repository::open(workdir)?;
                    get_git_diff_changes(&repo, &input.git_change_options, &checkpoint, workdir)
                        .await?
                }
            };
            let ao = analyze(&mut lookups, changes, false, false, true)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            run(
                &lookups,
                workdir,
                "todochangeme",
                &input.functions,
                targets.clone(),
                &target_groups,
            )
            .await
        }
        _ => {
            let mut lookups = Lookups::new(config, &input.targets)?;
            let ao = analyze(&mut lookups, None, false, false, true)?;
            let target_groups = ao
                .target_groups
                .ok_or(MonorailError::from("No target groups found"))?;
            run(
                &lookups,
                workdir,
                "todochangeme",
                &input.functions,
                targets.clone(),
                &target_groups,
            )
            .await
        }
    }
}

async fn run<'a>(
    lookups: &Lookups<'_>,
    workdir: &str,
    log_id: &str,
    functions: &'a [&'a String],
    targets: Vec<Target>,
    target_groups: &Vec<Vec<String>>,
) -> Result<RunOutput<'a>, MonorailError> {
    let mut o = RunOutput {
        failed: false,
        results: vec![],
    };
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
                        let error =
                            MonorailError::Generic(format!("Function could not execute: {}", e));
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
            if !frr.failures.is_empty() {
                o.failed = true;
            }
            o.results.push(frr);
        }
    }
    Ok(o)
}

pub struct LogPaths {
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
    let (stdout_log_file, stderr_log_file) = lp.open().map_err(|e| (target_path.clone(), e))?;
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
pub struct AnalyzeInput<'a> {
    pub(crate) git_change_options: GitChangeOptions<'a>,
    pub(crate) show_changes: bool,
    pub(crate) show_change_targets: bool,
    pub(crate) show_target_groups: bool,
    pub(crate) targets: HashSet<&'a String>,
}

#[derive(Serialize, Debug)]
pub struct AnalyzeOutput {
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
    config: &'a Config,
    input: &AnalyzeInput<'a>,
    workdir: &'a str,
) -> Result<AnalyzeOutput, MonorailError> {
    let (mut lookups, changes) = match input.targets.len() {
        0 => {
            let changes = match config.vcs.r#use {
                VcsKind::Git => match config.vcs.r#use {
                    VcsKind::Git => {
                        let tracking = tracking::Table::open(&get_tracking_path(workdir)).await?;
                        let checkpoint = Some(match tracking.open_checkpoint().await {
                            Ok(checkpoint) => checkpoint,
                            Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
                            Err(e) => {
                                return Err(e);
                            }
                        });
                        let repo = git2::Repository::open(workdir)?;
                        get_git_diff_changes(&repo, &input.git_change_options, &checkpoint, workdir)
                            .await?
                    }
                },
            };
            (
                Lookups::new(config, &config.get_target_path_set())?,
                changes,
            )
        }
        _ => (Lookups::new(config, &input.targets)?, None),
    };

    analyze(
        &mut lookups,
        changes,
        input.show_changes,
        input.show_change_targets,
        input.show_target_groups,
    )
}

fn analyze(
    lookups: &mut Lookups<'_>,
    changes: Option<Vec<Change>>,
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
pub struct HandleTargetListInput {
    pub show_target_groups: bool,
}
#[derive(Debug, Serialize)]
pub struct TargetListOutput {
    targets: Option<Vec<Target>>,
    target_groups: Option<Vec<Vec<String>>>,
}

pub fn handle_target_list(
    cfg: &Config,
    input: HandleTargetListInput,
) -> Result<TargetListOutput, MonorailError> {
    let mut o = TargetListOutput {
        targets: cfg.targets.clone(),
        target_groups: None,
    };
    if input.show_target_groups {
        let mut lookups = Lookups::new(cfg, &cfg.get_target_path_set())?;
        o.target_groups = Some(lookups.dag.get_labeled_groups()?);
    }
    Ok(o)
}

#[derive(Debug)]
pub struct HandleCheckpointUpdateInput<'a> {
    pub(crate) pending: bool,
    pub(crate) git_change_options: GitChangeOptions<'a>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointUpdateOutput {
    commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<HashMap<String, String>>,
}

pub async fn handle_checkpoint_update(
    cfg: &Config,
    input: &HandleCheckpointUpdateInput<'_>,
    workdir: &str,
) -> Result<CheckpointUpdateOutput, MonorailError> {
    match cfg.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(workdir)?;
            let tracking = tracking::Table::open(&get_tracking_path(workdir)).await?;
            let mut checkpoint = match tracking.open_checkpoint().await {
                Ok(cp) => cp,
                Err(MonorailError::CheckpointNotFound(_)) => tracking.new_checkpoint(),
                // TODO: need to set path on checkpoint tho; don't use default
                Err(e) => {
                    return Err(e);
                }
            };
            checkpoint.commit = repo.revparse_single("HEAD")?.id().to_string();
            if input.pending {
                let pending_changes =
                    get_git_diff_changes(&repo, &input.git_change_options, &None, workdir).await?;
                if let Some(pending_changes) = pending_changes {
                    if !pending_changes.is_empty() {
                        let mut pending = HashMap::new();
                        for change in pending_changes.iter() {
                            let p = Path::new(workdir).join(&change.name);
                            pending.insert(change.name.clone(), get_file_checksum(&p).await?);
                        }
                        checkpoint.pending = Some(pending);
                    }
                }
            }
            checkpoint.save().await?;
            Ok(CheckpointUpdateOutput {
                commit: checkpoint.commit,
                pending: checkpoint.pending,
            })
        }
    }
}

// fn handle_cmd_output(
//     output: std::io::Result<std::process::Output>,
// ) -> Result<Vec<u8>, MonorailError> {
//     match output {
//         Ok(output) => {
//             if output.status.success() {
//                 Ok(output.stdout)
//             } else {
//                 Err(String::from_utf8_lossy(&output.stderr).to_string().into())
//             }
//         }
//         Err(e) => Err(e.to_string().into()),
//     }
// }

// fn git_cmd_status(
//     git_path: &str,
//     workdir: Option<&std::path::Path>,
// ) -> Result<Vec<u8>, MonorailError> {
//     handle_cmd_output(
//         std::process::Command::new(git_path)
//             .args([
//                 "status",
//                 "--porcelain",
//                 "--untracked-files=all",
//                 "--renames",
//             ])
//             .current_dir(workdir.unwrap_or(&std::env::current_dir()?))
//             .output(),
//     )
// }

// fn git_cmd_status_changes(s: Vec<u8>) -> Vec<Change> {
//     let mut v: Vec<Change> = Vec::new();
//     if !s.is_empty() {
//         let iter = s.split(|x| char::from(*x) == '\n');
//         for w in iter {
//             let mut parts: Vec<Cow<str>> = w
//                 .split(|z| char::from(*z) == ' ')
//                 .map(String::from_utf8_lossy)
//                 .collect();
//             parts.retain(|z| z != "");
//             // not interested in anything else spurious from
//             // our parsing of line format
//             if parts.len() == 2 {
//                 v.push(Change {
//                     name: parts[1].to_string(),
//                 });
//             }
//         }
//         return v;
//     }
//     v
// }

async fn git_cmd_other_changes(
    git_path: &str,
    workdir: Option<&std::path::Path>,
) -> Result<Vec<Change>, MonorailError> {
    let mut args = vec!["ls-files", "--others", "--exclude-standard"];
    let mut child = tokio::process::Command::new(git_path)
        .args(&args)
        .current_dir(workdir.unwrap_or(&std::env::current_dir()?))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| MonorailError::from(e))?;
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
    workdir: Option<&std::path::Path>,
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
        .current_dir(workdir.unwrap_or(&std::env::current_dir()?))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| MonorailError::from(e))?;
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
    fn new(cfg: &'a Config, visible_targets: &HashSet<&String>) -> Result<Self, MonorailError> {
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
    #[serde(default)]
    vcs: Vcs,
    targets: Option<Vec<Target>>,

    // Additional CLI config
    #[serde(skip)]
    pub workdir: String,
}
impl Config {
    pub fn new(file_path: &str) -> Result<Config, MonorailError> {
        let path = Path::new(file_path);

        let file = File::open(path)?;
        let mut buf_reader = BufReader::new(file);
        let buf = buf_reader.fill_buf()?;
        Ok(toml::from_str(std::str::from_utf8(buf)?)?)
    }
    pub fn validate(&self) -> Result<(), MonorailError> {
        Ok(())
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
        let (repo, repo_path) = get_repo(false);
        let mut git_opts = GitChangeOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let start = format!("{}", oid1);

        // no start/end without a checkpoint or pending changes is ok(none)
        assert_eq!(
            get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
                .await
                .unwrap(),
            None
        );

        // start == end is ok
        git_opts.start = Some(&start);
        git_opts.end = Some(&start);
        assert_eq!(
            get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![])
        );
        git_opts.start = None;
        git_opts.end = None;

        // start < end with changes is ok
        let foo_path = Path::new(&repo_path).join("foo.txt");
        let _foo_checksum = write_with_checksum(&foo_path, &vec![1]).await?;
        let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
        let end = format!("{}", oid2);
        git_opts.start = Some(&start);
        git_opts.end = Some(&end);
        assert_eq!(
            get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );
        git_opts.start = None;
        git_opts.end = None;

        // start > end with changes is ok
        git_opts.start = Some(&end);
        git_opts.end = Some(&start);
        assert_eq!(
            get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
                .await
                .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        purge_repo(&repo_path);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_ok_with_checkpoint() -> Result<(), Box<dyn std::error::Error>>
    {
        let (repo, repo_path) = get_repo(false);
        let mut git_opts = GitChangeOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let start = format!("{}", oid1);

        // no start/end yields no changes, with checkpoint (empty commit) is ok(none)
        assert_eq!(
            get_git_diff_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
                    commit: "".to_string(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        "blarp".to_string()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap(),
            None
        );

        // start != end with checkpoint file checksum match
        let foo_path = Path::new(&repo_path).join("foo.txt");
        let foo_checksum = write_with_checksum(&foo_path, &vec![1]).await?;
        let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
        let end = format!("{}", oid2);
        git_opts.start = Some(&start);
        git_opts.end = Some(&end);
        assert_eq!(
            get_git_diff_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
                    commit: start.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        foo_checksum.clone()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![])
        );

        // start != end with checkpoint file checksum not match
        assert_eq!(
            get_git_diff_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
                    commit: start.clone(),
                    pending: Some(HashMap::from([(
                        "foo.txt".to_string(),
                        "flarp".to_string()
                    )])),
                }),
                &repo_path
            )
            .await
            .unwrap(),
            Some(vec![Change {
                name: "foo.txt".to_string()
            }])
        );

        purge_repo(&repo_path);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_diff_changes_err() -> Result<(), Box<dyn std::error::Error>> {
        let (repo, repo_path) = get_repo(false);
        let mut git_opts = GitChangeOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let start = format!("{}", oid1);

        // no start/end, with invalid checkpoint commit is err
        assert!(get_git_diff_changes(
            &repo,
            &git_opts,
            &Some(tracking::Checkpoint {
                path: Path::new("x").to_path_buf(),
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
        assert!(get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
            .await
            .is_err());

        // bad end is err
        git_opts.start = Some(&start);
        git_opts.end = Some("foo");
        assert!(get_git_diff_changes(&repo, &git_opts, &None, &repo_path)
            .await
            .is_err());
        git_opts.start = None;
        git_opts.end = None;

        purge_repo(&repo_path);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_git_all_changes() {
        let (repo, repo_path) = get_repo(false);
        let mut git_opts = GitChangeOptions {
            start: None,
            end: None,
            git_path: "git",
        };
        // create commit so there's something for HEAD to point to
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let start = format!("{}", oid1);

        // no changes, no checkpoint is ok
        assert!(get_git_all_changes(&repo, &git_opts, &None, &repo_path)
            .await
            .unwrap()
            .is_none());

        git_opts.start = Some(&start);

        // no changes, with checkpoint is ok
        assert_eq!(
            get_git_all_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
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

        // create a new file and check that it is seen
        let foo_path = Path::new(&repo_path).join("foo.txt");
        let foo_checksum = write_with_checksum(&foo_path, &vec![1]).await.unwrap();
        tokio::process::Command::new("git")
            .arg("commit")
            .arg("-a")
            .arg("-m")
            .arg("test")
            .status()
            .await
            .unwrap();
        // let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
        // let end = format!("{}", oid2);
        // git_opts.end = Some(&end);
        git_opts.end = None;

        assert_eq!(
            get_git_all_changes(&repo, &git_opts, &None, &repo_path)
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );

        // update checkpoint to include file and check that it is no longer seen
        assert_eq!(
            get_git_all_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
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

        // create another file and check that it is seen
        let bar_path = Path::new(&repo_path).join("bar.txt");
        let bar_checksum = write_with_checksum(&bar_path, &vec![2]).await.unwrap();
        assert_eq!(
            get_git_all_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
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
            1
        );

        // update checkpoint and verify second file is not seen
        assert_eq!(
            get_git_all_changes(
                &repo,
                &git_opts,
                &Some(tracking::Checkpoint {
                    path: Path::new("x").to_path_buf(),
                    commit: start.clone(),
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

        purge_repo(&repo_path);
    }

    #[tokio::test]
    async fn test_get_filtered_changes() {
        let (_repo, repo_path) = get_repo(false);
        let root_path = Path::new(&repo_path);
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
                &get_pending(&vec![(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &vec![1, 2, 3])
                        .await
                        .unwrap(),
                )]),
                &repo_path
            )
            .await,
            vec![]
        );

        // changes, pending with change checksum mismatch
        tokio::fs::write(Path::new(&repo_path).join(fname1), &vec![1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            get_filtered_changes(
                vec![change1.clone()],
                &get_pending(&vec![(fname1, "foo".into(),)]),
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
                &get_pending(&vec![(
                    fname1,
                    write_with_checksum(&root_path.join(fname1), &vec![1, 2, 3])
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
        let (_repo, repo_path) = get_repo(false);
        let fname1 = "test1.txt";

        let root_path = Path::new(&repo_path);
        let pending = get_pending(&vec![(
            fname1,
            write_with_checksum(&root_path.join(fname1), &vec![1, 2, 3])
                .await
                .unwrap(),
        )]);

        // checksums must match
        assert!(checksum_is_equal(&pending, &repo_path, &fname1).await);
        // file error (such as dne) interpreted as checksum mismatch
        assert!(!checksum_is_equal(&pending, &repo_path, "dne.txt").await);

        // write a file and use a pending entry with a mismatched checksum
        let fname2 = "test2.txt";
        tokio::fs::write(root_path.join(fname2), &vec![1])
            .await
            .unwrap();
        let pending2 = get_pending(&vec![(fname2, "foobar".into())]);
        // checksums don't match
        assert!(!checksum_is_equal(&pending2, &repo_path, &fname2).await);
    }

    #[tokio::test]
    async fn test_get_file_checksum() {
        let (_repo, repo_path) = get_repo(false);

        // files that don't exist can't be checksummed
        let p = Path::new(&repo_path).join("test.txt");
        assert!(get_file_checksum(&p).await.is_err());

        // write file and compare checksums
        let checksum = write_with_checksum(&p, &vec![1, 2, 3]).await.unwrap();
        assert_eq!(get_file_checksum(&p).await.unwrap(), checksum);
    }

    async fn write_with_checksum(path: &path::Path, data: &[u8]) -> Result<String, MonorailError> {
        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        tokio::fs::write(path, &data).await?;
        Ok(format!("{}", hex::encode(hasher.finalize())))
    }

    // #[tokio::test]
    // async fn test_get_git_all_changes() {
    //     let (repo, repo_path) = get_repo(false);
    //     let workdir = repo.workdir().unwrap();
    //     let checkpoint = tracking::Table::open(&Path::new(&workdir)).await.unwrap();
    //     let gco = GitChangeOptions {
    //         start: None,
    //         end: None,
    //     }

    //     let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);

    //     &repo,
    //     &input.git_change_options,
    //     &checkpoint,
    //     workdir,

    //     // no changes
    //     const USE_LIBGIT2: bool = true;
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid1, !USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         0
    //     );
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid1, USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         0
    //     );

    //     // committed changes
    //     let _f1 = create_file(&repo_path, "", "foo.txt", b"x");
    //     let oid2 = commit_file(&repo, "foo.txt", Some("HEAD"), &[&get_commit(&repo, oid1)]);
    //     let _f2 = create_file(&repo_path, "", "bar.txt", b"y");
    //     let oid3 = commit_file(&repo, "bar.txt", Some("HEAD"), &[&get_commit(&repo, oid2)]);
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid3, !USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         2
    //     );
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid3, USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         2
    //     );

    //     // committed + unstaged changes
    //     let _f3 = create_file(&repo_path, "", "baz.txt", b"z");
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid3, !USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         3
    //     );
    //     assert_eq!(
    //         get_git_all_changes(&repo, oid1, oid3, USE_LIBGIT2, "git")
    //             .await
    //             .unwrap()
    //             .unwrap()
    //             .len(),
    //         3
    //     );

    //     purge_repo(&repo_path);
    // }

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
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, false, false).unwrap();

        assert!(o.changes.unwrap().is_empty());
        assert!(o.targets.is_empty());
    }

    #[test]
    fn test_analyze_unknown() {
        let change1 = "foo.txt";
        let changes = vec![Change {
            name: change1.to_string(),
        }];
        let expected_targets: Vec<String> = vec![];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![]),
        }];

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![]));
    }

    #[test]
    fn test_analyze_target_file() {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }
    #[test]
    fn test_analyze_target() {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_ancestors() {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_uses() {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(expected_target_groups));
    }

    #[test]
    fn test_analyze_target_ignores() {
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

        let c = prep_raw_config_repo();
        let mut lookups = Lookups::new(&c, &c.get_target_path_set()).unwrap();
        let o = analyze(&mut lookups, Some(changes), true, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
        assert_eq!(o.target_groups, Some(vec![vec![target1.to_string()]]));
    }

    #[test]
    fn test_lookups() {
        let c = prep_raw_config_repo();
        let l = Lookups::new(&c, &c.get_target_path_set()).unwrap();

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

    //     #[test]
    //     fn test_git_cmd_status_changes() {
    //         let raw = r#" M .circleci/config.yml
    //  M Monorail.toml
    //  M rust/support/script/command.sh
    // ?? go/project/tator/
    // ?? out.json
    // "#;
    //         let changes = git_cmd_status_changes(Vec::from(raw));
    //         assert_eq!(
    //             changes,
    //             vec![
    //                 Change {
    //                     name: ".circleci/config.yml".to_string()
    //                 },
    //                 Change {
    //                     name: "Monorail.toml".to_string()
    //                 },
    //                 Change {
    //                     name: "rust/support/script/command.sh".to_string()
    //                 },
    //                 Change {
    //                     name: "go/project/tator/".to_string()
    //                 },
    //                 Change {
    //                     name: "out.json".to_string()
    //                 },
    //             ]
    //         );

    //         assert_eq!(git_cmd_status_changes(Vec::from("")), vec![]);
    //     }

    #[test]
    fn test_err_duplicate_target_path() {
        let config_str: &str = r#"
[[targets]]
path = "rust"

[[targets]]
path = "rust"
"#;
        let c: Config = toml::from_str(config_str).unwrap();
        assert!(Lookups::new(&c, &c.get_target_path_set()).is_err());
    }
}
