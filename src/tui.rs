#![allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
use crate::str_utils;
use crate::types::*;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{
        Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
    },
};
use std::collections::{HashMap, VecDeque};
use std::{io, time::Duration};
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub enum TuiEvent {
    RequestStarted {
        id: String,
        cid: String,
        method: String,
        model: String,
        intent: Option<Intent>,
    },
    StreamUpdate {
        id: String,
        content_delta: String,
        tool_call: Option<String>,
    },
    RequestFinished {
        id: String,
        status: u16,
        latency_ms: u128,
    },
    LogMessage {
        level: String,
        target: String,
        message: String,
        timestamp: String,
    },
    CostUpdate {
        id: String,
        model: String,
        usage: Usage,
        actual_cost: f64,
        potential_cost_no_cache: f64,
    },
    #[allow(dead_code)]
    ServerPulse {
        uptime_secs: u64,
        active_connections: usize,
    },
    UpstreamHealthUpdate {
        consecutive_failures: u32,
        total_requests: u64,
        failed_requests: u64,
        degraded: bool,
    },
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Intent {
    Plan,
    Agent,
    Ask,
    Debug,
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum ActiveTab {
    FlightDeck,
    StreamFocus,
    Console,
    Graphs,
}

#[allow(dead_code)]
struct RequestRecord {
    id: RequestId,
    cid: ConversationId,
    method: String,
    model: String,
    intent: Option<Intent>,
    content: String,
    status: Option<u16>,
    latency: Option<LatencyMs>,
    usage: Option<Usage>,
    actual_cost: Option<CostUsd>,
    potential_cost_no_cache: Option<CostUsd>,
    timestamp: std::time::Instant,
    active_tool: Option<String>,
    last_update: std::time::Instant,
    recorded_in_graphs: bool,
}

pub struct AppState {
    active_tab: ActiveTab,
    requests: VecDeque<RequestRecord>,
    list_state: ListState,
    active_request_id: Option<RequestId>,
    logs: VecDeque<String>,
    server_uptime: u64,
    active_connections: usize,
    total_requests: usize,
    session_cost: CostUsd,
    start_time: std::time::Instant,
    model_costs: HashMap<String, CostUsd>,
    tick: u64,
    should_quit: bool,
    upstream_health: Option<UpstreamHealthDisplay>,
    matrix_effect: MatrixEffect,
    graph_state: GraphState,
}

struct UpstreamHealthDisplay {
    consecutive_failures: u32,
    total_requests: u64,
    failed_requests: u64,
    degraded: bool,
}

struct MatrixDrop {
    x: u16,           // Horizontal column index
    y: f32,           // Vertical position (float for smooth speed handling)
    speed: f32,       // How many cells to drop per tick
    length: usize,    // Number of characters in the tail
    chars: Vec<char>, // The actual characters being displayed
}

struct MatrixEffect {
    drops: Vec<MatrixDrop>,
    width: u16, // Cache width to detect terminal resizes
    rng: fastrand::Rng,
    level: u8, // 0: Off, 1: Sparse, 2: Medium, 3: Heavy
}

#[derive(Debug, Clone)]
struct GraphBucket {
    tps_completion: f64,
    tokens_completion_delta: u64,
    cost_delta: f64,
}

#[derive(Debug, Clone)]
struct ModelGraphData {
    buckets: VecDeque<GraphBucket>,
    current_total_tokens: u64,
    current_total_cost: f64,
}

pub struct GraphState {
    window_size_secs: usize,
    models: HashMap<String, ModelGraphData>,
    model_colors: HashMap<String, Color>,
    color_palette: Vec<Color>,
    current_time: std::time::Instant,
}

impl AppState {
    fn new() -> Self {
        Self {
            active_tab: ActiveTab::FlightDeck,
            requests: VecDeque::with_capacity(50),
            list_state: ListState::default(),
            active_request_id: None,
            logs: VecDeque::with_capacity(1000),
            server_uptime: 0,
            active_connections: 0,
            total_requests: 0,
            session_cost: CostUsd(0.0),
            start_time: std::time::Instant::now(),
            model_costs: HashMap::new(),
            tick: 0,
            should_quit: false,
            upstream_health: None,
            matrix_effect: MatrixEffect::new(80, 2), // Initial width and default level
            graph_state: GraphState::new(),
        }
    }

    fn handle_event(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::UpstreamHealthUpdate {
                consecutive_failures,
                total_requests,
                failed_requests,
                degraded,
            } => self.handle_upstream_health_update(
                consecutive_failures,
                total_requests,
                failed_requests,
                degraded,
            ),
            TuiEvent::RequestStarted {
                id,
                cid,
                method,
                model,
                intent,
            } => self.handle_request_started(
                RequestId(id),
                ConversationId(cid),
                method,
                model,
                intent,
            ),
            TuiEvent::StreamUpdate {
                id,
                content_delta,
                tool_call,
            } => self.handle_stream_update(RequestId(id), content_delta, tool_call),
            TuiEvent::RequestFinished {
                id,
                status,
                latency_ms,
            } => self.handle_request_finished(RequestId(id), status, LatencyMs(latency_ms)),
            TuiEvent::LogMessage {
                timestamp,
                level,
                target,
                message,
            } => self.handle_log_message(timestamp, level, target, message),
            TuiEvent::CostUpdate {
                id,
                model,
                usage,
                actual_cost,
                potential_cost_no_cache,
            } => self.handle_cost_update(
                RequestId(id),
                model,
                usage,
                CostUsd(actual_cost),
                CostUsd(potential_cost_no_cache),
            ),
            TuiEvent::ServerPulse {
                uptime_secs,
                active_connections: _,
            } => {
                self.server_uptime = uptime_secs;
            }
        }
    }

    fn handle_upstream_health_update(
        &mut self,
        consecutive_failures: u32,
        total_requests: u64,
        failed_requests: u64,
        degraded: bool,
    ) {
        self.upstream_health = Some(UpstreamHealthDisplay {
            consecutive_failures,
            total_requests,
            failed_requests,
            degraded,
        });
    }

    fn handle_request_started(
        &mut self,
        id: RequestId,
        cid: ConversationId,
        method: String,
        model: String,
        intent: Option<Intent>,
    ) {
        // If we already have a record for this CID, update it. Otherwise, create one.
        if let Some(req) = self.requests.iter_mut().find(|r| r.cid == cid) {
            req.id = id.clone(); // Update to the latest request ID
            req.model = model;
            req.intent = intent;
            req.method = method;
            req.last_update = std::time::Instant::now();
            req.active_tool = None;
            req.status = None; // Reset status for the new request
        } else {
            self.requests.push_back(RequestRecord {
                id: id.clone(),
                cid,
                method,
                model,
                intent,
                content: String::new(),
                status: None,
                latency: None,
                usage: None,
                actual_cost: None,
                potential_cost_no_cache: None,
                timestamp: std::time::Instant::now(),
                active_tool: None,
                last_update: std::time::Instant::now(),
                recorded_in_graphs: false,
            });
        }

        if self.requests.len() > 50 {
            self.requests.pop_front();
        }
        self.total_requests += 1;
        self.active_connections += 1;

        if self.active_request_id.is_none() {
            self.active_request_id = Some(id);
        }
    }

    fn handle_stream_update(
        &mut self,
        id: RequestId,
        content_delta: String,
        tool_call: Option<String>,
    ) {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == id) {
            req.content.push_str(&content_delta);
            req.last_update = std::time::Instant::now();
            if let Some(tc) = tool_call {
                req.active_tool = Some(tc);
            }
        }
    }

    fn handle_request_finished(&mut self, id: RequestId, status: u16, latency: LatencyMs) {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == id) {
            req.status = Some(status);
            req.latency = Some(latency);
        }
        if self.active_connections > 0 {
            self.active_connections -= 1;
        }
    }

    fn handle_log_message(&mut self, timestamp: String, level: String, target: String, message: String) {
        let log_line = format!("{} [{}] {}: {}", timestamp, level, target, message);
        self.logs.push_back(log_line);
        if self.logs.len() > 1000 {
            self.logs.pop_front();
        }
    }

    fn handle_cost_update(
        &mut self,
        id: RequestId,
        model: String,
        usage: Usage,
        actual_cost: CostUsd,
        potential_cost_no_cache: CostUsd,
    ) {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == id) {
            req.usage = Some(usage);
            req.actual_cost = Some(actual_cost);
            req.potential_cost_no_cache = Some(potential_cost_no_cache);
        }
        self.session_cost.0 += actual_cost.0;
        self.model_costs.entry(model).or_insert(CostUsd(0.0)).0 += actual_cost.0;
    }

    fn select_down(&mut self) {
        if self.active_tab == ActiveTab::FlightDeck {
            let i = match self.list_state.selected() {
                Some(i) => {
                    if i + 1 < self.requests.len().min(12) {
                        i + 1
                    } else {
                        i
                    }
                }
                None => 0,
            };
            self.list_state.select(Some(i));
        }
    }

    fn select_up(&mut self) {
        if self.active_tab == ActiveTab::FlightDeck {
            let i = match self.list_state.selected() {
                Some(i) => i.saturating_sub(1),
                None => 0,
            };
            self.list_state.select(Some(i));
        }
    }

    fn select_right(&mut self) {
        if self.active_tab == ActiveTab::FlightDeck {
            let i = match self.list_state.selected() {
                Some(i) => {
                    if i < 6 && i + 6 < self.requests.len().min(12) {
                        i + 6
                    } else {
                        i
                    }
                }
                None => 0,
            };
            self.list_state.select(Some(i));
        }
    }

    fn select_left(&mut self) {
        if self.active_tab == ActiveTab::FlightDeck {
            let i = match self.list_state.selected() {
                Some(i) => {
                    if i >= 6 {
                        i - 6
                    } else {
                        i
                    }
                }
                None => 0,
            };
            self.list_state.select(Some(i));
        }
    }

    fn focus_selected(&mut self) {
        if self.active_tab == ActiveTab::FlightDeck {
            if let Some(i) = self.list_state.selected() {
                if let Some(req) = self.requests.get(i) {
                    self.active_request_id = Some(req.id.clone());
                    self.active_tab = ActiveTab::StreamFocus;
                }
            }
        }
    }

    fn check_and_record_graph_metrics(&mut self) {
        // Check for completed requests that haven't been recorded yet
        for req in self.requests.iter_mut() {
            if !req.recorded_in_graphs
                && req.status.is_some()
                && req.usage.is_some()
                && req.actual_cost.is_some()
                && req.latency.is_some()
            {
                if let (Some(usage), Some(latency), Some(cost)) =
                    (&req.usage, req.latency, req.actual_cost)
                {
                    self.graph_state.record_request_completion(
                        &req.model,
                        usage.completion_tokens,
                        latency,
                        cost,
                    );
                    req.recorded_in_graphs = true;
                }
            }
        }
    }
}

pub struct App {
    rx: broadcast::Receiver<TuiEvent>,
    state: AppState,
}

impl App {
    pub fn new(rx: broadcast::Receiver<TuiEvent>) -> Self {
        Self {
            rx,
            state: AppState::new(),
        }
    }

    pub async fn run(mut self) -> io::Result<()> {
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen);
            original_hook(panic_info);
        }));

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        loop {
            terminal.draw(|f| self.render(f))?;

            if crossterm::event::poll(Duration::from_millis(10))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match key.code {
                            KeyCode::Char('q') => self.state.should_quit = true,
                            KeyCode::Char('r') => self.state.matrix_effect.cycle_level(),
                            KeyCode::Char('1') => self.state.active_tab = ActiveTab::FlightDeck,
                            KeyCode::Char('2') => self.state.active_tab = ActiveTab::StreamFocus,
                            KeyCode::Char('3') => self.state.active_tab = ActiveTab::Console,
                            KeyCode::Char('4') => self.state.active_tab = ActiveTab::Graphs,

                            // Vim keybindings for grid navigation
                            KeyCode::Char('k') | KeyCode::Up => self.state.select_up(),
                            KeyCode::Char('j') | KeyCode::Down => self.state.select_down(),
                            KeyCode::Char('h') | KeyCode::Left => self.state.select_left(),
                            KeyCode::Char('l') | KeyCode::Right => self.state.select_right(),

                            KeyCode::Enter => self.state.focus_selected(),
                            KeyCode::Esc => self.state.active_tab = ActiveTab::FlightDeck,
                            _ => {}
                        }
                    }
                    Event::Resize(w, _h) => {
                        self.state.matrix_effect.resize(w);
                        terminal.autoresize()?;
                    }
                    _ => {}
                }
            }

            self.state.tick = self.state.tick.wrapping_add(1);
            self.state.matrix_effect.update(terminal.size()?.height);

            while let Ok(event) = self.rx.try_recv() {
                self.state.handle_event(event);
            }

            // Check for completed graph metrics
            self.state.check_and_record_graph_metrics();

            if self.state.should_quit {
                break;
            }
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }

    fn render(&mut self, f: &mut Frame) {
        self.state.matrix_effect.render(f);

        if f.size().width < 75 {
            self.render_size_warning(f, f.size());
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Header + Navigation
                Constraint::Min(0),    // Main Content
                Constraint::Length(3), // Footer
            ])
            .split(f.size());

        self.render_header(f, chunks[0]);

        match self.state.active_tab {
            ActiveTab::FlightDeck => self.render_flight_deck(f, chunks[1]),
            ActiveTab::StreamFocus => self.render_stream_focus(f, chunks[1]),
            ActiveTab::Console => self.render_console(f, chunks[1]),
            ActiveTab::Graphs => self.render_graphs(f, chunks[1]),
        }

        self.render_footer(f, chunks[2]);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(area);

        self.render_header_top(f, chunks[0], area.width < 85);
        self.render_header_middle(f, chunks[1], area.width < 85);
        f.render_widget(Paragraph::new("─".repeat(area.width as usize)), chunks[2]);
    }

    fn render_header_top(&self, f: &mut Frame, area: Rect, is_compact: bool) {
        let uptime = self.state.start_time.elapsed().as_secs();
        let hours = uptime / 3600;
        let minutes = (uptime % 3600) / 60;
        let seconds = uptime % 60;

        let header_text = if is_compact {
            format!(
                " SHIM v1.0 | {:02}:{:02}:{:02} | ${:.4} | Act: {} ",
                hours,
                minutes,
                seconds,
                self.state.session_cost.0,
                self.state.active_connections
            )
        } else {
            format!(
                " SHIM v1.0  [●] ONLINE  |  Session: {:02}:{:02}:{:02}  |  {:.1}k Toks / ${:.4}  |  Active: {} ",
                hours, minutes, seconds,
                self.state.total_requests as f64 * 0.45, // Approximation
                self.state.session_cost.0,
                self.state.active_connections
            )
        };

        let mut header_spans = vec![Span::styled(
            header_text,
            Style::default().add_modifier(Modifier::BOLD),
        )];

        if let Some(health) = &self.state.upstream_health {
            let health_color = if health.consecutive_failures > 0 || health.degraded {
                Color::Red
            } else {
                Color::Green
            };
            let success_rate = if health.total_requests > 0 {
                (health.total_requests - health.failed_requests) as f64
                    / health.total_requests as f64
                    * 100.0
            } else {
                100.0
            };

            let health_text = if is_compact {
                format!(
                    " | UP: {:.0}% ({}) ",
                    success_rate, health.consecutive_failures
                )
            } else {
                format!(
                    " | UPSTREAM: {:.1}% ({}) ",
                    success_rate, health.consecutive_failures
                )
            };

            header_spans.push(Span::styled(
                health_text,
                Style::default()
                    .fg(health_color)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let header = Paragraph::new(Line::from(header_spans))
            .style(Style::default().bg(Color::White).fg(Color::Black));
        f.render_widget(header, area);
    }

    fn render_header_middle(&self, f: &mut Frame, area: Rect, is_compact: bool) {
        let nav_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        self.render_tab_navigation(f, nav_layout[0], is_compact);
        self.render_tps_stats(f, nav_layout[1], is_compact);
    }

    fn render_tab_navigation(&self, f: &mut Frame, area: Rect, is_compact: bool) {
        let tabs = vec![
            Span::raw(if is_compact {
                " TABS: "
            } else {
                " TAB NAVIGATION:  "
            }),
            self.render_tab_item("[1] VIEW", "[1]", "GRID", ActiveTab::FlightDeck, is_compact),
            Span::raw("  "),
            self.render_tab_item(
                "[2] FOCUS",
                "[2]",
                "STREAM",
                ActiveTab::StreamFocus,
                is_compact,
            ),
            Span::raw("  "),
            self.render_tab_item("[3] SYSTEM", "[3]", "LOGS", ActiveTab::Console, is_compact),
            Span::raw("  "),
            self.render_tab_item("[4] GRAPHS", "[4]", "GRAPHS", ActiveTab::Graphs, is_compact),
        ];
        f.render_widget(Paragraph::new(Line::from(tabs)), area);
    }

    fn render_tab_item(
        &self,
        full_label: &str,
        compact_label: &str,
        tab_name: &str,
        tab: ActiveTab,
        is_compact: bool,
    ) -> Span<'_> {
        let (label, is_active) = if is_compact {
            (compact_label, self.state.active_tab == tab)
        } else {
            (full_label, self.state.active_tab == tab)
        };

        let tab_text = format!("{}{}{}", label, tab_name, if is_active { "*" } else { "" });

        if is_active {
            Span::styled(
                tab_text,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(tab_text, Style::default())
        }
    }

    fn render_tps_stats(&self, f: &mut Frame, area: Rect, is_compact: bool) {
        if is_compact {
            return;
        }

        let mut model_stats: HashMap<String, (u64, u128)> = HashMap::new();
        let mut total_tokens = 0;
        let mut total_time = 0;

        // Only consider requests from the last 5 minutes for more relevant TPS
        let now = std::time::Instant::now();
        for req in &self.state.requests {
            if now.duration_since(req.timestamp).as_secs() > 300 {
                continue;
            }
            if let (Some(usage), Some(latency)) = (&req.usage, req.latency) {
                if latency.0 > 0 {
                    let entry = model_stats.entry(req.model.clone()).or_default();
                    entry.0 += usage.completion_tokens as u64;
                    entry.1 += latency.0;

                    total_tokens += usage.completion_tokens as u64;
                    total_time += latency.0;
                }
            }
        }

        let system_tps = if total_time > 0 {
            (total_tokens as f64 / total_time as f64) * 1000.0
        } else {
            0.0
        };

        let mut tps_data: Vec<(String, f64)> = model_stats
            .into_iter()
            .map(|(model, (tokens, ms))| {
                let tps = if ms > 0 {
                    (tokens as f64 / ms as f64) * 1000.0
                } else {
                    0.0
                };
                (model, tps)
            })
            .collect();

        tps_data.sort_by(|a, b| match b.1.partial_cmp(&a.1) {
            Some(ord) => ord,
            None => std::cmp::Ordering::Equal,
        });

        let first_tps = match tps_data.first() {
            Some(x) => x.1,
            None => 1.0,
        };
        let max_tps = first_tps.max(1.0);

        let mut stat_spans = vec![
            Span::styled(
                format!(" AVG TPS: {:.1} ", system_tps),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("| "),
        ];

        for (model, tps) in tps_data.iter().take(3) {
            let bar_len = ((tps / max_tps) * 8.0).ceil() as usize;
            let bar = "▇".repeat(bar_len.min(8));
            // Simplify model name
            let simple_name = match model.split('/').next_back() {
                Some(name) => name,
                None => model,
            };
            let short_name = str_utils::prefix_chars(simple_name, 10);

            stat_spans.push(Span::styled(
                format!("{}: {} ", short_name, bar),
                Style::default().fg(Color::Cyan),
            ));
        }

        f.render_widget(
            Paragraph::new(Line::from(stat_spans)).alignment(Alignment::Right),
            area,
        );
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let is_compact = area.width < 85;
        let footer_text = if is_compact {
            " [Q] Quit | [R] Rain | [1-4] Tabs | [Ent] Focus | [Esc] Back "
        } else {
            " [Q] Quit application | [R] Cycle Rain | [1-4] Switch Tabs | [Enter] Focus Stream | [Esc] Back "
        };
        let footer = Paragraph::new(footer_text).block(
            Block::default()
                .borders(Borders::TOP)
                .border_type(BorderType::Plain),
        );
        f.render_widget(footer, area);
    }

    fn render_flight_deck(&mut self, f: &mut Frame, area: Rect) {
        // Use more of the space if available, but keep it centered if extremely wide
        let deck_area = if area.width > 200 {
            self.centered_rect(80, 100, area)
        } else {
            area
        };

        // Main grid layout: 2 columns, 6 rows = 12 stable slots
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(deck_area);

        let left_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 6); 6])
            .split(columns[0]);

        let right_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 6); 6])
            .split(columns[1]);

        let mut slots: Vec<Rect> = Vec::new();
        slots.extend(left_rows.iter());
        slots.extend(right_rows.iter());

        // Fill the 12 slots based on the conversations we have.
        // If we have fewer than 12, they fill from the first slot.
        // If we have more than 12, we show the 12 most recent ones.
        let num_to_take = self.state.requests.len().min(12);
        let display_reqs: Vec<_> = self
            .state
            .requests
            .iter()
            .rev() // Get newest first
            .take(num_to_take)
            .collect();

        for i in 0..12 {
            let slot_area = slots[i];

            if i < display_reqs.len() {
                let req = display_reqs[i];
                let is_selected = self.state.list_state.selected() == Some(i);

                // Cursor Mode Colors: Ask=Green, Plan=Yellow, Agent=Red, Debug=Magenta
                let border_color = match req.intent {
                    Some(Intent::Ask) => Color::Green,
                    Some(Intent::Plan) => Color::Yellow,
                    Some(Intent::Agent) => Color::Red,
                    Some(Intent::Debug) => Color::Magenta,
                    None => Color::DarkGray,
                };

                let border_type = if is_selected {
                    BorderType::Thick
                } else {
                    BorderType::Rounded
                };

                let last_update_ms = req.last_update.elapsed().as_millis();
                let is_streaming = req.status.is_none();

                let heartbeat_icon = if is_streaming {
                    if last_update_ms < 150 {
                        "●"
                    } else if last_update_ms < 400 {
                        "○"
                    } else {
                        " "
                    }
                } else {
                    " "
                };

                let heartbeat_style = if last_update_ms < 150 {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let status_indicator = if let Some(s) = req.status {
                    if s == 200 {
                        Span::styled("● OK", Style::default().fg(Color::Green))
                    } else {
                        Span::styled(format!("● ERR {}", s), Style::default().fg(Color::Red))
                    }
                } else {
                    let spinner = match (self.state.tick / 10) % 4 {
                        0 => "/",
                        1 => "-",
                        2 => "\\",
                        _ => "|",
                    };
                    Span::styled(
                        format!("{} STRM", spinner),
                        Style::default().fg(Color::Yellow),
                    )
                };

                let cost_cents = match req.actual_cost {
                    Some(c) => c.0 * 100.0,
                    None => 0.0,
                };
                let savings_cents = match (req.actual_cost, req.potential_cost_no_cache) {
                    (Some(actual), Some(potential)) => (potential.0 - actual.0) * 100.0,
                    _ => 0.0,
                };

                // Color cost based on efficiency (just an example: if it's very cheap/cached, make it brighter)
                let cost_color = if cost_cents < 0.5 {
                    Color::LightCyan
                } else {
                    Color::Green
                };

                let cost_display = if savings_cents > 0.01 {
                    format!("{:.1}¢ ({:.1}¢)", cost_cents, savings_cents)
                } else {
                    format!("{:.1}¢", cost_cents)
                };

                let time_text = match req.latency {
                    Some(l) => format!("{:.1}s", l.0 as f64 / 1000.0),
                    None => format!("{:.1}s", req.timestamp.elapsed().as_secs_f64()),
                };

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(border_type)
                    .border_style(if is_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(border_color)
                    })
                    .bg(Color::Black)
                    .title_alignment(Alignment::Left)
                    .title(Line::from(vec![
                        Span::styled(
                            format!(" #{} ", req.cid.short()),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" {} ", req.model.to_uppercase()),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                    ]));

                let inner = block.inner(slot_area);
                f.render_widget(block, slot_area);

                // Split inner area into stats and tool line
                let inner_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .split(inner);

                // Use a Table for rock-solid horizontal stability
                let table_widths = [
                    Constraint::Fill(1), // Intent
                    Constraint::Fill(1), // Status + Heartbeat
                    Constraint::Fill(1), // Cost
                    Constraint::Fill(1), // Time
                ];

                let stats_row = Row::new(vec![
                    Cell::from(Span::styled(
                        match req.intent {
                            Some(i) => format!("{:?}", i).to_uppercase(),
                            None => "AUTO".to_string(),
                        },
                        Style::default().fg(border_color),
                    )),
                    Cell::from(Line::from(vec![
                        status_indicator,
                        Span::raw(" "),
                        Span::styled(heartbeat_icon, heartbeat_style),
                    ])),
                    Cell::from(Span::styled(cost_display, Style::default().fg(cost_color))),
                    Cell::from(Span::raw(time_text.trim())),
                ]);

                let stats_table = Table::new(vec![stats_row], table_widths).column_spacing(1);

                f.render_widget(stats_table, inner_layout[0]);

                let tool_text = match req.active_tool.as_deref() {
                    Some(tool) => tool,
                    None => "idle",
                };
                let tool_line = Line::from(vec![
                    Span::styled("🛠 ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        tool_text,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]);

                f.render_widget(
                    Paragraph::new(tool_line).wrap(Wrap { trim: true }),
                    inner_layout[1],
                );
            } else {
                let is_selected = self.state.list_state.selected() == Some(i);
                // Empty slot placeholder
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(if is_selected {
                        BorderType::Thick
                    } else {
                        BorderType::Rounded
                    })
                    .border_style(if is_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM)
                    })
                    .bg(Color::Black)
                    .title(Span::styled(
                        " EMPTY SLOT ",
                        Style::default().fg(Color::DarkGray),
                    ));
                f.render_widget(block, slot_area);
            }
        }
    }

    fn render_stream_focus(&self, f: &mut Frame, area: Rect) {
        let selected = self
            .state
            .active_request_id
            .as_ref()
            .and_then(|id| self.state.requests.iter().find(|r| &r.id == id));

        if let Some(req) = selected {
            let intent_color = match req.intent {
                Some(Intent::Agent) => Color::Red,
                Some(Intent::Plan) => Color::Cyan,
                Some(Intent::Ask) => Color::White,
                Some(Intent::Debug) => Color::Magenta,
                None => Color::DarkGray,
            };

            let is_compact = area.width < 85;

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),                              // Info row 1
                    Constraint::Length(if is_compact { 4 } else { 3 }), // Info row 2 (Stats)
                    Constraint::Min(0),                                 // Content
                ])
                .split(area);

            // Row 1: Session Info
            let header1 = if is_compact {
                format!(" FOCUS: #{} | ", req.id.short())
            } else {
                format!(" FOCUS SESSION: #{} | MODE: ", req.id.short())
            };
            let header1_p = Paragraph::new(Line::from(vec![
                Span::raw(header1),
                Span::styled(
                    match req.intent {
                        Some(i) => format!(" [{:?}] ", i).to_uppercase(),
                        None => " [AUTO] ".to_string(),
                    },
                    Style::default()
                        .bg(intent_color)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .block(Block::default().borders(Borders::ALL).bg(Color::Black));
            f.render_widget(header1_p, layout[0]);

            // Row 2: Stats
            let tps = if let Some(latency) = req.latency {
                let tokens = match req.usage.as_ref() {
                    Some(u) => u.completion_tokens,
                    None => 0,
                };
                if latency.0 > 0 {
                    (tokens as f64 / (latency.0 as f64 / 1000.0)) as u32
                } else {
                    0
                }
            } else {
                0
            };

            let cache_pct = match req.usage.as_ref() {
                Some(u) => {
                    let total = u.total_tokens as f64;
                    let cached = match u.prompt_tokens_details.as_ref() {
                        Some(d) => match d.cached_tokens {
                            Some(c) => c,
                            None => 0,
                        },
                        None => 0,
                    } as f64;
                    if total > 0.0 {
                        (cached / total * 100.0) as u32
                    } else {
                        0
                    }
                }
                None => 0,
            };

            let (actual_cost, savings) = match (req.actual_cost, req.potential_cost_no_cache) {
                (Some(actual), Some(potential)) => (actual.0, potential.0 - actual.0),
                _ => (0.0, 0.0),
            };

            let latency_val = match req.latency {
                Some(l) => l.0,
                None => 0,
            };

            let prompt_tokens = match req.usage.as_ref() {
                Some(u) => u.prompt_tokens,
                None => 0,
            };

            let completion_tokens = match req.usage.as_ref() {
                Some(u) => u.completion_tokens,
                None => 0,
            };

            let stats_text = if is_compact {
                format!(
                    " MDL: {} | LAT: {}ms | TPS: {} | CST: {:.3}¢ ({:.3}¢)\n INP: {} | OUT: {} | CCH: {}% ",
                    req.model,
                    latency_val,
                    tps,
                    actual_cost * 100.0,
                    savings * 100.0,
                    prompt_tokens,
                    completion_tokens,
                    cache_pct
                )
            } else {
                format!(
                    " MODEL: {} | LATENCY: {}ms | TPS: {} | COST: {:.3}¢ (SAVED: {:.3}¢) | INPUT: {} | OUTPUT: {} | CACHE: {}% ",
                    req.model,
                    latency_val,
                    tps,
                    actual_cost * 100.0,
                    savings * 100.0,
                    prompt_tokens,
                    completion_tokens,
                    cache_pct
                )
            };
            let stats_p = Paragraph::new(stats_text)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).bg(Color::Black));
            f.render_widget(stats_p, layout[1]);

            // Chat Content
            let mut chat_content = Vec::new();
            chat_content.push(Line::from(vec![
                Span::styled(
                    "> [USER] ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("(Captured snippet here...)"),
            ]));
            chat_content.push(Line::from(""));
            chat_content.push(Line::from(
                "─".repeat(area.width.saturating_sub(2) as usize),
            ));
            chat_content.push(Line::from(""));
            chat_content.push(Line::from(vec![
                Span::styled(
                    "> [ASSISTANT] ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                if req.status.is_none() {
                    Span::styled("(Streaming...) ▊", Style::default().fg(Color::Green))
                } else {
                    Span::raw("")
                },
            ]));

            // Add actual response content
            for line in req.content.lines() {
                chat_content.push(Line::from(format!("  {}", line)));
            }

            let p = Paragraph::new(chat_content)
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .bg(Color::Black),
                );

            f.render_widget(p, layout[2]);
        } else {
            let p = Paragraph::new(
                "No stream selected. Select a request in Flight Deck and press Enter.",
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Stream Focus ")
                    .bg(Color::Black),
            );
            f.render_widget(p, area);
        }
    }

    fn render_console(&self, f: &mut Frame, area: Rect) {
        let logs_to_show: Vec<ListItem> = self
            .state
            .logs
            .iter()
            .rev()
            .take(area.height.saturating_sub(2) as usize)
            .rev()
            .map(|line| {
                let style = if line.contains("[ERROR]") {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else if line.contains("[WARN]") {
                    Style::default().fg(Color::Yellow)
                } else if line.contains("[DEBUG]") {
                    Style::default().fg(Color::DarkGray)
                } else if line.contains("[INFO]") {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                ListItem::new(line.clone()).style(style)
            })
            .collect();

        let list = List::new(logs_to_show).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" SYSTEM LOGS ")
                .border_style(Style::default().fg(Color::DarkGray))
                .bg(Color::Black),
        );

        f.render_widget(list, area);
    }

    fn render_graphs(&self, f: &mut Frame, area: Rect) {
        let is_compact = area.width < 85;

        // Layout: Top legend/table, then 3 chart areas
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if is_compact { 3 } else { 4 }), // Legend/table
                Constraint::Percentage(40),                         // TPS chart
                Constraint::Percentage(30),                         // Tokens chart
                Constraint::Percentage(30),                         // Cost chart
            ])
            .split(area);

        // Render legend/table
        self.render_graphs_legend(f, chunks[0]);

        // Render the three charts
        self.render_tps_chart(f, chunks[1]);
        self.render_tokens_chart(f, chunks[2]);
        self.render_cost_chart(f, chunks[3]);
    }

    fn render_graphs_legend(&self, f: &mut Frame, area: Rect) {
        let mut legend_items = Vec::new();

        // Add header
        if area.width > 60 {
            legend_items.push(Row::new(vec![
                Cell::from(Span::styled(
                    "MODEL",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    "TPS",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    "TOKENS",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    "COST",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ]));
        }

        // Add per-model data
        let models: Vec<_> = self.state.graph_state.models.keys().collect();
        for model in models {
            let color = self.state.graph_state.get_model_color(model);
            let model_data = &self.state.graph_state.models[model];

            let current_tps = model_data
                .buckets
                .back()
                .map(|b| b.tps_completion)
                .unwrap_or(0.0);

            let row = if area.width > 60 {
                Row::new(vec![
                    Cell::from(Span::styled(
                        model,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )),
                    Cell::from(Span::raw(format!("{:.1}", current_tps))),
                    Cell::from(Span::raw(model_data.current_total_tokens.to_string())),
                    Cell::from(Span::raw(format!("${}", model_data.current_total_cost))),
                ])
            } else {
                Row::new(vec![
                    Cell::from(Span::styled(
                        model,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )),
                    Cell::from(Span::raw(format!("{:.1} TPS", current_tps))),
                ])
            };
            legend_items.push(row);
        }

        // Add totals row
        let total_tps = self.state.graph_state.get_total_tps();
        let total_tokens = self.state.graph_state.get_total_tokens();
        let total_cost = self.state.graph_state.get_total_cost();

        let totals_row = if area.width > 60 {
            Row::new(vec![
                Cell::from(Span::styled(
                    "TOTAL",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    format!("{:.1}", total_tps),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    total_tokens.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    format!("${:.4}", total_cost),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ])
        } else {
            Row::new(vec![
                Cell::from(Span::styled(
                    "TOTAL",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Cell::from(Span::styled(
                    format!("{:.1} TPS", total_tps),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ])
        };
        legend_items.push(totals_row);

        let widths = if area.width > 60 {
            vec![
                Constraint::Percentage(40),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
            ]
        } else {
            vec![Constraint::Percentage(60), Constraint::Percentage(40)]
        };

        let table = Table::new(legend_items, widths)
            .block(Block::default().borders(Borders::ALL).title(" LEGEND "));

        f.render_widget(table, area);
    }

    fn render_tps_chart(&self, f: &mut Frame, area: Rect) {
        self.render_stacked_chart(
            f,
            area,
            " COMPLETION TPS ",
            |model_data| {
                model_data
                    .buckets
                    .back()
                    .map(|b| b.tps_completion)
                    .unwrap_or(0.0)
            },
            true, // show current value
        );
    }

    fn render_tokens_chart(&self, f: &mut Frame, area: Rect) {
        self.render_stacked_chart(
            f,
            area,
            " COMPLETION TOKENS (5m window) ",
            |model_data| model_data.current_total_tokens as f64,
            false, // don't show current value for cumulative
        );
    }

    fn render_cost_chart(&self, f: &mut Frame, area: Rect) {
        self.render_stacked_chart(
            f,
            area,
            " COST (5m window) ",
            |model_data| model_data.current_total_cost,
            false, // don't show current value for cumulative
        );
    }

    fn render_stacked_chart<F>(
        &self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        get_value: F,
        show_current: bool,
    ) where
        F: Fn(&ModelGraphData) -> f64,
    {
        let block = Block::default().borders(Borders::ALL).title(title);

        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.state.graph_state.models.is_empty() {
            let p = Paragraph::new("No data available")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(p, inner);
            return;
        }

        // Calculate chart dimensions
        let chart_width = inner.width.saturating_sub(2) as usize;
        let chart_height = inner.height.saturating_sub(2) as usize;

        if chart_width < 10 || chart_height < 3 {
            let p = Paragraph::new("Chart area too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(p, inner);
            return;
        }

        // Get current values for each model
        let mut model_values: Vec<(String, f64, Color)> = self
            .state
            .graph_state
            .models
            .iter()
            .map(|(model, data)| {
                let value = get_value(data);
                let color = self.state.graph_state.get_model_color(model);
                (model.clone(), value, color)
            })
            .collect();

        // Sort by value for better stacking (largest at bottom)
        model_values.sort_by(|a, b| match b.1.partial_cmp(&a.1) {
            Some(ord) => ord,
            None => std::cmp::Ordering::Equal,
        });

        let total_value: f64 = model_values.iter().map(|(_, v, _)| v).sum();

        if total_value <= 0.0 {
            let p = Paragraph::new("No activity")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(p, inner);
            return;
        }

        // Render the stacked bars
        for (x_offset, (_model, value, color)) in model_values.iter().enumerate() {
            if x_offset >= chart_width {
                break;
            }

            let bar_height = ((value / total_value) * chart_height as f64).round() as usize;
            if bar_height == 0 {
                continue;
            }

            // Render vertical stack for this x position
            for y in 0..bar_height {
                let y_pos = inner.y + inner.height - 2 - y as u16; // Start from bottom
                let x_pos = inner.x + 1 + x_offset as u16;

                if y_pos > inner.y && y_pos < inner.y + inner.height - 1 {
                    let cell = f.buffer_mut().get_mut(x_pos, y_pos);
                    {
                        cell.set_char('█').set_style(Style::default().fg(*color));
                    }
                }
            }
        }

        // Add current total value on the right side
        if show_current {
            let total_text = format!("{:.1}", total_value);
            let total_span = Span::styled(
                &total_text,
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::White),
            );

            let total_area = Rect {
                x: inner.x + inner.width.saturating_sub(total_text.len() as u16 + 2),
                y: inner.y,
                width: total_text.len() as u16 + 2,
                height: 1,
            };

            if total_area.width > 0 {
                f.render_widget(Paragraph::new(total_span), total_area);
            }
        }
    }

    fn render_size_warning(&self, f: &mut Frame, area: Rect) {
        let msg = vec![
            Line::from(vec![Span::styled(
                " TERMINAL TOO SMALL ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(format!(" Current width: {} ", area.width)),
            Line::from(" Minimum required: 75 "),
            Line::from(""),
            Line::from(" Please resize your terminal to continue. "),
        ];

        let p = Paragraph::new(msg)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });

        // Center the warning
        let area = self.centered_rect(60, 40, area);
        f.render_widget(p, area);
    }
}

impl GraphState {
    fn new() -> Self {
        Self {
            window_size_secs: 300, // 5 minutes
            models: HashMap::new(),
            model_colors: HashMap::new(),
            color_palette: vec![
                Color::Cyan,
                Color::Green,
                Color::Yellow,
                Color::Magenta,
                Color::Red,
                Color::Blue,
                Color::White,
                Color::Gray,
            ],
            current_time: std::time::Instant::now(),
        }
    }

    fn get_or_create_model_data(&mut self, model: &str) -> &mut ModelGraphData {
        // Assign color if new model
        if !self.model_colors.contains_key(model) {
            let color_index = self.model_colors.len() % self.color_palette.len();
            self.model_colors
                .insert(model.to_string(), self.color_palette[color_index]);
        }

        self.models
            .entry(model.to_string())
            .or_insert_with(|| ModelGraphData {
                buckets: VecDeque::with_capacity(300),
                current_total_tokens: 0,
                current_total_cost: 0.0,
            })
    }

    fn advance_time(&mut self) {
        let now = std::time::Instant::now();
        let elapsed_secs = now.duration_since(self.current_time).as_secs() as usize;

        if elapsed_secs > 0 {
            // Add empty buckets for elapsed time for all existing models
            for _ in 0..elapsed_secs.min(self.window_size_secs) {
                for model_data in self.models.values_mut() {
                    if model_data.buckets.len() >= self.window_size_secs {
                        model_data.buckets.pop_front();
                    }
                    model_data.buckets.push_back(GraphBucket {
                        tps_completion: 0.0,
                        tokens_completion_delta: 0,
                        cost_delta: 0.0,
                    });
                }
            }
            self.current_time = now;
        }
    }

    fn record_request_completion(
        &mut self,
        model: &str,
        completion_tokens: u32,
        latency_ms: LatencyMs,
        actual_cost: CostUsd,
    ) {
        self.advance_time();

        let model_data = self.get_or_create_model_data(model);

        let LatencyMs(latency_val) = latency_ms;
        let CostUsd(cost_val) = actual_cost;

        // Calculate TPS (tokens per second)
        let latency_secs = latency_val as f64 / 1000.0;
        let tps = if latency_secs > 0.0 {
            completion_tokens as f64 / latency_secs
        } else {
            0.0
        };

        // Update current totals
        model_data.current_total_tokens += completion_tokens as u64;
        model_data.current_total_cost += cost_val;

        // Ensure we have at least one bucket, then add to latest bucket
        if model_data.buckets.is_empty() {
            model_data.buckets.push_back(GraphBucket {
                tps_completion: 0.0,
                tokens_completion_delta: 0,
                cost_delta: 0.0,
            });
        }

        if let Some(bucket) = model_data.buckets.back_mut() {
            bucket.tps_completion += tps;
            bucket.tokens_completion_delta += completion_tokens as u64;
            bucket.cost_delta += cost_val;
        }
    }

    fn get_model_color(&self, model: &str) -> Color {
        match self.model_colors.get(model) {
            Some(color) => *color,
            None => Color::White,
        }
    }

    fn get_total_tps(&self) -> f64 {
        self.models
            .values()
            .flat_map(|data| data.buckets.back())
            .map(|bucket| bucket.tps_completion)
            .sum()
    }

    fn get_total_tokens(&self) -> u64 {
        self.models
            .values()
            .map(|data| data.current_total_tokens)
            .sum()
    }

    fn get_total_cost(&self) -> f64 {
        self.models
            .values()
            .map(|data| data.current_total_cost)
            .sum()
    }
}

impl MatrixEffect {
    fn new(width: u16, level: u8) -> Self {
        let mut effect = Self {
            drops: Vec::new(),
            width,
            rng: fastrand::Rng::new(),
            level,
        };
        effect.initialize_drops();
        effect
    }

    fn cycle_level(&mut self) {
        self.level = (self.level + 1) % 4;
        self.initialize_drops();
    }

    fn initialize_drops(&mut self) {
        self.drops.clear();
        if self.level == 0 {
            return;
        }

        // Levels: 1 (Sparse) = 6, 2 (Medium) = 3, 3 (Heavy) = 1
        let step = match self.level {
            1 => 6,
            2 => 3,
            3 => 1,
            _ => 3,
        };

        for x in (0..self.width).step_by(step) {
            self.drops.push(MatrixDrop {
                x,
                y: self.rng.f32() * -20.0,
                speed: self.rng.f32() * 0.5 + 0.1,
                length: self.rng.usize(5..15),
                chars: (0..20)
                    .map(|_| Self::random_matrix_char_with_rng(&mut self.rng))
                    .collect(),
            });
        }
    }

    fn random_matrix_char_with_rng(rng: &mut fastrand::Rng) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@#$%^&*()[]{}|;:,.<>?";
        let idx = rng.usize(0..CHARS.len());
        CHARS[idx] as char
    }

    fn update(&mut self, height: u16) {
        if self.level == 0 {
            return;
        }

        for drop in &mut self.drops {
            drop.y += drop.speed;

            // Reset drop when it goes off screen
            if drop.y > height as f32 {
                drop.y = self.rng.f32() * -10.0;
                drop.speed = self.rng.f32() * 0.5 + 0.1;
                drop.length = self.rng.usize(5..15);

                // Randomize a few characters for flicker effect
                let chars_len = drop.chars.len();
                if chars_len > 0 {
                    for _ in 0..3 {
                        let idx = self.rng.usize(0..chars_len);
                        drop.chars[idx] = Self::random_matrix_char_with_rng(&mut self.rng);
                    }
                }
            }
        }
    }

    fn resize(&mut self, new_width: u16) {
        if new_width != self.width {
            self.width = new_width;
            self.initialize_drops();
        }
    }

    fn render(&mut self, f: &mut Frame) {
        if self.level == 0 {
            return;
        }

        let area = f.size();
        if area.width != self.width {
            self.resize(area.width);
        }

        for drop in &self.drops {
            let x = drop.x;
            for i in 0..drop.length {
                let y_val = drop.y - i as f32;
                if y_val >= 0.0 && y_val < area.height as f32 {
                    let y = y_val as u16;
                    let char_idx = (self.random_offset(x, i)) % drop.chars.len();
                    let c = drop.chars[char_idx];

                    let style = if i == 0 {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if i < 3 {
                        Style::default().fg(Color::Green)
                    } else {
                        let color_val = 22 + (i as u8).min(5); // Darker greens
                        Style::default()
                            .fg(Color::Indexed(color_val))
                            .add_modifier(Modifier::DIM)
                    };

                    if x < area.width {
                        let cell = f.buffer_mut().get_mut(x, y);
                        {
                            cell.set_char(c).set_style(style);
                        }
                    }
                }
            }
        }
    }

    fn random_offset(&self, x: u16, i: usize) -> usize {
        // Deterministic-ish flicker based on x and i
        x as usize + i
    }
}

impl App {
    fn centered_rect(&self, percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }

    #[allow(dead_code)]
    fn render_status_bar(&self, f: &mut Frame, area: Rect) {
        let status_text = format!(
            " Uptime: {}s | Active: {} | Total: {} | Session Cost: ${}",
            self.state.server_uptime,
            self.state.active_connections,
            self.state.total_requests,
            self.state.session_cost
        );

        let p = Paragraph::new(status_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" System Status "),
            )
            .style(Style::default().fg(Color::Green));

        f.render_widget(p, area);
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    #[test]
    fn test_graph_state_creation() {
        let graph_state = GraphState::new();
        assert_eq!(graph_state.window_size_secs, 300);
        assert!(graph_state.models.is_empty());
        assert!(graph_state.model_colors.is_empty());
    }

    #[test]
    fn test_record_request_completion() {
        let mut graph_state = GraphState::new();

        // Record a request
        graph_state.record_request_completion("gpt-4", 100, LatencyMs(2000), CostUsd(0.01));

        assert_eq!(graph_state.models.len(), 1);
        assert!(graph_state.model_colors.contains_key("gpt-4"));

        let model_data = &graph_state.models["gpt-4"];
        assert_eq!(model_data.current_total_tokens, 100);
        assert_eq!(model_data.current_total_cost, 0.01);

        // Check that we have at least one bucket with data
        assert!(!model_data.buckets.is_empty());
        let bucket = match model_data.buckets.back() {
            Some(b) => b,
            None => return,
        };
        assert!(bucket.tps_completion > 0.0); // Should have some TPS
        assert_eq!(bucket.tokens_completion_delta, 100);
        assert_eq!(bucket.cost_delta, 0.01);
    }

    #[test]
    fn test_tps_calculation() {
        let mut graph_state = GraphState::new();

        // Test TPS calculation: 100 tokens in 2 seconds = 50 TPS
        graph_state.record_request_completion("gpt-4", 100, LatencyMs(2000), CostUsd(0.01));

        let model_data = &graph_state.models["gpt-4"];
        let bucket = match model_data.buckets.back() {
            Some(b) => b,
            None => return,
        };
        assert!((bucket.tps_completion - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_multiple_models() {
        let mut graph_state = GraphState::new();

        // Record requests for different models
        graph_state.record_request_completion("gpt-4", 100, LatencyMs(2000), CostUsd(0.01));
        graph_state.record_request_completion("claude-3", 150, LatencyMs(3000), CostUsd(0.015));

        assert_eq!(graph_state.models.len(), 2);
        assert!(graph_state.models.contains_key("gpt-4"));
        assert!(graph_state.models.contains_key("claude-3"));

        // Check totals
        assert_eq!(graph_state.get_total_tokens(), 250);
        assert!((graph_state.get_total_cost() - 0.025).abs() < 0.001);
    }

    #[test]
    fn test_color_assignment() {
        let mut graph_state = GraphState::new();

        // First model should get first color
        graph_state.record_request_completion("model1", 100, LatencyMs(2000), CostUsd(0.01));
        let color1 = graph_state.get_model_color("model1");

        // Second model should get second color
        graph_state.record_request_completion("model2", 100, LatencyMs(2000), CostUsd(0.01));
        let color2 = graph_state.get_model_color("model2");

        assert_ne!(color1, color2);

        // Same model should always get same color
        let color1_again = graph_state.get_model_color("model1");
        assert_eq!(color1, color1_again);
    }

    #[test]
    fn test_advance_time() {
        let mut graph_state = GraphState::new();

        // Add initial data
        graph_state.record_request_completion("gpt-4", 100, LatencyMs(2000), CostUsd(0.01));
        let initial_bucket_count = graph_state.models["gpt-4"].buckets.len();

        // Advance time by 1 second
        std::thread::sleep(std::time::Duration::from_millis(1100));
        graph_state.advance_time();

        // Should have added a new bucket
        let new_bucket_count = graph_state.models["gpt-4"].buckets.len();
        assert_eq!(new_bucket_count, initial_bucket_count + 1);
    }

    #[test]
    fn test_window_size_limit() {
        let mut graph_state = GraphState::new();
        graph_state.window_size_secs = 3; // Small window for testing

        // Add more buckets than window size
        for i in 0..5 {
            graph_state.record_request_completion(&format!("model{}", i % 2), 100, LatencyMs(2000), CostUsd(0.01));
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }

        // Should not exceed window size
        for model_data in graph_state.models.values() {
            assert!(model_data.buckets.len() <= 3);
        }
    }
}
