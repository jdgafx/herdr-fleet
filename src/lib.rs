//! herdr-fleet: a terminal dashboard for [herdr](https://herdr.dev) agent panes.
//!
//! It polls herdr's own JSON API (`herdr api snapshot`, falling back to
//! `herdr agent list`) and reads each Claude pane's session transcript to
//! report, per pane: status, model, context %, estimated spend, time in the
//! current state and the last output line. It flags panes that look stalled,
//! stuck in a command loop, or waiting on an approval dialog.
//!
//! The library is read-only except for two explicit actions: the TUI records
//! its own pane (`herdr pane current`) on start so `herdr-fleet back` can
//! focus it, and the interactive TUI sends keys/prompts/alerts only when you
//! press the key for it (or pass `--alerts`).

pub mod fetch;
pub mod herdr;
pub mod keys;
pub mod konsole;
pub mod model;
pub mod parser;
pub mod price;
pub mod queue;
pub mod relaunch;
pub mod roster;
pub mod theme;
pub mod transcript;
pub mod tui;
