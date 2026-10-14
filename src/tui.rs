//! The ratatui dashboard.

use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use ratatui::{Frame, Terminal};
use tachyonfx::{fx, Effect, Interpolation};
use throbber_widgets_tui::{symbols, Throbber, ThrobberState};
use tui_big_text::{BigText, PixelSize};

use crate::herdr::Herdr;
use crate::model::{AgentRow, Fleet, Options};
use crate::parser::truncate;
use crate::price;
use crate::theme::{Theme, ThemeName};
use crate::transcript::{ToolCall, TranscriptTracker};

/// How the dashboard looks: colour theme and whether the transition effects run.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub theme: ThemeName,
    pub fx: bool,
}

/// From this terminal width the header shows the big fleet count.
const WIDE: u16 = 100;
/// Length of the fade on a row whose state changed.
const FADE_MS: u32 = 450;

/// The Space-key detail view: a snapshot taken when it opens.
struct DetailView {
    pane_id: String,
    name: String,
    status: String,
    model: String,
    ctx: String,
    cost: String,
    queue: Option<(String, String)>,
    tools: Vec<ToolCall>,
    output: Vec<String>,
}

struct App {
    herdr: Herdr,
    queue_dir: Option<PathBuf>,
    options: Options,
    tracker: TranscriptTracker,
    interval: Duration,
    stall: Duration,
    alerts: bool,
    fleet: Fleet,
    table: TableState,
    /// pane_id -> (state_change_seq, first seen in that state)
    since: HashMap<String, (i64, Instant)>,
    /// Alert keys ("pane:kind") already fired; re-armed when the state clears.
    alerted: HashSet<String>,
    detail: Option<DetailView>,
    last_fetch: Instant,
    status: String,
    input: Option<String>,
    quit: bool,
    theme: Theme,
    fx: bool,
    /// The spinner on working panes, advanced once per frame.
    throbber: ThrobberState,
    /// Transition effects still running on table rows.
    effects: Vec<Effect>,
    /// Panes whose status changed since the last frame; they fade at the next table draw.
    fade_pending: Vec<String>,
    prev_status: HashMap<String, String>,
    last_draw: Instant,
}

impl App {
    fn new(
        herdr: Herdr,
        queue_dir: Option<PathBuf>,
        options: Options,
        interval: Duration,
        stall: Duration,
        alerts: bool,
        look: Look,
    ) -> Self {
        Self {
            herdr,
            queue_dir,
            options,
            tracker: TranscriptTracker::default(),
            interval,
            stall,
            alerts,
            fleet: Fleet::empty(),
            table: TableState::default(),
            since: HashMap::new(),
            alerted: HashSet::new(),
            detail: None,
            last_fetch: Instant::now(),
            status: "starting…".into(),
            input: None,
            quit: false,
            theme: Theme::named(look.theme),
            fx: look.fx,
            throbber: ThrobberState::default(),
            effects: Vec::new(),
            fade_pending: Vec::new(),
            prev_status: HashMap::new(),
            last_draw: Instant::now(),
        }
    }

    fn refresh(&mut self) {
        match Fleet::collect(
            &self.herdr,
            self.queue_dir.as_deref(),
            &self.options,
            &mut self.tracker,
        ) {
            Ok(fleet) => {
                let now = Instant::now();
                for a in &fleet.agents {
                    let seen = self.since.get(&a.pane_id).map(|(seq, _)| *seq);
                    if seen != Some(a.state_change_seq) {
                        self.since
                            .insert(a.pane_id.clone(), (a.state_change_seq, now));
                    }
                }
                let live: Vec<String> = fleet.agents.iter().map(|a| a.pane_id.clone()).collect();
                self.since.retain(|k, _| live.contains(k));
                if self.fx {
                    for a in &fleet.agents {
                        let changed = self
                            .prev_status
                            .get(&a.pane_id)
                            .is_some_and(|prev| *prev != a.status);
                        if changed {
                            self.fade_pending.push(a.pane_id.clone());
                        }
                        self.prev_status.insert(a.pane_id.clone(), a.status.clone());
                    }
                    self.prev_status.retain(|k, _| live.contains(k));
                }
                match self.table.selected() {
                    Some(i) if i < fleet.agents.len() => {}
                    _ => self.table.select(if fleet.agents.is_empty() {
                        None
                    } else {
                        Some(0)
                    }),
                }
                if self.alerts {
                    self.alert_on(&fleet);
                }
                self.status = format!(
                    "{} agents · {} queues · herdr {} · ~{} est spend",
                    fleet.agents.len(),
                    fleet.queues.len(),
                    fleet.herdr_version,
                    price::fmt_usd(fleet.total_cost_usd)
                );
                self.fleet = fleet;
                self.last_fetch = now;
            }
            Err(e) => self.status = format!("error: {e:#}"),
        }
    }

    /// Fire one herdr notification per pane per kind when it first appears.
    fn alert_on(&mut self, fleet: &Fleet) {
        let mut current: Vec<(String, String)> = Vec::new();
        for a in &fleet.agents {
            if a.status == "blocked" {
                current.push((a.pane_id.clone(), "blocked".into()));
            }
            if a.status == "working" {
                if let Some((_, t)) = self.since.get(&a.pane_id) {
                    if t.elapsed() >= self.stall {
                        current.push((a.pane_id.clone(), "stalled".into()));
                    }
                }
            }
            if a.flags.iter().any(|f| f.starts_with("loop")) {
                current.push((a.pane_id.clone(), "looping".into()));
            }
        }
        let mut to_fire: Vec<(String, String)> = Vec::new();
        for (pane, kind) in &current {
            if self.alerted.insert(format!("{pane}:{kind}")) {
                to_fire.push((pane.clone(), kind.clone()));
            }
        }
        let live: HashSet<String> = current.iter().map(|(p, k)| format!("{p}:{k}")).collect();
        self.alerted.retain(|k| live.contains(k));
        for (pane, kind) in to_fire {
            let body = fleet
                .agents
                .iter()
                .find(|a| a.pane_id == pane)
                .map(|a| truncate(&format!("{} — {}", a.status, a.last_line), 120))
                .unwrap_or_default();
            if let Err(e) = self
                .herdr
                .notify(&format!("herdr-fleet: {kind} {pane}"), &body)
            {
                self.status = format!("alert failed: {e:#}");
            }
        }
    }

    fn selected_pane(&self) -> Option<String> {
        self.table
            .selected()
            .and_then(|i| self.fleet.agents.get(i))
            .map(|a| a.pane_id.clone())
    }

    fn move_selection(&mut self, delta: isize) {
        if self.fleet.agents.is_empty() {
            self.table.select(None);
            return;
        }
        let n = self.fleet.agents.len() as isize;
        let cur = self.table.selected().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(n);
        self.table.select(Some(next as usize));
    }

    fn focus_selected(&mut self) {
        if let Some(pane) = self.selected_pane() {
            self.status = match self.herdr.focus(&pane) {
                Ok(()) => format!("focused {pane}"),
                Err(e) => format!("focus failed: {e:#}"),
            };
        }
    }

    fn send_prompt(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let Some(pane) = self.selected_pane() else {
            return;
        };
        self.status = match self.herdr.prompt(&pane, &text) {
            Ok(()) => format!("sent prompt to {pane}"),
            Err(e) => format!("prompt failed: {e:#}"),
        };
        self.refresh();
    }

    /// Space: open (or close) the detail view for the selected pane.
    fn toggle_detail(&mut self) {
        if self.detail.is_some() {
            self.detail = None;
            return;
        }
        let Some(i) = self.table.selected() else {
            return;
        };
        let Some(row) = self.fleet.agents.get(i).cloned() else {
            return;
        };
        let mut output: Vec<String> = Vec::new();
        match self.herdr.read_tail(&row.pane_id, 30) {
            Ok(tail) => {
                let all: Vec<&str> = tail.lines().collect();
                let start = all.len().saturating_sub(30);
                for l in &all[start..] {
                    output.push(l.trim_end().to_string());
                }
            }
            Err(e) => output.push(format!("(pane read failed: {e:#})")),
        }
        let queue = self
            .fleet
            .queues
            .iter()
            .find(|q| {
                let stem = PathBuf::from(&q.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                stem.eq_ignore_ascii_case(&row.label)
                    || stem.eq_ignore_ascii_case(&row.name)
                    || q.name.eq_ignore_ascii_case(&row.label)
            })
            .map(|q| {
                (
                    q.name.clone(),
                    q.first_open.clone().unwrap_or_else(|| "—".into()),
                )
            });
        self.detail = Some(DetailView {
            pane_id: row.pane_id.clone(),
            name: row.name.clone(),
            status: row.status.clone(),
            model: row.model.clone().unwrap_or_else(|| "-".into()),
            ctx: row
                .ctx_percent
                .map(|c| format!("{c}%"))
                .unwrap_or_else(|| "-".into()),
            cost: row
                .cost_usd
                .map(price::fmt_usd)
                .unwrap_or_else(|| "-".into()),
            queue,
            tools: row.tool_calls.iter().rev().take(5).rev().cloned().collect(),
            output,
        });
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.input.is_some() {
            match key.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let text = self.input.take().unwrap_or_default();
                    self.send_prompt(text);
                }
                KeyCode::Backspace => {
                    if let Some(buf) = self.input.as_mut() {
                        buf.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(buf) = self.input.as_mut() {
                        buf.push(c);
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Enter => self.focus_selected(),
            KeyCode::Char(' ') => self.toggle_detail(),
            KeyCode::Esc => self.detail = None,
            KeyCode::Char('i') if self.selected_pane().is_some() => {
                self.input = Some(String::new());
            }
            _ => {}
        }
    }

    /// The status as a coloured pill.
    fn status_pill(&self, status: &str) -> Span<'static> {
        let bg = match status {
            "working" => self.theme.working,
            "idle" => self.theme.idle,
            "done" => self.theme.done,
            "blocked" => self.theme.blocked,
            _ => self.theme.muted,
        };
        Span::styled(
            format!(" {status} "),
            Style::default()
                .fg(self.theme.on_accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )
    }

    /// The state column: time in the state, with a spinner while the pane is working.
    fn state_line(&self, a: &AgentRow) -> Line<'static> {
        let elapsed = self.since.get(&a.pane_id).map(|(_, t)| t.elapsed());
        let mut text = elapsed.map(fmt_duration).unwrap_or_else(|| "-".into());
        let mut color = self.theme.muted;
        if a.status == "working" {
            if let Some(e) = elapsed {
                if e >= self.stall {
                    color = self.theme.alarm;
                    text.push('!');
                }
            }
        }
        let mut spans = Vec::new();
        if a.status == "working" {
            spans.push(
                Throbber::default()
                    .throbber_style(Style::default().fg(self.theme.working))
                    .throbber_set(symbols::throbber::BRAILLE_SIX)
                    .to_symbol_span(&self.throbber),
            );
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(text, Style::default().fg(color)));
        Line::from(spans)
    }

    /// Context as a five-cell gauge and the percentage; just the percentage when `bar` is off.
    fn ctx_line(&self, percent: Option<u8>, bar: bool) -> Line<'static> {
        let Some(c) = percent else {
            return Line::from(Span::styled("-", Style::default().fg(self.theme.muted)));
        };
        if !bar {
            let color = if c >= 80 { self.theme.alarm } else { self.theme.text };
            return Line::from(Span::styled(format!("{c}%"), Style::default().fg(color)));
        }
        let filled = ((c as usize * 5 + 50) / 100).min(5);
        let fill_color = if c >= 80 {
            self.theme.alarm
        } else if c >= 50 {
            self.theme.warn
        } else {
            self.theme.done
        };
        Line::from(vec![
            Span::styled("█".repeat(filled), Style::default().fg(fill_color)),
            Span::styled(
                "░".repeat(5 - filled),
                Style::default().fg(self.theme.surface),
            ),
            Span::styled(
                format!(" {c:>3}%"),
                Style::default().fg(self.theme.text),
            ),
        ])
    }

    /// Cost as a five-cell gauge scaled to the fleet's highest spend, and the amount.
    fn cost_line(&self, cost: Option<f64>, max: f64) -> Line<'static> {
        let Some(c) = cost else {
            return Line::from(Span::styled("-", Style::default().fg(self.theme.muted)));
        };
        let filled = if max > 0.0 {
            ((c / max * 5.0).round() as usize).min(5)
        } else {
            0
        };
        Line::from(vec![
            Span::styled("█".repeat(filled), Style::default().fg(self.theme.accent)),
            Span::styled("░".repeat(5 - filled), Style::default().fg(self.theme.surface)),
            Span::styled(
                format!(" {}", price::fmt_usd(c)),
                Style::default().fg(self.theme.text),
            ),
        ])
    }

    /// Column widths sized to the data: NAME/MODEL/FLAGS get their content width
    /// (clamped, narrower below WIDE columns), and LAST OUTPUT takes what remains.
    /// Below WIDE the COST column is left out, so the alarms and the output keep room.
    fn column_widths(&self, wide: bool) -> (Vec<Constraint>, usize, usize, usize) {
        let (name_max, model_max, flags_max) = if wide { (28, 18, 30) } else { (9, 9, 6) };
        let name = self
            .fleet
            .agents
            .iter()
            .map(|a| a.name.chars().count())
            .max()
            .unwrap_or(4)
            .clamp(4, name_max);
        let model = self
            .fleet
            .agents
            .iter()
            .map(|a| a.model.as_deref().map(|m| m.chars().count()).unwrap_or(0))
            .max()
            .unwrap_or(5)
            .clamp(5, model_max);
        let flags = self
            .fleet
            .agents
            .iter()
            .map(|a| a.flags.join("; ").chars().count())
            .max()
            .unwrap_or(5)
            .clamp(5, flags_max);
        let mut widths = vec![
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(name as u16),
            Constraint::Length(9),
            Constraint::Length(model as u16),
            Constraint::Length(if wide { 10 } else { 4 }),
        ];
        if wide {
            widths.push(Constraint::Length(12));
        }
        widths.push(Constraint::Length(8));
        widths.push(Constraint::Length(flags as u16));
        widths.push(Constraint::Min(if wide { 16 } else { 8 }));
        (widths, name, model, flags)
    }

    fn render(&mut self, f: &mut Frame) {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_draw);
        self.last_draw = now;
        self.throbber.calc_next();

        // Size the fleet and queue blocks to their content, so neither leaves
        // a big empty box; the filler between them absorbs the slack.
        let wide = f.area().width >= WIDE;
        let has_queues = self.queue_dir.is_some() && !self.fleet.queues.is_empty();
        let table_h = (self.fleet.agents.len() as u16 + 3).clamp(4, 30);
        let header_h = if wide { 6 } else { 4 };
        let mut constraints = vec![Constraint::Length(header_h), Constraint::Length(table_h)];
        if has_queues {
            constraints.push(Constraint::Length(
                (self.fleet.queues.len() as u16 + 2).clamp(3, 12),
            ));
        }
        constraints.push(Constraint::Min(0));
        constraints.push(Constraint::Length(1));

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(f.area());

        let mut i = 0;
        self.render_header(f, rows[i], wide);
        i += 1;
        self.render_table(f, rows[i]);
        i += 1;
        if has_queues {
            self.render_queues(f, rows[i]);
            i += 1;
        }
        i += 1; // filler
        self.render_footer(f, rows[i]);

        // Effects act on the cells already drawn: the table rows that changed state.
        let frame_area = f.area();
        let buf = f.buffer_mut();
        for effect in self.effects.iter_mut() {
            effect.process(dt.into(), buf, frame_area);
        }
        self.effects.retain(|e| e.running());

        if let Some(buf) = self.input.clone() {
            self.render_input(f, f.area(), &buf);
        }
        if self.detail.is_some() {
            self.render_detail(f, f.area());
        }
    }

    /// The status text: the full line when there is room, a shorter one when not.
    fn header_status(&self, wide: bool) -> String {
        if wide || self.status.starts_with("error") || self.fleet.herdr_version.is_empty() {
            return self.status.clone();
        }
        format!(
            "{} agents · {} queues · ~{} est spend",
            self.fleet.agents.len(),
            self.fleet.queues.len(),
            price::fmt_usd(self.fleet.total_cost_usd)
        )
    }

    fn render_header(&self, f: &mut Frame, area: Rect, wide: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let age = self.last_fetch.elapsed().as_secs();
        let title = Line::from(vec![
            Span::styled(
                " herdr-fleet ",
                Style::default()
                    .fg(self.theme.on_accent)
                    .bg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", self.header_status(wide)),
                Style::default().fg(self.theme.text),
            ),
        ]);
        let legend = Line::from(vec![
            self.status_pill("working"),
            Span::raw(" "),
            self.status_pill("idle"),
            Span::raw(" "),
            self.status_pill("done"),
            Span::raw(" "),
            self.status_pill("blocked"),
            Span::styled(
                format!("   refreshed {age}s ago   IN STATE: observed since launch"),
                Style::default().fg(self.theme.muted),
            ),
        ]);
        let info = Paragraph::new(vec![title, legend]);

        if wide {
            let [count_area, info_area] =
                Layout::horizontal([Constraint::Length(20), Constraint::Min(0)]).areas(inner);
            let count = BigText::builder()
                .pixel_size(PixelSize::Quadrant)
                .style(Style::default().fg(self.theme.accent))
                .lines(vec![self.fleet.agents.len().to_string().into()])
                .build();
            f.render_widget(count, count_area);
            f.render_widget(info, info_area);
        } else {
            f.render_widget(info, inner);
        }
    }

    fn render_table(&mut self, f: &mut Frame, area: Rect) {
        let wide = area.width >= WIDE;
        let mut heads = vec!["", "PANE", "NAME", "STATUS", "MODEL", "CTX"];
        if wide {
            heads.push("COST");
        }
        heads.extend(["IN STATE", "FLAGS", "LAST OUTPUT"]);
        let header = Row::new(heads).style(
            Style::default()
                .fg(self.theme.muted)
                .add_modifier(Modifier::BOLD),
        );

        let (widths, name_w, model_w, flags_w) = self.column_widths(wide);
        let max_cost = self
            .fleet
            .agents
            .iter()
            .filter_map(|a| a.cost_usd)
            .fold(0.0_f64, f64::max);

        let rows: Vec<Row> = self
            .fleet
            .agents
            .iter()
            .map(|a| {
                let flags = a.flags.join("; ");
                let flag_color = if a.flags.iter().any(|f| {
                    f.starts_with("drift") || f.starts_with("dialog") || f.starts_with("loop")
                }) || a.status == "blocked"
                {
                    self.theme.alarm
                } else {
                    self.theme.muted
                };
                let mut cells = vec![
                    Cell::from(Span::styled(
                        if a.focused { "●" } else { " " },
                        Style::default().fg(self.theme.accent),
                    )),
                    Cell::from(a.pane_id.clone()),
                    Cell::from(truncate(&a.name, name_w)),
                    Cell::from(self.status_pill(&a.status)),
                    Cell::from(truncate(a.model.as_deref().unwrap_or("-"), model_w)),
                    Cell::from(self.ctx_line(a.ctx_percent, wide)),
                ];
                if wide {
                    cells.push(Cell::from(self.cost_line(a.cost_usd, max_cost)));
                }
                cells.push(Cell::from(self.state_line(a)));
                cells.push(Cell::from(Span::styled(
                    truncate(&flags, flags_w),
                    Style::default().fg(flag_color),
                )));
                cells.push(Cell::from(truncate(&a.last_line, 60)));
                Row::new(cells)
            })
            .collect();

        let table = Table::new(rows, widths)
            .header(header)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.border))
                    .title(" fleet "),
            )
            .row_highlight_style(
                Style::default()
                    .fg(self.theme.text)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ")
            .column_spacing(1);
        f.render_stateful_widget(table, area, &mut self.table);

        // Row i sits under the border and the header row: start the fade for changed rows.
        for (i, a) in self.fleet.agents.iter().enumerate() {
            if let Some(pos) = self.fade_pending.iter().position(|p| *p == a.pane_id) {
                self.fade_pending.remove(pos);
                let row = Rect {
                    x: area.x + 1,
                    y: area.y + 2 + i as u16,
                    width: area.width.saturating_sub(2),
                    height: 1,
                };
                if row.y < area.y + area.height.saturating_sub(1) {
                    let fade = fx::fade_from_fg(
                        self.theme.accent,
                        (FADE_MS, Interpolation::QuadOut),
                    )
                    .with_area(row);
                    self.effects.push(fade);
                }
            }
        }
        self.fade_pending.clear();
    }

    fn render_queues(&self, f: &mut Frame, area: Rect) {
        // One line per queue: no wrapping, anything too wide is clipped.
        let lines: Vec<Line> = self
            .fleet
            .queues
            .iter()
            .map(|q| {
                let open_color = if q.open > 0 {
                    self.theme.warn
                } else {
                    self.theme.muted
                };
                Line::from(vec![
                    Span::styled(
                        format!("{:<24}", truncate(&q.name, 24)),
                        Style::default().fg(self.theme.text),
                    ),
                    Span::styled(
                        format!(" open {:<3}", q.open),
                        Style::default().fg(open_color),
                    ),
                    Span::styled(
                        format!(" done {:<3}  ", q.done),
                        Style::default().fg(self.theme.muted),
                    ),
                    Span::styled(
                        truncate(q.first_open.as_deref().unwrap_or(""), 52),
                        Style::default().fg(self.theme.text),
                    ),
                ])
            })
            .collect();
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.border))
                    .title(" queues "),
            ),
            area,
        );
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let hints = if area.width >= WIDE {
            " q quit  ·  r refresh  ·  ↑/k ↓/j move  ·  ⏎ focus pane  ·  space detail  ·  i send prompt "
        } else {
            " q quit · r refresh · ↑↓ move · ⏎ focus · space detail · i prompt"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hints,
                Style::default().fg(self.theme.muted),
            ))),
            area,
        );
    }

    fn render_input(&self, f: &mut Frame, area: Rect, buf: &str) {
        let pane = self.selected_pane().unwrap_or_else(|| "?".into());
        let width = area.width.saturating_sub(8).min(90);
        let rect = centered(area, width, 3);
        f.render_widget(Clear, rect);
        let line = Line::from(vec![
            Span::styled("› ", Style::default().fg(self.theme.accent)),
            Span::styled(buf.to_string(), Style::default().fg(self.theme.text)),
            Span::styled("█", Style::default().fg(self.theme.accent)),
        ]);
        f.render_widget(
            Paragraph::new(line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.accent))
                    .title(format!(" send prompt to {pane} ")),
            ),
            rect,
        );
    }

    fn render_detail(&self, f: &mut Frame, area: Rect) {
        let Some(d) = &self.detail else {
            return;
        };
        let rect = centered(
            area,
            area.width.saturating_sub(8).min(110),
            area.height.saturating_sub(2).min(26),
        );
        f.render_widget(Clear, rect);
        let inner = rect.width.saturating_sub(2) as usize;
        let muted = Style::default().fg(self.theme.muted);
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", d.pane_id),
                Style::default()
                    .fg(self.theme.on_accent)
                    .bg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate(
                    &format!(
                        "  {}  ·  {}  ·  {}  ·  ctx {}  ·  ~{} est",
                        d.name, d.status, d.model, d.ctx, d.cost
                    ),
                    inner.saturating_sub(9),
                ),
                Style::default().fg(self.theme.text),
            ),
        ]));
        match &d.queue {
            Some((name, item)) => lines.push(Line::from(vec![
                Span::styled("queue   ", muted),
                Span::styled(
                    truncate(&format!("{name}: {item}"), inner.saturating_sub(8)),
                    Style::default().fg(self.theme.text),
                ),
            ])),
            None => lines.push(Line::from(Span::styled("queue   —", muted))),
        }
        lines.push(Line::from(Span::styled("last tool calls", muted)));
        if d.tools.is_empty() {
            lines.push(Line::from("  —"));
        }
        for t in &d.tools {
            lines.push(Line::from(truncate(
                &format!("  {}: {}", t.name, t.detail),
                inner,
            )));
        }
        lines.push(Line::from(Span::styled("output (last 30 lines)", muted)));
        for l in &d.output {
            lines.push(Line::from(truncate(l, inner)));
        }
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.accent))
                    .title(" pane detail — space/esc closes "),
            ),
            rect,
        );
    }

    fn main_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        while !self.quit {
            terminal.draw(|f| self.render(f))?;
            let remaining = self.interval.saturating_sub(self.last_fetch.elapsed());
            let timeout = remaining.min(Duration::from_millis(200));
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.on_key(key);
                    }
                }
            }
            if self.last_fetch.elapsed() >= self.interval {
                self.refresh();
            }
        }
        Ok(())
    }
}

/// Run the interactive dashboard against the given herdr executable.
pub fn run(
    herdr: Herdr,
    queue_dir: Option<PathBuf>,
    options: Options,
    interval_secs: u64,
    stall_secs: u64,
    alerts: bool,
    look: Look,
) -> Result<()> {
    // Record our own pane once, so `herdr-fleet back` can find it later.
    let _ = crate::keys::record_fleet_pane(&herdr);
    let mut terminal = ratatui::init();
    let mut app = App::new(
        herdr,
        queue_dir,
        options,
        Duration::from_secs(interval_secs.max(1)),
        Duration::from_secs(stall_secs.max(1)),
        alerts,
        look,
    );
    app.refresh();
    let result = app.main_loop(&mut terminal);
    ratatui::restore();
    result
}

/// Render one frame at the given size, without a terminal. Used for snapshots and tests.
pub fn snapshot(
    herdr: Herdr,
    queue_dir: Option<PathBuf>,
    options: Options,
    look: Look,
    width: u16,
    height: u16,
) -> Result<Buffer> {
    let mut app = App::new(
        herdr,
        queue_dir,
        options,
        Duration::from_secs(2),
        Duration::from_secs(900),
        false,
        look,
    );
    app.refresh();
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|f| app.render(f))?;
    Ok(terminal.backend().buffer().clone())
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn durations_render_compactly() {
        assert_eq!(fmt_duration(Duration::from_secs(5)), "5s");
        assert_eq!(fmt_duration(Duration::from_secs(90)), "1m");
        assert_eq!(fmt_duration(Duration::from_secs(3660)), "1h01m");
    }

    #[test]
    fn popup_is_centered_and_clamped() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let r = centered(area, 90, 3);
        assert_eq!(r.width, 40);
        assert_eq!(r.height, 3);
    }

    /// Renders with no herdr reachable (an empty fleet) at both required widths and
    /// checks the title and the footer hints are on screen, with no panic.
    #[test]
    fn renders_at_80_and_160_columns() {
        for width in [80u16, 160] {
            let look = Look {
                theme: ThemeName::Mocha,
                fx: true,
            };
            let buf = snapshot(
                Herdr::new("/nonexistent/herdr"),
                None,
                Options::default(),
                look,
                width,
                30,
            )
            .unwrap();
            let text: String = buf.content.iter().map(|c| c.symbol()).collect();
            assert!(text.contains("herdr-fleet"), "width {width}");
            assert!(text.contains("quit"), "width {width}");
        }
    }
}
