#!/usr/bin/env bash
set -euo pipefail
task_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="${1:-$task_dir/repo}"
node "$task_dir/verify.mjs" "$task_dir" "$repo_dir"
