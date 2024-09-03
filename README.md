# monorail
> Transform any git repository into a monorepo.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` is a tool for describing a repository as a trunk-based development monorepo. It uses a file describing the various directory paths and relationship between those paths, integrates with your version control tool, and outputs data about how the changes affect your defined targets. Striving to embrace the UNIX-philosophy, `monorail` is entirely agnostic to language, build tools, compilers, and so on. It emits structured text, making this output easily composed with other programs to act on those changes.

The provided `monorail-bash` script uses the output of `monorail` to drive a robust user-defined scripting build tool. To use it, you create ordinary shell script files, define functions, and then execute them either implicitly via `monorail` change detection, or explicitly by selecting targets.

See the [tutorial](#tutorial) below for a practical walkthrough of how `monorail` and `monorail-bash` work.

## Installation

At present, only source builds are supported. Packages for popular managers will be provided at a later time.

Ensure the following are installed and available on your system path:
* [Rust](https://www.rust-lang.org/tools/install)
* bash
* [`jq`](https://stedolan.github.io/jq/download/), used by the `monorail-bash` extension for the default `--output-format` of `json`

`monorail` and all extensions can be installed from source by cloning the repository and executing the following, from the root of the repo:

	./install.sh 
	
By default, it will place these in `/usr/local/bin`, if another location is preferred, use `./install.sh <destination>`.

Note that while `monorail` can be installed from crates.io via `cargo install`, `cargo` does not support installing additional resources such as the script entrypoint for `monorail-bash`. The `install.sh` script handles both the binary and extension installation.

## Commands

Use `monorail help`

## Configuration

Create a `Monorail.toml` configuration file in the root of the repository, referring to the `Monorail.reference.toml` file for an annotated example.

## Vocabulary

Monorail has a simple lexicon:

* vcs: the repository version control system that `monorail` integrates with to detect changes
* change: a CRUD operation reported by the vcs as a filesystem path
* checkpoint: a location in vcs history that marks the end of a sequence of changes

`monorail` works by labeling filesystem paths within a repository as meaningful. These labels determine how changes reported by the vcs are interpreted. The label types are:

* target: A unique container that can be referenced by change detection and script execution. For example, `ui`, `api`, and `database`, as directories carrying files specific to those projects, would be good candidates for being labeled as targets. So too would a parent directory like `apps`, because targets are _recursive_, which means that targets can lie within the path of other targets.
* links: A set of paths that recursively affect a target and any targets that lie in its path. For example, a `vendor` directory carrying copies of third-party code may require that all targets within a target's path subdirectories be re-tested.
* uses: A set of paths that affect a target. For example, a directory called `common`, carrying files used by multiple targets might be referred to by multiple targets in their `uses` arrays.
* ignores: a set of paths that do not affect a target

For executing code against targets, two more terms:

* extension: runs user-defined code written in a supported language
* command: a function optionally defined in a target's script

# Tutorial

_NOTE: this tutorial assumes a UNIX-like environment._

In this tutorial, you'll learn:

  * mapping repository paths to targets
  * analyzing
  * defining commands
  * executing commands
  * checkpointing

## One-time setup

First, create a fresh `git` repository, and another to act as a remote:

```sh
git init --initial-branch=master monorail-tutorial
git init --initial-branch=master monorail-tutorial-remote
REMOTE_TRUNK=$(git -C monorail-tutorial-remote branch --show-current)
git -C monorail-tutorial-remote checkout -b x
cd monorail-tutorial
git remote add origin ../monorail-tutorial-remote
git commit --allow-empty -m "HEAD"
git push --set-upstream origin $REMOTE_TRUNK
```

_NOTE_: the commit is to create a valid HEAD reference, and the branch checkout in the remote is to avoid `receive.denyCurrentBranch` errors from `git` when we push during the tutorial.

## Defining a target

To get started, generate a directory structure with the following shell commands:

```sh
mkdir -p rust
touch Monorail.toml
```
... which yields the following directory structure:

```
├── Monorail.toml
└── rust
```

_NOTE_: the remainder of this tutorial will apply updates to the `Monorail.toml` file with heredoc strings, for convenience.

Execute the following to map the path `rust` to a target in `Monorail.toml`; this will act as a sort of "group" for additional targets we will create later:

```sh
cat <<EOF > Monorail.toml
[[targets]]
path = "rust"
EOF
```

_NOTE_: All filesystem locations specified in `Monorail.toml` are relative to the root the repository.

## Analyzing changes

`monorail` will detect changes since the last checkpoint; see: [Checkpointing](##Checkpointing). For `git`, this means uncommitted, committed, and pushed files since the last annotated tag created by `monorail checkpoint`.

### Analyze showing no affected targets

Begin by viewing the output of `analyze`:

	monorail analyze | jq .

```json
{
  "targets": []
}
```

`monorail` is able to interrogate `git` and use the `Monorail.toml` config successfully. No targets are affect by the current changes; let's get more information about those changes by adding the `--show-changes` flag:

    monorail analyze --show-changes | jq

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

	git add * && git commit -am "x" && monorail analyze | jq .

Push, and view changes again:

	git push && monorail analyze | jq .

As with the commit, `monorail` still knows about the changes after a push. The reason for this will be explained in the section on checkpointing below.

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

To trigger change detection, create a file in library1:

	touch rust/common/library1/foo.proto

and then `monorail analyze --show-change-targets | jq .`:

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

```sh
mkdir rust/project3
mkdir rust/vendor
touch rust/vendor/bar.txt
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
Hello, world.. from rust!
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
Hello, world.. from rust!
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

	monorail-bash exec -t rust -c hello_world

```
Hello, world.. from rust!
```

For more information, execute `monorail-bash -h`, and `monorail-bash exec -h`.

## Checkpointing

`monorail` uses the backend VCS native mechanisms, e.g. tags in `git` as "checkpoint" markers. This creates a "checkpoint" for change detection. Without checkpoint tags, `monorail` is forced to search `git` history back to the first commit. This would be ineffecient and make change detection useless as all targets would be considered changed over a long enough timeline.

When a checkpoint is created, it applies to all targets that were changed since the previous checkpoint (or the first commit of the repository, if no checkpoints yet exist).

First, let's commit and push our current changes:

```sh
git add * && git commit -am "update commands" && git push
```

Assuming that we have committed all that we intend to, and target commands have been run to our satisfaction (e.g. CI has passed for the merge of our branch), we can dry-run a patch checkpoint with: 

```sh
monorail checkpoint --dry-run -t patch | jq .
```

```json
{
  "id": "v0.0.1",
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
monorail checkpoint -t patch | jq .
```

```json
{
  "id": "v0.0.1",
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
git -C ../monorail-tutorial-remote show -s --format=%B v0.0.1
```

... which outputs

```
tag v0.0.1
Tagger: John Doe <john.doe@gmail.com>

rust
rust/target1
rust/target2update commands

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
