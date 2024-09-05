# Change Log

## [2.1.0] - 2024-09-05

### Changed

- '-d' working directory flag changed to '-w'
- Delete local git tag when a checkpoint fails to push
- Changed git tagging format from incrementing semver to incrementing `monorail-N`, N > 0
 
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