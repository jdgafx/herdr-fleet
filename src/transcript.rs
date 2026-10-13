//! Read a Claude Code session transcript.
//!
//! `herdr agent read` returns pane text clamped to the pane width, which cuts
//! the status line off before `model | ctx:%`. The session transcript the pane
//! is writing has the same facts in full: the model on the last assistant
//! message, its token usage, the last thing it said, and its recent Bash calls.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// How much of the end of a transcript to read. The last assistant message and
/// its recent tool calls are always within this window.
const TAIL_BYTES: u64 = 1024 * 1024;
/// Recent Bash calls considered when looking for a command loop.
const RECENT_TOOL_CALLS: usize = 30;
/// A command seen this many times in the recent window is a loop.
const LOOP_REPEATS: usize = 3;

/// What a transcript tells us about a pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptInfo {
    pub model: Option<String>,
    pub ctx_tokens: Option<u64>,
    pub ctx_percent: Option<u8>,
    pub last_output: Option<String>,
    pub loop_cmd: Option<(String, usize)>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<Usage>,
    content: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

impl Usage {
    fn ctx_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }
}

/// `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
pub fn default_projects_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("projects");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".claude").join("projects")
}

/// Find `<projects>/*/<session_id>.jsonl`.
pub fn find_transcript(projects: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    for entry in std::fs::read_dir(projects).ok()?.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Model, context usage and recent activity, read from the end of a transcript.
pub fn read_info(path: &Path, context_window: u64) -> Result<TranscriptInfo> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let len = file.metadata()?.len();
    let tail = read_tail(file, len, TAIL_BYTES)?;

    let mut info = TranscriptInfo::default();
    let mut commands: Vec<String> = Vec::new();

    for line in tail.lines() {
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        if entry.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = entry.message else {
            continue;
        };
        if message.model.is_some() {
            info.model = message.model;
        }
        if let Some(usage) = message.usage {
            info.ctx_tokens = Some(usage.ctx_tokens());
        }
        collect_content(message.content.as_ref(), &mut info, &mut commands);
    }

    if let (Some(tokens), true) = (info.ctx_tokens, context_window > 0) {
        info.ctx_percent = Some(((tokens * 100) / context_window).min(100) as u8);
    }
    info.loop_cmd = detect_loop(&commands, RECENT_TOOL_CALLS);
    Ok(info)
}

/// The last `max_bytes` of the file, as text starting at a line boundary.
fn read_tail(mut file: File, len: u64, max_bytes: u64) -> Result<String> {
    let start = len.saturating_sub(max_bytes);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(pos) = text.find('\n') {
            return Ok(text[pos + 1..].to_string());
        }
    }
    Ok(text)
}

/// Pull the last text line and the recent Bash commands out of one message.
fn collect_content(
    content: Option<&serde_json::Value>,
    info: &mut TranscriptInfo,
    commands: &mut Vec<String>,
) {
    let Some(blocks) = content.and_then(serde_json::Value::as_array) else {
        return;
    };
    for block in blocks {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                let Some(text) = block.get("text").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
                    info.last_output = Some(line.trim().to_string());
                }
            }
            Some("tool_use")
                if block.get("name").and_then(serde_json::Value::as_str) == Some("Bash") =>
            {
                let Some(cmd) = block
                    .pointer("/input/command")
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                commands.push(cmd.to_string());
            }
            _ => {}
        }
    }
}

/// The most repeated Bash command in the recent window, if any repeats enough.
fn detect_loop(commands: &[String], window: usize) -> Option<(String, usize)> {
    use std::collections::HashMap;

    let start = commands.len().saturating_sub(window);
    let mut counts: HashMap<String, usize> = HashMap::new();
    for cmd in &commands[start..] {
        let normalized = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            continue;
        }
        *counts.entry(normalized).or_insert(0) += 1;
    }
    let mut hits: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, count)| *count >= LOOP_REPEATS)
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hits.into_iter().next()
}

/// `claude-opus-5-5` -> `Opus 5.5`; anything else is shown unchanged.
pub fn pretty_model(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else {
        return model.to_string();
    };
    let parts: Vec<&str> = rest.split('-').collect();
    let Some((family, version)) = parts.split_first() else {
        return model.to_string();
    };
    if version.is_empty()
        || !version
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        return model.to_string();
    }
    let mut chars = family.chars();
    let Some(first) = chars.next() else {
        return model.to_string();
    };
    let name = first.to_uppercase().collect::<String>() + chars.as_str();
    format!("{name} {}", version.join("."))
}
