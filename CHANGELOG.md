# Change Log

## [3.6.0] - 2024-12-01

### Changed
- Replaced `--argmap` JSON literals with `--argmaps`, which accepts a list of argmap names to load
- Replaced `--arg` with `--args` for consistency with other option pluralization
- Removed `--argmap-file` direct loading in favor of `--argmaps`
- Changed target `argmaps` configuration to align with new autoloading API
- Replaced `is_executable` boolean in `target show --commands` to `permissions` showing octal mode

## [3.5.7] - 2024-11-26

### Fixed
- Issue preventing symlinked directories from working with `checkpoint update --pending`
- (internal) Naming of change target reason `uses_target` to `uses ` for clarity

## [3.5.6] - 2024-11-23

### Changed
- (internal) Parallelized analyze

## [3.5.5] - 2024-11-21

### Added
- `server.log` config

## [3.5.4] - 2024-11-20

### Added
- `checkpointed` boolean on `run` and `analyze` output, indicating if the checkpoint was used

### Fixed
- `checkpoint update` run without args always forced a help message

### Changed
- `analyze` and `run` no longer query the change provider when a checkpoint does not exist
- `--changes` and `--change-targets` flags on `analyze` are disabled if no checkpoint is available to guide change detection
- (internal) `Cargo.lock` to git
- (internal) Removed `hex` dependency
- (internal) Removed `flume` dependency
- (internal) Changed resolver to "2"
- (internal) Update CI to use locked cargo commands


## [3.5.3] - 2024-11-19

### Added
- `config generate` API for generating a configuration file that is synchronized with its source file
- Initial implementation of lock server, preventing parallel monorail use for a subset of APIs

## [3.5.2] - 2024-11-15

### Added
- `target render` API for creating a visual representation of the target graph

## [3.5.1] - 2024-11-14

### Changed
- Removed `--target (-t)` flag from `monorail analyze`

### Fixed
-Issue preventing `monorail run` from returning non-zero exit codes

## [3.5.0] - 2024-11-13

### Changed
- Default commands directory from `monorail` to `monorail/cmd`

### Added
- Optional per-target base argmaps
- Default argmaps directory of `monorail/argmap`

### Changed

## [3.4.1] - 2024-11-12

### Changed
- `--arg, --arg-map, and --arg-map` may all be specified simultaneously, and are processed with a precendece order
- Support for multiple `--arg-map` and `--arg-map-file` arguments and merging of them

### Removed

- `command.definitions.args` has been removed in favor of multiple arg-map/arg-map-file support

## [3.3.1] - 2024-11-11

### Changed
- Configuration `output_dir` changed to `out_dir` for consistency

### Fixed
- Graph cycle detection when specific targets are specified to `analyze` and `run`


## [3.3.0] - 2024-11-08

### Changed

- Omit null fields in serialized run results structure
- Adjusted run results structure to reduce duplication and improve logical grouping
- The `invocation_args` field in the run results structure is now `invocation`

## [3.2.1] - 2024-11-08

### Added

- `--arg`, `--arg-map`, and `--arg-map-file` arguments to `monorail run` for providing runtime arguments to commands

### Changed

- `--fail-on-undefined` now always defaults to `false`, instead of `true` when one or more targets is supplied to `monorail run`

### Fixed

- Improved error clarity for configuration file-related issues

## [3.2.0] - 2024-11-02

### Changed

- Disallow unknown fields in `Monorail.json`
- Allow target file contains check to traverse subtree

## [3.2.0] - 2024-11-02

### Changed

- The `commands.path` field now always defaults to `monorail`, relative to the target path
- The `commands.definitions.<command_name>.path` field is now interpreted relative to the repository root
- The `commands.definitions.<command_name>.args` field now applies to mapped definitions and discovered commands

## [3.1.1] - 2024-11-02

### Fixed

- Fixes staged changes not appearing in changesets when a checkpoint is not present (@neopug)

## [3.1.0] - 2024-10-31

### Added

- `--commands` flag for `monorail target show` for displaying available commands for targets
- Command sequence support and the `--sequences` (`-s`) flag for `monorail run`

### Changed

- The `--command` argument for `run` and `log show` has changed to `--commands` for consistency
- The `--target` argument for `run`, `analyze`, and `log show` has changed to `--targets` for consistency
- The `--start` (`-s`) argument for `run` and `analyze` has changed to `--begin` (`-b`)

## [3.0.6] - 2024-10-30

### Changed

- `monorail log show` and `monorail log tail` now return an error if neither --stdout nor --stderr are provided

## [3.0.4] - 2024-10-28

### Added

- `monorail out delete --all` for purging all run, result, log, and tracking data

## [3.0.3] - 2024-10-27

### Added

- An optional `--id` (`-i`) can be provided to `checkpoint update` to provide and id to use, instead of inferring one from end of history

### Changed

- `checkpoint update` now always stores inferred references fully resolved instead of the HEAD alias

## [3.0.2] - 2024-10-27

### Added

- A RFC3339 timestamp to all success and error output structs

### Changed

- Verbose logging timestamp format adheres strictly to RFC3339

## [3.0.1] - 2024-10-26

### Changed

- Removed serialization of null fields in `monorail config show`

## [3.0.0] - 2024-10-26

Dependency graph, parallel execution, replaced extensions with `monorail run`, and checkpoint rework.

### Added

- Replaced `monorail-bash` for running user-defined commands with `monorail run`
- Generate a target DAG from target `uses` definitions
- Parallel execution of user defined executables guided by the target DAG
- Tracking table for storing the change detection checkpoint and for future internal use
- Collection of compressed logs and results
- Real-time log streaming

### Changed

- Replaced tag-based checkpoints with a universal checkpoint file
- Replaced `monorail checkpoint create` with `monorail checkpoint update`
- Removed `git.trunk`, `git.remote`, and `git.tags_refspec_prefix` configuration
- Removed `extension` configuration
- Removed `libgit` integration and `use_libgit2_status` argument

## [2.0.0] - 2024-09-03

A large internal refactor to improve speed and flexibility.

### Added

### Changed

- Replaced `inspect change` command with `analyze`
- Replaced `release` command with `checkpoint create`
- Updated tutorial and documentation to reflect changes
- Removed concept of `group` and `project`, replaced with `target`
- Replaced `depend` with `uses`, and is no longer required to be pre-declared on a top level target (previously, `depend` entries had to be declared on the parent group)
- Multiple `Monorail.toml` fields now have defaults: `vcs.use = "git"`, `vcs.git.trunk = "master"`, `vcs.git.remote = "origin"`, and `extension.use`
- Changed checkpoint tag messages from plain text to JSON, and added change count

### Fixed
- Minor tag message formatting issue when using git tags for checkpointing

## [2.1.0] - 2024-09-05

### Changed

- '-d' working directory flag changed to '-w'
- Delete local git tag when a checkpoint fails to push
- Changed checkpoint id format from incrementing semver to incrementing `monorail-N`, N > 0
