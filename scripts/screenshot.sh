#!/usr/bin/env bash
# Capture a real herdr-fleet run to a PNG for the README.
#
#   scripts/screenshot.sh [out.png]
#
# Env: BIN (default: target/release/herdr-fleet), QUEUE_DIR (optional),
#      SCREENSHOT_SIZE (default 210x50 — not SIZE, which LLVM toolchains export),
#      SCRUB_WORDS (optional regex of tokens the live panes may display and the
#      PNG must not carry, e.g. local codenames), WAIT (seconds, default 4).
# The LAST OUTPUT column is always redacted (scrub_last_output.py).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
out="${1:-$root/docs/screenshot.png}"
bin="${BIN:-$root/target/release/herdr-fleet}"
queue_dir="${QUEUE_DIR:-}"
size="${SCREENSHOT_SIZE:-210x50}"
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
# Capture, then scrub what must not reach the committed PNG: the local home
# path, and any SCRUB_WORDS tokens the live panes happen to show. `git grep`
# can never catch a leak that is only pixels.
scrub='s#(/home|/Users)/[^/[:space:]]+#\1/dev#g'
if [[ -n "${SCRUB_WORDS:-}" ]]; then
  scrub="${scrub}; s/(${SCRUB_WORDS})/[redacted]/g"
fi
tmux capture-pane -p -e -t "$sess" | sed -E "$scrub" > "$raw"
tmux kill-session -t "$sess" 2>/dev/null || true
# Always blank the LAST OUTPUT column of fleet rows: it carries live agent text
# that no word list can predict.
python3 "$here/scrub_last_output.py" "$raw" "$raw.out" && mv "$raw.out" "$raw"

python3 "$here/ansi_to_png.py" "$raw" "$out"
