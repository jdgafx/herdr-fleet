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

/// Commands a roster line may launch. Bare `muse` is deliberately absent (MJ):
/// auto-relaunch parses only claude/cc- launchers and cannot drive muse panes.
/// `cc-mimo` (KI) and `cc-muse` (ME) are wrappers: their `--model` flag is a
/// tier alias, and the real model comes from [`resolve_model`].
const LAUNCH_COMMANDS: [&str; 6] = [
    "claude",
    "cc-deepseek",
    "cc-luna",
    "cc-claude-luna",
    "cc-mimo",
    "cc-muse",
];

/// The model a pane really runs, given its launch command and `--model` flag.
/// Wrapper launchers fix the provider whatever the flag says (KO.2): cc-mimo
/// is always MiMo Flash; cc-muse maps the tier (opus -> wonder, everything
/// else -> spark, per ME).
pub fn resolve_model(command: &str, model_flag: Option<&str>) -> Option<String> {
    match command {
        _ if command.ends_with("cc-mimo") => Some("xiaomi/mimo-v2.6-flash".to_string()),
        _ if command.ends_with("cc-muse") => match model_flag {
            Some("opus") => Some("muse-wonder-1.0".to_string()),
            _ => Some("muse-spark-1.3-contributor".to_string()),
        },
        _ => model_flag.map(str::to_string),
    }
}

/// The launch command from an argv: the element naming a known launcher
/// (argv[0] is `env` when the pane was started with env overrides),
/// canonicalised (`muse-bin-1.4.9-*` -> `muse`).
pub fn argv_command(argv: &[String]) -> Option<&str> {
    argv.iter().find_map(|a| {
        let base = a.rsplit('/').next().unwrap_or(a);
        canonical_command(base)
    })
}

/// True for a wrapper launcher whose `--model` flag is a tier alias.
fn is_wrapper(command: &str) -> bool {
    command.ends_with("cc-mimo") || command.ends_with("cc-muse")
}

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
    /// `claude`, `cc-deepseek`, `cc-luna` (KC.3: Claude-flag drop-in on Codex Luna 6) or
    /// `cc-claude-luna` (KF: Claude Code itself, with Luna 6 behind a local proxy).
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
    try_live_launch(herdr, pane).ok().flatten()
}

/// The live launch of a pane. `Err` means herdr did not answer (for example a
/// timeout), which is not the same as "nothing is running there".
pub fn try_live_launch(herdr: &Herdr, pane: &str) -> Result<Option<Live>> {
    let Some((pid, argv)) = herdr
        .try_foreground_processes(pane)?
        .into_iter()
        .find(|(_, argv)| argv.iter().any(|a| is_launch_token(a)))
    else {
        return Ok(None);
    };
    Ok(Some(Live {
        argv,
        env: read_environ(pid),
    }))
}

/// Launchers recognised on a live pane. A superset of [`LAUNCH_COMMANDS`]:
/// bare `muse` is detected so drift tells the truth about muse-driven panes
/// ("launcher muse != roster cc-muse") instead of "not running", but it is
/// never roster-valid — auto-relaunch cannot drive muse panes (MJ).
const DETECT_COMMANDS: [&str; 7] = [
    "claude",
    "cc-deepseek",
    "cc-luna",
    "cc-claude-luna",
    "cc-mimo",
    "cc-muse",
    "muse",
];

/// The roster/drift name for a process basename: versioned `muse-bin-1.4.9-*`
/// binaries are the `muse` launcher.
fn canonical_command(base: &str) -> Option<&str> {
    if base == "muse" || base.starts_with("muse-bin") {
        return Some("muse");
    }
    DETECT_COMMANDS.iter().find(|c| **c == base).copied()
}

fn is_launch_token(arg: &str) -> bool {
    let base = arg.rsplit('/').next().unwrap_or(arg);
    canonical_command(base).is_some()
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

/// The model a live pane runs, resolved through [`resolve_model`] — for
/// display when no transcript names one (muse-driven panes, fresh wrappers).
pub fn live_model(argv: &[String]) -> Option<String> {
    let cmd = argv_command(argv)?;
    resolve_model(cmd, argv_value(argv, "--model"))
}

/// Differences between a roster entry and the live launch. Empty when they match.
pub fn drift(entry: &Entry, live: Option<&Live>) -> Vec<String> {
    let Some(live) = live else {
        return vec!["not running".into()];
    };
    let mut out = Vec::new();
    // Compare resolved models: on wrapper launchers (cc-mimo/cc-muse) the flag
    // is a tier alias, so a live `--model sonnet` on cc-mimo matches a roster
    // `cc-mimo --model sonnet` instead of flagging drift (KO.2).
    let live_cmd = argv_command(&live.argv).unwrap_or("");
    // A different launcher binary is itself drift.
    if live_cmd != entry.command {
        out.push(format!(
            "launcher {} != roster {}",
            if live_cmd.is_empty() {
                "none"
            } else {
                live_cmd
            },
            entry.command
        ));
    }
    // Compare resolved models: on wrapper launchers (cc-mimo/cc-muse) the flag
    // is a tier alias, so a live `--model sonnet` on cc-mimo matches a roster
    // `cc-mimo --model sonnet` instead of flagging drift (KO.2). A roster line
    // with no `--model` on a plain launcher pins nothing.
    if entry.model.is_some() || is_wrapper(&entry.command) {
        let want = resolve_model(&entry.command, entry.model.as_deref());
        let got = resolve_model(live_cmd, argv_value(&live.argv, "--model"));
        if want != got {
            out.push(format!(
                "model {} != roster {}",
                got.as_deref().unwrap_or("none"),
                want.as_deref().unwrap_or("none")
            ));
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
