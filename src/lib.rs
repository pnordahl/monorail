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
enum ErrorClass {
    #[serde(rename = "generic")]
    Generic,
    #[serde(rename = "git2")]
    Git2,
    #[serde(rename = "io")]
    Io,
    #[serde(rename = "toml_deserialize")]
    TomlDeserialize,
    #[serde(rename = "serde_json")]
    SerdeJSON,
    #[serde(rename = "utf8")]
    Utf8Error,
    #[serde(rename = "parse_int")]
    ParseIntError,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
struct MonorailError {
    class: ErrorClass,
    message: String,
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
                            if let Some(create) = checkpoint.subcommand_matches("create") {
                                match handle_checkpoint_create(
                                    &cfg,
                                    HandleCheckpointInput {
                                        dry_run: create.get_flag("dry-run"),
                                        git_path: create.get_one::<String>("git-path").unwrap(),
                                        use_libgit2_status: create.get_flag("use-libgit2-status"),
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
                            }
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
struct GitChangeOptions<'a> {
    start: Option<&'a str>,
    end: Option<&'a str>,
    git_path: &'a str,
    use_libgit2_status: bool,
}

#[derive(Debug)]
struct AnalyzeInput<'a> {
    git_change_options: GitChangeOptions<'a>,
    show_changes: bool,
    show_change_targets: bool,
}
#[derive(Serialize, Debug)]
struct AnalyzeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Vec<AnalyzedChange>>,
    targets: Vec<String>,
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
    #[serde(rename = "links")]
    Links,
    #[serde(rename = "uses")]
    Uses,
    #[serde(rename = "ignores")]
    Ignores,
}

fn handle_analyze<'a>(
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

// set of common prefix ignores         = I
// set of common prefix ignores targets = It
// set of common prefix links           = L
// set of common prefix links targets   = Lt
// set of common prefix uses            = U
// set of common prefix uses targets    = Ut
// set of common prefix targets         = T
// set of output targets                = O

// O = T + Ut + Lt - It
fn analyze(
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
                if let Some(v) = lookups.use2targets.get(m.as_str()) {
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

    // copy the hashmap into the output vector
    for t in output_targets.iter() {
        output.targets.push(t.to_owned());
    }
    output.targets.sort();

    Ok(output)
}

fn git_get_raw_changes(
    input: &GitChangeOptions,
    wd: &str,
) -> Result<Vec<RawChange>, MonorailError> {
    let repo = git2::Repository::open(wd)?;
    let start = match input.start {
        Some(s) => libgit2_find_oid(&repo, s)?,
        None => {
            match libgit2_latest_monorail_tag(&repo, libgit2_find_oid(&repo, "HEAD")?)? {
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

#[derive(Debug, Serialize)]
struct HandleCheckpointInput<'a> {
    dry_run: bool,
    git_path: &'a str,
    use_libgit2_status: bool,
}
#[derive(Debug, Serialize)]
struct CheckpointOutput {
    id: String,
    targets: Vec<String>,
    dry_run: bool,
}

fn handle_checkpoint_create(
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
            let lookups = Lookups::new(cfg)?;
            let o = analyze(lookups, changes, true, false)?;

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
                None => "monorail-1".to_string(),
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

            Ok(CheckpointOutput {
                id: tag_name,
                targets: o.targets,
                dry_run: input.dry_run,
            })
        }
    }
}

// Generates a new checkpoint id given an existing one
fn increment_checkpoint_id(id: &str) -> Result<String, MonorailError> {
    match id.strip_prefix("monorail-") {
        Some(s) => Ok(format!(
            "monorail-{}",
            s.parse::<usize>()
                .map_err(|e| {
                    MonorailError {
                        class: ErrorClass::ParseIntError,
                        message: format!("expected integer to increment for checkpoint: {}", e),
                    }
                })?
                .checked_add(1)
                .ok_or(MonorailError::from(format!(
                    "checkpoint id increment would overflow: {}",
                    s
                )))?
        )),
        None => Err(format!("expected checkpoint id with prefix 'monorail-', got {}", id).into()),
    }
}

#[derive(Serialize)]
struct CheckpointMessage<'a> {
    num_changes: usize,
    targets: &'a [String],
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

// given a commit, find the earliest annotated "monorail-N" tag behind it
// NOTE: this is pub for now, since it's used in integration tests;
// once the `checkpoint list` command is done, make this private
// and move integration tests to the command
pub fn libgit2_latest_monorail_tag(
    repo: &git2::Repository,
    oid: git2::Oid,
) -> Result<Option<git2::Tag>, git2::Error> {
    let o = repo.find_object(oid, None)?;

    let mut dopts = git2::DescribeOptions::new();
    dopts.pattern("monorail-*[0-9]");
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

struct Lookups<'a> {
    targets: Trie<u8>,
    links: Trie<u8>,
    ignores: Trie<u8>,
    uses: Trie<u8>,
    use2targets: HashMap<&'a str, Vec<&'a str>>,
    ignore2targets: HashMap<&'a str, Vec<&'a str>>,
}
impl<'a> Lookups<'a> {
    fn new(cfg: &'a Config) -> Result<Self, MonorailError> {
        let mut targets_builder = TrieBuilder::new();
        let mut links_builder = TrieBuilder::new();
        let mut ignores_builder = TrieBuilder::new();
        let mut uses_builder = TrieBuilder::new();
        let mut use2targets = HashMap::<&str, Vec<&str>>::new();
        let mut ignore2targets = HashMap::<&str, Vec<&str>>::new();

        let mut seen_targets = HashSet::new();
        if let Some(targets) = cfg.targets.as_ref() {
            targets.iter().try_for_each(|target| {
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
                        ignore2targets
                            .entry(s.as_str())
                            .or_default()
                            .push(target.path.as_str());
                    });
                }
                if let Some(uses) = target.uses.as_ref() {
                    uses.iter().for_each(|s| {
                        uses_builder.push(s.as_str());
                        use2targets.entry(s).or_default().push(target.path.as_str());
                    });
                }
                Ok(())
            })?
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

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RawChange {
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

#[derive(Serialize, Deserialize, Debug, Default)]
enum VcsKind {
    #[serde(rename = "git")]
    #[default]
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

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    #[serde(default)]
    vcs: Vcs,
    #[serde(default)]
    extension: Extension,
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
#[derive(Debug, Deserialize, Serialize, Default)]
enum ExtensionKind {
    #[serde(rename = "bash")]
    #[default]
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
struct Target {
    path: String,
    links: Option<Vec<String>>,
    uses: Option<Vec<String>>,
    ignores: Option<Vec<String>>,
}
impl Config {
    fn new(file_path: &str) -> Result<Config, MonorailError> {
        let path = Path::new(file_path);

        let file = File::open(path)?;
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        buf_reader.read_to_string(&mut contents)?;

        let config = toml::from_str(contents.as_str())?;
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
links = [
    "rust/vendor",
    "rust/Cargo.toml",
]
[[targets]]
path = "rust/target"
ignores = [
    "rust/Cargo.toml"
]
uses = [
    "rust/common"
]
"#;

    #[test]
    fn test_libgit2_find_oid() {
        let (repo, repo_path) = get_repo(false);

        let oid = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let oid2 = create_commit(
            &repo,
            &get_tree(&repo),
            "b",
            Some("HEAD"),
            &[&get_commit(&repo, oid)],
        );
        let head = libgit2_find_oid(&repo, "HEAD").unwrap();
        assert_eq!(oid2, head);

        purge_repo(&repo_path);
    }

    #[test]
    fn test_libgit2_latest_monorail_tag() {
        let (repo, repo_path) = get_repo(false);
        let oid1 = create_commit(&repo, &get_tree(&repo), "a", Some("HEAD"), &[]);
        let lt = libgit2_latest_monorail_tag(&repo, oid1).unwrap();
        assert!(lt.is_none());

        let tag_oid = repo
            .tag(
                "monorail-1",
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
            increment_checkpoint_id("monorail-1").unwrap(),
            "monorail-2".to_string()
        );
    }

    #[test]
    fn test_trie() {
        let mut builder = TrieBuilder::new();
        builder.push("rust/target/project1/README.md");
        builder.push("rust/common/log");
        builder.push("rust/common/error");
        builder.push("rust/foo/log");

        let trie = builder.build();

        assert!(trie.exact_match("rust/target/project1/README.md"));
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

    #[test]
    fn test_analyze_empty() {
        let changes = vec![];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, false).unwrap();

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

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }

    #[test]
    fn test_analyze_target_file() {
        let change1 = "rust/lib.rs";
        let changes = vec![RawChange {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let expected_targets = vec![target1.to_string()];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }
    #[test]
    fn test_analyze_target() {
        let change1 = "rust";
        let changes = vec![RawChange {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let expected_targets = vec![target1.to_string()];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![AnalyzedChangeTarget {
                path: target1.to_string(),
                reason: AnalyzedChangeTargetReason::Target,
            }]),
        }];

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }
    #[test]
    fn test_analyze_target_links() {
        let change1 = "rust/vendor";
        let changes = vec![RawChange {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let target2 = "rust/target";
        let expected_targets = vec![target1.to_string(), target2.to_string()];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Links,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Links,
                },
            ]),
        }];

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }

    #[test]
    fn test_analyze_target_uses() {
        let change1 = "rust/common/foo.txt";
        let changes = vec![RawChange {
            name: change1.to_string(),
        }];
        let target1 = "rust";
        let target2 = "rust/target";
        let expected_targets = vec![target1.to_string(), target2.to_string()];
        let expected_changes = vec![AnalyzedChange {
            path: change1.to_string(),
            targets: Some(vec![
                AnalyzedChangeTarget {
                    path: target1.to_string(),
                    reason: AnalyzedChangeTargetReason::Target,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Uses,
                },
            ]),
        }];

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }

    #[test]
    fn test_analyze_target_ignores() {
        let change1 = "rust/Cargo.toml";
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
                    reason: AnalyzedChangeTargetReason::Links,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Links,
                },
                AnalyzedChangeTarget {
                    path: target2.to_string(),
                    reason: AnalyzedChangeTargetReason::Ignores,
                },
            ]),
        }];

        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let lookups = Lookups::new(&c).unwrap();
        let o = analyze(lookups, changes, true, true).unwrap();

        assert_eq!(o.changes.unwrap(), expected_changes);
        assert_eq!(o.targets, expected_targets);
    }

    #[test]
    fn test_lookups() {
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let l = Lookups::new(&c).unwrap();

        assert_eq!(
            l.targets
                .common_prefix_search("rust/target/src/foo.rs")
                .collect::<Vec<String>>(),
            vec!["rust".to_string(), "rust/target".to_string()]
        );
        assert_eq!(
            l.links
                .common_prefix_search("rust/Cargo.toml")
                .collect::<Vec<String>>(),
            vec!["rust/Cargo.toml".to_string()]
        );
        assert_eq!(
            l.uses
                .common_prefix_search("rust/common/foo.txt")
                .collect::<Vec<String>>(),
            vec!["rust/common".to_string()]
        );
        assert_eq!(
            l.ignores
                .common_prefix_search("rust/Cargo.toml")
                .collect::<Vec<String>>(),
            vec!["rust/Cargo.toml".to_string()]
        );
        assert_eq!(
            *l.use2targets.get("rust/common").unwrap(),
            vec!["rust/target"]
        );
        assert_eq!(
            *l.ignore2targets.get("rust/Cargo.toml").unwrap(),
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
