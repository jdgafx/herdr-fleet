//! Thin wrapper over the `herdr` CLI (its socket JSON API).

use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::model::AgentInfo;

/// A handle to the herdr executable.
#[derive(Clone)]
pub struct Herdr {
    bin: String,
}

impl Herdr {
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    /// Run herdr with `args`, returning stdout on success.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .with_context(|| format!("failed to run `{} {}`", self.bin, args.join(" ")))?;
        if !out.status.success() {
            bail!(
                "`{} {}` exited with {}: {}",
                self.bin,
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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

    /// Submit a prompt to a pane. Called only on an explicit key press.
    pub fn prompt(&self, pane: &str, text: &str) -> Result<()> {
        self.run(&["agent", "prompt", pane, text]).map(|_| ())
    }
}
