//! The roster: which herdr pane runs which queue, and the launch flags it must run with.
//!
//! The roster file (`~/.config/herdr-fleet/roster.toml`) is the source of truth for
//! `relaunch` and for the drift check. `roster import` seeds it once from the roster
//! lines of a supervisor file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::Herdr;

/// Commands a roster line may launch.
const LAUNCH_COMMANDS: [&str; 2] = ["claude", "cc-deepseek"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster {
    #[serde(default, rename = "pane")]
    pub panes: Vec<Entry>,
}

/// One pane: its queue and the exact launch it must run with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub pane: String,
    pub queue: String,
    /// `claude` or `cc-deepseek`.
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Boolean flags, in launch order (e.g. `--dangerously-skip-permissions`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// Variables set before the command.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Variables removed before the command (`env -u`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
    /// Working folder the pane is started in (`~` allowed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// A paused pane is never relaunched.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

impl Roster {
    /// Read a roster file.
    pub fn load(path: &Path) -> Result<Roster> {
        let text = fs::read_to_string(path).with_context(|| {
            format!(
                "no roster at {} (seed it with `roster import`)",
                path.display()
            )
        })?;
        let roster: Roster =
            toml::from_str(&text).with_context(|| format!("bad roster {}", path.display()))?;
        for entry in &roster.panes {
            if !LAUNCH_COMMANDS.contains(&entry.command.as_str()) {
                bail!(
                    "pane {}: command `{}` is not one of {LAUNCH_COMMANDS:?}",
                    entry.pane,
                    entry.command
                );
            }
        }
        Ok(roster)
    }

    /// Write the roster file, creating its folder.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).context("serialising roster")?;
        fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// The entry for a pane id.
    pub fn find(&self, pane: &str) -> Option<&Entry> {
        self.panes.iter().find(|e| e.pane == pane)
    }

    /// Seed a roster from the roster lines of a supervisor file. Returns the
    /// roster and the panes whose line names no launch command (not relaunchable
    /// by this tool; they must be added by hand).
    pub fn import(text: &str) -> Result<(Roster, Vec<String>)> {
        let mut roster = Roster::default();
        let mut skipped = Vec::new();
        for line in text.lines() {
            let t = line.trim().trim_start_matches("- ").trim();
            let first = t.split_whitespace().next().unwrap_or("");
            if !is_pane_id(first) {
                continue;
            }
            match parse_line(t).with_context(|| format!("pane {first}"))? {
                Some(entry) => roster.panes.push(entry),
                None => skipped.push(first.to_string()),
            }
        }
        Ok((roster, skipped))
    }
}

/// True for a herdr pane id such as `w1F:p1`.
pub fn is_pane_id(s: &str) -> bool {
    s.starts_with('w') && s.contains(":p")
}

/// Parse one roster line such as
/// `w1F:p1 housekeeping env -u A CLAUDE_X=1 claude --model haiku --effort medium ~ (home)`.
/// Returns `None` when the line has no launch command.
pub fn parse_line(line: &str) -> Result<Option<Entry>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 3 || !is_pane_id(tokens[0]) {
        return Ok(None);
    }
    // Searched after the queue name, so a queue called `claude` is not read as the command.
    let Some(cmd_idx) = tokens
        .iter()
        .skip(2)
        .position(|t| LAUNCH_COMMANDS.contains(t))
        .map(|i| i + 2)
    else {
        return Ok(None);
    };
    let mut entry = Entry {
        pane: tokens[0].to_string(),
        queue: tokens[1].to_string(),
        command: tokens[cmd_idx].to_string(),
        paused: line.contains("PAUSED"),
        ..Entry::default()
    };

    let mut i = 2;
    while i < cmd_idx {
        let t = tokens[i];
        if t == "env" {
            // the wrapper itself carries no information
        } else if t == "-u" {
            let name = tokens.get(i + 1).context("`env -u` without a name")?;
            entry.unset.push(name.to_string());
            i += 1;
        } else if let Some((k, v)) = t.split_once('=') {
            entry.env.insert(k.to_string(), v.to_string());
        } else {
            bail!("unexpected `{t}` before the launch command");
        }
        i += 1;
    }

    let mut j = cmd_idx + 1;
    while j < tokens.len() {
        let t = tokens[j];
        if t == "--model" || t == "--effort" {
            let value = tokens
                .get(j + 1)
                .with_context(|| format!("{t} without a value"))?;
            if t == "--model" {
                entry.model = Some(value.to_string());
            } else {
                entry.effort = Some(value.to_string());
            }
            j += 2;
            continue;
        }
        if t.starts_with('~') || t.starts_with('/') {
            entry.folder = t.to_string();
        } else if t.starts_with('(') {
            break; // a trailing annotation such as "(JC)"
        } else if t == "PAUSED" {
            // the marker is read from the whole line (`paused`)
        } else if t.starts_with('-') {
            entry.flags.push(t.to_string());
        } else {
            bail!("unexpected `{t}` after the launch command");
        }
        j += 1;
    }
    Ok(Some(entry))
}

impl Entry {
    /// The shell line that starts this pane's worker fresh, with the roster flags.
    pub fn launch_line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.folder.is_empty() {
            parts.push(format!("cd {} &&", folder_arg(&self.folder)));
        }
        if !self.env.is_empty() || !self.unset.is_empty() {
            parts.push("env".into());
            for name in &self.unset {
                parts.push("-u".into());
                parts.push(shell_quote(name));
            }
            for (k, v) in &self.env {
                parts.push(shell_quote(&format!("{k}={v}")));
            }
        }
        parts.push(self.command.clone());
        parts.extend(self.flags.iter().map(|f| shell_quote(f)));
        if let Some(model) = &self.model {
            parts.push("--model".into());
            parts.push(shell_quote(model));
        }
        if let Some(effort) = &self.effort {
            parts.push("--effort".into());
            parts.push(shell_quote(effort));
        }
        parts.join(" ")
    }
}

/// A folder argument that keeps a leading `~` expandable by the shell.
fn folder_arg(folder: &str) -> String {
    if folder == "~" {
        return "~".into();
    }
    match folder.strip_prefix("~/") {
        Some(rest) => format!("~/{}", shell_quote(rest)),
        None => shell_quote(folder),
    }
}

/// Quote a word for a POSIX shell, leaving plain words unquoted.
pub fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./:=,+@%-".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// What a pane is actually running, read from herdr and `/proc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// The live launch of a pane's Claude worker, or `None` when none is running there.
pub fn live_launch(herdr: &Herdr, pane: &str) -> Option<Live> {
    let (pid, argv) = herdr
        .foreground_processes(pane)
        .into_iter()
        .find(|(_, argv)| argv.iter().any(|a| is_launch_token(a)))?;
    Some(Live {
        argv,
        env: read_environ(pid),
    })
}

fn is_launch_token(arg: &str) -> bool {
    let base = arg.rsplit('/').next().unwrap_or(arg);
    LAUNCH_COMMANDS.contains(&base)
}

/// The environment a process was started with, from `/proc/<pid>/environ`.
fn read_environ(pid: u32) -> BTreeMap<String, String> {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/environ")) else {
        return BTreeMap::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The value after `flag` in an argv, if present.
fn argv_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let i = argv.iter().position(|a| a == flag)?;
    argv.get(i + 1).map(String::as_str)
}

/// Differences between a roster entry and the live launch. Empty when they match.
pub fn drift(entry: &Entry, live: Option<&Live>) -> Vec<String> {
    let Some(live) = live else {
        return vec!["not running".into()];
    };
    let mut out = Vec::new();
    if let Some(want) = &entry.model {
        let got = argv_value(&live.argv, "--model");
        if got != Some(want.as_str()) {
            out.push(format!("model {} != roster {want}", got.unwrap_or("none")));
        }
    }
    if let Some(want) = &entry.effort {
        let got = argv_value(&live.argv, "--effort");
        if got != Some(want.as_str()) {
            out.push(format!("effort {} != roster {want}", got.unwrap_or("none")));
        }
    }
    for flag in &entry.flags {
        if !live.argv.iter().any(|a| a == flag) {
            out.push(format!("missing flag {flag}"));
        }
    }
    for (k, v) in &entry.env {
        match live.env.get(k) {
            Some(got) if got == v => {}
            Some(_) => out.push(format!("env {k} differs")),
            None => out.push(format!("env {k} missing")),
        }
    }
    for k in &entry.unset {
        if live.env.contains_key(k) {
            out.push(format!("env {k} should be unset"));
        }
    }
    out
}

/// The default roster location: `$XDG_CONFIG_HOME/herdr-fleet/roster.toml`, or under `~/.config`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("herdr-fleet").join("roster.toml")
}

/// The expected-loops list: `$XDG_CONFIG_HOME/herdr-fleet/expected-loops`, or under `~/.config`.
pub fn expected_loops_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("herdr-fleet").join("expected-loops")
}

/// One command fragment per line; blank lines and `#` comments are skipped.
/// A missing file means an empty list.
pub fn load_expected_loops(path: &Path) -> Result<Vec<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect())
}
