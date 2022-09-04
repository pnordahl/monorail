use crate::error::ErrorKind;
use std::io::Write;

use clap::ArgMatches;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;

use std::fs::{read_dir, File};
use std::io::BufReader;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::process::Output;
use std::result::Result;
use std::str::FromStr;

use clap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json;
use toml;

pub fn handle(app: clap::App) {
    let matches = app.get_matches();

    let output_format = matches.value_of("output-format").unwrap();

    match matches.value_of("config") {
        Some(cfg_path) => match Config::new(&cfg_path) {
            Ok(cfg) => {
                if let Some(_config) = matches.subcommand_matches("config") {
                    write_output(std::io::stdout(), &cfg, output_format);
                    std::process::exit(0);
                }

                if let Some(inspect) = matches.subcommand_matches("inspect") {
                    match handle_inspect(&cfg, inspect) {
                        Ok(output) => {
                            write_output(std::io::stdout(), &output, output_format);
                            std::process::exit(0);
                        }
                        Err(e) => {
                            write_output(
                                std::io::stderr(),
                                &ErrorOutput {
                                    error: &e.to_string(),
                                },
                                output_format,
                            );
                            std::process::exit(1);
                        }
                    }
                }
            }
            Err(e) => {
                write_output(
                    std::io::stderr(),
                    &ErrorOutput {
                        error: &e.to_string(),
                    },
                    output_format,
                );
                std::process::exit(1);
            }
        },
        None => {
            write_output(
                std::io::stderr(),
                &ErrorOutput {
                    error: "no configuration specified",
                },
                output_format,
            );
            std::process::exit(1);
        }
    };
}

fn handle_inspect<'a>(
    cfg: &'a Config,
    inspect: &'a ArgMatches,
) -> Result<InspectOutput, ErrorKind> {
    let mut output = InspectOutput::new();
    if let Some(change) = inspect.subcommand_matches("change") {
        let from = change.value_of("from").unwrap();
        let to = change.value_of("to").unwrap();
        match cfg.vcs {
            VCSKind::Git => {
                let mut ref_branch_diff = gitfiles_from_diff(git_from_to_diff(from, to)?);

                let mut unstaged = gitfiles_from_unstaged(git_unstaged()?);

                ref_branch_diff.append(&mut unstaged);
                ref_branch_diff.sort();
                ref_branch_diff.dedup();

                let all = &mut ref_branch_diff;

                let repo_root = trim_right(&mut git_repo_root()?, 1);

                // get all top-level subdirectories of {group_name}/{project_directory}
                let mut group_project_dirs: HashMap<String, Vec<String>> = HashMap::new();

                let regexes = cfg.regexes()?;

                let mut link_groups: HashSet<String> = HashSet::new();

                if let Some(cfg_group) = &cfg.group {
                    for (k, _) in cfg_group.iter() {
                        let gi = GroupInspect {
                            change: GroupChange::new(),
                        };
                        output.group.insert(k.to_string(), gi);
                    }
                    for gf in all.iter() {
                        let (group_name, group_subdir, project_name) = gitfile_name_parts(&gf.name);

                        // switch on group, apply regexes
                        if let Some(group_name) = group_name {
                            if let Some(group_change) = output.group.get_mut(group_name) {
                                // short-circuit if this group is flagged due to `group.link` matching
                                // a previous file
                                if link_groups.contains(group_name) {
                                    group_change.change.file.push(GroupFile {
                                        name: gf.name.to_owned(),
                                        action: FileActionKind::Include,
                                        reason: FileReasonKind::GroupLink,
                                    });
                                    continue;
                                }

                                if let Some(group) = cfg_group.get(group_name) {
                                    let group_root = format!("{}/", group_name);
                                    // valid unwrap if group_name is Some
                                    let group_subdir = group_subdir.unwrap();
                                    let group_project_root =
                                        format!("{}{}/", group_root, group.project_directory);
                                    let group_project_fs_root =
                                        format!("{}/{}", repo_root, group_project_root);

                                    if group_project_dirs.get(group_name).is_none() {
                                        let dirs = get_group_project_dirs(&Path::new(
                                            &group_project_fs_root,
                                        ))?;
                                        group_project_dirs.insert(group_name.to_string(), dirs);
                                    }

                                    let dirs = group_project_dirs.get(group_name).unwrap();

                                    // e.g. rust/, all projects should be included
                                    if gf.name == group_root {
                                        group_change.change.file.push(GroupFile {
                                            name: gf.name.to_owned(),
                                            action: FileActionKind::Include,
                                            reason: FileReasonKind::Group,
                                        });

                                        for d in dirs.iter() {
                                            group_change.change.project.insert(d.to_string());
                                        }
                                        continue;
                                    }

                                    let name = gf.name.trim_start_matches(&group_root);

                                    let group_regexes = regexes.group.get(group_name).unwrap();
                                    if let Some(link) = &group_regexes.link {
                                        match link.find(&name) {
                                            Some(mat) => {
                                                link_groups.insert(group_name.to_string());
                                                group_change
                                                    .change
                                                    .link
                                                    .insert(mat.as_str().to_string());

                                                group_change.change.file.push(GroupFile {
                                                    name: gf.name.to_owned(),
                                                    action: FileActionKind::Include,
                                                    reason: FileReasonKind::GroupLink,
                                                });
                                                for d in dirs.iter() {
                                                    group_change
                                                        .change
                                                        .project
                                                        .insert(d.to_string());
                                                }
                                                continue;
                                            }
                                            None => (),
                                        }
                                    }
                                    // for each group, switch on project and apply regexes
                                    if group_subdir == group.project_directory {
                                        // check the map for if this group_project_root has been ls'd
                                        // if not, ls it and store the subdirectory names in the map
                                        if let Some(project_name) = project_name {
                                            let project_root =
                                                format!("{}{}/", group_project_root, project_name);
                                            if gf.name == project_root {
                                                group_change.change.file.push(GroupFile {
                                                    name: gf.name.to_owned(),
                                                    action: FileActionKind::Include,
                                                    reason: FileReasonKind::Project,
                                                });
                                                group_change
                                                    .change
                                                    .project
                                                    .insert(project_name.to_string());
                                                continue;
                                            }
                                            if group.project.is_some() {
                                                // strip project_root from gf.name, and apply regex
                                                let name =
                                                    gf.name.trim_start_matches(&project_root);

                                                if let Some(project_regexes) =
                                                    group_regexes.project.get(project_name)
                                                {
                                                    if let Some(ignore) = &project_regexes.ignore {
                                                        let matched = ignore.is_match(&name);
                                                        match matched {
                                                            true => {
                                                                group_change
                                                                .change
                                                                .file
                                                                .push(GroupFile {
                                                                name: gf.name.to_owned(),
                                                                action: FileActionKind::Exclude,
                                                                reason:
                                                                    FileReasonKind::ProjectIgnore,
                                                            });
                                                                continue;
                                                            }
                                                            false => (),
                                                        }
                                                    }
                                                }
                                            }
                                            group_change.change.file.push(GroupFile {
                                                name: gf.name.to_owned(),
                                                action: FileActionKind::Include,
                                                reason: FileReasonKind::ProjectFile,
                                            });
                                            group_change
                                                .change
                                                .project
                                                .insert(project_name.to_string());
                                            continue;
                                        }
                                    }
                                }
                                // not a file that falls under a configured group
                                group_change.change.file.push(GroupFile {
                                    name: gf.name.to_owned(),
                                    action: FileActionKind::Exclude,
                                    reason: FileReasonKind::Inert,
                                });
                            }
                        }
                    }
                }

                // todo: for groups in link_groups, update all files for that group's projects to be action: Include and reason: GroupLink, since this trumps ignores
            }
        }
    }

    Ok(output)
}

fn get_group_project_dirs(group_project_path: &Path) -> Result<Vec<String>, ErrorKind> {
    match read_dir(group_project_path) {
        Ok(entries) => {
            let mut dirs: Vec<String> = Vec::new();
            for e in entries {
                match e {
                    Ok(e) => match e.file_type() {
                        Ok(file_type) => {
                            if file_type.is_dir() {
                                match e.file_name().into_string() {
                                    Ok(file_name) => dirs.push(file_name),
                                    Err(err) => {
                                        return Err(ErrorKind::Basic(err.into_string().unwrap()))
                                    }
                                }
                            }
                        }
                        Err(err) => return Err(ErrorKind::Io(err, "bad file type".to_string())),
                    },
                    Err(err) => return Err(ErrorKind::Io(err, "bad dir entry".to_string())),
                }
            }
            Ok(dirs)
        }
        Err(err) => Err(ErrorKind::Io(err, "bad group project path".to_string())),
    }
}

#[derive(Serialize)]
struct ErrorOutput<'a> {
    error: &'a str,
}

#[derive(Serialize)]
struct InspectOutput {
    group: HashMap<String, GroupInspect>,
}
impl InspectOutput {
    fn new() -> Self {
        InspectOutput {
            group: HashMap::new(),
        }
    }
}
#[derive(Serialize)]
struct GroupInspect {
    change: GroupChange,
}
#[derive(Serialize)]
struct GroupChange {
    file: Vec<GroupFile>,
    project: HashSet<String>,
    link: HashSet<String>,
}
impl GroupChange {
    fn new() -> Self {
        GroupChange {
            file: Vec::new(),
            project: HashSet::new(),
            link: HashSet::new(),
        }
    }
}
#[derive(Serialize)]
struct GroupFile {
    name: String,
    action: FileActionKind,
    reason: FileReasonKind,
}
#[derive(Serialize)]
enum FileActionKind {
    #[serde(rename = "exclude")]
    Exclude,
    #[serde(rename = "include")]
    Include,
}
#[derive(Serialize)]
enum FileReasonKind {
    #[serde(rename = "group")]
    Group,
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "project_file")]
    ProjectFile,
    #[serde(rename = "group_link")]
    GroupLink,
    #[serde(rename = "project_ignore")]
    ProjectIgnore,
    #[serde(rename = "inert")]
    Inert,
}

fn trim_right(v: &mut std::vec::Vec<u8>, num: usize) -> String {
    v.truncate(v.len() - num);
    String::from_utf8_lossy(&v).to_string()
}
#[derive(Debug)]
enum GitChangeKind {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    UpdatedUnmerged,

    Unknown,
}
impl FromStr for GitChangeKind {
    type Err = ();
    fn from_str(s: &str) -> Result<GitChangeKind, ()> {
        match s {
            "" => Ok(GitChangeKind::Unmodified),
            "M" => Ok(GitChangeKind::Modified),
            "A" => Ok(GitChangeKind::Added),
            "D" => Ok(GitChangeKind::Deleted),
            "R" => Ok(GitChangeKind::Renamed),
            "C" => Ok(GitChangeKind::Copied),
            "U" => Ok(GitChangeKind::UpdatedUnmerged),
            _ => Ok(GitChangeKind::Unknown),
        }
    }
}

#[derive(Debug)]
struct GitFile {
    change_change: GitChangeKind,
    name: String,
}
impl PartialEq for GitFile {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl PartialOrd for GitFile {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for GitFile {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}
impl Eq for GitFile {}

fn git_unstaged<'a>() -> Result<Vec<u8>, ErrorKind> {
    handle_cmd_output(
        Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .output(),
    )
}

fn git_repo_root<'a>() -> Result<Vec<u8>, ErrorKind> {
    handle_cmd_output(
        Command::new("git")
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output(),
    )
}

fn git_from_to_diff<'a>(from: &str, to: &str) -> Result<Vec<u8>, ErrorKind> {
    handle_cmd_output(
        Command::new("git")
            .arg("diff")
            .arg("--name-status")
            .arg(format!("{}..{}", from, to))
            .output(),
    )
}

fn handle_cmd_output<'a>(output: std::io::Result<Output>) -> Result<Vec<u8>, ErrorKind> {
    match output {
        Ok(output) => {
            if output.status.success() {
                Ok(output.stdout)
            } else {
                Err(ErrorKind::Basic(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ))
            }
        }
        Err(e) => Err(ErrorKind::Basic(e.to_string())),
    }
}
fn gitfiles_from_unstaged(s: Vec<u8>) -> Vec<GitFile> {
    let mut v: Vec<GitFile> = Vec::new();
    if s.len() > 0 {
        let iter = s.split(|x| char::from(*x) == '\n');
        for w in iter {
            let mut parts: Vec<Cow<str>> = w
                .split(|z| char::from(*z) == ' ')
                .map(|z| String::from_utf8_lossy(z))
                .collect();
            parts.retain(|z| z != "");
            // not interested in anything else spurious from
            // our parsing of line format
            if parts.len() == 2 {
                v.push(GitFile {
                    change_change: GitChangeKind::from_str(&parts[0]).unwrap(),
                    name: parts[1].to_string(),
                });
            }
        }
        return v;
    }
    v
}
fn gitfiles_from_diff(s: Vec<u8>) -> Vec<GitFile> {
    let mut v: Vec<GitFile> = Vec::new();
    if s.len() > 0 {
        let iter = s.split(|x| char::from(*x) == '\n');
        for w in iter {
            let mut parts: Vec<Cow<str>> = w
                .split(|z| char::from(*z) == '\t')
                .map(|z| String::from_utf8_lossy(z))
                .collect();
            parts.retain(|z| z != "");
            // not interested in anything else spurious from
            // our parsing of line format
            if parts.len() == 2 {
                v.push(GitFile {
                    change_change: GitChangeKind::from_str(&parts[0]).unwrap(),
                    name: parts[1].to_string(),
                });
            }
        }
    }
    v
}

fn gitfile_name_parts(name: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    // extract group, group_subdir, project from gf.name
    // NOTE: assuming here that group.project_directory has no slashes
    let parts: Vec<&str> = name.split("/").collect();

    // ones that affect projects: {group}/{group.project_directory}/...
    let mut group_name = None;
    let mut group_subdir = None;
    let mut project_name = None;
    if parts.len() >= 2 {
        group_name = Some(parts[0]);
        group_subdir = Some(parts[1]);
    }
    // e.g. `rust/project/name/foo.rs`, or `rust/project/name/`
    // ignores `rust/project/foo.rs`
    if parts.len() >= 4 {
        project_name = Some(parts[2]);
    }

    (group_name, group_subdir, project_name)
}

fn write_output<W, T>(writer: W, value: &T, output_format: &str)
where
    W: Write,
    T: Serialize,
{
    match output_format {
        "json" => {
            serde_json::to_writer(writer, value).unwrap();
        }
        _ => (),
    }
}

#[derive(Serialize, Deserialize, Debug)]
enum VCSKind {
    #[serde(rename = "git")]
    Git,
}
impl FromStr for VCSKind {
    type Err = ();
    fn from_str(s: &str) -> Result<VCSKind, ()> {
        match s {
            "git" => Ok(VCSKind::Git),
            _ => Err(()),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    vcs: VCSKind,
    group: Option<HashMap<String, Group>>,
}
#[derive(Serialize, Deserialize, Debug)]
struct Group {
    project_directory: String,
    link: Option<String>,
    project: Option<HashMap<String, Project>>,
}
#[derive(Serialize, Deserialize, Debug)]
struct Project {
    ignore: Option<String>,
}
struct Regexes {
    group: HashMap<String, GroupRegexes>,
}
struct GroupRegexes {
    link: Option<Regex>,
    project: HashMap<String, ProjectRegexes>,
}
struct ProjectRegexes {
    ignore: Option<Regex>,
}

impl Config {
    pub fn new(file_path: &str) -> Result<Config, ErrorKind> {
        let path = Path::new(file_path);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) => return Err(ErrorKind::Io(e, file_path.to_string())),
        };
        let mut buf_reader = BufReader::new(file);
        let mut contents = String::new();
        match buf_reader.read_to_string(&mut contents) {
            Ok(_) => (),
            Err(e) => return Err(ErrorKind::Io(e, file_path.to_string())),
        }

        match toml::from_str(contents.as_str()) {
            Ok(config) => Ok(config),
            Err(e) => return Err(ErrorKind::Toml(e, file_path.to_string())),
        }
    }
    // process all provided regexes and package them up
    pub fn regexes(&self) -> Result<Regexes, ErrorKind> {
        let mut regexes = Regexes {
            group: HashMap::new(),
        };
        if let Some(group) = self.group.as_ref() {
            for (gn, g) in group {
                let mut gr = GroupRegexes {
                    link: None,
                    project: HashMap::new(),
                };
                if let Some(link) = &g.link {
                    match Regex::new(&link) {
                        Ok(r) => gr.link = Some(r),
                        Err(e) => return Err(ErrorKind::Regex(e)),
                    }
                }
                if let Some(project) = &g.project {
                    for (pn, p) in project {
                        let mut pr = ProjectRegexes { ignore: None };
                        if let Some(ignore) = &p.ignore {
                            match Regex::new(&ignore) {
                                Ok(r) => pr.ignore = Some(r),
                                Err(e) => return Err(ErrorKind::Regex(e)),
                            }
                        }
                        gr.project.insert(pn.to_string(), pr);
                    }
                }
                regexes.group.insert(gn.to_string(), gr);
            }
        }
        Ok(regexes)
    }
}
