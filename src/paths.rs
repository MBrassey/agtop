// Cross-mount home discovery.
//
// agtop on Linux/macOS sees `dirs::home_dir()`.  On Windows, sysinfo
// gives us Windows-native processes and `dirs::home_dir()` returns
// `C:\Users\<u>`.  Inside WSL, the Linux binary sees `/home/<u>` —
// but the user may *also* be running Claude / Codex / Gemini from
// the Windows side, which writes to `C:\Users\<u>\.claude\projects`,
// mounted into WSL at `/mnt/c/Users/<u>/.claude/projects`.  And vice
// versa: a Windows-native agtop should still see agents running
// inside WSL, whose homes live under `\\wsl$\<distro>\home\<user>`.
//
// Without scanning the alternate mount, agtop misses every session on
// the other side.  This module enumerates candidate "home" prefixes
// the vendor modules then suffix with `.claude` / `.codex` /
// `.gemini` / `.config/goose`.
//
// Four sources, in order:
//   1. `dirs::home_dir()` — the binary's own home.
//   2. WSL→Windows auto-probe — if /proc/version reports Microsoft/WSL,
//      walk `/mnt/c/Users/*` and add each non-system user dir.
//   3. Windows→WSL auto-probe — on Windows, enumerate `\\wsl$\<distro>`
//      (or `\\wsl.localhost\<distro>`) and add every `\home\<user>`
//      plus `\root` from each running distro.
//   4. `AGTOP_EXTRA_HOMES` env var — `:`-separated (Unix) or
//      `;`-separated (Windows) list of extra home prefixes.  Escape
//      hatch for non-WSL mounted homes (sshfs, NFS, network drives).
//
// Results are deduped preserving insertion order so the user's own
// home is always probed first.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Refresh cadence for the cross-mount probes below.  Every vendor
/// module calls `home_roots()` once per tick, and the WSL↔Windows
/// probes stat a drvfs / 9p / UNC mount where a single stat costs
/// 1-10 ms — but the set of user profiles changes essentially never,
/// so a coarse TTL removes the per-tick cost without a manual
/// invalidation path.
const MOUNT_PROBE_TTL: Duration = Duration::from_secs(30);

pub fn home_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        out.push(h);
    }
    // Cheap parts (own home, env var) are recomputed every call; only
    // the slow cross-mount enumerations are cached.
    for h in cross_mount_homes() { out.push(h); }
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

/// TTL-cached union of the alternate-mount home prefixes: `/mnt/c/Users/*`
/// when running inside WSL, `\\wsl$\<distro>\home\*` when running on
/// Windows.  Empty everywhere else.
fn cross_mount_homes() -> Vec<PathBuf> {
    static CACHE: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, homes)) = guard.as_ref() {
        if at.elapsed() < MOUNT_PROBE_TTL {
            return homes.clone();
        }
    }
    let homes = probe_cross_mount_homes();
    *guard = Some((Instant::now(), homes.clone()));
    homes
}

fn probe_cross_mount_homes() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
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
    #[cfg(windows)]
    {
        for h in wsl_homes_from_windows() { out.push(h); }
    }
    out
}

/// Windows side: enumerate running WSL distros via the `\\wsl$\`
/// (or `\\wsl.localhost\`) virtual share and surface every
/// `\home\<user>` plus the bare `\root` directory as a candidate
/// home prefix.  Returns an empty Vec when WSL isn't installed —
/// `fs::read_dir` on a missing UNC share fails fast with `ENOENT`.
///
/// Falls back to invoking `wsl.exe -l -q` to list distros if neither
/// UNC share is enumerable — covers older Windows 10 builds where
/// the share doesn't expose itself to `FindFirstFileW` on a bare path.
#[cfg(windows)]
fn wsl_homes_from_windows() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let distros = list_wsl_distros();
    for share in [r"\\wsl$", r"\\wsl.localhost"] {
        for distro in &distros {
            let home_root = PathBuf::from(format!(r"{share}\{distro}\home"));
            if let Ok(rd) = std::fs::read_dir(&home_root) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if p.is_dir() { out.push(p); }
                }
            }
            // Root user's home — common on minimal distros where
            // the human is logged in as root rather than a regular user.
            let root_home = PathBuf::from(format!(r"{share}\{distro}\root"));
            if root_home.exists() { out.push(root_home); }
        }
        if !out.is_empty() { break; }
    }
    out
}

#[cfg(windows)]
fn list_wsl_distros() -> Vec<String> {
    // Preferred: directly enumerate the WSL share.  No subprocess,
    // no UTF-16 decode dance, no spin-up of the WSL service.
    for share in [r"\\wsl$\", r"\\wsl.localhost\"] {
        if let Ok(rd) = std::fs::read_dir(share) {
            let names: Vec<String> = rd.flatten()
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter(|n| !n.is_empty() && !n.starts_with('.'))
                .collect();
            if !names.is_empty() { return names; }
        }
    }
    // Fallback: `wsl.exe -l -q` lists running + installed distro names.
    // Output is UTF-16 LE with a BOM and CR/LF line endings.
    let output = match std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        .output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let bytes = output.stdout;
    // Strip BOM if present.
    let start = if bytes.starts_with(&[0xFF, 0xFE]) { 2 } else { 0 };
    let u16s: Vec<u16> = bytes[start..].chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16_lossy(&u16s);
    s.lines()
        .map(|l| l.trim().trim_matches('\0').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(target_os = "linux")]
fn is_wsl() -> bool {
    // The kernel identity can't change under a running process —
    // memoize so per-tick callers don't re-read /proc/version forever.
    static IS_WSL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_WSL.get_or_init(|| {
        if let Ok(s) = std::fs::read_to_string("/proc/version") {
            let l = s.to_ascii_lowercase();
            if l.contains("microsoft") || l.contains("wsl") { return true; }
        }
        if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
            let l = s.to_ascii_lowercase();
            if l.contains("microsoft") || l.contains("wsl") { return true; }
        }
        false
    })
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
        // Prime the cross-mount cache first: the env-var portion must
        // be re-read every call, not frozen into the TTL cache.
        let _ = home_roots();
        std::env::set_var("AGTOP_EXTRA_HOMES", &val);
        let r = home_roots();
        assert!(r.iter().any(|p| p == &PathBuf::from("/tmp/fake-home-a")));
        assert!(r.iter().any(|p| p == &PathBuf::from("/tmp/fake-home-b")));
        match prev {
            Some(v) => std::env::set_var("AGTOP_EXTRA_HOMES", v),
            None => std::env::remove_var("AGTOP_EXTRA_HOMES"),
        }
    }

    #[test]
    fn cross_mount_probe_is_stable_within_ttl() {
        let a = cross_mount_homes();
        let b = cross_mount_homes();
        assert_eq!(a, b);
    }
}
