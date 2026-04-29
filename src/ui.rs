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
use crate::format::{bytes, dur, pct, project_basename, shorten, shorten_left, tildeify};
use crate::model::{ActivityKind, Agent, Snapshot, Status};
use crate::theme;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, BorderType, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph,
        Row, Table, Wrap,
    },
    Frame, Terminal,
};
use std::io::{self, stdout};
use std::time::{Duration, Instant};

#[derive(Copy, Clone, PartialEq, Eq)]
enum Sort { Smart, Cpu, Mem, Uptime, Agent }

impl Sort {
    fn cycle(self) -> Self {
        match self {
            Sort::Smart  => Sort::Cpu,
            Sort::Cpu    => Sort::Mem,
            Sort::Mem    => Sort::Uptime,
            Sort::Uptime => Sort::Agent,
            Sort::Agent  => Sort::Smart,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Smart  => "smart",
            Sort::Cpu    => "cpu",
            Sort::Mem    => "mem",
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
    selected_pid: Option<u32>,
    visible_pid_order: Vec<u32>,
    quit: bool,
}

pub fn run(collector: Collector, args: Args) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let interval = Duration::from_millis((args.interval.max(0.1) * 1000.0) as u64);
    let initial_sort = match args.sort.as_str() {
        "cpu" => Sort::Cpu,
        "mem" => Sort::Mem,
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
        selected_pid: None,
        visible_pid_order: Vec::new(),
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
            if let Event::Key(key) = event::read()? {
                handle_key(app, key);
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if app.typing_filter {
        match key.code {
            KeyCode::Esc => {
                app.typing_filter = false;
                app.filter.clear();
            }
            KeyCode::Enter => {
                app.typing_filter = false;
            }
            KeyCode::Backspace => {
                app.filter.pop();
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('?') | KeyCode::Char('h') => app.show_help = !app.show_help,
        KeyCode::Char('p') => app.paused = !app.paused,
        KeyCode::Char('r') => {
            app.snap = app.collector.snapshot();
            app.last_tick = Instant::now();
        }
        KeyCode::Char('s') => app.sort = app.sort.cycle(),
        KeyCode::Char('g') => app.grouped = !app.grouped,
        KeyCode::Char('/') => {
            app.typing_filter = true;
            app.filter.clear();
        }
        KeyCode::Char('f') => {
            app.typing_filter = true;
            app.filter.clear();
        }
        KeyCode::Esc => {
            app.filter.clear();
        }
        KeyCode::Down | KeyCode::Char('j') => move_sel(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_sel(app, -1),
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
            Constraint::Length(8),    // CPU chart
            Constraint::Length(7),    // MEM chart
            Constraint::Min(6),       // active vs busy + sessions
        ])
        .split(body[1]);

    draw_agents(f, left[0], app);
    draw_left_bottom(f, left[1], &app.snap);
    draw_cpu_chart(f, right[0], &app.snap);
    draw_mem_chart(f, right[1], &app.snap);
    draw_right_bottom(f, right[2], &app.snap);

    draw_footer(f, chunks[2], app);

    if app.show_help {
        draw_help(f, area);
    } else if app.typing_filter {
        draw_filter_input(f, area, &app.filter);
    }
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
    chip("active",    a.active.to_string(),    theme::C_ACTIVE);
    chip("busy",      a.busy.to_string(),      theme::C_BUSY);
    chip("subagents", a.subagents.to_string(), theme::C_SPAWN);
    chip("waiting",   a.waiting.to_string(),   theme::C_WAIT);
    chip("done",      a.completed.to_string(), theme::C_DONE);
    chip("projects",  a.project_count.to_string(), theme::FG);
    chip("cpu",       pct(a.cpu),              theme::C_CHART_CPU);
    chip("mem",       format!("{}/{}", bytes(mem_used), bytes(snap.mem_total)),
                                               theme::C_CHART_MEM);
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
    let p = Paragraph::new(Line::from(spans)).block(block);
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
            // Project header row.
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
                header_spans.push(Span::styled(" sub",
                    Style::default().fg(theme::FG_DIM)));
            }
            let header_line = Line::from(header_spans);
            rows.push(Row::new(vec![header_line]).height(1));

            for a in list {
                pid_order.push(a.pid);
                rows.push(agent_row(a, app.selected_pid == Some(a.pid)));
            }
        }
    } else {
        for a in agents.iter() {
            pid_order.push(a.pid);
            rows.push(agent_row(a, app.selected_pid == Some(a.pid)));
        }
    }

    app.visible_pid_order = pid_order.clone();
    if app.selected_pid.is_none() {
        app.selected_pid = pid_order.first().copied();
    } else if let Some(p) = app.selected_pid {
        if !pid_order.contains(&p) {
            app.selected_pid = pid_order.first().copied();
        }
    }

    // Single-cell rows; we render as one wide column to keep the styled spans intact.
    let table = Table::new(rows, [Constraint::Percentage(100)])
        .column_spacing(0);
    f.render_widget(table, inner);
}

fn agent_row<'a>(a: &'a Agent, selected: bool) -> Row<'a> {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw("  "));
    // Status badge.
    spans.push(Span::styled(format!("{} {} ", a.status.glyph(), a.status.label()),
                            theme::status_style(a.status)));
    // Agent label chip.
    spans.push(Span::styled(format!("{:<12}", shorten(&a.label, 12)),
                            Style::default().fg(theme::agent_color(&a.label)).add_modifier(Modifier::BOLD)));
    // PID
    spans.push(Span::styled("pid ", Style::default().fg(theme::FG_DIM)));
    spans.push(Span::styled(format!("{:>7}", a.pid),
                            Style::default().fg(theme::FG)));
    spans.push(Span::raw(" "));
    // CPU
    spans.push(Span::styled(format!("{:>6}", pct(a.cpu)),
                            Style::default().fg(theme::cpu_color(a.cpu)).add_modifier(Modifier::BOLD)));
    spans.push(Span::raw(" "));
    // MEM
    spans.push(Span::styled(format!("{:>7}", bytes(a.rss)),
                            Style::default().fg(theme::C_CHART_MEM)));
    spans.push(Span::raw(" "));
    // Uptime
    spans.push(Span::styled(format!("{:>7}", dur(a.uptime_sec)),
                            Style::default().fg(theme::FG_DIM)));
    // Subagent badge
    if a.subagents > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("+{}", a.subagents),
                                Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(" sub", Style::default().fg(theme::FG_DIM)));
    } else {
        spans.push(Span::raw("       "));
    }
    spans.push(Span::raw("  "));
    // Doing
    spans.push(describe_doing_span(a));

    let line = Line::from(spans);
    let mut row = Row::new(vec![line]).height(1);
    if selected {
        row = row.style(Style::default().bg(theme::HL_BG));
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
    for p in snap.projects.iter().take(max_lines) {
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
        items.push(ListItem::new(Line::from(spans)));
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
    for e in snap.activity.iter().take(inner.height as usize) {
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
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled("  (no recent events)", Style::default().fg(theme::FG_DIM)))));
    }
    let list = List::new(items);
    f.render_widget(list, inner);
}

fn draw_cpu_chart(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let data: Vec<(f64, f64)> = snap.history.cpu.iter().enumerate()
        .map(|(i, v)| (i as f64, *v)).collect();
    let max_y = data.iter().map(|(_, y)| *y).fold(10.0_f64, f64::max);
    let max_y = (max_y / 10.0).ceil() * 10.0;

    let datasets = vec![
        Dataset::default()
            .name("CPU%")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(theme::C_CHART_CPU))
            .data(&data),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" CPU% ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("now {}  peak {}",
                pct(snap.aggregates.cpu),
                pct(data.iter().map(|(_, y)| *y).fold(0.0_f64, f64::max))),
                Style::default().fg(theme::FG_DIM)),
        ]));
    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(Axis::default()
            .style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, data.len().max(1) as f64 - 1.0]))
        .y_axis(Axis::default()
            .style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, max_y])
            .labels(vec![Span::raw("0"), Span::raw(format!("{:.0}", max_y / 2.0)), Span::raw(format!("{:.0}", max_y))]));
    f.render_widget(chart, area);
}

fn draw_mem_chart(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let data: Vec<(f64, f64)> = snap.history.mem.iter().enumerate()
        .map(|(i, v)| (i as f64, *v)).collect();
    let max_y = data.iter().map(|(_, y)| *y).fold(64.0_f64, f64::max);
    let datasets = vec![
        Dataset::default()
            .name("MB")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(theme::C_CHART_MEM))
            .data(&data),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Line::from(vec![
            Span::styled(" MEM ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!("now {}", bytes(snap.aggregates.mem_bytes)),
                Style::default().fg(theme::FG_DIM)),
        ]));
    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(Axis::default()
            .style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, data.len().max(1) as f64 - 1.0]))
        .y_axis(Axis::default()
            .style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, max_y])
            .labels(vec![Span::raw("0"),
                Span::raw(format!("{:.0}", max_y / 2.0)),
                Span::raw(format!("{:.0} MB", max_y))]));
    f.render_widget(chart, area);
}

fn draw_right_bottom(f: &mut Frame, area: Rect, snap: &Snapshot) {
    // Active vs Busy stacked-line chart on top, sessions panel underneath.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // Active vs busy chart.
    let active: Vec<(f64, f64)> = snap.history.active.iter().enumerate().map(|(i, v)| (i as f64, *v)).collect();
    let busy:   Vec<(f64, f64)> = snap.history.busy.iter().enumerate().map(|(i, v)| (i as f64, *v)).collect();
    let max_y = active.iter().chain(busy.iter()).map(|(_, y)| *y).fold(2.0_f64, f64::max);
    let datasets = vec![
        Dataset::default().name("active").marker(symbols::Marker::Braille).graph_type(GraphType::Line)
            .style(Style::default().fg(theme::C_CHART_ACTIVE)).data(&active),
        Dataset::default().name("busy").marker(symbols::Marker::Braille).graph_type(GraphType::Line)
            .style(Style::default().fg(theme::C_CHART_BUSY)).data(&busy),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Active vs Busy ", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(Axis::default().style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, active.len().max(1) as f64 - 1.0]))
        .y_axis(Axis::default().style(Style::default().fg(theme::BORDER_DIM))
            .bounds([0.0, max_y])
            .labels(vec![Span::raw("0"), Span::raw(format!("{:.0}", max_y))]));
    f.render_widget(chart, split[0]);

    // Sessions panel.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(" Claude sessions ",
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)));
    let inner = block.inner(split[1]);
    f.render_widget(block, split[1]);

    let s = &snap.sessions;
    let a = &snap.aggregates;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!(" {} ", a.busy), Style::default().fg(theme::C_BUSY).add_modifier(Modifier::BOLD)),
        Span::styled("busy   ", Style::default().fg(theme::FG_DIM)),
        Span::styled(format!("{} ", s.active.saturating_sub(a.busy)), Style::default().fg(theme::C_ACTIVE)),
        Span::styled("active   ", Style::default().fg(theme::FG_DIM)),
        Span::styled(format!("{} ", s.waiting), Style::default().fg(theme::C_WAIT)),
        Span::styled("waiting   ", Style::default().fg(theme::FG_DIM)),
        Span::styled(format!("{} ", s.completed), Style::default().fg(theme::C_DONE)),
        Span::styled("done", Style::default().fg(theme::FG_DIM)),
    ]));
    if a.subagents > 0 {
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", a.subagents), Style::default().fg(theme::C_SPAWN).add_modifier(Modifier::BOLD)),
            Span::styled(format!("Task subagent{} in flight",
                if a.subagents == 1 {""} else {"s"}),
                Style::default().fg(theme::FG_DIM)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(" Recent tasks", Style::default().fg(theme::FG).add_modifier(Modifier::BOLD))));
    if s.recent_tasks.is_empty() {
        lines.push(Line::from(Span::styled("  (none in last 24h)", Style::default().fg(theme::FG_DIM))));
    }
    for t in s.recent_tasks.iter().take((inner.height as usize).saturating_sub(4)) {
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
    f.render_widget(block, r);

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
        line(vec![key("  s           "), dim("cycle sort (smart / cpu / mem / uptime / agent)")]),
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
    let p = Paragraph::new(lines);
    f.render_widget(p, inner);
}
