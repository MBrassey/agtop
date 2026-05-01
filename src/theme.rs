// Pluggable color themes.
//
// All colors live on a single `Theme` struct.  The active theme is
// selected once at startup via `set_theme(name)` (driven by the
// `--theme` CLI flag) and stored in a `OnceLock`; every subsequent
// access to `theme::c_busy()`, `theme::fg()`, etc. routes through
// the lock and returns the active palette's value.
//
// Adding a new theme: define a `const` of type `Theme` and add a
// match arm in `set_theme`.  Defaults to `DEFAULT_DARK` if the name
// is unknown or `set_theme` was never called.

use crate::model::Status;
use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // Structural
    pub border:        Color,
    pub border_dim:    Color,
    pub fg:            Color,
    pub fg_dim:        Color,
    pub hl_bg:         Color,
    pub group_tint_a:  Color,
    pub group_tint_b:  Color,
    // Status accents
    pub c_busy:        Color,
    pub c_spawn:       Color,
    pub c_active:      Color,
    pub c_idle:        Color,
    pub c_wait:        Color,
    pub c_done:        Color,
    pub c_stale:       Color,
    // Chart colors
    pub c_chart_cpu:   Color,
    pub c_chart_mem:   Color,
    pub c_chart_tok:   Color,
    // Memory gauge segments
    pub c_gauge_used:  Color,
    pub c_gauge_agent: Color,
    pub c_gauge_free:  Color,
}

// ── Themes ────────────────────────────────────────────────────────────────
const DEFAULT_DARK: Theme = Theme {
    border:        Color::Rgb(125, 150, 165),
    border_dim:    Color::Rgb( 70,  85,  95),
    fg:            Color::Rgb(225, 222, 215),
    fg_dim:        Color::Rgb(165, 170, 178),
    hl_bg:         Color::Rgb( 50,  85, 120),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb( 20,  22,  26),
    c_busy:        Color::Rgb(120, 215, 150),
    c_spawn:       Color::Rgb(140, 215, 185),
    c_active:      Color::Rgb(195, 210, 130),
    c_idle:        Color::Rgb(195, 195, 195),
    c_wait:        Color::Rgb(225, 175, 110),
    c_done:        Color::Rgb(200, 175, 215),
    c_stale:       Color::Rgb(110, 105, 108),
    c_chart_cpu:   Color::Rgb(225, 195, 140),
    c_chart_mem:   Color::Rgb(200, 175, 215),
    c_chart_tok:   Color::Rgb(180, 200, 215),
    c_gauge_used:  Color::Rgb(160, 200, 150),
    c_gauge_agent: Color::Rgb(225, 195, 140),
    c_gauge_free:  Color::Rgb( 60,  65,  75),
};

const DRACULA: Theme = Theme {
    border:        Color::Rgb(189, 147, 249),
    border_dim:    Color::Rgb( 98,  79, 132),
    fg:            Color::Rgb(248, 248, 242),
    fg_dim:        Color::Rgb(150, 150, 165),
    hl_bg:         Color::Rgb( 68,  71,  90),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb( 39,  41,  53),
    c_busy:        Color::Rgb( 80, 250, 123),
    c_spawn:       Color::Rgb(139, 233, 253),
    c_active:      Color::Rgb(241, 250, 140),
    c_idle:        Color::Rgb(200, 200, 200),
    c_wait:        Color::Rgb(255, 184, 108),
    c_done:        Color::Rgb(189, 147, 249),
    c_stale:       Color::Rgb( 98,  79, 132),
    c_chart_cpu:   Color::Rgb(255, 121, 198),
    c_chart_mem:   Color::Rgb(189, 147, 249),
    c_chart_tok:   Color::Rgb(139, 233, 253),
    c_gauge_used:  Color::Rgb( 80, 250, 123),
    c_gauge_agent: Color::Rgb(255, 184, 108),
    c_gauge_free:  Color::Rgb( 50,  53,  72),
};

const NORD: Theme = Theme {
    border:        Color::Rgb(136, 192, 208),
    border_dim:    Color::Rgb( 76, 100, 116),
    fg:            Color::Rgb(236, 239, 244),
    fg_dim:        Color::Rgb(180, 188, 198),
    hl_bg:         Color::Rgb( 76,  86, 106),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb( 46,  52,  64),
    c_busy:        Color::Rgb(163, 190, 140),
    c_spawn:       Color::Rgb(143, 188, 187),
    c_active:      Color::Rgb(235, 203, 139),
    c_idle:        Color::Rgb(216, 222, 233),
    c_wait:        Color::Rgb(208, 135,  112),
    c_done:        Color::Rgb(180, 142, 173),
    c_stale:       Color::Rgb( 76, 100, 116),
    c_chart_cpu:   Color::Rgb(235, 203, 139),
    c_chart_mem:   Color::Rgb(180, 142, 173),
    c_chart_tok:   Color::Rgb(143, 188, 187),
    c_gauge_used:  Color::Rgb(163, 190, 140),
    c_gauge_agent: Color::Rgb(235, 203, 139),
    c_gauge_free:  Color::Rgb( 59,  66,  82),
};

const GRUVBOX: Theme = Theme {
    border:        Color::Rgb(214, 153,  88),
    border_dim:    Color::Rgb(102,  92,  84),
    fg:            Color::Rgb(235, 219, 178),
    fg_dim:        Color::Rgb(168, 153, 132),
    hl_bg:         Color::Rgb( 80,  73,  69),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb( 40,  40,  40),
    c_busy:        Color::Rgb(184, 187,  38),
    c_spawn:       Color::Rgb(142, 192, 124),
    c_active:      Color::Rgb(250, 189,  47),
    c_idle:        Color::Rgb(213, 196, 161),
    c_wait:        Color::Rgb(254, 128,  25),
    c_done:        Color::Rgb(211, 134, 155),
    c_stale:       Color::Rgb(124, 111,  100),
    c_chart_cpu:   Color::Rgb(250, 189,  47),
    c_chart_mem:   Color::Rgb(211, 134, 155),
    c_chart_tok:   Color::Rgb(131, 165, 152),
    c_gauge_used:  Color::Rgb(184, 187,  38),
    c_gauge_agent: Color::Rgb(254, 128,  25),
    c_gauge_free:  Color::Rgb( 60,  56,  54),
};

const MONOCHROME: Theme = Theme {
    border:        Color::Rgb(180, 180, 180),
    border_dim:    Color::Rgb( 90,  90,  90),
    fg:            Color::Rgb(230, 230, 230),
    fg_dim:        Color::Rgb(160, 160, 160),
    hl_bg:         Color::Rgb( 60,  60,  60),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb( 24,  24,  24),
    c_busy:        Color::Rgb(245, 245, 245),
    c_spawn:       Color::Rgb(220, 220, 220),
    c_active:      Color::Rgb(200, 200, 200),
    c_idle:        Color::Rgb(180, 180, 180),
    c_wait:        Color::Rgb(150, 150, 150),
    c_done:        Color::Rgb(120, 120, 120),
    c_stale:       Color::Rgb( 90,  90,  90),
    c_chart_cpu:   Color::Rgb(220, 220, 220),
    c_chart_mem:   Color::Rgb(180, 180, 180),
    c_chart_tok:   Color::Rgb(200, 200, 200),
    c_gauge_used:  Color::Rgb(220, 220, 220),
    c_gauge_agent: Color::Rgb(180, 180, 180),
    c_gauge_free:  Color::Rgb( 50,  50,  50),
};

static THEME: OnceLock<Theme> = OnceLock::new();

/// Install the active theme.  Idempotent — first writer wins
/// (prevents the late `--prices` parser from clobbering an
/// already-installed theme mid-run).  Unknown names fall back to
/// the default-dark palette.
pub fn set_theme(name: &str) {
    let t = match name.to_ascii_lowercase().as_str() {
        "dracula"            => DRACULA,
        "nord"               => NORD,
        "gruvbox"            => GRUVBOX,
        "mono" | "monochrome"=> MONOCHROME,
        _                    => DEFAULT_DARK,
    };
    let _ = THEME.set(t);
}

#[inline]
fn t() -> &'static Theme {
    THEME.get_or_init(|| DEFAULT_DARK)
}

// ── Per-field accessors used everywhere ───────────────────────────────────
#[inline] pub fn border()       -> Color { t().border }
#[inline] pub fn border_dim()   -> Color { t().border_dim }
#[inline] pub fn fg()           -> Color { t().fg }
#[inline] pub fn fg_dim()       -> Color { t().fg_dim }
#[inline] pub fn hl_bg()        -> Color { t().hl_bg }
#[inline] pub fn group_tint_a() -> Color { t().group_tint_a }
#[inline] pub fn group_tint_b() -> Color { t().group_tint_b }
#[inline] pub fn c_busy()       -> Color { t().c_busy }
#[inline] pub fn c_spawn()      -> Color { t().c_spawn }
#[inline] pub fn c_active()     -> Color { t().c_active }
#[inline] pub fn c_idle()       -> Color { t().c_idle }
#[inline] pub fn c_wait()       -> Color { t().c_wait }
#[inline] pub fn c_done()       -> Color { t().c_done }
#[inline] pub fn c_stale()      -> Color { t().c_stale }
#[inline] pub fn c_chart_cpu()  -> Color { t().c_chart_cpu }
#[inline] pub fn c_chart_mem()  -> Color { t().c_chart_mem }
#[inline] pub fn c_chart_tok()  -> Color { t().c_chart_tok }
#[inline] pub fn c_gauge_used() -> Color { t().c_gauge_used }
#[inline] pub fn c_gauge_agent()-> Color { t().c_gauge_agent }
#[inline] pub fn c_gauge_free() -> Color { t().c_gauge_free }

/// Pick the alternating tint for the group at index `i` (0-based).
pub fn group_tint(i: usize) -> Color {
    if i % 2 == 0 { group_tint_a() } else { group_tint_b() }
}

pub fn status_color(s: Status) -> Color {
    match s {
        Status::Busy      => c_busy(),
        Status::Spawning  => c_spawn(),
        Status::Active    => c_active(),
        Status::Idle      => c_idle(),
        Status::Waiting   => c_wait(),
        Status::Completed => c_done(),
        Status::Stale     => c_stale(),
    }
}

pub fn status_style(s: Status) -> Style {
    let st = Style::default().fg(status_color(s));
    if matches!(s, Status::Busy | Status::Spawning) {
        st.add_modifier(Modifier::BOLD)
    } else if matches!(s, Status::Stale) {
        st.add_modifier(Modifier::DIM)
    } else {
        st
    }
}

pub fn agent_color(label: &str) -> Color {
    match label {
        "claude" | "claude-code"   => Color::Rgb(160, 180, 215),
        "codex"  | "openai-codex"  => Color::Rgb(160, 200, 150),
        "aider"                    => Color::Rgb(220, 160, 155),
        "cursor-agent"             => Color::Rgb(200, 170, 210),
        "gemini"                   => Color::Rgb(165, 215, 210),
        "goose" | "block-goose"    => Color::Rgb(225, 195, 140),
        "continue"                 => Color::Rgb(225, 222, 215),
        "opencode"                 => Color::Rgb(200, 170, 210),
        "copilot"                  => Color::Rgb(165, 215, 210),
        "cody"                     => Color::Rgb(200, 170, 210),
        "amp"                      => Color::Rgb(225, 195, 140),
        "crush"                    => Color::Rgb(220, 160, 155),
        "mods"                     => Color::Rgb(160, 200, 150),
        "sgpt"                     => Color::Rgb(160, 180, 215),
        "llm"                      => Color::Rgb(165, 215, 210),
        "ollama"                   => Color::Rgb(225, 195, 140),
        "fabric"                   => Color::Rgb(225, 222, 215),
        _ => {
            let palette = [
                Color::Rgb(165, 215, 210), Color::Rgb(200, 170, 210), Color::Rgb(225, 195, 140),
                Color::Rgb(220, 160, 155), Color::Rgb(160, 200, 150), Color::Rgb(160, 180, 215),
            ];
            let mut h: u32 = 0;
            for b in label.bytes() { h = h.wrapping_mul(31).wrapping_add(b as u32); }
            palette[(h as usize) % palette.len()]
        }
    }
}

pub fn cpu_color(v: f64) -> Color {
    if !v.is_finite() || v < 0.0 { return fg_dim(); }
    if v >= 50.0      { Color::Rgb(220, 160, 155) }
    else if v >= 10.0 { Color::Rgb(235, 180, 110) }
    else if v >=  1.0 { Color::Rgb(160, 200, 150) }
    else              { fg_dim() }
}
