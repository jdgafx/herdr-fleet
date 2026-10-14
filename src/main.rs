use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use herdr_fleet::herdr::Herdr;
use herdr_fleet::keys;
use herdr_fleet::model::{Fleet, Options};
use herdr_fleet::relaunch;
use herdr_fleet::roster::{self, Roster};
use herdr_fleet::theme::ThemeName;
use herdr_fleet::transcript::TranscriptTracker;
use herdr_fleet::tui::{self, Look};

/// A terminal dashboard for herdr agent panes.
#[derive(Parser, Debug)]
#[command(name = "herdr-fleet", version, about, long_about = None)]
struct Cli {
    /// herdr executable to invoke
    #[arg(long, global = true, env = "HERDR_BIN", default_value = "herdr")]
    herdr: String,

    /// Directory of markdown checklists to summarise (optional)
    #[arg(long, global = true)]
    queue_dir: Option<PathBuf>,

    /// Directory holding Claude Code session transcripts
    /// (default: $CLAUDE_CONFIG_DIR/projects or ~/.claude/projects)
    #[arg(long, global = true, env = "CLAUDE_PROJECTS_DIR")]
    claude_projects: Option<PathBuf>,

    /// Expected-loops list: one command fragment per line (default: $XDG_CONFIG_HOME/herdr-fleet/expected-loops)
    #[arg(long, global = true)]
    expected_loops: Option<PathBuf>,

    /// Tokens in the model's context window; 1M-context models use 1000000
    #[arg(long, global = true, default_value_t = 1_000_000)]
    context_window: u64,

    /// Seconds between automatic refreshes in the TUI
    #[arg(long, global = true, default_value_t = 2)]
    interval: u64,

    /// Seconds a pane may stay "working" before it is flagged as stalled
    #[arg(long, global = true, default_value_t = 900)]
    stall_secs: u64,

    /// Minutes a working pane's transcript must be silent before it counts as stalled
    #[arg(long, global = true, default_value_t = 10)]
    quiet_mins: u64,

    /// Desktop alerts (herdr notification) when a pane goes blocked, stalled or looping
    #[arg(long, global = true)]
    alerts: bool,

    /// Roster file: which pane runs which queue, with which launch flags
    /// (default: $XDG_CONFIG_HOME/herdr-fleet/roster.toml)
    #[arg(long, global = true, env = "HERDR_FLEET_ROSTER")]
    roster: Option<PathBuf>,

    /// Colour theme: mocha (dark), latte (light) or ansi16 (the terminal's own 16 colours)
    #[arg(long, global = true, value_enum, default_value_t = ThemeName::Mocha)]
    theme: ThemeName,

    /// Turn off the transition effects (the fade on a row whose state changed)
    #[arg(long, global = true)]
    no_fx: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum RosterCmd {
    /// Seed the roster from the roster lines of a supervisor file
    Import {
        /// Markdown file with `- <pane> <queue> <launch> <folder>` lines
        file: PathBuf,
        /// Roster file to write (default: the --roster path)
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the interactive dashboard (default)
    Tui,
    /// Print one snapshot and exit
    List {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Focus the pane herdr-fleet was started in (recorded when the TUI ran)
    Back,
    /// Compare every roster pane's live launch with the roster; exits 1 on drift
    Check,
    /// Relaunch a roster pane fresh on its roster flags, then send its `/work <queue>`
    Relaunch {
        /// Pane id, e.g. w1F:p1
        pane: String,
        /// Print each step and change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage the roster file
    Roster {
        #[command(subcommand)]
        cmd: RosterCmd,
    },
    /// Write the herdr key binding that runs `herdr-fleet back`
    InstallKeys {
        /// herdr config.toml to edit (default: $HERDR_CONFIG_PATH or ~/.config/herdr/config.toml)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Key to bind
        #[arg(long, default_value = "prefix+f")]
        key: String,
        /// Do not ask the running herdr server to reload the config
        #[arg(long)]
        no_reload: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let herdr = Herdr::new(cli.herdr);

    let roster_path = cli.roster.clone().unwrap_or_else(roster::default_path);
    let mut options = Options::default();
    if let Some(dir) = cli.claude_projects {
        options.projects_dir = dir;
    }
    options.context_window = cli.context_window;
    let loops_path = cli
        .expected_loops
        .clone()
        .unwrap_or_else(roster::expected_loops_path);
    options.expected_loops = roster::load_expected_loops(&loops_path)?;
    options.quiet = std::time::Duration::from_secs(cli.quiet_mins * 60);
    if roster_path.exists() {
        options.roster = Some(Roster::load(&roster_path)?);
    }

    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(
            herdr,
            cli.queue_dir,
            options,
            cli.interval,
            cli.stall_secs,
            cli.alerts,
            Look {
                theme: cli.theme,
                fx: !cli.no_fx,
            },
        ),
        Cmd::List { json } => {
            let mut tracker = TranscriptTracker::default();
            let fleet = Fleet::collect(&herdr, cli.queue_dir.as_deref(), &options, &mut tracker)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&fleet)?);
            } else {
                print!("{}", fleet.render_text());
            }
            Ok(())
        }
        Cmd::Back => {
            println!("{}", keys::back(&herdr)?);
            Ok(())
        }
        Cmd::Check => {
            let roster = options.roster.clone().context("no roster to check")?;
            let mut drifted = false;
            println!("{:<8} {:<22} {:<8} DRIFT", "PANE", "QUEUE", "STATE");
            for entry in &roster.panes {
                let live = roster::live_launch(&herdr, &entry.pane);
                let drift = roster::drift(entry, live.as_ref());
                let state = match (entry.paused, live.is_some()) {
                    (true, _) => "paused",
                    (false, true) => "running",
                    (false, false) => "stopped",
                };
                drifted |= !entry.paused && !drift.is_empty();
                let verdict = if drift.is_empty() {
                    "ok".into()
                } else {
                    drift.join(", ")
                };
                println!(
                    "{:<8} {:<22} {:<8} {}",
                    entry.pane, entry.queue, state, verdict
                );
            }
            if drifted {
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Relaunch { pane, dry_run } => {
            let roster = options
                .roster
                .clone()
                .context("no roster to relaunch from")?;
            relaunch::relaunch(&herdr, &roster, &pane, dry_run)
        }
        Cmd::Roster {
            cmd: RosterCmd::Import { file, out },
        } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let (roster, skipped) = Roster::import(&text)?;
            let out = out.unwrap_or(roster_path);
            roster.save(&out)?;
            println!("wrote {} entries to {}", roster.panes.len(), out.display());
            for pane in skipped {
                println!("skipped {pane}: its line names no launch command; add it by hand");
            }
            Ok(())
        }
        Cmd::InstallKeys {
            config,
            key,
            no_reload,
        } => {
            let config = config.unwrap_or_else(keys::default_config_path);
            print!("{}", keys::install_keys(&herdr, &config, &key, !no_reload)?);
            Ok(())
        }
    }
}
