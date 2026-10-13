//! Exit-dialog parsing and the relaunch plan. Dialog text here is synthetic:
//! the real dialog wording has not been captured yet (see the live proof).

use herdr_fleet::relaunch::{is_leftover, parse_exit_dialog, plan_steps, ExitDialog};
use herdr_fleet::roster::Roster;

const LEFTOVERS: &str = "\
Background work is running
  - sleep 3600 holder
  - poller (finished)
  1. Exit and stop tasks
  2. Keep running
  Esc to cancel
";

const LIVE: &str = "\
│ Background work is running                 │
│ - npm run dev (still serving)              │
│ 1. Exit and stop tasks                     │
│ 2. Keep running                            │
";

#[test]
fn leftovers_only_chooses_the_exit_option() {
    match parse_exit_dialog(LEFTOVERS) {
        Some(ExitDialog::Leftovers { tasks, key }) => {
            assert_eq!(key, "1");
            assert_eq!(tasks.len(), 2);
        }
        other => panic!("expected leftovers, got {other:?}"),
    }
}

#[test]
fn a_live_task_stops_the_relaunch() {
    match parse_exit_dialog(LIVE) {
        Some(ExitDialog::Live { tasks }) => assert_eq!(tasks, vec!["npm run dev (still serving)"]),
        other => panic!("expected live, got {other:?}"),
    }
}

#[test]
fn no_dialog_means_none() {
    assert_eq!(parse_exit_dialog("plain pane output\n> prompt"), None);
}

#[test]
fn a_dialog_without_the_exit_option_is_not_acted_on() {
    let text = "Background work is running\n  - sleep 60 holder\n  2. Keep running\n";
    assert!(matches!(parse_exit_dialog(text), Some(ExitDialog::Live { .. })));
}

#[test]
fn leftover_classification() {
    assert!(is_leftover("sleep 3600 holder"));
    assert!(is_leftover("poller (finished)"));
    // live work that merely contains the words must never count as a leftover
    assert!(!is_leftover("while true; do build; sleep 60; done"));
    assert!(!is_leftover("python worker.py --sleep-interval 5"));
    assert!(!is_leftover("polling db, 3 done"));
    assert!(!is_leftover("npm run dev"));
}

#[test]
fn one_live_task_among_leftovers_stops_the_relaunch() {
    let text = "Background work is running\n  - sleep 60 holder\n  - npm run dev\n  1. Exit and stop tasks\n  2. Keep running\n";
    assert!(matches!(parse_exit_dialog(text), Some(ExitDialog::Live { .. })));
}

#[test]
fn a_numbered_live_task_is_not_taken_for_an_option() {
    let text = "Background work is running\n  - sleep 60 holder\n  1. sleep 5; ./run.sh\n  1. Exit and stop tasks\n  2. Keep running\n";
    match parse_exit_dialog(text) {
        Some(ExitDialog::Live { tasks }) => assert!(tasks.contains(&"1. sleep 5; ./run.sh".to_string())),
        other => panic!("expected live, got {other:?}"),
    }
}

#[test]
fn a_task_that_starts_with_an_option_label_stays_a_task() {
    let text = "Background work is running\n  - sleep 60 holder\n  1. Cancel nightly job; ./run.sh\n  1. Exit and stop tasks\n  2. Keep running\n";
    match parse_exit_dialog(text) {
        Some(ExitDialog::Live { tasks }) => assert!(tasks.contains(&"1. Cancel nightly job; ./run.sh".to_string())),
        other => panic!("expected live, got {other:?}"),
    }
}

#[test]
fn the_newest_dialog_is_the_one_read() {
    let text = "Background work is running\n  - npm run dev\n  1. Exit and stop tasks\n  2. Keep running\n$ prompt\nBackground work is running\n  - sleep 60 holder\n  1. Exit and stop tasks\n  2. Keep running\n";
    assert!(matches!(parse_exit_dialog(text), Some(ExitDialog::Leftovers { .. })));
}

#[test]
fn a_paused_pane_is_refused_before_any_herdr_call() {
    let (roster, _) = Roster::import("- w1C:p4 disputes claude --model sonnet --effort high ~/x PAUSED").unwrap();
    let herdr = herdr_fleet::herdr::Herdr::new("/nonexistent/herdr");
    let err = herdr_fleet::relaunch::relaunch(&herdr, &roster, "w1C:p4", false).unwrap_err();
    assert!(err.to_string().contains("paused"), "{err}");
}

#[test]
fn plan_for_a_running_worker_starts_with_exit_and_ends_with_work() {
    let (roster, _) = Roster::import(
        "- w1F:p1 housekeeping env -u A claude --model haiku --effort medium ~",
    )
    .unwrap();
    let entry = roster.find("w1F:p1").unwrap();
    let steps = plan_steps(entry, true);
    assert_eq!(steps.first().unwrap(), "herdr agent prompt w1F:p1 /exit");
    assert!(steps.iter().any(|s| s.starts_with("herdr pane run w1F:p1 ")));
    assert_eq!(steps.last().unwrap(), "herdr agent wait w1F:p1 --until working --timeout 120000");
    assert!(steps.contains(&"herdr agent prompt w1F:p1 '/work housekeeping'".to_string()));
}

#[test]
fn plan_for_an_empty_pane_skips_the_exit() {
    let (roster, _) = Roster::import("- w1F:p1 housekeeping claude --model haiku ~").unwrap();
    let steps = plan_steps(roster.find("w1F:p1").unwrap(), false);
    assert!(!steps.iter().any(|s| s.contains("/exit")));
    assert!(steps[0].starts_with("herdr pane run w1F:p1 "));
}

/// The exit dialog as Claude Code really shows it (captured 2026-10-13; task text generalised).
const REAL_DIALOG: &str = include_str!("fixtures/exit_dialog_real.txt");

#[test]
fn the_real_dialog_lists_only_its_task_not_the_intro_or_options() {
    match parse_exit_dialog(REAL_DIALOG) {
        Some(ExitDialog::Live { tasks }) => {
            assert_eq!(tasks, vec!["subagent · example web search"]);
        }
        other => panic!("a running subagent is live work, got {other:?}"),
    }
}

#[test]
fn live_tasks_that_contain_hint_words_are_never_dropped() {
    for task in [
        "enterprise-sync --watch",
        "job-to-cancel.sh",
        "The following will stop when you exit: npm run dev",
    ] {
        let text = format!(
            "Background work is running\n  - sleep 60 holder\n  - {task}\n  1. Exit and stop tasks\n  2. Keep running\n"
        );
        match parse_exit_dialog(&text) {
            Some(ExitDialog::Live { tasks }) => assert!(tasks.contains(&task.to_string()), "{task}: {tasks:?}"),
            other => panic!("{task}: a live task must stop the relaunch, got {other:?}"),
        }
    }
}

#[test]
fn the_real_layout_with_only_leftovers_chooses_option_one() {
    let leftovers = REAL_DIALOG.replace("subagent · example web search", "sleep 3600 holder");
    match parse_exit_dialog(&leftovers) {
        Some(ExitDialog::Leftovers { tasks, key }) => {
            assert_eq!(key, "1");
            assert_eq!(tasks, vec!["sleep 3600 holder"]);
        }
        other => panic!("expected leftovers on the real layout, got {other:?}"),
    }
}
