//! Fleet collection: turn herdr state plus pane text into renderable rows.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// A working pane counts as stalled only when its transcript has been silent this long.
    pub quiet: Duration,
    /// Commands that are meant to repeat (e.g. a wait loop); a `loop` flag naming one is dropped.
    pub expected_loops: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            projects_dir: transcript::default_projects_dir(),
            context_window: 1_000_000,
            roster: None,
            quiet: Duration::from_secs(10 * 60),
            expected_loops: Vec::new(),
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
    /// Where the figure came from: `api`, `est` or `plan` (KO).
    pub cost_basis: Option<String>,
    /// All tokens in this session, when a transcript reported usage.
    pub tokens: Option<u64>,
    pub cwd: String,
    pub focused: bool,
    pub last_line: String,
    pub flags: Vec<String>,
    pub state_change_seq: i64,
    /// The most recent tool calls, oldest first (detail view).
    pub tool_calls: Vec<transcript::ToolCall>,
    /// `transcript`, `pane`, `launch` or `none` — where the detail columns came from.
    pub source: String,
    /// Seconds since the Claude transcript last changed; `None` when there is none.
    pub transcript_quiet_secs: Option<u64>,
    #[serde(skip)]
    transcript_path: Option<PathBuf>,
}

impl AgentRow {
    fn from_info(
        herdr: &Herdr,
        info: &AgentInfo,
        options: &Options,
        tracker: &mut TranscriptTracker,
        prev: Option<&AgentRow>,
    ) -> AgentRow {
        let mut row = AgentRow {
            pane_id: info.pane_id.clone(),
            label: info.label.clone(),
            name: info.display_name(),
            status: info.agent_status.clone(),
            model: None,
            ctx_percent: None,
            cost_usd: None,
            cost_basis: None,
            tokens: None,
            cwd: info.cwd.clone(),
            focused: info.focused,
            last_line: String::new(),
            flags: Vec::new(),
            state_change_seq: info.state_change_seq,
            tool_calls: Vec::new(),
            source: SOURCE_NONE.to_string(),
            transcript_quiet_secs: None,
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
            match herdr.read_tail(&info.pane_id, 60) {
                Ok(tail) => scrape_pane(&mut row, &tail),
                Err(_) => keep_stale(&mut row, prev),
            }
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

        // No transcript named a model (a muse-driven pane, say): show what the
        // live launch resolves to rather than a bare dash. Cost still needs a
        // usage source, which only transcripts provide.
        if row.model.is_none() {
            if let Ok(Some(live)) = roster::try_live_launch(herdr, &info.pane_id) {
                row.model = roster::live_model(&live.argv);
            }
        }

        if info.agent_status == "blocked" {
            row.flags.push("blocked".into());
        }
        row.flags
            .retain(|f| !is_expected_loop(f, &options.expected_loops));
        row
    }
}

/// Read a non-Claude pane's status, dialog, loop and last line from its text.
fn scrape_pane(row: &mut AgentRow, tail: &str) {
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
}

/// herdr did not answer for this pane (a timeout, say): keep the last value we
/// had for it and mark it stale, so one slow pane never blanks or fails the fleet.
fn keep_stale(row: &mut AgentRow, prev: Option<&AgentRow>) {
    if let Some(old) = prev {
        row.model = old.model.clone();
        row.ctx_percent = old.ctx_percent;
        row.cost_usd = old.cost_usd;
        row.cost_basis = old.cost_basis.clone();
        row.tokens = old.tokens;
        row.last_line = old.last_line.clone();
        row.flags.push("stale".into());
    }
}

/// True for a `loop xN: <command>` flag whose command is on the expected list.
pub fn is_expected_loop(flag: &str, expected: &[String]) -> bool {
    flag.starts_with("loop x") && expected.iter().any(|e| flag.contains(e.as_str()))
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
    row.cost_basis = detail.cost_basis;
    row.tokens = detail.tokens;
    row.tool_calls = detail.tool_calls;
    if let Some(output) = detail.last_output {
        row.last_line = truncate(&output, 120);
    }
    if let Some((cmd, count)) = detail.loop_cmd {
        row.flags
            .push(format!("loop x{count}: {}", parser::short_command(&cmd)));
    }
    row.transcript_quiet_secs = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());
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
        Self::collect_with(herdr, queue_dir, options, tracker, &HashMap::new())
    }

    /// Like `collect`, but a pane that herdr fails to answer for keeps its
    /// values from `prev` (the previous snapshot, by pane id) marked `stale`.
    pub fn collect_with(
        herdr: &Herdr,
        queue_dir: Option<&Path>,
        options: &Options,
        tracker: &mut TranscriptTracker,
        prev: &HashMap<String, AgentRow>,
    ) -> Result<Fleet> {
        let herdr_version = herdr.version();
        let infos = herdr.agents()?;
        let mut agents: Vec<AgentRow> = infos
            .iter()
            .map(|info| AgentRow::from_info(herdr, info, options, tracker, prev.get(&info.pane_id)))
            .collect();
        let queues = queue_dir.map(scan_queue_dir).unwrap_or_default();
        if let Some(roster) = &options.roster {
            for row in agents.iter_mut() {
                let Some(entry) = roster.find(&row.pane_id) else {
                    continue;
                };
                row.name = roster_name(&entry.queue, &row.name);
                match roster::try_live_launch(herdr, &row.pane_id) {
                    Ok(live) => {
                        let drift = roster::drift(entry, live.as_ref());
                        if !drift.is_empty() {
                            row.flags
                                .push(format!("drift: {}", truncate(&drift.join(", "), 60)));
                        }
                    }
                    // No answer from herdr is not drift: keep the last drift verdict.
                    Err(_) => {
                        if let Some(old) = prev.get(&row.pane_id) {
                            row.flags.extend(
                                old.flags
                                    .iter()
                                    .filter(|f| f.starts_with("drift:"))
                                    .cloned(),
                            );
                            row.flags.push("stale".into());
                        }
                    }
                }
            }
        }
        for row in agents.iter_mut() {
            let queue = queue_for(&queues, &row.name, &row.label);
            let open_unblocked = queue.map(|q| q.open.saturating_sub(q.blocked)).unwrap_or(0);
            if let Some(reason) = waiting_reason(&row.status, &row.last_line, open_unblocked) {
                row.flags.push(reason);
            }
        }
        let live: Vec<PathBuf> = agents
            .iter()
            .filter_map(|a| a.transcript_path.clone())
            .collect();
        tracker.retain(&live);
        let total_cost_usd: f64 = agents.iter().filter_map(|a| a.cost_usd).sum();
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
            "{:<8} {:<22} {:<8} {:<14} {:>4} {:<12} {:<44} {}\n",
            "PANE", "NAME", "STATUS", "MODEL", "CTX", "COST", "LAST OUTPUT", "FLAGS"
        ));
        for a in &self.agents {
            out.push_str(&format!(
                "{:<8} {:<22} {:<8} {:<14} {:>4} {:<12} {:<44} {}\n",
                a.pane_id,
                truncate(&a.name, 22),
                a.status,
                truncate(a.model.as_deref().unwrap_or("-"), 14),
                a.ctx_percent
                    .map(|c| format!("{c}%"))
                    .unwrap_or_else(|| "-".into()),
                cost_cell(a),
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

/// The COST cell: the figure with its basis (`$1.23 api`), `plan` for
/// subscription billing (tokens only), or `-` when nothing priced it (KO).
pub fn cost_cell(row: &AgentRow) -> String {
    match (row.cost_usd, row.cost_basis.as_deref()) {
        (Some(c), Some(b)) => format!("{} {b}", price::fmt_usd(c)),
        (Some(c), None) => price::fmt_usd(c),
        (None, Some("plan")) => match row.tokens {
            Some(t) => format!("{} plan", fmt_tokens(t)),
            None => "plan".to_string(),
        },
        _ => "-".to_string(),
    }
}

/// `1234` -> `1.2k`, `2_500_000` -> `2.5M`.
pub fn fmt_tokens(t: u64) -> String {
    if t >= 1_000_000 {
        format!("{:.1}M", t as f64 / 1_000_000.0)
    } else if t >= 1_000 {
        format!("{:.1}k", t as f64 / 1_000.0)
    } else {
        t.to_string()
    }
}

/// The name a pane shows: its roster queue when it has one, else the terminal name.
pub fn roster_name(roster_queue: &str, terminal_name: &str) -> String {
    if roster_queue.is_empty() {
        terminal_name.to_string()
    } else {
        roster_queue.to_string()
    }
}

/// A working pane is stalled only when it has been in state past `stall` AND its
/// transcript has been silent for at least `quiet_min`. With no transcript (a
/// non-Claude pane) the state time alone decides.
pub fn is_stalled(
    status: &str,
    in_state: Duration,
    stall: Duration,
    quiet: Option<Duration>,
    quiet_min: Duration,
) -> bool {
    status == "working" && in_state >= stall && quiet.map_or(true, |q| q >= quiet_min)
}

/// Why an idle or done pane is waiting on someone: its last output asks for a decision,
/// or its queue has open items that are not blocked.
pub fn waiting_reason(status: &str, last_line: &str, open_unblocked: usize) -> Option<String> {
    if status != "idle" && status != "done" {
        return None;
    }
    let lower = last_line.to_lowercase();
    if lower.contains("decision for you")
        || lower.contains("for chris")
        || last_line.trim_end().ends_with('?')
    {
        return Some("waiting: decision".into());
    }
    (open_unblocked > 0).then(|| format!("waiting: {open_unblocked} open"))
}

/// The queue file a pane works on, matched by file stem to the pane's name or label.
pub fn queue_for<'a>(queues: &'a [QueueFile], name: &str, label: &str) -> Option<&'a QueueFile> {
    queues.iter().find(|q| {
        let stem = Path::new(&q.name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        stem.eq_ignore_ascii_case(name)
            || stem.eq_ignore_ascii_case(label)
            || q.name.eq_ignore_ascii_case(label)
    })
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
