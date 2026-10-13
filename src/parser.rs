//! Parsing of herdr pane text: the status line, and the stall/loop/dialog signals.

use serde::Serialize;

/// The trailing status line Claude Code prints at the bottom of a pane:
/// `user@host:/path | Model Name | ctx:15%`
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusLine {
    pub host: String,
    pub model: String,
    pub ctx_percent: Option<u8>,
}

/// Parse one line as a Claude Code status line, or `None` if it is not one.
///
/// Requires at least three `|`-separated fields, a first field that looks like
/// `user@host:path`, and a field that starts with `ctx:`.
pub fn parse_status_line(line: &str) -> Option<StatusLine> {
    let parts: Vec<&str> = line.split('|').map(str::trim).collect();
    if parts.len() < 3 {
        return None;
    }
    let host = parts[0];
    if !host.contains('@') {
        return None;
    }
    let ctx_idx = parts.iter().position(|p| p.starts_with("ctx:"))?;
    let ctx = parts[ctx_idx]
        .strip_prefix("ctx:")
        .and_then(|s| s.trim_end_matches('%').trim().parse::<u8>().ok());
    let model_idx = ctx_idx.checked_sub(1)?;
    let model = parts[model_idx].to_string();
    if model.is_empty() {
        return None;
    }
    Some(StatusLine {
        host: host.to_string(),
        model,
        ctx_percent: ctx,
    })
}

/// True for a horizontal rule made only of box-drawing/space characters.
pub fn is_border(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '━' | '═' | ' ' | '│'))
}

/// A Claude status line — including one the pane width has truncated before its
/// first `|`, leaving just `user@host:path…`. Those must never be shown as the
/// agent's last output.
pub fn is_status_chrome(line: &str) -> bool {
    let t = line.trim();
    if parse_status_line(t).is_some() {
        return true;
    }
    match t
        .split_whitespace()
        .next()
        .and_then(|tok| tok.split_once('@'))
    {
        Some((user, rest)) => !user.is_empty() && rest.contains(':'),
        None => false,
    }
}

/// Lines too short or too decorative to count as real output.
pub fn is_substantive(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 12
        && !is_border(t)
        && t != "❯"
        && !t.starts_with('⏵')
        && !is_status_chrome(t)
}

/// Collapse runs of whitespace so two near-identical lines compare equal.
pub fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The last line that is real output, skipping blank lines and the prompt,
/// status and hint chrome.
pub fn last_meaningful_line(lines: &[&str]) -> Option<String> {
    for line in lines.iter().rev() {
        let t = line.trim();
        // `❯` is the input box: an empty prompt, or one carrying the greyed-out
        // suggestion. Never the agent's output.
        if t.is_empty()
            || is_border(t)
            || t.starts_with('⏵')
            || t.starts_with('❯')
            || is_status_chrome(t)
        {
            continue;
        }
        return Some(t.to_string());
    }
    None
}

/// The text of an approval dialog, if the tail shows one.
///
/// Descriptive wording is preferred over a numbered menu, so the returned line
/// is the question rather than its first option.
pub fn detect_dialog(lines: &[&str]) -> Option<String> {
    const PROMPT: &[&str] = &[
        "Do you want to",
        "Enter to confirm",
        "Esc to cancel",
        "(y/n)",
        "(Y/n)",
        "(y/N)",
        "Yes, and don't ask again",
        "Press enter to",
    ];
    const MENU: &[&str] = &["❯ 1.", "❯ 1) "];
    for markers in [PROMPT, MENU] {
        for line in lines.iter().rev() {
            let t = line.trim();
            if markers.iter().any(|m| t.contains(m)) {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// A repeating line: the same substantive output seen at least `min_repeat`
/// times in the tail, preferring a submitted-command line when one repeats.
pub fn detect_loop(lines: &[&str], min_repeat: usize) -> Option<(String, usize)> {
    use std::collections::HashMap;

    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in lines {
        let t = line.trim();
        if !is_substantive(t) {
            continue;
        }
        *counts.entry(normalize_line(t)).or_insert(0) += 1;
    }

    let mut hits: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= min_repeat)
        .collect();
    // Deterministic: highest count first, then lexicographic.
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    hits.iter()
        .find(|(l, _)| l.starts_with('❯'))
        .cloned()
        .or_else(|| hits.into_iter().next())
}

/// Trim to at most `max` characters (by char count), adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        // Multi-byte input must not panic.
        assert_eq!(truncate("ααααα", 3), "αα…");
    }

    #[test]
    fn empty_status_line_is_none() {
        assert!(parse_status_line("").is_none());
        assert!(parse_status_line("a | b").is_none());
    }

    #[test]
    fn truncated_status_line_is_chrome() {
        let lines = [
            "● Real output line from the agent.",
            "  dev@box:/home/dev/project-alpha | Opus 5.5 | ctx:15%",
            "  dev@box:/home/dev/project-alph…",
            "❯ Try \"refactor tool-invoker.ts\"",
        ];
        // Both the full and the width-truncated status line are skipped, as is
        // the input box and its suggestion.
        assert_eq!(
            last_meaningful_line(&lines).as_deref(),
            Some("● Real output line from the agent.")
        );
        assert!(is_status_chrome("  dev@box:/home/dev/project-alph…"));
        assert!(!is_status_chrome("● Real output line from the agent."));
    }
}
