# monorail
> A fast and composable command execution tool optimized for monorepos.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` is optimized to support multi-project, multi-language monorepos. It provides APIs for describing paths in your repository and the relationships between them, analyzing changes, defining commands as executables written in any language, executing commands in parallel, logging, and more. `monorail` is designed to be fast, output structured text for use in other systems, allow the use of existing compilers and tools without the need for bespoke rules or plugins.

See the [tutorial](#tutorial) below for a practical walkthrough of how `monorail` works.

## Installation

### UNIX/Linux variants
At present, only source builds are supported. Packages for popular managers will be provided at a later time.

Ensure that Rust is installed and available on your system path:
* [Rust](https://www.rust-lang.org/tools/install)

Run `cargo install --path .`

### Windows

At this time, Windows is unsupported. However, there are no technical limitations preventing its support in the future.

## Overview

`monorail` is internally driven by two things:

1. A graph representation of your repository, built from a list of `target` entries and each target's `uses` list. 
2. A change detection provider, which provides a view of changes in the repository

Changes are mapped to affected targets, and a graph traversal powers various dependency-related tasks such as target grouping for parallel execution, "depended upon by" analysis, and so forth.

Monorail has a small lexicon:

* `change`: a created, updated, or deleted filesystem path as reported by a change provider
* `checkpoint`: a location in change provider history that marks the beginning of a sequence of changes
* `target`: a unique container that can be referenced by change detection and command execution
* `uses`: a set of paths that a target depends upon
* `ignores`: a set of paths that should not affect a target during change detection
* `command` executable code written in any language and invoked per target

The tutorial in the next section will elaborate on these concepts in a practical fashion.

# Tutorial

NOTE: You may want `jq` installed to pretty-print the output from various steps in this tutorial. If you don't want to install it, you can just omit any `| jq` that you find.

In this tutorial, you'll learn about:

  * mapping repository paths
  * analysis
  * defining and executing commands
  * command logging
  * checkpointing

## One-time setup

First, create a fresh `git` repository, and another to act as a remote:

```sh
git init --initial-branch=master monorail-tutorial
cd monorail-tutorial
git commit -m x --allow-empty # initial commit
echo 'monorail-out' > .gitignore
```

_NOTE_: the commit is to create a valid HEAD reference, and the branch checkout in the remote is to avoid `receive.denyCurrentBranch` errors from `git` when we push during the tutorial.


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

Now that our repository structure is in place, we will create a `Monorail.json` file that describes this structure in terms `monorail` understands. This command will create a `Monorail.json` file in the root of the repository, and simply map top level paths to targets:

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

Running `monorail config show | jq` produces the following output (which includes some default values for things not specified), indicating that our config file is well-formed:

```json
{
  "output_dir": "monorail-out",
  "max_retained_runs": 10,
  "change_provider": "git",
  "targets": [
    {
      "path": "rust",
      "uses": null,
      "ignores": null,
      "commands_path": "monorail",
      "commands": null
    },
    {
      "path": "python/app3",
      "uses": null,
      "ignores": null,
      "commands_path": "monorail",
      "commands": null
    },
    {
      "path": "proto",
      "uses": null,
      "ignores": null,
      "commands_path": "monorail",
      "commands": null
    }
  ]
}
```

## Preview: running commands

Commands will be covered in more depth later in the tutorial (along with logging), but now that we have a valid `Monorail.json` we can execute a command and view logs right away. Run the following to create an executable (in this case, a bash script) for the `rust` target:

```sh
cat <<EOF > rust/monorail/hello.sh
#!/bin/bash

echo 'Hello, world!'
echo 'An error message' >&2 
EOF
chmod +x rust/monorail/hello.sh
```

Now execute it: `monorail run -c hello`

```json
{
  "failed": false,
  "results": [
    {
      "command": "hello",
      "successes": [
        {
          "target": "rust",
          "code": 0,
          "stdout_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst",
          "stderr_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst",
          "runtime_secs": 0.002611583
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

Before we explore commands and logging in more detail, it's important to understand how `monorail` detects changes. In the next section we'll use the `analysis` API to do so.

## Analysis

`monorail` integrates with a `change_provider` to obtain a view of filesystem changes, which are processed along with a graph built from the specification in `Monorail.json`. Display an analysis of this changeset and graph with:

```sh
monorail analysis show | jq
```
```json
{
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

This indicates that based on our current changeset and graph, all three targets have changed. Display more information about the specific changes causing these targets to appear by adding `--change-targets`:
```sh
monorail analysis show --change-targets | jq
```
```json
{
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
monorail analysis show --changes | jq | less
```
```json
{
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

Running the analysis again, but as a preview for the next section add the `--target-groups` flag. Note that `proto` no longer appears in the list of changed targets, and a new array of arrays has appeared:

```sh
monorail analysis show --target-groups | jq
```
```json
{
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

Specifying dependencies between targets is done with the `uses` field on a `target` in `Monorail.json`. Run the following to set up a simple graph:

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
```

This has created a dependency on `proto` for `rust` and `python/app3` targets, connecting them for change detection and command execution. Observe the results of analysis now:

```sh
monorail analysis show --target-groups | jq
```
```json
{
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

## Logging and Results

Command output capture and result records are core responsibilities of a build tool like `monorail`, and so it provides a set of APIs for obtaining all data generated by `monorail run`. During command execution, all stdout and stderr is captured and stored in compressed archives, along with the overall results of the run. The number of historical runs retained is controlled by `max_retained_runs` in `Monorail.json`, defaulting to 10.

### Historical

We have already run a command, so let's show the result of that most recent run:

```sh
monorail result show | jq
```
```json
{
  "failed": false,
  "results": [
    {
      "command": "hello",
      "successes": [
        {
          "target": "rust",
          "code": 0,
          "stdout_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst",
          "stderr_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst",
          "runtime_secs": 0.002611583
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

This has started a server that will receive logs. Leave that running in a separate window, and let's update our existing command to demonstrate tailing:

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

This will run for a moment, printing output from stdout and stderr for us to observe. Now, run the command again with and additional flag `-v` to print useful workflow statements, and ensure that your log tailing window is in view:

```sh
monorail -v run -c hello -t rust
```
As this command executes, you'll see internal progress due to the inclusion of the `-v` flag:

```
{"timestamp":"2024-10-23T11:56:12.070820Z","level":"INFO","fields":{"message":"Connected to log stream server","address":"127.0.0.1:9201"}}
{"timestamp":"2024-10-23T11:56:12.073148Z","level":"INFO","fields":{"message":"processing groups","num":2}}
{"timestamp":"2024-10-23T11:56:12.073159Z","level":"INFO","fields":{"message":"processing targets","num":1,"command":"hello"}}
{"timestamp":"2024-10-23T11:56:12.073169Z","level":"INFO","fields":{"message":"task","status":"undefined","command":"hello","target":"proto"}}
{"timestamp":"2024-10-23T11:56:12.073612Z","level":"INFO","fields":{"message":"processing targets","num":1,"command":"hello"}}
{"timestamp":"2024-10-23T11:56:12.073626Z","level":"INFO","fields":{"message":"task","status":"scheduled","command":"hello","target":"rust"}}
{"timestamp":"2024-10-23T11:56:12.640462Z","level":"INFO","fields":{"message":"task","status":"success","command":"hello","target":"rust"}}
{"failed":false,"results":[{"command":"hello","successes":[],"failures":[],"unknowns":[{"target":"proto","code":null,"stdout_path":null,"stderr_path":null,"error":"command not found","runtime_secs":0.0}]},{"command":"hello","successes":[{"target":"rust","code":0,"stdout_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst","stderr_path":"/Users/patrick/lab/junk/monorail-tutorial/monorail-out/log/6/hello/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst","runtime_secs":0.5667086}],"failures":[],"unknowns":[]}]}
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

First, in the workflow logs:

```json
{"timestamp":"2024-10-23T11:16:06.143173Z","level":"INFO","fields":{"message":"task","status":"undefined","command":"hello","target":"proto"}}
```

Despite our selecting only the `rust` target with our run, `monorail` also attempted to run `hello` for the `proto` target. The reason for this behavior is to provide a measure of safety when choosing to run commands explicitly for a target. For example, if the `hello` command for `rust` required some output from its dependency `proto` to succeed, then it could fail if run in isolation. However, note that `proto` doesn't define this command so nothing happens and the run still succeeds.

The second item of note is the log stream header:
```
[monorail | stdout.zst, stderr.zst | (any target) | (any command)]
```

Every `monorail run` will print this header once at the beginning of a new log stream, indicating what data will be shown in the stream. When you start the tail process, by default it will not filter any targets or commands; this is indicated by the `(any target)` and `(any command)` entries in the header. If we had provided `--target rust proto` and `--command hello test build` (which can also be provided to `monorail log show`), then the header would look like `[monorail | stdout.zst, stderr.zst | rust, proto | hello, test, build]`, and only those logs that match these filters would appear. Additionally, note how multiple log block headers are printed; this is because the output from multiple files is independently collected and flushed at regular intervals and interleaved in the stream. In practice, you will often use target and command filters to reduce noise, but even without them block headers make it possible to visually parse the combined log stream.

## Commands

## Results


























Next, we will manipulate the changes being used to derive this list of targets with the `checkpoint`.

## Checkpoint

The `checkpoint` is a marker in change provider history (see: [Checkpointing](#checkpointing). When present, it is used as the beginning of an interval in history used for collecting a set of changes. The end of this interval is the latest point in change provider history. In addition, any pending changes not present in history are merged with the history set.

For `git`, the checkpoint is stored as a reference such as HEAD, or an object SHA. Pending changes are untracked and uncommitted files. Therefore, the set of changes reported by the `git` change provider is: `[<checkpoint>, ..., HEAD] + [untracked] + [uncommitted]`.

Now, for a practical example. First, query the checkpoint:

```sh
monorail checkpoint show
```
```json
{"kind":"error","type":"tracking_checkpoint_not_found","message":"Tracking checkpoint open error; No such file or directory (os error 2)"}
```

This error is fine, because by default no checkpoint exists. This means that all of the various files we've added are seen by `monorail` as changes. Now, update the checkpoint to include the current "latest point" in history; since we have only our initial HEAD commit, that's what we see:

```sh
monorail checkpoint update | jq
```
```json
{
  "checkpoint": {
    "id": "head",
    "pending": null
  }
}
```

This is however, not so useful; none of the files we've added have been committed so this doesn't affect analysis results like we'd want:

```sh
monorail analysis show | jq
```
```json
{
  "targets": [
    "python/app3",
    "rust"
  ]
}
```

The key is to add `--pending` or `-p` when updating the checkpoint, which will include those files and their SHA2 checksums in the checkpoint:

```sh
monorail checkpoint update --pending | jq
```
```json
{
  "checkpoint": {
    "id": "head",
    "pending": {
      "rust/monorail/hello.sh": "664e00829847270dd823957d46bf19b6c9618743527f7bfa057c338328911393",
      "rust/Cargo.toml": "a35f77bcdb163b0880db4c5efeb666f96496bcb409b4cd52ba6df517fb4d625b",
      "rust/app2/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9",
      "python/app3/tests/test_hello.py": "72b3668ed95f4f246150f5f618e71f6cdbd397af785cd6f1137ee87524566948",
      ".gitignore": "c1cf4f9ff4b1419420b8508426051e8925e2888b94b0d830e27b9071989e8e7d",
      "Monorail.json": "56d18cfea88a841a06e3f240f843e61aefec87866544aaee796de9b78b893a31",
      "rust/app1/Cargo.toml": "044de847669ad2d9681ba25c4c71e584b5f12d836b9a49e71b4c8d68119e5592",
      "rust/app2/Cargo.toml": "111f4cf0fd1b6ce6f690a5f938599be41963905db7d1169ec02684b00494e383",
      "proto/README.md": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "python/app3/hello.py": "3639634f2916441a55e4b9be3497673f110014d0ce3b241c93a9794ffcf2c910",
      "rust/app1/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9"
    }
  }
}
```

Now, `monorail` knows that these pending changes are no longer considered changed:

```sh
monorail analysis show | jq
```
```json
{
  "targets": []
}
```

<!-- 

-show how this affects implicit target selection for command execution
-show checkpoint deletion

-show ci example of `monorail run -c prep check build unit-test integration-test package && monorail checkpoint update` is useful for something like CI; only update the checkpoint if all commands for all affected targets succeed`

-give recommended usage of commands, e.g. checking if output from command already exists (like if target signature is the same)
-give recommended usage in ci (e.g. run branch builds on virts with a persistent volume, so that subsequent runs can use cached artifacts)

-address some things like <why no bazel feature>, e.g. caching and remote execution

-->

The `checkpoint` is a powerful way to control the view of changes in a repository. Here are a few ways you can use it:

1. 









# Development setup

This will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```
