# monorail
> A tool for effective polyglot, multi-project monorepo development.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` is optimized to support the needs of single or multi-project/multi-language monorepos. Using a lightweight graph configuration, it provides APIs for analyzing changes, parallel command execution, logging, and more. It is designed for flexibility and speed, to output structured text for use in other systems, and compose with existing compilers, interpreters, and tools without the need for bespoke rules or plugins.

See the [tutorial](#tutorial) below for a practical walkthrough of how `monorail` works.

## Installation

### UNIX/Linux variants
At present, only source builds are supported. Packages for popular managers will be provided at a later time.

Ensure that Rust 1.63+ is installed and available on your system path:
* [Rust](https://www.rust-lang.org/tools/install)

Run `cargo install --path .`

### Windows

At this time, Windows is unsupported. However, there are no technical limitations preventing its support in the future.

## Overview

`monorail` blends features of build systems like `bazel` and `buck` with the simplicity and flexibility of command runners like `make` and `just`. It manages dependencies and target mapping with `bazel`-like change tracking, enabling selective rebuilds and efficient parallel execution. Additionally, `monorail` provides a CLI-driven workflow for running custom code in any language across any subset of your monorepo.

`monorail` is internally driven by two things:

1. A graph representation of your repository, built from a list of `target` entries and each target's `uses` list. 
2. A change detection provider, which provides a view of changes in the repository

Changes are mapped to affected targets, and a graph traversal powers various dependency-related tasks such as target grouping for parallel execution, "depended upon by" analysis, and so forth.

Monorail has a small lexicon:

* `change`: a created, updated, or deleted filesystem path as reported by a change provider
* `checkpoint`: a location in change provider history that marks the beginning of an interval of changes
* `target`: a unique container that can be referenced by change detection and command execution
* `uses`: a set of paths that a target depends upon
* `ignores`: a set of paths that should not affect a target during change detection
* `command`: executable code written in any language and invoked per target
* `sequence`: a named list of commands

See the [Tutorial](./TUTORIAL.md) for a practical walkthrough of using `monorail` to build a toy monorepo, or [Monorail Example Repo](https://github.com/pnordahl/monorail-example) for a pre-built example.

## Roadmap

`monorail` is suitable for production use, but remains under active development. The following features are either planned, or in progress:

- Remote execution
- Signal capture and command interruption
- Fine-grained command execution scheduling control
- Various convenience APIs
- Additional change provider backends
- Expanded examples for various languages and CI/CD setups

## Concepts

Each of the core sets of APIs is linked below. For each section, a description and example use of related APIs is provided. For complete documentation, provide the `--help` flag to any command or subcommand.

### Base options

#### Verbosity

Some APIs support multiple levels of workflow and debug logging. Enable them with `monorail -v <command>`, `monorail -vv <command>`, or `monorail -vvv <command>`.

#### Config file location

Some APIs require a [configuration file](#config). Manually specifying the file is done with `monorail -f </path/to/Monorail.json>`.

- [Config](#config)
- [Targets](#targets)
- [Commands](#commands)
- [Logs](#logs)
- [Results](#results)
- [Analyze](#analyze)
- [Checkpoint](#checkpoint)
- [Out](#out)

## Config

`monorail` uses a JSON configuration file that describes the repository target mapping, target dependencies, and various other options. This file is fully documented in the `Monorail.reference.js` file at the root of this repository. This file is by default `Monorail.json` in the root of your repository, and while this location and name can be customized, all subsequent documentation will refer to it by this name.

### APIs

#### `config show`

Parses the configuration file and displays it, including defaults for fields not specified in the file.

## Targets

A `target` is a core abstraction of `monorail`. High-level, it is simply a label for a filesystem path. You choose which paths in your repository to operate over, and assign each of those those paths a target in `Monorail.json`. When files are added, modified, or deleted within a target, the target will be considered "changed".

Targets must be:
  - a directory containing at least one file in its tree
  - unique

For example, this simple directory structure has one target and the file within it is part of that target:

```sh
repo/
  rust/
    Cargo.toml
  Monorail.json
```

`Monorail.json`
```json
{
  "targets": [{"path": "rust"}]
}
```

### Nesting targets

Targets may be nested within other targets, creating a hierarchy. For example, this assigns a target to a subdirectory of an existing target:

```sh
repo/
  rust/
    app1/
      Cargo.toml
    Cargo.toml
  Monorail.json
```

`Monorail.json`
```json
{
  "targets": [{"path": "rust"}, {"path": "rust/app1"}]
}
```

In this example, `rust/app1` is now its own target. It is also part of the `rust` target, so changes within this directory will affect the `rust` target as well.

### Target dependencies

Targets can declare dependencies on other paths within the repository. When a change is detected in a dependency path, all dependent targets are affected.

Valid dependencies are any of the following:
  - targets
  - paths within targets
  - paths outside of targets

Dependencies are declared per-target with the `uses` array. For example:

```sh
repo/
  one/
    foo.txt
  two/
    bar.txt
  not_a_target/
    zap.txt
  Monorail.json
```
`Monorail.json`
```json
{
  "targets": [
    {
      "path": "one",
      "uses": ["not_a_target"]
    }, 
    {
      "path": "two",
      "uses": ["one"]
    }
  ]
}
```

Changes to target `one` will affect target `two`, and changes to `not_a_target` will affect target `one`.

### Graph cycles

However, targets may not declare dependencies that would create a circular reference. For example:

```sh
repo/
  one/
    foo.txt
  two/
    bar.txt
  Monorail.json
```
`Monorail.json`
```json
{
  "targets": [
    {
      "path": "one",
      "uses": ["two"]
    }, 
    {
      "path": "two",
      "uses": ["one"]
    }
  ]
}
```

This configuration will produce a graph cycle error when the target graph is used.

### Visualization

The target graph can be visualized using common tools, such as graphviz. This visualization is generated with the `target render` API. For example:

```sh
monorail target render
```

This will generate a `target.dot` file in your current directory. You can choose the output file location by providing the `-f` flag to render. In general, when passing this file to `dot` the `sfdp` layout is likely to produce the best visual results, but you are free to use others. For example, using the output from `monorail target render`, with `dot` installed:

```sh
dot -Ksfdp -Tpng target.dot -o target.png
```

### APIs

#### `target render [--output-file, -f] [--type, -t]`

Generate a visual representation of the target graph, and write it to an output file. Use `monorail target render -h` for details.

#### `target show [--target-groups, -g] [--commands, -c]`

Display configured targets, target groupings, and target commands. Use `monorail target show -h` for details.

## Commands

A command is an executable program called in the `run` API. Commands can be written in any language, and need only exist within the repository and have executable permissions. Common commands are those that install toolchains, lint and compile code, execute test binaries, build containers, deploy to servers, and so forth.

Commands are invoked by one or more targets, and each command may optionally be parameterized with runtime arguments on a per-target basis using [Args](#args).

Command output to stderr and stdout is captured and streamed to a [log tailing socket](#tailing-logs) (if active), and streamed to [compressed zstd archives](#showing-logs).

### Defining commands

Defining a command can be done automatically by accepting `monorail` defaults, or manually by specifying a label and location in `Monorail.json`.

#### Automatic

By default, targets store their commands in a `monorail/cmd` directory relative to the target's path. The stem of the file becomes the command name. For example:

```sh
repo/
  one/
    monorail/
      cmd/
        build.sh
```

This defines the `build` command for the `one` target.

#### Manual

The default command directory, command name, and/or executable location can be customized on a per-target basis in `Monorail.json`. Refer to `Monorail.reference.js` for more details.

Manually specifying commands is primarily useful when a command can be parameterized and reused by multiple targets, allowing the executable to exist in one place and be referenced by targets.

### Running commands

Commands are executed by providing a list of command names to `run`, either implictly in parallel by a graph-guided analysis of changes, or explicitly in series for a provided set of targets. If an invoked command is not defined for a target, this is noted in the results of the run but is by default not considered an error. This behavior can be overriden by specifying the `--fail-on-undefined` flag on `run`.

When executing multiple commands, each command must fully succeed for all involved targets before the next is invoked. Otherwise, the `run` will end with an error.

On completion, `run` returns a structure containing the results of the run. Its return code is non-zero if any errors were encountered during execution, or any commands returned non-zero exit codes.

#### Implicitly

Commands may be run for changed targets by using a dependency graph derived from `uses` declarations in `Monorail.json`. When commands are run this way, each command is run sequentially, in the order specified to `run`. For each command, a graph traversal produces groupings of unrelated targets (called "target groups"), and within each target group, the command is executed in parallel. For example:

```sh
monorail run -c lint build
```

This will first run the `lint` command for each target group in parallel, and then do the same for `build`.

#### Explicitly

Commands may be run for specific targets by providing a list of targets to the `run` command. By default, commands are executed sequentially for each provided target. For example:

```sh
monorail run -c lint build -t target1 target2
```

This will run the `lint` command for `target1`, then `target2`, and then `build` for `target1`, then `target2`, if they have this command defined.

However, providing the `--deps` flag will include dependencies for the provided targets, and enable target group parallelism. For example:

`Monorail.json`
```json
{
  "targets": [
    {
      "path": "target1",
      "uses": [
        "dep1"
      ]
    },
    {
      "path": "target2"
    },
    {
      "path": "dep1"
    }
  ]
}
```

```sh
monorail run -c build -t target1 target2 --deps
```

This will run `build` first for `dep1`, and then `target1` and `target2` in parallel.

#### Arguments

Commands can be provided with positional arguments at runtime. There are two mechanism for doing this; the base argmap, and as flags provided to `monorail run`. In your command executables, capture these positional arguments as you would in any program in that language. As is common, index 0 of the positional arguments is always an absolute path to the executable, so any additional arguments begin at index 1.

##### Base argmap
The base argmap is an optional file containing argument mappings, which, if provided, is automatically loaded before any other argument mappings provided to `monorail run`. This is useful for specifying static parameterization for commands, especially when a command executable is generic and reused among multiple targets. An example of this useful pattern is shown in https://github.com/pnordahl/monorail-example in the rust crate targets.

For example, here `target1` has a base argmap in the default location:

`target1/monorail/argmap/base.json`
```json
{
  "build": [
    "--all"
  ]
}
```

You can change both the argmap search path and the base argmap file by specifying them in `Monorail.json`. Refer to the `argmaps` section of `Monorail.reference.js` for more details.

Finally, while base argmaps are often required by commands that are built to expect them, you can disable base argmaps during a `monorail run` with the `--no-base-argmaps` switch.

##### Runtime flags

In addition to the base argmap, `monorail run` accepts `--arg (-a)`, `--target-argmap (-m)`, and `--target-argmap-file (-f)` flags for providing additional argument mappings. 

```sh
monorail run -c build -t target1 --arg '--release' --arg '-v'
```

This will first provide `-all` from the base argmap as the first positional argument, and then `--release` and `-v` as the second and third positional arguments to the `build` command for `target1`.

Note that when using `--arg`, you must specify exactly one command and target. For more flexibility, use `--target-argmap` and/or `--target-argmap-file`, which allow for specifying argument arrays for specific command-target combinations. For example:

```sh
monorail run -c build test --target-argmap '{"target1":{"build":["--release"],"test":["--release"]}}'
```

This will provide the specified arguments to the appropriate command-target combinations. Multiple `--target-argmap` flags may be provided, and if so keys that appear in both have their arrays appended to each other in the order specified. For example:

```sh
monorail run -c build test --target-argmap '{"target1":{"build":["--release"],"test":["--release"]}}' --target-argmap '{"target1":{"build":["-v"]}}'
```

In this case, `build` appears twice for `target1` so the arrays are combined in order, and the final arguments array for `target1` is `[--release, -v]`.

Additionally, you can provide the `--target-argmap-file` flag zero or more times with a filesystem path to a JSON file containing an argument mapping. The structure of this file is identical to the one described above for `--target-argmap`. As with that flag, files provided are merged from left to right. For example

```sh
monorail run -c build -f target1/monorail/argmap/foo.json -f target1/monorail/argmap/bar.json
```

Would merge the keys and values from each file in turn in order.

#### Sequences

A sequence is an array of commands, and is specified in `Monorail.json`. It's useful for bundling together related commands into a convenient alias, and when used with `run` simply expands into the list of commands it references. For example:

```json
{
  "targets": [],
  "sequences": {
    "validate": ["check", "build", "unit-test", "integration-test"]
  }
}
```

```sh
monorail run -s validate
```

This is analagous to `monorail run -c check build unit-test integration-test`.

### APIs

#### `run [--target, -t <1> <2> ... <M>] <--commands, -c <1> <2> ... <N> | --sequences, -s <1> <2> ... <P>>`

Runs commands or sequences for targets. At least one command or sequence is required. For more details, see `monorail run -h`.

## Logs

Commands can emit output to stdout and stderr, and this output is captured by `monorail`. This output can be routed to an active log tailing socket, and is also stored on disk in compressed archives on a per command + target basis.

### Showing logs

Logs generated by the `run` API are kept alongside the results of that run in the monorail output directory. These logs can be retrieved with the `log show` API. For example, this will retrieve all stdout an stderr logs from the most recent run:

```sh
monorail log show --stdout --stderr
```

This command will decompress each file and print it to stdout. Each file is preceded with a header indicating which command, target, and type (stdout, stderr) the output is for.

In the future, the ability to query specific historical logs beyond the most recent will be provided.

### Tailing logs

In addition to historical logs, `run` can optionally stream command output to a socket. As commands are often parallelized, this socket is interleaved with blocks of log output from each command. Each block is preceded by a header containing the command, target, and type. For example:

```sh
monorail log tail --stdout --stderr
```

When this is running in a separate shell process, `run` commands will connect to it and send logs. When those logs are received, they are printed by the tail process.

### APIs

#### `log show [--target, -t <1> <2> ... <M>] [--commands, -c <1> <2> ... <N>] <--stdout | --stderr>`

Retrieves compressed historical logs for the provided stream types, targets and/or commands, and prints them. For more details, see `monorail log show -h`.

#### `log tail [--target, -t <1> <2> ... <M>] [--commands, -c <1> <2> ... <N>] <--stdout | --stderr>`

Starts a socket server with filters for the provided stream types, targets and/or commands, and prints them as received. This must be run as a separate process. For more details, see `monorail log tail -h`.

## Results

In addition to logs, the result of each `run` API call is stored in the monorail out directory.

### APIs

#### `result show`

Retrieves run results and displays them.

In the future, the ability to query specific historical results beyond the most recent will be provided.

## Checkpoint

The checkpoint is what enables the change detection and incremental execution features of `monorail`. It is used as a starting point for change detection, allowing for a subset of changes in a repo to be queried. When the checkpoint is not present all targets are considered to be changed.

The checkpoint is essentially a marker in history, and may also include files not present in history. Effective use of `monorail` involves regular manipulation of this checkpoint. For example, as files within targets are modified, each target accumulates in the list of changed targets. Over time, this list can become unwieldy or wasteful to execute commands for repeatedly. When this occurs, the solution is to run the set of commands that would "validate" the changed targets, e.g. "if all of these commands pass, I'm done with that target", and then update the checkpoint. For example, this is a useful pattern:

```sh
monorail run -s validate && monorail checkpoint update -p
```

This runs the `validate` sequence, which you might define with a full suite of checks like `check`, `build`, `unit-test`, and `integration-test`, for all changed targets. If that fully succeeds for all targets involved, we then update the checkpoint to the latest point in history and include any pending edits on the local filesystem. Now, it will appear as if no targets are changed. Then, you can repeat this process as you make more changes. This ensures that your `run` invocations are only operating on the latest changes, and not on older ones that you've already checked.

### Updating the checkpoint

The checkpoint can be updated to any value supported by the change provider backend. 

#### Git backend

For example, `git` checkpoint ids are SHAs pointing to git objects, such as a commit or tag. When you update the checkpoint, you can optionally provide one as the id or allow `monorail` to select the SHA for the current HEAD. For example:

```sh
monorail checkpoint update
```

This will update the checkpoint to the SHA pointed to by the current HEAD reference.

```sh
monorail checkpoint update -i $(git rev-parse HEAD~1)
```

This will update the checkpoint to the SHA of the commit previous to the current HEAD reference.

#### Pending

As you work in a repository, many changes are not yet part of version control history, and these files and their checksums can be captured and stored in the checkpoint at any time. For example:

```sh
monorail checkpoint update --pending
```

For the `git` change provider, this will update the checkpoint with the current HEAD reference and include all uncommitted, unstaged, and staged changes in the checkpoint. 

### APIs

#### `checkpoint show`

Displays the current checkpoint.

#### `checkpoint update [--id, i <id>] [--pending, -p]`

Updates the checkpoint id and pending array. For more details, see `monorail checkpoint update -h`.

#### `checkpoint delete`

Deletes the checkpoint, reverting the repository to its default change detection state (all targets changed).

## Analyze

Many of the features of `monorail` involve an analysis of changes, targets, and the target graph. For example, this analysis is internally used by `run` to perform graph-guided command execution, and it's often useful to first see this analysis before running commands.

### All targets

By default, `analyze` will act over all targets in the graph. For example:

```sh
monorail analyze --target-groups
```

This will display the currently affected targets and how they fall into target groupings. You can also specify `--changes` and `--change-targets` for a more detailed breakdown how each change was seen by `monorail`, but this is typically not useful beyond debugging of `monorail` itself.

### APIs

#### `analyze [[--changes] [--change-targets] [--target-groups] | --all] [--targets, -t <1> <2> ... <N>`

Display an analysis containing changes, change targets, and/or target groups. Can be optionally scoped to subtrees of targets intead of the entire graph.

## Out

Monorail stores various artifacts and tracking information in a directory, by default `monorail-out`. This location can be customized in `Monorail.json` by setting the `out_dir` field.

### Deleting the output directory
While the out directory contains caches, run results, and the checkpoint, it can be deleted freely. For example:

```sh
monorail out delete --all
```

This will delete the checkpoint, all results, and all logs.

In the future, more selective deletion will be supported.

### APIs

# Development setup

`monorail` is written in Rust, so working on it is straightforward; this will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```
