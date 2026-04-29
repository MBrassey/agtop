// Ratatui TUI. One-file app — keeping it close so layout and event loop are
// easy to follow.
//
// Layout (resizes gracefully):
//   ┌ header bar (1 row) ───────────────────────────────────────────────────────┐
//   ├ left column (~58%)             ┬ right column (~42%) ─────────────────────┤
//   │ Agents (project-grouped)        │ CPU% chart                               │
//   │                                 ├ MEM(MB) chart ───────────────────────────┤
//   │                                 ├ Active vs Busy line ─────────────────────┤
//   ├ Projects | Activity ────────────┴ Claude sessions ───────────────────────┤
//   └ help footer (1 row) ──────────────────────────────────────────────────────┘
//
// We use Chart for axis-labeled history, Sparkline for a tiny per-project mood
// strip, BarChart for the per-agent CPU bar, and Table for the agents view.

use crate::cli::Args;
use crate::collector::Collector;
use crate::format::{bytes, dur, pct, project_basename, shorten, si, sparkline};
use crate::pricing::format_cost;
use crate::model::{ActivityKind, Agent, Snapshot, Status};
use crate::theme;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Row, Sparkline, Table, Wrap,
    },
    Frame, Terminal,
};
use std::io::{self, stdout};
use std::time::{Duration, Instant};

#[derive(Copy, Clone, PartialEq, Eq)]
enum Sort { Smart, Cpu, Mem, Uptime, Tokens, Agent }

impl Sort {
    fn cycle(self) -> Self {
        match self {
            Sort::Smart  => Sort::Cpu,
            Sort::Cpu    => Sort::Mem,
            Sort::Mem    => Sort::Tokens,
            Sort::Tokens => Sort::Uptime,
            Sort::Uptime => Sort::Agent,
            Sort::Agent  => Sort::Smart,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Smart  => "smart",
            Sort::Cpu    => "cpu",
            Sort::Mem    => "mem",
            Sort::Tokens => "tokens",
            Sort::Uptime => "uptime",
            Sort::Agent  => "agent",
        }
    }
}

struct App {
    collector: Collector,
    snap: Snapshot,
    last_tick: Instant,
    interval: Duration,
    paused: bool,
    grouped: bool,
    sort: Sort,
    filter: String,
    typing_filter: bool,
    show_help: bool,
    show_detail: bool,
    selected_pid: Option<u32>,
    visible_pid_order: Vec<u32>,
    /// `(row_y, pid)` of every clickable agent row in the agents panel,
    /// captured during the previous draw so mouse clicks can be hit-tested
    /// without rerendering.
    clickable_rows: Vec<(u16, u32)>,
    quit: bool,
}

pub fn run(collector: Collector, args: Args) -> Result<()> {
    // Install a panic hook that restores terminal state before unwinding so
    // a crash in draw / event handling doesn't leave the user in raw mode
    // on the alternate screen.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
        prev(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let interval = Duration::from_millis((args.interval.max(0.1) * 1000.0) as u64);
    let initial_sort = match args.sort.as_str() {
        "cpu" => Sort::Cpu,
        "mem" => Sort::Mem,
        "tokens" => Sort::Tokens,
        "uptime" => Sort::Uptime,
        "agent" => Sort::Agent,
        _ => Sort::Smart,
    };

    let mut app = App {
        collector,
        snap: Snapshot::default(),
        last_tick: Instant::now() - interval,
        interval,
        paused: false,
        grouped: true,
        sort: initial_sort,
        filter: args.filter.unwrap_or_default(),
        typing_filter: false,
        show_help: false,
        show_detail: false,
        selected_pid: None,
        visible_pid_order: Vec::new(),
        clickable_rows: Vec::new(),
        quit: false,
    };

    let res = main_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res
}

fn main_loop<B: ratatui::backend::Backend + io::Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    while !app.quit {
        // Refresh snapshot if interval elapsed.
        if !app.paused && app.last_tick.elapsed() >= app.interval {
            app.snap = app.collector.snapshot();
            app.last_tick = Instant::now();
        }

        terminal.draw(|f| draw(f, app))?;

        // Poll for input with a short timeout so we don't burn CPU.
        let timeout = Duration::from_millis(100);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => handle_key(app, key),
                Event::Mouse(m) => handle_mouse(app, m),
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Cap filter length — defuses a 1MB-paste DoS that'd run case-insensitive
    // contains() against every agent every tick.
    const FILTER_MAX: usize = 256;
    // Filter prompt is modal — accept input keys, escape closes it.
    if app.typing_filter {
        match key.code {
            KeyCode::Esc => {
                app.typing_filter = false;
                app.filter.clear();
            }
            KeyCode::Enter => app.typing_filter = false,
            KeyCode::Backspace => { app.filter.pop(); }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.filter.clear(),
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Strip trailing whitespace then trailing word.
                while matches!(app.filter.chars().last(), Some(c) if c.is_whitespace()) { app.filter.pop(); }
                while matches!(app.filter.chars().last(), Some(c) if !c.is_whitespace()) { app.filter.pop(); }
            }
            KeyCode::Char(c) if !c.is_control() && app.filter.len() < FILTER_MAX => {
                app.filter.push(c);
            }
            _ => {}
        }
        return;
    }

    // Popup-aware gating: when a popup is open, only the dismiss / toggle
    // keys are honoured — j/k/s/g/f/r/p don't fall through.
    if app.show_detail || app.show_help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                app.show_detail = false;
                app.show_help = false;
            }
            KeyCode::Char('?') | KeyCode::Char('h') => app.show_help = !app.show_help,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('?') | KeyCode::Char('h') => app.show_help = true,
        KeyCode::Enter => app.show_detail = true,
        KeyCode::Char('p') | KeyCode::Char(' ') => app.paused = !app.paused,
        KeyCode::Char('r') => {
            app.snap = app.collector.snapshot();
            app.last_tick = Instant::now();
        }
        KeyCode::Char('s') => app.sort = app.sort.cycle(),
        KeyCode::Char('g') => app.grouped = !app.grouped,
        KeyCode::Char('/') | KeyCode::Char('f') => {
            app.typing_filter = true;
            app.filter.clear();
        }
        // Esc with no popup open: clear filter as a quick "reset".
        KeyCode::Esc => app.filter.clear(),
        KeyCode::Down | KeyCode::Char('j') => move_sel(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_sel(app, -1),
        KeyCode::PageDown => move_sel(app, 10),
        KeyCode::PageUp => move_sel(app, -10),
        KeyCode::Home => move_sel(app, i32::MIN / 2),
        KeyCode::End => move_sel(app, i32::MAX / 2),
        _ => {}
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        // Click an agent row in the agents panel: select it.  Double-click
        // (handled here as click-on-selected) opens the detail popup.
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((_, pid)) = app.clickable_rows.iter().find(|(y, _)| *y == m.row) {
                if app.selected_pid == Some(*pid) {
                    app.show_detail = true;
                } else {
                    app.selected_pid = Some(*pid);
                }
            }
        }
        // Wheel scrolls the selection.
        MouseEventKind::ScrollUp   => move_sel(app, -3),
        MouseEventKind::ScrollDown => move_sel(app,  3),
        _ => {}
    }
}

fn move_sel(app: &mut App, delta: i32) {
    if app.visible_pid_order.is_empty() {
        return;
    }
    let cur_idx = app.selected_pid
        .and_then(|p| app.visible_pid_order.iter().position(|x| *x == p))
        .unwrap_or(0) as i32;
    let n = app.visible_pid_order.len() as i32;
    let next = (cur_idx + delta).max(0).min(n - 1);
    app.selected_pid = Some(app.visible_pid_order[next as usize]);
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Tiny-terminal guard: below the layout's minimum we can't render the
    // panel stack without truncation. Show a single instructional line
    // instead of a broken half-rendered TUI.
    if area.height < 16 || area.width < 60 {
        let p = Paragraph::new(format!(
            "  agtop needs at least 60×16 (have {}×{}).\n  Resize the terminal or use `agtop --once`.",
            area.width, area.height
        )).style(Style::default().fg(theme::FG_DIM));
        f.render_widget(p, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),    // header
            Constraint::Min(0),       // body
            Constraint::Length(1),    // footer
        ])
        .split(area);

    draw_header(f, chunks[0], &app.snap, app);

    // Body: left | right; left has agents on top + projects/activity on bottom.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(chunks[1]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(10)])
        .split(body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10),    // CPU panel: sparkline + per-agent bars
            Constraint::Length(10),    // Memory by agent + system gauge
            Constraint::Length(8),     // Tokens panel: rate sparkline + per-agent
            Constraint::Length(8),     // Status distribution bars
            Constraint::Min(6),        // Claude sessions panel
        ])
        .split(body[1]);

    draw_agents(f, left[0], app);
    draw_left_bottom(f, left[1], &app.snap);
    draw_cpu_panel(f, right[0], &app.snap);
    draw_memory_panel(f, right[1], &app.snap);
    draw_tokens_panel(f, right[2], &app.snap, app.interval);
    draw_status_distribution(f, right[3], &app.snap);
    draw_sessions(f, right[4], &app.snap);

    draw_footer(f, chunks[2], app);

    if app.show_help {
        draw_help(f, area);
    } else if app.show_detail {
        draw_detail(f, area, app);
    } else if app.typing_filter {
        draw_filter_input(f, area, &app.filter);
    }
}

fn draw_detail(f: &mut Frame, area: Rect, app: &App) {
    let agent = app.snap.agents.iter().find(|a| Some(a.pid) == app.selected_pid);
    let Some(a) = agent else { return; };

    let w = 100.min(area.width.saturating_sub(4));
    let h = 32.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let r = Rect { x, y, width: w, height: h };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(format!(" {} {} ", a.status.glyph(), a.status.label()),
                theme::status_style(a.status)),
            Span::styled(format!("{} ", a.label),
                Style::default().fg(theme::agent_color(&a.label)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("pid {} ", a.pid), Style::default().fg(theme::FG_DIM)),
            Span::styled(format!("· {} ", a.project),
                Style::default().fg(theme::BORDER).add_modifier(Modifier::BOLD)),
        ]));
    let inner = block.inner(r);
    f.render_widget(ratatui::widgets::Clear, r);
    f.render_widget(block, r);

    let dim   = |s: String| Span::styled(s, Style::default().fg(theme::FG_DIM));
    let lab   = |s: &str| Span::styled(format!("{:<10}", s), Style::default().fg(theme::FG_DIM));
    let val   = |s: String, c: ratatui::style::Color| Span::styled(s, Style::default().fg(c).add_modifier(Modifier::BOLD));

    let cpu_spark = sparkline(&a.cpu_history, 100.0, 24);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![lab("model"),
        a.model.as_deref().map(|m| val(m.into(), theme::C_CHART_TOK)).unwrap_or_else(|| dim("(unknown)".into()))]));
    lines.push(Line::from(vec![lab("cpu"),
        val(pct(a.cpu), theme::cpu_color(a.cpu)), Span::raw(" "),
        Span::styled(cpu_spark, Style::default().fg(theme::cpu_color(a.cpu)))]));
    lines.push(Line::from(vec![lab("memory"),
        val(bytes(a.rss), theme::C_CHART_MEM), dim(format!(" rss · {} vsize", bytes(a.vsize)))]));
    lines.push(Line::from(vec![lab("uptime"),  val(dur(a.uptime_sec), theme::FG)]));
    lines.push(Line::from(vec![lab("threads"), val(a.threads.to_string(), theme::FG),
        dim(format!("  state {}  ppid {}", a.state, a.ppid))]));
    lines.push(Line::from(vec![lab("tokens"),
        val(si(a.tokens_total), theme::C_CHART_TOK),
        dim(format!(" ({} in / {} out)", si(a.tokens_input), si(a.tokens_output)))]));
    if a.cost_usd > 0.0 {
        lines.push(Line::from(vec![lab("cost"), val(format_cost(a.cost_usd), theme::C_WAIT)]));
    }
    if a.subagents > 0 {
        lines.push(Line::from(vec![lab("subagents"),
            val(format!("{} in flight", a.subagents), theme::C_SPAWN)]));
        for s in a.in_flight_subagents.iter().take(8) {
            lines.push(Line::from(vec![
                Span::raw("           "),
                Span::styled("· ", Style::default().fg(theme::C_SPAWN)),
                Span::styled(shorten(s, (w as usize).saturating_sub(14)),
                    Style::default().fg(theme::FG)),
            ]));
        }
    }
    // tools-in-flight (Bash/Edit/Read/etc.) — separate from Task subagents
    // so users distinguish "spawned a subagent" from "running a tool".
    let tools_in_flight = a.subagents == 0 && a.session_id.is_some()
        && matches!(a.status, crate::model::Status::Busy);
    if tools_in_flight {
        lines.push(Line::from(vec![lab("tools"),
            val("running".to_string(), theme::C_SPAWN)]));
    }
    if let Some(sid) = &a.session_id {
        lines.push(Line::from(vec![lab("session"), dim(sid.clone())]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![lab("exe"),  Span::styled(shorten(&a.exe, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::FG))]));
    lines.push(Line::from(vec![lab("cwd"),  Span::styled(shorten(&a.cwd, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::FG))]));
    lines.push(Line::from(vec![lab("cmd"),  Span::styled(shorten(&a.cmdline, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::FG_DIM))]));
    if let Some(tool) = &a.current_tool {
        lines.push(Line::from(vec![lab("tool"),
            val(tool.clone(), theme::C_SPAWN)]));
    }
    if let Some(task) = &a.current_task {
        lines.push(Line::from(vec![lab("task"),
            Span::styled(shorten(task, (w as usize).saturating_sub(12)),
                Style::default().fg(theme::FG))]));
    }
    if !a.writing_files.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(lab("writing")));
        for f in a.writing_files.iter().take(4) {
            lines.push(Line::from(vec![Span::raw("    "),
                Span::styled(shorten(f, (w as usize).saturating_sub(6)),
                    Style::default().fg(theme::FG_DIM))]));
        }
    }
    // Live preview: a fenced box at the bottom showing the last few
    // events from the session transcript.  Color-coded by glyph: › prose,
    // → tool call, ← tool result.
    if !a.recent_activity.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  ─ Live preview ".to_string() + &"─".repeat((w as usize).saturating_sub(20)),
            Style::default().fg(theme::BORDER_DIM))));
        let cap = ((h as usize).saturating_sub(lines.len() + 3)).min(8);
        for ev in a.recent_activity.iter().rev().take(cap).rev() {
            let glyph_col = if ev.starts_with("› ")      { theme::FG }
                            else if ev.starts_with("→ ") { theme::C_SPAWN }
                            else if ev.starts_with("← ") { theme::C_ACTIVE }
                            else                          { theme::FG_DIM };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(shorten(ev, (w as usize).saturating_sub(4)),
                    Style::default().fg(glyph_col)),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("  Esc / Enter to close", Style::default().fg(theme::FG_DIM))));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_header(f: &mut Frame, area: Rect, snap: &Snapshot, app: &App) {
    let a = &snap.aggregates;
    let mem_used = snap.mem_total.saturating_sub(snap.mem_available);

    let mut spans: Vec<Span> = vec![
        Span::styled(" agtop ",
            Style::default().fg(theme::BORDER).add_modifier(Modifier::BOLD)),
        Span::styled(format!("v{}  ", env!("CARGO_PKG_VERSION")), Style::default().fg(theme::FG_DIM)),
    ];
    let mut chip = |label: &str, value: String, color: ratatui::style::Color| {
        spans.push(Span::styled(format!(" {} ", value),
                                Style::default().fg(color).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!("{} ", label), Style::default().fg(theme::FG_DIM)));
    };
    // Order calibrated by user-priority on first glance:
    // busy first (most actionable), then cost / tokens (financial
    // context), then load / capacity, then breakdown counts.
    chip("busy",      a.busy.to_string(),      theme::C_BUSY);
    if a.cost_usd > 0.0 {
        chip("cost",   format_cost(a.cost_usd), theme::C_WAIT);
    }
    if a.tokens_total > 0 {
        chip("tokens", si(a.tokens_total),     theme::C_CHART_TOK);
    }
    chip("cpu",       pct(a.cpu),              theme::C_CHART_CPU);
    chip("mem",       format!("{}/{}", bytes(mem_used), bytes(snap.mem_total)),
                                               theme::C_CHART_MEM);
    chip("active",    a.active.to_string(),    theme::C_ACTIVE);
    chip("waiting",   a.waiting.to_string(),   theme::C_WAIT);
    chip("done",      a.completed.to_string(), theme::C_DONE);
    chip("subagents", a.subagents.to_string(), theme::C_SPAWN);
    chip("projects",  a.project_count.to_string(), theme::FG);
    spans.push(Span::styled(format!(" sort:{}  group:{}  ", app.sort.label(), if app.grouped {"on"} else {"off"}),
                            Style::default().fg(theme::FG_DIM)));
    if !app.filter.is_empty() {
        spans.push(Span::styled(format!("filter:{}  ", app.filter),
                                Style::default().fg(theme::C_WAIT)));
    }
    if app.paused {
        spans.push(Span::styled(" PAUSED ",
                                Style::default().bg(theme::C_WAIT).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER));
    let p = Paragraph::new(Line::from(spans)).block(block).wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn filter_match(a: &Agent, f: &str) -> bool {
    if f.is_empty() { return true; }
    let f = f.to_lowercase();
    a.label.to_lowercase().contains(&f)
        || a.cmdline.to_lowercase().contains(&f)
        || a.cwd.to_lowercase().contains(&f)
        || a.project.to_lowercase().contains(&f)
        || a.pid.to_string() == f
}

fn draw_agents(f: &mut Frame, area: Rect, app: &mut App) {
    let snap = &app.snap;
    let mut agents: Vec<&Agent> = snap.agents.iter().filter(|a| filter_match(a, &app.filter)).collect();
    match app.sort {
        Sort::Smart => {} // already sorted by collector
        Sort::Cpu     => agents.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)),
        Sort::Mem     => agents.sort_by(|a, b| b.rss.cmp(&a.rss)),
        Sort::Tokens  => agents.sort_by(|a, b| b.tokens_total.cmp(&a.tokens_total)),
        Sort::Uptime  => agents.sort_by(|a, b| b.uptime_sec.cmp(&a.uptime_sec)),
        Sort::Agent   => agents.sort_by(|a, b| a.label.cmp(&b.label)),
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Agents ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)))
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut pid_order: Vec<u32> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut agent_row_indices: Vec<usize> = Vec::new();
    let mut group_index: usize = 0;

    if app.grouped {
        // Group agents by project, preserving collector ordering.
        let mut by_proj: Vec<(String, Vec<&Agent>)> = Vec::new();
        for a in agents.iter() {
            if let Some((_, list)) = by_proj.iter_mut().find(|(p, _)| p == &a.project) {
                list.push(*a);
            } else {
                by_proj.push((a.project.clone(), vec![*a]));
            }
        }
        // Order projects by best status -> name.
        by_proj.sort_by(|(p1, l1), (p2, l2)| {
            let r1 = l1.first().map(|a| a.status.rank()).unwrap_or(99);
            let r2 = l2.first().map(|a| a.status.rank()).unwrap_or(99);
            r1.cmp(&r2).then(p1.cmp(p2))
        });

        for (proj, list) in by_proj {
            let total_cpu: f64 = list.iter().map(|a| a.cpu).sum();
            let total_mem: u64 = list.iter().map(|a| a.rss).sum();
            let total_sub: u32 = list.iter().map(|a| a.subagents).sum();
            let total_tok: u64 = list.iter().map(|a| a.tokens_total).sum();
            let mut header_spans: Vec<Span> = Vec::new();
            header_spans.push(Span::styled("◆ ", Style::default().fg(theme::BORDER)));
            header_spans.push(Span::styled(proj.clone(),
                Style::default().fg(theme::BORDER).add_modifier(Modifier::BOLD)));
            header_spans.push(Span::styled(
                format!("  {} agent{} · {} cpu · {} mem",
                    list.len(),
                    if list.len() == 1 {""} else {"s"},
                    pct(total_cpu),
                    bytes(total_mem)),
                Style::default().fg(theme::FG_DIM)));
            if total_sub > 0 {
                header_spans.push(Span::styled(format!("  +{}", total_sub),
                    Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)));
                header_spans.push(Span::styled(" sub", Style::default().fg(theme::FG_DIM)));
            }
            if total_tok > 0 {
                header_spans.push(Span::styled(format!("  {}", si(total_tok)),
                    Style::default().fg(theme::C_CHART_TOK).add_modifier(Modifier::BOLD)));
                header_spans.push(Span::styled(" tok", Style::default().fg(theme::FG_DIM)));
            }
            let header_line = Line::from(header_spans);
            // Project header inherits the group's tint so the cluster
            // visually belongs together.
            let header_row = Row::new(vec![header_line]).height(1)
                .style(Style::default().bg(theme::group_tint(group_index)));
            rows.push(header_row);

            // Two subtle alternating tints — same project gets one tint,
            // the next group gets the other.  Subtle enough that the
            // panel still matches the rest of the app's background;
            // bright enough to mark group boundaries.
            let tint = Some(theme::group_tint(group_index));
            group_index += 1;
            for a in list {
                pid_order.push(a.pid);
                agent_row_indices.push(rows.len());
                rows.push(agent_row(a, app.selected_pid == Some(a.pid), tint));
            }
        }
    } else {
        // Ungrouped: alternate by project run-length so consecutive rows
        // for the same project still cluster visually.
        let mut last_proj: Option<&str> = None;
        for a in agents.iter() {
            if last_proj != Some(a.project.as_str()) {
                group_index += 1;
                last_proj = Some(a.project.as_str());
            }
            let tint = Some(theme::group_tint(group_index));
            pid_order.push(a.pid);
            agent_row_indices.push(rows.len());
            rows.push(agent_row(a, app.selected_pid == Some(a.pid), tint));
        }
    }

    app.visible_pid_order = pid_order.clone();
    if app.selected_pid.is_none() {
        app.selected_pid = pid_order.first().copied();
    } else if let Some(p) = app.selected_pid {
        if !pid_order.contains(&p) {
            // Selected agent disappeared — drop the popup if it was open
            // against the dead pid, snap selection to the first row.
            if app.show_detail { app.show_detail = false; }
            app.selected_pid = pid_order.first().copied();
        }
    }
    // Empty-state hint when filter matches nothing.
    if rows.is_empty() {
        let msg = if snap.agents.is_empty() {
            "  (no agents detected — try `agtop --list-builtins` or set $AGTOP_MATCH)".to_string()
        } else if !app.filter.is_empty() {
            format!("  (no agents match \"{}\" — Esc to clear)", app.filter)
        } else {
            "  (no rows)".into()
        };
        rows.push(Row::new(vec![Line::from(Span::styled(msg,
            Style::default().fg(theme::FG_DIM)))]).height(1));
    }

    // Capture screen-row → pid map for mouse hit-testing in the next event
    // poll.  inner.y is the y of the first visible row; each Row has height(1).
    app.clickable_rows.clear();
    for (i, ri) in agent_row_indices.iter().enumerate() {
        if let Some(pid) = pid_order.get(i) {
            let y = inner.y + (*ri as u16);
            if y < inner.y + inner.height {
                app.clickable_rows.push((y, *pid));
            }
        }
    }

    // Single-cell rows; we render as one wide column to keep the styled spans intact.
    let table = Table::new(rows, [Constraint::Percentage(100)])
        .column_spacing(0);
    f.render_widget(table, inner);
}

fn agent_row<'a>(a: &'a Agent, selected: bool, group_bg: Option<ratatui::style::Color>) -> Row<'a> {
    // Tight layout: every fixed-width column packs against the next so the
    // DOING field at the end gets the lion's share of the row.  Column
    // widths chosen to hold realistic values without overflow:
    //   2  leading indent
    //   6  status glyph + 4-char label  (e.g. "● BUSY")
    //   1  space
    //  10  agent label
    //   1  space
    //   7  pid (right-aligned)
    //   1  space
    //   5  cpu%  (e.g. "31.4%")
    //   1  space
    //   4  cpu mini-bar
    //   1  space
    //   5  mem  (e.g. "642M")
    //   1  space
    //   6  uptime (e.g. "3h17m")
    //   3  +N subagent chip (only if non-zero, otherwise spaces)
    //   5  tokens chip (only if non-zero, otherwise spaces)
    //   2  separator before DOING
    //   ∞  doing
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw("  "));

    // Status badge — compact "● BUSY" form (6 visible chars).
    spans.push(Span::styled(
        format!("{} {:<4}", a.status.glyph(), a.status.label()),
        theme::status_style(a.status),
    ));
    spans.push(Span::raw(" "));

    // Dangerous-mode marker: a single ▍ left-edge bar in amber, sized one
    // visible char.  The label itself stays in its normal accent so the
    // glyph alone carries the "this row is flagged" semantic.
    if a.dangerous {
        spans.push(Span::styled("▍",
            Style::default().fg(ratatui::style::Color::Rgb(240, 175, 95))
                .add_modifier(Modifier::BOLD)));
    } else {
        spans.push(Span::raw(" "));
    }
    // Agent label — always its normal accent color + bold.
    spans.push(Span::styled(format!("{:<9}", shorten(&a.label, 9)),
        Style::default().fg(theme::agent_color(&a.label)).add_modifier(Modifier::BOLD)));
    spans.push(Span::raw(" "));

    // PID
    spans.push(Span::styled(format!("{:>7}", a.pid),
                            Style::default().fg(theme::FG_DIM)));
    spans.push(Span::raw(" "));

    // CPU% + 4-cell mini bar.
    let cpu_col = theme::cpu_color(a.cpu);
    spans.push(Span::styled(format!("{:>5}", pct(a.cpu)),
                            Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
    spans.push(Span::raw(" "));
    let bar_cells = (a.cpu / 100.0 * 4.0).round().clamp(0.0, 4.0) as usize;
    spans.push(Span::styled("█".repeat(bar_cells),
                            Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
    spans.push(Span::styled("·".repeat(4 - bar_cells),
                            Style::default().fg(theme::BORDER_DIM)));
    spans.push(Span::raw(" "));

    // MEM
    spans.push(Span::styled(format!("{:>5}", bytes(a.rss)),
                            Style::default().fg(theme::C_CHART_MEM)));
    spans.push(Span::raw(" "));

    // Uptime
    spans.push(Span::styled(format!("{:>6}", dur(a.uptime_sec)),
                            Style::default().fg(theme::FG_DIM)));

    // Subagent chip — fixed 4-char slot, capped at 99 so two-digit counts
    // (which would otherwise widen the column and shift DOING) fit.
    if a.subagents > 0 {
        spans.push(Span::styled(format!(" +{:<2}", a.subagents.min(99)),
                                Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)));
    } else {
        spans.push(Span::raw("    "));
    }

    // Token chip — fixed 6-char slot, value clipped to 5 visible chars so
    // even "100M+" abbreviations don't push downstream columns.
    if a.tokens_total > 0 {
        let s = si(a.tokens_total);
        let clipped: String = s.chars().take(5).collect();
        spans.push(Span::styled(format!(" {:>5}", clipped),
                                Style::default().fg(theme::C_CHART_TOK)));
    } else {
        spans.push(Span::raw("      "));
    }

    // DOING — gets all remaining width.
    spans.push(Span::raw("  "));
    spans.push(describe_doing_span(a));

    let line = Line::from(spans);
    let mut row = Row::new(vec![line]).height(1);
    if selected {
        row = row.style(Style::default().bg(theme::HL_BG).add_modifier(Modifier::BOLD));
    } else if let Some(bg) = group_bg {
        row = row.style(Style::default().bg(bg));
    }
    row
}

fn describe_doing_span(a: &Agent) -> Span<'static> {
    if let Some(tool) = &a.current_tool {
        let suffix = a.current_task.as_deref().map(|t|
            format!(": {}", shorten(t, 60))
        ).unwrap_or_default();
        return Span::styled(format!("{}{}", tool, suffix),
            Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD));
    }
    if let Some(t) = &a.current_task {
        return Span::styled(shorten(t, 70).to_string(),
            Style::default().fg(theme::FG));
    }
    if a.status == Status::Idle {
        if let Some(age) = a.session_age_ms {
            return Span::styled(format!("(idle {})", dur(age / 1000)),
                Style::default().fg(theme::FG_DIM));
        }
    }
    if a.status == Status::Waiting {
        return Span::styled("(awaiting input)".to_string(), Style::default().fg(theme::C_WAIT));
    }
    if a.status == Status::Completed {
        return Span::styled("(session ended)".to_string(), Style::default().fg(theme::C_DONE));
    }
    Span::styled(shorten(&a.cmdline, 80).to_string(),
                 Style::default().fg(theme::FG_DIM))
}

fn draw_left_bottom(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    draw_projects(f, inner[0], snap);
    draw_activity(f, inner[1], snap);
}

fn draw_projects(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Projects ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Bar chart of per-project CPU%, with a colored badge of the dominant
    // status — the most informative single panel about "where is work happening".
    let max_lines = inner.height as usize;
    let mut items: Vec<ListItem> = Vec::new();
    for (i, p) in snap.projects.iter().take(max_lines).enumerate() {
        let row_bg = theme::group_tint(i);
        let dominant = if *p.statuses.get("busy").unwrap_or(&0) > 0 { Status::Busy }
                       else if *p.statuses.get("spawning").unwrap_or(&0) > 0 { Status::Spawning }
                       else if *p.statuses.get("active").unwrap_or(&0) > 0 { Status::Active }
                       else if *p.statuses.get("idle").unwrap_or(&0) > 0 { Status::Idle }
                       else { Status::Stale };
        let bar_w: usize = ((p.cpu / 100.0) * 12.0).round().max(0.0) as usize;
        let bar_w = bar_w.min(12);
        let bar = "█".repeat(bar_w);
        let bar_pad = " ".repeat(12usize.saturating_sub(bar_w));
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(format!("{} ", dominant.glyph()), theme::status_style(dominant)));
        spans.push(Span::styled(format!("{:<14}", shorten(&p.project, 14)),
                                Style::default().fg(theme::FG)));
        spans.push(Span::styled(format!("{:>2}", p.agents), Style::default().fg(theme::FG_DIM)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", pct(p.cpu)),
                                Style::default().fg(theme::cpu_color(p.cpu))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar, Style::default().fg(theme::cpu_color(p.cpu))));
        spans.push(Span::styled(bar_pad, Style::default().fg(theme::BORDER_DIM)));
        if p.subagents > 0 {
            spans.push(Span::styled(format!(" +{}", p.subagents),
                                    Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)));
        }
        items.push(ListItem::new(Line::from(spans)).style(Style::default().bg(row_bg)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled("  (no projects)", Style::default().fg(theme::FG_DIM)))));
    }
    let list = List::new(items);
    f.render_widget(list, inner);
}

fn draw_activity(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Activity ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut items: Vec<ListItem> = Vec::new();
    for (i, e) in snap.activity.iter().take(inner.height as usize).enumerate() {
        let row_bg = theme::group_tint(i);
        let secs = e.t / 1000;
        let nd = chrono::DateTime::<chrono::Local>::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        let t = nd.format("%H:%M:%S").to_string();
        let (glyph, glyph_style) = match e.kind {
            ActivityKind::Spawn => ("●", Style::default().fg(theme::C_BUSY).add_modifier(Modifier::BOLD)),
            ActivityKind::Exit  => ("◌", Style::default().fg(theme::FG_DIM)),
        };
        let kind = match e.kind { ActivityKind::Spawn => "spawn", ActivityKind::Exit => "exit " };
        let cwd = e.cwd.as_deref().map(|c| project_basename(c)).unwrap_or_default();
        let mut spans: Vec<Span> = vec![
            Span::styled(t, Style::default().fg(theme::FG_DIM)),
            Span::raw("  "),
            Span::styled(glyph.to_string(), glyph_style),
            Span::raw(" "),
            Span::styled(kind.to_string(), Style::default().fg(theme::FG_DIM)),
            Span::raw("  "),
            Span::styled(format!("{:<12}", shorten(&e.label, 12)),
                Style::default().fg(theme::agent_color(&e.label))),
            Span::raw(" "),
            Span::styled(format!("pid {:<7}", e.pid),
                Style::default().fg(theme::FG_DIM)),
        ];
        if !cwd.is_empty() {
            spans.push(Span::styled(format!("  {}", cwd),
                Style::default().fg(theme::FG)));
        }
        items.push(ListItem::new(Line::from(spans)).style(Style::default().bg(row_bg)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled("  (no recent events)", Style::default().fg(theme::FG_DIM)))));
    }
    let list = List::new(items);
    f.render_widget(list, inner);
}

// CPU panel: smooth braille Sparkline on top showing system trend, then a
// horizontal-bar list per agent — same visual language as the Memory and
// Status panels so the right column reads as a single coherent piece.
fn draw_cpu_panel(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let peak = snap.history.cpu.iter().copied().fold(0.0_f64, f64::max);
    let avg = if !snap.history.cpu.is_empty() {
        snap.history.cpu.iter().sum::<f64>() / snap.history.cpu.len() as f64
    } else { 0.0 };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" CPU ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} cores · ", snap.sys_cpus),
                Style::default().fg(theme::FG_DIM)),
            Span::styled("now ", Style::default().fg(theme::FG_DIM)),
            Span::styled(pct(snap.aggregates.cpu),
                Style::default().fg(theme::cpu_color(snap.aggregates.cpu)).add_modifier(Modifier::BOLD)),
            Span::styled(" · peak ", Style::default().fg(theme::FG_DIM)),
            Span::styled(pct(peak),
                Style::default().fg(theme::C_CHART_CPU).add_modifier(Modifier::BOLD)),
            Span::styled(" · avg ", Style::default().fg(theme::FG_DIM)),
            Span::styled(pct(avg),
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default().fg(theme::FG_DIM)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve top 2 rows for the system-CPU sparkline trend.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    // Sparkline: convert f64 % values to u64 (×10 to keep one decimal of
    // resolution) so the smooth ▁▂▃▄▅▆▇█ blocks track the real shape.
    let spark_data: Vec<u64> = snap.history.cpu.iter().map(|v| (v * 10.0) as u64).collect();
    let spark_max = ((peak.max(snap.aggregates.cpu).max(10.0) / 10.0).ceil() * 10.0 * 10.0) as u64;
    let sparkline = Sparkline::default()
        .data(&spark_data)
        .max(spark_max)
        .style(Style::default().fg(theme::C_CHART_CPU));
    f.render_widget(sparkline, split[0]);

    // Per-agent CPU bars. Sort by CPU desc (raw collector order is by status,
    // so we re-sort here for clearest "who's burning the most" reading).
    let mut agents: Vec<&Agent> = snap.agents.iter().collect();
    agents.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    // Bar scales to the max observed agent CPU (or system CPU if higher) so
    // even single-digit values produce a visible bar.
    let bar_basis = agents.first().map(|a| a.cpu).unwrap_or(0.0).max(snap.aggregates.cpu).max(1.0);

    let bar_width = (split[1].width as usize).saturating_sub(34).max(8);
    let mut items: Vec<ListItem> = Vec::new();
    let take = (split[1].height as usize).saturating_sub(0);
    for a in agents.iter().take(take) {
        let frac = (a.cpu / bar_basis).max(0.0);
        let filled = (frac * bar_width as f64).round() as usize;
        let filled = filled.min(bar_width);
        let bar_full  = "█".repeat(filled);
        let bar_empty = "·".repeat(bar_width - filled);
        let cpu_col = theme::cpu_color(a.cpu);
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(format!("{} ", a.status.glyph()), theme::status_style(a.status)));
        spans.push(Span::styled(format!("{:<10}", shorten(&a.project, 10)),
            Style::default().fg(theme::FG)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full, Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::BORDER_DIM)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", pct(a.cpu)),
            Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no agents)", Style::default().fg(theme::FG_DIM)))));
    }
    f.render_widget(List::new(items), split[1]);
}

// Memory panel: top-N agents by RSS as horizontal bars, plus a system memory
// gauge showing free / agent-attributable / other-used.
fn draw_memory_panel(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" Memory by agent ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} across {} agent{}",
                bytes(snap.aggregates.mem_bytes),
                snap.aggregates.active,
                if snap.aggregates.active == 1 {""} else {"s"}),
                Style::default().fg(theme::FG_DIM)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Reserve last two rows for the system memory gauge.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(2)])
        .split(inner);

    // Sort agents by RSS desc.
    let mut agents: Vec<&Agent> = snap.agents.iter().collect();
    agents.sort_by(|a, b| b.rss.cmp(&a.rss));

    let max_rss = agents.iter().map(|a| a.rss).max().unwrap_or(1).max(1);
    let bar_width = (split[0].width as usize).saturating_sub(34).max(8);

    let mut items: Vec<ListItem> = Vec::new();
    let take = (split[0].height as usize).saturating_sub(0);
    for a in agents.iter().take(take) {
        let frac = a.rss as f64 / max_rss as f64;
        let filled = (frac * bar_width as f64).round() as usize;
        let filled = filled.min(bar_width);
        let bar_full  = "█".repeat(filled);
        let bar_empty = "·".repeat(bar_width - filled);
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(format!("{} ", a.status.glyph()), theme::status_style(a.status)));
        spans.push(Span::styled(format!("{:<10}", shorten(&a.project, 10)),
            Style::default().fg(theme::FG)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full,  Style::default().fg(theme::C_CHART_MEM).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::BORDER_DIM)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>7}", bytes(a.rss)),
            Style::default().fg(theme::C_CHART_MEM)));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no agents)", Style::default().fg(theme::FG_DIM)))));
    }
    f.render_widget(List::new(items), split[0]);

    // System memory gauge — three-segment horizontal bar:
    //   used-by-agents | other-used | available
    let total = snap.mem_total.max(1);
    let avail = snap.mem_available;
    let used  = total.saturating_sub(avail);
    let agent_mem = snap.aggregates.mem_bytes.min(used);
    let other = used.saturating_sub(agent_mem);
    let w = split[1].width as usize;
    if w > 4 {
        let cells = (w as i64) - 2;
        let agent_cells = ((agent_mem as f64 / total as f64) * cells as f64).round() as i64;
        let other_cells = ((other as f64     / total as f64) * cells as f64).round() as i64;
        let free_cells  = (cells - agent_cells - other_cells).max(0);
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled("█".repeat(agent_cells.max(0) as usize),
                Style::default().fg(theme::C_GAUGE_AGENT)),
            Span::styled("█".repeat(other_cells.max(0) as usize),
                Style::default().fg(theme::C_GAUGE_USED)),
            Span::styled("░".repeat(free_cells.max(0) as usize),
                Style::default().fg(theme::C_GAUGE_FREE)),
        ]);
        let label = Line::from(vec![
            Span::styled(" agents ", Style::default().fg(theme::FG_DIM)),
            Span::styled(bytes(agent_mem),
                Style::default().fg(theme::C_GAUGE_AGENT).add_modifier(Modifier::BOLD)),
            Span::styled("  other ", Style::default().fg(theme::FG_DIM)),
            Span::styled(bytes(other),
                Style::default().fg(theme::C_GAUGE_USED).add_modifier(Modifier::BOLD)),
            Span::styled("  free ", Style::default().fg(theme::FG_DIM)),
            Span::styled(bytes(avail),
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" / {}", bytes(total)),
                Style::default().fg(theme::FG_DIM)),
        ]);
        f.render_widget(Paragraph::new(vec![line, label]), split[1]);
    }
}

// Tokens panel: rate sparkline at top, top-N agents by tokens beneath. Same
// horizontal-bar visual language as Memory panel.
fn draw_tokens_panel(f: &mut Frame, area: Rect, snap: &Snapshot, interval: Duration) {
    // Rate over the last ~30s, in tokens/min.
    let recent: Vec<f64> = snap.history.tokens_rate.iter().rev().take(20).copied().collect();
    let rate_per_tick = if !recent.is_empty() {
        recent.iter().sum::<f64>() / recent.len() as f64
    } else { 0.0 };
    let tick_secs = interval.as_secs_f64().max(0.1);
    let rate_per_min = rate_per_tick * 60.0 / tick_secs;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" Tokens ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled("total ", Style::default().fg(theme::FG_DIM)),
            Span::styled(si(snap.aggregates.tokens_total),
                Style::default().fg(theme::C_CHART_TOK).add_modifier(Modifier::BOLD)),
            Span::styled("  rate ", Style::default().fg(theme::FG_DIM)),
            Span::styled(format!("{}/min", si(rate_per_min as u64)),
                Style::default().fg(theme::C_CHART_TOK).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default().fg(theme::FG_DIM)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    // Sparkline of tokens-per-tick. Auto-scaled — bursts read as spikes.
    let spark_data: Vec<u64> = snap.history.tokens_rate.iter().map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .data(&spark_data)
        .style(Style::default().fg(theme::C_CHART_TOK));
    f.render_widget(sparkline, split[0]);

    // Per-agent tokens bars (only agents with tokens > 0).
    let mut agents: Vec<&Agent> = snap.agents.iter().filter(|a| a.tokens_total > 0).collect();
    agents.sort_by(|a, b| b.tokens_total.cmp(&a.tokens_total));
    let max = agents.first().map(|a| a.tokens_total).unwrap_or(1).max(1);
    let bar_width = (split[1].width as usize).saturating_sub(34).max(8);
    let mut items: Vec<ListItem> = Vec::new();
    let take = (split[1].height as usize).saturating_sub(0);
    for a in agents.iter().take(take) {
        let frac = a.tokens_total as f64 / max as f64;
        let filled = ((frac * bar_width as f64).round() as usize).min(bar_width);
        let bar_full  = "█".repeat(filled);
        let bar_empty = "·".repeat(bar_width - filled);
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(format!("{} ", a.status.glyph()), theme::status_style(a.status)));
        spans.push(Span::styled(format!("{:<10}", shorten(&a.project, 10)),
            Style::default().fg(theme::FG)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full,  Style::default().fg(theme::C_CHART_TOK).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::BORDER_DIM)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", si(a.tokens_total)),
            Style::default().fg(theme::C_CHART_TOK).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no token usage detected — open a Claude/Codex session)",
            Style::default().fg(theme::FG_DIM)))));
    }
    f.render_widget(List::new(items), split[1]);
}

// Status distribution: htop-style horizontal segment bars, one per status.
fn draw_status_distribution(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" Status distribution ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} live agent{} · {} session{}",
                snap.aggregates.active,
                if snap.aggregates.active == 1 {""} else {"s"},
                snap.sessions.waiting + snap.sessions.completed + snap.sessions.active,
                ""),
                Style::default().fg(theme::FG_DIM)),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let live = snap.aggregates.active;
    let waiting = snap.sessions.waiting;
    let done = snap.sessions.completed;
    let total = (live + waiting + done).max(1);

    // Count per process status across live agents.
    let mut counts: std::collections::HashMap<&'static str, u32> = std::collections::HashMap::new();
    for a in &snap.agents {
        *counts.entry(status_key(a.status)).or_insert(0) += 1;
    }
    let row = |name: &str, count: u32, status: Status| {
        let bar_w = (inner.width as usize).saturating_sub(28).max(8);
        let frac = count as f64 / total as f64;
        let filled = (frac * bar_w as f64).round() as usize;
        let filled = filled.min(bar_w);
        let pct_v = frac * 100.0;
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(format!("  {} {:<6} ", status.glyph(), name),
            theme::status_style(status)));
        spans.push(Span::styled(format!("{:>3} ", count),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("█".repeat(filled),
            Style::default().fg(theme::status_color(status)).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("·".repeat(bar_w - filled),
            Style::default().fg(theme::BORDER_DIM)));
        spans.push(Span::styled(format!(" {:>4.1}%", pct_v),
            Style::default().fg(theme::FG_DIM)));
        Line::from(spans)
    };
    let lines = vec![
        row("BUSY",   *counts.get("busy").unwrap_or(&0),     Status::Busy),
        row("SPWN",   *counts.get("spawning").unwrap_or(&0), Status::Spawning),
        row("ACTV",   *counts.get("active").unwrap_or(&0),   Status::Active),
        row("idle",   *counts.get("idle").unwrap_or(&0),     Status::Idle),
        row("WAIT",   waiting,                               Status::Waiting),
        row("DONE",   done,                                  Status::Completed),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_sessions(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Claude sessions — recent tasks ",
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let s = &snap.sessions;
    let a = &snap.aggregates;
    let mut lines: Vec<Line> = Vec::new();
    if a.subagents > 0 {
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", a.subagents),
                Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)),
            Span::styled(format!("Task subagent{} in flight ",
                if a.subagents == 1 {""} else {"s"}),
                Style::default().fg(theme::FG_DIM)),
        ]));
    }
    if s.recent_tasks.is_empty() {
        lines.push(Line::from(Span::styled("  (no tasks in last 24h)",
            Style::default().fg(theme::FG_DIM))));
    }
    let cap = (inner.height as usize).saturating_sub(if a.subagents > 0 { 1 } else { 0 });
    for t in s.recent_tasks.iter().take(cap) {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(t.status.glyph().to_string(), theme::status_style(t.status)),
            Span::raw(" "),
            Span::styled(format!("{:<14}", shorten(&t.project_short, 14)),
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(shorten(&t.task, 80).to_string(),
                Style::default().fg(theme::FG_DIM)),
        ]));
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, inner);
}

fn status_key(s: Status) -> &'static str {
    match s {
        Status::Busy => "busy",
        Status::Spawning => "spawning",
        Status::Active => "active",
        Status::Idle => "idle",
        Status::Waiting => "waiting",
        Status::Completed => "completed",
        Status::Stale => "stale",
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let s = format!(
        "  q quit · ? help · s sort({}) · g group({}) · / filter · p {} · r refresh · ↑↓ select",
        app.sort.label(),
        if app.grouped {"on"} else {"off"},
        if app.paused {"resume"} else {"pause"},
    );
    let p = Paragraph::new(Span::styled(s, Style::default().fg(theme::FG_DIM)));
    f.render_widget(p, area);
}

fn draw_filter_input(f: &mut Frame, area: Rect, filter: &str) {
    let r = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(2),
        width: area.width,
        height: 1,
    };
    let p = Paragraph::new(Line::from(vec![
        Span::styled(" filter: ", Style::default().bg(theme::BORDER).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(filter.to_string(), Style::default().fg(theme::FG)),
        Span::styled("█", Style::default().fg(theme::C_BUSY)),
    ]));
    f.render_widget(p, r);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 70.min(area.width);
    let h = 22.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let r = Rect { x, y, width: w, height: h };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Help ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(ratatui::widgets::Clear, r);
    f.render_widget(block.clone(), r);
    // Wrap so narrow terminals don't clip the keybinding text.
    let _ = block;

    let line = |spans: Vec<Span<'static>>| Line::from(spans);
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::FG_DIM));
    let hdr = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::FG).add_modifier(Modifier::BOLD));
    let key = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD));

    let lines: Vec<Line> = vec![
        line(vec![hdr("agtop"), dim(&format!("  v{}  — agent monitor", env!("CARGO_PKG_VERSION")))]),
        Line::raw(""),
        line(vec![key("  q, Ctrl-C   "), dim("quit")]),
        line(vec![key("  ?, h        "), dim("toggle this help")]),
        line(vec![key("  p           "), dim("pause / resume refresh")]),
        line(vec![key("  r           "), dim("refresh now")]),
        line(vec![key("  s           "), dim("cycle sort (smart / cpu / mem / tokens / uptime / agent)")]),
        line(vec![key("  g           "), dim("toggle project grouping")]),
        line(vec![key("  /, f        "), dim("filter agents by substring")]),
        line(vec![key("  Esc         "), dim("clear filter")]),
        line(vec![key("  j/k, ↓/↑    "), dim("move selection")]),
        Line::raw(""),
        line(vec![hdr("  Status legend:")]),
        line(vec![Span::styled("    ● BUSY ", Style::default().fg(theme::C_BUSY).add_modifier(Modifier::BOLD)),
                  dim("process active and writing in last 5s")]),
        line(vec![Span::styled("    ◆ SPWN ", Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)),
                  dim("Task subagents currently in flight")]),
        line(vec![Span::styled("    ● ACTV ", Style::default().fg(theme::C_ACTIVE)),
                  dim("process running recently")]),
        line(vec![Span::styled("    ○ idle ", Style::default().fg(theme::C_IDLE)),
                  dim("process up but quiet for >60s")]),
        line(vec![Span::styled("    ◌ WAIT ", Style::default().fg(theme::C_WAIT)),
                  dim("no live process, recent session activity")]),
        line(vec![Span::styled("    ✓ DONE ", Style::default().fg(theme::C_DONE)),
                  dim("session ended (stop_reason)")]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
