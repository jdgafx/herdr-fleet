//! Jump back to the fleet: the recorded pane, and the herdr key that runs it.
//!
//! `herdr-fleet` (the TUI) records its own pane on start; `herdr-fleet back`
//! focuses that pane again, so after Enter jumps you to an agent you have one
//! key back. `herdr-fleet install-keys` writes the herdr binding that runs it.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::herdr::Herdr;
use crate::model::now_epoch;

/// Marker line so the written block is recognisable and re-running is a no-op.
pub const MARKER: &str = "# added by herdr-fleet install-keys";

/// Where `herdr-fleet back` remembers its pane.
pub fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".local")
                .join("state")
        });
    base.join("herdr-fleet").join("back.json")
}

/// The herdr config.toml herdr itself would read.
pub fn default_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("HERDR_CONFIG_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
        .join("herdr")
        .join("config.toml")
}

/// The pane herdr-fleet runs in, as recorded at TUI start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetPane {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub recorded_at: u64,
}

/// Record the calling pane (the TUI's own pane) for `back` to focus later.
pub fn record_fleet_pane(herdr: &Herdr) -> Result<FleetPane> {
    let pane = herdr.pane_current()?;
    let fp = FleetPane {
        pane_id: pane.pane_id,
        workspace_id: pane.workspace_id,
        tab_id: pane.tab_id,
        recorded_at: now_epoch(),
    };
    let path = state_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&fp)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(fp)
}

/// The recorded fleet pane, if any.
pub fn read_fleet_pane() -> Result<FleetPane> {
    let path = state_path();
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no recorded fleet pane at {} — run `herdr-fleet` once in the pane you want to come back to",
            path.display()
        )
    })?;
    serde_json::from_str(&text).with_context(|| format!("bad {}", path.display()))
}

/// Focus the recorded fleet pane. The pane's agent is the precise target; a
/// plain shell pane (the TUI's normal home when no agent runs there) has none,
/// so fall back to the recorded tab, whose active pane is where the fleet ran.
/// If even that fails, move to the workspace but exit non-zero: the exact pane
/// could not be focused.
pub fn back(herdr: &Herdr) -> Result<String> {
    let fp = read_fleet_pane()?;
    let age = now_epoch().saturating_sub(fp.recorded_at);
    match herdr.focus(&fp.pane_id) {
        Ok(()) => Ok(format!("focused {} (recorded {age}s ago)", fp.pane_id)),
        Err(pane_err) => {
            if !fp.tab_id.is_empty() && herdr.focus_tab(&fp.tab_id).is_ok() {
                return Ok(format!(
                    "focused tab {} — pane {} has no agent to focus ({pane_err:#})",
                    fp.tab_id, fp.pane_id
                ));
            }
            herdr.focus_workspace(&fp.workspace_id).map_err(|workspace_err| {
                anyhow::anyhow!(
                    "focus {} failed ({pane_err:#}); tab {} and workspace {} focus failed too ({workspace_err:#})",
                    fp.pane_id,
                    fp.tab_id,
                    fp.workspace_id
                )
            })?;
            bail!(
                "focused workspace {} but not the recorded pane {} (no agent there, and tab {} focus failed: {pane_err:#})",
                fp.workspace_id,
                fp.pane_id,
                fp.tab_id
            );
        }
    }
}

/// The herdr key-binding block written by `install-keys`.
pub fn key_block(key: &str) -> String {
    format!(
        "\n{MARKER}: one key back to the herdr-fleet pane.\n[[keys.command]]\nkey = \"{key}\"\ntype = \"shell\"\ncommand = \"herdr-fleet back\"\n"
    )
}

/// Append the binding to `config` (creating it if needed), after a backup.
/// Pure file work; returns the backup path when one was made, or `None` when
/// the binding was already present or the file is new.
pub fn write_keys(config: &Path, key: &str) -> Result<Option<PathBuf>> {
    let existing = std::fs::read_to_string(config).unwrap_or_default();
    if existing.contains(MARKER) {
        return Ok(None);
    }
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let backup = if config.exists() {
        let backup = config.with_extension(format!("toml.bak-herdr-fleet-{}", now_epoch()));
        std::fs::copy(config, &backup).with_context(|| format!("back up {}", config.display()))?;
        Some(backup)
    } else {
        None
    };
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&key_block(key));
    std::fs::write(config, text).with_context(|| format!("write {}", config.display()))?;
    Ok(backup)
}

/// Restore a backup after a failed config check (or remove a file we created).
pub fn restore(config: &Path, backup: Option<&Path>) -> Result<()> {
    match backup {
        Some(b) => std::fs::copy(b, config)
            .map(|_| ())
            .with_context(|| format!("restore {}", config.display())),
        None => std::fs::remove_file(config)
            .or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .with_context(|| format!("remove {}", config.display())),
    }
}

/// Full install: write the binding, validate it with `herdr config check`
/// (only when we wrote the config herdr actually reads), and reload.
pub fn install_keys(herdr: &Herdr, config: &Path, key: &str, reload: bool) -> Result<String> {
    let mut out = String::new();
    let backup = write_keys(config, key)?;
    match &backup {
        None => out.push_str(&format!(
            "key binding already present in {} (nothing written)\n",
            config.display()
        )),
        Some(b) => out.push_str(&format!(
            "wrote the binding to {} (backup: {})\n",
            config.display(),
            b.display()
        )),
    }
    if config != default_config_path() {
        out.push_str("note: this is not the config herdr reads by default; skipping config check and reload\n");
        return Ok(out);
    }
    if let Err(e) = herdr.run(&["config", "check"]) {
        restore(config, backup.as_deref())?;
        bail!("herdr config check failed after writing — the backup was restored: {e:#}");
    }
    out.push_str("herdr config check: ok\n");
    if reload {
        match herdr.run(&["server", "reload-config"]) {
            Ok(_) => out.push_str("reloaded the running herdr config\n"),
            Err(e) => out.push_str(&format!(
                "note: reload failed ({e:#}); run `herdr server reload-config` or restart herdr\n"
            )),
        }
    }
    out.push_str(&format!(
        "press {key} inside herdr to jump back to the fleet pane\n"
    ));
    Ok(out)
}

#[cfg(test)]
mod unit {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hf-keys-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_backup_and_block_then_is_idempotent() {
        let dir = tmp_dir("write");
        let cfg = dir.join("config.toml");
        std::fs::write(&cfg, "onboarding = false\n").unwrap();
        let backup = write_keys(&cfg, "prefix+f").unwrap().expect("a backup");
        assert!(backup.exists());
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains(MARKER));
        assert!(text.contains("key = \"prefix+f\""));
        assert!(text.contains("command = \"herdr-fleet back\""));
        assert!(text.contains("onboarding = false"));
        assert!(
            write_keys(&cfg, "prefix+f").unwrap().is_none(),
            "second run is a no-op"
        );
    }

    #[test]
    fn creates_a_missing_config_without_a_backup() {
        let dir = tmp_dir("create");
        let cfg = dir.join("nested").join("config.toml");
        let backup = write_keys(&cfg, "prefix+g").unwrap();
        assert!(backup.is_none());
        assert!(std::fs::read_to_string(&cfg).unwrap().contains("prefix+g"));
    }

    #[test]
    fn restore_puts_the_backup_back() {
        let dir = tmp_dir("restore");
        let cfg = dir.join("config.toml");
        std::fs::write(&cfg, "keep = true\n").unwrap();
        let backup = write_keys(&cfg, "prefix+f").unwrap().expect("a backup");
        restore(&cfg, Some(&backup)).unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "keep = true\n");
    }
}
