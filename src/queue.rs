//! Optional plugin: summarise a directory of markdown checklists.
//!
//! Nothing about it is herdr- or project-specific: point `--queue-dir` at
//! any folder of `- [ ]` / `- [x]` markdown lists.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::parser::truncate;

/// One checklist file, summarised.
#[derive(Debug, Clone, Serialize)]
pub struct QueueFile {
    pub name: String,
    pub path: String,
    /// Open items, blocked ones included.
    pub open: usize,
    pub done: usize,
    /// Open items carrying a `| blocked <reason>` field (queue.py's marker).
    pub blocked: usize,
    /// The first open item that is not blocked.
    pub first_open: Option<String>,
}

/// The counts for one checklist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checklist {
    pub open: usize,
    pub done: usize,
    pub blocked: usize,
    pub first_open: Option<String>,
}

/// True when an item line carries the queue.py `| blocked <reason>` field.
pub fn is_blocked(line: &str) -> bool {
    line.contains(" | blocked ")
}

/// Count `- [ ]` (open) and `- [x]` (done) items; `first_open` is the first open item
/// that is not blocked.
pub fn checklist(text: &str) -> Checklist {
    let mut c = Checklist::default();
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- [ ]") {
            c.open += 1;
            if is_blocked(t) {
                c.blocked += 1;
            } else if c.first_open.is_none() {
                c.first_open = Some(truncate(rest, 80));
            }
        } else if t.starts_with("- [x]") || t.starts_with("- [X]") {
            c.done += 1;
        }
    }
    c
}

/// Count `- [ ]` (open) and `- [x]` (done) items, and capture the first open unblocked one.
pub fn count_checklist(text: &str) -> (usize, usize, Option<String>) {
    let c = checklist(text);
    (c.open, c.done, c.first_open)
}

/// True for a file that is not a work queue: the README and loop prompts.
pub fn is_queue_name(name: &str) -> bool {
    !name.eq_ignore_ascii_case("README.md") && !name.contains("prompt")
}

/// Summarise every `*.md` file directly inside `dir`, most-open first.
pub fn scan_queue_dir(dir: &Path) -> Vec<QueueFile> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let c = checklist(&text);
        // Only real work queues: a file with no checklist items is not one.
        if !is_queue_name(&name) || c.open + c.done == 0 {
            continue;
        }
        out.push(QueueFile {
            name,
            path: path.display().to_string(),
            open: c.open,
            done: c.done,
            blocked: c.blocked,
            first_open: c.first_open,
        });
    }
    out.sort_by(|a, b| b.open.cmp(&a.open).then_with(|| a.name.cmp(&b.name)));
    out
}
