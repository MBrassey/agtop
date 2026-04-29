// Color palette + per-agent accent colors. Centralised so the TUI keeps one
// coherent visual identity.

use crate::model::Status;
use ratatui::style::{Color, Modifier, Style};

// Borders, headings, accents — a calm cyan/teal palette with bright accents
// for state changes that the eye actually needs to catch.
pub const BORDER: Color = Color::Rgb(89, 178, 215);
pub const BORDER_DIM: Color = Color::Rgb(42, 90, 110);
pub const FG: Color = Color::Rgb(220, 224, 232);
pub const FG_DIM: Color = Color::Rgb(120, 130, 145);
pub const HL_BG: Color = Color::Rgb(30, 60, 80);

// Status accents.
pub const C_BUSY: Color = Color::Rgb(80, 220, 100);
pub const C_SPAWN: Color = Color::Rgb(120, 220, 220);
pub const C_ACTIVE: Color = Color::Rgb(80, 200, 120);
pub const C_IDLE: Color = Color::Rgb(140, 145, 155);
pub const C_WAIT: Color = Color::Rgb(240, 200, 90);
pub const C_DONE: Color = Color::Rgb(200, 130, 220);
pub const C_STALE: Color = Color::Rgb(110, 110, 120);

// Chart colors — distinct hue for each series.
pub const C_CHART_CPU: Color = Color::Rgb(255, 200, 80);
pub const C_CHART_MEM: Color = Color::Rgb(220, 130, 220);
pub const C_CHART_ACTIVE: Color = Color::Rgb(110, 220, 130);
pub const C_CHART_BUSY: Color = Color::Rgb(255, 110, 110);

pub fn status_color(s: Status) -> Color {
    match s {
        Status::Busy => C_BUSY,
        Status::Spawning => C_SPAWN,
        Status::Active => C_ACTIVE,
        Status::Idle => C_IDLE,
        Status::Waiting => C_WAIT,
        Status::Completed => C_DONE,
        Status::Stale => C_STALE,
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
pub fn agent_color(label: &str) -> Color {
    match label {
        "claude" | "claude-code" => Color::Rgb(120, 170, 255),
        "codex" | "openai-codex" => Color::Rgb(110, 220, 130),
        "aider" => Color::Rgb(240, 130, 130),
        "cursor-agent" => Color::Rgb(220, 150, 240),
        "gemini" => Color::Rgb(120, 220, 220),
        "goose" => Color::Rgb(240, 200, 90),
        "continue" => Color::Rgb(220, 224, 232),
        "opencode" => Color::Rgb(220, 130, 220),
        "copilot" => Color::Rgb(120, 220, 220),
        "cody" => Color::Rgb(220, 130, 220),
        "amp" => Color::Rgb(240, 200, 90),
        "crush" => Color::Rgb(240, 130, 130),
        "mods" => Color::Rgb(110, 220, 130),
        "sgpt" => Color::Rgb(120, 170, 255),
        "llm" => Color::Rgb(120, 220, 220),
        "ollama" => Color::Rgb(240, 200, 90),
        "fabric" => Color::Rgb(220, 224, 232),
        "block-goose" => Color::Rgb(240, 200, 90),
        _ => {
            // Hash-based stable color from the palette.
            let palette = [
                Color::Rgb(120, 220, 220), Color::Rgb(220, 150, 240), Color::Rgb(240, 200, 90),
                Color::Rgb(240, 130, 130), Color::Rgb(110, 220, 130), Color::Rgb(120, 170, 255),
            ];
            let mut h: u32 = 0;
            for b in label.bytes() { h = h.wrapping_mul(31).wrapping_add(b as u32); }
            palette[(h as usize) % palette.len()]
        }
    }
}

pub fn cpu_color(v: f64) -> Color {
    if v >= 50.0 { Color::Rgb(255, 110, 110) }
    else if v >= 10.0 { Color::Rgb(255, 200, 80) }
    else if v >= 1.0 { Color::Rgb(110, 220, 130) }
    else { FG_DIM }
}
