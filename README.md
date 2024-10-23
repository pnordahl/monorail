# monorail
> A fast and composable build tool optimized for monorepos.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` is a tool for describing a repository as a trunk-based development monorepo. It uses a file describing the various directory paths and relationship between those paths, integrates with your version control tool, and outputs data about how the changes affect your defined targets. Striving to embrace the UNIX-philosophy, `monorail` is entirely agnostic to language, build tools, compilers, and so on. It emits structured text, making this output easily composed with other programs to act on those changes.

See the [tutorial](#tutorial) below for a practical walkthrough of how `monorail` and `monorail-bash` work.

## Installation

### UNIX/Linux variants
At present, only source builds are supported. Packages for popular managers will be provided at a later time.

Ensure that Rust is installed and available on your system path:
* [Rust](https://www.rust-lang.org/tools/install)

Run `cargo install --path .`

### Windows

At this time, Windows is unsupported.

## Overview

`monorail` is internally driven by two things:

1. A graph representation of your repository, built from a list of `target` entries and each target's `uses` list. 
2. A change detection provider, which provides a list of files that have changed in the repository

Changes are mapped to affected targets, and a graph traversal powers various dependency-related tasks such as target grouping for parallel execution, "depended upon by" analysis, and so forth.

Monorail has a small lexicon:

* `change`: a created, updated, or deleted filesystem path as reported by a change provider
* `checkpoint`: a location in vcs history that marks the end of a sequence of changes
* `target`: a unique container that can be referenced by change detection and script execution.
* `uses`: a set of paths that a target depends upon
* `ignores`: a set of paths within a target path that should not be considered during change detection
* `function` code written in a supported language and invoked per target

The tutorial in the next section will elaborate on these concepts in a practical fashion.

# Tutorial

NOTE: You'll want `jq` installed to pretty-print the output from various steps in this tutorial. If you don't want to install it, you can just omit any `| jq` that you find.

In this tutorial, you'll learn about:

  * mapping repository paths to targets, uses, and ignores
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
  "max_retained_run_results": 10,
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

Commands will be covered in more depth later in the tutorial (along with logging), but now that we have a valid `Monorail.toml` we can execute a command and view logs right away. Run the following to create an executable (in this case, a bash script) for the `rust` target:

```sh
cat <<EOF > rust/monorail/test.sh
#!/bin/bash

echo "Hello, world!"
EOF
chmod +x rust/monorail/test.sh
```

Now execute it: `monorail run -c test`

```json
{
  "failed": false,
  "results": [
    {
      "command": "test",
      "successes": [
        {
          "target": "rust",
          "code": 0,
          "stdout_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/test/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stdout.zst",
          "stderr_path": "/Users/patrick/lab/monorail-tutorial/monorail-out/log/6/test/521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3/stderr.zst",
          "runtime_secs": 0.002611583
        }
      ],
      "failures": [],
      "unknowns": []
    }
  ]
}
```

You can also view logs, both historically and by tailing (shown in more detail later). Show the logs for the most recent `run`:

```sh
monorail log show --stdout
```
```
[monorail | stdout.zst | rust | test]
Hello, world!
```

In this example, we just used `monorail` as a simple command runner like `make` or `just`. However, unlike these other tools `monorail` is capable of running commands in parallel based on your dependency graph, streaming logs, collecting historical results and compressed logs, and more.

Before we explore commands and logging in more detail, it's important to understand how `monorail` detects changes. In the next section we'll use the 

## Analyzing changes

`monorail` integrates with a `change_provider` to obtain a view of filesystem changes, which are processed along with a graph built from the specification in `Monorail.toml`. Display an analysis of this changeset and graph with:

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

This indicates that based on our current changeset and graph, all three targets have changed. Display more information by adding `--change-targets`:
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

Unfortunately, hundreds of virtualenv files are in our changes array. In the next section, we'll rectify this.

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
      "path": "proto/app.proto"
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

The `target_groups` array is built by constructing a graph from the dependencies specified in `Monorail.json`. This is used to control parallel command execution, ensuring that maximum parallelism is achieved while respecting the hierarchy of dependencies at any given time. In the next section, we will establish a simple dependency graph between our targets.

## Dependencies

## Logging 

### Historical

### Tailing

## Commands




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

The key is to add --pending or -p when updating the checkpoint, which will include those files and their SHA2 checksums in the checkpoint:

```sh
monorail checkpoint update --pending | jq
```
```json
{
  "checkpoint": {
    "id": "head",
    "pending": {
      "rust/monorail/test.sh": "664e00829847270dd823957d46bf19b6c9618743527f7bfa057c338328911393",
      "rust/Cargo.toml": "a35f77bcdb163b0880db4c5efeb666f96496bcb409b4cd52ba6df517fb4d625b",
      "rust/app2/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9",
      "python/app3/tests/test_hello.py": "72b3668ed95f4f246150f5f618e71f6cdbd397af785cd6f1137ee87524566948",
      ".gitignore": "c1cf4f9ff4b1419420b8508426051e8925e2888b94b0d830e27b9071989e8e7d",
      "Monorail.json": "56d18cfea88a841a06e3f240f843e61aefec87866544aaee796de9b78b893a31",
      "rust/app1/Cargo.toml": "044de847669ad2d9681ba25c4c71e584b5f12d836b9a49e71b4c8d68119e5592",
      "rust/app2/Cargo.toml": "111f4cf0fd1b6ce6f690a5f938599be41963905db7d1169ec02684b00494e383",
      "proto/app.proto": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
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

The `checkpoint` is a powerful way to control the view of changes in a repository. Here are a few ways you can use it:

1. 























### Analyze showing no affected targets

Begin by viewing the output of `analyze`:

```sh
monorail analyze | jq .
```

```json
{
  "targets": []
}
```

`monorail` is able to interrogate `git` and use the `Monorail.toml` config successfully. No targets are affect by the current changes; let's get more information about those changes by adding the `--show-changes` flag:

```sh
monorail analyze --show-changes | jq
```

```json
{
  "changes": [
    {
      "path": "Monorail.toml"
    }
  ],
  "targets": []
}
```

The `changes` array contains a list of objects for each detected change. The file we added is represented in this array, but it has no relevance to the `rust` target because it lies outside of its path.

_NOTE_: the remainder of this tutorial will pass the `--show-change-targets` flag to `monorail analyze`, which will display changes with additional information about the targets affected. While verbose, this will provide insight into how changes are interacting with our `Monorail.toml` configuration. For general use outside of this tutorial, you can just use `monorail analyze`.

### Analyze showing an affected target

Let's start by adding another target, in a subdirectory of our target `rust`:

```sh
cat <<EOF > Monorail.toml
[[targets]]
path = "rust"

[[targets]]
path = "rust/target1"
EOF
```

Now, create a new file in `rust/target1` and `analyze`, this time with the `--show-change-targets` flag:

```sh
mkdir rust/target1 && \
touch rust/target1/foo.txt && \
monorail analyze --show-change-targets | jq .
```

```json
{
  "changes": [
    {
      "path": "Monorail.toml",
      "targets": []
    },
    {
      "path": "rust/target1/foo.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target1",
          "reason": "target"
        }
      ]
    }
  ],
  "targets": [
    "rust",
    "rust/target1"
  ]
}
```

`monorail` has identified that the newly added file affects both of our defined targets.

```json
[
  "rust",
  "rust/target1"
]
```

### Analyze showing a change, after commit or push
This understanding of what has changed persists between commits and pushes. Commit your changes, and then note that our change output is unchanged:

```sh
git add * && git commit -am "x" && monorail analyze | jq .
```

Push, and view changes again:

```sh
git push && monorail analyze | jq .
```

As with the commit, `monorail` still knows about the changes after a push. The reason for this will be explained in the section on [Checkpointing](#checkpointing) below.

## Declaring "uses" and "links"

`monorail` allows for targets to be affected by paths outside of the `path` each project has declared. This allows for targets to reference paths containing utility code, serialization files (e.g. protobuf definitions), configuration, etc. When these paths have changes, it triggers targets that use them to be changed.

### Uses

Begin by creating a directory to be used by `rust/target1`, and a directory to hold a new target, `rust/target2`:

```sh
mkdir -p rust/common/library1
mkdir rust/target2
```

Execute the following to adjust the `[[targets]]` section of `Monorail.toml` to specify `rust/target2`, and make `rust/target2` use `rust/common/library1`:

```sh
cat <<EOF > Monorail.toml
[[targets]]
path = "rust"

[[targets]]
path = "rust/target1"

[[targets]]
path = "rust/target2"
uses = [
  "rust/common/library1"
]
EOF
```

Any changes that lie within the paths defined in the target's `uses` array affect the target.

To trigger change detection, create a file in library1 and re-analyze:

```sh
touch rust/common/library1/foo.proto && \
monorail analyze --show-change-targets | jq .
```


```json
{
  "changes": [
    {
      "path": "Monorail.toml",
      "targets": []
    },
    {
      "path": "rust/common/library1/foo.proto",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target2",
          "reason": "uses"
        }
      ]
    },
    {
      "path": "rust/target1/foo.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target1",
          "reason": "target"
        }
      ]
    }
  ],
  "targets": [
    "rust",
    "rust/target1",
    "rust/target2"
  ]
}
```

Since the added `rust/common/library1/foo.proto` file lies within `rust/target2`'s `uses` array, that change has caused `rust/target2` to appear in the final `targets` array.

### Links

A change falling within a path specified in a `links` array affects all targets _recursively_ without them opting-in. To demonstrate, we will create a third project and a `rust/vendor` directory to link all targets to:

Execute the following to adjust the `[[targets]]` section of `Monorail.toml` to specify this new project, as well as a target `links`:

```sh
cat <<EOF > Monorail.toml
[[targets]]
path = "rust"
links = [
  "rust/vendor"
]

[[targets]]
path = "rust/target1"

[[targets]]
path = "rust/target2"
uses = [
  "rust/common/library1"
]

[[targets]]
path = "rust/target3"
EOF
```

Now, make the directory structure and re-analyze:

```sh
mkdir rust/project3
mkdir rust/vendor
touch rust/vendor/bar.txt
monorail analyze --show-change-targets | jq .
```

```json
{
  "changes": [
    {
      "path": "Monorail.toml",
      "targets": []
    },
    {
      "path": "rust/common/library1/foo.proto",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target2",
          "reason": "uses"
        }
      ]
    },
    {
      "path": "rust/target1/foo.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target1",
          "reason": "target"
        }
      ]
    },
    {
      "path": "rust/vendor/bar.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust",
          "reason": "links"
        },
        {
          "path": "rust/target1",
          "reason": "links"
        },
        {
          "path": "rust/target2",
          "reason": "links"
        },
        {
          "path": "rust/target3",
          "reason": "links"
        }
      ]
    }
  ],
  "targets": [
    "rust",
    "rust/target1",
    "rust/target2",
    "rust/target3"
  ]
}
```

Note that `project3` did not need to opt-in to changes in `rust/vendor`; simply being within the path of `rust`'s `links` definition is enough. While this is useful, sometimes a target needs to opt-out of certain paths. For that, there is the `ignores` array.

## Ignoring paths

Any changes that lie within the paths defined in the target's `ignores` array will not affect the target.

Execute the following to adjust the `[[targets]]` section of `Monorail.toml` to specify this new project, as well as a target `links`:

```sh
cat <<EOF > Monorail.toml
[[targets]]
path = "rust"
links = [
  "rust/vendor"
]

[[targets]]
path = "rust/target1"

[[targets]]
path = "rust/target2"
uses = [
  "rust/common/library1"
]

[[targets]]
path = "rust/target3"
ignores = [
  "rust/vendor/bar.txt"
]
EOF
```

Executing `monorail analyze --show-change-targets | jq .` yields:

```json
{
  "changes": [
    {
      "path": "Monorail.toml",
      "targets": []
    },
    {
      "path": "rust/common/library1/foo.proto",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target2",
          "reason": "uses"
        }
      ]
    },
    {
      "path": "rust/target1/foo.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust/target1",
          "reason": "target"
        }
      ]
    },
    {
      "path": "rust/vendor/bar.txt",
      "targets": [
        {
          "path": "rust",
          "reason": "target"
        },
        {
          "path": "rust",
          "reason": "links"
        },
        {
          "path": "rust/target1",
          "reason": "links"
        },
        {
          "path": "rust/target2",
          "reason": "links"
        },
        {
          "path": "rust/target3",
          "reason": "links"
        },
        {
          "path": "rust/target3",
          "reason": "ignores"
        }
      ]
    }
  ],
  "targets": [
    "rust",
    "rust/target1",
    "rust/target2"
  ]
}
```

The added entry to `rust/target3`'s `ignores` has taken precedence over the `rust` `links` entry, and removed `rust/target3` from the final targets list.

## Defining commands

Commands are run by extensions, which are "runners" for user-defined code. The default runner is `bash`, so we will proceed with `monorail-bash`:

Commands are stored in a file on a per-target basis, the path to which is defined in `Monorail.toml`. In our case, that path will be `support/script/monorail-exec.sh` (the default) relative to _each target's path_.

Create the path to this file with: 

```sh
mkdir -p rust/support/script
```

In the `rust/support/script/monorail-exec.sh` file, we will define a script containing three commands:

```sh
cat <<"EOF" > rust/support/script/monorail-exec.sh
#!/usr/bin/env bash

function hello_world() {
  echo "Hello, world... from rust!"
}
EOF
```

Command names can be named any valid UTF-8 string, and are free to do anything a normal `bash` script can do: source other shell scripts, call build tools, perform network requests, etc. One of the benefits of `monorail` is that it does not limit the build tooling you can use.

## Executing commands

With the command script defined, it can be called with `monorail-bash exec`. This can be done in one of two ways:

  * implicitly, from the change detection output of `monorail analyze`
  * explicitly, by specifying a list of targets

Commands are executed sequentially in the alphabetical order provided in the `targets` array from `monorail analyze`. In the future, a means to specify this execution order by way of a dependency graph may be added.

### Implicit

When done implicitly, `monorail-bash exec` uses the same processes that power `monorail analyze` to derive changed targets and execute commands against them. To illustrate this, execute: 

```sh
monorail-bash exec -c hello_world
```

which prints...
```
Hello, world... from rust!
```

Notice that we did not have to specify targets; `monorail-bash` used the output from `monorail analyze` to figure that out and then call the `hello_world` function for those targets that have defined it.

To enable a wealth of debugging information, pass the `-v` flag:

```sh
monorail-bash -v exec -c hello_world
```

```
Sep 03 09:40:16 monorail-bash : 'monorail' path:    monorail
Sep 03 09:40:16 monorail-bash : 'jq' path:          jq
Sep 03 09:40:16 monorail-bash : 'git' path:         git
Sep 03 09:40:16 monorail-bash : use libgit2 status: false
Sep 03 09:40:16 monorail-bash : 'monorail' config:  Monorail.toml
Sep 03 09:40:16 monorail-bash : working directory:  /Users/patrick/lab/github.com/pnordahl/monorail-tutorial
Sep 03 09:40:16 monorail-bash : command:            hello_world
Sep 03 09:40:16 monorail-bash : start:              
Sep 03 09:40:16 monorail-bash : end:                
Sep 03 09:40:17 monorail-bash : target (inferred):  rust
Sep 03 09:40:17 monorail-bash : target (inferred):  rust/target1
Sep 03 09:40:17 monorail-bash : target (inferred):  rust/target2
Sep 03 09:40:17 monorail-bash : Executing command; command: hello_world, target: rust
Hello, world... from rust!
Sep 03 09:40:17 monorail-bash : NOTE: Script not found; command: hello_world, target: rust/target1
Sep 03 09:40:17 monorail-bash : NOTE: Script not found; command: hello_world, target: rust/target2
```

The majority of this output is workflow and debugging information, but it's worth noting a few key pieces.

* targets that do not define a command script are skipped
* targets that do not define the desired command are skipped

Executing arbitrary bash functions against the changes detected by `monorail` has a number of applications, including:

  * executing commands against all projects/dependencies/links you've modified, without specifically targeting them; `monorail-bash` ensures that for each changed target, the requested commands are executed sequentially
  * running specific commands against all changed targets as part of CI (e.g. `check`, `build`, `test`, `deploy`, etc.)

### Explicit

Manually selecting targets gives one the ability to execute commands independent of VCS change detection. Applications include:

  * getting new developers up to speed working on a codebase, as one can define all setup code for each target in a function like `bootstrap` and execute it against a top-level target, e.g. `monorail-bash -c bootstrap -t projects`
  * run any command for any target in the entire repo, as desired

Manually selecting targets is simple enough; just pass a series of `-t <target>` options. Execute the following (removing the `-v` to cut down on the visual noise):

```sh
monorail-bash exec -t rust -c hello_world
```

```
Hello, world... from rust!
```

For more information, execute `monorail-bash -h`, and `monorail-bash exec -h`.

## Checkpointing

`monorail` uses the backend VCS native mechanisms, e.g. tags in `git` as "checkpoint" markers. This creates a "checkpoint" for change detection. Without checkpoint tags, `monorail` is forced to search `git` history back to the first commit. This would be inefficient and make change detection useless, as all targets would be considered changed forever.

When a checkpoint is created, it applies to all targets that were changed since the previous checkpoint (or the first commit of the repository, if no checkpoints yet exist).

First, let's commit and push our current changes:

```sh
git add * && git commit -am "update commands" && git push
```

Assuming that we have committed all that we intend to, and target commands have been run to our satisfaction (e.g. CI has passed for the merge of our branch), we can dry-run a patch checkpoint with: 

```sh
monorail checkpoint create --dry-run -t patch | jq .
```

```json
{
  "id": "monorail-1",
  "targets": [
    "group1/Lockfile",
    "group1/common/library1",
    "group1/project1",
    "group1/project2",
    "group1/project3"
  ],
  "dry_run": true
}
```

`monorail` creates checkpoints with an `id` appropriate to the conventions of the chosen VCS; in this case, that is the `git` semver tagging format. It also embeds the list of targets included as part of this checkpoint in the `targets` array; in the case of `git`, it will embed this list of targets in the checkpoint message.

Now, run a real checkpoint:

```sh
monorail checkpoint create -t patch | jq .
```

```json
{
  "id": "monorail-1",
  "targets": [
    "group1/Lockfile",
    "group1/common/library1",
    "group1/project1",
    "group1/project2",
    "group1/project3"
  ],
  "dry_run": false
}
```

To show that the checkpoint cleared out `monorail`s view of changes, execute: 

```sh
monorail analyze | jq .
```

```json
{
  "targets": []
}
```

Finally, our newly-pushed tag is now in the remote. To see this, execute:

```sh
git -C ../monorail-tutorial-remote show -s --format=%B monorail-1
```

... which outputs

```
tag monorail-1
Tagger: John Doe <john.doe@gmail.com>

rust
rust/target1
rust/target2

update commands
```

This concludes the tutorial. Now that you have seen how the core concepts of `monorail` and extensions work, you're ready to use it in real projects. Experiment with repository layouts, commands, CI, and working on a trunk-based development workflow that works for your teams.

Refer to `Monorail.reference.toml` for more specific information about the various configuration available for `monorail` and extensions.

# Development setup

This will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```

You can use `install.sh` to build a release binary of `monorail` and copy it, along with extensions, into your PATH.
