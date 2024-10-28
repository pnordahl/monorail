use crate::common::error::MonorailError;
use crate::core;

use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, ArgMatches, Command};
use once_cell::sync::OnceCell;
use serde::Serialize;
use std::io::Write;
use std::result::Result;
use std::{env, path};
use tokio::runtime::Runtime;

pub const CMD_MONORAIL: &str = "monorail";
pub const CMD_CONFIG: &str = "config";
pub const CMD_CHECKPOINT: &str = "checkpoint";
pub const CMD_DELETE: &str = "delete";
pub const CMD_UPDATE: &str = "update";
pub const CMD_SHOW: &str = "show";
pub const CMD_TARGET: &str = "target";
pub const CMD_RUN: &str = "run";
pub const CMD_ANALYZE: &str = "analyze";
pub const CMD_RESULT: &str = "result";
pub const CMD_LOG: &str = "log";
pub const CMD_TAIL: &str = "tail";

pub const ARG_GIT_PATH: &str = "git-path";
pub const ARG_START: &str = "start";
pub const ARG_END: &str = "end";
pub const ARG_CONFIG_FILE: &str = "config-file";
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
pub const ARG_DEPS: &str = "deps";
pub const ARG_FAIL_ON_UNDEFINED: &str = "fail-on-undefined";

pub const VAL_JSON: &str = "json";

// clap requires fields to be static, so dynamic fields like this need to be provided as static references
static DEFAULT_CONFIG_PATH: OnceCell<String> = OnceCell::new();

fn default_config_path() -> &'static str {
    DEFAULT_CONFIG_PATH.get_or_init(|| {
        env::current_dir()
            .unwrap()
            .join("Monorail.json")
            .display()
            .to_string()
    })
}

pub fn get_app() -> clap::Command {
    let arg_git_path = Arg::new(ARG_GIT_PATH)
        .long(ARG_GIT_PATH)
        .help("Absolute path to a `git` binary to use for certain operations. If unspecified, default value uses PATH to resolve.")
        .num_args(1)
        .default_value("git");
    let arg_start = Arg::new(ARG_START)
        .short('s')
        .long(ARG_START)
        .help("Start of the interval to consider for changes; if not provided, the checkpoint (or start of history, if no checkpoint exists) is used")
        .num_args(1)
        .required(false);
    let arg_end = Arg::new(ARG_END)
        .short('e')
        .long(ARG_END)
        .help(
            "End of the interval to consider for changes; if not provided, end of history is used",
        )
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
            .help("Absolute path to a configuration file")
            .num_args(1)
            .default_value(default_config_path()),
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
    .subcommand(Command::new(CMD_CONFIG).about("Parse and display configuration").subcommand(Command::new(CMD_SHOW).about("Show configuration, including runtime default values")))
    .subcommand(Command::new(CMD_CHECKPOINT)
        .about("Show, update, or delete the tracking checkpoint")
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
            .arg(
                Arg::new(ARG_ID)
                    .long(ARG_ID)
                    .short('i')
                    .required(false)
                    .num_args(1)
                    .help("ID to use for the updated checkpoint. If not provided, this will default to the beginning of history for the current change provider."),
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
        .about("Display targets and target groups")
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
        .arg(
            Arg::new(ARG_DEPS)
                .long(ARG_DEPS)
                .help("Include target dependencies")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new(ARG_FAIL_ON_UNDEFINED)
                .long(ARG_FAIL_ON_UNDEFINED)
                .long_help("Fail commands that are undefined by targets. If --target is specified, this defaults to true.")
                .default_value_if(ARG_TARGET, ArgPredicate::IsPresent, Some("true"))
                .action(ArgAction::SetTrue),
        )
    )
    .subcommand(Command::new(CMD_RESULT).about("Show historical results from runs").subcommand(Command::new(CMD_SHOW).about("Show results from `run` invocations")))
    /*

    log --tail --result (-r) <id> --command (-f) [f1 f2 ... fN] --target (-t) [t1 t2 ... tN] --stdout --stderr

    */
    // TODO: monorail log delete [--all] --result [r1 r2 r3 ... rN]
    .subcommand(
        Command::new(CMD_LOG)
        .about("Show historical or tail real-time logs")
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
        Command::new(CMD_ANALYZE).about("Analyze repository changes and targets")
            .after_help(r#"This command performs an analysis of historical changes from the checkpoint to end of history in a change provider, as well as pending changes on the filesystem. By default, this command only outputs a list of affected targets."#)
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
                    .help("Display targets grouped according to the dependency graph.")
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
                .help("Scope analysis to only the provided targets.")))
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

#[derive(Debug)]
pub struct OutputOptions<'a> {
    pub format: &'a str,
}

#[tracing::instrument]
pub fn handle<'a>(
    matches: &ArgMatches,
    output_options: &OutputOptions<'a>,
) -> Result<i32, MonorailError> {
    match matches.get_one::<String>(ARG_CONFIG_FILE) {
        Some(config_file) => {
            let config_path = path::Path::new(config_file);
            let work_path = path::Path::new(config_path)
                .parent()
                .ok_or(MonorailError::Generic(format!(
                    "Config file {} has no parent directory",
                    config_path.display()
                )))?;
            let config = core::Config::new(config_path)?;
            if let Some(config_matches) = matches.subcommand_matches(CMD_CONFIG) {
                if config_matches.subcommand_matches(CMD_SHOW).is_some() {
                    write_result(&Ok(config), output_options)?;
                    return Ok(HANDLE_OK);
                }
            }
            if let Some(checkpoint_matches) = matches.subcommand_matches(CMD_CHECKPOINT) {
                if let Some(update_matches) = checkpoint_matches.subcommand_matches(CMD_UPDATE) {
                    return handle_checkpoint_update(
                        &config,
                        update_matches,
                        output_options,
                        work_path,
                    );
                }
                if checkpoint_matches.subcommand_matches(CMD_DELETE).is_some() {
                    return handle_checkpoint_delete(&config, output_options, work_path);
                }
                if checkpoint_matches.subcommand_matches(CMD_SHOW).is_some() {
                    return handle_checkpoint_show(&config, output_options, work_path);
                }
            }
            if let Some(result_matches) = matches.subcommand_matches(CMD_RESULT) {
                if result_matches.subcommand_matches(CMD_SHOW).is_some() {
                    return handle_result_show(&config, result_matches, output_options, work_path);
                }
            }
            if let Some(target_matches) = matches.subcommand_matches(CMD_TARGET) {
                if let Some(list_matches) = target_matches.subcommand_matches(CMD_SHOW) {
                    return handle_target_list(&config, list_matches, output_options, work_path);
                }
            }
            if let Some(analyze_matches) = matches.subcommand_matches(CMD_ANALYZE) {
                return handle_analyze(&config, analyze_matches, output_options, work_path);
            }
            if let Some(run_matches) = matches.subcommand_matches(CMD_RUN) {
                return handle_run(&config, run_matches, output_options, work_path);
            }
            if let Some(log_matches) = matches.subcommand_matches(CMD_LOG) {
                if let Some(tail_matches) = log_matches.subcommand_matches(CMD_TAIL) {
                    return handle_log_tail(&config, tail_matches, output_options);
                }
                if let Some(show_matches) = log_matches.subcommand_matches(CMD_SHOW) {
                    return handle_log_show(&config, show_matches, output_options, work_path);
                }
            }
            Err(MonorailError::from("Command not recognized"))
        }
        None => Err(MonorailError::from("No configuration specified")),
    }
}

fn handle_analyze<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = core::HandleAnalyzeInput::from(matches);
    let res = rt.block_on(core::handle_analyze(config, &i, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_run<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = core::HandleRunInput::try_from(matches).unwrap();
    let invocation_args = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let o = rt.block_on(core::handle_run(config, &i, &invocation_args, work_path))?;
    let mut code = HANDLE_OK;
    if o.failed {
        code = HANDLE_ERR;
    }
    write_result(&Ok(o), output_options)?;
    Ok(code)
}

fn handle_checkpoint_update<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = core::CheckpointUpdateInput::try_from(matches)?;
    let res = rt.block_on(core::handle_checkpoint_update(config, &i, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_checkpoint_show<'a>(
    config: &'a core::Config,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let res = rt.block_on(core::handle_checkpoint_show(config, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_checkpoint_delete<'a>(
    config: &'a core::Config,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let res = rt.block_on(core::handle_checkpoint_delete(config, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_log_tail<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = core::LogTailInput::try_from(matches)?;
    match rt.block_on(core::log_tail(config, &i)) {
        Ok(_) => Ok(HANDLE_OK),
        Err(e) => {
            write_result(&Err::<(), MonorailError>(e), output_options)?;
            Ok(HANDLE_ERR)
        }
    }
}

fn handle_log_show<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let i = core::LogShowInput::try_from(matches)?;
    match core::log_show(config, &i, work_path) {
        Ok(_) => Ok(HANDLE_OK),
        Err(e) => {
            write_result(&Err::<(), MonorailError>(e), output_options)?;
            Ok(HANDLE_ERR)
        }
    }
}

fn handle_target_list<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let res = core::target_show(
        config,
        core::TargetShowInput {
            show_target_groups: matches.get_flag(ARG_TARGET_GROUPS),
        },
        work_path,
    );
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}
fn handle_result_show<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let i = core::ResultShowInput::try_from(matches)?;
    let res = core::result_show(config, work_path, &i);
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

#[derive(Serialize)]
struct TimestampWrapper<T: Serialize> {
    timestamp: String,
    #[serde(flatten)]
    data: T,
}
impl<T: Serialize> TimestampWrapper<T> {
    fn new(data: T) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            data,
        }
    }
}

pub fn write_result<T>(
    value: &Result<T, MonorailError>,
    opts: &OutputOptions<'_>,
) -> Result<(), MonorailError>
where
    T: Serialize,
{
    match opts.format {
        "json" => {
            match value {
                Ok(t) => {
                    let mut writer = std::io::stdout();
                    serde_json::to_writer(&mut writer, &TimestampWrapper::new(t))?;
                    writeln!(writer)?;
                }
                Err(e) => {
                    let mut writer = std::io::stderr();
                    serde_json::to_writer(&mut writer, &TimestampWrapper::new(e))?;
                    writeln!(writer)?;
                }
            }
            Ok(())
        }
        _ => Err(MonorailError::Generic(format!(
            "Unrecognized output format {}",
            opts.format
        ))),
    }
}

impl TryFrom<&clap::ArgMatches> for core::ResultShowInput {
    type Error = MonorailError;
    fn try_from(_cmd: &clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::CheckpointUpdateInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            id: cmd.get_one::<String>(ARG_ID).map(|x: &String| x.as_str()),
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
impl<'a> TryFrom<&'a clap::ArgMatches> for core::HandleRunInput<'a> {
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
            include_deps: cmd.get_flag(ARG_DEPS),
            fail_on_undefined: cmd.get_flag(ARG_FAIL_ON_UNDEFINED),
        })
    }
}

impl<'a> From<&'a clap::ArgMatches> for core::HandleAnalyzeInput<'a> {
    fn from(cmd: &'a clap::ArgMatches) -> Self {
        Self {
            git_opts: core::GitOptions {
                start: cmd
                    .get_one::<String>(ARG_START)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>(ARG_GIT_PATH).unwrap(),
            },
            targets: cmd
                .get_many::<String>(ARG_TARGET)
                .into_iter()
                .flatten()
                .collect(),
            analyze_input: core::AnalyzeInput {
                show_changes: cmd.get_flag(ARG_CHANGES),
                show_change_targets: cmd.get_flag(ARG_CHANGE_TARGETS),
                show_target_groups: cmd.get_flag(ARG_TARGET_GROUPS),
            },
        }
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::LogTailInput {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            filter_input: core::log::FilterInput::try_from(cmd)?,
        })
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::log::FilterInput {
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
            filter_input: core::log::FilterInput::try_from(cmd)?,
            id: cmd.get_one::<usize>(ARG_ID),
        })
    }
}
