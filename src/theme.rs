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

#[derive(Debug, Clone, Copy, PartialEq)]
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
    // Accents that used to be hardcoded at call sites — themed so the
    // light and no-color palettes stay legible.
    pub c_danger:      Color,
    pub c_cpu_hi:      Color,
    pub c_cpu_mid:     Color,
    pub c_cpu_lo:      Color,
    /// Six identity hues for agent labels; named agents map to fixed
    /// slots, unknown labels hash into them.
    pub agent_palette: [Color; 6],
}

/// Shared identity palette for the dark themes — matches the
/// pre-themed hardcoded agent colors exactly.
const AGENT_PALETTE_DARK: [Color; 6] = [
    Color::Rgb(165, 215, 210), Color::Rgb(200, 170, 210), Color::Rgb(225, 195, 140),
    Color::Rgb(220, 160, 155), Color::Rgb(160, 200, 150), Color::Rgb(160, 180, 215),
];

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
    c_danger:      Color::Rgb(240, 175,  95),
    c_cpu_hi:      Color::Rgb(220, 160, 155),
    c_cpu_mid:     Color::Rgb(235, 180, 110),
    c_cpu_lo:      Color::Rgb(160, 200, 150),
    agent_palette: AGENT_PALETTE_DARK,
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
    c_danger:      Color::Rgb(240, 175,  95),
    c_cpu_hi:      Color::Rgb(220, 160, 155),
    c_cpu_mid:     Color::Rgb(235, 180, 110),
    c_cpu_lo:      Color::Rgb(160, 200, 150),
    agent_palette: AGENT_PALETTE_DARK,
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
    c_danger:      Color::Rgb(240, 175,  95),
    c_cpu_hi:      Color::Rgb(220, 160, 155),
    c_cpu_mid:     Color::Rgb(235, 180, 110),
    c_cpu_lo:      Color::Rgb(160, 200, 150),
    agent_palette: AGENT_PALETTE_DARK,
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
    c_danger:      Color::Rgb(240, 175,  95),
    c_cpu_hi:      Color::Rgb(220, 160, 155),
    c_cpu_mid:     Color::Rgb(235, 180, 110),
    c_cpu_lo:      Color::Rgb(160, 200, 150),
    agent_palette: AGENT_PALETTE_DARK,
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
    // Monochrome keeps its promise now that these route through the
    // theme — pre-theming they leaked the dark-default hues.
    c_danger:      Color::Rgb(200, 200, 200),
    c_cpu_hi:      Color::Rgb(245, 245, 245),
    c_cpu_mid:     Color::Rgb(210, 210, 210),
    c_cpu_lo:      Color::Rgb(170, 170, 170),
    agent_palette: [
        Color::Rgb(235, 235, 235), Color::Rgb(215, 215, 215), Color::Rgb(195, 195, 195),
        Color::Rgb(175, 175, 175), Color::Rgb(205, 205, 205), Color::Rgb(225, 225, 225),
    ],
};

/// Light-background palette: dark-ink foregrounds, pale tints, and
/// status / chart accents darkened for contrast on a white ground.
const LIGHT: Theme = Theme {
    border:        Color::Rgb( 90, 110, 130),
    border_dim:    Color::Rgb(175, 185, 195),
    fg:            Color::Rgb( 40,  44,  52),
    fg_dim:        Color::Rgb(115, 122, 132),
    hl_bg:         Color::Rgb(195, 215, 240),
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Rgb(238, 240, 243),
    c_busy:        Color::Rgb( 25, 125,  65),
    c_spawn:       Color::Rgb( 20, 120, 110),
    c_active:      Color::Rgb(120, 125,  25),
    c_idle:        Color::Rgb(105, 105, 105),
    c_wait:        Color::Rgb(175, 105,  25),
    c_done:        Color::Rgb(120,  80, 150),
    c_stale:       Color::Rgb(160, 158, 160),
    c_chart_cpu:   Color::Rgb(155, 115,  30),
    c_chart_mem:   Color::Rgb(120,  80, 150),
    c_chart_tok:   Color::Rgb( 40,  95, 140),
    c_gauge_used:  Color::Rgb( 75, 135,  65),
    c_gauge_agent: Color::Rgb(155, 115,  30),
    c_gauge_free:  Color::Rgb(210, 215, 222),
    c_danger:      Color::Rgb(175, 110,  20),
    c_cpu_hi:      Color::Rgb(180,  55,  50),
    c_cpu_mid:     Color::Rgb(160, 105,  20),
    c_cpu_lo:      Color::Rgb( 55, 120,  45),
    agent_palette: [
        Color::Rgb( 25, 110, 105), Color::Rgb(110,  70, 130), Color::Rgb(145, 100,  20),
        Color::Rgb(150,  60,  55), Color::Rgb( 55, 110,  45), Color::Rgb( 45,  80, 140),
    ],
};

/// NO_COLOR / --no-color palette — only `Reset` and the two ANSI
/// greys, so nothing richer than the terminal's own defaults is ever
/// emitted (no-color.org).  Weight (BOLD / DIM) still carries the
/// status semantics.
const NOCOLOR: Theme = Theme {
    border:        Color::Reset,
    border_dim:    Color::DarkGray,
    fg:            Color::Reset,
    fg_dim:        Color::DarkGray,
    hl_bg:         Color::DarkGray,
    group_tint_a:  Color::Reset,
    group_tint_b:  Color::Reset,
    c_busy:        Color::Reset,
    c_spawn:       Color::Reset,
    c_active:      Color::Reset,
    c_idle:        Color::Gray,
    c_wait:        Color::Gray,
    c_done:        Color::Gray,
    c_stale:       Color::DarkGray,
    c_chart_cpu:   Color::Gray,
    c_chart_mem:   Color::Gray,
    c_chart_tok:   Color::Gray,
    c_gauge_used:  Color::Gray,
    c_gauge_agent: Color::Reset,
    c_gauge_free:  Color::DarkGray,
    c_danger:      Color::Reset,
    c_cpu_hi:      Color::Reset,
    c_cpu_mid:     Color::Gray,
    c_cpu_lo:      Color::DarkGray,
    agent_palette: [Color::Reset; 6],
};

static THEME: OnceLock<Theme> = OnceLock::new();

/// Resolve a theme name to its palette.  Unknown names fall back to
/// the default-dark palette.
fn theme_by_name(name: &str) -> Theme {
    match name.to_ascii_lowercase().as_str() {
        "dracula"            => DRACULA,
        "nord"               => NORD,
        "gruvbox"            => GRUVBOX,
        "mono" | "monochrome"=> MONOCHROME,
        "light"              => LIGHT,
        // Not offered via --theme; installed by the NO_COLOR /
        // --no-color path in cli::run.
        "no-color"           => NOCOLOR,
        _                    => DEFAULT_DARK,
    }
}

/// Install the active theme.  Idempotent — first writer wins
/// (prevents the late `--prices` parser from clobbering an
/// already-installed theme mid-run).
pub fn set_theme(name: &str) {
    let _ = THEME.set(theme_by_name(name));
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
#[inline] pub fn c_danger()     -> Color { t().c_danger }

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

/// Identity color for an agent label.  Named agents map to fixed
/// slots in the theme's agent palette (so identity hues follow the
/// palette and stay legible on light / no-color grounds); unknown
/// labels hash into the same six slots.
pub fn agent_color(label: &str) -> Color {
    let p = &t().agent_palette;
    match label {
        "claude" | "claude-code"   => p[5],
        "codex"  | "openai-codex"  => p[4],
        "aider"                    => p[3],
        "cursor-agent"             => p[1],
        "gemini"                   => p[0],
        "goose" | "block-goose"    => p[2],
        "continue"                 => fg(),
        "opencode"                 => p[1],
        "copilot"                  => p[0],
        "cody"                     => p[1],
        "amp"                      => p[2],
        "crush"                    => p[3],
        "mods"                     => p[4],
        "sgpt"                     => p[5],
        "llm"                      => p[0],
        "ollama"                   => p[2],
        "fabric"                   => fg(),
        _ => {
            let mut h: u32 = 0;
            for b in label.bytes() { h = h.wrapping_mul(31).wrapping_add(b as u32); }
            p[(h as usize) % p.len()]
        }
    }
}

pub fn cpu_color(v: f64) -> Color {
    if !v.is_finite() || v < 0.0 { return fg_dim(); }
    if v >= 50.0      { t().c_cpu_hi }
    else if v >= 10.0 { t().c_cpu_mid }
    else if v >=  1.0 { t().c_cpu_lo }
    else              { fg_dim() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_by_name_selects_and_falls_back() {
        assert_eq!(theme_by_name("light"), LIGHT);
        assert_eq!(theme_by_name("LIGHT"), LIGHT);
        assert_eq!(theme_by_name("dracula"), DRACULA);
        assert_eq!(theme_by_name("mono"), MONOCHROME);
        assert_eq!(theme_by_name("monochrome"), MONOCHROME);
        assert_eq!(theme_by_name("no-color"), NOCOLOR);
        assert_eq!(theme_by_name("default"), DEFAULT_DARK);
        assert_eq!(theme_by_name("bogus"), DEFAULT_DARK);
    }

    #[test]
    fn nocolor_palette_never_emits_truecolor() {
        let t = NOCOLOR;
        let all = [
            t.border, t.border_dim, t.fg, t.fg_dim, t.hl_bg,
            t.group_tint_a, t.group_tint_b,
            t.c_busy, t.c_spawn, t.c_active, t.c_idle, t.c_wait,
            t.c_done, t.c_stale,
            t.c_chart_cpu, t.c_chart_mem, t.c_chart_tok,
            t.c_gauge_used, t.c_gauge_agent, t.c_gauge_free,
            t.c_danger, t.c_cpu_hi, t.c_cpu_mid, t.c_cpu_lo,
        ];
        for c in all.iter().chain(t.agent_palette.iter()) {
            assert!(
                matches!(c, Color::Reset | Color::Gray | Color::DarkGray),
                "no-color palette leaked {:?}", c
            );
        }
    }

    #[test]
    fn light_palette_uses_dark_ink_foreground() {
        // Sanity guard against a dark fg regressing to a light one —
        // the whole point of the palette is legibility on white.
        match LIGHT.fg {
            Color::Rgb(r, g, b) => assert!(r < 100 && g < 100 && b < 100),
            other => panic!("light fg should be truecolor ink, got {:?}", other),
        }
    }
}
