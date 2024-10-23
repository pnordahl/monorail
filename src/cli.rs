use crate::common::error::MonorailError;
use crate::core;

use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, Command};
use serde::Serialize;
use std::env;
use std::io::Write;
use std::path;
use std::result::Result;

pub const CMD_MONORAIL: &str = "monorail";
pub const CMD_CONFIG: &str = "config";
pub const CMD_CHECKPOINT: &str = "checkpoint";
pub const CMD_DELETE: &str = "delete";
pub const CMD_UPDATE: &str = "update";
pub const CMD_SHOW: &str = "show";
pub const CMD_TARGET: &str = "target";
pub const CMD_RUN: &str = "run";
pub const CMD_ANALYSIS: &str = "analysis";
pub const CMD_RESULT: &str = "result";
pub const CMD_LOG: &str = "log";
pub const CMD_TAIL: &str = "tail";

pub const ARG_GIT_PATH: &str = "git-path";
pub const ARG_START: &str = "start";
pub const ARG_END: &str = "end";
pub const ARG_CONFIG_FILE: &str = "config-file";
pub const ARG_WORKING_DIRECTORY: &str = "working-directory";
pub const ARG_OUTPUT_FORMAT: &str = "output-format";
pub const ARG_PENDING: &str = "pending";
pub const ARG_COMMAND: &str = "command";
pub const ARG_TARGET: &str = "target";
pub const ARG_CHANGES: &str = "changes";
pub const ARG_CHANGE_TARGETS: &str = "change-targets";
pub const ARG_TARGET_GROUPS: &str = "target-groups";
pub const ARG_ALL: &str = "all";
pub const ARG_VERBOSE: &str = "verbose";
pub const ARG_STDERR: &str = "stderr";
pub const ARG_STDOUT: &str = "stdout";
pub const ARG_ID: &str = "id";

pub const VAL_JSON: &str = "json";

pub fn get_app() -> clap::Command {
    let arg_git_path = Arg::new(ARG_GIT_PATH)
        .long(ARG_GIT_PATH)
        .help("Absolute path to a `git` binary to use for certain operations. If unspecified, default value uses PATH to resolve.")
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

    let arg_log_id = Arg::new(ARG_ID)
        .long(ARG_ID)
        .short('i')
        .required(false)
        .value_parser(clap::value_parser!(usize))
        .num_args(1)
        .help("Result id to query; if not provided, the most recent will be used");
    let arg_log_command = Arg::new(ARG_COMMAND)
        .short('c')
        .long(ARG_COMMAND)
        .required(false)
        .num_args(1..)
        .value_delimiter(' ')
        .action(ArgAction::Append)
        .help("A list commands for which to include logs");
    let arg_log_target = Arg::new(ARG_TARGET)
        .short('t')
        .long(ARG_TARGET)
        .num_args(1..)
        .value_delimiter(' ')
        .required(false)
        .action(ArgAction::Append)
        .help("A list of targets for which to include logs");

    let arg_log_stderr = Arg::new(ARG_STDERR)
        .long(ARG_STDERR)
        .help("Include stderr logs")
        .action(ArgAction::SetTrue);

    let arg_log_stdout = Arg::new(ARG_STDOUT)
        .long(ARG_STDOUT)
        .help("Include stdout logs")
        .action(ArgAction::SetTrue);

    Command::new(CMD_MONORAIL)
    .version(env!("CARGO_PKG_VERSION"))
    .author("Patrick Nordahl <plnordahl@gmail.com>")
    .about("An overlay for effective monorepo development.")
    .arg(
        Arg::new(ARG_CONFIG_FILE)
            .short('f')
            .long(ARG_CONFIG_FILE)
            .help("Sets a file to use for configuration")
            .num_args(1)
            .default_value("Monorail.json"),
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
    .arg(
        Arg::new(ARG_VERBOSE)
            .short('v')
            .long(ARG_VERBOSE)
            .help("Emit additional workflow, result, and error logging. Specifying this flag multiple times (up to 3) increases the verbosity.")
            .action(ArgAction::Count),
    )
    .subcommand(Command::new(CMD_CONFIG).subcommand(Command::new(CMD_SHOW).about("Show configuration, including runtime default values")))
    .subcommand(Command::new(CMD_CHECKPOINT)
        .subcommand(
        Command::new(CMD_UPDATE)
            .about("Update the tracking checkpoint")
            .after_help(r#"This command updates the tracking checkpoint with data appropriate for the configured vcs."#)
            .arg(arg_git_path.clone())
            .arg(
                Arg::new(ARG_PENDING)
                    .short('p')
                    .long(ARG_PENDING)
                    .help("Add pending changes to the checkpoint. E.g. for git, this means the paths and content checksums of uncommitted and unstaged changes since the last commit.")
                    .action(ArgAction::SetTrue),
            )
        )
        .subcommand(
        Command::new(CMD_DELETE)
            .about("Remove the tracking checkpoint")
            .after_help(r#"This command removes the tracking checkpoint."#)
        )
        .subcommand(
        Command::new(CMD_SHOW)
            .about("Show the tracking checkpoint")
            .after_help(r#"This command displays the tracking checkpoint."#)
        )
    )
    .subcommand(Command::new(CMD_TARGET)
        .subcommand(
        Command::new(CMD_SHOW)
            .about("List targets and their properties.")
            .after_help(r#"This command reads targets from configuration data, and displays their properties."#)
            .arg(
                Arg::new(ARG_TARGET_GROUPS)
                    .short('g')
                    .long(ARG_TARGET_GROUPS)
                    .help("Display a representation of the 'depends on' relationship of targets. The array represents a topological ordering of the graph, with each element of the array being a set of targets that do not depend on each other at that position of the ordering.")
                    .action(ArgAction::SetTrue),
            )
    ))

    .subcommand(Command::new(CMD_RUN)
        .about("Run target-defined commands.")
        .after_help(r#"Execute the provided commands for a graph traversal rooted at the targets specified (optional), or inferred target groups via change detection and graph analysis."#)
        .arg(arg_git_path.clone())
        .arg(arg_start.clone())
        .arg(arg_end.clone())
        .arg(
            Arg::new(ARG_COMMAND)
                .short('c')
                .long(ARG_COMMAND)
                .required(true)
                .num_args(1..)
                .value_delimiter(' ')
                .action(ArgAction::Append)
                .help("A list commands that will be executed, in the order specified.")
        )
        .arg(
            Arg::new(ARG_TARGET)
                .short('t')
                .long(ARG_TARGET)
                .num_args(1..)
                .value_delimiter(' ')
                .required(false)
                .action(ArgAction::Append)
                .help("A list of targets for which commands will be executed. If not provided, target groups will be inferred from the target graph.")
        )
    )
    .subcommand(Command::new(CMD_RESULT).subcommand(Command::new(CMD_SHOW).about("Show results from `run` invocations")))
    /*

    log --tail --result (-r) <id> --command (-f) [f1 f2 ... fN] --target (-t) [t1 t2 ... tN] --stdout --stderr

    */
    // TODO: monorail log delete [--all] --result [r1 r2 r3 ... rN]
    .subcommand(
        Command::new(CMD_LOG)
        .subcommand(
            Command::new(CMD_SHOW).about("Display run logs")
                .after_help(r#"This command shows logs for current or historical run invocations."#)
                .arg(arg_log_id.clone())
                .arg(arg_log_command.clone())
                .arg(arg_log_target.clone())
                .arg(arg_log_stderr.clone())
                .arg(arg_log_stdout.clone()))
        .subcommand(
            Command::new(CMD_TAIL).about("Receive a stream of logs from executed commands")
                .arg(arg_log_id)
                .arg(arg_log_command)
                .arg(arg_log_target)
                .arg(arg_log_stderr)
                .arg(arg_log_stdout))
    )
    .subcommand(
        Command::new(CMD_ANALYSIS).subcommand(Command::new(CMD_SHOW).about("Display an analysis of repository changes and targets")
            .after_help(r#"This command shows an analysis of staged, unpushed, and pushed changes between two checkpoints in version control history, as well as unstaged changes present only in your local filesystem. By default, only outputs a list of affected targets."#)
            .arg(arg_git_path.clone())
            .arg(arg_start.clone())
            .arg(arg_end.clone())
            .arg(
                Arg::new(ARG_CHANGES)
                    .long(ARG_CHANGES)
                    .help("Display changes")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_CHANGE_TARGETS, ArgPredicate::IsPresent, Some("true"))
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_CHANGE_TARGETS)
                    .long(ARG_CHANGE_TARGETS)
                    .help("Display targets for each change")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_TARGET_GROUPS)
                    .long(ARG_TARGET_GROUPS)
                    .help("Display targets grouped according to the dependency graph. Each array in the output array contains the targets that are valid to execute in parallel.")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_ALL)
                    .long(ARG_ALL)
                    .help("Display changes, change targets, and targets")
                    .action(ArgAction::SetTrue),
            )
            .arg(
            Arg::new(ARG_TARGET)
                .short('t')
                .long(ARG_TARGET)
                .required(false)
                .num_args(1..)
                .value_delimiter(' ')
                .action(ArgAction::Append)
                .help("Scope analysis to only the provided targets."))))
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

#[tracing::instrument]
pub async fn handle(matches: &clap::ArgMatches, output_format: &str) -> Result<i32, MonorailError> {
    let work_dir: path::PathBuf = match matches.get_one::<String>(ARG_WORKING_DIRECTORY) {
        Some(wd) => path::Path::new(wd).to_path_buf(),
        None => env::current_dir()?,
    };

    match matches.get_one::<String>(ARG_CONFIG_FILE) {
        Some(cfg_path) => {
            let cfg = core::Config::new(&work_dir.join(cfg_path))?;
            if let Some(config) = matches.subcommand_matches(CMD_CONFIG) {
                if config.subcommand_matches(CMD_SHOW).is_some() {
                    write_result(&Ok(cfg), output_format)?;
                    return Ok(HANDLE_OK);
                }
            }
            if let Some(checkpoint) = matches.subcommand_matches(CMD_CHECKPOINT) {
                if let Some(update) = checkpoint.subcommand_matches(CMD_UPDATE) {
                    let i = core::HandleCheckpointUpdateInput::try_from(update)?;
                    let res = core::handle_checkpoint_update(&cfg, &i, &work_dir).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
                if checkpoint.subcommand_matches(CMD_DELETE).is_some() {
                    let res = core::handle_checkpoint_delete(&cfg, &work_dir).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
                if checkpoint.subcommand_matches(CMD_SHOW).is_some() {
                    let res = core::handle_checkpoint_show(&cfg, &work_dir).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(result) = matches.subcommand_matches(CMD_RESULT) {
                if result.subcommand_matches(CMD_SHOW).is_some() {
                    let i = core::HandleResultShowInput::try_from(result)?;
                    let res = core::handle_result_show(&cfg, &i, &work_dir).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(target) = matches.subcommand_matches(CMD_TARGET) {
                if let Some(list) = target.subcommand_matches(CMD_SHOW) {
                    let res = core::handle_target_show(
                        &cfg,
                        core::HandleTargetShowInput {
                            show_target_groups: list.get_flag(ARG_TARGET_GROUPS),
                        },
                        &work_dir,
                    );
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(analyze) = matches.subcommand_matches(CMD_ANALYSIS) {
                if let Some(show) = analyze.subcommand_matches(CMD_SHOW) {
                    let i = core::AnalysisShowInput::from(show);
                    let res = core::handle_analyze_show(&cfg, &i, &work_dir).await;
                    write_result(&res, output_format)?;
                    return Ok(get_code(res.is_err()));
                }
            }

            if let Some(run) = matches.subcommand_matches(CMD_RUN) {
                let i = core::RunInput::try_from(run).unwrap();
                match core::handle_run(&cfg, &i, &work_dir).await {
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
            if let Some(log) = matches.subcommand_matches(CMD_LOG) {
                if let Some(tail) = log.subcommand_matches(CMD_TAIL) {
                    let i = core::LogStreamArgs::try_from(tail)?;
                    match core::handle_log_tail(&cfg, &i).await {
                        Err(e) => {
                            write_result(&Err::<(), MonorailError>(e), output_format)?;
                            return Ok(HANDLE_ERR);
                        }
                        Ok(_) => return Ok(HANDLE_OK),
                    }
                }
                if let Some(show) = log.subcommand_matches(CMD_SHOW) {
                    let i = core::LogShowInput::try_from(show)?;
                    match core::handle_log_show(&cfg, &i, &work_dir) {
                        Err(e) => {
                            write_result(&Err::<(), MonorailError>(e), output_format)?;
                            return Ok(HANDLE_ERR);
                        }
                        Ok(_) => return Ok(HANDLE_OK),
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

impl TryFrom<&clap::ArgMatches> for core::HandleResultShowInput {
    type Error = MonorailError;
    fn try_from(_cmd: &clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::HandleCheckpointUpdateInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_opts: core::GitOptions {
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
            git_opts: core::GitOptions {
                start: cmd
                    .get_one::<String>(ARG_START)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd
                    .get_one::<String>(ARG_GIT_PATH)
                    .ok_or(MonorailError::MissingArg(ARG_GIT_PATH.into()))?,
            },
            commands: cmd
                .get_many::<String>(ARG_COMMAND)
                .ok_or(MonorailError::MissingArg(ARG_COMMAND.into()))
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

impl<'a> From<&'a clap::ArgMatches> for core::AnalysisShowInput<'a> {
    fn from(cmd: &'a clap::ArgMatches) -> Self {
        Self {
            git_opts: core::GitOptions {
                start: cmd
                    .get_one::<String>(ARG_START)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>(ARG_GIT_PATH).unwrap(),
            },
            show_changes: cmd.get_flag(ARG_CHANGES),
            show_change_targets: cmd.get_flag(ARG_CHANGE_TARGETS),
            show_target_groups: cmd.get_flag(ARG_TARGET_GROUPS),
            targets: cmd
                .get_many::<String>(ARG_TARGET)
                .into_iter()
                .flatten()
                .collect(),
        }
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::LogStreamArgs {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            include_stdout: cmd.get_flag(ARG_STDOUT),
            include_stderr: cmd.get_flag(ARG_STDERR),
            commands: cmd
                .get_many::<String>(ARG_COMMAND)
                .into_iter()
                .flatten()
                .map(|x| x.to_owned())
                .collect(),
            targets: cmd
                .get_many::<String>(ARG_TARGET)
                .into_iter()
                .flatten()
                .map(|x| x.to_owned())
                .collect(),
        })
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::LogShowInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            args: core::LogStreamArgs::try_from(cmd)?,
            id: cmd.get_one::<usize>(ARG_ID),
        })
    }
}
