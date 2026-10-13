//! Transcript parsing tests, driven by synthetic transcript fixtures.

use std::io::Write;
use std::path::PathBuf;

use herdr_fleet::transcript::{find_transcript, pretty_model, read_info, TranscriptTracker};

const SAMPLE: &str = include_str!("fixtures/transcript_sample.jsonl");
const NOLOOP: &str = include_str!("fixtures/transcript_noloop.jsonl");
const SHARED: &str = include_str!("fixtures/transcript_sharedprefix.jsonl");

/// Lay out `<root>/<name>/<name>.jsonl`, mirroring
/// `.claude/projects/<slug>/<session-id>.jsonl`. Returns the projects root and
/// the file. Each test uses its own `name`, so its root and session id are
/// unique and no two tests write the same path.
fn write_fixture(name: &str, contents: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("hf-tt-{}-{}", std::process::id(), name));
    let projects = root.join(name);
    std::fs::create_dir_all(&projects).unwrap();
    let path = projects.join(format!("{name}.jsonl"));
    std::fs::write(&path, contents).unwrap();
    (root, path)
}

#[test]
fn reads_model_context_output_and_loop() {
    let (_, path) = write_fixture("full", SAMPLE);
    let info = read_info(&path, 1_000_000).unwrap();
    assert_eq!(info.model.as_deref(), Some("claude-opus-5-5"));
    // 2 + 179_032 + 630 tokens, over a 1M window.
    assert_eq!(info.ctx_tokens, Some(179_664));
    assert_eq!(info.ctx_percent, Some(17));
    assert_eq!(info.last_output.as_deref(), Some("I will keep digging."));
    assert_eq!(info.loop_cmd, Some(("cargo test --all".to_string(), 3)));
    // All assistant turns summed: 20 input + 895_260 cache-read + 3_150
    // cache-write at Opus 5.5 list rates ($4 / $0.40 / $5 per 1M).
    let cost = info.cost_usd.expect("a priced session");
    assert!((cost - 0.373_934).abs() < 1e-6, "cost {cost}");
    // The last tool calls, newest last.
    assert_eq!(info.tool_calls.len(), 4);
    assert_eq!(info.tool_calls.last().unwrap().name, "Bash");
    assert_eq!(info.tool_calls.last().unwrap().detail, "cargo test --all");
}

#[test]
fn no_loop_when_commands_are_all_different() {
    let (_, path) = write_fixture("noloop", NOLOOP);
    let info = read_info(&path, 1_000_000).unwrap();
    assert_eq!(info.loop_cmd, None);
    assert_eq!(info.last_output.as_deref(), Some("All good."));
}

/// IQ.3b: commands are compared in full. Five commands that share a long
/// prefix but differ afterwards must never count as one repeated command.
#[test]
fn no_loop_when_commands_share_a_prefix_but_differ() {
    let (_, path) = write_fixture("shared", SHARED);
    let info = read_info(&path, 1_000_000).unwrap();
    assert_eq!(
        info.loop_cmd, None,
        "different commands sharing a prefix are not a loop"
    );
}

#[test]
fn tracker_reads_only_appended_bytes() {
    let (_, path) = write_fixture("incr", SAMPLE);
    let mut tracker = TranscriptTracker::default();
    let first = tracker.update(&path, 1_000_000).unwrap();
    assert_eq!(first.last_output.as_deref(), Some("I will keep digging."));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":1}},"content":[{{"type":"text","text":"After the append."}}]}}}}"#
    )
    .unwrap();
    drop(f);
    let second = tracker.update(&path, 1_000_000).unwrap();
    assert_eq!(second.last_output.as_deref(), Some("After the append."));
    assert_eq!(second.ctx_tokens, Some(1));
}

#[test]
fn context_percent_uses_the_window() {
    let (_, path) = write_fixture("window", SAMPLE);
    let info = read_info(&path, 200_000).unwrap();
    // The same tokens over a 200k window: 89%.
    assert_eq!(info.ctx_percent, Some(89));
}

#[test]
fn finds_a_transcript_by_session_id() {
    let (projects, path) = write_fixture("find", SAMPLE);
    assert_eq!(find_transcript(&projects, "find"), Some(path));
    assert_eq!(find_transcript(&projects, "does-not-exist"), None);
    assert_eq!(find_transcript(&projects, ""), None);
}

#[test]
fn prettifies_claude_model_ids_only() {
    assert_eq!(pretty_model("claude-opus-5-5"), "Opus 5.5");
    assert_eq!(pretty_model("claude-haiku-5-5"), "Haiku 5.5");
    // A context-window suffix is noise for the dashboard.
    assert_eq!(pretty_model("claude-opus-5-5[1m]"), "Opus 5.5");
    assert_eq!(pretty_model("deepseek-flash"), "deepseek-flash");
    assert_eq!(pretty_model("deepseek-flash[1m]"), "deepseek-flash[1m]");
}
