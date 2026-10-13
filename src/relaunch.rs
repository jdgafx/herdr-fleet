//! Relaunch a roster pane fresh on its roster flags, then send its `/work <queue>`.
//!
//! The exit dialog is the risky step: Claude asks what to do with background work
//! before it exits. We pick "Exit and stop tasks" only when every listed task is a
//! leftover of finished work. Anything live, or any dialog we cannot read, stops the
//! relaunch and is reported unchanged.

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::herdr::Herdr;
use crate::model::AgentInfo;
use crate::roster::{self, is_pane_id, Entry, Roster};

/// Header line of the exit dialog Claude shows when background work is running.
pub const DIALOG_HEADER: &str = "Background work is running";
/// Label of the option that stops the leftover tasks and exits.
pub const EXIT_OPTION: &str = "Exit and stop tasks";
/// Lines of pane text read for the exit dialog, both before and after `/exit`.
const EXIT_TAIL_LINES: u32 = 60;
/// Option labels the dialog offers. A numbered line is an option only when its text is
/// exactly one of these; a task that merely starts with a label is still a task.
const OPTION_LABELS: [&str; 5] = [
    EXIT_OPTION,
    "Move to background and exit",
    "Stay",
    "Keep running",
    "Cancel",
];

/// What the exit dialog says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitDialog {
    /// Only leftovers of finished work are listed; `key` selects `EXIT_OPTION`.
    Leftovers { tasks: Vec<String>, key: String },
    /// Live or unrecognised work is listed, or the exit option is missing.
    Live { tasks: Vec<String> },
}

/// Read the exit dialog from the last lines of a pane, or `None` when none is shown.
pub fn parse_exit_dialog(text: &str) -> Option<ExitDialog> {
    let lines: Vec<&str> = text.lines().collect();
    // The newest header: an older dialog left in scrollback must not be read.
    let header = lines.iter().rposition(|l| l.contains(DIALOG_HEADER))?;
    let mut tasks = Vec::new();
    let mut key = None;
    for line in &lines[header + 1..] {
        let t = trim_frame(line);
        if t.is_empty() || is_hint(t) {
            continue;
        }
        if let Some((number, label)) = option_number(t) {
            if OPTION_LABELS.contains(&label.as_str()) {
                if label == EXIT_OPTION {
                    key = Some(number);
                }
                continue;
            }
        }
        tasks.push(t.trim_start_matches(['-', '•', '*']).trim().to_string());
    }
    let all_leftover = !tasks.is_empty() && tasks.iter().all(|t| is_leftover(t));
    Some(match key {
        Some(key) if all_leftover => ExitDialog::Leftovers { tasks, key },
        _ => ExitDialog::Live { tasks },
    })
}

/// Strip the box-drawing frame and surrounding spaces from a dialog line.
fn trim_frame(line: &str) -> &str {
    // `❯` is the selection cursor on the highlighted option.
    line.trim_matches(|c: char| c.is_whitespace() || "│║┃|─═━❯".contains(c))
}

fn is_hint(t: &str) -> bool {
    let lower = t.to_lowercase();
    lower.starts_with("enter")
        || lower.contains("to cancel")
        || lower.contains("esc to")
        || lower.starts_with("the following will stop")
}

/// The number and label of a menu line such as `1. Exit and stop tasks`.
fn option_number(t: &str) -> Option<(String, String)> {
    let (num, rest) = t.split_once('.')?;
    let ok = !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' ');
    ok.then(|| (num.to_string(), rest.trim().to_string()))
}

/// A task is a leftover only when it is plainly a sleep holder (`sleep N`, no shell
/// control characters) or a poller whose line ends with a finished marker. Anything
/// else counts as live, so a command that merely contains "sleep" is never a leftover.
pub fn is_leftover(task: &str) -> bool {
    let t = task.trim();
    let words: Vec<&str> = t.split_whitespace().collect();
    let plain = !t.contains([';', '&', '|', '`', '$', '<', '>']);
    let sleep_holder = words.first() == Some(&"sleep") && plain && words.len() <= 4;
    let finished_poller = words.first().is_some_and(|w| w.contains("poll"))
        && ["(done)", "(finished)", "(exited)"].iter().any(|m| t.ends_with(m));
    sleep_holder || finished_poller
}

/// The steps a relaunch takes, as they will run. `running` says whether a Claude
/// worker is in the pane now; without one the `/exit` steps are left out.
pub fn plan_steps(entry: &Entry, running: bool) -> Vec<String> {
    let pane = &entry.pane;
    let mut steps = Vec::new();
    if running {
        steps.push(format!("herdr agent prompt {pane} /exit"));
        steps.push(format!(
            "read the pane for \"{DIALOG_HEADER}\": leftovers only -> send the option key ({EXIT_OPTION}); live or unreadable -> stop and report"
        ));
        steps.push(format!("wait until `herdr agent list` no longer shows {pane} as claude (60 s)"));
    }
    steps.push(format!("herdr pane run {pane} {}", shell_quote_arg(&entry.launch_line())));
    steps.push(format!("wait until `herdr agent list` shows {pane} as claude (60 s)"));
    steps.push(format!("herdr agent prompt {pane} '/work {}'", entry.queue));
    steps.push(format!("herdr agent wait {pane} --until working --timeout 120000"));
    steps
}

fn shell_quote_arg(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Relaunch one roster pane, or with `dry_run` print the steps and change nothing.
pub fn relaunch(herdr: &Herdr, roster: &Roster, pane: &str, dry_run: bool) -> Result<()> {
    if !is_pane_id(pane) {
        bail!("`{pane}` is not a pane id");
    }
    let Some(entry) = roster.find(pane) else {
        bail!("pane {pane} is not in the roster");
    };
    if entry.paused {
        bail!("pane {pane} ({}) is paused in the roster; refusing to relaunch", entry.queue);
    }
    let agent = claude_agent(herdr, pane)?;
    // A live process counts even when herdr's agent record has not caught up.
    let running = agent.is_some() || roster::live_launch(herdr, pane).is_some();
    // Safe only from a recorded idle or done worker, or from nothing running at all.
    // A live process with no record is of unknown state, so it is refused.
    let status = match (&agent, running) {
        (Some(a), _) => a.agent_status.clone(),
        (None, true) => "unrecorded".to_string(),
        (None, false) => "none".to_string(),
    };
    let safe_status = match &agent {
        Some(a) => matches!(a.agent_status.as_str(), "idle" | "done"),
        None => !running,
    };
    let steps = plan_steps(entry, running);
    if dry_run {
        println!("[dry-run] {pane} {} (claude running: {}, status: {status})", entry.queue, yes_no(running));
        if !safe_status {
            println!("[dry-run]   a live relaunch would refuse: status is {status}, only idle or done");
        }
        for step in &steps {
            println!("[dry-run]   {step}");
        }
        return Ok(());
    }
    if !safe_status {
        bail!("{pane} ({}) is {status}; relaunch only an idle or done worker", entry.queue);
    }

    println!("relaunch {pane} {} (claude running: {}, status: {status})", entry.queue, yes_no(running));
    if running {
        // Same window as the dialog wait below, so a dialog seen there is always a new one.
        let before = herdr.read_tail(pane, EXIT_TAIL_LINES)?;
        if before.contains(DIALOG_HEADER) {
            bail!("an exit dialog is already on screen in {pane}; nothing sent. Check the pane.");
        }
        herdr.run(&["agent", "prompt", pane, "/exit"])?;
        match wait_for_exit(herdr, pane)? {
            Exit::Gone => {}
            Exit::Dialog(ExitDialog::Leftovers { tasks, key }) => {
                println!("  exit dialog: leftovers only ({}), choosing {EXIT_OPTION}", tasks.join("; "));
                herdr.run(&["agent", "send-keys", pane, &key])?;
            }
            Exit::Dialog(ExitDialog::Live { tasks }) => {
                bail!(
                    "exit dialog lists live or unreadable work, nothing chosen: {}. Stopped; check the pane.",
                    tasks.join("; ")
                );
            }
        }
        if !wait_until(|| is_gone(herdr, pane), 60) {
            bail!("the Claude worker in {pane} did not exit within 60 s");
        }
    }

    herdr.run(&["pane", "run", pane, &entry.launch_line()])?;
    if !wait_until(|| claude_agent(herdr, pane).ok().flatten().is_some(), 60) {
        bail!("no Claude worker appeared in {pane} within 60 s");
    }
    herdr.run(&["agent", "prompt", pane, &format!("/work {}", entry.queue)])?;
    herdr.run(&["agent", "wait", pane, "--until", "working", "--timeout", "120000"])?;
    println!("  {pane} is working on {}", entry.queue);
    Ok(())
}

enum Exit {
    Gone,
    Dialog(ExitDialog),
}

/// Poll the pane until the worker is gone or its exit dialog shows.
fn wait_for_exit(herdr: &Herdr, pane: &str) -> Result<Exit> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(tail) = herdr.read_tail(pane, EXIT_TAIL_LINES) {
            if let Some(dialog) = parse_exit_dialog(&tail) {
                return Ok(Exit::Dialog(dialog));
            }
        }
        if is_gone(herdr, pane) {
            return Ok(Exit::Gone);
        }
        if Instant::now() >= deadline {
            bail!("{pane} did not exit or show an exit dialog within 30 s");
        }
        sleep(Duration::from_secs(1));
    }
}

fn wait_until(mut done: impl FnMut() -> bool, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        sleep(Duration::from_secs(1));
    }
    done()
}

/// The Claude agent record in a pane, if herdr lists one.
fn claude_agent(herdr: &Herdr, pane: &str) -> Result<Option<AgentInfo>> {
    Ok(herdr
        .agents()?
        .into_iter()
        .find(|a| a.pane_id == pane && a.agent == "claude"))
}

/// Gone means both herdr's agent record and the process are gone.
fn is_gone(herdr: &Herdr, pane: &str) -> bool {
    matches!(claude_agent(herdr, pane), Ok(None)) && roster::live_launch(herdr, pane).is_none()
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}
