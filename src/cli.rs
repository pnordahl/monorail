use crate::core;
use crate::error::MonorailError;

use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, Command};
use serde::Serialize;
use std::env;
use std::io::Write;
use std::path::Path;
use std::result::Result;

pub const CMD_MONORAIL: &str = "monorail";
pub const CMD_CONFIG: &str = "config";
pub const CMD_CHECKPOINT: &str = "checkpoint";
pub const CMD_UPDATE: &str = "update";
pub const CMD_TARGET: &str = "target";
pub const CMD_LIST: &str = "list";
pub const CMD_RUN: &str = "run";
pub const CMD_ANALYZE: &str = "analyze";

pub const ARG_GIT_PATH: &str = "git-path";
pub const ARG_START: &str = "start";
pub const ARG_END: &str = "end";
pub const ARG_CONFIG_FILE: &str = "config-file";
pub const ARG_WORKING_DIRECTORY: &str = "working-directory";
pub const ARG_OUTPUT_FORMAT: &str = "output-format";
pub const ARG_PENDING: &str = "pending";
pub const ARG_FUNCTION: &str = "function";
pub const ARG_TARGET: &str = "target";
pub const ARG_SHOW_CHANGES: &str = "show-changes";
pub const ARG_SHOW_CHANGE_TARGETS: &str = "show-change-targets";
pub const ARG_SHOW_TARGET_GROUPS: &str = "show-target-groups";
pub const ARG_SHOW_ALL: &str = "show-all";

pub const VAL_JSON: &str = "json";

pub fn get_app() -> clap::Command {
    let arg_git_path = Arg::new(ARG_GIT_PATH)
        .long(ARG_GIT_PATH)
        .help("Absolute path to a `git` binary to use for certain operations. Defaults to `git` on PATH")
        .num_args(1)
        .default_value("git");
    let arg_start = Arg::new(ARG_START)
                .short('s')
                .long(ARG_START)
                .help("Start of the interval to consider for changes; if not provided, the latest tag (or first commit, if no tags have been made) is used")
                .num_args(1)
                .required(false);
    let arg_end = Arg::new(ARG_END)
        .short('e')
        .long(ARG_END)
        .help("End of the interval to consider for changes; if not provided HEAD is used")
        .num_args(1)
        .required(false);

    Command::new(CMD_MONORAIL)
    .version(env!("CARGO_PKG_VERSION"))
    .author("Patrick Nordahl <plnordahl@gmail.com>")
    .about("An overlay for effective monorepo development.")
    .arg(
        Arg::new(ARG_CONFIG_FILE)
            .short('c')
            .long(ARG_CONFIG_FILE)
            .help("Sets a file to use for configuration")
            .num_args(1)
            .default_value("Monorail.toml"),
    )
    .arg(
        Arg::new(ARG_WORKING_DIRECTORY)
            .short('w')
            .long(ARG_WORKING_DIRECTORY)
            .help("Sets a directory to use for execution")
            .num_args(1),
    )
    .arg(
        Arg::new(ARG_OUTPUT_FORMAT)
            .short('o')
            .long(ARG_OUTPUT_FORMAT)
            .help("Format to use for program output")
            .value_parser([VAL_JSON])
            .default_value(VAL_JSON)
            .num_args(1),
    )
    .subcommand(Command::new(CMD_CONFIG).about("Show configuration, including runtime default values"))
    .subcommand(Command::new(CMD_CHECKPOINT)
        .subcommand(
        Command::new(CMD_UPDATE)
            .about("Update the tracking checkpoint")
            .after_help(r#"This command updates the tracking checkpoint file with data appropriate for the configured vcs."#)
            .arg(arg_git_path.clone())
            .arg(
                Arg::new(ARG_PENDING)
                    .short('p')
                    .long(ARG_PENDING)
                    .help("Add pending changes to the checkpoint. E.g. for git, this means the paths and content checksums of uncommitted and unstaged changes since the last commit.")
                    .action(ArgAction::SetTrue),
            )
        )
        // TODO: monorail checkpoint show
        // TODO: monorail checkpoint clear
    )
    .subcommand(Command::new(CMD_TARGET)
        .subcommand(
        Command::new(CMD_LIST)
            .about("List targets and their properties.")
            .after_help(r#"This command reads targets from configuration data, and displays their properties."#)
            .arg(
                Arg::new(ARG_SHOW_TARGET_GROUPS)
                    .short('g')
                    .long(ARG_SHOW_TARGET_GROUPS)
                    .help("Display a representation of the 'depends on' relationship of targets. The array represents a topological ordering of the graph, with each element of the array being a set of targets that do not depend on each other at that position of the ordering.")
                    .action(ArgAction::SetTrue),
            )
    ))

    .subcommand(Command::new(CMD_RUN)
        .about("Run target-defined functions.")
        .after_help(r#"This command analyzes the target graph and performs parallel or serial execution of the provided function names."#)
        .arg(arg_git_path.clone())
        .arg(arg_start.clone())
        .arg(arg_end.clone())
        .arg(
            Arg::new(ARG_FUNCTION)
                .short('f')
                .long(ARG_FUNCTION)
                .required(true)
                .action(ArgAction::Append)
                .help("A list functions that will be executed, in the order specified.")
        )
        .arg(
            Arg::new(ARG_TARGET)
                .short('t')
                .long(ARG_TARGET)
                .required(false)
                .action(ArgAction::Append)
                .help("A list of targets for which functions will be executed.")
        )
    )
    // TODO: monorail change analyze?
    .subcommand(
        Command::new(CMD_ANALYZE)
            .about("Analyze repository changes and targets")
            .after_help(r#"This command analyzes staged, unpushed, and pushed changes between two checkpoints in version control history, as well as unstaged changes present only in your local filesystem. By default, only outputs a list of affected targets."#)
            .arg(arg_git_path.clone())
            .arg(arg_start.clone())
            .arg(arg_end.clone())
            .arg(
                Arg::new(ARG_SHOW_CHANGES)
                    .long(ARG_SHOW_CHANGES)
                    .help("Display changes")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_SHOW_CHANGE_TARGETS, ArgPredicate::IsPresent, Some("true"))
                    .default_value_if(ARG_SHOW_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_SHOW_CHANGE_TARGETS)
                    .long(ARG_SHOW_CHANGE_TARGETS)
                    .help("Display targets for each change")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_SHOW_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_SHOW_TARGET_GROUPS)
                    .long(ARG_SHOW_TARGET_GROUPS)
                    .help("Display targets grouped according to the dependency graph. Each array in the output array contains the targets that are valid to execute in parallel.")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_SHOW_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_SHOW_ALL)
                    .long(ARG_SHOW_ALL)
                    .help("Display changes, change targets, and targets")
                    .action(ArgAction::SetTrue),
            )
            .arg(
            Arg::new(ARG_TARGET)
                .short('t')
                .long(ARG_TARGET)
                .required(false)
                .action(ArgAction::Append)
                .help("Scope analysis to only the provided targets.")
        )
    )
}

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

pub async fn handle(matches: &clap::ArgMatches, output_format: &str) -> Result<i32, MonorailError> {
    let wd: String = match matches.get_one::<String>(ARG_WORKING_DIRECTORY) {
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

    match matches.get_one::<String>(ARG_CONFIG_FILE) {
        Some(cfg_path) => {
            let mut cfg =
                core::Config::new(Path::new(&wd).join(cfg_path).to_str().unwrap_or(cfg_path))?;
            cfg.workdir.clone_from(&wd);
            cfg.validate()?;

            if let Some(_config) = matches.subcommand_matches(CMD_CONFIG) {
                write_result(&Ok(cfg), output_format)?;
                return Ok(HANDLE_OK);
            }
            if let Some(checkpoint) = matches.subcommand_matches(CMD_CHECKPOINT) {
                if let Some(update) = checkpoint.subcommand_matches(CMD_UPDATE) {
                    let i = core::HandleCheckpointUpdateInput::try_from(update)?;
                    let res = core::handle_checkpoint_update(&cfg, &i, &wd).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(target) = matches.subcommand_matches(CMD_TARGET) {
                if let Some(list) = target.subcommand_matches(CMD_LIST) {
                    let res = core::handle_target_list(
                        &cfg,
                        core::HandleTargetListInput {
                            show_target_groups: list.get_flag(ARG_SHOW_TARGET_GROUPS),
                        },
                    );
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(analyze) = matches.subcommand_matches(CMD_ANALYZE) {
                let i = core::AnalyzeInput::from(analyze);
                let res = core::handle_analyze(&cfg, &i, &wd).await;
                write_result(&res, output_format)?;
                return Ok(get_code(res.is_err()));
            }

            if let Some(run) = matches.subcommand_matches(CMD_RUN) {
                let i = core::RunInput::try_from(run).unwrap();
                match core::handle_run(&cfg, &i, &wd).await {
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

impl<'a> TryFrom<&'a clap::ArgMatches> for core::HandleCheckpointUpdateInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_change_options: core::GitChangeOptions {
                start: None,
                end: None,
                git_path: cmd
                    .get_one::<String>(ARG_GIT_PATH)
                    .ok_or(MonorailError::MissingArg(ARG_GIT_PATH.into()))?,
            },
            pending: cmd.get_flag(ARG_PENDING),
        })
    }
}
impl<'a> TryFrom<&'a clap::ArgMatches> for core::RunInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_change_options: core::GitChangeOptions {
                start: cmd
                    .get_one::<String>(ARG_START)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd
                    .get_one::<String>(ARG_GIT_PATH)
                    .ok_or(MonorailError::MissingArg(ARG_GIT_PATH.into()))?,
            },
            functions: cmd
                .get_many::<String>(ARG_FUNCTION)
                .ok_or(MonorailError::MissingArg(ARG_FUNCTION.into()))
                .into_iter()
                .flatten()
                .collect(),
            targets: cmd
                .get_many::<String>(ARG_TARGET)
                .into_iter()
                .flatten()
                .collect(),
        })
    }
}

impl<'a> From<&'a clap::ArgMatches> for core::AnalyzeInput<'a> {
    fn from(cmd: &'a clap::ArgMatches) -> Self {
        Self {
            git_change_options: core::GitChangeOptions {
                start: cmd
                    .get_one::<String>(ARG_START)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>(ARG_GIT_PATH).unwrap(),
            },
            show_changes: cmd.get_flag(ARG_SHOW_CHANGES),
            show_change_targets: cmd.get_flag(ARG_SHOW_CHANGE_TARGETS),
            show_target_groups: cmd.get_flag(ARG_SHOW_TARGET_GROUPS),
            targets: cmd
                .get_many::<String>(ARG_TARGET)
                .into_iter()
                .flatten()
                .collect(),
        }
    }
}
