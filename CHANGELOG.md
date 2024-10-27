# Change Log

## [2.1.0] - 2024-09-05

### Changed

- '-d' working directory flag changed to '-w'
- Delete local git tag when a checkpoint fails to push
- Changed checkpoint id format from incrementing semver to incrementing `monorail-N`, N > 0
 
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

## [3.0.1] - 2024-10-26

### Changed

- Removed serialization of null fields in `monorail config show`


## [3.0.2] - 2024-10-27

### Added

- A RFC3339 timestamp to all success and error output structs

### Changed

- Verbose logging timestamp format adheres strictly to RFC3339


## [3.0.3] - 2024-10-27

### Added

- An optional `--id` (`-i`) can be provided to `checkpoint update` to provide and id to use, instead of inferring one from end of history

### Changed

- `checkpoint update` now always stores inferred references fully resolved instead of the HEAD alias

