use clap::builder::ArgPredicate;
use clap::{Arg, ArgAction, Command};

fn main() {
    let app = get_app();

    monorail::handle(app)
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

    Command::new("monorail")
    .version(env!("CARGO_PKG_VERSION"))
    .author("Patrick Nordahl <plnordahl@gmail.com>")
    .about("A monorepo overlay for version control systems.")
    .arg(
        Arg::new("config-file")
            .short('f')
            .long("config-file")
            .help("Sets a file to use for configuration")
            .num_args(1)
            .default_value("Monorail.toml"),
    )
    .arg(
        Arg::new("working-directory")
            .short('d')
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
    .subcommand(
        Command::new("checkpoint")
            .about("Perform a checkpoint of changed targets")
            .after_help(r#"This command analyzes changed targets since the last checkpoint, constructs a checkpoint object appropriate for the configured vcs"#)
            .arg(arg_git_path.clone())
            .arg(arg_use_libgit2_status.clone())
            .arg(
                Arg::new("type")
                    .short('t')
                    .long("type")
                    .help("Semver component to increment for this checkpoint")
                    .value_parser(["patch", "minor", "major"])
                    .ignore_case(true)
                    .required(true)
                    .num_args(1),
            )
            .arg(
                Arg::new("dry-run")
                    .short('d')
                    .long("dry-run")
                    .help("Do not apply any changes locally (for a distributed version control system) or remotely")
                    .action(ArgAction::SetTrue),
            )
    )
    .subcommand(
        Command::new("analyze")
            .about("Analyze repository changes and targets")
            .after_help(r#"This command analyzes staged, unpushed, and pushed changes between two checkpoints in version control history, as well as unstaged changes present only in your local filesystem. By default, only outputs a list of affected targets."#)
            .arg(arg_git_path.clone())
            .arg(arg_use_libgit2_status.clone())
            .arg(
                Arg::new("start")
                    .short('s')
                    .long("start")
                    .help("Start of the interval to consider for changes; if not provided, the latest tag (or first commit, if no tags have been made) is used")
                    .num_args(1)
                    .required(false),
            )
            .arg(
                Arg::new("end")
                    .short('e')
                    .long("end")
                    .help("End of the interval to consider for changes; if not provided HEAD is used")
                    .num_args(1)
                    .required(false),
            )
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
                Arg::new("show-all")
                    .long("show-all")
                    .help("Display changes, change targets, and targets")
                    .action(ArgAction::SetTrue),
            )
    )
}
