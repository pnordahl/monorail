pub mod common;

use std::borrow::Cow;
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
    Generic,
    Git2,
    Io,
    TomlDeserialize,
    Command,
    SerdeJSON,
    Utf8Error,
    ParseIntError,
}

#[derive(Debug, Serialize)]
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

pub fn handle(app: clap::App) {
    let matches = app.get_matches();

    let output_format = matches.value_of("output-format").unwrap();

    let wd: String = match matches.value_of("working-directory") {
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

    match matches.value_of("config-file") {
        Some(cfg_path) => {
            match Config::new(Path::new(&wd).join(cfg_path).to_str().unwrap_or(cfg_path)) {
                Ok(cfg) => match cfg.validate() {
                    Ok(()) => {
                        if let Some(_config) = matches.subcommand_matches("config") {
                            write_output(std::io::stdout(), &cfg, output_format).unwrap();
                            std::process::exit(0);
                        }

                        if let Some(release) = matches.subcommand_matches("release") {
                            match handle_release(
                                &cfg,
                                HandleReleaseInput {
                                    release_type: release.value_of("type").unwrap(),
                                    dry_run: release.is_present("dry-run"),
                                    git_path: release.value_of("git-path").unwrap(),
                                    use_libgit2_status: release.is_present("use-libgit2-status"),
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

                        if let Some(inspect) = matches.subcommand_matches("inspect") {
                            if let Some(change) = inspect.subcommand_matches("change") {
                                let i = HandleInspectChangeInput {
                                    start: change.value_of("start"),
                                    end: change.value_of("end"),
                                    git_path: change.value_of("git-path").unwrap(),
                                    use_libgit2_status: change.is_present("use-libgit2-status"),
                                };
                                match handle_inspect_change(&cfg, i, &wd) {
                                    Ok(o) => {
                                        if change.is_present("targets-only") {
                                            let targets: Vec<String> = o.into_iter().collect();
                                            write_output(
                                                std::io::stdout(),
                                                &targets,
                                                output_format,
                                            )
                                            .unwrap();
                                            std::process::exit(0);
                                        }
                                        write_output(std::io::stdout(), &o, output_format).unwrap();
                                        std::process::exit(0);
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

#[derive(Debug, Serialize)]
pub struct HandleReleaseInput<'a> {
    pub release_type: &'a str,
    pub dry_run: bool,
    pub git_path: &'a str,
    pub use_libgit2_status: bool,
}
#[derive(Debug, Serialize)]
pub struct ReleaseOutput {
    pub id: String,
    pub targets: Vec<String>,
    pub dry_run: bool,
}

pub fn handle_release(
    cfg: &Config,
    input: HandleReleaseInput,
    wd: &str,
) -> Result<ReleaseOutput, MonorailError> {
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
                    "HEAD and the last release are the same commit {}, nothing to do",
                    start_oid
                )
                .into());
            }

            // TODO: error on [ci skip]

            // TODO: error if release would include unpushed changes

            // fetch changed targets
            let changes = process_inspect_change(
                &git_all_changes(
                    &repo,
                    start_oid,
                    end_oid,
                    input.use_libgit2_status,
                    input.git_path,
                )?,
                cfg,
            )?;

            let mut changed_targets = changes.into_iter().collect::<Vec<String>>();
            changed_targets.sort();

            // without targets, there's nothing to do
            if changed_targets.is_empty() {
                return Ok(ReleaseOutput {
                    id: "".to_string(),
                    targets: changed_targets,
                    dry_run: input.dry_run,
                });
            }

            // get new tag name
            let tag_name = match latest_tag {
                Some(latest_tag) => match latest_tag.name() {
                    Some(name) => increment_semver(name, input.release_type)?,
                    None => return Err("reference semver not provided".into()),
                },
                None => increment_semver("v0.0.0", input.release_type)?,
            };

            if !input.dry_run {
                // create tag and push
                repo.tag(
                    &tag_name,
                    &repo.find_object(end_oid, None)?,
                    &repo.signature()?,
                    &changed_targets.join("\n"),
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

            Ok(ReleaseOutput {
                id: tag_name,
                targets: changed_targets,
                dry_run: input.dry_run,
            })
        }
    }
}

fn increment_semver(semver: &str, release_type: &str) -> Result<String, MonorailError> {
    let v: Vec<&str> = semver.trim_start_matches('v').split('.').collect();
    if v.len() != 3 {
        return Err(format!("semver should have 3 parts, it has {}", v.len()).into());
    }
    let mut major = v[0].parse::<i32>()?;
    let mut minor = v[1].parse::<i32>()?;
    let mut patch = v[2].parse::<i32>()?;
    match release_type {
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
        _ => return Err(format!("unrecognized release type {}", release_type).into()),
    }
    Ok(format!("v{}.{}.{}", major, minor, patch))
}

pub struct HandleInspectChangeInput<'a> {
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub git_path: &'a str,
    pub use_libgit2_status: bool,
}
pub fn handle_inspect_change(
    cfg: &Config,
    input: HandleInspectChangeInput,
    wd: &str,
) -> Result<InspectChangeOutput, MonorailError> {
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
            process_inspect_change(
                &git_all_changes(&repo, start, end, input.use_libgit2_status, input.git_path)?,
                cfg,
            )
        }
    }
}

#[derive(Debug)]
struct ProcessedChange {
    file: GroupChangeFile,
    group_path: Option<String>,
    project_paths: Option<Vec<String>>,
    depend_path: Option<String>,
    link_path: Option<String>,
    ignore_path: Option<String>,
}
impl ProcessedChange {
    pub fn new(name: &str, target: Option<String>) -> Self {
        ProcessedChange {
            file: GroupChangeFile::new(name, target),
            group_path: None,
            project_paths: None,
            depend_path: None,
            link_path: None,
            ignore_path: None,
        }
    }
}

struct TargetLookup {
    prefixed_ignore: Option<Trie<u8>>,
}
impl TargetLookup {
    pub fn new() -> Self {
        TargetLookup {
            prefixed_ignore: None,
        }
    }
}

struct GroupLookup {
    project_paths: Option<Vec<String>>,
    depend2projects: HashMap<String, HashSet<String>>,
    prefixed_link: Option<Trie<u8>>,
    prefixed_depend: Option<Trie<u8>>,
    prefixed_projects: Option<Trie<u8>>,
    target_lookups: HashMap<String, TargetLookup>,
}
impl GroupLookup {
    pub fn new() -> Self {
        GroupLookup {
            project_paths: None,
            depend2projects: HashMap::new(),
            prefixed_link: None,
            prefixed_depend: None,
            prefixed_projects: None,
            target_lookups: HashMap::new(),
        }
    }
    pub fn populate(&mut self, group: &Group) -> Result<(), MonorailError> {
        let mut depend_hs: HashSet<String> = HashSet::new();
        if let Some(depends) = &group.depend {
            let mut builder = TrieBuilder::new();
            for d in depends {
                let prefixed = format!("{}/{}", &group.path, d);
                depend_hs.insert(prefixed.to_owned());
                builder.push(prefixed);
            }
            self.prefixed_depend = Some(builder.build());
        }
        if let Some(links) = &group.link {
            let mut builder = TrieBuilder::new();
            for l in links {
                builder.push(format!("{}/{}", &group.path, l));
            }
            self.prefixed_link = Some(builder.build());
        }
        if let Some(projects) = &group.project {
            let mut v: Vec<String> = Vec::new();
            let mut builder = TrieBuilder::new();
            for p in projects {
                let prefixed_target = format!("{}/{}", &group.path, &p.path);
                v.push(prefixed_target.to_owned());
                let mut target_lookup = TargetLookup::new();
                if let Some(depends) = &p.depend {
                    for d in depends {
                        let prefixed = format!("{}/{}", &group.path, d);
                        if depend_hs.contains(prefixed.as_str()) {
                            if self.depend2projects.contains_key(&prefixed) {
                                self.depend2projects
                                    .get_mut(&prefixed)
                                    .unwrap()
                                    .insert(prefixed_target.to_owned());
                            } else {
                                let mut hs = HashSet::new();
                                hs.insert(prefixed_target.to_owned());
                                self.depend2projects.insert(prefixed.to_owned(), hs);
                            }
                        } else {
                            return Err(format!(
                                "depend {:?} is not allowed, available: {:?}",
                                prefixed, depend_hs
                            )
                            .into());
                        }
                    }
                }
                if let Some(ignores) = &p.ignore {
                    let mut ignore_builder = TrieBuilder::new();
                    for i in ignores {
                        ignore_builder.push(format!("{}/{}", &prefixed_target, i));
                    }
                    target_lookup.prefixed_ignore = Some(ignore_builder.build());
                }

                builder.push(&prefixed_target);
                self.target_lookups
                    .insert(prefixed_target.to_owned(), target_lookup);
            }
            self.project_paths = Some(v);
            self.prefixed_projects = Some(builder.build());
        }
        Ok(())
    }
}

fn process_inspect_change(
    changes: &[RawChange],
    cfg: &Config,
) -> Result<InspectChangeOutput, MonorailError> {
    let mut output = InspectChangeOutput::new();

    if let Some(cfg_groups) = &cfg.group {
        // prepopulate lookups and output with known groups from config
        let mut builder = TrieBuilder::new();
        let mut group_lookups: HashMap<String, GroupLookup> = HashMap::new();
        for group in cfg_groups.iter() {
            // output keys
            let gi = GroupInspect {
                change: GroupChange::new(),
            };
            output.group.insert(group.path.to_owned(), gi);

            // group lookups
            let mut lookup = GroupLookup::new();
            lookup.populate(group)?;
            group_lookups.insert(group.path.to_owned(), lookup);
            builder.push(&group.path);
        }
        let groups_trie = builder.build();
        for c in changes.iter() {
            let mut pcs = get_processed_changes(c, &group_lookups, &groups_trie)?;
            for pc in pcs.drain(..) {
                // deal with processed change, applying logic specific to output
                if let Some(group_path) = pc.group_path {
                    if let Some(group_inspect) = output.group.get_mut(&group_path) {
                        if pc.file.action == FileActionKind::Use {
                            if let Some(project_paths) = pc.project_paths {
                                for pn in project_paths {
                                    group_inspect.change.project.insert(pn.to_string());
                                }
                            }
                            if let Some(link_path) = pc.link_path {
                                group_inspect.change.link.insert(link_path.to_string());
                            }
                            if let Some(depend_path) = pc.depend_path {
                                group_inspect.change.depend.insert(depend_path.to_string());
                            }
                        }
                        group_inspect.change.file.push(pc.file);
                    }
                }
            }
        }
    }

    Ok(output)
}
fn get_processed_changes(
    change: &RawChange,
    group_lookups: &HashMap<String, GroupLookup>,
    groups_trie: &Trie<u8>,
) -> Result<Vec<ProcessedChange>, MonorailError> {
    let mut pcs: Vec<ProcessedChange> = vec![];
    let group_matches = groups_trie.common_prefix_search(&change.name);
    match group_matches.len() {
        0 => (),
        1 => {
            let group_path_match = String::from_utf8_lossy(&group_matches[0]).to_string();
            let group_lookup = group_lookups.get(&group_path_match);
            match group_lookup {
                Some(group_lookup) => {
                    if let Some(prefixed_link) = &group_lookup.prefixed_link {
                        let group_link_matches = prefixed_link.common_prefix_search(&change.name);
                        if !group_link_matches.is_empty() {
                            let link_path =
                                String::from_utf8_lossy(&group_link_matches[0]).to_string();
                            let mut pc = ProcessedChange::new(&change.name, None);
                            pc.group_path = Some(group_path_match.to_owned());
                            pc.link_path = Some(link_path);
                            pc.project_paths.clone_from(&group_lookup.project_paths);
                            pc.file.action = FileActionKind::Use;
                            pc.file.reason = FileReasonKind::GroupLinkEffect;
                            pcs.push(pc);
                            return Ok(pcs);
                        }
                    }
                    if let Some(prefixed_depend) = &group_lookup.prefixed_depend {
                        let group_depend_matches =
                            prefixed_depend.common_prefix_search(&change.name);
                        if !group_depend_matches.is_empty() {
                            let mut pc = ProcessedChange::new(&change.name, None);
                            pc.group_path = Some(group_path_match.to_owned());
                            let depend_path =
                                String::from_utf8_lossy(&group_depend_matches[0]).to_string();
                            pc.depend_path = Some(depend_path.to_owned());
                            if let Some(projects) = group_lookup.depend2projects.get(&depend_path) {
                                pc.project_paths =
                                    Some(projects.iter().map(|p| p.to_owned()).collect());
                                pc.file.action = FileActionKind::Use;
                                pc.file.reason = FileReasonKind::TargetDependEffect;
                                pcs.push(pc);
                                return Ok(pcs);
                            }
                        }
                    }

                    if let Some(prefixed_projects) = &group_lookup.prefixed_projects {
                        let group_project_matches =
                            prefixed_projects.common_prefix_search(&change.name);
                        for m in group_project_matches.iter() {
                            let mut pc = ProcessedChange::new(&change.name, None);
                            pc.group_path = Some(group_path_match.to_owned());
                            let project_match = String::from_utf8_lossy(m).to_string();
                            let target_lookup = group_lookup.target_lookups.get(&project_match);
                            match target_lookup {
                                Some(target_lookup) => {
                                    pc.project_paths = Some(vec![project_match.to_owned()]);
                                    pc.file.target = Some(project_match);
                                    if let Some(prefixed_ignore) = &target_lookup.prefixed_ignore {
                                        let project_ignore_matches =
                                            prefixed_ignore.common_prefix_search(&change.name);
                                        if !project_ignore_matches.is_empty() {
                                            let project_ignore_match =
                                                String::from_utf8_lossy(&project_ignore_matches[0])
                                                    .to_string();
                                            pc.ignore_path = Some(project_ignore_match);
                                            pc.file.action = FileActionKind::Ignore;
                                            pc.file.reason = FileReasonKind::TargetIgnoreEffect;
                                            pcs.push(pc);
                                            continue;
                                        }
                                    }
                                    pc.file.action = FileActionKind::Use;
                                    pc.file.reason = FileReasonKind::TargetMatch;
                                    pcs.push(pc);
                                }
                                None => {
                                    return Err(format!(
                                        "unexpectedly missing target lookup for {}",
                                        &change.name
                                    )
                                    .into())
                                }
                            }
                        }
                    }

                    if pcs.is_empty() {
                        // this is an inert change, so we return a placeholder
                        let mut pc = ProcessedChange::new(&change.name, None);
                        pc.group_path = Some(group_path_match.to_owned());
                        pcs.push(pc);
                    }
                }
                None => {
                    return Err(
                        format!("unexpectedly missing group lookup for {}", &change.name).into(),
                    )
                }
            }
        }
        x => return Err(format!("{} ambiguous group matches for {}", x, change.name).into()),
    }

    Ok(pcs)
}

#[derive(Serialize, Debug)]
pub struct InspectChangeOutput {
    pub group: HashMap<String, GroupInspect>,
}
impl InspectChangeOutput {
    fn new() -> Self {
        InspectChangeOutput {
            group: HashMap::new(),
        }
    }
}
impl IntoIterator for InspectChangeOutput {
    type Item = String;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let mut vs: Vec<String> = vec![];
        for (_k, v) in self.group {
            let targets = v.into_iter().collect::<Vec<String>>();
            if !targets.is_empty() {
                vs.extend(targets);
            }
        }
        vs.into_iter()
    }
}
#[derive(Serialize, Debug)]
pub struct GroupInspect {
    pub change: GroupChange,
}
impl IntoIterator for GroupInspect {
    type Item = String;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.change.into_iter()
    }
}
#[derive(Serialize, Debug)]
pub struct GroupChange {
    pub file: Vec<GroupChangeFile>,
    pub project: HashSet<String>,
    pub link: HashSet<String>,
    pub depend: HashSet<String>,
}
impl GroupChange {
    fn new() -> Self {
        GroupChange {
            file: Vec::new(),
            project: HashSet::new(),
            link: HashSet::new(),
            depend: HashSet::new(),
        }
    }
}
impl IntoIterator for GroupChange {
    type Item = String;
    type IntoIter = std::vec::IntoIter<String>;

    // TODO: refactor this
    #[allow(clippy::needless_collect)]
    fn into_iter(self) -> Self::IntoIter {
        let v: Vec<String> = self
            .link
            .into_iter()
            .chain(self.depend.into_iter().chain(self.project))
            .collect::<Vec<String>>();
        v.into_iter()
    }
}
#[derive(Serialize, Debug)]
pub struct GroupChangeFile {
    pub name: String,
    pub target: Option<String>,
    pub action: FileActionKind,
    pub reason: FileReasonKind,
}
impl GroupChangeFile {
    pub fn new(name: &str, target: Option<String>) -> Self {
        GroupChangeFile {
            name: name.to_owned(),
            target,
            action: FileActionKind::Ignore,
            reason: FileReasonKind::Inert,
        }
    }
}
#[derive(Serialize, Debug, PartialEq, Eq)]
pub enum FileActionKind {
    #[serde(rename = "ignore")]
    Ignore,
    #[serde(rename = "use")]
    Use,
}
#[derive(Serialize, Debug, PartialEq, Eq)]
pub enum FileReasonKind {
    #[serde(rename = "group_link_effect")]
    GroupLinkEffect,
    #[serde(rename = "project_match")]
    TargetMatch,
    #[serde(rename = "project_ignore_effect")]
    TargetIgnoreEffect,
    #[serde(rename = "project_depend_effect")]
    TargetDependEffect,
    #[serde(rename = "inert")]
    Inert,
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
#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    vcs: Vcs,
    extension: Extension,
    group: Option<Vec<Group>>,
}
#[derive(Serialize, Deserialize, Debug)]
struct Backend {}
#[derive(Serialize, Deserialize, Debug)]
struct Vcs {
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
#[derive(Serialize, Deserialize, Debug)]
struct Extension {
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
struct Group {
    path: String,
    link: Option<Vec<String>>,
    depend: Option<Vec<String>>,
    project: Option<Vec<Project>>,
}
#[derive(Serialize, Deserialize, Debug)]
struct Project {
    path: String,
    ignore: Option<Vec<String>>,
    depend: Option<Vec<String>>,
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

[[group]]
path = "rust"
link = [
    ".cargo",
    "vendor",
    "Cargo.toml"
]
depend = [
    "common/log",
    "common/error"
]
[[group.project]]
path = "target/project1"
ignore = [
    "README.md"
]
depend = [
    "common/log"
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
        let matches = trie.common_prefix_search("rust/common/log/bar.rs");
        assert_eq!(String::from_utf8_lossy(&matches[0]), "rust/common/log");
    }

    #[test]
    fn test_config_new() {
        // TODO
    }

    #[test]
    fn test_process_inspect_change_project_file() {
        let changes = vec![RawChange {
            name: "rust/target/project1/lib.rs".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();

        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/target/project1/lib.rs".to_string());
        assert_eq!(gcf.action, FileActionKind::Use);
        assert_eq!(gcf.reason, FileReasonKind::TargetMatch);

        assert!(gc.project.contains("rust/target/project1"));
        assert!(gc.link.is_empty());
        assert!(gc.depend.is_empty());
    }

    #[test]
    fn test_process_inspect_change_project() {
        let changes = vec![RawChange {
            name: "rust/target/project1/".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();

        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/target/project1/".to_string());
        assert_eq!(gcf.action, FileActionKind::Use);
        assert_eq!(gcf.reason, FileReasonKind::TargetMatch);

        assert!(gc.project.contains("rust/target/project1"));
        assert!(gc.link.is_empty());
        assert!(gc.depend.is_empty());
    }

    #[test]
    fn test_process_inspect_change_group_link() {
        let changes = vec![RawChange {
            name: "rust/vendor/foo/bar.rs".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();

        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/vendor/foo/bar.rs".to_string());
        assert_eq!(gcf.action, FileActionKind::Use);
        assert_eq!(gcf.reason, FileReasonKind::GroupLinkEffect);

        assert!(gc.project.contains("rust/target/project1"));
        assert!(gc.link.contains("rust/vendor"));
        assert!(gc.depend.is_empty());
    }

    #[test]
    fn test_process_inspect_change_project_depend() {
        let changes = vec![RawChange {
            name: "rust/common/log/src/lib.rs".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();

        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/common/log/src/lib.rs".to_string());
        assert_eq!(gcf.action, FileActionKind::Use);
        assert_eq!(gcf.reason, FileReasonKind::TargetDependEffect);

        assert!(gc.project.contains("rust/target/project1"));
        assert!(gc.link.is_empty());
        assert!(gc.depend.contains("rust/common/log"));
    }

    #[test]
    fn test_process_inspect_change_project_ignore() {
        let changes = vec![RawChange {
            name: "rust/target/project1/README.md".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();

        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/target/project1/README.md".to_string());
        assert_eq!(gcf.action, FileActionKind::Ignore);
        assert_eq!(gcf.reason, FileReasonKind::TargetIgnoreEffect);

        assert!(gc.project.is_empty());
        assert!(gc.link.is_empty());
        assert!(gc.depend.is_empty());
    }

    #[test]
    fn test_process_inspect_change_inert() {
        let changes = vec![RawChange {
            name: "rust/inert.rs".to_string(),
        }];
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();
        let o = process_inspect_change(&changes, &c).unwrap();
        let gc = &o.group.get("rust").unwrap().change;
        let gcf = gc.file.get(0).unwrap();
        assert_eq!(gcf.name, "rust/inert.rs".to_string());
        assert_eq!(gcf.action, FileActionKind::Ignore);
        assert_eq!(gcf.reason, FileReasonKind::Inert);

        assert!(gc.project.is_empty());
        assert!(gc.link.is_empty());
        assert!(gc.depend.is_empty());
    }

    #[test]
    fn test_group_lookup_populate() {
        let c: Config = toml::from_str(RAW_CONFIG).unwrap();

        let mut gl = GroupLookup::new();
        gl.populate(&c.group.unwrap().get(0).unwrap()).unwrap();

        assert!(gl
            .depend2projects
            .get("rust/common/log")
            .unwrap()
            .contains("rust/target/project1"));
        assert_eq!(
            gl.prefixed_link
                .unwrap()
                .common_prefix_search("rust/Cargo.toml"),
            vec![Vec::from("rust/Cargo.toml")]
        );
        assert_eq!(
            gl.prefixed_depend
                .unwrap()
                .common_prefix_search("rust/common/log/foo.rs"),
            vec![Vec::from("rust/common/log")]
        );
        assert_eq!(
            gl.prefixed_projects
                .unwrap()
                .common_prefix_search("rust/target/project1/src/foo.rs"),
            vec![Vec::from("rust/target/project1")]
        );
        assert_eq!(
            gl.target_lookups
                .get("rust/target/project1")
                .unwrap()
                .prefixed_ignore
                .as_ref()
                .unwrap()
                .common_prefix_search("rust/target/project1/README.md"),
            vec![Vec::from("rust/target/project1/README.md")]
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
}
