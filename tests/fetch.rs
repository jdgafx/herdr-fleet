//! The background worker and the herdr call timeout, against a fake herdr whose
//! `agent read` can be made to hang. Proves: a stuck pane never blocks a key
//! action, a call is abandoned after `CALL_TIMEOUT`, and a stuck pane keeps its
//! last value marked stale.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use herdr_fleet::fetch::{Done, Worker};
use herdr_fleet::herdr::{Herdr, CALL_TIMEOUT};
use herdr_fleet::model::Options;

/// Script creation is serialised: writing an executable while another test forks
/// can fail with "text file busy" on Linux.
static SCRIPTS: Mutex<()> = Mutex::new(());

/// A fake herdr. `agent read` prints a line, and hangs for 10 s while `stuck` exists.
/// `api snapshot` reports one non-Claude agent pane, so the worker scrapes its text.
fn fake_herdr(name: &str) -> (Herdr, PathBuf) {
    let _guard = SCRIPTS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("herdr-fleet-fetch-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let stuck = dir.join("stuck");
    let script = dir.join("herdr");
    let body = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "--version") echo "herdr 9.9.9" ;;
  "api snapshot")
    echo '{{"result":{{"snapshot":{{"panes":[{{"pane_id":"w1:p1","agent":"agy","agent_status":"idle","label":"lead"}}]}}}}}}' ;;
  "agent read")
    if [ -f "{stuck}" ]; then sleep 10; fi
    echo "hello world" ;;
  "pane process-info") echo '{{"result":{{"process_info":{{"foreground_processes":[]}}}}}}' ;;
  *) echo "{{}}" ;;
esac
"#,
        stuck = stuck.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (Herdr::new(script.to_string_lossy().to_string()), stuck)
}

fn options() -> Options {
    Options {
        projects_dir: PathBuf::from("/nonexistent-projects"),
        context_window: 200_000,
        roster: None,
        quiet: Duration::from_secs(300),
        expected_loops: Vec::new(),
    }
}

/// Wait for the next fleet snapshot, failing the test after `wait`.
fn next_fleet(worker: &Worker, wait: Duration) -> Result<herdr_fleet::model::Fleet, String> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "no fleet snapshot within {wait:?}");
        match worker.recv_timeout(left) {
            Some(Done::Fleet(result)) => return result,
            Some(_) => continue,
            None => panic!("no fleet snapshot within {wait:?}"),
        }
    }
}

#[test]
fn a_hung_herdr_call_is_abandoned_at_the_timeout() {
    let (herdr, stuck) = fake_herdr("timeout");
    std::fs::write(&stuck, "").unwrap();
    let started = Instant::now();
    let err = herdr
        .read_tail("w1:p1", 60)
        .expect_err("a call that hangs must fail");
    let took = started.elapsed();
    assert!(
        took < CALL_TIMEOUT + Duration::from_millis(1500),
        "abandoned after {took:?}, expected about {CALL_TIMEOUT:?}"
    );
    assert!(format!("{err:#}").contains("timed out"), "{err:#}");
}

#[test]
fn a_key_action_answers_while_a_refresh_is_stuck() {
    let (herdr, stuck) = fake_herdr("latency");
    std::fs::write(&stuck, "").unwrap();
    let worker = Worker::spawn(herdr, None, options());

    worker.request_refresh();
    // The refresh is now waiting on a hung `agent read`. A key action must still answer at once.
    std::thread::sleep(Duration::from_millis(50));
    let pressed = Instant::now();
    worker.action(|_| Done::Note("focused w1:p1".into()));
    let answer = loop {
        match worker.recv_timeout(Duration::from_secs(1)) {
            Some(Done::Note(n)) => break (n, pressed.elapsed()),
            Some(_) => continue,
            None => panic!("the key action never answered"),
        }
    };
    assert_eq!(answer.0, "focused w1:p1");
    assert!(
        answer.1 < Duration::from_millis(50),
        "key action took {:?} while a refresh was in flight",
        answer.1
    );
}

#[test]
fn a_stuck_pane_keeps_its_last_value_marked_stale() {
    let (herdr, stuck) = fake_herdr("stale");
    let worker = Worker::spawn(herdr, None, options());

    worker.request_refresh();
    let first = next_fleet(&worker, Duration::from_secs(10)).expect("first refresh");
    assert_eq!(first.agents.len(), 1);
    assert_eq!(first.agents[0].last_line, "hello world");
    assert!(!first.agents[0].flags.iter().any(|f| f == "stale"));

    std::fs::write(&stuck, "").unwrap();
    worker.request_refresh();
    let started = Instant::now();
    let second = next_fleet(&worker, CALL_TIMEOUT + Duration::from_secs(4))
        .expect("the fleet must come back even though one pane hangs");
    assert!(started.elapsed() < CALL_TIMEOUT + Duration::from_secs(3));
    let row = &second.agents[0];
    assert_eq!(row.last_line, "hello world", "last value kept");
    assert!(row.flags.iter().any(|f| f == "stale"), "{:?}", row.flags);
}
