# herdr-fleet

A terminal dashboard for [herdr](https://herdr.dev) agent panes. One screen shows
every agent's status, how long it has been in that state, its last line of output,
and anything that looks stuck — plus the open-item counts of your markdown queues.

Built for a machine that runs a fleet of Claude Code agents in herdr panes and
needs to see, at a glance, which pane needs attention.

![herdr-fleet running against a live fleet](docs/screenshot.png)

## What it shows

| Column | Meaning |
| --- | --- |
| `PANE` | herdr pane id; `●` marks the focused pane |
| `NAME` | the pane's title (or its label) |
| `STATUS` | `working` / `idle` / `done` / `blocked`, from herdr |
| `MODEL` | the model named in the pane's status line |
| `CTX` | context used %, from the pane's status line |
| `IN STATE` | time in the current state (`!` = past the stall budget) |
| `FLAGS` | `dialog:` an approval prompt is waiting; `loop xN:` the same line N times |
| `LAST OUTPUT` | the last real line of output |

A `queues` panel lists every `*.md` checklist in `--queue-dir` with `open`/`done`
counts and the first open item.

## Install

```bash
cargo install --path .
```

Or run it straight from the tree:

```bash
cargo run --release -- tui --queue-dir ~/dev/CC-MAESTRO/queue
```

## Usage

```
herdr-fleet [--herdr BIN] [--queue-dir DIR] [--interval SECS] [--stall-secs SECS] [COMMAND]

Commands:
  tui    Run the interactive dashboard (default)
  list   Print one snapshot and exit  [--json]
```

`herdr-fleet list` against a live fleet:

```
herdr-fleet · herdr 0.9.2-preview · 5 agents
PANE      NAME                   STATUS   MODEL         CTX  LAST OUTPUT                        FLAGS
w1D:pM    Deepseek agents with … idle     -               -  Reviewed the latest commit… 
w19:pG    Shared image           done     -               -  Done: the review is on file.
w1C:p3    Claude Code            idle     -               -  waiting for the next item
w1F:p1    Claude Code            working  -               -  running the test suite
w1E:p1    Autocompact thrashi…   working  -               -  retrying the last edit
```

`--json` prints the same snapshot as machine-readable JSON, so the dashboard can
feed other tools.

## Keys

| Key | Action |
| --- | --- |
| `q` | quit |
| `r` | refresh now |
| `↑`/`k`, `↓`/`j` | move the selection |
| `⏎` | focus the selected pane in herdr (`herdr agent focus`) |
| `i` | type a prompt for the selected pane (`herdr agent prompt`); `Esc` cancels |

The dashboard is **read-only by default**: it never sends keys or prompts unless
you press `⏎` or `i`.

## Queues

Point `--queue-dir` at any folder of markdown checklists. Each `*.md` file is
summarised by its `- [ ]` (open) and `- [x]` (done) lines; nothing about the
format is herdr- or project-specific.

## How it reads herdr

`herdr-fleet` shells out to the `herdr` CLI: `herdr api snapshot` (falling back to
`herdr agent list`) for the pane list, and `herdr agent read <pane>` for each pane's
tail. `--herdr` (or `$HERDR_BIN`) selects the executable.

## Limitations

- `MODEL` and `CTX` come from the status line Claude Code prints at the bottom of a
  pane. If the pane is narrower than that line, it is truncated and these columns
  show `-`.
- Time-in-state is measured from when `herdr-fleet` first sees a pane in a state,
  so it starts at zero on launch and needs the dashboard open to accrue.
- The loop and dialog detectors are heuristics over the pane text: a repeated
  substantive line, and a small set of approval-dialog phrases.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs all three, plus a release build, on every push and pull request.

`scripts/screenshot.sh` regenerates the image above from a real run via tmux.

## License

MIT — see [LICENSE](LICENSE).
