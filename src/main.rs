use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use herdr_fleet::herdr::Herdr;
use herdr_fleet::model::{Fleet, Options};
use herdr_fleet::tui;

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

    /// Tokens in the model's context window; 1M-context models use 1000000
    #[arg(long, global = true, default_value_t = 1_000_000)]
    context_window: u64,

    /// Seconds between automatic refreshes in the TUI
    #[arg(long, global = true, default_value_t = 2)]
    interval: u64,

    /// Seconds a pane may stay "working" before it is flagged as stalled
    #[arg(long, global = true, default_value_t = 900)]
    stall_secs: u64,

    #[command(subcommand)]
    cmd: Option<Cmd>,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let herdr = Herdr::new(cli.herdr);

    let mut options = Options::default();
    if let Some(dir) = cli.claude_projects {
        options.projects_dir = dir;
    }
    options.context_window = cli.context_window;

    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(herdr, cli.queue_dir, options, cli.interval, cli.stall_secs),
        Cmd::List { json } => {
            let fleet = Fleet::collect(&herdr, cli.queue_dir.as_deref(), &options)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&fleet)?);
            } else {
                print!("{}", fleet.render_text());
            }
            Ok(())
        }
    }
}
