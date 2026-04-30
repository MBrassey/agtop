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
    if v >= 100.0 {
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
    format!("{}d{}h", d, h)
}

/// SI-suffixed integer formatter for token counts and similar.
/// 1234 → "1.2k", 1_234_567 → "1.2M".
pub fn si(n: u64) -> String {
    if n < 1_000 { return n.to_string(); }
    if n < 1_000_000 { return format!("{:.1}k", n as f64 / 1_000.0); }
    if n < 1_000_000_000 { return format!("{:.1}M", n as f64 / 1_000_000.0); }
    format!("{:.1}B", n as f64 / 1_000_000_000.0)
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
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\t' => out.push(' '),
            '\x1b' => {
                // Eat any CSI / OSC parameter run + final byte / terminator.
                if let Some(&n) = chars.peek() {
                    if n == '[' || n == ']' || n == '(' || n == ')' || n == 'P' || n == '_' || n == '^' {
                        chars.next();
                        for c2 in chars.by_ref() {
                            if c2 == '\x07' || c2 == '\x1b' { break; }
                            if c2.is_ascii_alphabetic() || c2 == '\\' { break; }
                        }
                    } else {
                        chars.next();
                    }
                }
            }
            c if (c as u32) < 0x20 || c == '\x7f' => { /* drop */ }
            // C1 controls 0x80..=0x9f are dropped.
            c if (c as u32) >= 0x80 && (c as u32) <= 0x9f => { /* drop */ }
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
            trimmed.rsplit(|c| c == '/' || c == '\\')
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
    let exe_basename = exe.trim_end_matches('/').rsplit('/').next().unwrap_or("");
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
}
