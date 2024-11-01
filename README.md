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

`monorail` blends features of build systems like `bazel` and `buck` with the simplicity and flexibility of command runners like `make` and `just`. It manages dependencies and target mapping with `bazel`-like change tracking, enabling selective rebuilds and efficient parallel execution. At the same time, `monorail` simplifies scripting, providing a streamlined, CLI-driven workflow for running custom tasks in any language across a monorepo.

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

The tutorial in the next section will elaborate on these concepts in a practical fashion.

# Tutorial

To illustrate multi-language command executables, ensure that `awk` and `python3` are installed. Additionally, you may want `jq` installed to pretty-print the output from various steps in this tutorial. If you don't want to install it, you can just omit any `| jq` that you find.

In this tutorial, you'll learn about:

  * mapping repository paths
  * analyzing changes
  * defining and executing commands
  * logging
  * checkpointing

## One-time setup

First, create a fresh `git` repository in your current directory:

```sh
git init monorail-tutorial
cd monorail-tutorial
git commit -m x --allow-empty
echo 'monorail-out' > .gitignore
```

_NOTE_: the commit is to create a valid HEAD reference, which is needed in a later part of the tutorial.

First, we will set up some toy projects to help get a feel for using `monorail`. Since `rust` is already installed, we'll use that in addition to `python` (which you likely also have installed; if not, go ahead and do so). The third project will not include any real code, because it's just going to illustrate how shared dependencies are considered during execution of commands. Finally, each will get an empty `monorail.sh` file that we will add code to as the tutorial proceeds.

### Rust
These commands will make a rust workspace with two member projects with tests:

```sh
mkdir -p rust/monorail
pushd rust
cargo init --lib app1
cargo init --lib app2
cat <<EOF > Cargo.toml
[workspace]
resolver = "2"
members = [
  "app1",
  "app2"
]
EOF
popd
```

### Python
These commands will make a simple python project with a virtualenv and a test:
```sh
mkdir -p python/app3/monorail
pushd python/app3

python3 -m venv "venv"
source venv/bin/activate 
cat <<EOF > hello.py
def get_message():
    return "Hello, World!"

def main():
    print(get_message())

if __name__ == "__main__":
    main()
EOF
mkdir tests
cat <<EOF > tests/test_hello.py
import unittest
from hello import get_message

class TestHello(unittest.TestCase):
    def test_get_message(self):
        self.assertEqual(get_message(), "Hello, World!")

if __name__ == "__main__":
    unittest.main()
EOF
deactivate
popd
```
### Protobuf
```sh
mkdir -p proto/monorail
pushd proto
touch README.md
popd
```

## Mapping targets

Now that our repository structure is in place, we will create a `Monorail.json` file that describes this structure in terms `monorail` understands. This command will create that file in the root of the repository, and simply map top level paths to targets:

```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust" },
    { "path": "python/app3" },
    { "path": "proto" }
  ]
}
EOF
```

This is how you generally specify targets; a unique filesystem path relative to the root of your repository.

Run the following, to show our config and ensure our input is well-formed:

```sh
monorail config show | jq
```
```json
{
  "timestamp": "2024-10-27T11:12:56.287190+00:00",
  "output_dir": "monorail-out",
  "max_retained_runs": 10,
  "change_provider": {
    "use": "git"
  },
  "targets": [
    {
      "path": "rust",
      "commands": {
        "path": "monorail"
      }
    },
    {
      "path": "python/app3",
      "commands": {
        "path": "monorail"
      }
    },
    {
      "path": "proto",
      "commands": {
        "path": "monorail"
      }
    }
  ]
}
```

This output includes some default values for things not specified, but otherwise reflects what we have entered. An additional note about the location of the `Monorail.json` file; you can specify an absolute path with `-c`, e.g. `monorail -c </path/to/your/config/file>`, and this will be used instead of the default (`$(pwd)/Monorail.json`). All of `monorail`s commands are executed, and internal tracking files and logs stored, _relative to this path_.

## Preview: running commands

Commands will be covered in more depth later in the tutorial (along with logging), but now that we have a valid `Monorail.json` we can execute a command and view logs right away. Run the following to create an executable (in this case, a `bash` script) for the `rust` target:

```sh
cat <<EOF > rust/monorail/hello.sh
#!/bin/bash

echo 'Hello, world!'
echo 'An error message' >&2 
EOF
chmod +x rust/monorail/hello.sh
```

Now execute it:

```sh
monorail run -c hello -t rust | jq
```

```json
{
  "timestamp": "2024-10-27T11:13:32.378921+00:00",
  "failed": false,
  "invocation_args": "run -c hello -t rust",
  "results": [
    {
      "command": "hello",
      "successes": [
        {
          "target": "rust",
          "code": 0,
          "stdout_path": "/tmp/monorail-tutorial/monorail-out/log/2/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst",
          "stderr_path": "/tmp/monorail-tutorial/monorail-out/log/2/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst",
          "runtime_secs": 0.001518792
        }
      ],
      "failures": [],
      "unknowns": []
    }
  ]
}
```

This output is stored and queryable with `monorail result show`. In addition, you can view logs, both historically and by tailing (shown in more detail later). Show the logs for the most recent `run`:

```sh
monorail log show --stdout --stderr
```
```
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
```

In this example, we just used `monorail` as a simple command runner like `make` or `just`. However, unlike these other tools `monorail` is capable of running commands in parallel based on your dependency graph, streaming logs, collecting historical results and compressed logs, and more.

Before we explore commands and logging in more detail, it's important to understand how `monorail` detects changes. In the next section we'll use the `analyze` API to do so.

## Analyze

`monorail` integrates with a `change_provider` to obtain a view of filesystem changes, which are processed along with a graph built from the specification in `Monorail.json`. Display an analysis of this changeset and graph with:

```sh
monorail analyze | jq
```
```json
{
  "timestamp": "2024-10-27T11:13:55.324761+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

This indicates that based on our current changeset and graph, all three targets have changed. Display more information about the specific changes causing these targets to appear by adding `--change-targets`:
```sh
monorail analyze --changes | jq
```
```json
{
  "timestamp": "2024-10-27T11:14:05.187894+00:00",
  "changes": ["... hundreds of entries ..."],
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

Unfortunately, hundreds of undesired virtualenv files are in our changes array. In the next section, we'll rectify this by using our change providers native mechanisms.

### Ignoring with .gitignore

As mentioned, `monorail` defers change detection to a provider. Since our change provider is `git`, we can exclude these files by adding them to `.gitignore`:

```sh
echo 'python/app3/venv' >> .gitignore
```

Re-running the commnand, notice that all of the offending files are gone:
```sh
monorail analyze --changes | jq
```
```json
{
  "timestamp": "2024-10-27T11:14:35.901180+00:00",
  "changes": [
    {
      "path": ".gitignore"
    },
    {
      "path": "Monorail.json"
    },
    {
      "path": "proto/README.md"
    },
    {
      "path": "python/app3/hello.py"
    },
    {
      "path": "python/app3/tests/test_hello.py"
    },
    {
      "path": "rust/Cargo.toml"
    },
    {
      "path": "rust/app1/Cargo.toml"
    },
    {
      "path": "rust/app1/src/lib.rs"
    },
    {
      "path": "rust/app2/Cargo.toml"
    },
    {
      "path": "rust/app2/src/lib.rs"
    },
    {
      "path": "rust/monorail/hello.sh",
    }
  ],
  "targets": [
    "proto",
    "python/app3",
    "rust"

```

### Ignoring with 'ignores'

For directories and files we need checked in, but do not want to consider as a reason for a target to be considered changed, there is the 'ignores' array. This is useful for things like a README.md, docs, etc. Run the following to ignore the README file in the proto target:


```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust" },
    { "path": "python/app3" },
    { "path": "proto", "ignores": [ "proto/README.md" ] }
  ]
}
EOF
```

Run the analyze again, but as a preview for the next section add the `--target-groups` flag. Note that `proto` no longer appears in the list of changed targets, and a new array of arrays has appeared:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-10-27T11:15:12.382101+00:00",
  "targets": [
    "python/app3",
    "rust"
  ],
  "target_groups": [
    [
      "rust",
      "python/app3"
    ]
  ]
}
```

The `target_groups` array is built by constructing a graph from the dependencies specified in `Monorail.json`. This is used to control command execution, ensuring that maximum parallelism is achieved while respecting the hierarchy of dependencies at any given time. In the next section, we will establish a simple dependency graph between our targets.

## Dependencies

Specifying dependencies between targets is done with the `uses` field on a `target` in `Monorail.json`. Run the following to set up a simple graph (and create a file so our `proto` project is visible again, since we ignored `README.md`):

```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust", "uses": ["proto"] },
    { "path": "python/app3", "uses": ["proto"] },
    { "path": "proto", "ignores": [ "proto/README.md" ] }
  ]
}
EOF
touch proto/LICENSE.md
```

This has created a dependency on `proto` for `rust` and `python/app3` targets, connecting them for change detection and command execution. Observe the results of analyze now:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-10-27T11:15:33.233479+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ],
  "target_groups": [
    [
      "proto"
    ],
    [
      "rust",
      "python/app3"
    ]
  ]
}
```

Semantically, each element of `target_groups` is an array of targets that can be considered "independent" of subsequent elements. Since `proto` does not depend on any other targets, it stands alone and prior to `rust` and `python/app3`, which depend on `proto`. As it pertains to command execution, each target found within a group is executed in parallel, and when successful `monorail` will move on to the next array and do the same. For change detection, changes to dependencies of a target will cause that target to be considered changed as well; i.e. a "what depends on me" graph traversal. In the final sections, we will cover logging and parallel command execution with practical examples.

Finally, keep in mind that you can specify non-target paths in `uses` and those paths will now be considered part of that target. In general, this is not commonly used but remains a convenient way to link targets to paths they would otherwise not be.

## Logging and Results

Command output capture and result records are core responsibilities of a build tool like `monorail`, and so it provides a set of APIs for obtaining all data generated by `monorail run`. During command execution, all stdout and stderr is captured and stored in compressed archives, along with the overall results of the run. The number of historical runs retained is controlled by `max_retained_runs` in `Monorail.json`, defaulting to 10.

### Historical

We have already run a command, so let's show the result of that most recent run:

```sh
monorail result show | jq
```
```json
{
  "timestamp": "2024-10-27T11:15:45.900643+00:00",
  "failed": false,
  "invocation_args": "run -c hello -t rust",
  "results": [
    {
      "command": "hello",
      "successes": [
        {
          "target": "rust",
          "code": 0,
          "stdout_path": "/tmp/monorail-tutorial/monorail-out/log/2/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst",
          "stderr_path": "/tmp/monorail-tutorial/monorail-out/log/2/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst",
          "runtime_secs": 0.001518792
        }
      ],
      "failures": [],
      "unknowns": []
    }
  ]
}
```

Interpreting this result is straightforward. The run succeeded because `failed` is `false`, and various data about which commands and targets ran is provided. 

The log files are available on disk to query, so if you prefer to use external tools you can do so (just note that you'll need `zstd` installed to decompress the archives):

```sh
monorail result show | jq -r '.results[] | .failures + .successes + .unknowns | .[]  | .stderr_path,.stdout_path' | xargs -I {} zstd -dc {}
```
```
An error message
Hello, world!
```

However, `monorail` provides a convenient API (which we already saw above) for doing so in a way that makes viewing multiple logs easier to read:

```sh
monorail log show --stderr --stdout
```
```
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
```

When queried through `monorail`, blocks of related logs begin with a header of format `[monorail | {{file name}} | {{target}} | {{command}}]`. Historical logs are decompressed and emitted serially to stdout, so only one of these will be present in each file. However, when tailing logs for multiple commands and targets these headers are regularly injected as blocks of logs from each task are emitted. In the next section, we will update our existing command to demonstrate log tailing.

### Tailing

It's often useful to observe how a `monorail run` invocation is progressing; to do so, we will start a log streaming tail process in a separate shell (you may also want to do this in a separate terminal window, so you can see the logs as you run commands):

```sh
monorail log tail --stderr --stdout
```

This has started a server that will receive logs. For the rest of this tutorial, leave that running in a separate window. Now, let's update our existing command to demonstrate tailing:

```sh
cat <<EOF > rust/monorail/hello.sh
#!/bin/bash

for ((i=0; i<20; i++)); do
    echo 'Hello, world!'
    sleep 0.02
done &

for ((i=0; i<10; i++)); do
    echo 'An error message' >&2
    sleep 0.04
done

wait
EOF
```

This will run for a moment, printing output from stdout and stderr for us to observe. Now, run the command again with and additional flag `-v` to print useful workflow statements (`-vv` and `-vvv` print even more), and ensure that your log tailing window is in view:

```sh
monorail -v run -c hello -t rust
```
As this command executes, you'll see internal progress due to the inclusion of the `-v` flag:

```
{"timestamp":"2024-10-27T11:16:33.827760+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-10-27T11:16:33.829118+00:00","level":"INFO","message":"processing groups","num":1}
{"timestamp":"2024-10-27T11:16:33.829131+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-10-27T11:16:33.829143+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:16:34.393215+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:16:34.394552+00:00","failed":false,"invocation_args":"-v run -c hello -t rust","results":[{"command":"hello","successes":[{"target":"rust","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/3/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/3/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst","runtime_secs":0.5639998}],"failures":[],"unknowns":[]}]}
```

... as well as log statements from the command itself in your log tailing window:

```
[monorail | stdout.zst, stderr.zst | (any target) | (any command)]
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
[monorail | stderr.zst | rust | hello]
An error message
An error message
An error message
An error message
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
[monorail | stderr.zst | rust | hello]
An error message
An error message
An error message
An error message
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
```

A few things are worth noting in this output.

First, is the log stream header:
```
[monorail | stdout.zst, stderr.zst | (any target) | (any command)]
```

Every `monorail run` will print this header once at the beginning of a new log stream, indicating what data will be shown in the stream. When you start the tail process, by default it will not filter any targets or commands; this is indicated by the `(any target)` and `(any command)` entries in the header. If we had provided `--targets rust proto` and `--commands hello test build` (which can also be provided to `monorail log show`), then the header would look like `[monorail | stdout.zst, stderr.zst | rust, proto | hello, test, build]`, and only those logs that match these filters would appear.

Second, note how multiple log block headers (e.g. `[monorail | stderr.zst | rust | hello]`) are printed; this is because the output from multiple files is independently collected and flushed at regular intervals and interleaved in the stream. In practice, you will often use target and command filters to reduce noise, but even without them block headers make it possible to visually parse the combined log stream.

## Commands

Commands are the way `monorail` executes your code against targets. Each target implements a command with a unique name, e.g. `test`, `build`, etc. as an executable file, and that file is executed and monitored as a subprocess of `monorail`. Depending on the graph defined by `Monorail.json`, these commands may be executed in parallel when it's safe to do so. For convenience, all commands are executed relative to the target path, e.g. for a target with path `rust`, the following code runs within the `rust` directory:

```sh
#!/bin/bash

cargo test -- --nocapture
```

### Defining a command

By default, `monorail` will use the `commands_path` (default: a `monorail` directory in the target path) field of a target as a search path for commands, and by default look for a file with a stem of `{{command}}`, e.g. `{{command}}.sh`. The command we defined earlier in the tutorial, `rust/monorail/hello.sh`, used these defaults; While customizing these defaults is possible via `Monorail.json`, it's not necessary for this tutorial. Let's define two new executables, this time in Python and Awk:

```sh
cat <<EOF > python/app3/monorail/hello.py
#!venv/bin/python3
import sys

print("Hello, from python/app3 and virtualenv python!")
print("An error occurred", file=sys.stderr)
EOF
chmod +x python/app3/monorail/hello.py
```

NOTE: While we're able to use the venv python3 executable directly in this trivial way, we wouldn't be able to access things that are installed in the environment without first activating the venv in a shell. So, in practice Python projects generally require a command written in a shell script that activates the env and then calls the script.

```sh
cat <<EOF > proto/monorail/hello.awk
#!/usr/bin/awk -f

BEGIN {
    print "Hello, from proto and awk!"
}
EOF
chmod +x proto/monorail/hello.awk
```

As mentioned earlier, commands can be written in any language, and need only be executable. We're using hashbangs to avoid cluttering the tutorial with compilation steps, but commands could be compiled to machine code, stored as something like `hello`, and executed just the same (though this approach wouldn't be portable across OS/architectures). Before we run this command, let's look at the output of analyze:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-10-27T11:17:24.753641+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ],
  "target_groups": [
    [
      "proto"
    ],
    [
      "rust",
      "python/app3"
    ]
  ]
}
```

In the next section, we will see how the target graph guides parallel command execution.

### Running commands

We have already seen an example of explictly choosing target(s) to run commands for with `monorail run -c hello -t rust`, but `monorail` is also capable of executing commands based on a dependency graph-guided analysis of changes, automatically. To do this, simply leave off a list of targets for the `run`:

```sh
monorail -v run -c hello
```
```json
{"timestamp":"2024-10-27T11:17:44.298254+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-10-27T11:17:44.299285+00:00","level":"INFO","message":"processing groups","num":2}
{"timestamp":"2024-10-27T11:17:44.299295+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-10-27T11:17:44.299304+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-10-27T11:17:44.447297+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-10-27T11:17:44.447637+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-10-27T11:17:44.447686+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:17:44.448154+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-10-27T11:17:44.577691+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-10-27T11:17:44.997866+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:17:44.999339+00:00","failed":false,"invocation_args":"-v run -c hello","results":[{"command":"hello","successes":[{"target":"proto","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stderr.zst","runtime_secs":0.1479375}],"failures":[],"unknowns":[]},{"command":"hello","successes":[{"target":"python/app3","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stderr.zst","runtime_secs":0.12952638},{"target":"rust","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/4/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst","runtime_secs":0.5501715}],"failures":[],"unknowns":[]}]}
```

In this sequence of events: the `hello` command is scheduled and executed to completion for `proto` prior to the same for `python/app3` and `rust`. This is because both of the latter depend on `proto`. A practical scenario where this is relevant is building protobuf files for use by both `python/app3` and `rust`. By encoding this dependency in `Monorail.json`, we have ensured that when protobuf files in `proto` change, we have definitely compiled them by the time we execute commands for `python/app3` and `rust`.

You can also execute multiple commands. Each command in `-t <command1> <command2> ... <commandN>` is executed in the order listed, serially; the parallelism of `run` occurs within a command, at the target group level. Add a non-existent command to the list and run again:

```sh
monorail -v run -c hello build
```
```json
{"timestamp":"2024-10-27T11:18:04.689+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-10-27T11:18:04.690384+00:00","level":"INFO","message":"processing groups","num":4}
{"timestamp":"2024-10-27T11:18:04.690400+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-10-27T11:18:04.690408+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-10-27T11:18:04.692275+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-10-27T11:18:04.692456+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-10-27T11:18:04.692474+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:18:04.692706+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-10-27T11:18:04.715289+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-10-27T11:18:05.247114+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-10-27T11:18:05.248133+00:00","level":"INFO","message":"processing targets","num":1,"command":"build"}
{"timestamp":"2024-10-27T11:18:05.248190+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"proto"}
{"timestamp":"2024-10-27T11:18:05.248986+00:00","level":"INFO","message":"processing targets","num":2,"command":"build"}
{"timestamp":"2024-10-27T11:18:05.249005+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"rust"}
{"timestamp":"2024-10-27T11:18:05.249013+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"python/app3"}
{"timestamp":"2024-10-27T11:18:05.250504+00:00","failed":false,"invocation_args":"-v run -c hello build","results":[{"command":"hello","successes":[{"target":"proto","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stderr.zst","runtime_secs":0.001844916}],"failures":[],"unknowns":[]},{"command":"hello","successes":[{"target":"python/app3","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stderr.zst","runtime_secs":0.022576958},{"target":"rust","code":0,"stdout_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst","stderr_path":"/tmp/monorail-tutorial/monorail-out/log/5/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst","runtime_secs":0.5545724}],"failures":[],"unknowns":[]},{"command":"build","successes":[],"failures":[],"unknowns":[{"target":"proto","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0}]},{"command":"build","successes":[],"failures":[],"unknowns":[{"target":"rust","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0},{"target":"python/app3","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0}]}]}
```

You might notice the exit code of 0 and `"failed":false`, and that's because by default it is not required for a target to define a command. You can override this behavior with `--fail-on-undefined`, but in general this allows targets to define only the commands they need and eliminates the need for "stubs" that may never be implemented. One exception to this is when providing a list of targets to `run`, where `--fail-on-undefined` defaults to true. The reason for this is that when executing a command directly for targets, one expects that command to exist.

### Running multiple commands with sequences

While any number of commands may be specified with `-c`, you can define `sequences` of commands in your configuration file for convenience and consistency. Edit the configuration file to create a `"dev"` sequence:
```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust", "uses": ["proto"] },
    { "path": "python/app3", "uses": ["proto"] },
    { "path": "proto", "ignores": [ "proto/README.md" ] }
  ],
  "sequences": { "dev": ["hello", "build"] }
}
EOF
```

Now run the sequence with `--sequences` or `-s`:
```sh
monorail -v run -s dev
```
```json
{"timestamp":"2024-11-01T11:13:03.550061+00:00","level":"INFO","message":"processing groups","num":4}
{"timestamp":"2024-11-01T11:13:03.550115+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-11-01T11:13:03.550155+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-11-01T11:13:03.552012+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-11-01T11:13:03.554001+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-11-01T11:13:03.554036+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-11-01T11:13:03.554253+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-01T11:13:03.575991+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-01T11:13:04.125682+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-11-01T11:13:04.126924+00:00","level":"INFO","message":"processing targets","num":1,"command":"build"}
{"timestamp":"2024-11-01T11:13:04.127075+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"proto"}
{"timestamp":"2024-11-01T11:13:04.127920+00:00","level":"INFO","message":"processing targets","num":2,"command":"build"}
{"timestamp":"2024-11-01T11:13:04.127978+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"rust"}
{"timestamp":"2024-11-01T11:13:04.128022+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"python/app3"}
{"timestamp":"2024-11-01T11:13:04.129937+00:00","failed":false,"invocation_args":"-v run -s dev","results":[{"command":"hello","successes":[{"target":"proto","code":0,"stdout_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stdout.zst","stderr_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6/stderr.zst","runtime_secs":0.001837875}],"failures":[],"unknowns":[]},{"command":"hello","successes":[{"target":"python/app3","code":0,"stdout_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stdout.zst","stderr_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7/stderr.zst","runtime_secs":0.021706708},{"target":"rust","code":0,"stdout_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst","stderr_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/run/5/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst","runtime_secs":0.57150763}],"failures":[],"unknowns":[]},{"command":"build","successes":[],"failures":[],"unknowns":[{"target":"proto","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0}]},{"command":"build","successes":[],"failures":[],"unknowns":[{"target":"rust","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0},{"target":"python/app3","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0}]}]}
```

When you run a sequence, it is first expanded into the commands it maps to. Then, any commands provided with `--commands` or `-c` are added to this list. For example, `monorail run -s dev -c foo bar` is equivalent to `monorail run -c hello build foo bar`. Sequences are useful for defining commands for use in specific contexts, such as setting up a repository for a new user, general development, and CI/CD.

### Viewing available commands

Lastly, you can query targets and include information about the commands that are available for each target. The `commands` table is built by merging executables found on the filesystem with any definitions in your configuration file:

```sh
monorail target show --commands
```
```json
{
  "timestamp": "2024-11-01T01:27:28.409154+00:00",
  "targets": [
    {
      "path": "rust",
      "uses": [
        "proto"
      ],
      "commands": {
        "hello": {
          "path": "monorail/hello.sh",
          "args": [],
          "is_executable": true
        }
      }
    },
    {
      "path": "python/app3",
      "uses": [
        "proto"
      ],
      "commands": {
        "hello": {
          "path": "monorail/hello.py",
          "args": [],
          "is_executable": true
        }
      }
    },
    {
      "path": "proto",
      "ignores": [
        "proto/README.md"
      ],
      "commands": {
        "hello": {
          "path": "monorail/hello.awk",
          "args": [],
          "is_executable": true
        }
      }
    }
  ],
}
```

In the final section of this tutorial, we will manipulate the changes being used for `analyze` and guided `run` with the `checkpoint`.

## Checkpoint

The `checkpoint` is a marker in change provider history. When present, it is used as the beginning of an interval in history used for collecting a set of changes. The end of this interval is the latest point in change provider history. In addition, any pending changes not present in history are merged with the history set.

For `git`, the checkpoint is stored as a reference such as HEAD, or an object SHA. Pending changes are untracked and uncommitted files. Therefore, the set of changes reported by the `git` change provider is: `[<checkpoint>, ..., HEAD] + [untracked] + [uncommitted]`.

Now, for a practical example. First, query the checkpoint:

```sh
monorail checkpoint show
```
```json
{"timestamp":"2024-10-27T11:18:28.576758+00:00","kind":"error","type":"tracking_checkpoint_not_found","message":"Tracking checkpoint open error; No such file or directory (os error 2)"}
```

This error (which was printed to stderr) is fine, because by default no checkpoint exists. This means that all of the various files we've added are seen by `monorail` as changes. Now, update the checkpoint to include the current "latest point" in history; since we have only our initial HEAD commit, that's what we see:

```sh
monorail checkpoint update | jq
```
```json
{
  "timestamp": "2024-10-27T11:19:34.929320+00:00",
  "checkpoint": {
    "id": "40f5a4e3e01bdd2bc8f1053d9158e25cf274ca99",
    "pending": null
  }
}
```

This is however, not so useful; none of the files we've added have been committed so this doesn't affect analysis results like we'd want:

```sh
monorail analyze | jq
```
```json
{
  "timestamp": "2024-10-27T11:19:46.533262+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

Before we resolve this, let's delete the checkpoint:

```sh
monorail checkpoint delete | jq
```
```json
{
  "timestamp": "2024-10-27T13:32:54.968691+00:00",
  "checkpoint": {
    "id": "",
    "pending": null
  }
}
```

By deleting the checkpoint, we have told `monorail` to treat every target in our repository as changed bypassing change detection entirely.

In this tutorial, where all of our changes are "pending" (i.e. we haven't committed anything thus far), the key is to add `--pending` or `-p` when updating the checkpoint. This will include those files and their SHA2 checksums in the checkpoint.


```sh
monorail checkpoint update --pending | jq
```
```json
{
  "timestamp": "2024-10-27T11:19:58.311279+00:00",
  "checkpoint": {
    "id": "40f5a4e3e01bdd2bc8f1053d9158e25cf274ca99",
    "pending": {
      "proto/LICENSE.md": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "rust/app1/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9",
      "python/app3/tests/test_hello.py": "72b3668ed95f4f246150f5f618e71f6cdbd397af785cd6f1137ee87524566948",
      "rust/app2/Cargo.toml": "111f4cf0fd1b6ce6f690a5f938599be41963905db7d1169ec02684b00494e383",
      "rust/app2/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9",
      "rust/monorail/hello.sh": "c1b9355995507cd3e90727bc79a0d6716b3f921a29b003f9d7834882218e2020",
      "Monorail.json": "cdea86b01e3e6f719abecde8e127ba3d450dcaccfd2f9e5f9ffb27dd0ad2dadb",
      ".gitignore": "c1cf4f9ff4b1419420b8508426051e8925e2888b94b0d830e27b9071989e8e7d",
      "proto/README.md": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "proto/monorail/hello.awk": "5af404fedc153710aec00c8bf788d8f71b00c733c506d4c28fda1b7d618e4af6",
      "python/app3/monorail/hello.py": "fad357d1f5adadb0e270dfcf1029c6ed76e2565e62c811613eb315de10143ceb",
      "rust/Cargo.toml": "a35f77bcdb163b0880db4c5efeb666f96496bcb409b4cd52ba6df517fb4d625b",
      "python/app3/hello.py": "3639634f2916441a55e4b9be3497673f110014d0ce3b241c93a9794ffcf2c910",
      "rust/app1/Cargo.toml": "044de847669ad2d9681ba25c4c71e584b5f12d836b9a49e71b4c8d68119e5592"
    }
  }
}
```

Now, `monorail` knows that these pending changes are no longer considered changed:

```sh
monorail analyze | jq
```
```json
{
  "timestamp": "2024-10-27T11:20:10.471386+00:00",
  "targets": []
}
```

```sh
monorail -v run -c hello build
```
```json
{"timestamp":"2024-10-27T11:20:21.499439+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-10-27T11:20:21.500174+00:00","level":"INFO","message":"processing groups","num":0}
{"timestamp":"2024-10-27T11:20:21.500453+00:00","failed":false,"invocation_args":"-v run -c hello build","results":[]}
```

The `checkpoint` is a powerful way to control the behaviors of `analyze` and guided `run`. Since the checkpoint controls `monorail`s view of what has changed in the repository, and you can set the checkpoint id to any valid change provider reference, you can `analyze` and `run` for any point in history.

For example, this useful one-liner will perform a graph-guided run of the provided commands, and if successful, update the checkpoint to avoid running any commands for the involved targets until they change again: `monorail run -c <cmd1> <cmd2> ... <cmdN> && monorail checkpoint update -p`. This is most useful when the sequence of commands you provide covers everything you want to execute for a target in order for it to be considered "complete".

Locally, that might just be a bare minimum of commands; `monorail run prep check test integration-test && monorail checkpoint update -p`. In CI/CD, it might be more comprehensive: `monorail run prep check test integration-test package smoke dry-deploy && monorail checkpoint update -p`.

### Checkpointing in CI/CD

In a CI/CD environment, the id returned by `checkpoint update` or `checkpoint show` can be stored as a build artifact for one build and restored for the next one. This provides a means to do incremental command execution for builds, tests, packaging, deploys, and more.

To restore a checkpoint, provide `--id` or `-i` with a valid ID (sourced from a previous `checkpoint update` invocation) when updating the checkpoint:

```sh
monorail checkpoint update -p -i <id from previous checkpoint>
```

Additionally, using `monorail` with CI/CD it can be advantageous to retain state from previous builds (e.g. mounting a snapshot of a volume that is being used for builds for the current branch). This can make checkpoint restoration more useful, as you carry forward state from previous builds that have already been completed.

## Conclusion
This concludes the tutorial on the fundamentals of `monorail`. For most of what was covered here, additional options and configuration exists but are outside the scope of an introductory tutorial. For more information, use `monorail help` or pass the `--help` flag to subcommands.

# Development setup

`monorail` is written in Rust, so working on it is straightforward; this will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```
