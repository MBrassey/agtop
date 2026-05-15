//! Agents panel — the main table of running agents.  Tracks
//! selection, scrolling (via TableState), grouping by project,
//! tree-mode sub-rows, sticky group headers, and per-column
//! visibility (compact rows + 1–7 toggles).

use super::{App, Sort, TokenMode, COL_CPU, COL_DANGER, COL_MEM, COL_PID, COL_SUB, COL_TOK, COL_UPTIME};
use crate::format::{bytes, dur, pct, shorten, si};
use crate::model::{Agent, Status};
use crate::theme;

use ratatui::{
    layout::{Alignment, Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Row,
        Scrollbar, ScrollbarOrientation, ScrollbarState,
        Table,
    },
    Frame,
};

pub(super) fn filter_match(a: &Agent, f: &str) -> bool {
    if f.is_empty() { return true; }
    let f = f.to_lowercase();
    a.label.to_lowercase().contains(&f)
        || a.cmdline.to_lowercase().contains(&f)
        || a.cwd.to_lowercase().contains(&f)
        || a.project.to_lowercase().contains(&f)
        || a.pid.to_string() == f
}

pub(super) fn draw_agents(f: &mut Frame, area: Rect, app: &mut App) {
    let snap = &app.snap;
    let mut agents: Vec<&Agent> = snap.agents.iter().filter(|a| filter_match(a, &app.filter)).collect();
    match app.sort {
        Sort::Smart => {} // already sorted by collector
        Sort::Cpu     => agents.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)),
        Sort::Mem     => agents.sort_by(|a, b| b.rss.cmp(&a.rss)),
        Sort::Tokens  => {
            let m = app.tokens_mode;
            agents.sort_by_key(|a| std::cmp::Reverse(m.value(a)));
        }
        Sort::Uptime  => agents.sort_by(|a, b| b.uptime_sec.cmp(&a.uptime_sec)),
        Sort::Agent   => agents.sort_by(|a, b| a.label.cmp(&b.label)),
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(" Agents ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)))
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut pid_order: Vec<u32> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut agent_row_indices: Vec<usize> = Vec::new();
    let mut group_index: usize = 0;
    // (header_row_index, project_label, group_tint_index) — used for
    // sticky-header overlay when scrolled past the actual header.
    let mut header_meta: Vec<(usize, Line<'_>, usize)> = Vec::new();

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
        // Order projects according to the active sort key — the
        // per-agent sort above already shaped each project's row
        // order, but without this projects themselves stayed
        // anchored in status-rank order and the user-visible effect
        // of pressing `s` was hidden inside each group.  Smart mode
        // keeps the original status-rank-then-name behaviour.
        by_proj.sort_by(|(p1, l1), (p2, l2)| {
            use std::cmp::Ordering;
            match app.sort {
                Sort::Smart => {
                    let r1 = l1.first().map(|a| a.status.rank()).unwrap_or(99);
                    let r2 = l2.first().map(|a| a.status.rank()).unwrap_or(99);
                    r1.cmp(&r2).then(p1.cmp(p2))
                }
                Sort::Cpu => {
                    let c1: f64 = l1.iter().map(|a| a.cpu).sum();
                    let c2: f64 = l2.iter().map(|a| a.cpu).sum();
                    c2.partial_cmp(&c1).unwrap_or(Ordering::Equal).then(p1.cmp(p2))
                }
                Sort::Mem => {
                    let m1: u64 = l1.iter().map(|a| a.rss).sum();
                    let m2: u64 = l2.iter().map(|a| a.rss).sum();
                    m2.cmp(&m1).then(p1.cmp(p2))
                }
                Sort::Tokens => {
                    let m = app.tokens_mode;
                    let t1: u64 = l1.iter().map(|a| m.value(a)).sum();
                    let t2: u64 = l2.iter().map(|a| m.value(a)).sum();
                    t2.cmp(&t1).then(p1.cmp(p2))
                }
                Sort::Uptime => {
                    let u1: u64 = l1.iter().map(|a| a.uptime_sec).max().unwrap_or(0);
                    let u2: u64 = l2.iter().map(|a| a.uptime_sec).max().unwrap_or(0);
                    u2.cmp(&u1).then(p1.cmp(p2))
                }
                Sort::Agent => p1.cmp(p2),
            }
        });

        for (proj, list) in by_proj {
            let total_cpu: f64 = list.iter().map(|a| a.cpu).sum();
            let total_mem: u64 = list.iter().map(|a| a.rss).sum();
            let total_sub: u32 = list.iter().map(|a| a.subagents).sum();
            let total_tok: u64 = list.iter().map(|a| app.tokens_mode.value(a)).sum();
            let mut header_spans: Vec<Span> = Vec::new();
            header_spans.push(Span::styled("● ", Style::default().fg(theme::border())));
            header_spans.push(Span::styled(proj.clone(),
                Style::default().fg(theme::border()).add_modifier(Modifier::BOLD)));
            header_spans.push(Span::styled(
                format!("  {} agent{} · {} cpu · {} mem",
                    list.len(),
                    if list.len() == 1 {""} else {"s"},
                    pct(total_cpu),
                    bytes(total_mem)),
                Style::default().fg(theme::fg_dim())));
            if total_sub > 0 {
                header_spans.push(Span::styled(format!("  +{}", total_sub),
                    Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD)));
                header_spans.push(Span::styled(" sub", Style::default().fg(theme::fg_dim())));
            }
            if total_tok > 0 {
                header_spans.push(Span::styled(format!("  {}", si(total_tok)),
                    Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
                header_spans.push(Span::styled(" tok", Style::default().fg(theme::fg_dim())));
            }
            let header_line = Line::from(header_spans);
            // Project header inherits the group's tint so the cluster
            // visually belongs together.
            let header_row = Row::new(vec![header_line.clone()]).height(1)
                .style(Style::default().bg(theme::group_tint(group_index)));
            header_meta.push((rows.len(), header_line, group_index));
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
                rows.push(agent_row(a, app.selected_pid == Some(a.pid), tint, app.compact_rows, app.cols, app.tokens_mode));
            // (second call site — non-grouped path)
                if app.tree_mode {
                    push_child_rows(&mut rows, a, tint);
                }
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
            rows.push(agent_row(a, app.selected_pid == Some(a.pid), tint, app.compact_rows, app.cols, app.tokens_mode));
            if app.tree_mode {
                push_child_rows(&mut rows, a, tint);
            }
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
            Style::default().fg(theme::fg_dim())))]).height(1));
    }

    // Capture screen-row → pid map for mouse hit-testing in the next
    // event poll.  Each Row has height(1); subtract the table's own
    // scroll offset so clicks line up with what's actually painted
    // when the viewport has scrolled.
    let offset = app.agents_state.offset();
    app.clickable_rows.clear();
    for (i, ri) in agent_row_indices.iter().enumerate() {
        if let Some(pid) = pid_order.get(i) {
            let row_idx = *ri;
            if row_idx < offset { continue; }
            let y_local = (row_idx - offset) as u16;
            if y_local >= inner.height { continue; }
            app.clickable_rows.push((inner.y + y_local, *pid));
        }
    }

    // Drive auto-scroll via TableState — pass the table-row index of
    // the currently-selected agent so ratatui scrolls it into view
    // when selection moves past the viewport.  Inline row styling
    // continues to handle the visible "selected" highlight.
    let selected_table_idx = app.selected_pid
        .and_then(|p| pid_order.iter().position(|x| *x == p))
        .and_then(|i| agent_row_indices.get(i).copied());
    app.agents_state.select(selected_table_idx);
    app.agents_total_rows = rows.len();
    let total_rows = rows.len();

    // Reserve one column on the right for the panel's scrollbar.  The
    // scrollbar only renders when content actually overflows; the
    // reserved column is benign when it doesn't (just one extra space
    // of padding).
    let body_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width.saturating_sub(1),
        height: inner.height,
    };
    let table = Table::new(rows, [Constraint::Percentage(100)])
        .column_spacing(0);
    f.render_stateful_widget(table, body_area, &mut app.agents_state);

    if total_rows > inner.height as usize {
        // content_length = (max_scroll + 1) so thumb reaches the
        // very bottom of the track when the last row is at the
        // bottom of the viewport.  See popup.rs for the math.
        let viewport_h = inner.height as usize;
        let max_scroll = total_rows.saturating_sub(viewport_h);
        let mut sbs = ScrollbarState::default()
            .content_length(max_scroll.saturating_add(1))
            .viewport_content_length(viewport_h)
            .position(app.agents_state.offset());
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(theme::border_dim()))
            .thumb_style(Style::default().fg(theme::border()).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(sb, inner, &mut sbs);
    }

    // Sticky group header — when grouped and the viewport has
    // scrolled past the header row of the topmost-visible project,
    // re-render that header pinned to row 0 so the user always knows
    // which project owns the visible agents.  Only triggers when
    // offset > 0 and the actual header row is no longer in view.
    if app.grouped && offset > 0 && !header_meta.is_empty() {
        let pinned = header_meta.iter().rev()
            .find(|(idx, _, _)| *idx <= offset);
        if let Some((idx, line, gi)) = pinned {
            // Skip the overlay if the real header is itself visible
            // (the natural rendering already covers it).
            if *idx < offset {
                let bar = Rect {
                    x: body_area.x,
                    y: body_area.y,
                    width: body_area.width,
                    height: 1,
                };
                f.render_widget(ratatui::widgets::Clear, bar);
                let bg = Style::default().bg(theme::group_tint(*gi));
                f.render_widget(
                    Paragraph::new(line.clone()).style(bg),
                    bar,
                );
            }
        }
    }
}

/// Tree-mode helper: append `└─ pid (comm)` indented sub-rows for
/// every immediate child of `parent`.  Capped at 5 children per
/// parent to keep the panel scannable; spans a single Cell so the
/// row's column layout doesn't get desynced from agent_row's.
fn push_child_rows<'a>(rows: &mut Vec<Row<'a>>, parent: &'a Agent,
                       group_bg: Option<ratatui::style::Color>) {
    let mut style = Style::default().fg(theme::fg_dim());
    if let Some(bg) = group_bg { style = style.bg(bg); }
    for (i, (cpid, comm)) in parent.children.iter().take(5).enumerate() {
        let last = i + 1 == parent.children.len().min(5);
        let glyph = if last { "└─" } else { "├─" };
        let line = Line::from(Span::styled(
            format!("    {} {} ({})", glyph, comm, cpid),
            style,
        ));
        let mut r = Row::new(vec![line]);
        if let Some(bg) = group_bg { r = r.style(Style::default().bg(bg)); }
        rows.push(r);
    }
}

pub(super) fn agent_row<'a>(a: &'a Agent, selected: bool, group_bg: Option<ratatui::style::Color>, compact: bool, cols: u32, tokens_mode: TokenMode) -> Row<'a> {
    // Compact mode short-circuits per-column toggles by collapsing
    // the bitmap to "label + cpu + mem + danger + status only".  We
    // reuse the same column-gating logic below so there's a single
    // code path through the row builder.
    let cols = if compact { COL_CPU | COL_MEM | COL_DANGER } else { cols };
    let show = |c: u32| (cols & c) != 0;
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
    if show(COL_DANGER) && a.dangerous {
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

    // PID — for WSL-hosted agents this is the namespaced u32; the
    // helper strips the upper bits and returns the real Linux PID
    // inside the guest so the row reads naturally.  A small coloured
    // `w` prefix marks WSL rows without widening the column.
    if show(COL_PID) {
        let pid_str = format!("{}", a.display_pid());
        if a.host.starts_with("wsl:") {
            // Right-align inside 7 cells: pad, glyph, digits.
            let pad = 7usize.saturating_sub(1 + pid_str.len());
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled("w",
                Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(pid_str,
                Style::default().fg(theme::fg_dim())));
        } else {
            spans.push(Span::styled(format!("{:>7}", pid_str),
                                    Style::default().fg(theme::fg_dim())));
        }
        spans.push(Span::raw(" "));
    }

    // CPU% + 4-cell mini bar.
    if show(COL_CPU) {
        let cpu_col = theme::cpu_color(a.cpu);
        spans.push(Span::styled(format!("{:>5}", pct(a.cpu)),
                                Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(" "));
        let bar_cells = (a.cpu / 100.0 * 4.0).round().clamp(0.0, 4.0) as usize;
        spans.push(Span::styled("█".repeat(bar_cells),
                                Style::default().fg(cpu_col).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled("·".repeat(4 - bar_cells),
                                Style::default().fg(theme::border_dim())));
        spans.push(Span::raw(" "));
    }

    // MEM
    if show(COL_MEM) {
        spans.push(Span::styled(format!("{:>5}", bytes(a.rss)),
                                Style::default().fg(theme::c_chart_mem())));
        spans.push(Span::raw(" "));
    }

    // Uptime
    if show(COL_UPTIME) {
        spans.push(Span::styled(format!("{:>6}", dur(a.uptime_sec)),
                                Style::default().fg(theme::fg_dim())));
    }

    // Subagent chip — fixed 4-char slot, capped at 99 so two-digit counts
    // (which would otherwise widen the column and shift DOING) fit.
    if show(COL_SUB) {
        if a.subagents > 0 {
            spans.push(Span::styled(format!(" +{:<2}", a.subagents.min(99)),
                                    Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD)));
        } else {
            spans.push(Span::raw("    "));
        }
    }

    // Token chip — fixed 6-char slot, value clipped to 5 visible chars so
    // even "100M+" abbreviations don't push downstream columns.
    if show(COL_TOK) {
        let tok_val = tokens_mode.value(a);
        if tok_val > 0 {
            let s = si(tok_val);
            let clipped: String = s.chars().take(5).collect();
            spans.push(Span::styled(format!(" {:>5}", clipped),
                                    Style::default().fg(theme::c_chart_tok())));
        } else {
            spans.push(Span::raw("      "));
        }
    }

    // DOING — gets all remaining width.
    spans.push(Span::raw("  "));
    spans.push(describe_doing_span(a));

    let line = Line::from(spans);
    let mut row = Row::new(vec![line]).height(1);
    if selected {
        row = row.style(Style::default().bg(theme::hl_bg()).add_modifier(Modifier::BOLD));
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
            Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD));
    }
    if let Some(t) = &a.current_task {
        return Span::styled(shorten(t, 70).to_string(),
            Style::default().fg(theme::fg()));
    }
    if a.status == Status::Idle {
        if let Some(age) = a.session_age_ms {
            return Span::styled(format!("(idle {})", dur(age / 1000)),
                Style::default().fg(theme::fg_dim()));
        }
    }
    if a.status == Status::Waiting {
        return Span::styled("(awaiting input)".to_string(), Style::default().fg(theme::c_wait()));
    }
    if a.status == Status::Completed {
        return Span::styled("(session ended)".to_string(), Style::default().fg(theme::c_done()));
    }
    Span::styled(shorten(&a.cmdline, 80).to_string(),
                 Style::default().fg(theme::fg_dim()))
}

