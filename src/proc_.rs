// Linux /proc parser. Reads only what we need, swallows races (PID disappearing
// mid-read), and returns Option<T> rather than propagating errors so the
// collector can simply skip processes that vanished.

use std::fs;
use std::path::PathBuf;

pub const CLK_TCK: u64 = 100; // glibc default on Linux. sysconf would need libc.
pub const PAGE_SIZE: u64 = 4096;

pub fn is_linux() -> bool {
    cfg!(target_os = "linux") && std::path::Path::new("/proc").exists()
}

pub fn list_pids() -> Vec<u32> {
    let mut out = Vec::with_capacity(256);
    if let Ok(rd) = fs::read_dir("/proc") {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.bytes().next().map(|b| b.is_ascii_digit()) == Some(true) {
                    if let Ok(pid) = name.parse::<u32>() {
                        out.push(pid);
                    }
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // pid + comm are part of the Stat record we may later
                    // surface in --json or use for ppid-tree analysis.
pub struct Stat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub utime: u64,
    pub stime: u64,
    pub num_threads: u64,
    pub starttime: u64,
    pub vsize: u64,
    pub rss_pages: u64,
}

pub fn parse_stat(text: &str) -> Option<Stat> {
    // Field 2 is "(comm)" which itself can contain spaces or parens
    // per `man 5 proc`.  Anchor the right paren to the literal sequence
    // ") " since the kernel always writes a single space between comm
    // and the state field; this survives `comm`s like "bash) (foo)".
    let lp = text.find('(')?;
    let rp = text.rfind(") ").map(|i| i + 1).or_else(|| text.rfind(')'))?;
    if rp <= lp { return None; }
    let pid: u32 = text[..lp].trim().parse().ok()?;
    let comm = text[lp + 1..rp].to_string();
    let rest: Vec<&str> = text[rp + 1..].split_whitespace().collect();
    if rest.len() < 22 { return None; }
    let state = rest[0].chars().next().unwrap_or('?');
    let ppid: u32 = rest[1].parse().ok()?;
    let utime: u64 = rest[11].parse().unwrap_or(0);
    let stime: u64 = rest[12].parse().unwrap_or(0);
    let num_threads: u64 = rest[17].parse().unwrap_or(1);
    let starttime: u64 = rest[19].parse().unwrap_or(0);
    let vsize: u64 = rest[20].parse().unwrap_or(0);
    let rss_pages: u64 = rest[21].parse().unwrap_or(0);
    Some(Stat { pid, comm, state, ppid, utime, stime, num_threads, starttime, vsize, rss_pages })
}

pub fn read_stat(pid: u32) -> Option<Stat> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&text)
}

pub fn read_cmdline(pid: u32) -> String {
    match fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(buf) => {
            // NUL-separated argv.  Decode UTF-8 lossily so non-ASCII args
            // (paths with emoji etc.) survive instead of being byte-cast.
            let s = String::from_utf8_lossy(&buf);
            s.replace('\0', " ").trim().to_string()
        }
        Err(_) => String::new(),
    }
}

pub fn read_link(pid: u32, name: &str) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/{name}")).ok()
}

/// Read the kernel-recorded comm name (basename of the binary as the
/// task started it) for any pid.  Used to resolve PPIDs to a
/// human-readable shell / launcher name (`zsh`, `bash`, `fish`,
/// `nu`, `tmux`, `code`, `kitty`, …).  Shell-agnostic — whatever
/// the kernel sees.  Returns None if the pid is gone or unreadable.
pub fn read_comm(pid: u32) -> Option<String> {
    let s = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// Enumerate the *read-mode* file descriptors of `pid` and resolve
/// each to its on-disk path.  Mirror of [`read_writing_files`] but
/// for `O_RDONLY` entries — surfaces what the agent is actively
/// reading (project files during context indexing, MCP server
/// configs, etc.) when nothing else explains where its CPU is going.
pub fn read_reading_files(pid: u32, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let fdinfo_dir = format!("/proc/{pid}/fdinfo");
    let fd_dir     = format!("/proc/{pid}/fd");
    let entries = match fs::read_dir(&fdinfo_dir) { Ok(e) => e, Err(_) => return out };
    for entry in entries.flatten() {
        if out.len() >= limit { break; }
        let name = entry.file_name();
        let info = match fs::read_to_string(format!("{}/{}", fdinfo_dir, name.to_string_lossy())) {
            Ok(s) => s, Err(_) => continue,
        };
        let flags_line = info.lines().find(|l| l.starts_with("flags:"));
        let Some(line) = flags_line else { continue };
        let flags = line.split_whitespace().nth(1)
            .and_then(|s| u64::from_str_radix(s, 8).ok())
            .unwrap_or(0);
        // Skip write-mode entries (O_WRONLY = 1, O_RDWR = 2);
        // those are covered by read_writing_files.
        if (flags & 0x3) != 0 { continue; }
        let target = match fs::read_link(format!("{}/{}", fd_dir, name.to_string_lossy())) {
            Ok(t) => t, Err(_) => continue,
        };
        let s = target.to_string_lossy();
        if s.starts_with("/dev/")
            || s.starts_with("pipe:") || s.starts_with("socket:")
            || s.starts_with("anon_inode:") || s.starts_with("memfd:")
            || s.starts_with("dmabuf:")
            || s.ends_with(" (deleted)")
            || s == "/dev/null" {
            continue;
        }
        out.push(target);
    }
    out
}

/// Enumerate the immediate child processes of `pid` (one level deep,
/// not the whole descendant tree) and return `(child_pid, comm)`
/// pairs.  Used to surface what an agent has spawned — Claude Code's
/// hooks, MCP server processes, post-tool shell commands, etc.
pub fn read_children(pid: u32, limit: usize) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    // /proc/<pid>/task/<tid>/children lists the child pids of every
    // thread in the process; the union is the process's child set.
    let task_dir = format!("/proc/{pid}/task");
    let tasks = match fs::read_dir(&task_dir) { Ok(e) => e, Err(_) => return out };
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for t in tasks.flatten() {
        if out.len() >= limit { break; }
        let path = t.path().join("children");
        let s = match fs::read_to_string(&path) { Ok(s) => s, Err(_) => continue };
        for tok in s.split_whitespace() {
            if out.len() >= limit { break; }
            let Ok(cpid) = tok.parse::<u32>() else { continue };
            if !seen.insert(cpid) { continue; }
            let comm = read_comm(cpid).unwrap_or_else(|| "?".into());
            out.push((cpid, comm));
        }
    }
    out
}

/// Count the established network connections owned by `pid`.  Walks
/// /proc/<pid>/net/{tcp,tcp6,udp,udp6} and counts the rows whose
/// state is ESTABLISHED (state code `01` for TCP).  Surfaces "agent
/// is talking to its API / an MCP server" in the popup.
pub fn count_net_established(pid: u32) -> u32 {
    let mut n = 0u32;
    for proto in ["tcp", "tcp6"] {
        let path = format!("/proc/{pid}/net/{proto}");
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for line in text.lines().skip(1) {
            let mut cols = line.split_whitespace();
            let _local = cols.next();
            let _remote = cols.next();
            let state = cols.next().unwrap_or("");
            // 01 = TCP_ESTABLISHED in the kernel's enum.
            if state == "01" { n += 1; }
        }
    }
    n
}

pub fn read_cwd(pid: u32) -> Option<PathBuf> { read_link(pid, "cwd") }
pub fn read_exe(pid: u32) -> Option<PathBuf> { read_link(pid, "exe") }

#[derive(Debug, Clone, Default)]
pub struct ProcIo {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

pub fn read_io(pid: u32) -> Option<ProcIo> {
    let text = fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut io = ProcIo::default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().parse::<u64>().unwrap_or(0);
            match k {
                "read_bytes" => io.read_bytes = v,
                "write_bytes" => io.write_bytes = v,
                _ => {}
            }
        }
    }
    Some(io)
}

/// Open writable files for a process, filtered to skip /dev/, pipes, sockets,
/// anon-inode / memfd / dmabuf, and deleted-file stubs. Capped at `limit`.
pub fn read_writing_files(pid: u32, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let fdinfo_dir = format!("/proc/{pid}/fdinfo");
    let fd_dir = format!("/proc/{pid}/fd");
    let entries = match fs::read_dir(&fdinfo_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        if out.len() >= limit { break; }
        let name = entry.file_name();
        let info = match fs::read_to_string(format!("{}/{}", fdinfo_dir, name.to_string_lossy())) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let flags_line = info.lines().find(|l| l.starts_with("flags:"));
        let Some(line) = flags_line else { continue; };
        // flags is octal
        let flags = line.split_whitespace().nth(1)
            .and_then(|s| u64::from_str_radix(s, 8).ok())
            .unwrap_or(0);
        if (flags & 0x3) == 0 {
            continue; // O_RDONLY
        }
        let target = match fs::read_link(format!("{}/{}", fd_dir, name.to_string_lossy())) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let s = target.to_string_lossy();
        if s.starts_with("/dev/")
            || s.starts_with("pipe:") || s.starts_with("socket:")
            || s.starts_with("anon_inode:") || s.starts_with("memfd:")
            || s.starts_with("dmabuf:")
            || s.ends_with(" (deleted)")
            || s == "/dev/null" {
            continue;
        }
        out.push(target);
    }
    out
}

pub fn read_boot_time() -> u64 {
    let text = match fs::read_to_string("/proc/stat") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().unwrap_or(0);
        }
    }
    0
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
}

pub fn read_meminfo() -> MemInfo {
    let mut out = MemInfo::default();
    let text = match fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return out,
    };
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let val = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        match key {
            "MemTotal:"     => out.total     = val * 1024,
            "MemAvailable:" => out.available = val * 1024,
            _ => {}
        }
    }
    out
}

pub fn read_system_cpu_total() -> u64 {
    let text = match fs::read_to_string("/proc/stat") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let line = match text.lines().find(|l| l.starts_with("cpu ")) {
        Some(l) => l,
        None => return 0,
    };
    line.split_whitespace().skip(1).filter_map(|s| s.parse::<u64>().ok()).sum()
}

pub fn num_cpus() -> usize {
    // cgroup-aware via std; aarch64-friendly (avoids /proc/cpuinfo "processor"
    // line quirks on some kernels).
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

