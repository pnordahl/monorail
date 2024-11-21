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
	// Source information, for use with `config generate`.
	"source": {
		/*
		Path to the source file that was used to generate this file.
		If provided, this configuration will be validated against the
		source for integrity.

		Required, if this configuration was generated. default: ""
		*/
		"path": "path/within/repository",
		/*
		The algorithm used to checksum the source and generated files.

		Default: null; restricted for internal use only
		*/
		"algorithm": null,
		/*
		The checksum of the source file, used for integrity checks.

		Default: null; restricted for internal use only
		*/
		"checksum": null
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
			Configuration and overrides for argument mappings.
			*/
			"argmaps": {
				/*
				Default location within this target path containing argmap 
				definition files. This is used when locating argmaps for this
				target automatically when it is changed.

				Optional, default: "monorail/argmap"
				*/
				"path": "path/within/this/target",

				/*
				Path to the default argmap to load for this target. This argmap
				is loaded prior to any arguments supplied by use of 
				--arg, --target-argmap, and --target-argmap-file switches on `monorail run`.

				Optional, default: "base.json"
				*/
				"base": "path/within/target/argmaps/path"
			}


			/*
			Configuration and overrides for this targets commands.
			Optional.
			*/
			"commands": {
				/*
				Default location within this target path containing command
				executables. This is used when locating executables for a
				command when no definition path for that command is specified.

				Optional, default: "monorail/cmd"
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
						"path": "path/within/repository"
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
	},
	"server": {
		// Log server for command output streams
		"log": {
			/*
			Host to bind.

			Optional: default: "127.0.0.1"
			*/
			"host": "127.0.0.1",
			/*
			Port to bind.

			Optional: default: 5917
			*/
			"port": 5918,
			/*
			Milliseconds to wait for a successful bind of the host:port.

			Optional: default: 1000
			*/
			"bind_timeout_ms": 1000,
		},
		// Lock server used for concurrency control of a subset of monorail APIs.
		"lock": {
			/*
			Host to bind.

			Optional: default: "127.0.0.1"
			*/
			"host": "127.0.0.1",
			/*
			Port to bind.

			Optional: default: 5917
			*/
			"port": 5917,
			/*
			Milliseconds to wait for a successful bind of the host:port.

			Optional: default: 1000
			*/
			"bind_timeout_ms": 1000,
		}
	}
}