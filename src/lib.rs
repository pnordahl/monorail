pub mod common;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::env;
use std::error::Error;
use std::fmt;

use std::io::Write;

use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

use std::result::Result;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use trie_rs::{Trie, TrieBuilder};

#[derive(Debug, Serialize, Eq, PartialEq)]
pub enum ErrorClass {
    #[serde(rename = "generic")]
    Generic,
    #[serde(rename = "git2")]
    Git2,
    #[serde(rename = "io")]
    Io,
    #[serde(rename = "toml_deserialize")]
    TomlDeserialize,
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "serde_json")]
    SerdeJSON,
    #[serde(rename = "utf8")]
    Utf8Error,
    #[serde(rename = "parse_int")]
    ParseIntError,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct MonorailError {
    pub class: ErrorClass,
    pub message: String,
}
impl Error for MonorailError {}
impl fmt::Display for MonorailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "class: {:?}, message: {}", self.class, self.message)
    }
}
impl From<git2::Error> for MonorailError {
    fn from(error: git2::Error) -> Self {
        MonorailError {
            class: ErrorClass::Git2,
            message: format!(
                "class: {:?}, code: {:?}, message: {:?}",
                error.class(),
                error.code(),
                error.message()
            ),
        }
    }
}
impl From<&str> for MonorailError {
    fn from(error: &str) -> Self {
        MonorailError {
            class: ErrorClass::Generic,
            message: error.to_string(),
        }
    }
}
impl From<String> for MonorailError {
    fn from(error: String) -> Self {
        MonorailError {
            class: ErrorClass::Generic,
            message: error,
        }
    }
}
impl From<std::io::Error> for MonorailError {
    fn from(error: std::io::Error) -> Self {
        MonorailError {
            class: ErrorClass::Io,
            message: error.to_string(),
        }
    }
}
impl From<std::str::Utf8Error> for MonorailError {
    fn from(error: std::str::Utf8Error) -> Self {
        MonorailError {
            class: ErrorClass::Utf8Error,
            message: error.to_string(),
        }
    }
}
impl From<toml::de::Error> for MonorailError {
    fn from(error: toml::de::Error) -> Self {
        MonorailError {
            class: ErrorClass::TomlDeserialize,
            message: error.to_string(),
        }
    }
}
impl From<serde_json::error::Error> for MonorailError {
    fn from(error: serde_json::error::Error) -> Self {
        MonorailError {
            class: ErrorClass::SerdeJSON,
            message: error.to_string(),
        }
    }
}
impl From<std::num::ParseIntError> for MonorailError {
    fn from(error: std::num::ParseIntError) -> Self {
        MonorailError {
            class: ErrorClass::ParseIntError,
            message: error.to_string(),
        }
    }
}

fn exit_with_error(err: MonorailError, fmt: &str) {
    write_output(std::io::stderr(), &err, fmt).unwrap();
    std::process::exit(1);
}

pub fn handle(cmd: clap::Command) {
    let matches = cmd.get_matches();

    let output_format = matches.get_one::<String>("output-format").unwrap();

    let wd: String = match matches.get_one::<String>("working-directory") {
        Some(wd) => wd.into(),
        None => match env::current_dir() {
            Ok(pb) => {
                let s = pb.to_str();
                match s {
                    Some(s) => s.to_string(),
                    None => {
                        return exit_with_error("failed to get current dir".into(), output_format);
                    }
                }
            }
            Err(e) => {
                return exit_with_error(e.into(), output_format);
            }
        },
    };

    match matches.get_one::<String>("config-file") {
        Some(cfg_path) => {
            match Config::new(Path::new(&wd).join(cfg_path).to_str().unwrap_or(cfg_path)) {
                Ok(cfg) => match cfg.validate() {
                    Ok(()) => {
                        if let Some(_config) = matches.subcommand_matches("config") {
                            write_output(std::io::stdout(), &cfg, output_format).unwrap();
                            std::process::exit(0);
                        }

                        if let Some(checkpoint) = matches.subcommand_matches("checkpoint") {
                            match handle_checkpoint(
                                &cfg,
                                HandleCheckpointInput {
                                    checkpoint_type: checkpoint.get_one::<String>("type").unwrap(),
                                    dry_run: checkpoint.get_flag("dry-run"),
                                    git_path: checkpoint.get_one::<String>("git-path").unwrap(),
                                    use_libgit2_status: checkpoint.get_flag("use-libgit2-status"),
                                },
                                &wd,
                            ) {
                                Ok(o) => {
                                    write_output(std::io::stdout(), &o, output_format).unwrap();
                                    std::process::exit(0);
                                }
                                Err(e) => {
                                    exit_with_error(e, output_format);
                                }
                            }
                            std::process::exit(0);
                        }

                        if let Some(analyze) = matches.subcommand_matches("analyze") {
                            let i = AnalyzeInput {
                                git_change_options: GitChangeOptions {
                                    start: analyze
                                        .get_one::<String>("start")
                                        .map(|x: &String| x.as_str()),
                                    end: analyze
                                        .get_one::<String>("end")
                                        .map(|x: &String| x.as_str()),
                                    git_path: analyze.get_one::<String>("git-path").unwrap(),
                                    use_libgit2_status: analyze.get_flag("use-libgit2-status"),
                                },
                                show_changes: analyze.get_flag("show-changes"),
                                show_change_targets: analyze.get_flag("show-change-targets"),
                            };
                            match handle_analyze(&cfg, &i, &wd) {
                                Ok(ref output) => {
                                    write_output(std::io::stdout(), &output, output_format)
                                        .unwrap();
                                    std::process::exit(0);
                                }
                                Err(e) => {
                                    exit_with_error(e, output_format);
                                }
                            }
                        }

                        if let Some(inspect) = matches.subcommand_matches("inspect") {
                            if let Some(change) = inspect.subcommand_matches("change") {
                                let start = change
                                    .get_one::<String>("start")
                                    .map(|x: &String| x.as_str());
                                let end = change.get_one("end").map(|x: &String| x.as_str());
                                let git_path = change.get_one::<String>("git-path").unwrap();
                                let i = HandleInspectChangeInput {
                                    start,
                                    end,
                                    git_path,
                                    use_libgit2_status: change.get_flag("use-libgit2-status"),
                                };
                                match get_raw_changes(&cfg, &i, &wd) {
                                    Ok(raw_changes) => {
                                        match process_inspect_change(&cfg, &raw_changes, true) {
                                            Ok(o) => {
                                                if change.get_flag("targets-only") {
                                                    write_output(
                                                        std::io::stdout(),
                                                        &o.targets,
                                                        output_format,
                                                    )
                                                    .unwrap();
                                                    std::process::exit(0);
                                                }
                                                write_output(std::io::stdout(), &o, output_format)
                                                    .unwrap();
                                                std::process::exit(0);
                                            }
                                            Err(e) => {
                                                exit_with_error(e, output_format);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        exit_with_error(e, output_format);
                                    }
                                }
                            } else {
                                exit_with_error("no valid subcommand match".into(), output_format);
                            }
                        }
                    }
                    Err(e) => exit_with_error(e, output_format),
                },
                Err(e) => {
                    exit_with_error(e, output_format);
                }
            }
        }
        None => {
            exit_with_error("no configuration specified".into(), output_format);
        }
    };
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

#[derive(Debug)]
pub struct GitChangeOptions<'a> {
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub git_path: &'a str,
    pub use_libgit2_status: bool,
}

#[derive(Debug)]
pub struct AnalyzeInput<'a> {
    pub git_change_options: GitChangeOptions<'a>,
    pub show_changes: bool,
    pub show_change_targets: bool,
}
#[derive(Serialize, Debug)]
pub struct AnalyzeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<AnalyzedChange>>,
    pub targets: Vec<String>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub struct AnalyzedChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<AnalyzedChangeTarget>>,
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
pub enum AnalyzedChangeTargetReason {
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "links")]
    Links,
    #[serde(rename = "uses")]
    Uses,
    #[serde(rename = "ignores")]
    Ignores,
}

pub fn handle_analyze<'a>(
    config: &'a Config,
    input: &AnalyzeInput<'a>,
    workdir: &'a str,
) -> Result<AnalyzeOutput, MonorailError> {
    let lookups = Lookups::new(config)?;
    match config.vcs.r#use {
        VcsKind::Git => {
            let changes = git_get_raw_changes(&input.git_change_options, workdir)?;
            analyze(
                lookups,
                changes,
                input.show_changes,
                input.show_change_targets,
            )
        }
    }
}

// let set of common prefix ignores         = I
// let set of common prefix ignores targets = It
// let set of common prefix links           = L
// let set of common prefix links targets   = Lt
// let set of common prefix uses            = U
// let set of common prefix uses targets    = Ut
// let set of common prefix targets         = T
// let set of output targets                = O

// O = T + Ut + Lt - It
pub fn analyze(
    lookups: Lookups,
    changes: Vec<RawChange>,
    show_changes: bool,
    show_change_targets: bool,
) -> Result<AnalyzeOutput, MonorailError> {
    let mut output = AnalyzeOutput {
        changes: if show_changes { Some(vec![]) } else { None },
        targets: vec![],
    };

    let mut output_targets = HashSet::new();

    changes.iter().for_each(|c| {
        let mut change_targets = if show_change_targets {
            Some(vec![])
        } else {
            None
        };
        let mut add_targets = HashSet::new();

        lookups
            .targets
            .common_prefix_search(&c.name)
            .for_each(|target: String| {
                if let Some(change_targets) = change_targets.as_mut() {
                    change_targets.push(AnalyzedChangeTarget {
                        path: target.to_owned(),
                        reason: AnalyzedChangeTargetReason::Target,
                    });
                }
                add_targets.insert(target);
            });
        lookups
            .links
            .common_prefix_search(&c.name)
            .for_each(|link: String| {
                // find any targets that have this link as a prefix, aka set Lt
                lookups
                    .targets
                    .common_prefix_search(&link)
                    .for_each(|parent: String| {
                        if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(AnalyzedChangeTarget {
                                path: parent.to_owned(),
                                reason: AnalyzedChangeTargetReason::Links,
                            });
                        }
                        // find all children of matched targets
                        lookups
                            .targets
                            .postfix_search(&parent)
                            .for_each(|child: String| {
                                let child_path = format!("{}{}", &parent, child);
                                if let Some(change_targets) = change_targets.as_mut() {
                                    change_targets.push(AnalyzedChangeTarget {
                                        path: child_path.to_owned(),
                                        reason: AnalyzedChangeTargetReason::Links,
                                    });
                                }
                                add_targets.insert(child_path);
                            });
                        add_targets.insert(parent);
                    });
            });
        lookups
            .uses
            .common_prefix_search(&c.name)
            .for_each(|m: String| {
                if let Some(v) = lookups.use2targets.get(&m) {
                    v.iter().for_each(|target| {
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
                if let Some(v) = lookups.ignore2targets.get(&m) {
                    v.iter().for_each(|target| {
                        add_targets.remove(target.as_str());
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

    // copy the hashmap into the output vector
    for t in output_targets.iter() {
        output.targets.push(t.to_owned());
    }
    output.targets.sort();

    Ok(output)
}

pub fn git_get_raw_changes(
    input: &GitChangeOptions,
    wd: &str,
) -> Result<Vec<RawChange>, MonorailError> {
    let repo = git2::Repository::open(wd)?;
    let start = match input.start {
        Some(s) => libgit2_find_oid(&repo, s)?,
        None => {
            match libgit2_latest_tag(&repo, libgit2_find_oid(&repo, "HEAD")?)? {
                Some(lr) => lr.target_id(),
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
            }
        }
    };

    let end = libgit2_find_oid(&repo, input.end.unwrap_or("HEAD"))?;
    git_all_changes(&repo, start, end, input.use_libgit2_status, input.git_path)
}

pub fn get_raw_changes(
    cfg: &Config,
    input: &HandleInspectChangeInput,
    wd: &str,
) -> Result<Vec<RawChange>, MonorailError> {
    match cfg.vcs.r#use {
        VcsKind::Git => {
            let repo = git2::Repository::open(wd)?;
            let start =
                match input.start {
                    Some(s) => libgit2_find_oid(&repo, s)?,
                    None => {
                        match libgit2_latest_tag(&repo, libgit2_find_oid(&repo, "HEAD")?)? {
                            Some(lr) => lr.target_id(),
                            None => {
                                // no tags, use first commit of the repo
                                match libgit2_first_commit(&repo)? {
                                    Some(fc) => fc,
                                    None => return Err(
                                        "couldn't find a starting point; commit something first"
                                            .into(),
                                    ),
                                }
                            }
                        }
                    }
                };

            let end = libgit2_find_oid(&repo, input.end.unwrap_or("HEAD"))?;
            git_all_changes(&repo, start, end, input.use_libgit2_status, input.git_path)
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HandleCheckpointInput<'a> {
    pub checkpoint_type: &'a str,
    pub dry_run: bool,
    pub git_path: &'a str,
    pub use_libgit2_status: bool,
}
#[derive(Debug, Serialize)]
pub struct CheckpointOutput {
    pub id: String,
    pub targets: Vec<String>,
    pub dry_run: bool,
}

pub fn handle_checkpoint(
    cfg: &Config,
    input: HandleCheckpointInput,
    wd: &str,
) -> Result<CheckpointOutput, MonorailError> {
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

            let end_oid = libgit2_find_oid(&repo, "HEAD")?;
            let latest_tag = libgit2_latest_tag(&repo, end_oid)?;
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
            let o = process_inspect_change(cfg, &changes, false)?;

            // let mut changed_targets = pico.into_iter().collect::<Vec<String>>();
            // changed_targets.sort();

            // without targets, there's nothing to do
            if o.targets.is_empty() {
                return Ok(CheckpointOutput {
                    id: "".to_string(),
                    targets: o.targets,
                    dry_run: input.dry_run,
                });
            }

            // get new tag name
            let tag_name = match latest_tag {
                Some(latest_tag) => match latest_tag.name() {
                    Some(name) => increment_semver(name, input.checkpoint_type)?,
                    None => return Err("reference semver not provided".into()),
                },
                None => increment_semver("v0.0.0", input.checkpoint_type)?,
            };

            if !input.dry_run {
                // create tag and push
                repo.tag(
                    &tag_name,
                    &repo.find_object(end_oid, None)?,
                    &repo.signature()?,
                    &format!("{}\n\n", &o.targets.join("\n")),
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
                    return Err(format!(
                        "failed to push tags: {}",
                        std::str::from_utf8(&output.stderr)?
                    )
                    .into());
                }
            }

            Ok(CheckpointOutput {
                id: tag_name,
                targets: o.targets,
                dry_run: input.dry_run,
            })
        }
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

// given a commit, find the earliest annotated tag behind it
pub fn libgit2_latest_tag(
    repo: &git2::Repository,
    oid: git2::Oid,
) -> Result<Option<git2::Tag>, git2::Error> {
    let o = repo.find_object(oid, None)?;

    let dopts = git2::DescribeOptions::new();
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

// git rev-list --max-parents=0
fn libgit2_first_commit(repo: &git2::Repository) -> Result<Option<git2::Oid>, git2::Error> {
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

// A universal oid lookup function, allowing users to freely pass refs, revs, sha, or oid
// and still resolve.
fn libgit2_find_oid(repo: &git2::Repository, s: &str) -> Result<git2::Oid, MonorailError> {
    // first, attempt a direct lookup (for SHA)
    match git2::Oid::from_str(s) {
        Ok(o) => Ok(o),
        Err(_) => {
            // look up by reference (tags, HEAD, etc.)
            let r = repo.resolve_reference_from_short_name(s)?;
            if r.is_tag() {
                let t = r.peel_to_tag()?;
                Ok(t.target_id())
            } else {
                match r.target() {
                    Some(o) => Ok(o),
                    None => Err("object not found".into()),
                }
            }
        }
    }
}

fn increment_semver(semver: &str, checkpoint_type: &str) -> Result<String, MonorailError> {
    let v: Vec<&str> = semver.trim_start_matches('v').split('.').collect();
    if v.len() != 3 {
        return Err(format!("semver should have 3 parts, it has {}", v.len()).into());
    }
    let mut major = v[0].parse::<i32>()?;
    let mut minor = v[1].parse::<i32>()?;
    let mut patch = v[2].parse::<i32>()?;
    match checkpoint_type {
        "major" => {
            major += 1;
            minor = 0;
            patch = 0;
        }
        "minor" => {
            minor += 1;
            patch = 0;
        }
        "patch" => {
            patch += 1;
        }
        _ => return Err(format!("unrecognized checkpoint type {}", checkpoint_type).into()),
    }
    Ok(format!("v{}.{}.{}", major, minor, patch))
}

pub struct Lookups<'a> {
    targets: Trie<u8>,
    links: Trie<u8>,
    ignores: Trie<u8>,
    uses: Trie<u8>,
    use2targets: HashMap<&'a String, Vec<&'a String>>,
    ignore2targets: HashMap<&'a String, Vec<&'a String>>,
}
impl<'a> Lookups<'a> {
    fn new(cfg: &'a Config) -> Result<Self, MonorailError> {
        let mut targets_builder = TrieBuilder::new();
        let mut links_builder = TrieBuilder::new();
        let mut ignores_builder = TrieBuilder::new();
        let mut uses_builder = TrieBuilder::new();
        let mut use2targets = HashMap::new();
        let mut ignore2targets = HashMap::new();

        let mut seen_targets = HashSet::new();
        if let Some(targets) = cfg.targets.as_ref() {
            if let Err(e) = targets.iter().try_for_each(|target| {
                if seen_targets.contains(&target.path) {
                    return Err(MonorailError {
                        class: ErrorClass::Generic,
                        message: format!("Duplicate target path provided: {}", &target.path),
                    });
                }
                seen_targets.insert(&target.path);
                targets_builder.push(&target.path);
                if let Some(links) = target.links.as_ref() {
                    links.iter().for_each(|s| {
                        links_builder.push(s);
                    });
                }
                if let Some(ignores) = target.ignores.as_ref() {
                    ignores.iter().for_each(|s| {
                        ignores_builder.push(s);
                        ignore2targets.entry(s).or_insert(vec![]).push(&target.path);
                    });
                }
                if let Some(uses) = target.uses.as_ref() {
                    uses.iter().for_each(|s| {
                        uses_builder.push(s);
                        use2targets.entry(s).or_insert(vec![]).push(&target.path);
                    });
                }
                Ok(())
            }) {
                return Err(e);
            }
        }
        Ok(Self {
            targets: targets_builder.build(),
            links: links_builder.build(),
            ignores: ignores_builder.build(),
            uses: uses_builder.build(),
            use2targets,
            ignore2targets,
        })
    }
}

pub struct HandleInspectChangeInput<'a> {
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub git_path: &'a str,
    pub use_libgit2_status: bool,
}

// let set of common prefix ignores         = I
// let set of common prefix ignores targets = It
// let set of common prefix links           = L
// let set of common prefix links targets   = Lt
// let set of common prefix uses            = U
// let set of common prefix uses targets    = Ut
// let set of common prefix targets         = T
// let set of output targets                = O

// O = T + Ut + Lt - It
pub fn process_inspect_change<'a>(
    cfg: &'a Config,
    changes: &'a [RawChange],
    include_change_targets: bool,
) -> Result<InspectChangeOutput<'a>, MonorailError> {
    let lookups = Lookups::new(cfg)?;
    let mut output = InspectChangeOutput::new();
    let mut output_targets = HashSet::new();

    changes.iter().for_each(|c| {
        let mut change_targets = if include_change_targets {
            Some(vec![])
        } else {
            None
        };
        let mut add_targets = HashSet::new();

        lookups
            .targets
            .common_prefix_search(&c.name)
            .for_each(|target: String| {
                if let Some(change_targets) = change_targets.as_mut() {
                    change_targets.push(ChangeTarget {
                        path: target.to_owned(),
                        reason: ChangeTargetReason::Target,
                    });
                }
                add_targets.insert(target);
            });
        lookups
            .links
            .common_prefix_search(&c.name)
            .for_each(|link: String| {
                // find any targets that have this link as a prefix, aka set Lt
                lookups
                    .targets
                    .common_prefix_search(&link)
                    .for_each(|parent: String| {
                        if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(ChangeTarget {
                                path: parent.to_owned(),
                                reason: ChangeTargetReason::Links,
                            });
                        }
                        // find all children of matched targets
                        lookups
                            .targets
                            .postfix_search(&parent)
                            .for_each(|child: String| {
                                if let Some(change_targets) = change_targets.as_mut() {
                                    change_targets.push(ChangeTarget {
                                        path: child.strip_prefix("/").unwrap_or(&child).to_owned(),
                                        reason: ChangeTargetReason::Links,
                                    });
                                }
                                add_targets.insert(format!("{}{}", &parent, child));
                            });
                        add_targets.insert(parent);
                    });
            });
        lookups
            .uses
            .common_prefix_search(&c.name)
            .for_each(|m: String| {
                if let Some(v) = lookups.use2targets.get(&m) {
                    v.iter().for_each(|target| {
                        if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(ChangeTarget {
                                path: target.to_string(),
                                reason: ChangeTargetReason::Uses,
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
                if let Some(v) = lookups.ignore2targets.get(&m) {
                    v.iter().for_each(|target| {
                        add_targets.remove(target.as_str());
                        if let Some(change_targets) = change_targets.as_mut() {
                            change_targets.push(ChangeTarget {
                                path: target.to_string(),
                                reason: ChangeTargetReason::Ignores,
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

        output.changes.push(ProcessedChange {
            path: c.name.as_str(),
            targets: change_targets,
        });
    });

    output.targets = output_targets.into_iter().collect::<Vec<String>>();
    output.targets.sort();

    Ok(output)
}

#[derive(Serialize, Debug)]
pub struct InspectChangeOutput<'a> {
    pub changes: Vec<ProcessedChange<'a>>,
    pub targets: Vec<String>,
}
impl<'a> InspectChangeOutput<'a> {
    fn new() -> Self {
        Self {
            changes: vec![],
            targets: vec![],
        }
    }
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub struct ProcessedChange<'a> {
    pub path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<ChangeTarget>>,
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub struct ChangeTarget {
    path: String,
    reason: ChangeTargetReason,
}
impl Ord for ChangeTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}
impl PartialOrd for ChangeTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Serialize, Debug, Eq, PartialEq)]
pub enum ChangeTargetReason {
    #[serde(rename = "target")]
    Target,
    #[serde(rename = "links")]
    Links,
    #[serde(rename = "uses")]
    Uses,
    #[serde(rename = "ignores")]
    Ignores,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RawChange {
    name: String,
}

fn write_output<W, T>(mut writer: W, value: &T, output_format: &str) -> Result<(), MonorailError>
where
    W: Write,
    T: Serialize,
{
    match output_format {
        "json" => {
            serde_json::to_writer(&mut writer, value)?;
            writeln!(writer)?;
            Ok(())
        }
        _ => Err(format!("unrecognized output format {}", output_format).into()),
    }
}

#[derive(Serialize, Deserialize, Debug)]
enum VcsKind {
    #[serde(rename = "git")]
    Git,
}
impl FromStr for VcsKind {
    type Err = ();
    fn from_str(s: &str) -> Result<VcsKind, ()> {
        match s {
            "git" => Ok(VcsKind::Git),
            _ => Err(()),
        }
    }
}
impl Default for VcsKind {
    fn default() -> Self {
        VcsKind::Git
    }
}
#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    vcs: Vcs,
    #[serde(default)]
    extension: Extension,
    // group: Option<Vec<Group>>,
    targets: Option<Vec<Target>>,
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
#[derive(Debug, Deserialize, Serialize)]
enum ExtensionKind {
    #[serde(rename = "bash")]
    Bash,
}
impl FromStr for ExtensionKind {
    type Err = ();
    fn from_str(s: &str) -> Result<ExtensionKind, ()> {
        match s {
            "bash" => Ok(ExtensionKind::Bash),
            _ => Err(()),
        }
    }
}
impl Default for ExtensionKind {
    fn default() -> Self {
        ExtensionKind::Bash
    }
}
#[derive(Serialize, Deserialize, Debug, Default)]
struct Extension {
    #[serde(default)]
    r#use: ExtensionKind,
    #[serde(default)]
    bash: ExtensionBash,
}
#[derive(Serialize, Deserialize, Debug, Default)]
struct ExtensionBash {
    #[serde(default)]
    exec: ExtensionBashExec,
}
#[derive(Serialize, Deserialize, Debug)]
struct ExtensionBashExec {
    source: Option<Vec<String>>,
    #[serde(default = "ExtensionBashExec::default_entrypoint")]
    entrypoint: String,
}
impl ExtensionBashExec {
    fn default_entrypoint() -> String {
        "support/script/monorail-exec.sh".into()
    }
}
impl Default for ExtensionBashExec {
    fn default() -> Self {
        ExtensionBashExec {
            source: None,
            entrypoint: ExtensionBashExec::default_entrypoint(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Target {
    path: String,
    links: Option<Vec<String>>,
    uses: Option<Vec<String>>,
    ignores: Option<Vec<String>>,
}
impl Config {
    pub fn new(file_path: &str) -> Result<Config, MonorailError> {
        let path = Path::new(file_path);

        let file = File::open(path)?;
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader.read_to_string(&mut contents)?;

        let config = toml::from_str(contents.as_str())?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), MonorailError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    const RAW_CONFIG: &'static str = r#"
[vcs]
use = "git"

[vcs.git]
trunk = "master"
remote = "origin"

[extension]
use = "bash"

[[targets]]
path = "rust"
links = [
    "rust/.cargo",
    "rust/vendor",
    "rust/Cargo.toml",
]

[[targets]]
path = "rust/target/project1"
ignores = [
    "rust/target/project1/README.md",
    "rust/vendor"
]
uses = [
    "rust/common/log"
]
"#;

    #[test]
    fn test_libgit2_find_oid() {
        let (repo, repo_path) = get_repo(false);

        let oid = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
        let oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &vec![&get_commit(&repo, oid)],
        );
        let head = libgit2_find_oid(&repo, "HEAD").unwrap();
        assert_eq!(oid2, head);

        purge_repo(&repo_path);
    }

    #[test]
    fn test_libgit2_latest_tag() {
        let (repo, repo_path) = get_repo(false);
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
        let lt = libgit2_latest_tag(&repo, oid1).unwrap();
        assert!(lt.is_none());

        let tag_oid = repo
            .tag(
                "t1",
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
            &vec![&get_commit(&repo, oid1)],
        );
        assert_eq!(
            libgit2_latest_tag(&repo, oid2).unwrap().unwrap().id(),
            tag_oid
        );

        purge_repo(&repo_path);
    }
    #[test]
    fn test_libgit2_first_commit() {
        let (repo, repo_path) = get_repo(false);

        assert_eq!(libgit2_first_commit(&repo).unwrap(), None);
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
        let _oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &vec![&get_commit(&repo, oid1)],
        );
        assert_eq!(libgit2_first_commit(&repo).unwrap(), Some(oid1));

        purge_repo(&repo_path);
    }

    #[test]
    fn test_libgit2_start_end_diff_changes() {
        let (repo, repo_path) = get_repo(false);

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);
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

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);

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

        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &vec![]);

        assert_eq!(libgit2_status_changes(&repo).unwrap().len(), 0);

        // check that unstaged changes are detected
        let fname = "foo.txt";
        let fpath = std::path::Path::new(&repo_path).join(fname);
        let mut file = File::create(&fpath).unwrap();
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
            &vec![&get_commit(&repo, oid1)],
        );
        assert_eq!(libgit2_status_changes(&repo).unwrap().len(), 0);

        // TODO: check that renames are detected

        // TODO: check that untracked directories are recursed

        purge_repo(&repo_path);
    }

    #[test]
    fn test_increment_semver() {
        // invalid semver
        assert!(increment_semver("kljfasldkjf", "").is_err());

        // valid bump major
        assert_eq!(increment_semver("v3.2.1", "major").unwrap(), "v4.0.0");

        // valid bump minor
        assert_eq!(increment_semver("v3.2.1", "minor").unwrap(), "v3.3.0");

        // valid bump patch
        assert_eq!(increment_semver("v3.2.1", "patch").unwrap(), "v3.2.2");

        // initial version bump by type
        assert_eq!(increment_semver("v0.0.0", "major").unwrap(), "v1.0.0");
    }

    #[test]
    fn test_trie() {
        let mut builder = TrieBuilder::new();
        builder.push("rust/target/project1/README.md");
        builder.push("rust/common/log");
        builder.push("rust/common/error");
        builder.push("rust/foo/log");

        let trie = builder.build();

        assert_eq!(trie.exact_match("rust/target/project1/README.md"), true);
        let matches = trie
            .common_prefix_search("rust/common/log/bar.rs")
            .collect::<Vec<String>>();
        assert_eq!(
            String::from_utf8_lossy(matches[0].as_bytes()),
            "rust/common/log"
        );
    }

    #[test]
    fn test_config_new() {
        // TODO
    }

    // #[test]
    // fn test_process_inspect_change_target_file() {
    //     let name = "rust/target/project1/lib.rs".to_string();
    //     let targets = vec!["rust".to_string(), "rust/target/project1".to_string()];
    //     let targets = vec![ChangeTarget{path: "rust", reason: ChangeTargetReason::Target, ChangeTarget{path: "rust/target/project1", reason: ChangeTargetReason::Target}];
    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: targets.clone(),
    //             ignored_targets: vec![]
    //         }]
    //     );
    //     assert_eq!(o.targets, targets);
    // }

    // #[test]
    // fn test_process_inspect_change_target() {
    //     let name = "rust/target/project1/".to_string();
    //     let targets = vec!["rust".to_string(), "rust/target/project1".to_string()];
    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: targets.clone(),
    //             ignored_targets: vec![]
    //         }]
    //     );
    //     assert_eq!(o.targets, targets);
    // }

    // #[test]
    // fn test_process_inspect_change_group_link() {
    //     let name = "rust/.cargo/test.txt".to_string();
    //     let targets = vec!["rust".to_string(), "rust/target/project1".to_string()];

    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: targets.clone(),
    //             ignored_targets: vec![]
    //         }]
    //     );
    //     assert_eq!(o.targets, targets);
    // }

    // #[test]
    // fn test_process_inspect_change_project_depend() {
    //     let name = "rust/common/log/src/lib.rs".to_string();
    //     let targets = vec!["rust".to_string(), "rust/target/project1".to_string()];

    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: targets.clone(),
    //             ignored_targets: vec![]
    //         }]
    //     );
    //     assert_eq!(o.targets, targets);
    // }

    // #[test]
    // fn test_process_inspect_change_project_ignore() {
    //     let name = "rust/vendor/test.txt".to_string();
    //     let expected_targets = vec!["rust".to_string()];

    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: expected_targets.clone(),
    //             ignored_targets: vec!["rust/target/project1".to_string()]
    //         }]
    //     );
    //     assert_eq!(o.targets, expected_targets);
    // }

    // #[test]
    // fn test_process_inspect_change_project_ignore_file() {
    //     let name = "rust/target/project1/README.md".to_string();
    //     let expected_targets = vec!["rust".to_string()];

    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: expected_targets.clone(),
    //             ignored_targets: vec!["rust/target/project1".to_string()]
    //         }]
    //     );
    //     assert_eq!(o.targets, expected_targets);
    // }

    // #[test]
    // fn test_process_inspect_change_inert() {
    //     let name = "rust/inert.rs".to_string();
    //     let expected_targets = vec!["rust".to_string()];

    //     let changes = vec![RawChange { name: name.clone() }];
    //     let c: Config = toml::from_str(RAW_CONFIG).unwrap();
    //     let o = process_inspect_change(&c, &changes).unwrap();

    //     assert_eq!(
    //         o.changes,
    //         vec![ProcessedChange {
    //             path: &name,
    //             added_targets: expected_targets.clone(),
    //             ignored_targets: vec![]
    //         }]
    //     );
    //     assert_eq!(o.targets, expected_targets);
    // }

    #[test]
    fn test_lookups() {
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let l = Lookups::new(&c).unwrap();

        assert_eq!(
            l.targets
                .common_prefix_search("rust/target/project1/src/foo.rs")
                .collect::<Vec<String>>(),
            vec!["rust".to_string(), "rust/target/project1".to_string()]
        );
        assert_eq!(
            l.links
                .common_prefix_search("rust/Cargo.toml")
                .collect::<Vec<String>>(),
            vec!["rust/Cargo.toml".to_string()]
        );
        assert_eq!(
            l.uses
                .common_prefix_search("rust/common/log/foo.txt")
                .collect::<Vec<String>>(),
            vec!["rust/common/log".to_string()]
        );
        assert_eq!(
            l.ignores
                .common_prefix_search("rust/target/project1/README.md")
                .collect::<Vec<String>>(),
            vec!["rust/target/project1/README.md".to_string()]
        );
        assert_eq!(
            *l.use2targets.get(&"rust/common/log".to_string()).unwrap(),
            vec!["rust/target/project1"]
        );
        assert_eq!(
            *l.ignore2targets
                .get(&"rust/target/project1/README.md".to_string())
                .unwrap(),
            vec!["rust/target/project1"]
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
        let config_str: &'static str = r#"
[vcs]
use = "git"

[vcs.git]
trunk = "master"
remote = "origin"

[extension]
use = "bash"

[[targets]]
path = "rust"

[[targets]]
path = "rust"
"#;
        let name = "rust/".to_string();
        let changes = vec![RawChange { name }];
        let c: Config = toml::from_str(config_str).unwrap();
        assert_eq!(
            process_inspect_change(&c, &changes, false).err().unwrap(),
            MonorailError {
                class: ErrorClass::Generic,
                message: "Duplicate target path provided: rust".to_string()
            }
        );
    }
}
