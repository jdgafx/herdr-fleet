//! Thin wrapper over the `herdr` CLI (its socket JSON API).

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::model::AgentInfo;

/// Longest a single herdr call may run before it is killed and reported as failed.
/// A slow call must not stall the dashboard: its pane keeps its last value instead.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// A handle to the herdr executable.
#[derive(Clone)]
pub struct Herdr {
    bin: String,
}

/// The pane herdr resolves as the caller's own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneCurrent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
}

impl Herdr {
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    /// The executable this handle runs (the TUI uses it for attach-in-place).
    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// Run herdr with `args`, returning stdout on success. Fails after `CALL_TIMEOUT`.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let cmd = format!("{} {}", self.bin, args.join(" "));
        let mut child = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to run `{cmd}`"))?;
        let stdout = pipe_reader(child.stdout.take());
        let stderr = pipe_reader(child.stderr.take());
        let deadline = Instant::now() + CALL_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().context("waiting on herdr")? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("`{cmd}` timed out after {}s", CALL_TIMEOUT.as_secs());
            }
            thread::sleep(Duration::from_millis(5));
        };
        let out = stdout.join().unwrap_or_default();
        if !status.success() {
            let err = stderr.join().unwrap_or_default();
            bail!(
                "`{cmd}` exited with {status}: {}",
                String::from_utf8_lossy(&err).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn json(&self, args: &[&str]) -> Result<Value> {
        let s = self.run(args)?;
        serde_json::from_str(&s)
            .with_context(|| format!("invalid JSON from `{} {}`", self.bin, args.join(" ")))
    }

    /// The running herdr version, or "unknown".
    pub fn version(&self) -> String {
        self.run(&["--version"])
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).map(str::to_string))
            .unwrap_or_else(|| "unknown".into())
    }

    /// Agent panes from the full snapshot, if it carries any.
    fn snapshot_agents(&self) -> Option<Vec<AgentInfo>> {
        let value = self.json(&["api", "snapshot"]).ok()?;
        let panes = value
            .pointer("/result/snapshot/panes")
            .and_then(Value::as_array)?;
        let agents: Vec<AgentInfo> = panes
            .iter()
            .filter_map(|p| serde_json::from_value::<AgentInfo>(p.clone()).ok())
            .filter(|a| !a.agent.is_empty() && !a.pane_id.is_empty())
            .collect();
        if agents.is_empty() {
            None
        } else {
            Some(agents)
        }
    }

    /// Every agent pane. Prefers the full snapshot (it carries pane labels);
    /// falls back to `herdr agent list`.
    pub fn agents(&self) -> Result<Vec<AgentInfo>> {
        if let Some(agents) = self.snapshot_agents() {
            return Ok(agents);
        }
        let value = self.json(&["agent", "list"])?;
        let arr = value
            .pointer("/result/agents")
            .and_then(Value::as_array)
            .context("unexpected `herdr agent list` shape")?;
        arr.iter()
            .map(|item| serde_json::from_value(item.clone()).context("bad agent entry"))
            .collect()
    }

    /// The calling pane: herdr injects `HERDR_*` into managed panes, and the
    /// CLI can resolve it from the process tree when they are absent.
    pub fn pane_current(&self) -> Result<PaneCurrent> {
        let from_env = std::env::var("HERDR_PANE_ID").unwrap_or_default();
        if !from_env.is_empty() {
            return Ok(PaneCurrent {
                pane_id: from_env,
                workspace_id: std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default(),
                tab_id: std::env::var("HERDR_TAB_ID").unwrap_or_default(),
            });
        }
        let value = self.json(&["pane", "current", "--current"])?;
        let pane = value
            .pointer("/result/pane")
            .context("unexpected `herdr pane current` shape")?;
        let get = |k: &str| {
            pane.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let out = PaneCurrent {
            pane_id: get("pane_id"),
            workspace_id: get("workspace_id"),
            tab_id: get("tab_id"),
        };
        if out.pane_id.is_empty() {
            bail!("`herdr pane current` returned no pane id");
        }
        Ok(out)
    }

    /// The `--model` value the pane's foreground process was launched with,
    /// if any. For a Claude pane with no assistant turn yet, this is the only
    /// honest answer to "which model is this?".
    pub fn launch_model(&self, pane: &str) -> Option<String> {
        let value = self.json(&["pane", "process-info", "--pane", pane]).ok()?;
        let procs = value
            .pointer("/result/process_info/foreground_processes")?
            .as_array()?;
        for p in procs {
            let argv = p.get("argv")?.as_array()?;
            for (i, a) in argv.iter().enumerate() {
                if a.as_str() == Some("--model") {
                    if let Some(m) = argv.get(i + 1).and_then(Value::as_str) {
                        return Some(m.to_string());
                    }
                }
            }
        }
        None
    }

    /// The foreground processes of a pane as (pid, argv), for the drift check.
    pub fn foreground_processes(&self, pane: &str) -> Vec<(u32, Vec<String>)> {
        self.try_foreground_processes(pane).unwrap_or_default()
    }

    /// Like `foreground_processes`, but a failed or timed-out call is an error,
    /// so the caller can tell "no process" from "herdr did not answer".
    pub fn try_foreground_processes(&self, pane: &str) -> Result<Vec<(u32, Vec<String>)>> {
        let value = self.json(&["pane", "process-info", "--pane", pane])?;
        let Some(procs) = value
            .pointer("/result/process_info/foreground_processes")
            .and_then(Value::as_array)
        else {
            return Ok(Vec::new());
        };
        Ok(procs
            .iter()
            .filter_map(|p| {
                let pid = u32::try_from(p.get("pid")?.as_u64()?).ok()?;
                let argv = p
                    .get("argv")?
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect();
                Some((pid, argv))
            })
            .collect())
    }

    /// The last `lines` lines of a pane's terminal output, as plain text.
    pub fn read_tail(&self, pane: &str, lines: u32) -> Result<String> {
        self.run(&[
            "agent",
            "read",
            pane,
            "--lines",
            &lines.to_string(),
            "--format",
            "text",
        ])
    }

    /// Focus a pane in herdr.
    pub fn focus(&self, pane: &str) -> Result<()> {
        self.run(&["agent", "focus", pane]).map(|_| ())
    }

    /// Focus the tab a pane lives in (fallback when the pane's agent is gone).
    pub fn focus_tab(&self, tab: &str) -> Result<()> {
        self.run(&["tab", "focus", tab]).map(|_| ())
    }

    /// Focus the workspace a pane lives in (last-resort fallback).
    pub fn focus_workspace(&self, workspace: &str) -> Result<()> {
        self.run(&["workspace", "focus", workspace]).map(|_| ())
    }

    /// Submit a prompt to a pane. Called only on an explicit key press.
    pub fn prompt(&self, pane: &str, text: &str) -> Result<()> {
        self.run(&["agent", "prompt", pane, text]).map(|_| ())
    }

    /// A herdr desktop notification. Used only with `--alerts`.
    pub fn notify(&self, title: &str, body: &str) -> Result<()> {
        self.run(&[
            "notification",
            "show",
            title,
            "--body",
            body,
            "--sound",
            "request",
        ])
        .map(|_| ())
    }
}

/// Drain a child's pipe on its own thread, so a full pipe never blocks the child.
fn pipe_reader<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}
