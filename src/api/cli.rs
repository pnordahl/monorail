use crate::app;
use crate::core::{self, error::MonorailError};

use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use once_cell::sync::OnceCell;
use serde::Serialize;
use std::io::Write;
use std::result::Result;
use std::str::FromStr;
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
pub const CMD_OUT: &str = "out";
pub const CMD_RENDER: &str = "render";
pub const CMD_GENERATE: &str = "generate";

pub const ARG_GIT_PATH: &str = "git-path";
pub const ARG_BEGIN: &str = "begin";
pub const ARG_END: &str = "end";
pub const ARG_CONFIG_FILE: &str = "config-file";
pub const ARG_OUTPUT_FORMAT: &str = "output-format";
pub const ARG_PENDING: &str = "pending";
pub const ARG_COMMANDS: &str = "commands";
pub const ARG_TARGETS: &str = "targets";
pub const ARG_CHANGES: &str = "changes";
pub const ARG_CHANGE_TARGETS: &str = "change-targets";
pub const ARG_TARGET_GROUPS: &str = "target-groups";
pub const ARG_SEQUENCES: &str = "sequences";
pub const ARG_ALL: &str = "all";
pub const ARG_VERBOSE: &str = "verbose";
pub const ARG_STDERR: &str = "stderr";
pub const ARG_STDOUT: &str = "stdout";
pub const ARG_ID: &str = "id";
pub const ARG_DEPS: &str = "deps";
pub const ARG_ARG: &str = "arg";
pub const ARG_ARGMAP: &str = "argmap";
pub const ARG_ARGMAP_FILE: &str = "argmap-file";
pub const ARG_FAIL_ON_UNDEFINED: &str = "fail-on-undefined";
pub const ARG_NO_BASE_ARGMAPS: &str = "no-base-argmaps";
pub const ARG_FORMAT: &str = "format";
pub const ARG_TYPE: &str = "type";
pub const ARG_OUTPUT_FILE: &str = "output-file";

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

pub fn build() -> clap::Command {
    let arg_git_path = Arg::new(ARG_GIT_PATH)
        .long(ARG_GIT_PATH)
        .help("Absolute path to a `git` binary to use for certain operations. If unspecified, default value uses PATH to resolve.")
        .num_args(1)
        .default_value("git");
    let arg_begin = Arg::new(ARG_BEGIN)
        .short('b')
        .long(ARG_BEGIN)
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
    let arg_log_command = Arg::new(ARG_COMMANDS)
        .short('c')
        .long(ARG_COMMANDS)
        .required(false)
        .num_args(1..)
        .value_delimiter(' ')
        .action(ArgAction::Append)
        .help("A list commands for which to include logs");
    let arg_log_target = Arg::new(ARG_TARGETS)
        .short('t')
        .long(ARG_TARGETS)
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
    .about("A tool for effective polyglot, multi-project monorepo development.")
    .arg_required_else_help(true)
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
    .subcommand(
        Command::new(CMD_CONFIG)
            .arg_required_else_help(true)
            .about("Parse and display configuration")
            .subcommand(
                Command::new(CMD_SHOW)
                    .about("Show configuration, including runtime default values"))
            .subcommand(
                Command::new(CMD_GENERATE)
                    .about("Generate a configuration file from, and linked to, a source file")
                    .after_help("This command accepts a valid JSON configuration file over stdin that includes a top-level 'source' object, validating and synchronizing the source file and generated files.")))
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
        .arg_required_else_help(true)
        .subcommand(
            Command::new(CMD_RENDER)
                .about("Output a visualization of the target graph.")
                .after_help("This command parses the target graph and emits it in a visual format for use in other tools, such as .dot.")
                .arg(
                    Arg::new(ARG_TYPE)
                        .short('t')
                        .long(ARG_TYPE)
                        .help("File type to render")
                        .required(false)
                        .num_args(1)
                )
                .arg(
                    Arg::new(ARG_OUTPUT_FILE)
                        .short('f')
                        .long(ARG_OUTPUT_FILE)
                        .help("Filesystem location to render to")
                        .required(false)
                        .num_args(1)
                )
        )
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
            .arg(
                Arg::new(ARG_COMMANDS)
                    .short('c')
                    .long(ARG_COMMANDS)
                    .help("Display information for the commands that are defined for this target.")
                    .action(ArgAction::SetTrue),
            )
    ))

    .subcommand(Command::new(CMD_RUN)
        .about("Run target-defined commands.")
        .after_help(r#"
When --target-argmap-file, --target-argmap, and/or --arg are provided, keys that appear multiple times will have their respective arrays concatenated in the following order, after any base argmaps for targets involved in the run:

    1. Each --target-argmap-file, in the order provided
    2. Each --target-argmap literal, in the order provided
    3. Each --arg, in the order provided

Refer to --help for more information on these options.
"#)
        .arg_required_else_help(true)
        .arg(arg_git_path.clone())
        .arg(arg_begin.clone())
        .arg(arg_end.clone())
        .arg(
            Arg::new(ARG_COMMANDS)
                .short('c')
                .long(ARG_COMMANDS)
                .required(false)
                .num_args(1..)
                .value_delimiter(' ')
                .action(ArgAction::Append)
                .help("A list of commands that will be executed, in the order provided. If one or more sequences are provided, they will be expanded and executed first.")
        )
        .arg(
            Arg::new(ARG_SEQUENCES)
                .short('s')
                .long(ARG_SEQUENCES)
                .required(false)
                .num_args(1..)
                .value_delimiter(' ')
                .action(ArgAction::Append)
                .help("A list of command sequences that will be expanded and executed in the order provided. If one or more commands are provided, they will be executed after all sequences.")
        )
        .group(
            ArgGroup::new("commands_and_or_sequences")
                .args([ARG_COMMANDS, ARG_SEQUENCES])
                .required(true)
                .multiple(true),
        )
        .arg(
            Arg::new(ARG_TARGETS)
                .short('t')
                .long(ARG_TARGETS)
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
                .long_help("Fail commands that are undefined by targets.")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new(ARG_NO_BASE_ARGMAPS)
                .long(ARG_NO_BASE_ARGMAPS)
                .long_help("Disable loading base argmaps for run targets. By default, all base argmaps are loaded.")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new(ARG_ARG)
                .short('a')
                .long(ARG_ARG)
                .num_args(1..)
                .required(false)
                .action(ArgAction::Append)
                .help("One or more runtime argument(s) to be provided when executing a command for a single target.")
                .long_help("This is a shorthand form of the more expressive '--target-argmap' and '--target-argmap-file', designed for single command + single target use. Providing this flag without specifying exactly one command and one target will result in an error.")
        )
        .arg(
            Arg::new(ARG_ARGMAP)
                .long(ARG_ARGMAP)
                .short('m')
                .num_args(1..)
                .required(false)
                .action(ArgAction::Append)
                .help("A list of JSON literals containing nested command-target-argument mappings to use when executing commands.")
                .long_help(r#"Each object provided is merged from left to right, and keys that appear in multiple mappings have their arrays of arguments concatenated according to the order of the lists provided.

An argmap has the following form:

{
    "<target>": {
        "<command>": [
            "arg1",
            "arg2",
            ...
            "argN"
        ]
    }
}

See `monorail run -h` for information on how this interacts with other arg-related options.

"#),
        )
        .arg(
            Arg::new(ARG_ARGMAP_FILE)
                .long(ARG_ARGMAP_FILE)
                .short('f')
                .num_args(1..)
                .required(false)
                .action(ArgAction::Append)
                .help("A list of files containing nested command-target-args mappings to use when executing commands.")
                .long_help(r#"Each file provided is merged from left to right, and keys that appear in multiple files have their arrays of arguments concatenated according to the order of the lists provided.

An argmap has the following form:

{
    "<target>": {
        "<command>": [
            "arg1",
            "arg2",
            ...
            "argN"
        ]
    }
}

See `monorail run -h` for information on how this interacts with other arg-related options.
"#),
        )

    )
    .subcommand(Command::new(CMD_RESULT)
        .about("Show historical results from runs")
        .arg_required_else_help(true)
        .subcommand(Command::new(CMD_SHOW)
            .about("Show results from `run` invocations")))

    // TODO: monorail log delete [--all] --result [r1 r2 r3 ... rN]
    .subcommand(
        Command::new(CMD_LOG)
        .about("Show historical or tail real-time logs")
        .arg_required_else_help(true)
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
            .arg(arg_begin.clone())
            .arg(arg_end.clone())
            .arg(
                Arg::new(ARG_CHANGES)
                    .long(ARG_CHANGES)
                    .help("Displays detailed information about changes. If no checkpoint is available to guide change detection, this flag is ignored.")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_CHANGE_TARGETS, ArgPredicate::IsPresent, Some("true"))
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_CHANGE_TARGETS)
                    .long(ARG_CHANGE_TARGETS)
                    .help("For each change, display the targets affected. If no checkpoint is available to guide change detection, this flag is ignored.")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_TARGET_GROUPS)
                    .long(ARG_TARGET_GROUPS)
                    .help("Display targets grouped according to the dependency graph. If no checkpoint is available to guide change detection, all targets are returned.")
                    .action(ArgAction::SetTrue)
                    .default_value_if(ARG_ALL, ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new(ARG_ALL)
                    .long(ARG_ALL)
                    .help("Display changes, change targets, and targets")
                    .action(ArgAction::SetTrue),
            ))
    .subcommand(
        Command::new(CMD_OUT)
        .about("Manipulate data in the monorail output directory")
        .arg_required_else_help(true)
        .subcommand(
            Command::new(CMD_DELETE).about("Delete files from the output dir")
                .after_help(r#""#)
                .arg(
                Arg::new(ARG_ALL)
                    .long(ARG_ALL)
                    .help("Delete all subdirectories")
                    .action(ArgAction::SetTrue),
            )
    ))
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
    let verbosity = matches.get_one::<u8>(ARG_VERBOSE).unwrap_or(&0);
    app::setup_tracing(output_options.format, *verbosity)?;

    match matches.get_one::<String>(ARG_CONFIG_FILE) {
        Some(config_file) => {
            let config_file_path = path::Path::new(&config_file);
            if !config_file_path.is_absolute() {
                return Err(MonorailError::Generic(format!(
                    "Configuration file path '{}' is not absolute",
                    &config_file
                )));
            }
            // Config generation does not use or require an existing config file, but will use the path from `-f`
            if let Some(config_matches) = matches.subcommand_matches(CMD_CONFIG) {
                if config_matches.subcommand_matches(CMD_GENERATE).is_some() {
                    return handle_config_generate(config_file_path, output_options);
                }
            }
            let work_path =
                path::Path::new(config_file_path)
                    .parent()
                    .ok_or(MonorailError::Generic(format!(
                        "Config file {} has no parent directory",
                        config_file_path.display()
                    )))?;
            let config = core::Config::new(config_file_path)?;
            config.check(config_file_path, work_path)?;
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
                if let Some(show_matches) = target_matches.subcommand_matches(CMD_SHOW) {
                    return handle_target_show(&config, show_matches, output_options, work_path);
                }
                if let Some(render_matches) = target_matches.subcommand_matches(CMD_RENDER) {
                    return handle_target_render(
                        &config,
                        render_matches,
                        output_options,
                        work_path,
                    );
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
            if let Some(out_matches) = matches.subcommand_matches(CMD_OUT) {
                if let Some(delete_matches) = out_matches.subcommand_matches(CMD_DELETE) {
                    return handle_out_delete(&config, delete_matches, output_options);
                }
            }
            Err(MonorailError::from("Command not recognized"))
        }
        None => Err(MonorailError::from("No configuration specified")),
    }
}

fn handle_out_delete<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let _guard =
        rt.block_on(core::server::LockServer::new(config.server.lock.clone()).acquire())?;
    let i = app::out::OutDeleteInput::try_from(matches)?;
    let res = app::out::out_delete(&config.out_dir, &i);
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_analyze<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = app::analyze::HandleAnalyzeInput::from(matches);
    let res = rt.block_on(app::analyze::handle_analyze(config, &i, work_path));
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
    let _guard =
        rt.block_on(core::server::LockServer::new(config.server.lock.clone()).acquire())?;
    let i = app::run::HandleRunInput::try_from(matches).unwrap();
    let invocation = env::args().skip(1).collect::<Vec<_>>().join(" ");
    let o = rt.block_on(app::run::handle_run(config, &i, &invocation, work_path))?;
    let mut code = HANDLE_OK;
    if o.failed {
        code = HANDLE_ERR;
    }
    write_result(&Ok(o), output_options)?;
    Ok(code)
}

fn handle_config_generate(
    output_file_path: &path::Path,
    output_options: &OutputOptions<'_>,
) -> Result<i32, MonorailError> {
    let res = app::config::config_generate(app::config::ConfigGenerateInput { output_file_path });
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_checkpoint_update<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let _guard =
        rt.block_on(core::server::LockServer::new(config.server.lock.clone()).acquire())?;
    let i = app::checkpoint::CheckpointUpdateInput::try_from(matches)?;
    let res = rt.block_on(app::checkpoint::handle_checkpoint_update(
        config, &i, work_path,
    ));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_checkpoint_show<'a>(
    config: &'a core::Config,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let res = rt.block_on(app::checkpoint::handle_checkpoint_show(config, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_checkpoint_delete<'a>(
    config: &'a core::Config,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let _guard =
        rt.block_on(core::server::LockServer::new(config.server.lock.clone()).acquire())?;
    let res = rt.block_on(app::checkpoint::handle_checkpoint_delete(config, work_path));
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}

fn handle_log_tail<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
) -> Result<i32, MonorailError> {
    let rt = Runtime::new()?;
    let i = app::log::LogTailInput::try_from(matches)?;
    match rt.block_on(app::log::log_tail(config, &i)) {
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
    let i = app::log::LogShowInput::try_from(matches)?;
    match app::log::log_show(config, &i, work_path) {
        Ok(_) => Ok(HANDLE_OK),
        Err(e) => {
            write_result(&Err::<(), MonorailError>(e), output_options)?;
            Ok(HANDLE_ERR)
        }
    }
}
fn handle_target_render<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let output_file = match matches.get_one::<String>(ARG_OUTPUT_FILE) {
        Some(v) => {
            let vp = path::Path::new(v);
            if vp.is_absolute() {
                vp.to_path_buf()
            } else {
                work_path.join(vp)
            }
        }
        None => work_path.join("target.dot"),
    };
    let render_type = match matches.get_one::<String>(ARG_TYPE) {
        Some(v) => app::target::TargetRenderType::from_str(v)?,
        None => app::target::TargetRenderType::Dot,
    };
    let res = app::target::target_render(
        config,
        app::target::TargetRenderInput {
            render_type,
            output_file,
        },
        work_path,
    );
    write_result(&res, output_options)?;
    Ok(get_code(res.is_err()))
}
fn handle_target_show<'a>(
    config: &'a core::Config,
    matches: &'a ArgMatches,
    output_options: &OutputOptions<'a>,
    work_path: &'a path::Path,
) -> Result<i32, MonorailError> {
    let res = app::target::target_show(
        config,
        app::target::TargetShowInput {
            show_target_groups: matches.get_flag(ARG_TARGET_GROUPS),
            show_commands: matches.get_flag(ARG_COMMANDS),
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
    let i = app::result::ResultShowInput::try_from(matches)?;
    let res = app::result::result_show(config, work_path, &i);
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

impl TryFrom<&clap::ArgMatches> for app::result::ResultShowInput {
    type Error = MonorailError;
    fn try_from(_cmd: &clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for app::checkpoint::CheckpointUpdateInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            id: cmd.get_one::<String>(ARG_ID).map(|x: &String| x.as_str()),
            git_opts: core::git::GitOptions {
                begin: None,
                end: None,
                git_path: cmd
                    .get_one::<String>(ARG_GIT_PATH)
                    .ok_or(MonorailError::MissingArg(ARG_GIT_PATH.into()))?,
            },
            pending: cmd.get_flag(ARG_PENDING),
        })
    }
}
impl<'a> TryFrom<&'a clap::ArgMatches> for app::run::HandleRunInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            git_opts: core::git::GitOptions {
                begin: cmd
                    .get_one::<String>(ARG_BEGIN)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd
                    .get_one::<String>(ARG_GIT_PATH)
                    .ok_or(MonorailError::MissingArg(ARG_GIT_PATH.into()))?,
            },
            commands: cmd
                .get_many::<String>(ARG_COMMANDS)
                .ok_or(MonorailError::MissingArg(ARG_COMMANDS.into()))
                .into_iter()
                .flatten()
                .collect(),
            targets: cmd
                .get_many::<String>(ARG_TARGETS)
                .into_iter()
                .flatten()
                .collect(),
            sequences: cmd
                .get_many::<String>(ARG_SEQUENCES)
                .into_iter()
                .flatten()
                .collect(),
            args: cmd
                .get_many::<String>(ARG_ARG)
                .into_iter()
                .flatten()
                .collect(),
            argmap: cmd
                .get_many::<String>(ARG_ARGMAP)
                .into_iter()
                .flatten()
                .collect(),
            argmap_file: cmd
                .get_many::<String>(ARG_ARGMAP_FILE)
                .into_iter()
                .flatten()
                .collect(),
            include_deps: cmd.get_flag(ARG_DEPS),
            fail_on_undefined: cmd.get_flag(ARG_FAIL_ON_UNDEFINED),
            use_base_argmaps: !cmd.get_flag(ARG_NO_BASE_ARGMAPS),
        })
    }
}

impl<'a> From<&'a clap::ArgMatches> for app::analyze::HandleAnalyzeInput<'a> {
    fn from(cmd: &'a clap::ArgMatches) -> Self {
        Self {
            git_opts: core::git::GitOptions {
                begin: cmd
                    .get_one::<String>(ARG_BEGIN)
                    .map(|x: &String| x.as_str()),
                end: cmd.get_one::<String>(ARG_END).map(|x: &String| x.as_str()),
                git_path: cmd.get_one::<String>(ARG_GIT_PATH).unwrap(),
            },
            analyze_input: app::analyze::AnalyzeInput {
                show_changes: cmd.get_flag(ARG_CHANGES),
                show_change_targets: cmd.get_flag(ARG_CHANGE_TARGETS),
                show_target_groups: cmd.get_flag(ARG_TARGET_GROUPS),
            },
        }
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for app::log::LogTailInput {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            filter_input: core::server::LogFilterInput::try_from(cmd)?,
        })
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for core::server::LogFilterInput {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            include_stdout: cmd.get_flag(ARG_STDOUT),
            include_stderr: cmd.get_flag(ARG_STDERR),
            commands: cmd
                .get_many::<String>(ARG_COMMANDS)
                .into_iter()
                .flatten()
                .map(|x| x.to_owned())
                .collect(),
            targets: cmd
                .get_many::<String>(ARG_TARGETS)
                .into_iter()
                .flatten()
                .map(|x| x.to_owned())
                .collect(),
        })
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for app::log::LogShowInput<'a> {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            filter_input: core::server::LogFilterInput::try_from(cmd)?,
            id: cmd.get_one::<usize>(ARG_ID),
        })
    }
}

impl<'a> TryFrom<&'a clap::ArgMatches> for app::out::OutDeleteInput {
    type Error = MonorailError;
    fn try_from(cmd: &'a clap::ArgMatches) -> Result<Self, Self::Error> {
        Ok(Self {
            all: cmd.get_flag(ARG_ALL),
        })
    }
}
