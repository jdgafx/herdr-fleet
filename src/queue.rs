//! Optional plugin: summarise a directory of markdown checklists.
//!
//! Nothing about it is herdr- or CC-MAESTRO-specific: point `--queue-dir` at
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
    pub open: usize,
    pub done: usize,
    pub first_open: Option<String>,
}

/// Count `- [ ]` (open) and `- [x]` (done) items, and capture the first open one.
pub fn count_checklist(text: &str) -> (usize, usize, Option<String>) {
    let mut open = 0;
    let mut done = 0;
    let mut first = None;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- [ ]") {
            open += 1;
            if first.is_none() {
                first = Some(truncate(rest, 80));
            }
        } else if t.starts_with("- [x]") || t.starts_with("- [X]") {
            done += 1;
        }
    }
    (open, done, first)
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
        let (open, done, first_open) = count_checklist(&text);
        out.push(QueueFile {
            name: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string(),
            path: path.display().to_string(),
            open,
            done,
            first_open,
        });
    }
    out.sort_by(|a, b| b.open.cmp(&a.open).then_with(|| a.name.cmp(&b.name)));
    out
}
