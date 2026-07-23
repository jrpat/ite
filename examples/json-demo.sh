#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(dirname "$script_dir")

if [ "$#" -eq 0 ]; then
    set -- --expand 1
fi

cd "$repo_root"
exec cargo run --quiet -- --json "$script_dir/sample.json" "$@"
