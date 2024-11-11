// This is a JavaScript representation of the JSON configuration file
// used for customizing monorail, due to the need for comments and JSON
// not supporting them.

const reference = {
	// Directory to place logs, caches, and other runtime outputs.
	// Optional, default: "monorail-out"
	"out_dir": "monorail-out",

	// Number of historical run results and logs to retain on disk.
	// Optional, default: 10
	"max_retained_runs": 10,

	// Change provider configuration.
	"change_provider": {
		// Which change provider to use.
		// Optional, default: "git"
		"use": "git",
	},
	// An array of targets to configure.
	"targets": [
		{
			/*
			The path for this target. The value is relative to the 
			repository root. Duplicate paths are not allowed.

			Required.
			*/
			"path": "path/within/repository",
			/*
			Array of paths that this target depends on. These paths 
			are relative to the repository root. Paths provided are
			interpreted in one of two ways:

			1. A path that lies within another targets path will create 
			a dependency between that target and this target.

			2. A path that does not lie within another targets path will
			be treated as if it were part of this targets path.

			Optional, default: []
			*/
			"uses": [
				"path/within/repository"
			],

			/*
			Array of paths that will not affect this target. These
			paths are relative to the repository root. Paths provided
			have the highest priority when determining if a change path
			affects this target.

			If a directory is provided, all files within that directory
			will be ignoreed.

			Optional, default: []
			*/
			"ignores": [
				"path/within/repository"
			],

			/*
			Configuration and overrides for this targets commands.
			Optional.
			*/
			"commands": {
				/*
				Default location within this target path containing command
				executables. This is used when locating executables for a
				command when no definition path for that command is specified.

				Optional, default: "monorail"
				*/
				"path": "path/within/this/target"

				/*
				Overrides for this targets command executables and
				their arguments.

				Optional, default: {}
				*/
				"definitions": {
					"<command_name>": {
						// Path to an executable to use for this command.
						// Optional. default: <commands.path>/<command_name>.<extension>,
						// where <extension> is discovered on the filesystem.
						"path": "path/within/repository",
						// Array of flags, switches, and arguments to
						// provide when running the command executable.
						// Optional, default: []
						"args": []
					}
				}
			}
		}
	],
	// Mapping of sequence names to a list of commands. When used with `run`,
	// a sequence is expanded into the list of commands it is mapped to.
	"sequences": {
		"<sequence name>": [
			"<command name 1>",
			"<command name 2>",
			"<command name 3>"
		]
	}
}