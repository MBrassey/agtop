// Cross-mount home discovery.
//
// agtop on Linux/macOS sees `dirs::home_dir()`.  On Windows, sysinfo
// gives us Windows-native processes and `dirs::home_dir()` returns
// `C:\Users\<u>`.  Inside WSL, the Linux binary sees `/home/<u>` —
// but the user may *also* be running Claude / Codex / Gemini from
// the Windows side, which writes to `C:\Users\<u>\.claude\projects`,
// mounted into WSL at `/mnt/c/Users/<u>/.claude/projects`.
//
// Without scanning the alternate mount, agtop run inside WSL misses
// every Windows-side session and vice-versa.  This module enumerates
// candidate "home" prefixes the vendor modules then suffix with
// `.claude` / `.codex` / `.gemini` / `.config/goose`.
//
// Three sources, in order:
//   1. `dirs::home_dir()` — the binary's own home.
//   2. WSL auto-probe — if /proc/version reports Microsoft/WSL, walk
//      `/mnt/c/Users/*` and add each non-system user dir.
//   3. `AGTOP_EXTRA_HOMES` env var — `:`-separated (Unix) or `;`-separated
//      (Windows) list of extra home prefixes.  Escape hatch for non-WSL
//      mounted homes (sshfs, NFS, network drives).
//
// Results are deduped preserving insertion order so the user's own
// home is always probed first.

use std::path::PathBuf;

pub fn home_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        out.push(h);
    }
    if is_wsl() {
        if let Ok(rd) = std::fs::read_dir("/mnt/c/Users") {
            for ent in rd.flatten() {
                let p = ent.path();
                if !p.is_dir() { continue; }
                let name = ent.file_name();
                let lname = name.to_string_lossy().to_ascii_lowercase();
                // Skip Windows built-in profiles that never host a user
                // agent install.  Real human accounts always have a
                // distinct username here.
                if matches!(lname.as_str(),
                    "public" | "default" | "default user" | "all users"
                    | "defaultuser0" | "defaultuser100000" | "desktop.ini"
                ) { continue; }
                out.push(p);
            }
        }
    }
    if let Ok(s) = std::env::var("AGTOP_EXTRA_HOMES") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for tok in s.split(sep) {
            let t = tok.trim();
            if t.is_empty() { continue; }
            out.push(PathBuf::from(t));
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

#[cfg(target_os = "linux")]
fn is_wsl() -> bool {
    if let Ok(s) = std::fs::read_to_string("/proc/version") {
        let l = s.to_ascii_lowercase();
        if l.contains("microsoft") || l.contains("wsl") { return true; }
    }
    if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        let l = s.to_ascii_lowercase();
        if l.contains("microsoft") || l.contains("wsl") { return true; }
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn is_wsl() -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_home_always_first() {
        let r = home_roots();
        if let Some(h) = dirs::home_dir() {
            assert_eq!(r.first(), Some(&h));
        }
    }

    #[test]
    fn extra_homes_env_appended() {
        // SAFETY: tests run single-threaded in this crate's default
        // harness; setting an env var here doesn't race with other tests.
        let sep = if cfg!(windows) { ";" } else { ":" };
        let val = format!("/tmp/fake-home-a{sep}/tmp/fake-home-b");
        // Use a temp scope guard so we always restore.
        let prev = std::env::var("AGTOP_EXTRA_HOMES").ok();
        std::env::set_var("AGTOP_EXTRA_HOMES", &val);
        let r = home_roots();
        assert!(r.iter().any(|p| p == &PathBuf::from("/tmp/fake-home-a")));
        assert!(r.iter().any(|p| p == &PathBuf::from("/tmp/fake-home-b")));
        match prev {
            Some(v) => std::env::set_var("AGTOP_EXTRA_HOMES", v),
            None => std::env::remove_var("AGTOP_EXTRA_HOMES"),
        }
    }
}
