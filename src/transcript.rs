//! Read Claude Code session transcripts, incrementally.
//!
//! `herdr agent read` returns pane text clamped to the pane width, which cuts
//! the status line off before `model | ctx:%`. The session transcript the pane
//! is writing has the same facts in full: the model on the latest assistant
//! message, its token usage, the last thing it said, and its recent tool calls.
//!
//! `TranscriptTracker` reads each file once and then only the bytes appended
//! since the last call, so a TUI refresh never re-parses a long session.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::price;

/// Bash calls kept when looking for a command loop.
pub const RECENT_TOOL_CALLS: usize = 30;
/// A command seen this many times in the recent window is a loop.
const LOOP_REPEATS: usize = 3;
/// Tool calls kept for the detail view.
const TOOL_CALLS_KEPT: usize = 8;

/// One recent tool call, for the detail view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub detail: String,
}

/// What a transcript tells us about a pane.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptInfo {
    pub model: Option<String>,
    pub ctx_tokens: Option<u64>,
    pub ctx_percent: Option<u8>,
    pub last_output: Option<String>,
    pub loop_cmd: Option<(String, usize)>,
    /// Estimated session spend at list prices, all assistant turns summed.
    pub cost_usd: Option<f64>,
    /// The most recent tool calls, oldest first.
    pub tool_calls: Vec<ToolCall>,
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

#[derive(Deserialize, Default, Clone, Copy)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl Usage {
    fn ctx_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }

    fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Incremental transcript reader. Keep one for the life of the process and
/// call `update` per refresh; each file is parsed at most once.
#[derive(Default)]
pub struct TranscriptTracker {
    files: HashMap<PathBuf, Tracked>,
}

#[derive(Default)]
struct Tracked {
    offset: u64,
    model: Option<String>,
    ctx_tokens: Option<u64>,
    last_output: Option<String>,
    commands: VecDeque<String>,
    tools: VecDeque<ToolCall>,
    totals: HashMap<String, Usage>,
}

impl TranscriptTracker {
    /// Facts for one transcript, reading only what is new since the last call.
    pub fn update(&mut self, path: &Path, context_window: u64) -> Result<TranscriptInfo> {
        let tracked = self.files.entry(path.to_path_buf()).or_default();
        tracked.read_new(path)?;
        Ok(tracked.info(context_window))
    }

    /// Forget files that are no longer live pane transcripts.
    pub fn retain(&mut self, live: &[PathBuf]) {
        self.files.retain(|p, _| live.contains(p));
    }
}

impl Tracked {
    fn read_new(&mut self, path: &Path) -> Result<()> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let len = file.metadata()?.len();
        if len < self.offset {
            // Truncated or replaced: start over.
            *self = Tracked::default();
        }
        if len == self.offset {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let text = String::from_utf8_lossy(&buf);
        let Some(end) = text.rfind('\n').map(|i| i + 1) else {
            // No complete line yet; retry when more is appended.
            return Ok(());
        };
        self.offset += end as u64;
        for line in text[..end].lines() {
            self.consume(line);
        }
        Ok(())
    }

    fn consume(&mut self, line: &str) {
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            return;
        };
        if entry.kind.as_deref() != Some("assistant") {
            return;
        }
        let Some(message) = entry.message else {
            return;
        };
        if let Some(model) = message.model.filter(|m| !m.is_empty()) {
            self.model = Some(model);
        }
        if let Some(usage) = message.usage {
            if let Some(model) = self.model.clone() {
                self.totals.entry(model).or_default().add(&usage);
            }
            self.ctx_tokens = Some(usage.ctx_tokens());
        }
        if let Some(content) = message.content.as_ref() {
            self.consume_content(content);
        }
    }

    fn consume_content(&mut self, content: &serde_json::Value) {
        let Some(blocks) = content.as_array() else {
            return;
        };
        for block in blocks {
            match block.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => {
                    let Some(text) = block.get("text").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
                        self.last_output = Some(line.trim().to_string());
                    }
                }
                Some("tool_use") => {
                    let name = block
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    let input = block.get("input");
                    if name == "Bash" {
                        if let Some(cmd) = input
                            .and_then(|i| i.get("command"))
                            .and_then(serde_json::Value::as_str)
                        {
                            push_capped(&mut self.commands, cmd.to_string(), RECENT_TOOL_CALLS);
                        }
                    }
                    let detail = tool_detail(input);
                    push_capped(&mut self.tools, ToolCall { name, detail }, TOOL_CALLS_KEPT);
                }
                _ => {}
            }
        }
    }

    fn info(&self, context_window: u64) -> TranscriptInfo {
        let mut info = TranscriptInfo {
            model: self.model.clone(),
            ctx_tokens: self.ctx_tokens,
            ctx_percent: None,
            last_output: self.last_output.clone(),
            loop_cmd: detect_loop(&self.commands),
            cost_usd: None,
            tool_calls: self.tools.iter().cloned().collect(),
        };
        if let (Some(tokens), true) = (info.ctx_tokens, context_window > 0) {
            info.ctx_percent = Some(((tokens * 100) / context_window).min(100) as u8);
        }
        let mut cost = 0.0;
        let mut priced = false;
        for (model, usage) in &self.totals {
            if let Some(c) = price::cost_usd(
                model,
                usage.input_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
                usage.output_tokens,
            ) {
                cost += c;
                priced = true;
            }
        }
        if priced {
            info.cost_usd = Some(cost);
        }
        info
    }
}

fn push_capped<T>(dq: &mut VecDeque<T>, v: T, cap: usize) {
    dq.push_back(v);
    while dq.len() > cap {
        dq.pop_front();
    }
}

/// A one-line summary of a tool call: the command, the file, or another short
/// string from the input.
fn tool_detail(input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
    ] {
        if let Some(v) = input.get(key).and_then(serde_json::Value::as_str) {
            return v.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }
    String::new()
}

/// One-shot read of a whole transcript (tests and `list`). The TUI keeps a
/// [`TranscriptTracker`] instead, so it only reads what is new.
pub fn read_info(path: &Path, context_window: u64) -> Result<TranscriptInfo> {
    let mut tracker = TranscriptTracker::default();
    tracker.update(path, context_window)
}

/// The most repeated Bash command in the recent window, if any repeats enough.
///
/// Commands are compared in full — whitespace-normalised, but nothing is
/// truncated or prefix-matched — so five different commands that happen to
/// share a prefix can never count as one.
fn detect_loop(commands: &VecDeque<String>) -> Option<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for cmd in commands {
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

/// `claude-opus-5-5` -> `Opus 5.5` (a `[1m]` suffix is dropped);
/// anything else is shown unchanged.
pub fn pretty_model(model: &str) -> String {
    let base = match model.rfind('[') {
        Some(i) if model.ends_with(']') => &model[..i],
        _ => model,
    };
    let Some(rest) = base.strip_prefix("claude-") else {
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
