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
/// look like a real project directory.  Use `derive_project` for the
/// caller-friendly name that always returns *something* meaningful.
pub fn project_basename(cwd: &str) -> String {
    let p = cwd.trim_end_matches('/');
    p.rsplit('/').find(|s| !s.is_empty()).unwrap_or("").to_string()
}

/// Caller-friendly project label.  Falls back through cwd-basename →
/// `<label> <subcommand>` (for daemons like `ollama serve`) → exe basename
/// → label so we never display a bare `?`.
pub fn derive_project(cwd: &str, exe: &str, cmdline: &str, label: &str) -> String {
    // Primary: cwd basename, if the cwd actually identifies a project.
    let cwd_basename = project_basename(cwd);
    let cwd_bad = cwd_basename.is_empty()
        || cwd == "/"
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
        let p = derive_project("/home/matt/code/xsol", "/usr/bin/claude",
                               "claude --resume", "claude");
        assert_eq!(p, "xsol");
    }

    #[test]
    fn project_falls_back_to_label_for_bare_invocation() {
        let p = derive_project("/proc/self", "/usr/bin/claude", "claude", "claude");
        assert_eq!(p, "claude");
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
