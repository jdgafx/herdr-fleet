//! The ratatui dashboard.

use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

use crate::herdr::Herdr;
use crate::model::{AgentRow, Fleet, Options};
use crate::parser::truncate;

struct App {
    herdr: Herdr,
    queue_dir: Option<PathBuf>,
    options: Options,
    interval: Duration,
    stall: Duration,
    fleet: Fleet,
    table: TableState,
    /// pane_id -> (state_change_seq, first seen in that state)
    since: HashMap<String, (i64, Instant)>,
    last_fetch: Instant,
    status: String,
    input: Option<String>,
    quit: bool,
}

impl App {
    fn new(
        herdr: Herdr,
        queue_dir: Option<PathBuf>,
        options: Options,
        interval: Duration,
        stall: Duration,
    ) -> Self {
        Self {
            herdr,
            queue_dir,
            options,
            interval,
            stall,
            fleet: Fleet::empty(),
            table: TableState::default(),
            since: HashMap::new(),
            last_fetch: Instant::now(),
            status: "starting…".into(),
            input: None,
            quit: false,
        }
    }

    fn refresh(&mut self) {
        match Fleet::collect(&self.herdr, self.queue_dir.as_deref(), &self.options) {
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
                match self.table.selected() {
                    Some(i) if i < fleet.agents.len() => {}
                    _ => self.table.select(if fleet.agents.is_empty() {
                        None
                    } else {
                        Some(0)
                    }),
                }
                self.status = format!(
                    "{} agents · {} queues · herdr {}",
                    fleet.agents.len(),
                    fleet.queues.len(),
                    fleet.herdr_version
                );
                self.fleet = fleet;
                self.last_fetch = now;
            }
            Err(e) => self.status = format!("error: {e:#}"),
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
            KeyCode::Char('i') if self.selected_pane().is_some() => {
                self.input = Some(String::new());
            }
            _ => {}
        }
    }

    fn state_cell(&self, a: &AgentRow) -> (String, Color) {
        let elapsed = self.since.get(&a.pane_id).map(|(_, t)| t.elapsed());
        let mut text = elapsed.map(fmt_duration).unwrap_or_default();
        let mut color = match a.status.as_str() {
            "working" => Color::Yellow,
            "blocked" => Color::Red,
            "idle" => Color::Cyan,
            "done" => Color::Green,
            _ => Color::Gray,
        };
        if a.status == "working" {
            if let Some(e) = elapsed {
                if e >= self.stall {
                    color = Color::Red;
                    text.push('!');
                }
            }
        }
        if text.is_empty() {
            text.push('-');
        }
        (text, color)
    }

    fn render(&mut self, f: &mut Frame) {
        // Size the fleet and queue blocks to their content, so neither leaves
        // a big empty box; the filler between them absorbs the slack.
        let has_queues = self.queue_dir.is_some() && !self.fleet.queues.is_empty();
        let table_h = (self.fleet.agents.len() as u16 + 3).clamp(4, 30);
        let mut constraints = vec![Constraint::Length(4), Constraint::Length(table_h)];
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
        self.render_header(f, rows[i]);
        i += 1;
        self.render_table(f, rows[i]);
        i += 1;
        if has_queues {
            self.render_queues(f, rows[i]);
            i += 1;
        }
        i += 1; // filler
        self.render_footer(f, rows[i]);
        if let Some(buf) = self.input.clone() {
            self.render_input(f, f.area(), &buf);
        }
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let age = self.last_fetch.elapsed().as_secs();
        let title = Line::from(vec![
            Span::styled(
                " herdr-fleet ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}", self.status)),
        ]);
        let legend = Line::from(vec![
            Span::styled(
                format!("  refreshed {age}s ago   "),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("working", Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled("idle", Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled("done", Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled("stall/loop/dialog", Style::default().fg(Color::Red)),
            Span::styled(
                "   IN STATE: observed since launch",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(
            Paragraph::new(vec![title, legend]).block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    fn render_table(&mut self, f: &mut Frame, area: Rect) {
        let header = Row::new(vec![
            "",
            "PANE",
            "NAME",
            "STATUS",
            "MODEL",
            "CTX",
            "IN STATE",
            "FLAGS",
            "LAST OUTPUT",
        ])
        .style(
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        );

        let rows: Vec<Row> = self
            .fleet
            .agents
            .iter()
            .map(|a| {
                let (state, color) = self.state_cell(a);
                Row::new(vec![
                    Cell::from(if a.focused { "●" } else { " " }),
                    Cell::from(a.pane_id.clone()),
                    Cell::from(truncate(&a.name, 20)),
                    Cell::from(a.status.clone()),
                    Cell::from(a.model.clone().unwrap_or_else(|| "-".into())),
                    Cell::from(
                        a.ctx_percent
                            .map(|c| format!("{c}%"))
                            .unwrap_or_else(|| "-".into()),
                    ),
                    Cell::from(state),
                    Cell::from(truncate(&a.flags.join("; "), 26)),
                    Cell::from(truncate(&a.last_line, 60)),
                ])
                .style(Style::default().fg(color))
            })
            .collect();

        let widths = [
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(24),
            Constraint::Length(8),
            Constraint::Length(16),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(28),
            Constraint::Min(20),
        ];
        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(" fleet "))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ")
            .column_spacing(1);
        f.render_stateful_widget(table, area, &mut self.table);
    }

    fn render_queues(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .fleet
            .queues
            .iter()
            .map(|q| {
                let open_color = if q.open > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                Line::from(vec![
                    Span::styled(
                        format!("{:<24}", truncate(&q.name, 24)),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!(" open {:<3}", q.open),
                        Style::default().fg(open_color),
                    ),
                    Span::styled(
                        format!(" done {:<3}  ", q.done),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        truncate(q.first_open.as_deref().unwrap_or(""), 64),
                        Style::default().fg(Color::Gray),
                    ),
                ])
            })
            .collect();
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" queues "))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let hints = " q quit  ·  r refresh  ·  ↑/k ↓/j move  ·  ⏎ focus pane  ·  i send prompt ";
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hints,
                Style::default().fg(Color::DarkGray),
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
            Span::styled("› ", Style::default().fg(Color::Cyan)),
            Span::raw(buf.to_string()),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ]);
        f.render_widget(
            Paragraph::new(line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" send prompt to {pane} ")),
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
) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(
        herdr,
        queue_dir,
        options,
        Duration::from_secs(interval_secs.max(1)),
        Duration::from_secs(stall_secs.max(1)),
    );
    app.refresh();
    let result = app.main_loop(&mut terminal);
    ratatui::restore();
    result
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
}
