//! Roster parsing, launch lines and the drift check. Fixtures are synthetic.

use std::collections::BTreeMap;

use herdr_fleet::roster::{drift, parse_line, shell_quote, Entry, Live, Roster};

const HOUSEKEEPING: &str = "- w1F:p1 housekeeping env -u ANTHROPIC_API_KEY -u ANTHROPIC_BASE_URL \
CLAUDE_CODE_SUBAGENT_MODEL=haiku CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1 claude \
--dangerously-skip-permissions --model haiku --effort medium   ~ (home) (JC)";

fn argv(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

fn housekeeping() -> Entry {
    parse_line(HOUSEKEEPING.trim_start_matches("- "))
        .unwrap()
        .unwrap()
}

fn live(args: &str, env: &[(&str, &str)]) -> Live {
    Live {
        argv: argv(args),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn parses_a_worker_line() {
    let e = housekeeping();
    assert_eq!(e.pane, "w1F:p1");
    assert_eq!(e.queue, "housekeeping");
    assert_eq!(e.command, "claude");
    assert_eq!(e.model.as_deref(), Some("haiku"));
    assert_eq!(e.effort.as_deref(), Some("medium"));
    assert_eq!(e.flags, vec!["--dangerously-skip-permissions"]);
    assert_eq!(e.unset, vec!["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"]);
    assert_eq!(
        e.env
            .get("CLAUDE_CODE_DISABLE_ADVISOR_TOOL")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(e.folder, "~");
    assert!(!e.paused);
}

#[test]
fn builds_the_launch_line_from_the_entry() {
    assert_eq!(
        housekeeping().launch_line(),
        "cd ~ && env -u ANTHROPIC_API_KEY -u ANTHROPIC_BASE_URL \
CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1 CLAUDE_CODE_SUBAGENT_MODEL=haiku claude \
--dangerously-skip-permissions --model haiku --effort medium"
    );
}

#[test]
fn a_line_without_a_command_is_not_a_worker() {
    let line = "w1D:pM supervisor opus medium ~/dev/some-repo (this pane)";
    assert_eq!(parse_line(line).unwrap(), None);
}

#[test]
fn a_paused_line_is_marked_paused() {
    let line = "w1C:p4 pme-disputes claude --model sonnet --effort high ~/dev/some-folder PAUSED";
    let e = parse_line(line).unwrap().unwrap();
    assert!(e.paused);
    assert_eq!(e.folder, "~/dev/some-folder");
}

#[test]
fn an_unknown_token_is_an_error_not_a_guess() {
    let line = "w1C:p9 odd claude --model haiku --mystery-flag-with-value x ~/dev";
    assert!(parse_line(line).is_err());
}

#[test]
fn import_reports_skipped_panes() {
    let text = format!("# Roster\n{HOUSEKEEPING}\n- w1D:pM supervisor opus medium ~/dev/x (this pane)\n- Not a pane line\n");
    let (roster, skipped) = Roster::import(&text).unwrap();
    assert_eq!(roster.panes.len(), 1);
    assert_eq!(skipped, vec!["w1D:pM"]);
}

#[test]
fn roster_round_trips_through_toml() {
    let (roster, _) = Roster::import(HOUSEKEEPING).unwrap();
    let text = toml::to_string_pretty(&roster).unwrap();
    let back: Roster = toml::from_str(&text).unwrap();
    assert_eq!(back, roster);
    assert_eq!(back.find("w1F:p1").unwrap().queue, "housekeeping");
}

#[test]
fn no_drift_when_the_live_launch_matches() {
    let e = housekeeping();
    let l = live(
        "claude --dangerously-skip-permissions --model haiku --effort medium",
        &[
            ("CLAUDE_CODE_SUBAGENT_MODEL", "haiku"),
            ("CLAUDE_CODE_DISABLE_ADVISOR_TOOL", "1"),
        ],
    );
    assert!(drift(&e, Some(&l)).is_empty());
}

#[test]
fn drift_names_each_difference() {
    let e = housekeeping();
    let l = live(
        "claude --model opus --effort medium",
        &[
            ("CLAUDE_CODE_SUBAGENT_MODEL", "opus"),
            ("ANTHROPIC_API_KEY", "set"),
        ],
    );
    let d = drift(&e, Some(&l));
    assert!(
        d.contains(&"model opus != roster haiku".to_string()),
        "{d:?}"
    );
    assert!(
        d.contains(&"missing flag --dangerously-skip-permissions".to_string()),
        "{d:?}"
    );
    assert!(
        d.contains(&"env CLAUDE_CODE_SUBAGENT_MODEL differs".to_string()),
        "{d:?}"
    );
    assert!(
        d.contains(&"env CLAUDE_CODE_DISABLE_ADVISOR_TOOL missing".to_string()),
        "{d:?}"
    );
    assert!(
        d.contains(&"env ANTHROPIC_API_KEY should be unset".to_string()),
        "{d:?}"
    );
}

#[test]
fn a_pane_with_no_worker_is_drift() {
    assert_eq!(drift(&housekeeping(), None), vec!["not running"]);
}

#[test]
fn shell_quote_leaves_plain_words_and_quotes_the_rest() {
    assert_eq!(shell_quote("haiku"), "haiku");
    assert_eq!(shell_quote("it's here"), "'it'\\''s here'");
}

#[test]
fn drift_reports_a_wrong_effort() {
    let e = housekeeping();
    let l = live(
        "claude --dangerously-skip-permissions --model haiku --effort high",
        &[
            ("CLAUDE_CODE_SUBAGENT_MODEL", "haiku"),
            ("CLAUDE_CODE_DISABLE_ADVISOR_TOOL", "1"),
        ],
    );
    assert_eq!(drift(&e, Some(&l)), vec!["effort high != roster medium"]);
}

#[test]
fn a_hand_edited_command_is_rejected_on_load() {
    let dir = std::env::temp_dir().join(format!("roster-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roster.toml");
    std::fs::write(
        &path,
        "[[pane]]\npane = \"w1F:p1\"\nqueue = \"q\"\ncommand = \"rm\"\n",
    )
    .unwrap();
    let err = Roster::load(&path).unwrap_err();
    assert!(err.to_string().contains("not one of"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}
