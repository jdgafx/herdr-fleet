#!/usr/bin/env bash
# Capture the fleet TUI to a PNG for the README.
#
#   scripts/screenshot.sh [out.png]
#
# DEMO (default 1): a synthetic fleet from scripts/demo: a stub herdr, demo
#   transcripts, a demo roster and the demo queues in docs/demo-queue. Nothing in
#   it is live, so the image can be published as is.
# DEMO=0: the live fleet. The LAST OUTPUT column is always redacted
#   (scripts/scrub_last_output.py), and SCRUB_WORDS (a regex) is redacted too,
#   e.g. local codenames, names and hosts.
# Also: BIN (default target/release/herdr-fleet), SCREENSHOT_SIZE (default
#   170x26 demo, 210x50 live: not SIZE, which LLVM toolchains export), WAIT (seconds,
#   default 4), QUEUE_DIR (live only).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
demo="$here/demo"
out="${1:-$root/docs/screenshot.png}"
bin="${BIN:-$root/target/release/herdr-fleet}"
mode="${DEMO:-1}"
wait_s="${WAIT:-4}"
if [[ "$mode" == 1 ]]; then
  size="${SCREENSHOT_SIZE:-170x26}"
else
  size="${SCREENSHOT_SIZE:-210x50}"
fi

raw="$(mktemp)"
pids="$(mktemp)"
sess="hfs_$$"
sleepers=()

cleanup() {
  tmux kill-session -t "$sess" 2>/dev/null || true
  for p in "${sleepers[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -f "$raw" "$raw.out" "$pids"
}
trap cleanup EXIT

if [[ "$mode" == 1 ]]; then
  # One sleeping process per roster pane, with the roster's environment, so the
  # drift check sees the launch the roster describes.
  for pane in w1A:p1 w1B:p2 w1C:p3 w1D:p4 w1E:p5; do
    env CLAUDE_CODE_SUBAGENT_MODEL=haiku sleep 3600 &
    sleepers+=("$!")
    echo "$pane $!" >> "$pids"
  done
  cmd="clear && env HERDR_BIN=$demo/herdr DEMO_PIDS=$pids $(printf '%q ' "$bin" --roster "$demo/roster.toml" --claude-projects "$demo/projects" tui --interval 2 --queue-dir "$root/docs/demo-queue")"
else
  args=(tui --interval 2)
  [[ -n "${QUEUE_DIR:-}" ]] && args+=(--queue-dir "$QUEUE_DIR")
  cmd="clear && $(printf '%q ' "$bin" "${args[@]}")"
fi

mkdir -p "$(dirname "$out")"
tmux new-session -d -s "$sess" -x "${size%x*}" -y "${size#*x}"
tmux send-keys -t "$sess" "$cmd" Enter
sleep "$wait_s"
# Capture, then scrub what must not reach the committed PNG: the local home
# path, and in live mode any SCRUB_WORDS tokens the panes happen to show.
scrub='s#(/home|/Users)/[^/[:space:]]+#\1/dev#g'
if [[ "$mode" != 1 && -n "${SCRUB_WORDS:-}" ]]; then
  scrub="${scrub}; s/(${SCRUB_WORDS})/[redacted]/g"
fi
tmux capture-pane -p -e -t "$sess" | sed -E "$scrub" > "$raw"
tmux kill-session -t "$sess" 2>/dev/null || true
# Live output is blanked by position: no word list can predict it. The demo
# output is synthetic, so it stays readable.
if [[ "$mode" != 1 ]]; then
  python3 "$here/scrub_last_output.py" "$raw" "$raw.out" && mv "$raw.out" "$raw"
fi

python3 "$here/ansi_to_png.py" "$raw" "$out"
