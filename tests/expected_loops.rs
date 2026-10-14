//! The expected-loops allowlist: a repeating command that is meant to repeat is not flagged.

use herdr_fleet::model::is_expected_loop;
use herdr_fleet::roster::load_expected_loops;

#[test]
fn an_expected_command_is_dropped_and_others_are_kept() {
    let allow = vec!["inbox-wait.sh".to_string()];
    assert!(is_expected_loop("loop x6: inbox-wait.sh", &allow));
    assert!(!is_expected_loop("loop x5: npm test", &allow));
    assert!(
        !is_expected_loop("dialog: inbox-wait.sh", &allow),
        "only loop flags are filtered"
    );
}

#[test]
fn an_empty_list_flags_everything() {
    assert!(!is_expected_loop("loop x6: inbox-wait.sh", &[]));
}

#[test]
fn the_file_skips_comments_and_blank_lines() {
    let dir = std::env::temp_dir().join(format!("expected-loops-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("expected-loops");
    std::fs::write(
        &path,
        "# wait loops\n\n  inbox-wait.sh  \n# none yet\nwake.sh\n",
    )
    .unwrap();
    let got = load_expected_loops(&path).unwrap();
    assert_eq!(got, vec!["inbox-wait.sh", "wake.sh"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_file_means_no_expected_loops() {
    let got = load_expected_loops(std::path::Path::new(
        "/nonexistent/herdr-fleet/expected-loops",
    ))
    .unwrap();
    assert!(got.is_empty());
}
