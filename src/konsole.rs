//! Raise the Konsole window/tab that runs a herdr client (KM.3).
//!
//! herdr's own focus only moves the selection inside a herdr *client*, which usually
//! lives in another Konsole tab: without raising that tab nothing visible happens.
//! Konsole exposes org.kde.konsole-<pid> on the session bus; each /Sessions/N has a
//! foregroundProcessId (the client) and each /Windows/N can setCurrentSession.
//! The raise itself is best-effort: QWidget.raise() through dbus; on Wayland the
//! compositor has the final say, but the recent keypress usually carries activation.

use std::process::Command;

fn qdbus(args: &[&str]) -> Option<String> {
    for bin in ["qdbus6", "qdbus"] {
        if let Ok(out) = Command::new(bin).args(args).output() {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    None
}

fn comm_of(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Find a Konsole session whose foreground process is the herdr client.
/// Returns (service, session id).
fn find_herdr_session() -> Option<(String, i32)> {
    let bus = qdbus(&[])?;
    for line in bus.lines() {
        let svc = line.trim();
        if !svc.starts_with("org.kde.konsole-") {
            continue;
        }
        // Child object paths appear only in the service-root listing, not under the
        // parent path (qdbus answers the parent with its interface methods).
        let tree = qdbus(&[svc])?;
        for sline in tree.lines() {
            let path = sline.trim();
            if !path.starts_with("/Sessions/") {
                continue;
            }
            let id: i32 = path.rsplit('/').next()?.parse().ok()?;
            let fg = qdbus(&[svc, path, "org.kde.konsole.Session.foregroundProcessId"])?;
            let pid: i32 = fg.trim().parse().unwrap_or(0);
            if pid > 0 && comm_of(pid) == "herdr" {
                return Some((svc.to_string(), id));
            }
        }
    }
    None
}

/// Focus the herdr client's tab in its Konsole window and raise the window.
/// Returns a status note, or None when no herdr client tab exists on the bus.
pub fn raise_herdr_client() -> Option<String> {
    let (svc, id) = find_herdr_session()?;
    let tree = qdbus(&[&svc])?;
    let mut switched = false;
    for wline in tree.lines() {
        let wpath = wline.trim();
        if !wpath.starts_with("/Windows/") {
            continue;
        }
        qdbus(&[
            &svc,
            wpath,
            "org.kde.konsole.Window.setCurrentSession",
            &id.to_string(),
        ]);
        // Verify: a session in another window (not exported here) is a silent no-op.
        if let Some(cur) = qdbus(&[&svc, wpath, "org.kde.konsole.Window.currentSession"]) {
            if cur.trim() == id.to_string() {
                switched = true;
            }
        }
        // Best-effort window raise (X11 outright; on Wayland KWin has the final say).
        qdbus(&[&svc, wpath, "org.qtproject.Qt.QWidget.raise"]);
        qdbus(&[&svc, wpath, "org.qtproject.Qt.QWidget.showNormal"]);
    }
    if switched {
        Some(format!("raised Konsole herdr tab (session {id})"))
    } else {
        // The client exists but lives in a window Konsole does not export over dbus:
        // the tab cannot be switched from here; the caller attaches in place instead.
        None
    }
}

/// Run the herdr client in this terminal (KM.2 attach-in-place fallback).
/// The caller suspends the TUI first and redraws after we return.
pub fn attach_here(bin: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new(bin).status()
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "live dbus; run manually: cargo test -- --ignored konsole"]
    fn live_find_and_raise() {
        match super::find_herdr_session() {
            Some((svc, id)) => println!("found {svc} session {id}"),
            None => println!("no herdr client on dbus"),
        }
        println!("raise -> {:?}", super::raise_herdr_client());
    }
}
