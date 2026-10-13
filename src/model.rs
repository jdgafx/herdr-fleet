//! Fleet collection: turn herdr state plus pane text into renderable rows.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use crate::herdr::Herdr;
use crate::parser::{self, truncate};
use crate::queue::{scan_queue_dir, QueueFile};
use crate::transcript;

/// How the row's detail columns were filled.
pub const SOURCE_TRANSCRIPT: &str = "transcript";
pub const SOURCE_PANE: &str = "pane";

/// Settings that steer where the row detail comes from.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory holding Claude Code session transcripts.
    pub projects_dir: PathBuf,
    /// Tokens in the model's context window, for the ctx % column.
    pub context_window: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            projects_dir: transcript::default_projects_dir(),
            context_window: 1_000_000,
        }
    }
}

/// The session a pane is running, as herdr reports it.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AgentSession {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub value: String,
}

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
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

impl AgentInfo {
    /// The Claude Code session id this pane reports, if any.
    pub fn session_id(&self) -> String {
        self.agent_session
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_default()
    }

    fn display_name(&self) -> String {
        if !self.terminal_title_stripped.is_empty() {
            self.terminal_title_stripped.clone()
        } else if !self.terminal_title.is_empty() {
            self.terminal_title.clone()
        } else if !self.label.is_empty() {
            self.label.clone()
        } else {
            self.agent.clone()
        }
    }
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
    /// `transcript` or `pane` — where the detail columns came from.
    pub source: String,
}

impl AgentRow {
    fn from_info(herdr: &Herdr, info: &AgentInfo, options: &Options) -> AgentRow {
        let mut row = AgentRow {
            pane_id: info.pane_id.clone(),
            label: info.label.clone(),
            name: info.display_name(),
            status: info.agent_status.clone(),
            model: None,
            ctx_percent: None,
            cwd: info.cwd.clone(),
            focused: info.focused,
            last_line: String::new(),
            flags: Vec::new(),
            state_change_seq: info.state_change_seq,
            source: SOURCE_PANE.to_string(),
        };

        // Preferred: the Claude session transcript. It has the full model id,
        // token usage and recent tool calls that the clamped pane text lacks.
        if fill_from_transcript(info, options, &mut row) {
            row.source = SOURCE_TRANSCRIPT.to_string();
        }

        // Fallback: scrape the pane text. Clamped to the pane width, but the
        // only source for non-Claude agents or a pane with no transcript yet.
        if row.source != SOURCE_TRANSCRIPT {
            let tail = herdr.read_tail(&info.pane_id, 60).unwrap_or_default();
            let lines: Vec<&str> = tail.lines().collect();
            if let Some(status) = lines
                .iter()
                .rev()
                .find_map(|l| parser::parse_status_line(l))
            {
                row.model = Some(status.model).filter(|m| !m.is_empty());
                row.ctx_percent = status.ctx_percent;
            }
            if let Some(dialog) = parser::detect_dialog(&lines) {
                row.flags.push(format!("dialog: {}", truncate(&dialog, 48)));
            }
            if let Some((line, count)) = parser::detect_loop(&lines, 3) {
                row.flags
                    .push(format!("loop x{count}: {}", truncate(&line, 44)));
            }
            row.last_line = parser::last_meaningful_line(&lines)
                .map(|s| truncate(&s, 120))
                .unwrap_or_default();
        }

        if info.agent_status == "blocked" {
            row.flags.push("blocked".into());
        }
        row
    }
}

/// Fill the detail columns from the pane's Claude transcript. Returns false
/// (leaving `row` untouched) when there is no usable transcript.
fn fill_from_transcript(info: &AgentInfo, options: &Options, row: &mut AgentRow) -> bool {
    if info.agent != "claude" {
        return false;
    }
    let Some(path) = transcript::find_transcript(&options.projects_dir, &info.session_id()) else {
        return false;
    };
    let Ok(detail) = transcript::read_info(&path, options.context_window) else {
        return false;
    };
    if detail.model.is_none() && detail.ctx_tokens.is_none() {
        return false;
    }
    row.model = detail.model.as_deref().map(transcript::pretty_model);
    row.ctx_percent = detail.ctx_percent;
    if let Some(output) = detail.last_output {
        row.last_line = truncate(&output, 120);
    }
    if let Some((cmd, count)) = detail.loop_cmd {
        row.flags
            .push(format!("loop x{count}: {}", truncate(&cmd, 44)));
    }
    true
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

    /// Collect one snapshot: herdr state, each pane's detail, and (optionally)
    /// the checklist summaries.
    pub fn collect(herdr: &Herdr, queue_dir: Option<&Path>, options: &Options) -> Result<Fleet> {
        let herdr_version = herdr.version();
        let infos = herdr.agents()?;
        let agents = infos
            .iter()
            .map(|info| AgentRow::from_info(herdr, info, options))
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
