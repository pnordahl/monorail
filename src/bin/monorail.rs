use clap::{App, Arg, SubCommand};

fn main() {
    let app = get_app();

    monorail::handle(app)
}

fn get_app<'a, 'b>() -> clap::App<'a, 'b> {
    let arg_use_libgit2_status = Arg::with_name("use-libgit2-status")
        .long("use-libgit2-status")
        .help("Whether to use the slower libgit2 repo function `statuses` as part of change detection. When unset monorail will use the `git` program in a subprocess, which is presently substantially faster.");
    let arg_git_path = Arg::with_name("git-path")
        .long("git-path")
        .help("Absolute path to a `git` binary to use for certain operations. Defaults to `git` on PATH")
        .required(true)
        .takes_value(true)
        .default_value("git");

    App::new("monorail")
    .version(env!("CARGO_PKG_VERSION"))
    .author("Patrick Nordahl <plnordahl@gmail.com>")
    .about("A monorepo overlay for version control systems")
    .arg(
        Arg::with_name("config-file")
            .short("f")
            .long("config-file")
            .help("Sets a file to use for configuration")
            .takes_value(true)
            .default_value("Monorail.toml"),
    )
    .arg(
        Arg::with_name("working-directory")
            .short("d")
            .long("working-directory")
            .help("Sets a directory to use for execution")
            .takes_value(true),
    )
    .arg(
        Arg::with_name("output-format")
            .short("o")
            .long("output-format")
            .help("Format to use for program output")
            .possible_values(vec!["json"].as_slice())
            .default_value("json")
            .takes_value(true),
    )
    .subcommand(SubCommand::with_name("config").about("Show configuration, including runtime default values"))
    .subcommand(
        SubCommand::with_name("release")
            .about("Perform a release of changed targets")
            .help(r#"This command analyzes changed targets since the last release, constructs a release object appropriate for the configured vcs"#)
            .arg(arg_git_path.clone())
            .arg(arg_use_libgit2_status.clone())
            .arg(
                Arg::with_name("type")
                    .short("t")
                    .long("type")
                    .help("Semver component to increment for this release")
                    .possible_values(&["patch", "minor", "major"])
                    .case_insensitive(true)
                    .required(true)
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("dry-run")
                    .short("d")
                    .long("dry-run")
                    .help("Do not apply any changes locally (for a distributed version control system) or remotely"),
            )
    )
    .subcommand(
        SubCommand::with_name("inspect")
            .about("Inspect the repository, producing a report")
            .subcommand(
                SubCommand::with_name("change")
                    .about("Show unstaged and staged repository changes")
                    .help(r#"This command analyzes staged, unpushed, and pushed changes between two locations in version control history, as well as unstaged changes present only in your local filesystem. It applies optional prefix patterns specified in a configuration file, and produces a detailed report of changes in the repository."#)
                    .arg(arg_git_path.clone())
                    .arg(arg_use_libgit2_status.clone())
                    .arg(
                        Arg::with_name("start")
                            .short("s")
                            .long("start")
                            .help("Start of the interval to consider for changes; if not provided, the latest tag (or first commit, if no tags have been made) is used")
                            .takes_value(true)
                            .required(false),
                    )
                    .arg(
                        Arg::with_name("end")
                            .short("e")
                            .long("end")
                            .help("End of the interval to consider for changes; if not provided HEAD is used")
                            .takes_value(true)
                            .required(false),
                    )
                    .arg(
                        Arg::with_name("targets-only")
                            .short("t")
                            .long("targets-only")
                            .help("Only output changed targets")
                    ),
            ),
    )
}
