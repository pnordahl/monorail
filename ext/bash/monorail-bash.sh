#!/usr/bin/env bash

# defaults
working_directory=$(pwd)
monorail_path=monorail
git_path=git
use_libgit2_status=false
jq_path=jq
config_file='Monorail.toml'
verbose=false
targets=()
commands=()
start=""
end=""

function print_help {
	echo "Usage: monorail-bash [dfvh]"
	echo ""
	echo "    -h               Display this help message."
	echo "    -f <file>        The monorail configuration file to use. Default: Monorail.toml"
	echo "    -w <path>        The directory to run commands from. Default: current directory"
	echo "    -v               Print extra information."
	echo ""
	echo "Subcommands:"
	echo "    monorail-bash [exec]"
	echo ""
}
function print_exec_help {
	echo "Usage: monorail-bash [dfvhg] exec [hsetmj] -c <command>"
	echo ""
	echo "    -h                       Display this help message."
	echo "    -s <start>               Location in VCS history to use as a starting point for analyzing changes."
	echo "    -e <end>                 Location in VCS history to use as an end point for analyzing changes."
	echo "    -c <command>             A command to execute."
	echo "    -t <target>              A target to execute commands for."
	echo "    -m <path>                Absolute path to the 'monorail' binary to use. Default: 'which monorail'"
	echo "    -g <path>                Absolute path to the 'git' binary to use. Default: 'which git'"
	echo "    -j <path>                Absolute path to the 'jq' binary to use. Default: 'which jq'"
}

function exit_if_error {
	code=$1
	msg=$2
	if [ "$code" -ne 0 ]; then
		log_error "$msg"
		exit "$code"
	fi
}

function process_exec {
	if [ "$MONORAIL_USE_LIBGIT2_STATUS" == "true" ]; then
		use_libgit2_status=true
	fi
	if [ "$verbose" = true ]; then
		log_verbose "$(printf "'monorail' path:    %s" "$monorail_path")"
		log_verbose "$(printf "'jq' path:          %s" "$jq_path")"
		log_verbose "$(printf "'git' path:         %s" "$git_path")"
		log_verbose "$(printf "use libgit2 status: %s" "$use_libgit2_status")"
		log_verbose "$(printf "'monorail' config:  %s" "$config_file")"
		log_verbose "$(printf "working directory:  %s" "$working_directory")"
		for command in "${commands[@]}"; do
			log_verbose "$(printf "command:            %s" "$command")"
		done
		for target in "${targets[@]}"; do
			log_verbose "$(printf "target:             %s" "$target")"
		done
		log_verbose "$(printf "start:              %s" "$start")"
		log_verbose "$(printf "end:                %s" "$end")"
	fi

	# build array of args to pass to monorail
	monorail_args=("-f" "$config_file" "-w" "$working_directory")

	if [ ${#commands[@]} -eq 0 ]; then
	    log_error "error: no commands specified"
	    print_exec_help
	    exit 1
	fi

	# get config
	config=$($monorail_path "${monorail_args[@]}" config)
	exit_if_error $? "$(printf "Error getting monorail config; file: %s" "$config_file")"

	vcs_use=$($jq_path -cr '.vcs.use' <<< "$config")
	exit_if_error $? "Error getting monorail config parameter; parameter: 'vcs.use'"

	# get global extension that should be used
	extension_use=$($jq_path -cr '.extension.use' <<< "$config")
	exit_if_error $? "Error getting monorail config parameter; parameter: 'extension.use'"
	if [ "$extension_use" != "bash" ]; then
		log_error "Wrong extension; use monorail-extension-${extension_use} instead of this program"
		exit 1
	fi

	# get global entrypoint script location
	script_location=$($jq_path -cr '.extension.bash.exec.entrypoint' <<< "$config")
	exit_if_error $? "Error getting monorail config parameter; parameter: 'extension.bash.exec.entrypoint'"

	# get global list of scripts to source before each command invocation
	sources_str=$($jq_path -cr '.extension.bash.exec.source | .[]?' <<< "$config")
	exit_if_error $? "Error getting monorail config parameter; parameter: 'extension.bash.exec.source'"
	IFS=$'\n' read -rd '' -a sources <<< "$sources_str"
	for src in "${sources[@]}"; do
		log_verbose "$(printf "source:             %s" "$src")"
	done

	# set optional 'analyze' parameters
	monorail_analyze_args=("analyze")
	if [ "$vcs_use" == "git" ]; then
		monorail_analyze_args+=("--git-path" "$git_path")
		if [ "$use_libgit2_status" == true ]; then
			monorail_analyze_args+=("--use-libgit2-status")
		fi
	fi

	if [ -n "${start}" ]; then
		monorail_analyze_args+=("-s" "$start")
	fi
	if [ -n "${end}" ]; then
		monorail_analyze_args+=("-e" "$end")
	fi
	if [ ${#targets[@]} -eq 0 ]; then
	    # extract targets for execution from monorail
	    targets_str=$($monorail_path "${monorail_args[@]}" "${monorail_analyze_args[@]}")
		exit_if_error $? "Error getting monorail targets"

		targets_str=$(jq -r '.targets[]' <<< "$targets_str")
		IFS=$'\n' read -rd '' -a targets <<< "$targets_str"
		for target in "${targets[@]}"; do
			log_verbose "$(printf "target (inferred):  %s" "$target")"
		done
	fi

	# if there are still no targets, there is nothing to do
	if [ ${#targets[@]} -eq 0 ]; then
		log_verbose "No targets provided or inferred, nothing to do"
		exit 0
	fi

	# execute commands for each target
	for command in "${commands[@]}"; do
		for target in "${targets[@]}"; do
			for src in "${sources[@]}"; do
				file_path="${working_directory}/${src}"
				# shellcheck disable=SC1090
				source "$file_path"
				exit_if_error $? "$(printf "Error sourcing; file: %s, target: %s" "$file_path" "$target")"
			done
			execute_target_command "$command" "$target" "$script_location"
			exit_if_error $? "$(printf "Error executing; command: %s, target: %s" "$command" "$target")"
		done
	done
}

function log {
	case "$1" in
		0) printf "%s monorail-bash : %s\n" "$(date '+%b %d %T')" "$2" ;;
		1) printf "%s monorail-bash : %s\n" "$(date '+%b %d %T')" "$2" >&2 ;;
	esac
}
function log_verbose {
	if [ "$verbose" = true ]; then
		log 0 "$1"
	fi
}
function log_error {
	log 1 "$1"
}

function execute_target_command {
	local command=$1
	local target=$2
	local script=$3
	pushd "$working_directory" > /dev/null || return
	if [ ! -w "$target" ]; then
		log_verbose  "$(printf "NOTE: Ignoring command for non-directory target; command: %s, target: %s" "$command" "$target")"
		popd > /dev/null || return
		return
	fi
	pushd "$target" > /dev/null || return
	(
		if [[ -f $script ]]; then
			# shellcheck disable=SC1090
			source "$script"
			if [[ $(type -t "$command") == function ]]; then
				log_verbose "$(printf "Executing command; command: %s, target: %s" "$command" "$target")"
				$command
			else
				log_verbose "$(printf "NOTE: Undefined command; command: %s, target: %s" "$command" "$target")"
			fi
		else
			log_verbose "$(printf "NOTE: Script not found; command: %s, target: %s" "$command" "$target")"
		fi
	)
	code=$?
	popd > /dev/null || return
	popd > /dev/null || return
	if [ $code -ne 0 ]; then return $code; fi
}

# parse options to `monorail-bash`
while getopts "f:w:hv" opt; do
	case "$opt" in
		h )
			print_help
			exit 0
			;;
		f )
			config_file=$OPTARG
			;;
		w )
			working_directory=$OPTARG
			;;
		v )
			verbose=true
			;;
		\? )
			echo "Invalid Option: -$OPTARG" 1>&2
			exit 1
			;;
  esac
done

shift $((OPTIND - 1))

# process subcommands
subcommand=$1; shift
OPTIND=1
case "$subcommand" in
	exec)
		while getopts "s:e:c:t:m:g:j:h" opt; do
			case "$opt" in
				h ) 
					print_exec_help
					exit 0
					;;
				s ) start=$OPTARG;;
				e ) end=$OPTARG;;
				c ) commands+=("$OPTARG");;
				t ) targets+=("$OPTARG");;
				m ) monorail_path=$OPTARG;;
				g ) git_path=$OPTARG;;
				j ) jq_path=$OPTARG;;
				\? )
					echo "Invalid Option: -$OPTARG" 1>&2
					exit 1
					;;
			esac
		done
		shift $((OPTIND -1))
		process_exec
		;;
	* )
		print_help
		exit 1
		;;
esac
