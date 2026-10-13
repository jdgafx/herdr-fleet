//! Parser tests driven by captured herdr pane text and JSON (redacted).

use herdr_fleet::model::AgentInfo;
use herdr_fleet::parser::{detect_dialog, detect_loop, last_meaningful_line, parse_status_line};
use herdr_fleet::queue::count_checklist;

const WORKING: &str = include_str!("fixtures/pane_status_working.txt");
const DIALOG: &str = include_str!("fixtures/pane_status_dialog.txt");
const LOOP: &str = include_str!("fixtures/pane_status_loop.txt");
const AGENT_LIST: &str = include_str!("fixtures/agent_list.json");
const QUEUE: &str = include_str!("fixtures/queue_sample.md");

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[test]
fn parses_status_line_from_real_tail() {
    let text = lines(WORKING);
    let status = text
        .iter()
        .rev()
        .find_map(|l| parse_status_line(l))
        .expect("a status line");
    assert_eq!(status.model, "Opus 5.5");
    assert_eq!(status.ctx_percent, Some(15));
    assert!(status.host.contains('@'));
}

#[test]
fn ignores_lines_that_are_not_status_lines() {
    assert!(parse_status_line("just some | text | here").is_none());
    assert!(parse_status_line("● Done.").is_none());
    assert!(parse_status_line("").is_none());
}

#[test]
fn detects_an_approval_dialog() {
    let dialog = detect_dialog(&lines(DIALOG)).expect("dialog");
    assert!(dialog.contains("Do you want to proceed"), "got: {dialog}");
}

#[test]
fn no_dialog_in_a_normal_tail() {
    assert!(detect_dialog(&lines(WORKING)).is_none());
}

#[test]
fn detects_a_repeated_command_loop() {
    let (line, count) = detect_loop(&lines(LOOP), 3).expect("loop");
    assert_eq!(count, 3);
    assert!(line.contains("cargo test --all"), "got: {line}");
}

#[test]
fn no_loop_in_a_normal_tail() {
    assert!(detect_loop(&lines(WORKING), 3).is_none());
}

#[test]
fn last_meaningful_line_skips_prompt_and_status() {
    let last = last_meaningful_line(&lines(WORKING)).expect("a line");
    assert!(
        !last.contains("ctx:"),
        "should skip the status line: {last}"
    );
    assert!(!last.starts_with('⏵'));
    assert!(!last.starts_with('❯'));
}

#[test]
fn parses_agent_list_json() {
    let value: serde_json::Value = serde_json::from_str(AGENT_LIST).unwrap();
    let agents = value
        .pointer("/result/agents")
        .and_then(|x| x.as_array())
        .unwrap();
    assert_eq!(agents.len(), 3);
    let first: AgentInfo = serde_json::from_value(agents[0].clone()).unwrap();
    assert_eq!(first.pane_id, "w1A:p1");
    assert_eq!(first.agent_status, "working");
    assert!(first.focused);
}

#[test]
fn counts_checklist_items() {
    let (open, done, first) = count_checklist(QUEUE);
    assert_eq!(open, 2);
    assert_eq!(done, 1);
    assert!(first.unwrap().contains("second item"));
}
