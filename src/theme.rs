// Refined pastel + greyscale palette: still colorful enough to read state at
// a glance, but soft enough to feel professional rather than neon. Every
// color is RGB so it lands the same on any modern terminal.

use crate::model::Status;
use ratatui::style::{Color, Modifier, Style};

// ── Structural colors ──────────────────────────────────────────────────────
pub const BORDER:     Color = Color::Rgb(125, 150, 165);   // soft slate-cyan
pub const BORDER_DIM: Color = Color::Rgb( 70,  85,  95);   // gentle dim
pub const FG:         Color = Color::Rgb(225, 222, 215);   // warm off-white
pub const FG_DIM:     Color = Color::Rgb(140, 140, 140);   // neutral mid-gray
pub const HL_BG:      Color = Color::Rgb( 40,  45,  55);   // subtle warm dark for selection

// ── Status accents (soft pastel, still distinguishable) ────────────────────
pub const C_BUSY:   Color = Color::Rgb(150, 210, 165);     // pastel sage green
pub const C_SPAWN:  Color = Color::Rgb(165, 215, 210);     // pastel teal
pub const C_ACTIVE: Color = Color::Rgb(160, 200, 150);     // muted sage
pub const C_IDLE:   Color = Color::Rgb(155, 155, 155);     // neutral gray
pub const C_WAIT:   Color = Color::Rgb(225, 195, 140);     // warm peach
pub const C_DONE:   Color = Color::Rgb(200, 175, 215);     // soft lavender
pub const C_STALE:  Color = Color::Rgb(110, 105, 108);     // dim gray

// ── Chart colors ───────────────────────────────────────────────────────────
pub const C_CHART_CPU: Color = Color::Rgb(225, 195, 140);  // warm peach
pub const C_CHART_MEM: Color = Color::Rgb(200, 175, 215);  // soft lavender
pub const C_CHART_TOK: Color = Color::Rgb(180, 200, 215);  // pastel sky for token streams

// ── System memory gauge segments ───────────────────────────────────────────
pub const C_GAUGE_USED:  Color = Color::Rgb(160, 200, 150);  // sage
pub const C_GAUGE_AGENT: Color = Color::Rgb(225, 195, 140);  // peach
pub const C_GAUGE_FREE:  Color = Color::Rgb( 60,  65,  75);  // slate

pub fn status_color(s: Status) -> Color {
    match s {
        Status::Busy      => C_BUSY,
        Status::Spawning  => C_SPAWN,
        Status::Active    => C_ACTIVE,
        Status::Idle      => C_IDLE,
        Status::Waiting   => C_WAIT,
        Status::Completed => C_DONE,
        Status::Stale     => C_STALE,
    }
}

pub fn status_style(s: Status) -> Style {
    let st = Style::default().fg(status_color(s));
    if matches!(s, Status::Busy | Status::Spawning) {
        st.add_modifier(Modifier::BOLD)
    } else if matches!(s, Status::Idle | Status::Stale) {
        st.add_modifier(Modifier::DIM)
    } else {
        st
    }
}

// Per-agent-type accent so rows for the same agent visually cluster.
// Same pastel discipline as the rest of the palette.
pub fn agent_color(label: &str) -> Color {
    match label {
        "claude" | "claude-code"   => Color::Rgb(160, 180, 215),  // soft blue
        "codex"  | "openai-codex"  => Color::Rgb(160, 200, 150),  // sage
        "aider"                    => Color::Rgb(220, 160, 155),  // dusty rose
        "cursor-agent"             => Color::Rgb(200, 170, 210),  // lavender
        "gemini"                   => Color::Rgb(165, 215, 210),  // teal
        "goose" | "block-goose"    => Color::Rgb(225, 195, 140),  // peach
        "continue"                 => Color::Rgb(225, 222, 215),  // off-white
        "opencode"                 => Color::Rgb(200, 170, 210),  // lavender
        "copilot"                  => Color::Rgb(165, 215, 210),  // teal
        "cody"                     => Color::Rgb(200, 170, 210),  // lavender
        "amp"                      => Color::Rgb(225, 195, 140),  // peach
        "crush"                    => Color::Rgb(220, 160, 155),  // dusty rose
        "mods"                     => Color::Rgb(160, 200, 150),  // sage
        "sgpt"                     => Color::Rgb(160, 180, 215),  // soft blue
        "llm"                      => Color::Rgb(165, 215, 210),  // teal
        "ollama"                   => Color::Rgb(225, 195, 140),  // peach
        "fabric"                   => Color::Rgb(225, 222, 215),  // off-white
        _ => {
            // Hash-based stable color from the soft palette.
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
    if v >= 50.0      { Color::Rgb(220, 160, 155) }   // dusty rose
    else if v >= 10.0 { Color::Rgb(225, 195, 140) }   // peach
    else if v >=  1.0 { Color::Rgb(160, 200, 150) }   // sage
    else              { FG_DIM }
}
