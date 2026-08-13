// Human-friendly formatters used by both the TUI and the --once / --json paths.

pub fn bytes(n: u64) -> String {
    if n == 0 {
        return "0B".into();
    }
    let units = ["B", "K", "M", "G", "T", "P"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    // Classify against the *rounded* value: 99.96 formats as "100.0M"
    // under `{:.1}` (6 chars), overflowing the 5-wide MEM column.  The
    // 99.95 cut sends it to the no-decimal branch ("100M", 4 chars).
    if v >= 99.95 {
        format!("{:.0}{}", v, units[i])
    } else {
        format!("{:.1}{}", v, units[i])
    }
}

pub fn pct(v: f64) -> String {
    format!("{:.1}%", v)
}

pub fn dur(sec: u64) -> String {
    // Defensive cap — if /proc gives us a value near u64::MAX (rare clock
    // glitch) the resulting "X days" string would explode column width.
    if sec > 10 * 365 * 86_400 { return "10y+".into(); }
    if sec < 60 {
        return format!("{}s", sec);
    }
    if sec < 3600 {
        let m = sec / 60;
        let s = sec % 60;
        return format!("{}m{:02}s", m, s);
    }
    if sec < 86400 {
        let h = sec / 3600;
        let m = (sec % 3600) / 60;
        return format!("{}h{:02}m", h, m);
    }
    let d = sec / 86400;
    let h = (sec % 86400) / 3600;
    // Past 99 days, "{d}d{h}h" (e.g. "3650d23h", 8 chars) overflows the
    // 6-wide uptime column; drop the hours so the widest value ("3650d",
    // 5 chars, at the 10-year cap) still fits.
    if d >= 100 {
        format!("{}d", d)
    } else {
        format!("{}d{}h", d, h)
    }
}

/// SI-suffixed integer formatter for token counts and similar.
/// 1234 → "1.2k", 1_234_567 → "1.2M".
pub fn si(n: u64) -> String {
    // Thresholds are set on the *rounded* boundary so e.g. 999_999 doesn't
    // format as "1000.0k" (7 chars) and overflow the 6-wide token column —
    // it rolls up to "1.0M" instead.  Same for the k→M→B steps.
    if n < 1_000 { return n.to_string(); }
    if n < 999_950 { return format!("{:.1}k", n as f64 / 1_000.0); }
    if n < 999_950_000 { return format!("{:.1}M", n as f64 / 1_000_000.0); }
    if n < 999_950_000_000 { return format!("{:.1}B", n as f64 / 1_000_000_000.0); }
    // Trillion tier keeps the width ≤6 for any realistic token aggregate
    // (a session or fleet never approaches 10^15 tokens).
    format!("{:.1}T", n as f64 / 1_000_000_000_000.0)
}

/// 8-cell unicode sparkline ▁▂▃▄▅▆▇█ for an arbitrary slice of values.
/// `max` should be a stable upper bound (e.g. observed peak) so sparklines
/// across rows are visually comparable.  `width` is the number of glyphs.
pub fn sparkline(values: &[f64], max: f64, width: usize) -> String {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = if max <= 0.0 { 1.0 } else { max };
    let n = values.len();
    if n == 0 { return " ".repeat(width); }
    let step = (n as f64 / width as f64).max(1.0);
    let mut out = String::with_capacity(width);
    for i in 0..width {
        let start = (i as f64 * step) as usize;
        let end_raw = ((i + 1) as f64 * step) as usize;
        let end = end_raw.min(n);
        if start >= end {
            out.push(BLOCKS[0]);
            continue;
        }
        let avg: f64 = values[start..end].iter().copied().sum::<f64>() / (end - start) as f64;
        let idx = ((avg.max(0.0) / max) * (BLOCKS.len() - 1) as f64).round() as usize;
        out.push(BLOCKS[idx.min(BLOCKS.len() - 1)]);
    }
    out
}

/// Strip C0/C1 control bytes (everything <0x20 except `\t`, plus 0x7f, plus
/// the OSC introducer `\x1b]…(BEL|ST)`).  Used to sanitise session-derived
/// strings (assistant prose, tool subjects, recent-task text) before they
/// hit stdout in --once / --json or get passed to ratatui — a malicious or
/// corrupted JSONL transcript can otherwise hijack the cursor / clipboard /
/// title via embedded ANSI sequences.
pub fn sanitize_control(s: &str) -> String {
    /// Hard cap on how far we'll scan for a CSI/OSC terminator.  A
    /// pathological transcript with a bare `ESC[` followed by 1 MiB
    /// of digits and no terminator would otherwise let those bytes
    /// fall through to the output (they look like parameter chars,
    /// not the final byte).  256 is well above the longest legitimate
    /// CSI/OSC sequence (~80 chars for a real terminal-control op).
    const MAX_ESC_RUN: usize = 256;
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\t' => out.push(' '),
            '\x1b' => {
                // Eat any CSI / OSC parameter run + final byte / terminator.
                let intro = chars.peek().copied();
                let is_csi_or_osc = matches!(intro, Some('[' | ']' | '(' | ')' | 'P' | '_' | '^'));
                if is_csi_or_osc {
                    chars.next();
                    let mut consumed = 0usize;
                    let mut terminated = false;
                    for c2 in chars.by_ref() {
                        consumed += 1;
                        if c2 == '\x07' || c2 == '\x1b' || c2.is_ascii_alphabetic() || c2 == '\\' {
                            terminated = true;
                            break;
                        }
                        if consumed >= MAX_ESC_RUN {
                            // Bail out — drop everything that followed.
                            // Already consumed the parameter bytes silently.
                            break;
                        }
                    }
                    let _ = terminated;
                } else if intro.is_some() {
                    // Two-byte ESC sequence (like ESC + 'D') — drop both.
                    chars.next();
                }
                // Bare trailing ESC at end of string — already consumed,
                // nothing emitted.
            }
            c if (c as u32) < 0x20 || c == '\x7f' => { /* drop */ }
            // C1 controls 0x80..=0x9f are dropped.
            c if (c as u32) >= 0x80 && (c as u32) <= 0x9f => { /* drop */ }
            // Unicode bidi/format controls (Cf): zero-width space/joiners,
            // the LRE/RLE/PDF/LRO/RLO overrides, the LRI/RLI/FSI/PDI
            // isolates, and the BOM/ZWNBSP.  A monitoring tool must not let
            // an attacker-controlled cmdline or path visually reorder text —
            // U+202E (RTL override) can hide `--dangerously-skip-permissions`
            // from the "GOD" flag or make `edualc` render as `claude`.
            // ratatui's own control-char filter is C0/C1/DEL only, so this
            // is the sole defence for both the TUI and the --once printer.
            c if matches!(c as u32,
                0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFEFF)
                => { /* drop */ }
            c => out.push(c),
        }
    }
    out
}

pub fn shorten(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Just the trailing path component of a cwd, or empty if the cwd doesn't
/// look like a real project directory.  Path-aware so it works on Windows
/// (`C:\Users\u\code\proj` → `proj`) too.  Use `derive_project` for the
/// caller-friendly name that always returns *something* meaningful.
pub fn project_basename(cwd: &str) -> String {
    let p = std::path::Path::new(cwd);
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        // Fallback: the manual trim-trailing-sep logic, for paths sysinfo
        // hands us with a trailing slash.
        .unwrap_or_else(|| {
            let trimmed = cwd.trim_end_matches(&['/', '\\'][..]);
            trimmed.rsplit(['/', '\\'])
                .find(|s| !s.is_empty()).unwrap_or("").to_string()
        })
}

/// Caller-friendly project label.  Falls back through cwd-basename →
/// `<label> <subcommand>` (for daemons like `ollama serve`) → exe basename
/// → label so we never display a bare `?`.  Path-aware: works for POSIX,
/// Windows (`C:\Users\u\code\proj` → `proj`), and root-like cwds.  Result
/// is always run through `sanitize_control` so a malicious cwd or argv
/// can't smuggle ANSI escapes into the row label.
pub fn derive_project(cwd: &str, exe: &str, cmdline: &str, label: &str) -> String {
    sanitize_control(&derive_project_raw(cwd, exe, cmdline, label))
}

fn derive_project_raw(cwd: &str, exe: &str, cmdline: &str, label: &str) -> String {
    // Primary: cwd basename, if the cwd actually identifies a project.
    let cwd_basename = project_basename(cwd);
    let cwd_path = std::path::Path::new(cwd);
    let is_root = cwd_path.parent().is_none()    // unix root, win drive root
        || cwd == "/" || cwd == "\\";
    // `/proc` is a Linux convention — but checking it on every platform
    // is harmless (no other OS uses it) and keeps the unit tests
    // passing identically across linux / macOS / Windows runners.
    let cwd_bad = cwd_basename.is_empty()
        || is_root
        || cwd.starts_with("/proc")
        || cwd_basename == "tmp"
        || cwd_basename == "?";
    if !cwd_bad {
        return cwd_basename;
    }
    // Secondary: <label> + first non-flag token of cmdline (e.g. "ollama serve").
    let mut tokens = cmdline.split_whitespace();
    let _bin = tokens.next();
    for t in tokens {
        if t.starts_with('-') {
            continue;
        }
        let trimmed: String = t.chars().take(20).collect();
        return format!("{} {}", label, trimmed);
    }
    // Tertiary: exe basename, if it's not just a copy of the label.
    // Windows exes use backslash separators and `.exe` suffix.
    let exe_trimmed = exe.trim_end_matches(&['/', '\\'][..]);
    let exe_basename = exe_trimmed.rsplit(['/', '\\'])
        .find(|s| !s.is_empty()).unwrap_or("")
        .trim_end_matches(".exe");
    if !exe_basename.is_empty() && exe_basename != label && exe_basename != "?" {
        return exe_basename.to_string();
    }
    // Final: just the label — at least it identifies the agent kind.
    label.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_falls_back_when_cwd_is_root() {
        // ollama serve: cwd is /, exe is /usr/bin/ollama, cmdline has the subcommand
        let p = derive_project("/", "/usr/bin/ollama", "/usr/bin/ollama serve", "ollama");
        assert_eq!(p, "ollama serve");
    }

    #[test]
    fn project_uses_cwd_basename_when_present() {
        let p = derive_project("/home/user/code/zk-rollup-prover", "/usr/bin/claude",
                               "claude --resume", "claude");
        assert_eq!(p, "zk-rollup-prover");
    }

    #[test]
    fn project_falls_back_to_label_for_bare_invocation() {
        let p = derive_project("/proc/self", "/usr/bin/claude", "claude", "claude");
        assert_eq!(p, "claude");
    }

    #[test]
    fn sparkline_handles_empty_and_uniform() {
        assert_eq!(sparkline(&[], 100.0, 8), "        ");
        let s = sparkline(&[50.0, 50.0, 50.0, 50.0, 50.0, 50.0, 50.0, 50.0], 100.0, 8);
        assert_eq!(s.chars().count(), 8);
        // 50/100 → mid-block.
        for c in s.chars() {
            assert!("▁▂▃▄▅".contains(c), "got {}", c);
        }
    }

    #[test]
    fn bytes_is_human_friendly() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(512), "512B");   // <100? actually 512 → 512B
        assert_eq!(bytes(1024), "1.0K");
        assert_eq!(bytes(1024 * 1024), "1.0M");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.0G");
    }

    #[test]
    fn formatters_never_exceed_column_widths() {
        // MEM column is {:>5}; a value rounding into [99.95,100) must not
        // render as the 6-char "100.0M".
        let almost_100m = (100.0 * 1024.0 * 1024.0 - 1.0) as u64;
        assert!(bytes(almost_100m).chars().count() <= 5, "{}", bytes(almost_100m));
        // TOK column is {:>6}; 999_999 must roll up rather than "1000.0k".
        assert!(si(999_999).chars().count() <= 6, "{}", si(999_999));
        assert_eq!(si(999_999), "1.0M");
        // Realistic upper bounds (a fleet aggregate stays well under 10^14).
        assert!(si(999_999_999_999).chars().count() <= 6, "{}", si(999_999_999_999));
        assert!(si(99_999_999_999_999).chars().count() <= 6, "{}", si(99_999_999_999_999));
        // uptime column is {:>6}; a multi-year process must not blow it out.
        assert!(dur(3650 * 86_400 - 1).chars().count() <= 6, "{}", dur(3650 * 86_400 - 1));
        assert!(dur(200 * 86_400).chars().count() <= 6, "{}", dur(200 * 86_400));
    }

    #[test]
    fn sanitize_strips_bidi_and_format_controls() {
        // U+202E (RTL override) used to smuggle a reversed/hidden flag past
        // the display must be dropped, along with zero-width chars and BOM.
        let sneaky = "claude \u{202e}snoissimrep-piks-ylsuoregnad--\u{202c} x";
        let clean = sanitize_control(sneaky);
        assert!(!clean.contains('\u{202e}'));
        assert!(!clean.contains('\u{202c}'));
        assert_eq!(sanitize_control("a\u{200b}b\u{feff}c"), "abc");
        // Ordinary text and legitimate non-ASCII survive.
        assert_eq!(sanitize_control("café ☕ 日本語"), "café ☕ 日本語");
    }
}
