#!/usr/bin/env bash
# Capture a real herdr-fleet run to a PNG for the README.
#
#   scripts/screenshot.sh [out.png]
#
# Env: BIN (default: target/release/herdr-fleet), QUEUE_DIR (optional),
#      SIZE (default 210x50), WAIT (seconds to let a refresh land, default 4)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
out="${1:-$root/docs/screenshot.png}"
bin="${BIN:-$root/target/release/herdr-fleet}"
queue_dir="${QUEUE_DIR:-}"
size="${SIZE:-210x50}"
wait_s="${WAIT:-4}"

args=(tui --interval 2)
[[ -n "$queue_dir" ]] && args+=(--queue-dir "$queue_dir")

raw="$(mktemp)"
sess="hfs_$$"

cleanup() { tmux kill-session -t "$sess" 2>/dev/null || true; rm -f "$raw"; }
trap cleanup EXIT

mkdir -p "$(dirname "$out")"
tmux new-session -d -s "$sess" -x "${size%x*}" -y "${size#*x}"
tmux send-keys -t "$sess" "clear && $(printf '%q ' "$bin" "${args[@]}")" Enter
sleep "$wait_s"
tmux capture-pane -p -e -t "$sess" > "$raw"
tmux kill-session -t "$sess" 2>/dev/null || true

python3 "$here/ansi_to_png.py" "$raw" "$out"
