#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
log_dir="${XDG_CONFIG_HOME:-$HOME/.config}/rockcast"
echo "Log file: $log_dir/rockcast.log"
export RUST_LOG="${RUST_LOG:-rockcast=debug}"
exec cargo run --release "$@"
