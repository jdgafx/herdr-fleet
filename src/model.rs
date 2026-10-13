//! Fleet collection: turn herdr state plus pane text into renderable rows.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use crate::herdr::Herdr;
use crate::parser::{self, truncate};
use crate::queue::{scan_queue_dir, QueueFile};

/// One agent pane as herdr reports it. Unknown fields are ignored, so both
/// `herdr agent list` entries and `herdr api snapshot` panes deserialise here.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub agent_status: String,
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub foreground_cwd: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub terminal_title: String,
    #[serde(default)]
    pub terminal_title_stripped: String,
    #[serde(default)]
    pub completion_seq: i64,
    #[serde(default)]
    pub state_change_seq: i64,
}

/// One rendered fleet row.
#[derive(Debug, Clone, Serialize)]
pub struct AgentRow {
    pub pane_id: String,
    pub label: String,
    pub name: String,
    pub status: String,
    pub model: Option<String>,
    pub ctx_percent: Option<u8>,
    pub cwd: String,
    pub focused: bool,
    pub last_line: String,
    pub flags: Vec<String>,
    pub state_change_seq: i64,
}

impl AgentRow {
    fn from_info(herdr: &Herdr, info: &AgentInfo) -> AgentRow {
        let tail = herdr.read_tail(&info.pane_id, 60).unwrap_or_default();
        let lines: Vec<&str> = tail.lines().collect();

        let status_line = lines
            .iter()
            .rev()
            .find_map(|l| parser::parse_status_line(l));
        let (model, ctx_percent) = match status_line {
            Some(s) => (Some(s.model).filter(|m| !m.is_empty()), s.ctx_percent),
            None => (None, None),
        };

        let mut flags = Vec::new();
        if let Some(dialog) = parser::detect_dialog(&lines) {
            flags.push(format!("dialog: {}", truncate(&dialog, 48)));
        }
        if let Some((line, count)) = parser::detect_loop(&lines, 3) {
            flags.push(format!("loop x{count}: {}", truncate(&line, 44)));
        }

        let name = if !info.terminal_title_stripped.is_empty() {
            info.terminal_title_stripped.clone()
        } else if !info.terminal_title.is_empty() {
            info.terminal_title.clone()
        } else if !info.label.is_empty() {
            info.label.clone()
        } else {
            info.agent.clone()
        };

        AgentRow {
            pane_id: info.pane_id.clone(),
            label: info.label.clone(),
            name,
            status: info.agent_status.clone(),
            model,
            ctx_percent,
            cwd: info.cwd.clone(),
            focused: info.focused,
            last_line: parser::last_meaningful_line(&lines)
                .map(|s| truncate(&s, 120))
                .unwrap_or_default(),
            flags,
            state_change_seq: info.state_change_seq,
        }
    }
}

/// A full snapshot of the fleet.
#[derive(Debug, Clone, Serialize)]
pub struct Fleet {
    pub herdr_version: String,
    pub fetched_at_epoch: u64,
    pub agents: Vec<AgentRow>,
    pub queues: Vec<QueueFile>,
}

impl Fleet {
    /// An empty fleet, for the TUI to render before its first refresh.
    pub fn empty() -> Fleet {
        Fleet {
            herdr_version: String::new(),
            fetched_at_epoch: 0,
            agents: Vec::new(),
            queues: Vec::new(),
        }
    }

    /// Collect one snapshot: herdr state, each pane's tail, and (optionally)
    /// the checklist summaries.
    pub fn collect(herdr: &Herdr, queue_dir: Option<&Path>) -> Result<Fleet> {
        let herdr_version = herdr.version();
        let infos = herdr.agents()?;
        let agents = infos
            .iter()
            .map(|info| AgentRow::from_info(herdr, info))
            .collect();
        let queues = queue_dir.map(scan_queue_dir).unwrap_or_default();
        Ok(Fleet {
            herdr_version,
            fetched_at_epoch: now_epoch(),
            agents,
            queues,
        })
    }

    /// A plain-text rendering for `herdr-fleet list`.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "herdr-fleet · herdr {} · {} agents\n",
            self.herdr_version,
            self.agents.len()
        ));
        out.push_str(&format!(
            "{:<9} {:<22} {:<8} {:<12} {:>4}  {:<44} {}\n",
            "PANE", "NAME", "STATUS", "MODEL", "CTX", "LAST OUTPUT", "FLAGS"
        ));
        for a in &self.agents {
            out.push_str(&format!(
                "{:<9} {:<22} {:<8} {:<12} {:>4}  {:<44} {}\n",
                a.pane_id,
                truncate(&a.name, 22),
                a.status,
                a.model.clone().unwrap_or_else(|| "-".into()),
                a.ctx_percent
                    .map(|c| format!("{c}%"))
                    .unwrap_or_else(|| "-".into()),
                truncate(&a.last_line, 44),
                a.flags.join("; "),
            ));
        }
        if !self.queues.is_empty() {
            out.push('\n');
            out.push_str("queues\n");
            for q in &self.queues {
                out.push_str(&format!(
                    "  {:<26} open {:<3} done {:<3} {}\n",
                    truncate(&q.name, 26),
                    q.open,
                    q.done,
                    q.first_open.clone().unwrap_or_default(),
                ));
            }
        }
        out
    }
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
