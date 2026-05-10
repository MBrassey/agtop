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
use crate::model::Snapshot;
use crate::theme;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::{Paragraph, TableState},
    Frame, Terminal,
};
use std::io::{self, stdout};
use std::time::{Duration, Instant};

mod popup;
mod panels;
mod agents;

// Column-visibility bitmap flags for the agents panel.  Status badge
// and agent label are always shown — every other column has a digit
// shortcut to hide / restore it.  Numeric layout matches the visual
// left-to-right order of the row so `1` is leftmost optional column.
pub(super) const COL_PID:    u32 = 1 << 0;  // 1
pub(super) const COL_CPU:    u32 = 1 << 1;  // 2  — both the % and mini-bar
pub(super) const COL_MEM:    u32 = 1 << 2;  // 3
pub(super) const COL_UPTIME: u32 = 1 << 3;  // 4
pub(super) const COL_SUB:    u32 = 1 << 4;  // 5  — subagent chip
pub(super) const COL_TOK:    u32 = 1 << 5;  // 6  — tokens chip
pub(super) const COL_DANGER: u32 = 1 << 6;  // 7  — dangerous left-edge marker

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum Sort { Smart, Cpu, Mem, Uptime, Tokens, Agent }

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum TokenMode { Cumulative, Fresh }

impl TokenMode {
    /// Resolve the "displayed tokens" value for an agent.
    ///
    ///   Cumulative: tokens_total (= every turn summed, includes
    ///               cache_read re-reads of the same prompt).
    ///   Fresh:      tokens_input - tokens_cache_read + tokens_output
    ///               (saturating).  Approximates "non-cache-hit
    ///               tokens" — what the user paid full input rate
    ///               for, not the inflated total.
    pub(super) fn value(self, a: &crate::model::Agent) -> u64 {
        match self {
            TokenMode::Cumulative => a.tokens_total,
            TokenMode::Fresh => {
                let fresh_in = a.tokens_input.saturating_sub(a.tokens_cache_read);
                fresh_in.saturating_add(a.tokens_output)
            }
        }
    }
    #[allow(dead_code)]
    pub(super) fn label(self) -> &'static str {
        match self { TokenMode::Cumulative => "cumulative", TokenMode::Fresh => "fresh" }
    }
}

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
    pub(super) fn label(self) -> &'static str {
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

pub(super) struct App {
    pub(super) collector: Collector,
    pub(super) snap: Snapshot,
    pub(super) last_tick: Instant,
    pub(super) interval: Duration,
    pub(super) paused: bool,
    pub(super) grouped: bool,
    pub(super) sort: Sort,
    pub(super) filter: String,
    pub(super) typing_filter: bool,
    pub(super) show_help: bool,
    pub(super) show_detail: bool,
    /// Vertical scroll offset (in lines) for the currently-open popup.
    /// Bumped by j/k/↓/↑/PgUp/PgDn while a popup is open.
    pub(super) detail_scroll: u16,
    /// Persistent per-pid scroll memory — when the user re-opens a
    /// popup they had scrolled into, we restore where they were
    /// instead of snapping to the top.  Keyed by pid; pruned when the
    /// pid disappears from the snapshot (handled lazily on lookup).
    pub(super) detail_scroll_by_pid: std::collections::HashMap<u32, u16>,
    /// Live-tail mode for the detail popup's preview block.  When
    /// `detail_scroll` is at the bottom (max), each tick of new
    /// recent_activity bumps the scroll target so the latest event
    /// stays in view.  Toggled implicitly: scrolling up turns it off
    /// (we detect by comparing to max_scroll); pressing End re-pins.
    pub(super) detail_tail: bool,
    /// Popup-scoped substring filter — `/` while a popup is open
    /// hides every line whose visible text doesn't contain the query.
    /// Useful when an agent has 100+ open files and you want just
    /// `*.toml`.
    pub(super) popup_filter: String,
    pub(super) popup_filter_typing: bool,
    /// Line offsets of the major section headers in the most-recently
    /// rendered detail popup.  Powers n/N "jump to next/prev section"
    /// without needing to recompute the line layout in the key
    /// handler.  Empty when popup is closed.
    pub(super) popup_sections: Vec<u16>,
    /// Total rendered line count of the most-recent detail popup —
    /// used by `End` / live-tail to clamp without re-rendering.
    pub(super) popup_total_lines: u16,
    /// When `Some(pid)`, a SIGTERM-confirmation popup is open for
    /// that pid; `y` confirms (sends SIGTERM via libc::kill), `n`
    /// or `Esc` cancels.  `K` opens the popup for the selected row.
    pub(super) confirm_kill: Option<u32>,
    /// Tree view: when on, each row in the agents panel is followed
    /// by indented sub-rows for its immediate child processes
    /// (hooks, MCP servers, shell commands).  Toggled with `t`.
    pub(super) tree_mode: bool,
    pub(super) selected_pid: Option<u32>,
    pub(super) visible_pid_order: Vec<u32>,
    /// `(row_y, pid)` of every clickable agent row in the agents panel,
    /// captured during the previous draw so mouse clicks can be hit-tested
    /// without rerendering.
    pub(super) clickable_rows: Vec<(u16, u32)>,
    /// Persistent ratatui table state for the agents panel — drives
    /// auto-scroll when the selection moves below the viewport.  We
    /// keep our own inline row highlighting (colour + glyph), so
    /// `highlight_style` stays unset; this state exists purely for
    /// the scroll-into-view behaviour.
    pub(super) agents_state: TableState,
    /// Scroll offset for the Claude-sessions panel; bumped by the
    /// shift-arrow / shift-wheel combo so the user can leaf through
    /// recent tasks without the panel hogging its small viewport.
    pub(super) sessions_scroll: u16,
    /// Last-rendered rect for the sessions panel — used by the
    /// mouse handler to route wheel events into the panel when the
    /// pointer is over it (instead of the default selection-scroll).
    pub(super) sessions_rect: Rect,
    /// Total row count of the most-recent draw of the agents panel
    /// — used by the panel's scrollbar so it can size the thumb.
    pub(super) agents_total_rows: usize,
    /// Compact-row toggle — hides the PID, uptime, subagent and
    /// token chips so DOING gets the rest of the row.  Useful on
    /// narrow terminals or when you only care about activity, not
    /// metrics.  Bound to `C` (capital).
    pub(super) compact_rows: bool,
    /// Per-column visibility bitmap for the agents panel.  Bits
    /// match `Col` enum below; bound to digit keys `1`–`7`.  When
    /// `compact_rows` is on this is ignored (compact has its own
    /// fixed rule of "everything optional → off").
    pub(super) cols: u32,
    /// Token metric used everywhere `tokens_total` is displayed
    /// or sorted on.  CLI flag `--tokens cumulative|fresh`.
    pub(super) tokens_mode: TokenMode,
    pub(super) quit: bool,
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

    let pre_pid = args.pid;
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
        show_detail: pre_pid.is_some(),
        detail_scroll: 0,
        detail_scroll_by_pid: std::collections::HashMap::new(),
        detail_tail: true,
        popup_filter: String::new(),
        popup_filter_typing: false,
        popup_sections: Vec::new(),
        popup_total_lines: 0,
        confirm_kill: None,
        tree_mode: false,
        selected_pid: pre_pid,
        visible_pid_order: Vec::new(),
        clickable_rows: Vec::new(),
        agents_state: TableState::default(),
        sessions_scroll: 0,
        sessions_rect: Rect::default(),
        agents_total_rows: 0,
        compact_rows: false,
        cols: COL_PID | COL_CPU | COL_MEM | COL_UPTIME | COL_SUB | COL_TOK | COL_DANGER,
        tokens_mode: match args.tokens.as_str() {
            "fresh" => TokenMode::Fresh,
            _       => TokenMode::Cumulative,
        },
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

        terminal.draw(|f| draw(f, app)).map_err(|e| anyhow::anyhow!("ratatui draw failed: {e}"))?;

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
    // crossterm on Windows fires KeyEventKind::Press AND ::Release for
    // every keystroke (POSIX terminals only fire Press).  Without this
    // filter every Space / Enter toggled the detail popup open on Press
    // and immediately closed it on Release — looked like the popup
    // flashed and disappeared.  Drop everything except Press so the
    // handler runs exactly once per keystroke on every platform.
    if key.kind != KeyEventKind::Press { return; }
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
    if app.confirm_kill.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(pid) = app.confirm_kill.take() {
                    // Re-validate the pid still maps to a known
                    // agent in the *current* snapshot before signal
                    // delivery.  Defends against pid recycling
                    // between popup-open and key-press: if the
                    // original process exited and the kernel
                    // reassigned that pid to an unrelated process,
                    // we'd otherwise SIGTERM the wrong target.
                    let still_present = app.snap.agents.iter().any(|a| a.pid == pid);
                    if still_present {
                        send_sigterm(pid);
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.confirm_kill = None;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
            _ => {}
        }
        return;
    }
    if app.show_detail || app.show_help {
        // Popup-scoped filter input mode — the popup-filter prompt is
        // modal: typing edits the filter, Esc clears, Enter accepts.
        if app.popup_filter_typing {
            match key.code {
                KeyCode::Esc => {
                    app.popup_filter_typing = false;
                    app.popup_filter.clear();
                }
                KeyCode::Enter => app.popup_filter_typing = false,
                KeyCode::Backspace => { app.popup_filter.pop(); }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.popup_filter.clear(),
                KeyCode::Char(c) if !c.is_control() && app.popup_filter.len() < 256 => {
                    app.popup_filter.push(c);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q')
            | KeyCode::Enter | KeyCode::Char(' ') => {
                // Save scroll position before closing so re-opening
                // the same agent restores it.
                if app.show_detail {
                    if let Some(pid) = app.selected_pid {
                        app.detail_scroll_by_pid.insert(pid, app.detail_scroll);
                    }
                }
                app.show_detail = false;
                app.show_help = false;
                app.detail_scroll = 0;
                app.popup_filter.clear();
            }
            KeyCode::Char('?') => app.show_help = !app.show_help,
            // Both popups scroll with j/k/↓/↑/PgUp/PgDn so long
            // content (skills lists, writing/reading files, recent-
            // activity transcripts, the help legend) doesn't get
            // truncated on small terminals.
            KeyCode::Down | KeyCode::Char('j') => {
                app.detail_scroll = app.detail_scroll.saturating_add(1);
                app.detail_tail = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
                app.detail_tail = false;
            }
            KeyCode::PageDown => {
                app.detail_scroll = app.detail_scroll.saturating_add(10);
                app.detail_tail = false;
            }
            KeyCode::PageUp => {
                app.detail_scroll = app.detail_scroll.saturating_sub(10);
                app.detail_tail = false;
            }
            // g = top, G = bottom (vim convention).  Inside a popup
            // these don't conflict with the global `g` (group toggle)
            // — the popup-gated handler intercepts before the global
            // match runs.  G also re-pins live-tail.
            KeyCode::Char('g') => {
                app.detail_scroll = 0;
                app.detail_tail = false;
            }
            KeyCode::Char('G') | KeyCode::End => {
                app.detail_scroll = u16::MAX;
                app.detail_tail = true;
            }
            KeyCode::Home => {
                app.detail_scroll = 0;
                app.detail_tail = false;
            }
            // n / N — jump to next / previous section header.  Section
            // offsets were captured during the last draw.  Wraps at
            // the end so 'n' on the last section returns to the top.
            KeyCode::Char('n') if app.show_detail => {
                let pos = app.detail_scroll;
                let next = app.popup_sections.iter().copied()
                    .find(|&s| s > pos)
                    .or_else(|| app.popup_sections.first().copied());
                if let Some(s) = next {
                    app.detail_scroll = s;
                    app.detail_tail = false;
                }
            }
            KeyCode::Char('N') if app.show_detail => {
                let pos = app.detail_scroll;
                let prev = app.popup_sections.iter().rev().copied()
                    .find(|&s| s < pos)
                    .or_else(|| app.popup_sections.last().copied());
                if let Some(s) = prev {
                    app.detail_scroll = s;
                    app.detail_tail = false;
                }
            }
            // / — start popup-scoped filter typing.  Matches the
            // global filter convention (Esc clears, Enter accepts).
            KeyCode::Char('/') if app.show_detail => {
                app.popup_filter_typing = true;
                app.popup_filter.clear();
            }
            // y — copy a one-line agent identity snippet to the
            // clipboard via the OSC 52 escape sequence (works in
            // tmux, kitty, iTerm2, Wezterm, foot, modern xterm).  No
            // dependency cost — we already own stdout.  The flash on
            // the footer hint tells the user it landed.
            KeyCode::Char('y') if app.show_detail => {
                if let Some(pid) = app.selected_pid {
                    if let Some(a) = app.snap.agents.iter().find(|a| a.pid == pid) {
                        let payload = format!(
                            "agent={} pid={} cwd={} cmd={} session={}",
                            a.label, a.pid, a.cwd, a.cmdline,
                            a.session_id.as_deref().unwrap_or("-"),
                        );
                        copy_via_osc52(&payload);
                    }
                }
            }
            // 'h' closes the help popup when it's open; in the detail
            // popup it's a no-op.
            KeyCode::Char('h') if app.show_help => app.show_help = !app.show_help,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('?') | KeyCode::Char('h') => app.show_help = true,
        // Both Enter AND Space open the detail popup (and Space also
        // closes it when open — see the popup-gated branch above).
        // Pause keeps `p` only.
        KeyCode::Enter | KeyCode::Char(' ') => {
            app.show_detail = true;
            app.detail_scroll = app.selected_pid
                .and_then(|p| app.detail_scroll_by_pid.get(&p).copied())
                .unwrap_or(0);
            app.popup_filter.clear();
            // Open scrolled to TOP, not bottom.  Pre-2.4.8 we
            // pre-enabled live-tail on every popup open which made
            // draw_detail snap scroll → max_scroll on the first
            // frame; the user landed at the bottom of the content
            // before they'd read the model / tokens / cost rows
            // at the top.  Live-tail re-engages when the user
            // explicitly presses G / End or wheels to bottom.
            app.detail_tail = false;
        }
        KeyCode::Char('p') => app.paused = !app.paused,
        KeyCode::Char('r') => {
            app.snap = app.collector.snapshot();
            app.last_tick = Instant::now();
        }
        KeyCode::Char('s') => app.sort = app.sort.cycle(),
        KeyCode::Char('g') => app.grouped = !app.grouped,
        KeyCode::Char('t') => app.tree_mode = !app.tree_mode,
        // Capital C toggles compact rows; lowercase c is reserved
        // for Ctrl-C above.
        KeyCode::Char('C') => app.compact_rows = !app.compact_rows,
        // Per-column toggles.  Mirrored against the COL_* bitmap so
        // the on/off state is queryable from anywhere that needs it
        // (footer, help text, agent_row).  Compact mode overrides
        // these — switch compact off first to use them again.
        KeyCode::Char('1') => app.cols ^= COL_PID,
        KeyCode::Char('2') => app.cols ^= COL_CPU,
        KeyCode::Char('3') => app.cols ^= COL_MEM,
        KeyCode::Char('4') => app.cols ^= COL_UPTIME,
        KeyCode::Char('5') => app.cols ^= COL_SUB,
        KeyCode::Char('6') => app.cols ^= COL_TOK,
        KeyCode::Char('7') => app.cols ^= COL_DANGER,
        // Capital K opens the SIGTERM-confirmation dialog for the
        // currently-selected agent.  Lowercase k stays bound to
        // upward navigation (vim convention).
        KeyCode::Char('K') => {
            if let Some(pid) = app.selected_pid {
                app.confirm_kill = Some(pid);
            }
        }
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

/// Copy `payload` to the system clipboard using the OSC 52 terminal
/// escape sequence.  Works without any clipboard library — modern
/// terminals (tmux, kitty, iTerm2, Wezterm, foot, xterm with
/// allowWindowOps) intercept the sequence and stuff it into the
/// system clipboard.  Silent no-op in legacy / locked-down terminals
/// (no error path needed; clipboard ops are opportunistic).
fn copy_via_osc52(payload: &str) {
    use std::io::Write;
    // Hand-rolled base64 (RFC 4648 standard alphabet).  Avoids
    // pulling in `base64` for one call site.
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = payload.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((v >> 18) & 0x3F) as usize] as char);
        out.push(A[((v >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { A[((v >> 6) & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(v & 0x3F) as usize] as char } else { '=' });
    }
    // OSC 52: ESC ] 52 ; c ; <base64> BEL.  `c` selects the system
    // clipboard (vs. `p` primary on X11).
    let mut so = stdout();
    let _ = write!(so, "\x1b]52;c;{}\x07", out);
    let _ = so.flush();
}

/// Send SIGTERM to a pid.  Best-effort: returns silently on EPERM
/// (process not ours), ESRCH (process gone), and any other error
/// — the user sees the row disappear (or not) on the next tick.
#[cfg(unix)]
fn send_sigterm(pid: u32) {
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
}
#[cfg(windows)]
fn send_sigterm(pid: u32) {
    // Windows equivalent: OpenProcess(PROCESS_TERMINATE) +
    // TerminateProcess.  Best-effort: silently no-op if the process
    // is gone, the user lacks permissions, or the kernel refuses.
    // Exit code 1 mirrors what most Windows tooling uses for "killed
    // by user".
    unsafe {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
        use windows_sys::Win32::Foundation::CloseHandle;
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
}
#[cfg(not(any(unix, windows)))]
fn send_sigterm(_pid: u32) { /* no-op on truly exotic targets */ }

fn handle_mouse(app: &mut App, m: MouseEvent) {
    // Wheel routing: when either popup is open, the wheel scrolls
    // the popup body instead of the underlying agent list — otherwise
    // scrolling-to-read inside the popup would also rotate the
    // selection underneath, which is jarring.
    if app.show_detail || app.show_help {
        match m.kind {
            MouseEventKind::ScrollUp => {
                app.detail_scroll = app.detail_scroll.saturating_sub(3);
                app.detail_tail = false;
            }
            MouseEventKind::ScrollDown => {
                app.detail_scroll = app.detail_scroll.saturating_add(3);
                // If user wheels to bottom, opt back into live-tail.
                if app.detail_scroll >= app.popup_total_lines {
                    app.detail_tail = true;
                }
            }
            _ => {}
        }
        return;
    }
    // Wheel over the sessions panel: scroll its body, not the agent
    // selection.  Hit-tested against the panel's last-rendered rect.
    if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown)
        && rect_contains(app.sessions_rect, m.column, m.row)
    {
        match m.kind {
            MouseEventKind::ScrollUp   => app.sessions_scroll = app.sessions_scroll.saturating_sub(2),
            MouseEventKind::ScrollDown => app.sessions_scroll = app.sessions_scroll.saturating_add(2),
            _ => {}
        }
        return;
    }
    match m.kind {
        // Click an agent row in the agents panel: select it.  Double-click
        // (handled here as click-on-selected) opens the detail popup.
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((_, pid)) = app.clickable_rows.iter().find(|(y, _)| *y == m.row) {
                if app.selected_pid == Some(*pid) {
                    app.show_detail = true;
                    app.detail_scroll = app.detail_scroll_by_pid.get(pid).copied().unwrap_or(0);
                    app.popup_filter.clear();
                    // Open at top — see Enter/Space handler for the
                    // rationale on detail_tail = false at open.
                    app.detail_tail = false;
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
        )).style(Style::default().fg(theme::fg_dim()));
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

    panels::draw_header(f, chunks[0], &app.snap, app);

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

    agents::draw_agents(f, left[0], app);
    panels::draw_left_bottom(f, left[1], &app.snap);
    panels::draw_cpu_panel(f, right[0], &app.snap);
    panels::draw_memory_panel(f, right[1], &app.snap);
    panels::draw_tokens_panel(f, right[2], &app.snap, app.interval);
    panels::draw_status_distribution(f, right[3], &app.snap);
    app.sessions_rect = right[4];
    panels::draw_sessions(f, right[4], &app.snap, &mut app.sessions_scroll);

    panels::draw_footer(f, chunks[2], app);

    if let Some(pid) = app.confirm_kill {
        popup::draw_confirm_kill(f, area, &app.snap, pid);
    } else if app.show_help {
        popup::draw_help(f, area, app);
    } else if app.show_detail {
        popup::draw_detail(f, area, app);
    } else if app.typing_filter {
        popup::draw_filter_input(f, area, &app.filter);
    }
}

/// SIGTERM-confirmation popup.  Shows the target row in full so the
/// user double-checks before killing the wrong agent.  Centered,
/// 60×8 fixed.
/// because `Line` is generic over span styling; we just walk spans.
/// to route wheel events to whichever panel is under the cursor.
fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x.saturating_add(r.width)
        && row >= r.y && row < r.y.saturating_add(r.height)
}

/// Center a `(width × height)` `Rect` inside `area`, with a margin
/// guard so the result never overflows.  All popup chrome (detail,
/// help, kill confirm) goes through this so window-sizing edge cases
/// are handled in one place rather than copied per call site.
pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

