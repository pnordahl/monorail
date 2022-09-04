# monorail
> Transform any git repository into a monorepo.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` transforms any git repo into a trunk-based development monorepo. It uses a file describing the various directory paths and relationship between those paths, extracts changes from git since the last tag, and derives what has changed. These changes can then be fed to other programs (e.g. `monorail-bash`) that can act on those changes.

`monorail` boils down to:

1. A `Monorail.toml` file that describes your repository layout
2. the `monorail inspect change` command, which reads `Monorail.toml`, analyzes `git` state between refs (usually between the most recent git annotated tag and HEAD), and returns what has changed
3. the `monorail-bash` program, which executes user-defined `bash` scripts either directly, or informed by `monorail inspect change` output
4. the `monorail release` command, which creates annotated tags (essentially the "checkpoint" for `monorail` change detection)

See the [tutorial](#tutorial) below for a practical walkthrough of how `monorail` and `monorail-bash` work.

## Installation

Ensure the following are installed and available on your system path:
* [Rust](https://www.rust-lang.org/tools/install)
* bash
* [`jq`](https://stedolan.github.io/jq/download/), used by the `monorail-bash` extension

`monorail` and all extensions can be installed from source by cloning the repository and executing the following, from the root of the repo:

	./install.sh 
	
By default, it will place these in `/usr/local/bin`, if another location is preferred, use `./install.sh <destination>`.

## Commands

Use `monorail help`

## Configuration

Create a `Monorail.toml` configuration file in the root of the repository you want `monorail` and extensions to use, referring to the `Monorail.reference.toml` file for an annotated example.

## Vocabulary

* extension: runs user-defined code written in a supported language; e.g. `bash`
* command: a function, defined by a target, that can be invoked in an executor-dependent fashion
* target: a path containing related files; the lowest level object that can be targeted by a command
* group: a path containing a collection of targets and related configuration

# Tutorial
In this tutorial, you'll learn:

  * how to inspect changes
  * how to define commands
  * how to execute commands
  * how to release

_NOTE: this tutorial assumes a UNIX-like environment._

First, create a fresh `git` repository (_not_ bare) that has been configured with at least one remote named `origin`. The easiest way to do this is to create a new repo on Github, and clone it locally. 

To get started, generate the following directory structure with `mkdir -p group1/foo/bar/support/script && touch Monorail.toml`:

```
group1
  foo/
    bar/
      support/
        script/
          monorail-exec.sh
Monorail.toml
```

Enter the following into `Monorail.toml`:

```
[vcs]
use = "git"

[extension]
use = "bash"

[[group]]
path = "group1"

  [[group.target]]
  path = "foo/bar"
```

If your repository trunk branch is `main`, also set the following (the default value is `master`):

```
[vcs.git]
trunk = "main"
```

## Inspecting changes

`monorail` will detect changes since the last release; see: [Releasing](##Releasing). For `git`, this means uncommitted, committed, and pushed files since the last annotated tag created by `monorail release`.

### Inspect showing no changes

Begin by viewing the output of `inspect change`:

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [],
        "target": [],
        "link": [],
        "depend": []
      }
    }
  }
}
```

As expected there are no changes.

Create a file in the path of one of the targets specified in `Monorail.toml` above. From the root of the repo, execute `touch group1/foo/bar/baz.txt`.

### Inspect showing a change

View changes again:

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
          {
            "name": "group1/foo/bar/baz.txt",
            "target": "group1/foo/bar",
            "action": "use",
            "reason": "target_match"
          }
        ],
        "target": [
          "group1/foo/bar"
        ],
        "link": [],
        "depend": []
      }
    }
  }
}
```

Our newly added file has been identified as matching target `group1/foo/bar`. 

### Inspect showing a change, after commit or push
This understanding persists between commits and pushes. Commit your changes: 

	git add * && git commit -am "first commit"

Then, view changes again:

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
          {
            "name": "group1/foo/bar/baz.txt",
            "target": "group1/foo/bar",
            "action": "use",
            "reason": "target_match"
          }
        ],
        "target": [
          "group1/foo/bar"
        ],
        "link": [],
        "depend": []
      }
    }
  }
}
```

`monorail` still knows about the change to target `group1/foo/bar`. 

Push, and view changes again:

	git push && monorail inspect change | jq .

The output remains the same.

## Defining commands

Commands are stored in a file on a per-target basis, the path to which is defined in `Monorail.toml`. In our case, that path will be `support/script/monorail-exec.sh` (the default) relative to `group1/foo/bar`. Create this file with: `touch group1/foo/bar/support/script/monorail-exec.sh`. Inside, we will define a simple script containing three commands:


```sh
#!/usr/bin/env bash

function command1() {
  echo "Hello, from command1"
  echo "The calling environment is inherited: ${SOME_EXTRA_VAR}"
  echo "some data" > side_effects.txt
}

function command2() {
  echo "Hello, from command2"
  cat side_effects.txt
}

function setup() {
  echo "Installing everything you need"
}
```

Commands can be named any valid UTF-8 string, and are free to do anything a normal `bash` script can do: call external build tools, perform network requests, etc.

## Executing commands

With the command script defined, it can be called with `monorail-bash exec`. This can be done in one of two ways:

  * implicitly, from change detection
  * explicitly, by specifying a list of targets


### Implicit

When done implicitly, `monorail-bash exec` uses the same processes that power `monorail inspect change` to derive targets and execute commands against them. To illustrate this use the following (`SOME_EXTRA_VAR` just illustrates that context can be passed to entrypoint scripts): 

	SOME_EXTRA_VAR=foo monorail-bash exec -c command1 -c command2 

```
Hello, from command1
The calling environment is inherited: foo
Hello, from command2
some data
```

The requested commands were executed in the desired order and did what was expected. 

Executing arbitrary functions against the changes detected by `monorail` has a number of applications, including:

  * executing commands against all projects you're currently working on, without specifically targeting them; `monorail` change detection ensures that for each changed target, the requested commands are executed sequentially
  * running specific commands against all changed targets as part of CI

### Explicit

Manually selecting targets gives one the ability to execute commands independent of VCS change detection. Applications include:

  * getting new developers up to speed working on a target codebase, as one can define all setup code in a set of commands and execute it against the target
  * run any command for any target in the entire repo, at will

To illustrate manually selecting targets, we will run the `setup` command we defined but did not execute previously. We will also use the `-v` flag to `monorail-bash` to instruct it to emit additional information. Execute the following:

	monorail-bash -v exec -t group1/foo/bar -c setup

```
Jan 21 15:13:21 monorail-bash : 'monorail' path:     monorail
Jan 21 15:13:21 monorail-bash : 'jq' path:          jq
Jan 21 15:13:21 monorail-bash : 'monorail' config:   Monorail.toml
Jan 21 15:13:21 monorail-bash : working directory:  /Users/patrick/scratch/monorail-tutorial
Jan 21 15:13:21 monorail-bash : commands:           setup
Jan 21 15:13:21 monorail-bash : targets:            group1/foo/bar
Jan 21 15:13:21 monorail-bash : start:
Jan 21 15:13:21 monorail-bash : end:
Jan 21 15:13:21 monorail-bash : Executing command; command: setup, target: group1/foo/bar
Installing everything you need
```

Before running our defined `setup` command, `monorail-bash -v` has emitted the configuration it's using, as well as a workflow message `Executing command; command: setup, target: group1/foo/bar`. 

## Releasing

`monorail` uses the backend VCS native mechanisms, e.g. tags in `git` as "release" markers. When a release is performed, it applies to all targets that were changed since the last release.

First, let's commit and push our current changes:

	git add * && git commit -am "update commands" && git push

Assuming that target commands have been run to our satisfaction and we have committed all that we intend to, we can dry run a patch release with: 

	monorail release --dry-run -t patch | jq .

```json
{
  "id": "v0.0.1",
  "targets": [
    "group1/foo/bar"
  ],
  "dry_run": true
}
```

`monorail` creates releases with an `id` appropriate to the conventions of the chosen VCS; in this case, that is the `git` semver tagging format. It also embeds the list of targets included as part of this release in the `targets` array; in the case of `git`, it will embed this list of targets in the release message.

Now, run a real release:

	monorail release -t patch | jq .

```json
{
  "id": "v0.0.1",
  "targets": [
    "group1/foo/bar"
  ],
  "dry_run": false
}
```

To show that the release cleared out `monorail`s view of changes, execute: 

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [],
        "target": [],
        "link": [],
        "depend": []
      }
    }
  }
}
```

While there are more subtleties we could show (multiple groups, multiple targets, dependencies, links, etc.), these are extensions of the fundamentals illustrated by this tutorial. Refer to `Monorail.reference.toml` for more information.


## Invariants

In order to work, `monorail` requires the following implicit invariants:

  1. groups may not reference paths in other groups
  2. targets may not reference paths in other targets
  3. only paths specified in a group's `link`, or `depend` configuration may be shared between targets of that group

_If any of these are violated, the behavior of `monorail` commands that analyze changes to the repository is undefined._


# Development setup

This will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```
