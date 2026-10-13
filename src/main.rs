use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use herdr_fleet::herdr::Herdr;
use herdr_fleet::model::Fleet;
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
    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(herdr, cli.queue_dir, cli.interval, cli.stall_secs),
        Cmd::List { json } => {
            let fleet = Fleet::collect(&herdr, cli.queue_dir.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&fleet)?);
            } else {
                print!("{}", fleet.render_text());
            }
            Ok(())
        }
    }
}
