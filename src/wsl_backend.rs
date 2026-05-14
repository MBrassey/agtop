// Windows-only: surface live Linux processes running inside WSL2
// distros so a Windows agtop sees both Windows-native agents (via
// sysinfo) AND WSL agents in one snapshot.  The WSL2 utility VM
// runs a separate Linux kernel, so the Windows process table never
// exposes its PIDs — we shell out via `wsl.exe -d <distro>` per
// snapshot and read /proc inside the guest.
//
// Cost: one wsl.exe spawn per running distro per tick.  The first
// spawn after `wsl --shutdown` cold-starts the VM (~1-3 s); steady
// state runs are ~30-80 ms per distro because the VM keeps a
// background `init` warm.  We only enumerate distros reported as
// `Running` by `wsl.exe -l --running --quiet` to avoid forcing a
// cold-start on a distro the user has deliberately stopped.
//
// Output protocol: one record per process, fields base64-encoded
// so cmdlines containing `|`, spaces, or non-UTF-8 bytes round-trip
// safely.  Format:
//
//   <pid> <comm_b64> <exe_b64> <cwd_b64> <cmdline_b64> <stat_b64>
//
// All five b64 strings are URL-safe-padding-stripped on the Rust
// side because GNU `base64` and BusyBox `base64` emit identical
// canonical output.

#![cfg(windows)]

use crate::matchers::{classify, Matcher, UserMatcher};
use crate::model::{Agent, Status, encode_wsl_pid};
use crate::proc_::{self, CLK_TCK, PAGE_SIZE};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Per-pid CPU-delta state.  Key is the namespaced (Windows-side) PID;
/// value is `(utime+stime ticks, wall-clock ms at sample time)`.  The
/// HashMap is shared across `collect()` calls so the *second* tick
/// produces a meaningful CPU%.  Dead PIDs are evicted at end of each
/// `collect()` to keep the map bounded.
fn prev_cpu() -> &'static Mutex<HashMap<u32, (u64, u64)>> {
    static PREV: OnceLock<Mutex<HashMap<u32, (u64, u64)>>> = OnceLock::new();
    PREV.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-tick collection entry-point.  Returns one `Agent` per live
/// WSL process that matches a known agent matcher, or an empty Vec
/// when WSL isn't installed / no distro is running / wsl.exe is
/// missing from PATH.
pub fn collect(builtins: &[Matcher], user: &[UserMatcher]) -> Vec<Agent> {
    let distros = running_distros();
    if distros.is_empty() { return Vec::new(); }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut prev = prev_cpu().lock().expect("wsl prev_cpu mutex poisoned");
    let mut out: Vec<Agent> = Vec::new();
    for (idx, distro) in distros.iter().enumerate() {
        // u8 fits the distro index because we cap encoding at 128
        // (encode_wsl_pid masks to 7 bits anyway).
        let idx_u8 = if idx > 127 { 127 } else { idx as u8 };
        let blob = match dump_proc(distro) {
            Some(b) => b,
            None => continue,
        };
        for line in blob.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() { continue; }
            let cols: Vec<&str> = line.splitn(6, ' ').collect();
            if cols.len() < 6 { continue; }
            let linux_pid: u32 = match cols[0].parse() { Ok(p) => p, Err(_) => continue };
            let comm    = b64_to_string(cols[1]);
            let exe     = b64_to_string(cols[2]);
            let cwd     = b64_to_string(cols[3]);
            let cmdline = b64_to_string(cols[4]);
            let stat    = b64_to_string(cols[5]);
            // Empty cmdline ⇒ kernel thread.  Match by comm fallback
            // (same trick as the Windows sysbackend path uses with
            // proc.name).
            let mut classify_input = String::with_capacity(
                cmdline.len() + exe.len() + comm.len() + 2);
            classify_input.push_str(&cmdline);
            if !exe.is_empty() {
                classify_input.push(' ');
                classify_input.push_str(&exe);
            }
            if !comm.is_empty() {
                classify_input.push(' ');
                classify_input.push_str(&comm);
            }
            if classify_input.trim().is_empty() { continue; }
            let label = match classify(&classify_input, builtins, user) {
                Some(l) => l.to_string(),
                None => continue,
            };
            let parsed = match proc_::parse_stat(&stat) {
                Some(s) => s,
                None => continue,
            };
            let rss_bytes = parsed.rss_pages.saturating_mul(PAGE_SIZE);
            // Best-effort uptime: WSL kernel uptime is independent of
            // Windows; we use clock-tick start time but lack a boot
            // baseline, so report seconds-since-tick-zero by clamping.
            // Worst case: a fresh distro shows a few seconds of skew.
            // Real uptime fidelity would cost another wsl.exe spawn per
            // tick — not worth it.
            let uptime_sec = parsed.starttime / CLK_TCK;

            // CPU%: delta utime+stime ticks ÷ delta wall ms, scaled
            // to a 1-core percentage so multi-threaded saturation
            // reads >100% the same way `top` shows it.  First sample
            // for any PID is 0%; second tick onwards produces the
            // real value.
            let namespaced = encode_wsl_pid(idx_u8, linux_pid);
            let proc_total = parsed.utime.saturating_add(parsed.stime);
            let cpu_pct = match prev.get(&namespaced) {
                Some((pt, pms)) if now_ms > *pms => {
                    let dt_s = (now_ms - *pms) as f64 / 1000.0;
                    let delta = proc_total.saturating_sub(*pt) as f64;
                    if dt_s > 0.0 {
                        (delta / CLK_TCK as f64) / dt_s * 100.0
                    } else { 0.0 }
                }
                _ => 0.0,
            };
            prev.insert(namespaced, (proc_total, now_ms));

            let project = crate::format::derive_project(&cwd, &exe, &cmdline, &label);
            let cmdline_clean = crate::format::sanitize_control(&cmdline);
            let cwd_clean     = crate::format::sanitize_control(&cwd);
            let exe_clean     = crate::format::sanitize_control(&exe);

            out.push(Agent {
                pid: namespaced,
                label,
                status: Status::Active,
                project,
                current_tool: None,
                current_task: None,
                subagents: 0,
                session_id: None,
                session_age_ms: None,
                tokens_total: 0,
                tokens_input: 0,
                tokens_output: 0,
                tokens_cache_read: 0,
                tokens_cache_write: 0,
                cost_usd: 0.0,
                cost_basis: "unknown".into(),
                context_used: 0,
                context_limit: 0,
                loaded_skills: Vec::new(),
                loaded_plugins: Vec::new(),
                tool_counts: Vec::new(),
                ppid_name: String::new(),
                session_started_ms: 0,
                dangerous_flag: crate::collector::dangerous_flag_for_cmdline(&cmdline_clean),
                model: None,
                dangerous: crate::collector::is_dangerous_for_cmdline(&cmdline_clean),
                in_flight_subagents: Vec::new(),
                recent_activity: Vec::new(),
                cpu_history: Vec::new(),
                tokens_history: Vec::new(),
                cpu: cpu_pct,
                cpu_raw: cpu_pct,
                rss: rss_bytes,
                vsize: parsed.vsize,
                threads: parsed.num_threads,
                state: parsed.state.to_string(),
                ppid: parsed.ppid,
                uptime_sec,
                cwd: cwd_clean,
                exe: exe_clean,
                cmdline: cmdline_clean,
                read_bytes: 0,
                write_bytes: 0,
                writing_files: Vec::new(),
                writing_dirs: Vec::new(),
                reading_files: Vec::new(),
                children: Vec::new(),
                net_established: 0,
                read_rate_bps: 0,
                write_rate_bps: 0,
                gpu_pct: 0.0,
                gpu_mem_bytes: 0,
                host: format!("wsl:{}", distro),
            });
        }
    }
    // Evict prev-tick CPU samples for PIDs that disappeared.
    let live: std::collections::HashSet<u32> =
        out.iter().map(|a| a.pid).collect();
    prev.retain(|k, _| live.contains(k));
    out
}

/// Distros currently in the `Running` state.  `wsl.exe -l --running
/// --quiet` emits one distro per line in UTF-16 LE.
fn running_distros() -> Vec<String> {
    let output = match Command::new("wsl.exe")
        .args(["-l", "--running", "--quiet"])
        .output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    decode_utf16(&output.stdout)
        .lines()
        .map(|l| l.trim().trim_matches('\0').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Run a small shell script inside `<distro>` that walks /proc and
/// prints one base64-packed record per PID.  Failure (subprocess
/// non-zero, malformed output, missing base64) returns None and the
/// caller skips this distro for this tick.
fn dump_proc(distro: &str) -> Option<String> {
    // Single-quoted heredoc keeps the script literal (no Windows
    // path-quoting / variable expansion concerns).  `tr -d '\n' < stat`
    // collapses the rare multi-line stat (kernel threads with embedded
    // newlines in comm) onto one line so the b64 round-trip is safe.
    const SCRIPT: &str = r#"
b64() { base64 -w0 2>/dev/null || base64 | tr -d '\n'; }
for d in /proc/[0-9]*; do
  pid="${d#/proc/}"
  [ -d "$d" ] || continue
  comm=$(cat "$d/comm" 2>/dev/null    | b64)
  exe=$(readlink "$d/exe" 2>/dev/null  | b64)
  cwd=$(readlink "$d/cwd" 2>/dev/null  | b64)
  cmdline=$(tr '\0' ' ' < "$d/cmdline" 2>/dev/null | b64)
  stat=$(tr -d '\n' < "$d/stat" 2>/dev/null | b64)
  printf '%s %s %s %s %s %s\n' "$pid" "$comm" "$exe" "$cwd" "$cmdline" "$stat"
done
"#;
    let output = Command::new("wsl.exe")
        .args(["-d", distro, "--exec", "/bin/sh", "-c", SCRIPT])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    // wsl.exe outputs the *guest's* stdout verbatim (no UTF-16 conv)
    // when invoked with `--exec`, so this is plain UTF-8.  Decoding
    // as UTF-16 here would corrupt the base64 payload.
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn b64_to_string(s: &str) -> String {
    if s.is_empty() { return String::new(); }
    // GNU base64 emits `+`, `/`, `=`.  Plain decoder, no external dep.
    let bytes = b64_decode(s).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Minimal base64 decoder (RFC 4648 standard alphabet).  Tolerates
/// missing padding so a stray newline at end-of-line in the wsl.exe
/// output stream doesn't trip the decoder.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => return None,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.bytes() {
        if c == b'=' { break; }
        let v = match val(c) { Some(v) => v, None => continue };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// Decode a UTF-16 LE byte slice (with optional BOM) into a Rust
/// String.  Used for `wsl.exe -l` output; `--exec` output is plain
/// UTF-8 and bypasses this.
fn decode_utf16(bytes: &[u8]) -> String {
    let start = if bytes.starts_with(&[0xFF, 0xFE]) { 2 } else { 0 };
    let u16s: Vec<u16> = bytes[start..].chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_decode_roundtrip() {
        // "claude --resume" base64-encoded:
        let s = "Y2xhdWRlIC0tcmVzdW1l";
        let decoded = b64_decode(s).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "claude --resume");
    }

    #[test]
    fn b64_decode_empty() {
        assert_eq!(b64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn b64_to_string_decodes_utf8() {
        // "/home/user/projects/agtop"
        let s = "L2hvbWUvdXNlci9wcm9qZWN0cy9hZ3RvcA==";
        assert_eq!(b64_to_string(s), "/home/user/projects/agtop");
    }

    #[test]
    fn decode_utf16_strips_bom() {
        let bytes = [0xFF, 0xFE, b'h', 0x00, b'i', 0x00];
        assert_eq!(decode_utf16(&bytes), "hi");
    }
}
