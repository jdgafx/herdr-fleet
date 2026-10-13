# herdr-fleet

A terminal dashboard for [herdr](https://herdr.dev) agent panes. One screen shows
every agent's status, model, context usage, how long it has been in that state,
its last line of output, and anything that looks stuck — plus the open-item
counts of your markdown queues.

Built for a machine that runs a fleet of Claude Code agents in herdr panes and
needs to see, at a glance, which pane needs attention.

![herdr-fleet running against a live fleet](docs/screenshot.png)

The capture is a real run against a live fleet; the queues panel uses
[`docs/demo-queue`](docs/demo-queue) so no private checklist names are shown.

## What it shows

| Column | Meaning |
| --- | --- |
| `PANE` | herdr pane id; `●` marks the focused pane |
| `NAME` | the pane's title (or its label) |
| `STATUS` | `working` / `idle` / `done` / `blocked`, from herdr |
| `MODEL` | the model running in the pane |
| `CTX` | context used % = tokens over the context window |
| `IN STATE` | time in the current state, as observed since launch (`!` = past the stall budget) |
| `FLAGS` | `loop xN:` the same Bash command N times; `blocked`; `dialog:` a waiting prompt |
| `LAST OUTPUT` | the last line the agent wrote |

A `queues` panel lists every `*.md` checklist in `--queue-dir` with `open`/`done`
counts and the first open item.

## How it reads the fleet

`herdr agent read` returns pane text **clamped to the pane width**, which cuts the
status line off before `model | ctx:%` — so that is not a reliable source. Instead,
herdr-fleet follows the session id herdr reports for each Claude pane to the
session transcript under `~/.claude/projects/*/<session-id>.jsonl` and reads, from
the last assistant message:

- the **model** and its **token usage** (`input + cache_read + cache_creation`, over
  `--context-window`, default `1000000` for 1M-context models);
- the last line of assistant **text**;
- its recent `Bash` tool calls, for the **loop** flag (the same command 3+ times).

For a pane with no transcript yet, or a non-Claude agent, it falls back to
scraping the pane text. The `source` field in `--json` output says which was used
(`transcript` or `pane`).

The pane list itself comes from `herdr api snapshot` (falling back to
`herdr agent list`). `--herdr` (or `$HERDR_BIN`) selects the executable.

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
herdr-fleet [OPTIONS] [COMMAND]

Options:
      --herdr <BIN>              herdr executable            [env: HERDR_BIN] [default: herdr]
      --queue-dir <DIR>          markdown checklists to summarise
      --claude-projects <DIR>    session transcripts          [env: CLAUDE_PROJECTS_DIR]
                                 [default: $CLAUDE_CONFIG_DIR/projects or ~/.claude/projects]
      --context-window <TOKENS>  context window for the ctx %  [default: 1000000]
      --interval <SECS>          TUI refresh interval          [default: 2]
      --stall-secs <SECS>        "working" past this = stalled [default: 900]

Commands:
  tui    Run the interactive dashboard (default)
  list   Print one snapshot and exit  [--json]
```

`herdr-fleet list` against a live fleet:

```
herdr-fleet · herdr 0.9.2-preview · 5 agents
PANE      NAME                   STATUS   MODEL            CTX  LAST OUTPUT                       FLAGS
w1D:pM    Deepseek agents with … done     Opus 5.5         18%  do you want to read the answers b…  loop x3: /home/dev/tools/inbox-wait…
w19:pG    Shared image           done     deepseek-flash   38%  Q8 stays open until your Gmail S…
w1C:p3    Claude Code            idle     -                 -   chris2@box:/home/dev/project-chro…
w1F:p1    herdr-fleet accepta…   working  deepseek-flash   47%  Header is 3 rows but holds 2 li…
w1E:p1    Autocompact thrashi…   working  deepseek-flash   56%  I'll report Q7's scores when th…
```

`--json` prints the same snapshot as machine-readable JSON.

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

## Limitations

- **IN STATE** is counted from when herdr-fleet first sees a pane in a state, not
  from herdr's own state-change time (herdr exposes a sequence number, not a
  timestamp). It is labelled as observed-since-launch for that reason.
- The `CTX` percentage is only as good as `--context-window`. It defaults to
  1,000,000 (the 1M-context models this was built against).
- A Claude pane with no assistant message in its transcript yet, or a non-Claude
  agent, falls back to the clamped pane text. `MODEL`/`CTX` are usually blank
  there, but the status line (even truncated) and the input box are filtered out
  of `LAST OUTPUT`.
- The loop and dialog detectors are heuristics: a repeated Bash command in the
  recent transcript, and a small set of approval-dialog phrases in pane text.

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
