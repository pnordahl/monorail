# Tutorial

To illustrate multi-language command executables, ensure that `awk` and `python3` are installed. Additionally, you may want `jq` installed to pretty-print the output from various steps in this tutorial. If you don't want to install it, you can just omit any `| jq` that you find.

In this tutorial, you'll learn about:

  * mapping repository paths
  * analyzing changes
  * defining and executing commands
  * logging
  * checkpointing

## One-time setup

First, create a fresh `git` repository in your current directory:

```sh
git init monorail-tutorial
cd monorail-tutorial
git commit -m x --allow-empty
echo 'monorail-out' > .gitignore
```

_NOTE_: the commit is to create a valid HEAD reference, which is needed in a later part of the tutorial.

First, we will set up some toy projects to help get a feel for using `monorail`. Since `rust` is already installed, we'll use that in addition to `python` (which you likely also have installed; if not, go ahead and do so). The third project will not include any real code, because it's just going to illustrate how shared dependencies are considered during execution of commands. Finally, each will get an empty `monorail.sh` file that we will add code to as the tutorial proceeds.

### Initial configuration and mapping tagets

We will soon fill out our repository with some toy projects, but before we do, let's create a `Monorail.json` file that describes this structure in terms `monorail` understands. This command will create that file in the root of the repository, and simply map top level paths (which we will create in a moment) to targets:

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

Run the following, to show our config and ensure our input is well-formed:

```sh
monorail config show | jq
```
```json
{
  "timestamp": "2024-11-10T10:32:21.846108+00:00",
  "output_dir": "monorail-out",
  "max_retained_runs": 10,
  "change_provider": {
    "use": "git"
  },
  "targets": [
    {
      "path": "rust",
      "commands": {
        "path": "monorail"
      }
    },
    {
      "path": "python/app3",
      "commands": {
        "path": "monorail"
      }
    },
    {
      "path": "proto",
      "commands": {
        "path": "monorail"
      }
    }
  ],
  "log": {
    "flush_interval_ms": 500
  }
}
```

This output includes some default values for things not specified, but otherwise reflects what we have entered. An additional note about the location of the `Monorail.json` file; you can specify an absolute path with `-c`, e.g. `monorail -c </path/to/your/config/file>`, and this will be used instead of the default (`$(pwd)/Monorail.json`). All of `monorail`s commands are executed, and internal tracking files and logs stored, _relative to this path_.

Before we continue, let's create an initial `checkpoint`. The checkpoint will be described in more detail in a later section of this tutorial, but think of it as a "marker" for the beginning of a change detection window. For now, just run the command:

```sh
monorail checkpoint update
```
```json
{
  "timestamp": "2024-11-10T10:32:29.961560+00:00",
  "checkpoint": {
    "id": "4b5c5a4ce18a05b0175c1db6a14fb69bf1ca30d3",
    "pending": null
  }
}
```

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

## Preview: running commands

Commands will be covered in more depth later in the tutorial (along with logging), but now that we have a valid `Monorail.json` we can execute a command and view logs right away. Run the following to create an executable (in this case, a `bash` script) for the `rust` target:

```sh
cat <<EOF > rust/monorail/hello.sh
#!/bin/bash

echo 'Hello, world!'
echo 'An error message' >&2 
EOF
chmod +x rust/monorail/hello.sh
```

Now execute it:

```sh
monorail run -c hello -t rust | jq
```

```json
{
  "timestamp": "2024-11-10T10:32:57.434933+00:00",
  "failed": false,
  "invocation": "run -c hello -t rust",
  "out": {
    "run": {
      "path": "/private/tmp/monorail-tutorial/monorail-out/run/1",
      "files": {
        "stdout": "stdout.zst",
        "stderr": "stderr.zst"
      },
      "targets": {
        "rust": "521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3"
      }
    }
  },
  "results": [
    {
      "command": "hello",
      "target_groups": [
        {
          "rust": {
            "status": "success",
            "code": 0,
            "runtime_secs": 0.13038129
          }
        }
      ]
    }
  ]
}
```

This output is stored and queryable with `monorail result show`. In addition, you can view logs, both historically and by tailing (shown in more detail later). Show the logs for the most recent `run`:

```sh
monorail log show --stdout --stderr
```
```
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
```

In this example, we just used `monorail` as a simple command runner like `make` or `just`. However, unlike these other tools `monorail` is capable of running commands in parallel based on your dependency graph, streaming logs, collecting historical results and compressed logs, and more.

Before we explore commands and logging in more detail, it's important to understand how `monorail` detects changes. In the next section we'll use the `analyze` API to do so.

## Analyze

`monorail` integrates with a `change_provider` to obtain a view of filesystem changes, which are processed along with a graph built from the specification in `Monorail.json`. Display an analysis of this changeset and graph with:

```sh
monorail analyze | jq
```
```json
{
  "timestamp": "2024-11-10T10:33:40.195249+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

This indicates that based on our current changeset and graph, all three targets have changed. Display more information about the specific changes causing these targets to appear by adding `--change-targets`:
```sh
monorail analyze --changes | jq
```
```json
{
  "timestamp": "2024-11-10T10:35:22.595337+00:00",
  "changes": ["... hundreds of entries ..."],
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ]
}
```

Unfortunately, hundreds of undesired virtualenv files are in our changes array. In the next section, we'll rectify this by using our change providers native mechanisms.

### Ignoring with .gitignore

As mentioned, `monorail` defers change detection to a provider. Since our change provider is `git`, we can exclude these files by adding them to `.gitignore`:

```sh
echo 'python/app3/venv' >> .gitignore
```

Re-running the commnand, notice that all of the offending files are gone:
```sh
monorail analyze --changes | jq
```
```json
{
  "timestamp": "2024-10-27T11:14:35.901180+00:00",
  "changes": [
    {
      "path": ".gitignore"
    },
    {
      "path": "Monorail.json"
    },
    {
      "path": "proto/README.md"
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
    },
    {
      "path": "rust/monorail/hello.sh",
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

Run the analyze again, but as a preview for the next section add the `--target-groups` flag. Note that `proto` no longer appears in the list of changed targets, and a new array of arrays has appeared:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-11-10T10:36:09.013839+00:00",
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

The `target_groups` array is built by constructing a graph from the dependencies specified in `Monorail.json`. This is used to control command execution, ensuring that maximum parallelism is achieved while respecting the hierarchy of dependencies at any given time. In the next section, we will establish a simple dependency graph between our targets.

## Dependencies

Specifying dependencies between targets is done with the `uses` field on a `target` in `Monorail.json`. Run the following to set up a simple graph (and create a file so our `proto` project is visible again, since we ignored `README.md`):

```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust", "uses": ["proto"] },
    { "path": "python/app3", "uses": ["proto"] },
    { "path": "proto", "ignores": [ "proto/README.md" ] }
  ]
}
EOF
touch proto/LICENSE.md
```

This has created a dependency on `proto` for `rust` and `python/app3` targets, connecting them for change detection and command execution. Observe the results of analyze now:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-11-10T10:36:34.260394+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ],
  "target_groups": [
    [
      "proto"
    ],
    [
      "rust",
      "python/app3"
    ]
  ]
}
```

Semantically, each element of `target_groups` is an array of targets that can be considered "independent" of subsequent elements. Since `proto` does not depend on any other targets, it stands alone and prior to `rust` and `python/app3`, which depend on `proto`. As it pertains to command execution, each target found within a group is executed in parallel, and when successful `monorail` will move on to the next array and do the same. For change detection, changes to dependencies of a target will cause that target to be considered changed as well; i.e. a "what depends on me" graph traversal. In the final sections, we will cover logging and parallel command execution with practical examples.

Finally, keep in mind that you can specify non-target paths in `uses` and those paths will now be considered part of that target. In general, this is not commonly used but remains a convenient way to link targets to paths they would otherwise not be.

## Logging and Results

Command output capture and result records are core responsibilities of a build tool like `monorail`, and so it provides a set of APIs for obtaining all data generated by `monorail run`. During command execution, all stdout and stderr is captured and stored in compressed archives, along with the overall results of the run. The number of historical runs retained is controlled by `max_retained_runs` in `Monorail.json`, defaulting to 10.

### Historical

We have already run a command, so let's show the result of that most recent run:

```sh
monorail result show | jq
```
```json
{
  "timestamp": "2024-11-10T10:36:49.883069+00:00",
  "failed": false,
  "invocation": "run -c hello -t rust",
  "out": {
    "run": {
      "path": "/private/tmp/monorail-tutorial/monorail-out/run/1",
      "files": {
        "result": "result.json.zst",
        "stdout": "stdout.zst",
        "stderr": "stderr.zst"
      },
      "targets": {
        "rust": "521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3"
      }
    }
  },
  "results": [
    {
      "command": "hello",
      "target_groups": [
        {
          "rust": {
            "status": "success",
            "code": 0,
            "runtime_secs": 0.13038129
          }
        }
      ]
    }
  ]
}
```

This output contains information about the last run, such as if the run `failed`, various metadata about the location of artifacts on disk, and a breakdown by command how each target group's execution proceeded.

`monorail` provides APIs for accessing the logs and results of runs, but if you have a use case necessitating external tools you can process this structured output. For example, this extracts all of the log files for the last run and decompresses them with `zstd` (note that if you run this example, you'll need it installed):

```sh
monorail result show | jq -r '
  .out as $out |
  .results[] |
  .command as $command |
  .target_groups[] |
  to_entries[] | 
  (
    [$out.run.path, $command, $out.run.targets[.key]] as $base_path |
    {
      stdout: ($base_path + [$out.run.files.stdout]) | join("/"),
      stderr: ($base_path + [$out.run.files.stderr]) | join("/")
    }
  ) | .stdout, .stderr
' | xargs -I {} zstd -dc {}
```
```
An error message
Hello, world!
```

However, as mentioned `monorail` provides a convenient API (which we already saw above) for doing so in a way that makes viewing multiple logs easier to read:

```sh
monorail log show --stderr --stdout
```
```
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
```

When queried through `monorail`, blocks of related logs begin with a header of format `[monorail | {{file name}} | {{target}} | {{command}}]`. Historical logs are decompressed and emitted serially to stdout, so only one of these will be present in each file. However, when tailing logs for multiple commands and targets these headers are regularly injected as blocks of logs from each task are emitted. In the next section, we will update our existing command to demonstrate log tailing.

### Tailing

It's often useful to observe how a `monorail run` invocation is progressing; to do so, we will start a log streaming tail process in a separate shell (you may also want to do this in a separate terminal window, so you can see the logs as you run commands):

```sh
monorail log tail --stderr --stdout
```

This has started a server that will receive logs. For the rest of this tutorial, leave that running in a separate window. Now, let's update our existing command to demonstrate tailing:

```sh
cat <<EOF > rust/monorail/hello.sh
#!/bin/bash

for ((i=0; i<20; i++)); do
    echo 'Hello, world!'
    sleep 0.02
done &

for ((i=0; i<10; i++)); do
    echo 'An error message' >&2
    sleep 0.04
done

wait
EOF
```

This will run for a moment, printing output from stdout and stderr for us to observe. Now, run the command again with and additional flag `-v` to print useful workflow statements (`-vv` and `-vvv` print even more), and ensure that your log tailing window is in view:

```sh
monorail -v run -c hello -t rust
```
As this command executes, you'll see internal progress due to the inclusion of the `-v` flag:

```json
{"timestamp":"2024-11-10T11:16:48.802518+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-11-10T11:16:48.801236+00:00","level":"INFO","message":"processing plan"}
{"timestamp":"2024-11-10T11:16:48.802296+00:00","level":"INFO","message":"processing commands","num":1}
{"timestamp":"2024-11-10T11:16:48.802605+00:00","level":"INFO","message":"processing target groups","num":1,"command":"hello"}
{"timestamp":"2024-11-10T11:16:48.802646+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-11-10T11:16:48.802652+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:16:49.372712+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:16:49.373482+00:00","failed":false,"invocation":"-v run -c hello -t rust","out":{"run":{"path":"/private/tmp/monorail-tutorial/monorail-out/run/3","files":{"result":"result.json.zst","stdout":"stdout.zst","stderr":"stderr.zst"},"targets":{"rust":"521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3"}}},"results":[{"command":"hello","target_groups":[{"rust":{"status":"success","code":0,"runtime_secs":0.57000625}}]}]
```

... as well as log statements from the command itself in your log tailing window:

```
[monorail | stdout.zst, stderr.zst | (any target) | (any command)]
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
[monorail | stderr.zst | rust | hello]
An error message
An error message
An error message
An error message
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
[monorail | stderr.zst | rust | hello]
An error message
An error message
An error message
An error message
[monorail | stderr.zst | rust | hello]
An error message
[monorail | stdout.zst | rust | hello]
Hello, world!
Hello, world!
Hello, world!
Hello, world!
Hello, world!
```

A few things are worth noting in this output.

First, is the log stream header:
```
[monorail | stdout.zst, stderr.zst | (any target) | (any command)]
```

Every `monorail run` will print this header once at the beginning of a new log stream, indicating what data will be shown in the stream. When you start the tail process, by default it will not filter any targets or commands; this is indicated by the `(any target)` and `(any command)` entries in the header. If we had provided `--targets rust proto` and `--commands hello test build` (which can also be provided to `monorail log show`), then the header would look like `[monorail | stdout.zst, stderr.zst | rust, proto | hello, test, build]`, and only those logs that match these filters would appear.

Second, note how multiple log block headers (e.g. `[monorail | stderr.zst | rust | hello]`) are printed; this is because the output from multiple files is independently collected and flushed at regular intervals and interleaved in the stream. In practice, you will often use target and command filters to reduce noise, but even without them block headers make it possible to visually parse the combined log stream.

## Commands

Commands are the way `monorail` executes your code against targets. Each target implements a command with a unique name, e.g. `test`, `build`, etc. as an executable file, and that file is executed and monitored as a subprocess of `monorail`. Depending on the graph defined by `Monorail.json`, these commands may be executed in parallel when it's safe to do so. For convenience, all commands are executed relative to the target path, e.g. for a target with path `rust`, the following code runs within the `rust` directory:

```sh
#!/bin/bash

cargo test -- --nocapture
```

### Defining a command

By default, `monorail` will use the `commands_path` (default: a `monorail` directory in the target path) field of a target as a search path for commands, and by default look for a file with a stem of `{{command}}`, e.g. `{{command}}.sh`. The command we defined earlier in the tutorial, `rust/monorail/hello.sh`, used these defaults; While customizing these defaults is possible via `Monorail.json`, it's not necessary for this tutorial. Let's define two new executables, this time in Python and Awk:

```sh
cat <<EOF > python/app3/monorail/hello.py
#!venv/bin/python3
import sys

print("Hello, from python/app3 and virtualenv python!")
print("An error occurred", file=sys.stderr)
EOF
chmod +x python/app3/monorail/hello.py
```

NOTE: While we're able to use the venv python3 executable directly in this trivial way, we wouldn't be able to access things that are installed in the environment without first activating the venv in a shell. So, in practice Python projects generally require a command written in a shell script that activates the env and then calls the script.

```sh
cat <<EOF > proto/monorail/hello.awk
#!/usr/bin/awk -f

BEGIN {
    print "Hello, from proto and awk!"
}
EOF
chmod +x proto/monorail/hello.awk
```

As mentioned earlier, commands can be written in any language, and need only be executable. We're using hashbangs to avoid cluttering the tutorial with compilation steps, but commands could be compiled to machine code, stored as something like `hello`, and executed just the same (though this approach wouldn't be portable across OS/architectures). Before we run this command, let's look at the output of analyze:

```sh
monorail analyze --target-groups | jq
```
```json
{
  "timestamp": "2024-11-10T11:17:59.095431+00:00",
  "targets": [
    "proto",
    "python/app3",
    "rust"
  ],
  "target_groups": [
    [
      "proto"
    ],
    [
      "rust",
      "python/app3"
    ]
  ]
}
```

In the next section, we will see how the target graph guides parallel command execution.

### Running commands

We have already seen an example of explictly choosing target(s) to run commands for with `monorail run -c hello -t rust`, but `monorail` is also capable of executing commands based on a dependency graph-guided analysis of changes, automatically. To do this, simply leave off a list of targets for the `run`:

```sh
monorail -v run -c hello
```
```json
{"timestamp":"2024-11-10T11:21:10.118903+00:00","level":"INFO","message":"processing plan"}
{"timestamp":"2024-11-10T11:21:10.119183+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-11-10T11:21:10.119373+00:00","level":"INFO","message":"processing commands","num":1}
{"timestamp":"2024-11-10T11:21:10.119383+00:00","level":"INFO","message":"processing target groups","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:21:10.119410+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-11-10T11:21:10.119419+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:21:10.121374+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:21:10.121587+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:21:10.121602+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:21:10.121846+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:21:10.144093+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:21:10.685876+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:21:10.687816+00:00","failed":false,"invocation":"-v run -c hello","out":{"run":{"path":"/private/tmp/monorail-tutorial/monorail-out/run/6","files":{"result":"result.json.zst","stdout":"stdout.zst","stderr":"stderr.zst"},"targets":{"python/app3":"585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7","proto":"1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6","rust":"521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3"}}},"results":[{"command":"hello","target_groups":[{"proto":{"status":"success","code":0,"runtime_secs":0.001930458}},{"python/app3":{"status":"success","code":0,"runtime_secs":0.02222046},"rust":{"status":"success","code":0,"runtime_secs":0.56421417}}]}]}
```

In this sequence of events: the `hello` command is scheduled and executed to completion for `proto` prior to the same for `python/app3` and `rust`. This is because both of the latter depend on `proto`. A practical scenario where this is relevant is building protobuf files for use by both `python/app3` and `rust`. By encoding this dependency in `Monorail.json`, we have ensured that when protobuf files in `proto` change, we have definitely compiled them by the time we execute commands for `python/app3` and `rust`.

You can also execute multiple commands. Each command in `-t <command1> <command2> ... <commandN>` is executed in the order listed, serially; the parallelism of `run` occurs within a command, at the target group level. Add a non-existent command to the list and run again:

```sh
monorail -v run -c hello build
```
```json
{"timestamp":"2024-11-10T11:21:40.781055+00:00","level":"INFO","message":"processing plan"}
{"timestamp":"2024-11-10T11:21:40.781333+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-11-10T11:21:40.781496+00:00","level":"INFO","message":"processing commands","num":2}
{"timestamp":"2024-11-10T11:21:40.781502+00:00","level":"INFO","message":"processing target groups","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:21:40.781529+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-11-10T11:21:40.781542+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:21:40.783250+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:21:40.783462+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:21:40.783472+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:21:40.783680+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:21:40.805540+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:21:41.343898+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:21:41.344924+00:00","level":"INFO","message":"processing target groups","num":2,"command":"build"}
{"timestamp":"2024-11-10T11:21:41.345047+00:00","level":"INFO","message":"processing targets","num":1,"command":"build"}
{"timestamp":"2024-11-10T11:21:41.345066+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"proto"}
{"timestamp":"2024-11-10T11:21:41.345703+00:00","level":"INFO","message":"processing targets","num":2,"command":"build"}
{"timestamp":"2024-11-10T11:21:41.345719+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"rust"}
{"timestamp":"2024-11-10T11:21:41.345729+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"python/app3"}
{"timestamp":"2024-11-10T11:21:41.347199+00:00","failed":false,"invocation":"-v run -c hello build","out":{"run":{"path":"/private/tmp/monorail-tutorial/monorail-out/run/7","files":{"result":"result.json.zst","stdout":"stdout.zst","stderr":"stderr.zst"},"targets":{"rust":"521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3","python/app3":"585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7","proto":"1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6"}}},"results":[{"command":"hello","target_groups":[{"proto":{"status":"success","code":0,"runtime_secs":0.001693833}},{"python/app3":{"status":"success","code":0,"runtime_secs":0.02183325},"rust":{"status":"success","code":0,"runtime_secs":0.5603645}}]},{"command":"build","target_groups":[{"proto":{"status":"undefined"}},{"python/app3":{"status":"undefined"},"rust":{"status":"undefined"}}]}]}
```

You might notice the exit code of 0 and `"failed":false`, and that's because by default it is not required for a target to define a command. You can override this behavior with `--fail-on-undefined`, but in general this allows targets to define only the commands they need and eliminates the need for "stubs" that may never be implemented.

### Running multiple commands with sequences

While any number of commands may be specified with `-c`, you can define `sequences` of commands in your configuration file for convenience and consistency. Edit the configuration file to create a `"dev"` sequence:
```sh
cat <<EOF > Monorail.json
{
  "targets": [
    { "path": "rust", "uses": ["proto"] },
    { "path": "python/app3", "uses": ["proto"] },
    { "path": "proto", "ignores": [ "proto/README.md" ] }
  ],
  "sequences": { "dev": ["hello", "build"] }
}
EOF
```

Now run the sequence with `--sequences` or `-s`:
```sh
monorail -v run -s dev
```
```json
{"timestamp":"2024-11-10T11:23:30.657032+00:00","level":"INFO","message":"processing plan"}
{"timestamp":"2024-11-10T11:23:30.657324+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-11-10T11:23:30.657508+00:00","level":"INFO","message":"processing commands","num":2}
{"timestamp":"2024-11-10T11:23:30.657515+00:00","level":"INFO","message":"processing target groups","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:23:30.657544+00:00","level":"INFO","message":"processing targets","num":1,"command":"hello"}
{"timestamp":"2024-11-10T11:23:30.657554+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:23:30.659575+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"proto"}
{"timestamp":"2024-11-10T11:23:30.659799+00:00","level":"INFO","message":"processing targets","num":2,"command":"hello"}
{"timestamp":"2024-11-10T11:23:30.659810+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:23:30.660046+00:00","level":"INFO","message":"task","status":"scheduled","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:23:30.683126+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"python/app3"}
{"timestamp":"2024-11-10T11:23:31.229140+00:00","level":"INFO","message":"task","status":"success","command":"hello","target":"rust"}
{"timestamp":"2024-11-10T11:23:31.230339+00:00","level":"INFO","message":"processing target groups","num":2,"command":"build"}
{"timestamp":"2024-11-10T11:23:31.230440+00:00","level":"INFO","message":"processing targets","num":1,"command":"build"}
{"timestamp":"2024-11-10T11:23:31.230459+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"proto"}
{"timestamp":"2024-11-10T11:23:31.231144+00:00","level":"INFO","message":"processing targets","num":2,"command":"build"}
{"timestamp":"2024-11-10T11:23:31.231158+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"rust"}
{"timestamp":"2024-11-10T11:23:31.231171+00:00","level":"INFO","message":"task","status":"undefined","command":"build","target":"python/app3"}
{"timestamp":"2024-11-10T11:23:31.232863+00:00","failed":false,"invocation":"-v run -s dev","out":{"run":{"path":"/private/tmp/monorail-tutorial/monorail-out/run/8","files":{"result":"result.json.zst","stdout":"stdout.zst","stderr":"stderr.zst"},"targets":{"proto":"1cafa6d851c65817d04c841673d025dcf4ed498435407058d3a36608d17e32b6","rust":"521fe5c9ece1aa1f8b66228171598263574aefc6fa4ba06a61747ec81ee9f5a3","python/app3":"585b3a9bcac009158d3e5df009aab9e31ab98ee466a2e818a8753736aefdfda7"}}},"results":[{"command":"hello","target_groups":[{"proto":{"status":"success","code":0,"runtime_secs":0.00200725}},{"python/app3":{"status":"success","code":0,"runtime_secs":0.023067666},"rust":{"status":"success","code":0,"runtime_secs":0.56926215}}]},{"command":"build","target_groups":[{"proto":{"status":"undefined"}},{"rust":{"status":"undefined"},"python/app3":{"status":"undefined"}}]}]}
```

When you run a sequence, it is first expanded into the commands it maps to. Then, any commands provided with `--commands` or `-c` are added to this list. For example, `monorail run -s dev -c foo bar` is equivalent to `monorail run -c hello build foo bar`. Sequences are useful for defining commands for use in specific contexts, such as setting up a repository for a new user, general development, and CI/CD.

### Viewing available commands

Lastly, you can query targets and include information about the commands that are available for each target. The `commands` table is built by merging executables found on the filesystem with any definitions in your configuration file:

```sh
monorail target show --commands | jq
```
```json
{
  "timestamp": "2024-11-10T11:23:48.655393+00:00",
  "targets": [
    {
      "path": "rust",
      "uses": [
        "proto"
      ],
      "commands": {
        "hello": {
          "name": "hello",
          "path": "/private/tmp/monorail-tutorial/rust/monorail/hello.sh",
          "args": null,
          "is_executable": true
        }
      }
    },
    {
      "path": "python/app3",
      "uses": [
        "proto"
      ],
      "commands": {
        "hello": {
          "name": "hello",
          "path": "/private/tmp/monorail-tutorial/python/app3/monorail/hello.py",
          "args": null,
          "is_executable": true
        }
      }
    },
    {
      "path": "proto",
      "ignores": [
        "proto/README.md"
      ],
      "commands": {
        "hello": {
          "name": "hello",
          "path": "/private/tmp/monorail-tutorial/proto/monorail/hello.awk",
          "args": null,
          "is_executable": true
        }
      }
    }
  ]
}
```

In the final section of this tutorial, we will manipulate the changes being used for `analyze` and guided `run` with the `checkpoint`.

## Checkpoint

The `checkpoint` is a marker in change provider history. When present, it is used as the beginning of an interval in history used for collecting a set of changes. The end of this interval is the latest point in change provider history. In addition, any pending changes not present in history are merged with the history set.

For `git`, the checkpoint is stored as a reference such as HEAD, or an object SHA. Pending changes are untracked and uncommitted files. Therefore, the set of changes reported by the `git` change provider is: `[<checkpoint>, ..., HEAD] + [untracked] + [uncommitted]`.

Now, for a practical example. First, query the checkpoint:

```sh
monorail checkpoint show | jq
```
```json
{
  "timestamp": "2024-11-10T11:24:10.154040+00:00",
  "checkpoint": {
    "id": "4b5c5a4ce18a05b0175c1db6a14fb69bf1ca30d3",
    "pending": null
  }
}
```

This is the checkpoint we created earlier in the tutorial. As mentioned, it's a marker for `monorail` to use as a starting point for determining what has changed in the repo. Its current value is the initial HEAD we created when the repo was created, and since we haven't committed anything thus far, it's not reducing the changeset. Thus, we need to include these untracked changes in the checkpoint to have an effect on analysis; do this by adding `--pending` or `-p` when updating the checkpoint. This will include those files and their SHA2 checksums in the checkpoint.


```sh
monorail checkpoint update --pending | jq
```
```json
{
  "timestamp": "2024-11-10T11:24:19.481342+00:00",
  "checkpoint": {
    "id": "4b5c5a4ce18a05b0175c1db6a14fb69bf1ca30d3",
    "pending": {
      "python/app3/monorail/hello.py": "fad357d1f5adadb0e270dfcf1029c6ed76e2565e62c811613eb315de10143ceb",
      "rust/app1/Cargo.toml": "044de847669ad2d9681ba25c4c71e584b5f12d836b9a49e71b4c8d68119e5592",
      "proto/LICENSE.md": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "Monorail.json": "b02cc0db02ef8c35ba8c285336748810c590950c263b5555f1781ac80f49a6da",
      "proto/README.md": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "rust/app2/Cargo.toml": "111f4cf0fd1b6ce6f690a5f938599be41963905db7d1169ec02684b00494e383",
      ".gitignore": "c1cf4f9ff4b1419420b8508426051e8925e2888b94b0d830e27b9071989e8e7d",
      "rust/monorail/hello.sh": "c1b9355995507cd3e90727bc79a0d6716b3f921a29b003f9d7834882218e2020",
      "python/app3/hello.py": "3639634f2916441a55e4b9be3497673f110014d0ce3b241c93a9794ffcf2c910",
      "python/app3/tests/test_hello.py": "72b3668ed95f4f246150f5f618e71f6cdbd397af785cd6f1137ee87524566948",
      "rust/Cargo.toml": "a35f77bcdb163b0880db4c5efeb666f96496bcb409b4cd52ba6df517fb4d625b",
      "rust/app2/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9",
      "proto/monorail/hello.awk": "5af404fedc153710aec00c8bf788d8f71b00c733c506d4c28fda1b7d618e4af6",
      "rust/app1/src/lib.rs": "536215b9277326854bd1c31401224ddf8f2d7758065c9076182b37621ad68bd9"
    }
  }
}
```

Now, `monorail` knows that these pending changes are no longer considered changed:

```sh
monorail analyze | jq
```
```json
{
  "timestamp": "2024-11-10T11:24:30.349076+00:00",
  "targets": []
}
```

... and thus there's nothing to run:

```sh
monorail -v run -c hello build
```
```json
{"timestamp":"2024-11-10T11:25:21.765947+00:00","level":"INFO","message":"processing plan"}
{"timestamp":"2024-11-10T11:25:21.766158+00:00","level":"INFO","message":"Connected to log stream server","address":"127.0.0.1:9201"}
{"timestamp":"2024-11-10T11:25:21.766272+00:00","level":"INFO","message":"processing commands","num":2}
{"timestamp":"2024-11-10T11:25:21.766282+00:00","level":"INFO","message":"processing target groups","num":0,"command":"hello"}
{"timestamp":"2024-11-10T11:25:21.766285+00:00","level":"INFO","message":"processing target groups","num":0,"command":"build"}
{"timestamp":"2024-11-10T11:25:21.766730+00:00","failed":false,"invocation":"-v run -c hello build","out":{"run":{"path":"/private/tmp/monorail-tutorial/monorail-out/run/9","files":{"result":"result.json.zst","stdout":"stdout.zst","stderr":"stderr.zst"},"targets":{}}},"results":[{"command":"hello","target_groups":[]},{"command":"build","target_groups":[]}]}
```

You can also freely `monorail checkpoint delete` and essentially bypass change detection entirely, causing all targets will be considered changed. You can also manually set the checkpoint with `monorail checkpoint update -i <id>`, where `<id>` is a change provider reference (e.g. for git, this is usually a commit SHA).

### Using the checkpoint

The `checkpoint` is a powerful way to control the behaviors of `analyze` and guided `run`. Since the checkpoint controls `monorail`s view of what has changed in the repository, and you can set the checkpoint id to any valid change provider reference, you can `analyze` and `run` for any point in history.

For example, this useful one-liner will perform a graph-guided run of the provided commands, and if successful, update the checkpoint to avoid running any commands for the involved targets until they change again: `monorail run -c <cmd1> <cmd2> ... <cmdN> && monorail checkpoint update -p`. This is most useful when the sequence of commands you provide covers everything you want to execute for a target in order for it to be considered "complete".

Locally, that might just be a bare minimum of commands; `monorail run prep check test integration-test && monorail checkpoint update -p`. In CI/CD, it might be more comprehensive: `monorail run prep check test integration-test package smoke dry-deploy && monorail checkpoint update -p`.

### Checkpointing in CI/CD

In a CI/CD environment, the id returned by `checkpoint update` or `checkpoint show` can be stored as a build artifact on a per-branch basis for one build and restored for the next one. This provides a means to do incremental command execution for builds, tests, packaging, deploys, and more.

To restore a checkpoint, provide `--id` or `-i` with a valid ID (sourced from a previous `checkpoint update` invocation) when updating the checkpoint:

```sh
monorail checkpoint update -p -i <id from previous checkpoint>
```

Additionally, using `monorail` with CI/CD it can be advantageous to retain state from previous builds (e.g. mounting a snapshot of a volume that is being used for builds for the current branch). This can make checkpoint restoration more useful, as you carry forward state from previous builds that have already been completed.

## Conclusion
This concludes the tutorial on the fundamentals of `monorail`. For most of what was covered here, additional options and configuration exists but are outside the scope of an introductory tutorial. For more information, use `monorail help` or pass the `--help` flag to subcommands.