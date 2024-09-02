# monorail
> Transform any git repository into a monorepo.

![Build Status](https://github.com/pnordahl/monorail/actions/workflows/branch.yml/badge.svg?branch=main)
[![Cargo](https://img.shields.io/crates/v/monorail.svg)](https://crates.io/crates/monorail)

`monorail` transforms any git repo into a trunk-based development monorepo. It uses a file describing the various directory paths and relationship between those paths, extracts changes from git since the last tag, and derives what has changed. These changes can then be fed to other programs (e.g. `monorail-bash`) that can act on those changes. While `monorail` currently supports only `git` as the VCS backend, support for others could be added.

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
* [`jq`](https://stedolan.github.io/jq/download/), used by the `monorail-bash` extension for the default `--output-format` of `json`

`monorail` and all extensions can be installed from source by cloning the repository and executing the following, from the root of the repo:

	./install.sh 
	
By default, it will place these in `/usr/local/bin`, if another location is preferred, use `./install.sh <destination>`.

Note that while `monorail` can be installed from crates.io via `cargo install`, `cargo` does not support installing additional resources such as the script entrypoint for `monorail-bash`. The `install.sh` script handles both the binary and extension installation.

## Commands

Use `monorail help`

## Configuration

Create a `Monorail.toml` configuration file in the root of the repository you want `monorail` and extensions to use, referring to the `Monorail.reference.toml` file for an annotated example.

## Vocabulary

Monorail has a simple lexicon:

* vcs: the repository version control system, and internal functions that `monorail` uses to detect changes
* change: a CRUD operation reported by the vcs as a filesystem path

`monorail` works by labeling filesystem paths within a repository as meaningful. These labels determine how changes reported by the vcs are interpreted. The label types are:

* target: a unique container that can be referenced by change detection and script execution; targets are _recursive_, which means that targets can lie within other targets
* links: a set of paths that recursively affect a target and any targets that lie in its path
* uses: a set of paths that affect a target
* ignores: a set of paths that do not affect a target

For executing code against targets, two more terms:

* extension: runs user-defined code written in a supported language
* command: a function optionally defined in a target's script

# Tutorial

_NOTE: this tutorial assumes a UNIX-like environment._

In this tutorial, you'll learn:

  * how to declare a group, project, depend, and link
  * how to inspect changes
  * how to define commands
  * how to execute commands
  * how to release

First, create a fresh `git` repository, and another to act as a remote:

_NOTE_: This assumes a `init.defaultBranch` of `master` or empty string, the default for `git`. If yours is something else, change the `master` in the `git push` command to that.

```sh
git init monorail-tutorial
git init monorail-tutorial-remote
REMOTE_TRUNK=$(git -C monorail-tutorial-remote branch --show-current)
git -C monorail-tutorial-remote checkout -b x
pushd monorail-tutorial
git remote add origin ../monorail-tutorial-remote
git commit --allow-empty -m "HEAD"
git push --set-upstream origin $REMOTE_TRUNK
popd
```

_NOTE_: the commit is to create a valid HEAD reference, and the branch checkout in the remote is to avoid `receive.denyCurrentBranch` errors from `git` when we push during the tutorial.

To get started, generate a directory structure with the following shell commands:

```sh
cd monorail-tutorial
mkdir -p group1/project1
touch Monorail.toml
```
... which yields the following directory structure:

```
├── Monorail.toml
└── group1
    └── project1
```

_NOTE_: the remainder of this tutorial will apply updates to the `Monorail.toml` file with heredoc strings, for convenience.

Execute the following to specify the new group and project in `Monorail.toml`, as well as an `extension` to be used later in the tutorial:

```sh
cat <<EOF > Monorail.toml
[vcs]
use = "git"

[vcs.git]
trunk = "$(git branch --show-current)"

[extension] 
use = "bash"

[[group]]
path = "group1"

  [[group.project]]
  	path = "project1"

EOF
```

### Recap

Your `monorail` config file (default: `Monorail.toml`) describes your existing repository layout in terms of `monorail` concepts. A `project` is a path to be developed/deployed/tested as a unit, e.g. a backend service, web app, etc. A `group` is a set of _related projects_, and defines what can be shared amongst projects (more on sharing later).

Finally, many of `monorail`s capabilities are path-based. Our definition of `group.path` (relative to the repository root) and `project.path` (relative to the specified `group.path`) declare where these objects live in our repository.

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
        "project": [],
        "link": [],
        "depend": []
      }
    }
  }
}
```

As expected there are no changes, and `monorail` is able to interrogate `git` and use the `Monorail.toml` config successfully. The meaning of the `file`, `project`, `link`, and `depend` fields will be explained as the tutorial progresses.

### Inspect showing a change

To show what an actual change looks like in `monorail` output, create a new file in `project1`:

	touch group1/project1/foo.txt

View changes again:

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
          {
            "name": "group1/project1/foo.txt",
            "project": "group1/project1",
            "action": "use",
            "reason": "project_match"
          }
        ],
        "project": [
          "group1/project1"
        ],
        "link": [],
        "depend": []
      }
    }
  }
}
```

`monorail` has identified that the newly added file represents a meaningful change, based on our configuration in `Monorail.toml`.

The `change.file` array contains a list of objects containing metadata about the change detected. It contains the `name` of the file (a path relative to the repository root), the `project` the file belongs to, the `action` taken by `monorail` during change detection (e.g. it was `use`d), and the `reason` that `action` was taken (e.g. it matched a declared project).

The `change.project` array contains a list of paths relative to the repo root for projects detected as changed. This list is de-duped across all `change.file` entries; a project will appear at most once in this list.

### Inspect showing a change, after commit or push
This understanding of what has changed persists between commits and pushes. Commit your changes: 

	git add * && git commit -am "x"

Then, view changes again:

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
          {
            "name": "group1/project1/foo.txt",
            "project": "group1/project1",
            "action": "use",
            "reason": "project_match"
          }
        ],
        "project": [
          "group1/project1"
        ],
        "link": [],
        "depend": []
      }
    }
  }
}
```

`monorail` still knows about the change to the project `group1/project1`.

Push, and view changes again:

	git push && monorail inspect change | jq .

The output remains the same.


## Declaring dependencies and links

`monorail` allows for projects to depend on paths outside of the `path` each project has declared. This allows for reference paths containing utility code, serialization files (e.g. protobuf definitions), configuration, etc. When these paths have changes, it triggers projects that depend on them to be changed.


### Dependencies

Begin by creating a directory to be used as a dependency, and a directory to hold a new project, `project2`:

```sh
mkdir -p group1/common/library1
mkdir group1/project2
```

Execute the following to adjust the `[[group]]` section of `Monorail.toml` to add `library1` as a depend-able path, specify `project2`, and make `project2` depend on `library1`:

```sh
cat <<EOF > Monorail.toml
[vcs]
use = "git"

[vcs.git]
trunk = "$(git branch --show-current)"

[extension] 
use = "bash"

[[group]]
path = "group1"
depend = [
	"common/library1"
]

  [[group.project]]
  	path = "project1"

  [[group.project]]
  	path = "project2"

  	depend = [
  		"common/library1"
  	]

EOF
```

The `depend` declaration in the `group` section indicates that this path _can_ be depended on. The `project.depend` is where you specify zero or more of these paths your project _does_ depend on.

To trigger change detection, create a file in library1:

	touch group1/common/library1/foo.proto

and then `monorail inspect change | jq .`:

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
        	{
            "name": "group1/common/library1/foo.proto",
            "project": null,
            "action": "use",
            "reason": "project_depend_effect"
          },
          {
            "name": "group1/project1/foo.txt",
            "project": "group1/project1",
            "action": "use",
            "reason": "project_match"
          }
        ],
        "project": [
          "group1/project2",
          "group1/project1"
        ],
        "link": [],
        "depend": [
          "group1/common/library1"
        ]
      }
    }
  }
}
```

Our original file entry is still there, but another for the newly-created file has appeared. It has a `project` of `null` because it does not lie in the path of a project, and a `reason` that indicates it is being used due to a project depending on a path containing the file (`project_depend_effect`).

An entry of `group1/project2` has appeared in `group.project`, indicating that this project is now part of the set of projects that have changed. We didn't change any files in `project2` (indeed, none exist!), but did modify a path that `project2` depends on.

Furthermore, an entry has appeared in `group.depend` for our library path.


### Links

A `link` works similarly to a `depend`, but applies to all projects in a group without them opting-in. To demonstrate, we will create a third project and a contrived `Lockfile` to link all projects to:

	mkdir group1/project3
	touch group1/Lockfile

Execute the following to adjust the `[[group]]` section of `Monorail.toml` to specify this new project, as well as a group `link`:

```sh
cat <<EOF > Monorail.toml
[vcs]
use = "git"

[vcs.git]
trunk = "$(git branch --show-current)"

[extension] 
use = "bash"

[[group]]
path = "group1"
depend = [
	"common/library1"
]
link = [
	"Lockfile"
]

  [[group.project]]
  	path = "project1"

  [[group.project]]
  	path = "project2"

  	depend = [
  		"common/library1"
  	]
  [[group.project]]
  	path = "project3"

EOF
```

Executing `monocle inspect change | jq .` yields:

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [
        	{
            "name": "group1/Lockfile",
            "project": null,
            "action": "use",
            "reason": "group_link_effect"
          },
        	{
            "name": "group1/common/library1/foo.proto",
            "project": null,
            "action": "use",
            "reason": "project_depend_effect"
          },
          {
            "name": "group1/project1/foo.txt",
            "project": "group1/project1",
            "action": "use",
            "reason": "project_match"
          }
        ],
        "project": [
          "group1/project3",
          "group1/project2",
          "group1/project1"
        ],
        "link": [
          "group1/Lockfile"
        ],
        "depend": [
          "group1/common/library1"
        ]
      }
    }
  }
}
```

Again, our original changes to `project1` and `library1` remain. A new `change.file` for `Lockfile` has appeared, a new `change.project` for the `project3` has been added, and `change.link` now has a path to the `Lockfile` we changed.

Note that `project3` did not need to explicitly depend on `Lockfile`; simply being a member of `group1` does this.

## Defining commands

Commands are run by extensions, which are "runners" for user-defined code. We have already specified the following in `Monorail.toml`, so we will proceed with `monorail-bash`:

```
[extension]
use = "bash"
```

Commands are stored in a file on a per-target basis, the path to which is defined in `Monorail.toml`. In our case, that path will be `support/script/monorail-exec.sh` (the default) relative to `group1/project1`.

Create the path to this file with: 

	mkdir -p group1/project1/support/script

In the `group1/project1/support/script/monorail-exec.sh` file, we will define a script containing three commands:


```sh
cat <<"EOF" > group1/project1/support/script/monorail-exec.sh
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
EOF
```

Command names can be named any valid UTF-8 string, and are free to do anything a normal `bash` script can do: source other scripts, call external build tools, perform network requests, etc. One of the benefits of `monorail` is that it does not limit the build tooling you can use.

## Executing commands

With the command script defined, it can be called with `monorail-bash exec`. This can be done in one of two ways:

  * implicitly, from the change detection output of `monorail`
  * explicitly, by specifying a list of targets


### Implicit

When done implicitly, `monorail-bash exec` uses the same processes that power `monorail inspect change` to derive changed targets and execute commands against them. To illustrate this use the following (`SOME_EXTRA_VAR` just shows that parent shell values can be passed to commands): 

	SOME_EXTRA_VAR=foo monorail-bash -v exec -c command1 -c command2

```
Sep 10 07:34:07 monorail-bash : 'monorail' path:    monorail
Sep 10 07:34:07 monorail-bash : 'jq' path:          jq
Sep 10 07:34:07 monorail-bash : 'git' path:         git
Sep 10 07:34:07 monorail-bash : use libgit2 status: false
Sep 10 07:34:07 monorail-bash : 'monorail' config:  Monorail.toml
Sep 10 07:34:07 monorail-bash : working directory:  /Users/patrick/lab/github.com/pnordahl/monorail-tutorial
Sep 10 07:34:07 monorail-bash : command:            command1
Sep 10 07:34:07 monorail-bash : command:            command2
Sep 10 07:34:07 monorail-bash : start:              
Sep 10 07:34:07 monorail-bash : end:                
Sep 10 07:34:07 monorail-bash : target (inferred):             group1/Lockfile
Sep 10 07:34:07 monorail-bash : target (inferred):             group1/common/library1
Sep 10 07:34:07 monorail-bash : target (inferred):             group1/project2
Sep 10 07:34:07 monorail-bash : target (inferred):             group1/project3
Sep 10 07:34:07 monorail-bash : target (inferred):             group1/project1
Sep 10 07:34:07 monorail-bash : NOTE: Ignoring command for non-directory target; command: command1, target: group1/Lockfile
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command1, target: group1/common/library1
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command1, target: group1/project2
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command1, target: group1/project3
Sep 10 07:34:07 monorail-bash : Executing command; command: command1, target: group1/project1
Hello, from command1
The calling environment is inherited: foo
Sep 10 07:34:07 monorail-bash : NOTE: Ignoring command for non-directory target; command: command2, target: group1/Lockfile
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command2, target: group1/common/library1
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command2, target: group1/project2
Sep 10 07:34:07 monorail-bash : NOTE: Script not found; command: command2, target: group1/project3
Sep 10 07:34:07 monorail-bash : Executing command; command: command2, target: group1/project1
Hello, from command2
some data
```

The majority of this output is workflow and debugging information, but it's worth noting a few key pieces.

* commands were executed in the order specified by the `-c` options.
* paths we specified as `depend` and `link` entries could have specified their own implementations of the `command1` and `command2` commands
* paths that point to individual files cannot specify a command implementation (e.g. our Lockfile)
* paths without implementations for `command1` and/or `command2` were noted and ignored

Executing arbitrary bash functions against the changes detected by `monorail` has a number of applications, including:

  * executing commands against all projects/dependencies/links you've modified, without specifically targeting them; `monorail-bash` ensures that for each changed target, the requested commands are executed sequentially
  * running specific commands against all changed targets as part of CI (e.g. `check`, `build`, `test`, `deploy`, etc.)

### Explicit

Manually selecting targets gives one the ability to execute commands independent of VCS change detection. Applications include:

  * getting new developers up to speed working on a codebase, as one can define all setup code in a set of commands and execute it against the target, e.g. a `bootstrap` command
  * run any command for any target in the entire repo, as desired

To illustrate manually selecting targets, we will run the `setup` command we defined but did not execute previously. Execute the following (removing the `-v` to cut down on the visual noise):

	monorail-bash exec -t group1/project1 -c setup

```
Installing everything you need
```

For more information, execute `monorail-bash -h`, and `monorail-bash exec -h`.

## Releasing

`monorail` uses the backend VCS native mechanisms, e.g. tags in `git` as "release" markers. This creates a "checkpoint" for change detection. Without release tags, `monorail` is forced to search `git` history back to the first commit. This would be ineffecient and make change detection useless as all targets would be considered changed over a long enough timeline.

When a release is performed, it applies to all targets that were changed since the previous release (or the first commit of the repository, if no releases yet exist).

First, let's commit and push our current changes:

	git add * && git commit -am "update commands" && git push

Assuming that we have committed all that we intend to, and target commands have been run to our satisfaction (e.g. CI has passed for the merge of our branch), we can dry-run a patch release with: 

	monorail release --dry-run -t patch | jq .

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

`monorail` creates releases with an `id` appropriate to the conventions of the chosen VCS; in this case, that is the `git` semver tagging format. It also embeds the list of targets included as part of this release in the `targets` array; in the case of `git`, it will embed this list of targets in the release message.

Now, run a real release:

	monorail release -t patch | jq .

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

To show that the release cleared out `monorail`s view of changes, execute: 

	monorail inspect change | jq .

```json
{
  "group": {
    "group1": {
      "change": {
        "file": [],
        "project": [],
        "link": [],
        "depend": []
      }
    }
  }
}
```

Finally, our newly-pushed tag is now in the remote. To see this, execute:

	git -C ../monorail-tutorial-remote show -s --format=%B v0.0.1

... which outputs

```
tag v0.0.1
Tagger: you <email@domain.com>

group1/Lockfile
group1/common/library1
group1/project1
group1/project2
group1/project3
```

This concludes the tutorial. Now that you have seen how the core concepts of `monorail` and extensions work, you're ready to use it in real projects. Experiment with repository layouts, commands, CI, and working on a trunk-based development workflow that works for your teams.

Refer to `Monorail.reference.toml` for more specific information about the various configuration available for `monorail` and extensions.

## Invariants

In order to work, `monorail` requires the following invariants be satisfied:

  1. groups may not reference paths in other groups
  2. targets may not reference paths in other targets
  3. only paths specified in a group's `link`, or `depend` configuration may be shared between projects of that group

_If any of these are violated, the behavior of `monorail` commands that analyze changes to the repository is undefined._


# Development setup

This will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```

You can use `install.sh` to build a release binary of `monorail` and copy it, along with extensions, into your PATH.
