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
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
            MouseButton, MouseEvent, MouseEventKind},
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
use std::time::Duration;

mod popup;
mod panels;
mod agents;
mod game;

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

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Sort { Smart, Cpu, Mem, Uptime, Tokens, Cost, Agent }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
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
    pub(super) fn label(self) -> &'static str {
        match self { TokenMode::Cumulative => "cumulative", TokenMode::Fresh => "fresh" }
    }
    pub(super) fn toggled(self) -> Self {
        match self { TokenMode::Cumulative => TokenMode::Fresh, TokenMode::Fresh => TokenMode::Cumulative }
    }
}

impl Sort {
    fn cycle(self) -> Self {
        match self {
            Sort::Smart  => Sort::Cpu,
            Sort::Cpu    => Sort::Mem,
            Sort::Mem    => Sort::Tokens,
            Sort::Tokens => Sort::Cost,
            Sort::Cost   => Sort::Uptime,
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
            Sort::Cost   => "cost",
            Sort::Uptime => "uptime",
            Sort::Agent  => "agent",
        }
    }
    /// Direction glyph for the header / footer sort indicator.
    /// `desc = true` is the natural orientation — numeric keys read
    /// high→low, agent reads A→Z; `S` flips whatever is active.
    pub(super) fn arrow(self, desc: bool) -> &'static str {
        let up = (self == Sort::Agent) == desc;
        if up { "▲" } else { "▼" }
    }
}

pub(super) struct App {
    /// Latest snapshot, delivered by the collector thread over a channel.
    pub(super) snap: Snapshot,
    /// Clone of the (immutable) price table — the collector now lives on its
    /// own thread, so the UI keeps its own copy for the cache-savings stat
    /// instead of reaching into the collector.
    pub(super) pricing: crate::pricing::PriceTable,
    /// Control channel to the collector thread (refresh-now / pause / quit).
    pub(super) ctrl_tx: std::sync::mpsc::Sender<CollectorCmd>,
    pub(super) interval: Duration,
    pub(super) paused: bool,
    pub(super) grouped: bool,
    pub(super) sort: Sort,
    /// Sort direction — `true` is the natural per-key orientation
    /// (numeric high→low, agent A→Z); `S` flips it.
    pub(super) sort_desc: bool,
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
    /// Hidden side-scrolling dodger easter egg.  Toggled with `` ` ``.
    /// `None` = inactive (zero overhead); `Some` = game replaces the
    /// bottom-right panel slot and intercepts SPACE / `b` / `Z` /
    /// `` ` `` / Esc.  All other keys still drive normal agtop behaviour.
    /// Visibility intentionally narrower than peers — `GameState`
    /// itself only escapes to `crate::ui` (private_interfaces lint).
    pub(in crate::ui) game: Option<game::GameState>,
    pub(super) quit: bool,
}

/// Best-effort restore of the terminal to a sane state: leave the mouse
/// reporting modes, show the cursor (ratatui hides it on every draw and the
/// alt-screen restore does NOT re-show it), leave bracketed paste and the
/// alternate screen, and drop raw mode.  Safe to call more than once.
fn restore_terminal() {
    let _ = disable_raw_mode();
    // Bracketed paste goes as its own best-effort command: on legacy
    // Windows consoles (no VT) crossterm reports it Unsupported, and
    // inside the chain below that error would short-circuit cursor::Show
    // and LeaveAlternateScreen — stranding the user on the alt buffer.
    let _ = execute!(stdout(), DisableBracketedPaste);
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        crossterm::cursor::Show,
        LeaveAlternateScreen,
    );
}

/// RAII guard for terminal state.  Whatever ends `run` — an error `?` during
/// init, an early return from the loop, a normal quit, or an unwind — `Drop`
/// restores the terminal exactly once, so there is no path that can leave the
/// user stuck in raw mode / the alt screen with a hidden cursor.
struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        // If entering the alt screen fails, undo the full partial init
        // (raw mode AND whatever commands in the chain already took
        // effect, e.g. the alt screen itself) before bailing.
        if let Err(e) = execute!(stdout(), EnterAlternateScreen, EnableMouseCapture) {
            restore_terminal();
            return Err(e.into());
        }
        // Best-effort: legacy Windows consoles without VT support report
        // bracketed paste as Unsupported — that must degrade (pastes
        // arrive as key events) rather than abort startup.
        let _ = execute!(stdout(), EnableBracketedPaste);
        Ok(TerminalGuard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Control messages from the UI thread to the collector thread.
pub(super) enum CollectorCmd {
    /// Produce a snapshot immediately (the `r` key).
    RefreshNow,
    /// Stop / resume producing snapshots (the `p` key).
    SetPaused(bool),
    /// Exit the collector loop.
    Quit,
}

/// Spawn the collector on a dedicated thread.  It owns the `Collector` and
/// free-runs at `interval`, sending each `Snapshot` over `snap_tx`; the UI
/// thread reads the latest without ever blocking on collection.  This is what
/// keeps a slow or wedged snapshot (nvidia-smi on a hung GPU, wsl.exe on a
/// stuck LxssManager, a stalled network read, a Windows handle-table sweep)
/// from freezing input and rendering — the UI just keeps showing the last good
/// frame until a fresh one arrives.  Same on Linux, macOS, and Windows:
/// std threads + mpsc, no platform-specific machinery.
fn spawn_collector(
    mut collector: Collector,
    interval: Duration,
    snap_tx: std::sync::mpsc::Sender<Snapshot>,
    ctrl_rx: std::sync::mpsc::Receiver<CollectorCmd>,
) -> std::thread::JoinHandle<()> {
    use std::sync::mpsc::RecvTimeoutError;
    std::thread::Builder::new()
        .name("agtop-collector".into())
        .spawn(move || {
            let mut paused = false;
            // Emit an initial snapshot right away so the UI has a first frame.
            let mut produce = true;
            loop {
                if produce {
                    let snap = collector.snapshot();
                    if snap_tx.send(snap).is_err() {
                        break; // UI gone
                    }
                    produce = false;
                }
                // When paused, only a control message can wake us; otherwise
                // wake on the tick interval to produce the next snapshot.
                let recv = if paused {
                    collector_recv_blocking(&ctrl_rx)
                } else {
                    ctrl_rx.recv_timeout(interval)
                };
                match recv {
                    Ok(CollectorCmd::Quit) => break,
                    Ok(CollectorCmd::RefreshNow) => produce = true,
                    Ok(CollectorCmd::SetPaused(p)) => {
                        paused = p;
                        if !paused {
                            produce = true; // resume the cadence immediately
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => produce = true, // normal tick
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn agtop-collector thread")
}

/// Block on the control channel, mapping a closed channel to the same
/// `Disconnected` variant `recv_timeout` uses so the caller can match one type.
fn collector_recv_blocking(
    rx: &std::sync::mpsc::Receiver<CollectorCmd>,
) -> Result<CollectorCmd, std::sync::mpsc::RecvTimeoutError> {
    rx.recv().map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
}

pub fn run(collector: Collector, args: Args) -> Result<()> {
    // Unix: restore the terminal on SIGTERM / SIGHUP / SIGINT (delivered at
    // logout, on `kill`, or when a wedged agtop is killed externally) before
    // the process dies with default disposition — otherwise the shell is left
    // in raw mode spewing escapes.  Must run before raw mode is enabled so the
    // saved termios is the cooked one.
    #[cfg(unix)]
    signal_restore::install();

    // Install a panic hook that restores terminal state *before* the panic
    // message prints, so a crash doesn't dump a backtrace into raw mode / the
    // alt screen with a hidden cursor.  The RAII guard below covers the
    // non-panic exit paths.  Restore ONLY for a panic on this (UI) thread:
    // a background thread's panic (collector, reader/watchdog helpers) must
    // not drop the live UI into cooked mode mid-render — the UI notices a
    // dead collector via the closed snapshot channel and reports it instead.
    let ui_thread = std::thread::current().id();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            restore_terminal();
        }
        prev(info);
    }));

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let interval = Duration::from_millis((args.interval.max(0.1) * 1000.0) as u64);
    let initial_sort = match args.sort.as_str() {
        "cpu" => Sort::Cpu,
        "mem" => Sort::Mem,
        "tokens" => Sort::Tokens,
        "cost" => Sort::Cost,
        "uptime" => Sort::Uptime,
        "agent" => Sort::Agent,
        _ => Sort::Smart,
    };

    // Take a copy of the price table, then hand the collector to its own
    // thread.  From here the UI never touches the collector directly.
    let pricing = collector.pricing().clone();
    let (snap_tx, snap_rx) = std::sync::mpsc::channel::<Snapshot>();
    let (ctrl_tx, ctrl_rx) = std::sync::mpsc::channel::<CollectorCmd>();
    let _collector_thread = spawn_collector(collector, interval, snap_tx, ctrl_rx);

    let pre_pid = args.pid;
    let mut app = App {
        snap: Snapshot::default(),
        pricing,
        ctrl_tx,
        interval,
        paused: false,
        grouped: true,
        sort: initial_sort,
        sort_desc: args.sort_desc,
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
        compact_rows: args.compact,
        cols: COL_PID | COL_CPU | COL_MEM | COL_UPTIME | COL_SUB | COL_TOK | COL_DANGER,
        tokens_mode: match args.tokens.as_str() {
            "fresh" => TokenMode::Fresh,
            _       => TokenMode::Cumulative,
        },
        game: None,
        quit: false,
    };

    let res = main_loop(&mut terminal, &mut app, &snap_rx);

    // Tell the collector to stop, but do NOT join — if it's mid-snapshot in a
    // wedged external call we must not block the UI's exit / terminal restore
    // on it.  The `run_capped` timeouts bound any single call, and the thread
    // exits on its own once it sees the control channel closed; if the process
    // exits first, the detached thread dies with it.
    let _ = app.ctrl_tx.send(CollectorCmd::Quit);

    // `_guard` restores the terminal on drop (including on the `?` paths and a
    // panic), so there is no manual teardown here — a teardown that chained
    // `?` could skip a step and leave the terminal half-restored.
    res
}

/// SIGTERM/SIGHUP/SIGINT terminal restore (Unix).  A signal that kills the
/// process with default disposition never runs Rust destructors, so without
/// this the terminal would be left in raw mode + alt screen.  The handler
/// restores the saved (cooked) termios and writes the alt-screen/cursor/mouse
/// reset escapes with a single async-signal-safe `write`, then re-raises the
/// signal with default disposition so the exit status reflects it.
#[cfg(unix)]
mod signal_restore {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    // Saved cooked-mode termios, captured before raw mode is enabled.  Only
    // written once (in `install`, before any signal can fire) and only read in
    // the handler, so the single-threaded-init access is race-free in practice.
    static mut SAVED: Option<libc::termios> = None;

    // Leave any-motion/drag/button/SGR mouse modes, leave bracketed paste,
    // show the cursor, then leave the alternate screen.
    const RESET: &[u8] =
        b"\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?1049l";

    extern "C" fn handle(sig: libc::c_int) {
        unsafe {
            if let Some(t) = SAVED {
                libc::tcsetattr(libc::STDOUT_FILENO, libc::TCSANOW, &t);
            }
            let _ = libc::write(
                libc::STDOUT_FILENO,
                RESET.as_ptr() as *const libc::c_void,
                RESET.len(),
            );
            // Restore default disposition and re-raise so we actually die with
            // the signal's semantics rather than looping.
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    pub fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDOUT_FILENO, &mut t) == 0 {
                SAVED = Some(t);
            }
            let h = handle as extern "C" fn(libc::c_int) as libc::sighandler_t;
            for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
                libc::signal(sig, h);
            }
        }
    }
}

fn main_loop<B: ratatui::backend::Backend + io::Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    snap_rx: &std::sync::mpsc::Receiver<Snapshot>,
) -> Result<()> {
    // Only redraw when something actually changed — a fresh snapshot, a
    // handled input event, a resize, or an animating game.  Previously the
    // widget tree was rebuilt ~10×/s regardless, and any event (including a
    // stream of mouse-motion reports) forced an extra full redraw, so idly
    // hovering the cursor pegged a core.
    let mut dirty = true;
    // Set once the snapshot channel disconnects — a collector thread that
    // panicked or exited.  Without this the UI would free-run on stale data
    // forever with no indication anything died.
    let mut collector_dead = false;
    while !app.quit {
        // Drain any snapshots the collector thread produced since the last
        // iteration and keep only the newest.  Collection happens on that
        // thread now, so nothing here can block on a slow/wedged snapshot —
        // the UI just keeps showing the last good frame until a new one lands.
        loop {
            use std::sync::mpsc::TryRecvError;
            match snap_rx.try_recv() {
                Ok(snap) => { app.snap = snap; dirty = true; }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !collector_dead {
                        collector_dead = true;
                        app.snap.note = Some(
                            "collector thread died — data frozen, press q to quit".into());
                        dirty = true;
                    }
                    break;
                }
            }
        }

        // Easter-egg game tick (no-op when inactive).  When active it
        // animates every frame, so keep redrawing.
        if let Some(g) = app.game.as_mut() {
            g.tick(&app.snap);
            dirty = true;
        }

        if dirty {
            terminal.draw(|f| draw(f, app))
                .map_err(|e| anyhow::anyhow!("ratatui draw failed: {e}"))?;
            dirty = false;
        }

        // Wake to (a) service input and (b) pick up newly-arrived snapshots.
        // A game frame needs ~20fps; otherwise a modest cadence bounds how
        // long a fresh snapshot waits before it's displayed (imperceptible)
        // while keeping idle CPU near zero.
        let poll_to = if app.game.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };

        if event::poll(poll_to)? {
            // Drain everything already queued before redrawing — a paste or
            // a mouse-motion burst is handled in one batch, one redraw.
            loop {
                match event::read()? {
                    Event::Key(key) => { handle_key(app, key); dirty = true; }
                    Event::Mouse(m) => {
                        // Motion events change nothing on their own; don't
                        // mark the frame dirty for them.
                        if !matches!(m.kind, MouseEventKind::Moved) {
                            dirty = true;
                        }
                        handle_mouse(app, m);
                    }
                    Event::Paste(text) => { handle_paste(app, &text); dirty = true; }
                    Event::Resize(_, _) => { dirty = true; }
                    _ => {}
                }
                if app.quit { break; }
                if !event::poll(Duration::ZERO)? { break; }
            }
        }
    }
    Ok(())
}

/// Handle a bracketed-paste event.  Routes the pasted text into whichever
/// prompt is open (the agents filter or the popup filter), capped, so a large
/// paste arrives as one event instead of one key event per character (which
/// used to trigger a full redraw per character — a multi-second freeze).
fn handle_paste(app: &mut App, text: &str) {
    const FILTER_MAX: usize = 256;
    let sink = if app.typing_filter {
        Some(&mut app.filter)
    } else if (app.show_detail || app.show_help) && app.popup_filter_typing {
        Some(&mut app.popup_filter)
    } else {
        None
    };
    if let Some(buf) = sink {
        for c in text.chars() {
            if c.is_control() { continue; }
            if buf.len() >= FILTER_MAX { break; }
            buf.push(c);
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // crossterm on Windows fires KeyEventKind::Press AND ::Release for
    // every keystroke (POSIX terminals only fire Press).  Without this
    // filter every Space / Enter toggled the detail popup open on Press
    // and immediately closed it on Release — looked like the popup
    // flashed and disappeared.  Drop everything except Press so the
    // handler runs exactly once per keystroke on every platform.
    if key.kind != KeyEventKind::Press { return; }

    // Easter-egg game key interception.  Only runs when the game is
    // active and no other modal owns the foreground; SPACE / b / ` /
    // Esc go to the game, everything else falls through so the
    // normal agtop keys (q, /, j/k, etc.) still work.
    if app.game.is_some()
        && !app.typing_filter
        && !app.show_help
        && !app.show_detail
        && app.confirm_kill.is_none()
    {
        let game_intercepts = matches!(key.code,
            KeyCode::Char(' ') | KeyCode::Char('b') | KeyCode::Char('Z')
            | KeyCode::Char('`') | KeyCode::Esc
        );
        if game_intercepts {
            if let Some(g) = app.game.as_mut() {
                match g.handle_key(key) {
                    game::KeyDispatch::CloseGame => app.game = None,
                    game::KeyDispatch::Handled => {}
                }
            }
            return;
        }
    }
    // Toggle ON: backtick with no popup / modal active.  When game is
    // already active the interception block above handles toggle-off.
    if app.game.is_none()
        && key.code == KeyCode::Char('`')
        && !app.typing_filter
        && !app.show_help
        && !app.show_detail
        && app.confirm_kill.is_none()
    {
        app.game = Some(game::GameState::new());
        return;
    }
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
                    deliver_kill(app, pid, false);
                }
            }
            // `9` escalates to SIGKILL (kill -9) for agents that
            // ignore SIGTERM — wedged in an FFI call, or a stuck
            // child holding the process group.  Same identity
            // re-checks as `y`; the target gets no cleanup chance.
            KeyCode::Char('9') => {
                if let Some(pid) = app.confirm_kill.take() {
                    deliver_kill(app, pid, true);
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
        KeyCode::Enter | KeyCode::Char(' ') if app.selected_pid.is_some() => {
            // Only open the detail popup when an agent is actually selected —
            // otherwise `draw_detail` bails early and the invisible modal
            // silently swallows keys (j/k stop moving, the first `q` only
            // "closes" the nothing).
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
        KeyCode::Char('p') => {
            app.paused = !app.paused;
            let _ = app.ctrl_tx.send(CollectorCmd::SetPaused(app.paused));
        }
        KeyCode::Char('r') => {
            // Ask the collector thread for a fresh snapshot; it arrives on the
            // channel and is picked up by main_loop's drain.
            let _ = app.ctrl_tx.send(CollectorCmd::RefreshNow);
        }
        KeyCode::Char('s') => app.sort = app.sort.cycle(),
        // Capital S reverses the sort direction — same key family as
        // the lowercase cycle so the pair is discoverable.
        KeyCode::Char('S') => app.sort_desc = !app.sort_desc,
        // x flips the token metric live (cumulative ↔ fresh) — same
        // semantics as the --tokens startup flag.  Every consumer
        // reads app.tokens_mode, so the header chip, table chip,
        // and token sort all follow on the next frame.
        KeyCode::Char('x') => app.tokens_mode = app.tokens_mode.toggled(),
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
                // While paused the snapshot can be arbitrarily stale;
                // ask for a fresh one so the confirm popup (and the
                // re-validation on `y`) run against current data.  The
                // collector honours RefreshNow even when paused.
                if app.paused {
                    let _ = app.ctrl_tx.send(CollectorCmd::RefreshNow);
                }
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

/// Shared delivery path for the kill-confirm dialog (`y` = SIGTERM,
/// `9` = SIGKILL).  Both escalation levels run the same two-layer
/// identity re-check, so a recycled pid is refused rather than
/// signaled regardless of which key confirmed.
fn deliver_kill(app: &App, pid: u32, hard: bool) {
    // Re-validate the pid still maps to a known agent in the
    // *current* snapshot before signal delivery.  Defends against
    // pid recycling between popup-open and key-press: if the
    // original process exited and the kernel reassigned that pid
    // to an unrelated process, we'd otherwise signal the wrong
    // target.
    let agent = app.snap.agents.iter().find(|a| a.pid == pid);
    if let Some(a) = agent {
        if let Some(distro) = a.host.strip_prefix("wsl:") {
            // WSL-hosted: the signal has to be delivered *inside*
            // the guest, so we shell out via wsl.exe.  Linux PID
            // is the lower 24 bits of the namespaced u32.
            #[cfg(windows)]
            send_signal_wsl(distro, a.display_pid(), &a.exe, hard);
            #[cfg(not(windows))]
            { let _ = distro; }
        } else if pid_identity_matches(a, app.snap.now) {
            // Second layer: the snapshot row can itself be stale
            // (paused UI, recycled pid) — confirm the live
            // process's start time still matches what the snapshot
            // recorded before signaling.
            if hard { send_sigkill(pid); } else { send_sigterm(pid); }
        }
    }
}

/// Verify the live process behind `pid` is still the one the snapshot
/// described, by comparing start times: the snapshot's `uptime_sec`
/// pins when the recorded process began, and /proc/<pid>/stat gives
/// the current occupant's start.  A recycled pid started later, so a
/// mismatch (beyond rounding slack) means "different process — don't
/// signal".  Fail-open only when /proc bookkeeping is unavailable
/// (unreadable boot time); fail-closed when the pid has vanished.
#[cfg(target_os = "linux")]
fn pid_identity_matches(a: &crate::model::Agent, snap_now_ms: u64) -> bool {
    let boot = crate::proc_::read_boot_time();
    if boot == 0 || snap_now_ms == 0 { return true; } // can't verify
    let stat = match crate::proc_::read_stat(a.pid) {
        Some(s) => s,
        None => return false, // process already gone
    };
    let started_now  = boot.saturating_add(stat.starttime / crate::proc_::CLK_TCK);
    let started_snap = (snap_now_ms / 1000).saturating_sub(a.uptime_sec);
    // ±2 s absorbs the second-granularity rounding on both sides.
    started_now.abs_diff(started_snap) <= 2
}
/// Non-Linux: no /proc start-time oracle; the snapshot-presence check
/// in the caller is the only guard available.
#[cfg(not(target_os = "linux"))]
fn pid_identity_matches(_a: &crate::model::Agent, _snap_now_ms: u64) -> bool { true }

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

/// Hard-kill escalation.  SIGKILL can't be caught or ignored, so a
/// target wedged in an FFI call or blocked on a dead child dies
/// anyway.  Same best-effort semantics as `send_sigterm`.
#[cfg(unix)]
fn send_sigkill(pid: u32) {
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL); }
}
/// Windows has no soft/hard distinction — TerminateProcess is
/// already the unconditional kill, so escalation aliases it.
#[cfg(windows)]
fn send_sigkill(pid: u32) { send_sigterm(pid); }
#[cfg(not(any(unix, windows)))]
fn send_sigkill(_pid: u32) { /* no-op on truly exotic targets */ }

/// Deliver SIGTERM (or SIGKILL when `hard`) to a PID *inside* a WSL2
/// distro.  Windows-only —
/// we can't reach into the guest kernel from the Win32 process API,
/// so we shell out via wsl.exe.  Runs on a detached thread with a
/// deadline: wsl.exe against a stuck LxssManager can block for
/// minutes, and this is called from the key handler on the UI thread.
///
/// The guest-side script re-checks the target's identity (its
/// /proc/<pid>/exe against the exe the snapshot recorded) before
/// signaling, so a pid recycled inside the guest is refused rather
/// than killed.  Delivery is tried as the distro's default user
/// first; `-u root` only as a fallback on a permission failure —
/// never after an identity mismatch.  Failure is silent: the row
/// either disappears on the next tick or it doesn't.
#[cfg(windows)]
fn send_signal_wsl(distro: &str, linux_pid: u32, expected_exe: &str, hard: bool) {
    let distro = distro.to_string();
    let exe = expected_exe.to_string();
    std::thread::spawn(move || {
        // Exit 0 always; the verdict rides on stdout so run_capped's
        // "success only" contract still surfaces it: M = identity
        // mismatch, K = killed, E = kill failed (likely EPERM).
        const SCRIPT: &str = r#"pid="$1"; want="$2"; sig="$3"
have=$(readlink "/proc/$pid/exe" 2>/dev/null)
if [ -n "$want" ] && [ -n "$have" ] && [ "$have" != "$want" ]; then echo M; exit 0; fi
if kill "-$sig" "$pid" 2>/dev/null; then echo K; else echo E; fi"#;
        let pid_s = linux_pid.to_string();
        let sig = if hard { "KILL" } else { "TERM" };
        let run = |as_root: bool| -> Option<Vec<u8>> {
            let mut cmd = std::process::Command::new("wsl.exe");
            cmd.arg("-d").arg(&distro);
            if as_root { cmd.args(["-u", "root"]); }
            cmd.args(["--exec", "/bin/sh", "-c", SCRIPT, "sh", &pid_s, &exe, sig]);
            crate::collector::run_capped(cmd, std::time::Duration::from_millis(5000))
        };
        if let Some(out) = run(false) {
            if out.starts_with(b"E") {
                // Default user lacked permission (agent owned by another
                // guest user); root retry re-runs the identity check too.
                let _ = run(true);
            }
        }
    });
}

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

/// Width below which the right chart column is dropped so the agents
/// table gets the full terminal width (a stock 80×24 otherwise clips
/// the row before the DOING cell renders).
pub(super) const NARROW_W: u16 = 100;
/// Width below which agent rows are forced compact — the fixed
/// columns alone would consume nearly the whole row before DOING.
pub(super) const COMPACT_W: u16 = 80;

/// Body layout tiers, decided fresh every frame from terminal size.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum BodyLayout {
    /// Below the render floor — a resize hint replaces the UI.
    Tiny,
    /// 60–99 cols: right chart column dropped, agents full-width,
    /// projects/activity keep the bottom strip.
    Stacked,
    /// Two-column layout with the right chart stack.
    Full,
}

pub(super) fn body_layout(width: u16, height: u16) -> BodyLayout {
    if height < 16 || width < 60 { BodyLayout::Tiny }
    else if width < NARROW_W     { BodyLayout::Stacked }
    else                         { BodyLayout::Full }
}

/// Modal overlay stack — identical across every body layout, so the
/// draw paths share it.
fn draw_popups(f: &mut Frame, area: Rect, app: &mut App) {
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

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let layout = body_layout(area.width, area.height);
    // Tiny-terminal guard: below the layout's minimum we can't render the
    // panel stack without truncation. Show a single instructional line
    // instead of a broken half-rendered TUI.
    if layout == BodyLayout::Tiny {
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

    // Full-screen game: the body area belongs entirely to the dodger.
    // Header (live agent chips — the game's input signals) and footer
    // (key hints) stay so the user can see what's driving the
    // mappings and how to back out.
    if app.game.as_ref().map(|g| g.fullscreen).unwrap_or(false) {
        // Wipe sessions_rect so wheel events don't try to scroll a
        // panel that isn't drawn this frame.
        app.sessions_rect = Rect::default();
        // Reset agents bookkeeping (no agents table this frame).
        app.clickable_rows.clear();
        app.visible_pid_order.clear();
        app.agents_total_rows = 0;
        if let Some(g) = app.game.as_mut() {
            g.draw(f, chunks[1]);
        }
        panels::draw_footer(f, chunks[2], app);
        draw_popups(f, area, app);
        return;
    }

    // Narrow terminal: no room for the right chart column, so the
    // agents table takes the full width (DOING survives on a stock
    // 80×24) and projects/activity keep the bottom strip.  Charts
    // come back the moment the terminal is widened — pure per-frame
    // width decision, no state.
    if layout == BodyLayout::Stacked {
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(10)])
            .split(chunks[1]);
        // Sessions panel isn't drawn this frame — wipe its cached
        // rect so wheel events don't scroll a phantom panel.
        app.sessions_rect = Rect::default();
        agents::draw_agents(f, left[0], app);
        if app.game.is_some() {
            // No right-column slot to borrow in this layout — the
            // dodger takes the bottom strip instead.
            if let Some(g) = app.game.as_mut() {
                g.draw(f, left[1]);
            }
        } else {
            panels::draw_left_bottom(f, left[1], &app.snap);
        }
        panels::draw_footer(f, chunks[2], app);
        draw_popups(f, area, app);
        return;
    }

    // Body: left | right; left has agents on top + projects/activity on bottom.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(chunks[1]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(10)])
        .split(body[0]);

    // Right column.  When the game is active we merge the bottom two
    // slots (status distribution + sessions) into one taller panel
    // so the dodger has playable headroom — six inner rows is the
    // floor; merging gives ≥12 reliably.
    let right_constraints: &[Constraint] = if app.game.is_some() {
        &[
            Constraint::Length(10),    // CPU
            Constraint::Length(10),    // Memory
            Constraint::Length(8),     // Tokens
            Constraint::Min(14),       // Game (merged status + sessions slot)
        ]
    } else {
        &[
            Constraint::Length(10),    // CPU panel: sparkline + per-agent bars
            Constraint::Length(10),    // Memory by agent + system gauge
            Constraint::Length(8),     // Tokens panel: rate sparkline + per-agent
            Constraint::Length(8),     // Status distribution bars
            Constraint::Min(6),        // Claude sessions panel
        ]
    };
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints(right_constraints)
        .split(body[1]);

    agents::draw_agents(f, left[0], app);
    panels::draw_left_bottom(f, left[1], &app.snap);
    panels::draw_cpu_panel(f, right[0], &app.snap);
    panels::draw_memory_panel(f, right[1], &app.snap);
    panels::draw_tokens_panel(f, right[2], &app.snap, app.interval);
    // Easter-egg: when the game is active, it owns the bottom of the
    // right column (replacing both status distribution + sessions).
    // Toggle back with `` ` `` to restore them.
    if app.game.is_some() {
        // Wipe the cached sessions rect so wheel events over this
        // area don't try to scroll a panel that isn't being drawn.
        // Write the rect before taking &mut on game to keep the two
        // disjoint field borrows trivially in sequence.
        app.sessions_rect = Rect::default();
        if let Some(g) = app.game.as_mut() {
            g.draw(f, right[3]);
        }
    } else {
        panels::draw_status_distribution(f, right[3], &app.snap);
        app.sessions_rect = right[4];
        panels::draw_sessions(f, right[4], &app.snap, &mut app.sessions_scroll);
    }

    panels::draw_footer(f, chunks[2], app);

    draw_popups(f, area, app);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;

    #[test]
    fn sort_cycle_visits_every_key_once() {
        let mut s = Sort::Smart;
        let mut seen = vec![s];
        loop {
            s = s.cycle();
            if s == Sort::Smart { break; }
            assert!(!seen.contains(&s), "cycle revisited {:?}", s);
            seen.push(s);
        }
        // All seven keys, cost included, before wrapping to smart.
        assert_eq!(seen.len(), 7);
        assert!(seen.contains(&Sort::Cost));
    }

    #[test]
    fn sort_arrow_flips_with_direction() {
        assert_eq!(Sort::Cpu.arrow(true), "▼");
        assert_eq!(Sort::Cpu.arrow(false), "▲");
        // Agent's natural direction is ascending (A→Z).
        assert_eq!(Sort::Agent.arrow(true), "▲");
        assert_eq!(Sort::Agent.arrow(false), "▼");
    }

    #[test]
    fn tokens_mode_value_math() {
        let a = Agent {
            tokens_total: 100,
            tokens_input: 80,
            tokens_output: 15,
            tokens_cache_read: 60,
            ..Agent::default()
        };
        assert_eq!(TokenMode::Cumulative.value(&a), 100);
        assert_eq!(TokenMode::Fresh.value(&a), 80 - 60 + 15);
        assert_eq!(TokenMode::Cumulative.toggled(), TokenMode::Fresh);
        assert_eq!(TokenMode::Fresh.toggled(), TokenMode::Cumulative);
    }

    #[test]
    fn tokens_mode_fresh_saturates() {
        // cache_read can exceed the input bucket on a malformed
        // transcript — must clamp to output, not wrap.
        let a = Agent {
            tokens_input: 10,
            tokens_cache_read: 50,
            tokens_output: 5,
            ..Agent::default()
        };
        assert_eq!(TokenMode::Fresh.value(&a), 5);
    }

    #[test]
    fn body_layout_breakpoints() {
        assert_eq!(body_layout(59, 40), BodyLayout::Tiny);
        assert_eq!(body_layout(200, 15), BodyLayout::Tiny);
        assert_eq!(body_layout(60, 16), BodyLayout::Stacked);
        assert_eq!(body_layout(NARROW_W - 1, 24), BodyLayout::Stacked);
        assert_eq!(body_layout(NARROW_W, 24), BodyLayout::Full);
        assert_eq!(body_layout(220, 60), BodyLayout::Full);
    }
}

