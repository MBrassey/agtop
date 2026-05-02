//! Static panels rendered around the agents table — header chips,
//! footer hints, the left-bottom projects/activity strip, and all
//! the right-column charts (CPU / memory / tokens / status / Claude
//! sessions).  These read snapshot data; only the sessions panel
//! mutates state (its scroll offset).

use super::{App, Sort};
use crate::format::{bytes, pct, project_basename, shorten, si};
use crate::pricing::format_cost;
use crate::model::{ActivityKind, Agent, Snapshot, Status};
use crate::theme;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState,
        Sparkline, Wrap,
    },
    Frame,
};
use std::time::Duration;

pub(super) fn draw_header(f: &mut Frame, area: Rect, snap: &Snapshot, app: &App) {
    let a = &snap.aggregates;
    let mem_used = snap.mem_total.saturating_sub(snap.mem_available);

    let mut spans: Vec<Span> = vec![
        Span::styled(" agtop ",
            Style::default().fg(theme::border()).add_modifier(Modifier::BOLD)),
        Span::styled(format!("v{}  ", env!("CARGO_PKG_VERSION")), Style::default().fg(theme::fg_dim())),
    ];
    let mut chip = |label: &str, value: String, color: ratatui::style::Color| {
        spans.push(Span::styled(format!(" {} ", value),
                                Style::default().fg(color).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!("{} ", label), Style::default().fg(theme::fg_dim())));
    };
    // Order calibrated by user-priority on first glance:
    // busy first (most actionable), then cost / tokens (financial
    // context), then load / capacity, then breakdown counts.
    chip("busy",      a.busy.to_string(),      theme::c_busy());
    if a.cost_usd > 0.0 {
        chip("cost",   format_cost(a.cost_usd), theme::c_wait());
    }
    if a.tokens_total > 0 {
        chip("tokens", si(a.tokens_total),     theme::c_chart_tok());
    }
    chip("cpu",       pct(a.cpu),              theme::c_chart_cpu());
    chip("mem",       format!("{}/{}", bytes(mem_used), bytes(snap.mem_total)),
                                               theme::c_chart_mem());
    chip("active",    a.active.to_string(),    theme::c_active());
    chip("waiting",   a.waiting.to_string(),   theme::c_wait());
    chip("done",      a.completed.to_string(), theme::c_done());
    chip("subagents", a.subagents.to_string(), theme::c_spawn());
    chip("projects",  a.project_count.to_string(), theme::fg());
    // ▼ marker on the sort label — the sort always reads
    // descending-by-key (highest cpu/mem/tokens/uptime first; agent
    // is alphabetical ascending).  Visual cue saves users from having
    // to re-cycle `s` to confirm what's active.
    let sort_arrow = match app.sort {
        Sort::Agent => "▲",
        _ => "▼",
    };
    spans.push(Span::styled(
        format!(" sort:{}{}  group:{}  ",
            app.sort.label(), sort_arrow,
            if app.grouped {"on"} else {"off"}),
        Style::default().fg(theme::fg_dim())));
    // Cache-savings chip — sum of Anthropic prompt-cache reads
    // converted to dollars saved across all agents.  Only renders
    // when the savings exceed a cent, so the header doesn't clutter
    // up with `$0.00 saved` for fresh sessions.
    let total_saved: f64 = snap.agents.iter()
        .filter_map(|a| {
            let model = a.model.as_deref()?;
            let p = app.collector.pricing().lookup(model)?;
            if a.tokens_cache_read == 0 { return None; }
            Some((a.tokens_cache_read as f64 / 1_000_000.0)
                * p.input_per_mtok * 0.90)
        })
        .sum();
    if total_saved >= 0.01 {
        spans.push(Span::styled(format!(" {} ", format_cost(total_saved)),
            Style::default().fg(theme::c_active()).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("cache-saved  ", Style::default().fg(theme::fg_dim())));
    }
    if !app.filter.is_empty() {
        spans.push(Span::styled(format!("filter:{}  ", app.filter),
                                Style::default().fg(theme::c_wait())));
    }
    if app.paused {
        spans.push(Span::styled(" PAUSED ",
                                Style::default().bg(theme::c_wait()).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)));
    }
    // Surface the platform note (e.g. "running via sysinfo backend — IO
    // bytes and writing-files unavailable") so non-Linux users know why
    // certain columns will read `—` instead of misleadingly showing 0.
    if let Some(note) = snap.note.as_deref() {
        spans.push(Span::styled(format!("  ⓘ {} ", note),
                                Style::default().fg(theme::fg_dim())));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()));
    let p = Paragraph::new(Line::from(spans)).block(block).wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

pub(super) fn draw_left_bottom(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    draw_projects(f, inner[0], snap);
    draw_activity(f, inner[1], snap);
}

pub(super) fn draw_projects(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(" Projects ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
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
                                Style::default().fg(theme::fg())));
        spans.push(Span::styled(format!("{:>2}", p.agents), Style::default().fg(theme::fg_dim())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", pct(p.cpu)),
                                Style::default().fg(theme::cpu_color(p.cpu))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar, Style::default().fg(theme::cpu_color(p.cpu))));
        spans.push(Span::styled(bar_pad, Style::default().fg(theme::border_dim())));
        if p.subagents > 0 {
            spans.push(Span::styled(format!(" +{}", p.subagents),
                                    Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD)));
        }
        items.push(ListItem::new(Line::from(spans)).style(Style::default().bg(row_bg)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled("  (no projects)", Style::default().fg(theme::fg_dim())))));
    }
    let list = List::new(items);
    f.render_widget(list, inner);
}

pub(super) fn draw_activity(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(" Activity ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
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
            ActivityKind::Spawn => ("●", Style::default().fg(theme::c_busy()).add_modifier(Modifier::BOLD)),
            ActivityKind::Exit  => ("◌", Style::default().fg(theme::fg_dim())),
        };
        let kind = match e.kind { ActivityKind::Spawn => "spawn", ActivityKind::Exit => "exit " };
        let cwd = e.cwd.as_deref().map(project_basename).unwrap_or_default();
        let mut spans: Vec<Span> = vec![
            Span::styled(t, Style::default().fg(theme::fg_dim())),
            Span::raw("  "),
            Span::styled(glyph.to_string(), glyph_style),
            Span::raw(" "),
            Span::styled(kind.to_string(), Style::default().fg(theme::fg_dim())),
            Span::raw("  "),
            Span::styled(format!("{:<12}", shorten(&e.label, 12)),
                Style::default().fg(theme::agent_color(&e.label))),
            Span::raw(" "),
            Span::styled(format!("pid {:<7}", e.pid),
                Style::default().fg(theme::fg_dim())),
        ];
        if !cwd.is_empty() {
            spans.push(Span::styled(format!("  {}", cwd),
                Style::default().fg(theme::fg())));
        }
        items.push(ListItem::new(Line::from(spans)).style(Style::default().bg(row_bg)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled("  (no recent events)", Style::default().fg(theme::fg_dim())))));
    }
    let list = List::new(items);
    f.render_widget(list, inner);
}

// CPU panel: smooth braille Sparkline on top showing system trend, then a
// horizontal-bar list per agent — same visual language as the Memory and
// Status panels so the right column reads as a single coherent piece.
pub(super) fn draw_cpu_panel(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let peak = snap.history.cpu.iter().copied().fold(0.0_f64, f64::max);
    let avg = if !snap.history.cpu.is_empty() {
        snap.history.cpu.iter().sum::<f64>() / snap.history.cpu.len() as f64
    } else { 0.0 };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Line::from(vec![
            Span::styled(" CPU ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} cores · ", snap.sys_cpus),
                Style::default().fg(theme::fg_dim())),
            Span::styled("now ", Style::default().fg(theme::fg_dim())),
            Span::styled(pct(snap.aggregates.cpu),
                Style::default().fg(theme::cpu_color(snap.aggregates.cpu)).add_modifier(Modifier::BOLD)),
            Span::styled(" · peak ", Style::default().fg(theme::fg_dim())),
            Span::styled(pct(peak),
                Style::default().fg(theme::c_chart_cpu()).add_modifier(Modifier::BOLD)),
            Span::styled(" · avg ", Style::default().fg(theme::fg_dim())),
            Span::styled(pct(avg),
                Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default().fg(theme::fg_dim())),
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
    // Slice to the *current* chart width so a live terminal resize
    // expands/contracts the chart immediately rather than freezing at
    // whatever width was active when the app started.  The collector
    // keeps a generous 240-sample ring (HISTORY) so a wide terminal
    // has enough data to fill its area; we always show the most recent
    // `width` samples (oldest fall off the left).
    let want = split[0].width as usize;
    let spark_data: Vec<u64> = snap.history.cpu.iter()
        .rev().take(want).rev()
        .map(|v| (v * 10.0) as u64).collect();
    let spark_max = ((peak.max(snap.aggregates.cpu).max(10.0) / 10.0).ceil() * 10.0 * 10.0) as u64;
    let sparkline = Sparkline::default()
        .data(&spark_data)
        .max(spark_max)
        .style(Style::default().fg(theme::c_chart_cpu()));
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
            Style::default().fg(theme::fg())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full, Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::border_dim())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", pct(a.cpu)),
            Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no agents)", Style::default().fg(theme::fg_dim())))));
    }
    f.render_widget(List::new(items), split[1]);
}

// Memory panel: top-N agents by RSS as horizontal bars, plus a system memory
// gauge showing free / agent-attributable / other-used.
pub(super) fn draw_memory_panel(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Line::from(vec![
            Span::styled(" Memory by agent ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} across {} agent{}",
                bytes(snap.aggregates.mem_bytes),
                snap.aggregates.active,
                if snap.aggregates.active == 1 {""} else {"s"}),
                Style::default().fg(theme::fg_dim())),
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
            Style::default().fg(theme::fg())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full,  Style::default().fg(theme::c_chart_mem()).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::border_dim())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>7}", bytes(a.rss)),
            Style::default().fg(theme::c_chart_mem())));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no agents)", Style::default().fg(theme::fg_dim())))));
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
                Style::default().fg(theme::c_gauge_agent())),
            Span::styled("█".repeat(other_cells.max(0) as usize),
                Style::default().fg(theme::c_gauge_used())),
            Span::styled("░".repeat(free_cells.max(0) as usize),
                Style::default().fg(theme::c_gauge_free())),
        ]);
        let label = Line::from(vec![
            Span::styled(" agents ", Style::default().fg(theme::fg_dim())),
            Span::styled(bytes(agent_mem),
                Style::default().fg(theme::c_gauge_agent()).add_modifier(Modifier::BOLD)),
            Span::styled("  other ", Style::default().fg(theme::fg_dim())),
            Span::styled(bytes(other),
                Style::default().fg(theme::c_gauge_used()).add_modifier(Modifier::BOLD)),
            Span::styled("  free ", Style::default().fg(theme::fg_dim())),
            Span::styled(bytes(avail),
                Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" / {}", bytes(total)),
                Style::default().fg(theme::fg_dim())),
        ]);
        f.render_widget(Paragraph::new(vec![line, label]), split[1]);
    }
}

// Tokens panel: rate sparkline at top, top-N agents by tokens beneath. Same
// horizontal-bar visual language as Memory panel.
pub(super) fn draw_tokens_panel(f: &mut Frame, area: Rect, snap: &Snapshot, interval: Duration) {
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
        .border_style(Style::default().fg(theme::border()))
        .title(Line::from(vec![
            Span::styled(" Tokens ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled("total ", Style::default().fg(theme::fg_dim())),
            Span::styled(si(snap.aggregates.tokens_total),
                Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)),
            Span::styled("  rate ", Style::default().fg(theme::fg_dim())),
            Span::styled(format!("{}/min", si(rate_per_min as u64)),
                Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)),
            Span::styled(" ", Style::default().fg(theme::fg_dim())),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    // Sparkline of tokens-per-tick.  Slice to the live chart width so
    // a terminal resize re-fills/contracts the chart on the next tick.
    let want = split[0].width as usize;
    let spark_data: Vec<u64> = snap.history.tokens_rate.iter()
        .rev().take(want).rev()
        .map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .data(&spark_data)
        .style(Style::default().fg(theme::c_chart_tok()));
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
            Style::default().fg(theme::fg())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:<8}", shorten(&a.label, 8)),
            Style::default().fg(theme::agent_color(&a.label))));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(bar_full,  Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(bar_empty, Style::default().fg(theme::border_dim())));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{:>6}", si(a.tokens_total)),
            Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
        items.push(ListItem::new(Line::from(spans)));
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no token usage detected — open a Claude/Codex session)",
            Style::default().fg(theme::fg_dim())))));
    }
    f.render_widget(List::new(items), split[1]);
}

// Status distribution: htop-style horizontal segment bars, one per status.
pub(super) fn draw_status_distribution(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Line::from(vec![
            Span::styled(" Status distribution ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} live agent{} · {} session{}",
                snap.aggregates.active,
                if snap.aggregates.active == 1 {""} else {"s"},
                snap.sessions.waiting + snap.sessions.completed + snap.sessions.active,
                ""),
                Style::default().fg(theme::fg_dim())),
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
            Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("█".repeat(filled),
            Style::default().fg(theme::status_color(status)).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("·".repeat(bar_w - filled),
            Style::default().fg(theme::border_dim())));
        spans.push(Span::styled(format!(" {:>4.1}%", pct_v),
            Style::default().fg(theme::fg_dim())));
        Line::from(spans)
    };
    let lines = vec![
        row("BUSY",   *counts.get("busy").unwrap_or(&0),     Status::Busy),
        row("SPWN",   *counts.get("spawning").unwrap_or(&0), Status::Spawning),
        row("ACTV",   *counts.get("active").unwrap_or(&0),   Status::Active),
        row("IDLE",   *counts.get("idle").unwrap_or(&0),     Status::Idle),
        row("WAIT",   waiting,                               Status::Waiting),
        row("DONE",   done,                                  Status::Completed),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_sessions(f: &mut Frame, area: Rect, snap: &Snapshot, scroll: &mut u16) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(" Claude sessions — recent tasks ",
            Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let s = &snap.sessions;
    let a = &snap.aggregates;
    let mut lines: Vec<Line> = Vec::new();
    if a.subagents > 0 {
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", a.subagents),
                Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("Task subagent{} in flight ",
                if a.subagents == 1 {""} else {"s"}),
                Style::default().fg(theme::fg_dim())),
        ]));
    }
    if s.recent_tasks.is_empty() {
        lines.push(Line::from(Span::styled("  (no tasks in last 24h)",
            Style::default().fg(theme::fg_dim()))));
    }
    // Render every recent task — used to clip to viewport, but with a
    // scrollbar in place we surface the full list (Snapshot already
    // caps it to a sensible window upstream).
    for t in s.recent_tasks.iter() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(t.status.glyph().to_string(), theme::status_style(t.status)),
            Span::raw(" "),
            Span::styled(format!("{:<14}", shorten(&t.project_short, 14)),
                Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(shorten(&t.task, 80).to_string(),
                Style::default().fg(theme::fg_dim())),
        ]));
    }

    let total = lines.len() as u16;
    let max_scroll = total.saturating_sub(inner.height.saturating_sub(1));
    if *scroll > max_scroll { *scroll = max_scroll; }

    let body = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width.saturating_sub(if max_scroll > 0 { 1 } else { 0 }),
        height: inner.height,
    };
    let p = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((*scroll, 0));
    f.render_widget(p, body);

    if max_scroll > 0 {
        let mut sbs = ScrollbarState::default()
            .content_length(total as usize)
            .viewport_content_length(inner.height as usize)
            .position(*scroll as usize);
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(theme::border_dim()))
            .thumb_style(Style::default().fg(theme::border()).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(sb, inner, &mut sbs);
    }
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

pub(super) fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let s = format!(
        "  q quit · ? help · s sort({}{}) · g group({}) · t tree({}) · C compact({}) · / filter · K kill · p {} · ↑↓ select · Enter detail",
        app.sort.label(),
        match app.sort { Sort::Agent => "▲", _ => "▼" },
        if app.grouped {"on"} else {"off"},
        if app.tree_mode {"on"} else {"off"},
        if app.compact_rows {"on"} else {"off"},
        if app.paused {"resume"} else {"pause"},
    );
    let p = Paragraph::new(Span::styled(s, Style::default().fg(theme::fg_dim())));
    f.render_widget(p, area);
}


