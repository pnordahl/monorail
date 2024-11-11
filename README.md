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

- [Config](#config)
- [Checkpoint](#checkpoint)
- [Target](#target)
- [Run](#run)
- [Result](#result)
- [Log](#log)
- [Analyze](#analyze)
- [Out](#out)


### Config

`monorail` uses a JSON configuration file that describes the repository target mapping, target dependencies, and various other options. This file is fully documented in the `Monorail.reference.js` file at the root of this repository. This file is by default `Monorail.json` in the root of your repository, and while this location and name can be customized, all subsequent documentation will refer to it by this name.

#### `config show`

Parses the configuration file and displays it, including defaults for fields not specified in the file.

### Target

A `target` is a core abstraction of `monorail`. High-level, it is simply a label for a filesystem path. You choose which paths in your repository to operate over, and assign each of those those paths a target in `Monorail.json`. When files are added, modified, or deleted within a target, the target will be considered "changed".

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

#### Nesting targets

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

#### Target dependencies

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

Will return an error in any command that uses the target graph.

#### Commands

#### `target show `

Display configured targets, target groupings, and target commands.


### Checkpoint

### Run
### Result
### Log
### Analyze
### Out

# Development setup

`monorail` is written in Rust, so working on it is straightforward; this will build the project and run the tests:

```sh
cargo build
cargo test -- --nocapture
```
