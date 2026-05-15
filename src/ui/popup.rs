//! Modal popups: detail (per-agent), help, kill confirmation, filter
//! prompt.  All draw functions take `&mut App` so they can persist
//! per-popup state (scroll, filter, sections) back through the
//! parent module's state struct.

use super::{App, centered_rect};
use crate::format::{bytes, dur, pct, shorten, si, sparkline};
use crate::pricing::format_cost;
use crate::model::Snapshot;
use crate::theme;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
    Frame,
};

pub(super) fn draw_confirm_kill(f: &mut Frame, area: Rect, snap: &Snapshot, pid: u32) {
    let agent = snap.agents.iter().find(|a| a.pid == pid);
    let r = centered_rect(64, 8, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::c_busy()))
        .title(Span::styled(" Stop agent? ",
            Style::default().fg(theme::c_busy()).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(ratatui::widgets::Clear, r);
    f.render_widget(block, r);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(a) = agent {
        let mut header = vec![
            Span::styled(format!("  {} ", a.status.glyph()), theme::status_style(a.status)),
            Span::styled(format!("{:<10} ", a.label),
                Style::default().fg(theme::agent_color(&a.label)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("pid {} ", a.display_pid()),
                Style::default().fg(theme::fg_dim())),
        ];
        if !a.host.is_empty() {
            header.push(Span::styled(format!("{} ", a.host),
                Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
        }
        header.push(Span::styled(format!("· {}", a.project),
            Style::default().fg(theme::border()).add_modifier(Modifier::BOLD)));
        lines.push(Line::from(header));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Sending SIGTERM gives the agent a chance to clean up.",
            Style::default().fg(theme::fg_dim()))));
        lines.push(Line::from(Span::styled(
            "  Hit `y` to confirm, `n` or `Esc` to cancel.",
            Style::default().fg(theme::fg()))));
    } else {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  pid {pid} is no longer in the agent list. `Esc` to dismiss."),
            Style::default().fg(theme::fg_dim()))));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_detail(f: &mut Frame, area: Rect, app: &mut App) {
    let agent = app.snap.agents.iter().find(|a| Some(a.pid) == app.selected_pid);
    let Some(a) = agent else { return; };

    // Pop-out is now larger by default — we used to cap at 100×32 on
    // the assumption it would always fit, which truncated the lower
    // sections (background activity / live preview / writing files).
    // With scroll support we let it grow to ~80% of viewport so most
    // content is visible without paging, and scrolling handles the
    // rest.
    let r = centered_rect(
        (area.width.saturating_mul(8) / 10).max(60),
        (area.height.saturating_mul(9) / 10).max(20),
        area,
    );
    let w = r.width;
    let _h = r.height;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title({
            let mut title = vec![
                Span::styled(format!(" {} {} ", a.status.glyph(), a.status.label()),
                    theme::status_style(a.status)),
                Span::styled(format!("{} ", a.label),
                    Style::default().fg(theme::agent_color(&a.label)).add_modifier(Modifier::BOLD)),
                Span::styled(format!("pid {} ", a.display_pid()),
                    Style::default().fg(theme::fg_dim())),
            ];
            if !a.host.is_empty() {
                title.push(Span::styled(format!("{} ", a.host),
                    Style::default().fg(theme::c_chart_tok()).add_modifier(Modifier::BOLD)));
            }
            title.push(Span::styled(format!("· {} ", a.project),
                Style::default().fg(theme::border()).add_modifier(Modifier::BOLD)));
            Line::from(title)
        });
    let inner = block.inner(r);
    f.render_widget(ratatui::widgets::Clear, r);
    f.render_widget(block, r);

    let dim   = |s: String| Span::styled(s, Style::default().fg(theme::fg_dim()));
    let lab   = |s: &str| Span::styled(format!("{:<10}", s), Style::default().fg(theme::fg_dim()));
    let val   = |s: String, c: ratatui::style::Color| Span::styled(s, Style::default().fg(c).add_modifier(Modifier::BOLD));

    let cpu_spark = sparkline(&a.cpu_history, 100.0, 24);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![lab("model"),
        a.model.as_deref().map(|m| val(m.into(), theme::c_chart_tok())).unwrap_or_else(|| dim("(unknown)".into()))]));
    // Host: where this agent's process actually lives.  Empty (native
    // = the same OS agtop is running on) is skipped to avoid noise;
    // WSL agents render as `wsl · <distro>` so the user knows the
    // PID lives inside the guest, not the Windows kernel.
    if !a.host.is_empty() {
        let host_label = if let Some(distro) = a.host.strip_prefix("wsl:") {
            format!("wsl · {}", distro)
        } else {
            a.host.clone()
        };
        lines.push(Line::from(vec![lab("host"),
            val(host_label, theme::c_chart_tok())]));
    }
    lines.push(Line::from(vec![lab("cpu"),
        val(pct(a.cpu), theme::cpu_color(a.cpu)), Span::raw(" "),
        Span::styled(cpu_spark, Style::default().fg(theme::cpu_color(a.cpu)))]));
    lines.push(Line::from(vec![lab("memory"),
        val(bytes(a.rss), theme::c_chart_mem()), dim(format!(" rss · {} vsize", bytes(a.vsize)))]));
    // Uptime + session-start divergence: when claude --resume is used,
    // the process is brand new but the session is days old.  Show both.
    let mut uptime_spans = vec![lab("uptime"), val(dur(a.uptime_sec), theme::fg())];
    if a.session_started_ms > 0 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let session_age_s = now_ms.saturating_sub(a.session_started_ms) / 1000;
        let resumed = session_age_s > a.uptime_sec.saturating_add(60);
        let suffix = if resumed { " (resumed)" } else { "" };
        uptime_spans.push(dim(format!("  ·  session {}{}", dur(session_age_s), suffix)));
    }
    lines.push(Line::from(uptime_spans));
    let mut thread_spans = vec![lab("threads"), val(a.threads.to_string(), theme::fg())];
    let ppid_label = if a.ppid_name.is_empty() {
        format!("  state {}  ppid {}", a.state, a.ppid)
    } else {
        format!("  state {}  ppid {} ({})", a.state, a.ppid, a.ppid_name)
    };
    thread_spans.push(dim(ppid_label));
    lines.push(Line::from(thread_spans));
    // Permission flag breakdown — when dangerous=true, surface WHICH
    // flag triggered the classifier so the user knows what's actually
    // in play (vs. just a generic GOD-mode marker).
    if a.dangerous && !a.dangerous_flag.is_empty() {
        lines.push(Line::from(vec![
            lab("dangerous"),
            val(a.dangerous_flag.clone(), theme::c_wait()),
        ]));
    }
    lines.push(Line::from(vec![lab("tokens"),
        val(si(a.tokens_total), theme::c_chart_tok()),
        dim(format!(" ({} in / {} out)", si(a.tokens_input), si(a.tokens_output)))]));
    // Per-agent token-rate sparkline.  Each cell is one tick of
    // `tokens consumed since the previous snapshot` so a flat low
    // baseline + occasional spikes matches a typical Claude session
    // (idle while user types, burst on each turn).  Scale max is
    // the history's own peak so quiet sessions still show texture.
    if !a.tokens_history.is_empty() && a.tokens_history.iter().any(|v| *v > 0.0) {
        let peak = a.tokens_history.iter().copied().fold(0.0_f64, f64::max).max(1.0);
        let spark = sparkline(&a.tokens_history, peak, 24);
        let avg = a.tokens_history.iter().sum::<f64>() / a.tokens_history.len() as f64;
        let per_min = avg * 60.0 / app.interval.as_secs_f64().max(0.1);
        lines.push(Line::from(vec![
            lab("rate"),
            Span::styled(spark, Style::default().fg(theme::c_chart_tok())),
            dim(format!(" {}/min avg · peak {}", si(per_min as u64), si(peak as u64))),
        ]));
    }
    match a.cost_basis.as_str() {
        "api" if a.cost_usd > 0.0 => {
            lines.push(Line::from(vec![
                lab("cost"),
                val(format_cost(a.cost_usd), theme::c_wait()),
                dim(format!("  api · prices as of {}", crate::pricing::prices_updated())),
            ]));
        }
        "local" => {
            lines.push(Line::from(vec![
                lab("cost"),
                val("local".to_string(), theme::c_idle()),
                dim("  no API cost — model runs on this machine".to_string()),
            ]));
        }
        "unknown" if a.model.is_some() => {
            lines.push(Line::from(vec![
                lab("cost"),
                val("unknown".to_string(), theme::c_idle()),
                dim(format!(
                    "  no price for `{}` — pass --prices to set one",
                    a.model.as_deref().unwrap_or(""),
                )),
            ]));
        }
        _ => {}
    }
    // Cache-hit ratio + dollars saved.  Only meaningful when we have
    // a known model price + non-trivial cache reads.  Computed
    // independently of the cost row so it shows even on local /
    // unknown rows where cost_usd is 0.
    if a.tokens_cache_read > 0 && a.tokens_input > 0 {
        let hit_pct = (a.tokens_cache_read as f64 / a.tokens_input as f64) * 100.0;
        let mut spans = vec![lab("cache"),
            val(format!("{:.0}% hit", hit_pct), theme::c_chart_tok()),
            dim(format!("  ({} of {} input tok cached)",
                si(a.tokens_cache_read), si(a.tokens_input)))];
        // Anthropic prompt-cache reads are billed at 0.1× input rate;
        // the savings are 0.9× the input rate × cache_read tokens.
        if let Some(model) = a.model.as_deref() {
            if let Some(p) = app.collector.pricing().lookup(model) {
                let saved = (a.tokens_cache_read as f64 / 1_000_000.0)
                    * p.input_per_mtok * 0.90;
                if saved >= 0.01 {
                    spans.push(dim(format!("  · saved {} vs uncached", format_cost(saved))));
                }
            }
        }
        lines.push(Line::from(spans));
    }
    // Context-window fill — only meaningful when we know both used and
    // limit.  Renders a 24-cell tinted bar plus "X% used (used / limit)".
    if a.context_used > 0 && a.context_limit > 0 {
        let pct_used = (a.context_used as f64 / a.context_limit as f64).clamp(0.0, 1.0);
        let pct_int = (pct_used * 100.0) as u32;
        let bar_w   = 24usize;
        let filled  = ((pct_used * bar_w as f64).round() as usize).min(bar_w);
        let empty   = bar_w.saturating_sub(filled);
        // Colour-code the bar against compaction proximity.  Claude
        // Code triggers auto-compaction around 95% of the model's
        // context window; agtop nudges amber at 70% and red at 90%
        // so the user has time to act.
        let bar_color = if pct_used >= 0.90 { theme::c_busy() }
                        else if pct_used >= 0.70 { theme::c_wait() }
                        else { theme::c_active() };
        let mut spans = vec![lab("context")];
        spans.push(Span::styled("█".repeat(filled), Style::default().fg(bar_color)));
        spans.push(Span::styled("░".repeat(empty),  Style::default().fg(theme::fg_dim())));
        spans.push(Span::styled(
            format!(" {}% ", pct_int),
            Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(dim(format!("({} / {} tok)", si(a.context_used), si(a.context_limit))));
        if pct_used >= 0.90 {
            spans.push(dim("  · approaching auto-compaction".to_string()));
        }
        // Time-to-compaction extrapolation (only when we have enough
        // history and the trend is positive).  Shows next to the bar
        // for the visual at-a-glance read.
        if let Some(secs) = app.collector.time_to_compaction_secs(a.pid, a.context_limit) {
            let rate = app.collector.context_growth_per_min(a.pid).unwrap_or(0);
            spans.push(dim(format!("  · ≈{} to compaction (+{}/min)",
                dur(secs), si(rate))));
        }
        lines.push(Line::from(spans));
    }
    // Skills loaded for this session (Claude Code only — populated by
    // skills::skills_for_cwd which scans <cwd>/.claude/skills/ +
    // ~/.claude/skills/).  Always show the row for Claude agents so
    // a "0 loaded" line is discoverable; non-Claude vendors omit.
    if a.label == "claude" {
        if a.loaded_skills.is_empty() {
            lines.push(Line::from(vec![
                lab("skills"),
                dim("0 loaded — drop one in ~/.claude/skills/<name>/SKILL.md".to_string()),
            ]));
        } else {
            lines.push(Line::from(vec![
                lab("skills"),
                val(format!("{} loaded", a.loaded_skills.len()), theme::c_chart_tok()),
                dim(format!("  {}", shorten(&a.loaded_skills.join(", "),
                    (w as usize).saturating_sub(28)))),
            ]));
        }
        // Plugins are user-global, resolved from
        // ~/.claude/settings.json (enabledPlugins map) intersected
        // with ~/.claude/plugins/installed_plugins.json.  Same set
        // for every Claude row in the snapshot.
        if a.loaded_plugins.is_empty() {
            lines.push(Line::from(vec![
                lab("plugins"),
                dim("0 enabled — `/plugin marketplace add` to install".to_string()),
            ]));
        } else {
            lines.push(Line::from(vec![
                lab("plugins"),
                val(format!("{} enabled", a.loaded_plugins.len()), theme::c_spawn()),
                dim(format!("  {}", shorten(&a.loaded_plugins.join(", "),
                    (w as usize).saturating_sub(28)))),
            ]));
        }
    }
    if a.subagents > 0 {
        lines.push(Line::from(vec![lab("subagents"),
            val(format!("{} in flight", a.subagents), theme::c_spawn())]));
        for s in a.in_flight_subagents.iter().take(16) {
            lines.push(Line::from(vec![
                Span::raw("           "),
                Span::styled("· ", Style::default().fg(theme::c_spawn())),
                Span::styled(shorten(s, (w as usize).saturating_sub(14)),
                    Style::default().fg(theme::fg())),
            ]));
        }
    }
    // tools-in-flight (Bash/Edit/Read/etc.) — separate from Task subagents
    // so users distinguish "spawned a subagent" from "running a tool".
    let tools_in_flight = a.subagents == 0 && a.session_id.is_some()
        && matches!(a.status, crate::model::Status::Busy);
    if tools_in_flight {
        lines.push(Line::from(vec![lab("tools"),
            val("running".to_string(), theme::c_spawn())]));
    }
    if let Some(sid) = &a.session_id {
        lines.push(Line::from(vec![lab("session"), dim(sid.clone())]));
    }
    // Top tools used in this session (already pre-sorted descending by
    // count in the vendor enricher).  Surfaces effort allocation —
    // surprisingly informative for understanding what the agent
    // actually spent its time doing.
    if !a.tool_counts.is_empty() {
        let parts: Vec<String> = a.tool_counts.iter().take(5)
            .map(|(name, n)| format!("{} {}", name, n))
            .collect();
        lines.push(Line::from(vec![
            lab("tools"),
            Span::styled(parts.join(" · "), Style::default().fg(theme::c_chart_tok())),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![lab("bin"),  Span::styled(shorten(&a.exe, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::fg()))]));
    lines.push(Line::from(vec![lab("cwd"),  Span::styled(shorten(&a.cwd, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::fg()))]));
    lines.push(Line::from(vec![lab("cmd"),  Span::styled(shorten(&a.cmdline, (w as usize).saturating_sub(12)),
        Style::default().fg(theme::fg_dim()))]));
    if let Some(tool) = &a.current_tool {
        lines.push(Line::from(vec![lab("tool"),
            val(tool.clone(), theme::c_spawn())]));
    }
    if let Some(task) = &a.current_task {
        lines.push(Line::from(vec![lab("task"),
            Span::styled(shorten(task, (w as usize).saturating_sub(12)),
                Style::default().fg(theme::fg()))]));
    }
    if !a.writing_files.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![lab("writing"),
            dim(format!("{} file{}", a.writing_files.len(),
                if a.writing_files.len() == 1 { "" } else { "s" }))]));
        for f in a.writing_files.iter().take(16) {
            lines.push(Line::from(vec![Span::raw("    "),
                Span::styled(shorten(f, (w as usize).saturating_sub(6)),
                    Style::default().fg(theme::fg_dim()))]));
        }
    }
    // Background-activity surface — what's the agent actually doing
    // when no tokens are flowing?  Reading FDs, spawned children,
    // open network connections.  Only rendered when there's something
    // to show, but the section header lights up whenever any of the
    // three signals is present so the user has a single place to
    // look for "why is this row at 15% CPU with nothing happening".
    let has_bg = !a.reading_files.is_empty()
        || !a.children.is_empty()
        || a.net_established > 0;
    if has_bg {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  ─ Background activity ".to_string()
                + &"─".repeat((w as usize).saturating_sub(26)),
            Style::default().fg(theme::border_dim()))));
        if a.net_established > 0 {
            lines.push(Line::from(vec![
                lab("network"),
                val(format!("{} established", a.net_established), theme::c_chart_tok()),
                dim("  (TCP — agent is talking to API / MCP / network)".to_string()),
            ]));
        }
        if !a.children.is_empty() {
            let parts: Vec<String> = a.children.iter().take(6)
                .map(|(p, c)| format!("{} ({})", c, p)).collect();
            lines.push(Line::from(vec![
                lab("children"),
                val(format!("{} spawned", a.children.len()), theme::c_spawn()),
                dim(format!("  {}", shorten(&parts.join(" · "),
                    (w as usize).saturating_sub(28)))),
            ]));
        }
        if !a.reading_files.is_empty() {
            lines.push(Line::from(vec![lab("reading"),
                dim(format!("{} file{}", a.reading_files.len(),
                    if a.reading_files.len() == 1 { "" } else { "s" }))]));
            for f in a.reading_files.iter().take(16) {
                lines.push(Line::from(vec![Span::raw("    "),
                    Span::styled(shorten(f, (w as usize).saturating_sub(6)),
                        Style::default().fg(theme::fg_dim()))]));
            }
        }
    }
    // Live preview: a fenced box at the bottom showing the last few
    // events from the session transcript.  Color-coded by glyph: › prose,
    // → tool call, ← tool result.  Always rendered so users know whether
    // the lack-of-content is "no activity" vs. "this agent doesn't ship a
    // transcript reader".
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ─ Live preview ".to_string() + &"─".repeat((w as usize).saturating_sub(20)),
        Style::default().fg(theme::border_dim()))));
    if a.recent_activity.is_empty() {
        // Empty preview can mean three different things; spell each
        // out explicitly so the user isn't staring at a mysteriously
        // blank box.
        let has_session   = a.session_id.is_some();
        let has_reader    = matches!(a.label.as_str(),
            "claude" | "codex" | "goose" | "aider" | "gemini");
        let hint: &str = if has_session {
            "  (no recent activity in this session)"
        } else if has_reader {
            // We have a vendor reader but couldn't link any session
            // JSONL to this PID's cwd.  Most common cause for Claude
            // is the cwd-encoding mismatch (process cd'd after start)
            // or the session being too new to have its first turn.
            match a.label.as_str() {
                "claude"  => "  (no Claude session found for this cwd — `ls ~/.claude/projects/` to inspect)",
                "codex"   => "  (no Codex rollout found for this cwd in ~/.codex/sessions/)",
                "goose"   => "  (no Goose session found in ~/.config/goose/sessions/)",
                "aider"   => "  (no .aider.chat.history.md in this cwd yet)",
                "gemini"  => "  (no Gemini session found in ~/.gemini/sessions/)",
                _ => "  (no session found)",
            }
        } else {
            "  (no transcript reader for this agent type — see /proc fields above)"
        };
        lines.push(Line::from(Span::styled(hint.to_string(),
            Style::default().fg(theme::fg_dim()))));
    } else {
        // With scroll support we no longer have to cram the preview
        // into whatever rows happen to be left in the popup; surface
        // up to the last 50 events and let the user scroll through.
        for ev in a.recent_activity.iter().rev().take(50).rev() {
            let glyph_col = if ev.starts_with("› ")      { theme::fg() }
                            else if ev.starts_with("→ ") { theme::c_spawn() }
                            else if ev.starts_with("← ") { theme::c_active() }
                            else                          { theme::fg_dim() };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(shorten(ev, (w as usize).saturating_sub(4)),
                    Style::default().fg(glyph_col)),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  Esc / Enter close · j/k scroll · g/G top/bot · n/N section",
            Style::default().fg(theme::fg_dim())),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  / filter · y copy · PgUp/PgDn page",
            Style::default().fg(theme::fg_dim())),
    ]));

    // Apply popup-scoped filter (set via `/` while popup is open).
    // Lines whose plain text doesn't contain the query are dropped,
    // except section dividers (so the user can still see structure)
    // and blank separators (visual breathing room).
    let q = app.popup_filter.to_lowercase();
    if !q.is_empty() {
        lines.retain(|l| {
            let t = line_text(l).to_lowercase();
            t.contains(&q) || t.contains("─ ") || t.trim().is_empty()
        });
    }

    // Capture section header offsets for n / N navigation.
    let sections: Vec<u16> = lines.iter().enumerate()
        .filter(|(_, l)| line_text(l).contains("─ "))
        .map(|(i, _)| i as u16)
        .collect();
    app.popup_sections = sections;
    // popup_total_lines should match the rendered-row extent the
    // scroll keys act on — pre-2.4.5 this stored lines.len() (logical
    // line count) which made the wheel-to-bottom-re-engages-tail
    // detection in handle_mouse() trigger early on long popups.
    // We compute it again post-render below in `total_rows`; this
    // is set then, not here.

    // Clamp scroll so the user can't keep pressing j past the end
    // (would otherwise leave the popup blank — `End` on the keyboard
    // intentionally jumps to u16::MAX, we resolve it to the last
    // page-fitting offset here so it lands on the last line of
    // content rather than a blank window).
    // Compute the rendered-row count post-wrap, NOT the logical line
    // count.  Paragraph::scroll operates on rendered rows, so the
    // scrollbar must measure the same surface or the thumb position
    // drifts (long cwd / cmd / writing-files paths wrap to 2-3
    // rendered rows; using lines.len() under-reported scroll extent
    // and the thumb appeared stuck near the top).
    let body_width = inner.width.saturating_sub(1).max(1) as usize;
    let total_rows: u16 = lines.iter()
        .map(|l| {
            let len: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if len == 0 { 1u16 }
            else { (len.div_ceil(body_width)).min(u16::MAX as usize) as u16 }
        })
        .sum();
    app.popup_total_lines = total_rows;
    let viewport = inner.height;
    let max_scroll = total_rows.saturating_sub(viewport);
    // Live-tail: when enabled and content overflows, snap to bottom
    // so newly-arrived recent_activity / writing_files entries stay
    // visible.  Disabled the moment the user scrolls up; re-enabled
    // by pressing `G` (or End) or wheeling all the way to bottom.
    if app.detail_tail && max_scroll > 0 {
        app.detail_scroll = max_scroll;
    }
    if app.detail_scroll > max_scroll {
        app.detail_scroll = max_scroll;
    }
    // Persist scroll for this pid so re-opening lands the user where
    // they left off.
    if let Some(pid) = app.selected_pid {
        app.detail_scroll_by_pid.insert(pid, app.detail_scroll);
    }
    let scroll = app.detail_scroll;

    // Render the body inside `inner` minus the rightmost column so
    // the scrollbar has its own track, then overlay the scrollbar
    // itself on the popup's right border so it reads as part of the
    // frame rather than sitting awkwardly inside the body.
    let body_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width.saturating_sub(1),
        height: inner.height,
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        body_area,
    );

    // Themed scrollbar — only render when content actually overflows.
    // ScrollbarState semantics in ratatui-widgets 0.3.x: the thumb
    // ratio is `position / max(content_length - viewport_content_length, 1)`
    // and thumb size is `viewport / content_length` of the track.  So
    // we pass the *full rendered-row count* as content_length and the
    // current top-row as position; the math then matches the actual
    // viewport-over-content fraction the user perceives.
    if max_scroll > 0 {
        // ratatui-widgets 0.3.0 Scrollbar render math:
        //   max_position = content_length - 1
        //   max_viewport_position = max_position + viewport_length
        //   thumb_end = (position + viewport) * track / max_viewport_position
        // For the thumb to land at the *very bottom* when scroll is
        // at end, max_position must equal max_scroll (i.e. content_length
        // = max_scroll + 1, NOT total_rows).  Pre-2.4.6 we passed
        // total_rows here so position=max_scroll only got us
        // ~total/(total+viewport) of the way down — visible "thumb
        // never reaches bottom" effect.
        let mut sbs = ScrollbarState::default()
            .content_length((max_scroll as usize).saturating_add(1))
            .viewport_content_length(viewport as usize)
            .position(scroll as usize);
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(theme::border_dim()))
            .thumb_style(Style::default().fg(theme::border()).add_modifier(Modifier::BOLD));
        // Render onto the inner rect (including the column we kept
        // out of the body) — Scrollbar paints only its right edge.
        f.render_stateful_widget(sb, inner, &mut sbs);
    }

    // Filter bar — pinned to the bottom row of the popup body when
    // the filter is being typed or has a value.  Black-on-accent for
    // legibility against the popup tint.
    if app.popup_filter_typing || !app.popup_filter.is_empty() {
        let bar = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        f.render_widget(ratatui::widgets::Clear, bar);
        let mut spans = vec![
            Span::styled(" / ",
                Style::default().bg(theme::border()).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(app.popup_filter.clone(), Style::default().fg(theme::fg())),
        ];
        if app.popup_filter_typing {
            spans.push(Span::styled("█", Style::default().fg(theme::c_busy())));
        }
        spans.push(Span::styled("  Enter accept · Esc clear",
            Style::default().fg(theme::fg_dim())));
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
    }

    // Live-tail badge — small "TAIL" pill in the popup's top-right
    // when live-tail is engaged AND the content actually overflows
    // the viewport.  Tells the user "I'm auto-following the bottom".
    if app.detail_tail && max_scroll > 0 {
        let badge = " TAIL ";
        let bw = badge.len() as u16;
        if inner.width > bw + 2 {
            let bar = Rect {
                x: inner.x + inner.width - bw - 1,
                y: inner.y,
                width: bw,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Span::styled(badge,
                    Style::default().bg(theme::c_active()).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD))),
                bar,
            );
        }
    }
}

/// Concatenate the visible text of a `Line` for substring search /
/// section-header detection.  ratatui doesn't expose this directly
pub(super) fn line_text(l: &Line<'_>) -> String {
    let mut s = String::new();
    for sp in &l.spans { s.push_str(&sp.content); }
    s
}

/// Hit-test: is `(col, row)` inside `r`?  Used by the mouse handler
pub(super) fn draw_filter_input(f: &mut Frame, area: Rect, filter: &str) {
    let r = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(2),
        width: area.width,
        height: 1,
    };
    let p = Paragraph::new(Line::from(vec![
        Span::styled(" filter: ", Style::default().bg(theme::border()).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(filter.to_string(), Style::default().fg(theme::fg())),
        Span::styled("█", Style::default().fg(theme::c_busy())),
    ]));
    f.render_widget(p, r);
}
pub(super) fn draw_help(f: &mut Frame, area: Rect, app: &mut App) {
    // Help grows with the help text — capped to ~85% of viewport so
    // we always have room for the surrounding chrome.  Scroll handles
    // anything that overflows on small terminals.
    let r = centered_rect(78, 32, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(" Help ", Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(ratatui::widgets::Clear, r);
    f.render_widget(block.clone(), r);
    // Wrap so narrow terminals don't clip the keybinding text.
    let _ = block;

    let line = |spans: Vec<Span<'static>>| Line::from(spans);
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::fg_dim()));
    let hdr = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::fg()).add_modifier(Modifier::BOLD));
    let key = |s: &str| Span::styled(s.to_string(), Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD));

    let lines: Vec<Line> = vec![
        line(vec![hdr("agtop"), dim(&format!("  v{}  — agent monitor", env!("CARGO_PKG_VERSION")))]),
        line(vec![dim(&format!("  prices as of {} ({}) — `--prices PATH` to override",
            crate::pricing::prices_updated(),
            crate::pricing::prices_source()))]),
        Line::raw(""),
        line(vec![key("  q, Ctrl-C   "), dim("quit (closes popup first if open)")]),
        line(vec![key("  ?, h        "), dim("toggle this help")]),
        line(vec![key("  p           "), dim("pause / resume refresh")]),
        line(vec![key("  Enter, Space"), dim("open / close detail popup")]),
        line(vec![key("  r           "), dim("refresh now")]),
        line(vec![key("  s           "), dim("cycle sort (smart / cpu / mem / tokens / uptime / agent)")]),
        line(vec![key("  g           "), dim("toggle project grouping")]),
        line(vec![key("  t           "), dim("toggle tree mode (indented children under each agent)")]),
        line(vec![key("  K (capital) "), dim("SIGTERM the selected agent (confirm y / n)")]),
        line(vec![dim("              lowercase k stays bound to ↑ (vim convention)")]),
        line(vec![key("  C (capital) "), dim("toggle compact rows (hides PID / uptime / chips)")]),
        line(vec![key("  1 – 7       "), dim("toggle individual columns (PID / CPU / MEM / UP / SUB / TOK / ▍)")]),
        line(vec![key("  /, f        "), dim("filter (Ctrl-U clears, Ctrl-W deletes word)")]),
        line(vec![key("  Esc         "), dim("close popup, clear filter")]),
        line(vec![key("  j/k, ↓/↑    "), dim("move selection — also scrolls open popup")]),
        line(vec![key("  PgUp/PgDn   "), dim("move 10 rows — also pages open popup")]),
        line(vec![key("  Home/End    "), dim("first / last agent — jumps in popup")]),
        line(vec![key("  Mouse       "), dim("click row → select; double-click → detail; wheel → scroll")]),
        Line::raw(""),
        line(vec![hdr("  In the detail popup:")]),
        line(vec![key("  j/k, ↓/↑    "), dim("scroll body line by line")]),
        line(vec![key("  PgUp/PgDn   "), dim("page through long content (writing / reading / preview)")]),
        line(vec![key("  g, G        "), dim("jump to top / bottom (G also re-engages live-tail)")]),
        line(vec![key("  Home/End    "), dim("jump to top / bottom")]),
        line(vec![key("  n, N        "), dim("jump to next / previous section header")]),
        line(vec![key("  /           "), dim("filter popup contents (Esc clears, Enter accepts)")]),
        line(vec![key("  y           "), dim("copy `agent / pid / cwd / cmd / session` via OSC 52")]),
        line(vec![key("  Wheel       "), dim("scroll while popup is open")]),
        Line::raw(""),
        line(vec![hdr("  CLI flags worth knowing:")]),
        line(vec![key("  --pid PID   "), dim("open the TUI focused on PID with detail popup showing")]),
        line(vec![key("  --json      "), dim("machine-readable snapshot (implies --once)")]),
        line(vec![key("  --watch     "), dim("one summary line per tick — pipes cleanly")]),
        Line::raw(""),
        line(vec![hdr("  Status legend:")]),
        line(vec![Span::styled("    ● BUSY ", Style::default().fg(theme::c_busy()).add_modifier(Modifier::BOLD)),
                  dim("process active and writing in last 5s")]),
        line(vec![Span::styled("    ● SPWN ", Style::default().fg(theme::c_spawn()).add_modifier(Modifier::BOLD)),
                  dim("Task subagents currently in flight")]),
        line(vec![Span::styled("    ● ACTV ", Style::default().fg(theme::c_active())),
                  dim("process running recently")]),
        line(vec![Span::styled("    ○ IDLE ", Style::default().fg(theme::c_idle()).add_modifier(Modifier::BOLD)),
                  dim("process up but quiet for >60s")]),
        line(vec![Span::styled("    ◌ WAIT ", Style::default().fg(theme::c_wait()).add_modifier(Modifier::BOLD)),
                  dim("no live process, recent session activity")]),
        line(vec![Span::styled("    ✓ DONE ", Style::default().fg(theme::c_done()).add_modifier(Modifier::BOLD)),
                  dim("session ended (stop_reason)")]),
        line(vec![Span::styled("    · STAL ", Style::default().fg(theme::fg_dim()).add_modifier(Modifier::BOLD)),
                  dim("last activity older than 24 h")]),
    ];

    // Help reuses the same scroll surface as the detail popup so the
    // scrollback covers both — `app.detail_scroll` is reset to 0 each
    // time either popup opens, which keeps the two from cross-talking.
    // Wrap-aware row count, same approach as draw_detail.
    let body_width = inner.width.saturating_sub(1).max(1) as usize;
    let total_rows: u16 = lines.iter()
        .map(|l| {
            let len: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if len == 0 { 1u16 }
            else { (len.div_ceil(body_width)).min(u16::MAX as usize) as u16 }
        })
        .sum();
    let viewport = inner.height;
    let max_scroll = total_rows.saturating_sub(viewport);
    if app.detail_scroll > max_scroll {
        app.detail_scroll = max_scroll;
    }
    let scroll = app.detail_scroll;

    let body_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width.saturating_sub(1),
        height: inner.height,
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        body_area,
    );
    if max_scroll > 0 {
        let mut sbs = ScrollbarState::default()
            .content_length((max_scroll as usize).saturating_add(1))
            .viewport_content_length(viewport as usize)
            .position(scroll as usize);
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
