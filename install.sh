#!/usr/bin/env bash

# This script installs all command-line utilities onto the current system.

location=/usr/local/bin
if [ "$1" != "" ]; then
    location="$1"
fi

# compile `monorail` and deposit it in the desired location
cargo build --release
echo "* Copying monorail to ${location}"
cp target/release/monorail "$location"

# set executable bit and install ext components to the desired location
echo "* Copying monorail-bash to ${location}"
chmod +x ext/bash/monorail-bash.sh
cp ext/bash/monorail-bash.sh "${location}/monorail-bash"
