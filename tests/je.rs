//! §JE fixes: stall vs a growing transcript, waiting flag, roster name, real queues and blocked items.
//! All fixtures are synthetic.

use std::time::Duration;

use herdr_fleet::model::{is_stalled, queue_for, roster_name, waiting_reason};
use herdr_fleet::queue::{checklist, count_checklist, is_queue_name, scan_queue_dir, QueueFile};

const MIN: u64 = 60;

fn mins(m: u64) -> Duration {
    Duration::from_secs(m * MIN)
}

#[test]
fn a_working_pane_with_a_growing_transcript_is_not_stalled() {
    // 20 minutes in state, past the 15-minute stall, but the transcript changed 2 minutes ago.
    assert!(!is_stalled("working", mins(20), mins(15), Some(mins(2)), mins(10)));
}

#[test]
fn a_working_pane_silent_for_the_quiet_window_is_stalled() {
    assert!(is_stalled("working", mins(20), mins(15), Some(mins(11)), mins(10)));
}

#[test]
fn a_working_pane_with_no_transcript_is_stalled_on_state_time_alone() {
    assert!(is_stalled("working", mins(20), mins(15), None, mins(10)));
}

#[test]
fn no_stall_before_the_stall_time_or_when_not_working() {
    assert!(!is_stalled("working", mins(5), mins(15), None, mins(10)));
    assert!(!is_stalled("idle", mins(60), mins(15), None, mins(10)));
}

#[test]
fn an_idle_pane_whose_last_output_asks_for_a_decision_is_waiting() {
    let shown = "**Decision for you:** Q5 and Q6 should be closed as superseded";
    assert_eq!(waiting_reason("idle", shown, 0).as_deref(), Some("waiting: decision"));
    assert_eq!(
        waiting_reason("done", "FOR CHRIS: approve the subdomain links", 0).as_deref(),
        Some("waiting: decision")
    );
    assert_eq!(waiting_reason("idle", "Which one should I take?", 0).as_deref(), Some("waiting: decision"));
}

#[test]
fn an_idle_pane_with_open_unblocked_items_is_waiting() {
    assert_eq!(waiting_reason("idle", "all good", 3).as_deref(), Some("waiting: 3 open"));
    assert_eq!(waiting_reason("idle", "all good", 0), None);
}

#[test]
fn a_working_pane_is_never_waiting() {
    assert_eq!(waiting_reason("working", "Decision for you: yes?", 4), None);
}

#[test]
fn the_roster_queue_name_replaces_the_terminal_name() {
    assert_eq!(roster_name("housekeeping", "CC-MAESTRO user dec..."), "housekeeping");
    assert_eq!(roster_name("", "Claude Code"), "Claude Code");
}

fn queue(name: &str, open: usize, blocked: usize) -> QueueFile {
    QueueFile {
        name: name.into(),
        path: String::new(),
        open,
        done: 0,
        blocked,
        first_open: None,
    }
}

#[test]
fn a_pane_is_matched_to_its_queue_file_by_stem() {
    let queues = vec![queue("conductor.md", 2, 0), queue("housekeeping.md", 4, 1)];
    assert_eq!(queue_for(&queues, "housekeeping", "Claude Code").map(|q| q.open), Some(4));
    assert!(queue_for(&queues, "Claude Code", "Claude Code").is_none());
}

#[test]
fn blocked_items_are_counted_and_skipped_for_the_first_open_item() {
    let text = "- [ ] Q1 | first | blocked waiting on Chris\n- [ ] Q2 | second\n- [x] Q3 | done\n";
    let c = checklist(text);
    assert_eq!((c.open, c.done, c.blocked), (2, 1, 1));
    assert_eq!(c.first_open.as_deref(), Some("Q2 | second"));
}

#[test]
fn the_count_helper_keeps_its_old_shape() {
    let (open, done, first) = count_checklist("- [ ] Q1 | a\n- [x] Q2 | b\n");
    assert_eq!((open, done), (1, 1));
    assert_eq!(first.as_deref(), Some("Q1 | a"));
}

#[test]
fn readme_and_loop_prompts_are_not_queues() {
    assert!(!is_queue_name("README.md"));
    assert!(!is_queue_name("loop-prompt.md"));
    assert!(is_queue_name("housekeeping.md"));
}

#[test]
fn the_queue_panel_lists_only_real_queues() {
    let dir = std::env::temp_dir().join(format!("je-queues-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("housekeeping.md"), "- [ ] Q1 | a | blocked waiting\n- [ ] Q2 | b\n").unwrap();
    std::fs::write(dir.join("README.md"), "- [ ] looks like an item but is a README\n").unwrap();
    std::fs::write(dir.join("loop-prompt.md"), "- [ ] a prompt, not a queue\n").unwrap();
    std::fs::write(dir.join("notes.md"), "just prose, no checklist\n").unwrap();
    let found = scan_queue_dir(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "housekeeping.md");
    assert_eq!((found[0].open, found[0].blocked), (2, 1));
    assert_eq!(found[0].first_open.as_deref(), Some("Q2 | b"));
}
