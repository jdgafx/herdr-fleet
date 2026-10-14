//! Fleet collection: turn herdr state plus pane text into renderable rows.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use crate::herdr::Herdr;
use crate::parser::{self, truncate};
use crate::price;
use crate::queue::{scan_queue_dir, QueueFile};
use crate::roster::{self, Roster};
use crate::transcript::{self, TranscriptTracker};

/// How the row's detail columns were filled.
pub const SOURCE_TRANSCRIPT: &str = "transcript";
pub const SOURCE_PANE: &str = "pane";
/// A fresh Claude pane: the model comes from the launch arguments.
pub const SOURCE_LAUNCH: &str = "launch";
/// Nothing readable yet.
pub const SOURCE_NONE: &str = "none";

/// Settings that steer where the row detail comes from.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory holding Claude Code session transcripts.
    pub projects_dir: PathBuf,
    /// Tokens in the model's context window, for the ctx % column.
    pub context_window: u64,
    /// The roster, when one is loaded; its panes get a `drift` flag when they differ.
    pub roster: Option<Roster>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            projects_dir: transcript::default_projects_dir(),
            context_window: 1_000_000,
            roster: None,
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
    /// Estimated spend for this session at list prices; `None` when unpriced.
    pub cost_usd: Option<f64>,
    pub cwd: String,
    pub focused: bool,
    pub last_line: String,
    pub flags: Vec<String>,
    pub state_change_seq: i64,
    /// The most recent tool calls, oldest first (detail view).
    pub tool_calls: Vec<transcript::ToolCall>,
    /// `transcript`, `pane`, `launch` or `none` — where the detail columns came from.
    pub source: String,
    #[serde(skip)]
    transcript_path: Option<PathBuf>,
}

impl AgentRow {
    fn from_info(
        herdr: &Herdr,
        info: &AgentInfo,
        options: &Options,
        tracker: &mut TranscriptTracker,
    ) -> AgentRow {
        let mut row = AgentRow {
            pane_id: info.pane_id.clone(),
            label: info.label.clone(),
            name: info.display_name(),
            status: info.agent_status.clone(),
            model: None,
            ctx_percent: None,
            cost_usd: None,
            cwd: info.cwd.clone(),
            focused: info.focused,
            last_line: String::new(),
            flags: Vec::new(),
            state_change_seq: info.state_change_seq,
            tool_calls: Vec::new(),
            source: SOURCE_NONE.to_string(),
            transcript_path: None,
        };

        // Preferred: the Claude session transcript. It has the full model id,
        // token usage and recent tool calls that the clamped pane text lacks.
        if fill_from_transcript(info, options, tracker, &mut row) {
            row.source = SOURCE_TRANSCRIPT.to_string();
        } else if info.agent != "claude" {
            // Fallback: scrape the pane text — for non-Claude agents only. A
            // Claude pane is read from its transcript, so Claude's own UI chrome
            // (status line, input box, footer) can never be mistaken for output.
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
                    .push(format!("loop x{count}: {}", parser::short_command(&line)));
            }
            row.last_line = parser::last_meaningful_line(&lines)
                .map(|s| truncate(&s, 120))
                .unwrap_or_default();
            row.source = SOURCE_PANE.to_string();
        } else {
            // A fresh Claude pane with no assistant turn yet: show the model
            // it was launched with (from the process arguments) rather than a
            // bare dash, or the word "fresh" when even that is unreadable.
            match herdr.launch_model(&info.pane_id) {
                Some(model) => {
                    row.model = Some(transcript::pretty_model(&model));
                    row.source = SOURCE_LAUNCH.to_string();
                }
                None => {
                    row.model = Some("fresh".to_string());
                }
            }
        }

        if info.agent_status == "blocked" {
            row.flags.push("blocked".into());
        }
        row
    }
}

/// Fill the detail columns from the pane's Claude transcript. Returns false
/// (leaving `row` untouched) when there is no usable transcript.
fn fill_from_transcript(
    info: &AgentInfo,
    options: &Options,
    tracker: &mut TranscriptTracker,
    row: &mut AgentRow,
) -> bool {
    if info.agent != "claude" {
        return false;
    }
    let Some(path) = transcript::find_transcript(&options.projects_dir, &info.session_id()) else {
        return false;
    };
    let Ok(detail) = tracker.update(&path, options.context_window) else {
        return false;
    };
    if detail.model.is_none() && detail.ctx_tokens.is_none() {
        return false;
    }
    row.model = detail.model.as_deref().map(transcript::pretty_model);
    row.ctx_percent = detail.ctx_percent;
    row.cost_usd = detail.cost_usd;
    row.tool_calls = detail.tool_calls;
    if let Some(output) = detail.last_output {
        row.last_line = truncate(&output, 120);
    }
    if let Some((cmd, count)) = detail.loop_cmd {
        row.flags
            .push(format!("loop x{count}: {}", parser::short_command(&cmd)));
    }
    row.transcript_path = Some(path);
    true
}

/// A full snapshot of the fleet.
#[derive(Debug, Clone, Serialize)]
pub struct Fleet {
    pub herdr_version: String,
    pub fetched_at_epoch: u64,
    pub agents: Vec<AgentRow>,
    pub queues: Vec<QueueFile>,
    /// Sum of the panes' estimated spend, USD.
    pub total_cost_usd: f64,
}

impl Fleet {
    /// An empty fleet, for the TUI to render before its first refresh.
    pub fn empty() -> Fleet {
        Fleet {
            herdr_version: String::new(),
            fetched_at_epoch: 0,
            agents: Vec::new(),
            queues: Vec::new(),
            total_cost_usd: 0.0,
        }
    }

    /// Collect one snapshot: herdr state, each pane's detail, and (optionally)
    /// the checklist summaries.
    pub fn collect(
        herdr: &Herdr,
        queue_dir: Option<&Path>,
        options: &Options,
        tracker: &mut TranscriptTracker,
    ) -> Result<Fleet> {
        let herdr_version = herdr.version();
        let infos = herdr.agents()?;
        let mut agents: Vec<AgentRow> = infos
            .iter()
            .map(|info| AgentRow::from_info(herdr, info, options, tracker))
            .collect();
        if let Some(roster) = &options.roster {
            for row in agents.iter_mut() {
                let Some(entry) = roster.find(&row.pane_id) else {
                    continue;
                };
                let live = roster::live_launch(herdr, &row.pane_id);
                let drift = roster::drift(entry, live.as_ref());
                if !drift.is_empty() {
                    row.flags.push(format!("drift: {}", truncate(&drift.join(", "), 60)));
                }
            }
        }
        let live: Vec<PathBuf> = agents
            .iter()
            .filter_map(|a| a.transcript_path.clone())
            .collect();
        tracker.retain(&live);
        let total_cost_usd: f64 = agents.iter().filter_map(|a| a.cost_usd).sum();
        let queues = queue_dir.map(scan_queue_dir).unwrap_or_default();
        Ok(Fleet {
            herdr_version,
            fetched_at_epoch: now_epoch(),
            agents,
            queues,
            total_cost_usd,
        })
    }

    /// A plain-text rendering for `herdr-fleet list`.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "herdr-fleet · herdr {} · {} agents · ~{} est spend\n",
            self.herdr_version,
            self.agents.len(),
            price::fmt_usd(self.total_cost_usd)
        ));
        out.push_str(&format!(
            "{:<8} {:<22} {:<8} {:<14} {:>4} {:>7}  {:<44} {}\n",
            "PANE", "NAME", "STATUS", "MODEL", "CTX", "COST", "LAST OUTPUT", "FLAGS"
        ));
        for a in &self.agents {
            out.push_str(&format!(
                "{:<8} {:<22} {:<8} {:<14} {:>4} {:>7}  {:<44} {}\n",
                a.pane_id,
                truncate(&a.name, 22),
                a.status,
                truncate(a.model.as_deref().unwrap_or("-"), 14),
                a.ctx_percent
                    .map(|c| format!("{c}%"))
                    .unwrap_or_else(|| "-".into()),
                a.cost_usd.map(price::fmt_usd).unwrap_or_else(|| "-".into()),
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
