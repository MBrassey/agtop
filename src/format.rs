// Human-friendly formatters used by both the TUI and the --once / --json paths.

use std::path::Path;

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

pub fn tildeify<P: AsRef<Path>>(p: P) -> String {
    let p = p.as_ref().to_string_lossy().into_owned();
    if let Some(home) = dirs::home_dir() {
        let h = home.to_string_lossy();
        if let Some(rest) = p.strip_prefix(h.as_ref()) {
            return format!("~{}", rest);
        }
    }
    p
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

pub fn shorten_left(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let skip = count - (n.saturating_sub(1));
    let mut out = String::from("…");
    out.extend(s.chars().skip(skip));
    out
}

pub fn project_basename(cwd: &str) -> String {
    let p = cwd.trim_end_matches('/');
    p.rsplit('/').find(|s| !s.is_empty()).unwrap_or("?").to_string()
}
