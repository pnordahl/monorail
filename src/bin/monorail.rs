use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, Command};

#[tokio::main]
async fn main() {
    let app = get_app();
    let matches = app.get_matches();
    let output_format = matches.get_one::<String>("output-format").unwrap();

    match monorail::handle(&matches, output_format).await {
        Ok(code) => {
            std::process::exit(code);
        }
        Err(e) => {
            monorail::write_result::<()>(&Err(e), output_format)
                .expect("Failed to write fatal result");
            std::process::exit(monorail::HANDLE_FATAL);
        }
    }
}

fn get_app() -> clap::Command {
    let arg_use_libgit2_status = Arg::new("use-libgit2-status")
        .long("use-libgit2-status")
        .help("Whether to use the slower libgit2 repo function `statuses` as part of change detection. When unset monorail will use the `git` program in a subprocess, which is presently substantially faster.")
        .action(ArgAction::SetTrue);
    let arg_git_path = Arg::new("git-path")
        .long("git-path")
        .help("Absolute path to a `git` binary to use for certain operations. Defaults to `git` on PATH")
        .num_args(1)
        .default_value("git");
    let arg_start = Arg::new("start")
                .short('s')
                .long("start")
                .help("Start of the interval to consider for changes; if not provided, the latest tag (or first commit, if no tags have been made) is used")
                .num_args(1)
                .required(false);
    let arg_end = Arg::new("end")
        .short('e')
        .long("end")
        .help("End of the interval to consider for changes; if not provided HEAD is used")
        .num_args(1)
        .required(false);

    Command::new("monorail")
    .version(env!("CARGO_PKG_VERSION"))
    .author("Patrick Nordahl <plnordahl@gmail.com>")
    .about("An overlay for effective monorepo development.")
    .arg(
        Arg::new("config-file")
            .short('c')
            .long("config-file")
            .help("Sets a file to use for configuration")
            .num_args(1)
            .default_value("Monorail.toml"),
    )
    .arg(
        Arg::new("working-directory")
            .short('w')
            .long("working-directory")
            .help("Sets a directory to use for execution")
            .num_args(1),
    )
    .arg(
        Arg::new("output-format")
            .short('o')
            .long("output-format")
            .help("Format to use for program output")
            .value_parser(["json"])
            .default_value("json")
            .num_args(1),
    )
    .subcommand(Command::new("config").about("Show configuration, including runtime default values"))
    .subcommand(Command::new("checkpoint")
        .subcommand(
        Command::new("update")
            .about("Update the tracking checkpoint")
            .after_help(r#"This command updates the tracking checkpoint file with data appropriate for the configured vcs."#)
            .arg(arg_git_path.clone())
            .arg(arg_use_libgit2_status.clone())
            .arg(
                Arg::new("pending")
                    .short('p')
                    .long("pending")
                    .help("Add pending changes to the checkpoint. E.g. for git, this means the paths and content checksums of uncommitted and unstaged changes since the last commit.")
                    .action(ArgAction::SetTrue),
            )
        )
        // TODO: monorail checkpoint show
        // TODO: monorail checkpoint clear
    )
    .subcommand(Command::new("target")
        .subcommand(
        Command::new("list")
            .about("List targets and their properties.")
            .after_help(r#"This command reads targets from configuration data, and displays their properties."#)
            .arg(
                Arg::new("show-target-groups")
                    .short('g')
                    .long("show-target-groups")
                    .help("Display a representation of the 'depends on' relationship of targets. The array represents a topological ordering of the graph, with each element of the array being a set of targets that do not depend on each other at that position of the ordering.")
                    .action(ArgAction::SetTrue),
            )
    ))

    .subcommand(Command::new("run")
        .about("Run target-defined functions.")
        .after_help(r#"This command analyzes the target graph and performs parallel or serial execution of the provided function names."#)
        .arg(arg_git_path.clone())
        .arg(arg_use_libgit2_status.clone())
        .arg(arg_start.clone())
        .arg(arg_end.clone())
        .arg(
            Arg::new("function")
                .short('f')
                .long("function")
                .required(true)
                .action(ArgAction::Append)
                .help("A list functions that will be executed, in the order specified.")
        )
        .arg(
            Arg::new("target")
                .short('t')
                .long("target")
                .required(false)
                .action(ArgAction::Append)
                .help("A list of targets for which functions will be executed.")
        )
    )
    // TODO: monorail change analyze?
    .subcommand(
        Command::new("analyze")
            .about("Analyze repository changes and targets")
            .after_help(r#"This command analyzes staged, unpushed, and pushed changes between two checkpoints in version control history, as well as unstaged changes present only in your local filesystem. By default, only outputs a list of affected targets."#)
            .arg(arg_git_path.clone())
            .arg(arg_use_libgit2_status.clone())
            .arg(arg_start.clone())
            .arg(arg_end.clone())
            .arg(
                Arg::new("show-changes")
                    .long("show-changes")
                    .help("Display changes")
                    .action(ArgAction::SetTrue)
                    .default_value_if("show-change-targets", ArgPredicate::IsPresent, Some("true"))
                    .default_value_if("show-all", ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new("show-change-targets")
                    .long("show-change-targets")
                    .help("Display targets for each change")
                    .action(ArgAction::SetTrue)
                    .default_value_if("show-all", ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new("show-target-groups")
                    .long("show-target-groups")
                    .help("Display targets grouped according to the dependency graph. Each array in the output array contains the targets that are valid to execute in parallel.")
                    .action(ArgAction::SetTrue)
                    .default_value_if("show-all", ArgPredicate::IsPresent, Some("true")),
            )
            .arg(
                Arg::new("show-all")
                    .long("show-all")
                    .help("Display changes, change targets, and targets")
                    .action(ArgAction::SetTrue),
            )
    )
}
