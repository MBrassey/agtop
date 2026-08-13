// Stitches /proc, matchers, and ~/.claude/projects into a single Snapshot.
// Holds smoothing state across snapshots so the TUI doesn't jitter.

use crate::sessions::{self, LiveAgentRef};
use crate::{aider, claude, codex, gemini, generic, goose};
use crate::format::derive_project;
use crate::pricing::PriceTable;
use crate::sysbackend::SysBackend;

/// One-shot per-tick lookup of NVIDIA GPU usage by PID.  Spawns
/// `nvidia-smi --query-compute-apps=pid,used_gpu_memory --format=csv,noheader,nounits`
/// plus a follow-up `--query-gpu` for utilisation.  Returns
/// `pid → (utilisation_pct, used_vram_bytes)`.  Empty (no
/// allocations) if nvidia-smi isn't on PATH or returns non-zero.
///
/// Cost: one fork+exec per snapshot.  The query itself takes
/// ~30-60 ms on a Linux host with one GPU; safe to run inside the
/// collector tick.  AMD / Apple Silicon / Intel Arc are TODO.
fn read_gpu_usage() -> std::collections::HashMap<u32, (f64, u64)> {
    use std::collections::HashMap;
    use std::process::Command;
    use std::time::Duration;
    let mut out: HashMap<u32, (f64, u64)> = HashMap::new();
    // Skip the fork entirely on the overwhelmingly common non-NVIDIA host
    // — probing PATH once (see `nvidia_smi_present`) avoids a doomed
    // spawn every tick forever.
    if !nvidia_smi_present() { return out; }
    // Per-process: pid + used_memory (MiB).  --query-compute-apps
    // doesn't include per-process utilisation; we attribute the
    // host-wide gpu utilisation pct to every process on it weighted
    // by VRAM share — close enough for "is this agent burning GPU".
    // Capped at 1.5 s so a wedged GPU driver (Xid errors, a powered-down
    // GPU) can't hang the synchronous collector tick — and with it the
    // whole TUI — indefinitely.
    let mut apps_cmd = Command::new("nvidia-smi");
    apps_cmd.args(["--query-compute-apps=pid,used_gpu_memory",
                   "--format=csv,noheader,nounits"]);
    let apps = match run_capped(apps_cmd, Duration::from_millis(1500)) {
        Some(b) => b,
        None => return out,
    };
    let mut total_mem: u64 = 0;
    let mut entries: Vec<(u32, u64)> = Vec::new();
    for line in std::str::from_utf8(&apps).unwrap_or("").lines() {
        let mut cols = line.split(',').map(str::trim);
        let Some(pid_s) = cols.next() else { continue };
        let Some(mem_s) = cols.next() else { continue };
        let Ok(pid) = pid_s.parse::<u32>() else { continue };
        let mem_mib = mem_s.parse::<u64>().unwrap_or(0);
        // saturating_mul defends against an adversarial nvidia-smi
        // shim (or future driver bug) that returns a >2^54 MiB
        // value, which would otherwise overflow u64.
        let mem_bytes = mem_mib.saturating_mul(1024 * 1024);
        total_mem = total_mem.saturating_add(mem_bytes);
        entries.push((pid, mem_bytes));
    }
    if entries.is_empty() { return out; }

    // Host-wide utilisation (sum across all GPUs).
    let mut util_cmd = Command::new("nvidia-smi");
    util_cmd.args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"]);
    let util_total = run_capped(util_cmd, Duration::from_millis(1500))
        .map(|b| {
            std::str::from_utf8(&b).unwrap_or("").lines()
                .filter_map(|l| l.trim().parse::<f64>().ok()).sum::<f64>()
        }).unwrap_or(0.0);

    for (pid, mem_bytes) in entries {
        let share = if total_mem > 0 { mem_bytes as f64 / total_mem as f64 } else { 0.0 };
        out.insert(pid, (util_total * share, mem_bytes));
    }
    out
}

/// True if an `nvidia-smi` binary is on PATH.  Resolved once per process
/// — a non-NVIDIA host would otherwise pay a failed fork+exec every tick.
fn nvidia_smi_present() -> bool {
    use std::sync::OnceLock;
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
        std::env::var_os("PATH").map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join("nvidia-smi");
                p.is_file() || p.with_extension("exe").is_file()
            })
        }).unwrap_or(false)
    })
}

/// Run `cmd` capturing stdout, but never block longer than `timeout`
/// (plus a small residual grace) — if the child overruns it is killed
/// and `None` is returned.  A worker thread drains stdout so a chatty
/// child can't deadlock on a full pipe while the deadline is polled.
/// stdin is nulled so a child can never steal keystrokes from the
/// raw-mode TUI.  Returns `Some(stdout)` only on a clean zero-exit.
pub(crate) fn run_capped(cmd: std::process::Command, timeout: std::time::Duration)
    -> Option<Vec<u8>>
{
    run_capped_max(cmd, timeout, usize::MAX)
}

/// `run_capped` with a stdout size cap: the reader stops accepting
/// bytes past `max_bytes`, and an overrun returns `None` (the child
/// then blocks on the full pipe and is killed at the deadline).  Use
/// for shellouts whose output size is attacker-influenced.
pub(crate) fn run_capped_max(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn().ok()?;
    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout {
            // Read one byte past the cap so an overrun is detectable.
            let take = (max_bytes as u64).saturating_add(1);
            let _ = out.take(take).read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();   // closes the pipe → reader hits EOF
                    reap_bounded(child);
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => { reap_bounded(child); break None; }
        }
    };
    // Residual wait for the pipe to reach EOF, bounded: a descendant of
    // the (possibly killed) child that inherited the stdout write end
    // would otherwise hold the pipe open forever and `recv()` with it.
    let residual = timeout
        .saturating_sub(start.elapsed())
        .max(Duration::from_millis(50));
    let buf = match rx.recv_timeout(residual) {
        Ok(b) => { let _ = reader.join(); b }
        // Pipe still open past the deadline — abandon the reader thread
        // (it exits on its own at EOF) rather than blocking the tick.
        Err(_) => return None,
    };
    match status {
        Some(s) if s.success() && buf.len() <= max_bytes => Some(buf),
        _ => None,
    }
}

/// Reap a killed/failed child without ever blocking indefinitely: a
/// process stuck in uninterruptible sleep (wedged driver ioctl) leaves
/// even SIGKILL pending, and a plain `wait()` would stall the collector
/// with it.  Poll briefly, then hand the zombie to a detached reaper.
fn reap_bounded(mut child: std::process::Child) {
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    std::thread::spawn(move || { let _ = child.wait(); });
}

/// Patterns in a process cmdline that indicate elevated / "god mode" agent
/// permissions — `--dangerously-skip-permissions`, `--yolo`, `--no-permissions`,
/// `--allow-dangerously-…`.  The collector flags these so the TUI can pulsate
/// the row.
/// Public re-export for sysbackend.rs which needs to compute dangerous-ness
/// without the collector context.
pub fn is_dangerous_for_cmdline(s: &str) -> bool { is_dangerous_invocation(s) }

/// Identify the *specific* dangerous flag in a cmdline so the TUI can
/// surface it (rather than just a generic "GOD" marker).  Returns the
/// matched substring or empty when the cmdline is benign.
pub fn dangerous_flag_for_cmdline(cmdline: &str) -> String {
    let s = cmdline.to_ascii_lowercase();
    for pat in [
        "--dangerously-skip-permissions",
        "--dangerously",
        "--no-permission-prompt",
        "--no-permissions",
        "--allow-dangerously-",
        "--allow-dangerous",
        "--yolo",
    ] {
        if s.contains(pat) { return pat.to_string(); }
    }
    if s.starts_with("sudo claude") || s.contains(" sudo claude") {
        return "sudo claude".into();
    }
    if s.starts_with("sudo codex") || s.contains(" sudo codex") {
        return "sudo codex".into();
    }
    String::new()
}

fn is_dangerous_invocation(cmdline: &str) -> bool {
    !dangerous_flag_for_cmdline(cmdline).is_empty()
}
use crate::matchers::{builtin, classify, Matcher, UserMatcher};
use crate::model::{
    ActivityEvent, ActivityKind, Agent, Aggregates, History, ProjectAgg, Snapshot, Status,
};
use crate::proc_;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// System-wide history depth.  Sized to fill the CPU chart in any
/// realistic terminal width (incl. ultrawides / fullscreen
/// 240-col terminals × ratatui's 1-bar-per-sample rendering) so a
/// live resize doesn't expose a hard right-edge where the data
/// stops.  At 8 bytes per f64 × 6 history series, the in-memory
/// cost is ~12 KiB — trivial.  Renderers slice the most recent
/// `area_width` samples from this so older data falls off the
/// left as new ticks arrive.
const HISTORY: usize = 240;
const MAX_ACTIVITY: usize = 300;

/// Anthropic / OpenAI / Google publish their context windows at
/// these standard sizes.  Used to round an observed-larger-than-table
/// prompt up to the next published window so the bar reads cleanly
/// (e.g. 515k observed → snap to 1M, not 541k).
const STANDARD_WINDOWS: &[u64] = &[
    128_000, 200_000, 256_000, 400_000, 1_000_000, 2_000_000,
];

pub struct Collector {
    builtins: Vec<Matcher>,
    user: Vec<UserMatcher>,
    /// Cached at construction so the `snapshot` path can't see a different
    /// answer than the constructor used when deciding whether to set up the
    /// sysinfo backend.  Without this, a TOCTOU between `is_linux()` calls
    /// would panic the `expect` on `self.sys` access.
    use_sysinfo: bool,
    prev: HashMap<u32, PrevCpu>,
    prev_total: u64,
    cpu_smooth: HashMap<u32, f64>,
    /// Per-pid CPU% history for the inline sparkline column.
    agent_cpu_hist: HashMap<u32, VecDeque<f64>>,
    /// Per-agent rolling history of token deltas (tokens consumed
    /// since the previous snapshot).  Powers the per-agent token-rate
    /// sparkline in the detail popup.  `prev_tokens_per_pid` holds
    /// the previous total so we can compute deltas without storing
    /// every sample on the agent itself.
    agent_tokens_hist: HashMap<u32, VecDeque<f64>>,
    prev_tokens_per_pid: HashMap<u32, u64>,
    /// Per-pid (timestamp_ms, context_used) ring used to extrapolate
    /// time-to-compaction in the detail popup.  Same size cap as the
    /// CPU ring; entries are evicted when the pid exits.
    agent_ctx_hist: HashMap<u32, VecDeque<(u64, u64)>>,
    /// Per-pid (timestamp_ms, read_bytes, write_bytes) snapshot of
    /// the previous tick.  Drives the read_rate_bps / write_rate_bps
    /// fields on Agent.  Evicted on pid exit alongside the other
    /// per-pid maps.
    agent_io_prev: HashMap<u32, (u64, u64, u64)>,
    boot_time: u64,
    num_cpus: usize,
    known_pids: HashMap<u32, String>,
    activity: VecDeque<ActivityEvent>,
    history_total:        VecDeque<f64>,
    history_active:       VecDeque<f64>,
    history_busy:         VecDeque<f64>,
    history_cpu:          VecDeque<f64>,
    history_mem:          VecDeque<f64>,
    history_tokens_rate:  VecDeque<f64>,
    prev_tokens_total:    u64,
    /// Last GPU-usage table (pid → util%, vram) and a tick counter so we
    /// only re-query nvidia-smi every few ticks instead of every one —
    /// the query costs 30-60 ms and the numbers move slowly.
    gpu_cache: HashMap<u32, (f64, u64)>,
    gpu_tick: u32,
    pricing: PriceTable,
    sys: Option<SysBackend>,
}

/// Re-query GPU usage every Nth tick.  The nvidia-smi round-trip is the
/// single most expensive thing in the collector loop; VRAM/utilisation
/// changes slowly enough that a ~4-tick cadence is imperceptible.
const GPU_REFRESH_EVERY: u32 = 4;

const PER_AGENT_HISTORY: usize = 24;

struct PrevCpu {
    total: u64,
    /// Process start-time in clock ticks since boot, used to detect PID
    /// reuse: if a new pid lookup finds the same numeric pid but a
    /// different starttime, it's a different process and we must
    /// discard the previous CPU sample to avoid a wildly wrong delta.
    starttime: u64,
}

impl Collector {
    pub fn new(user: Vec<UserMatcher>, pricing: PriceTable) -> Self {
        // Hand the (possibly --prices-merged) table to the claude parser,
        // which prices per-model at parse time and needs more than the
        // last-seen model the collector sees.
        claude::install_price_table(pricing.clone());
        let use_sysinfo = !proc_::is_linux();
        let sys = if use_sysinfo { Some(SysBackend::new()) } else { None };
        // On Linux, normalise CPU% against the number of *online* CPUs
        // (the scope of /proc/stat's aggregate `cpu` line) rather than
        // `available_parallelism`, which honours scheduler affinity and
        // cgroup quota — under `taskset -c 0` or a 2-CPU container on a
        // 32-core host that would deflate every agent's CPU% by the
        // affinity/quota ratio.  `sys_cpus` uses the same value so the
        // header and the per-agent math stay consistent.
        let num_cpus = match sys.as_ref() {
            Some(s) => s.num_cpus(),
            None => {
                let online = proc_::online_cpu_count();
                if online > 0 { online } else { proc_::num_cpus() }
            }
        };
        Self {
            builtins: builtin(),
            user,
            use_sysinfo,
            prev: HashMap::new(),
            prev_total: 0,
            cpu_smooth: HashMap::new(),
            agent_cpu_hist: HashMap::new(),
            agent_tokens_hist: HashMap::new(),
            prev_tokens_per_pid: HashMap::new(),
            agent_ctx_hist: HashMap::new(),
            agent_io_prev:  HashMap::new(),
            boot_time: proc_::read_boot_time(),
            num_cpus,
            known_pids: HashMap::new(),
            activity: VecDeque::with_capacity(MAX_ACTIVITY),
            history_total:        VecDeque::with_capacity(HISTORY),
            history_active:       VecDeque::with_capacity(HISTORY),
            history_busy:         VecDeque::with_capacity(HISTORY),
            history_cpu:          VecDeque::with_capacity(HISTORY),
            history_mem:          VecDeque::with_capacity(HISTORY),
            history_tokens_rate:  VecDeque::with_capacity(HISTORY),
            prev_tokens_total:    0,
            gpu_cache: HashMap::new(),
            gpu_tick: 0,
            pricing,
            sys,
        }
    }

    /// Read-only access to the price table for the UI's cache-savings
    /// stat (which needs the per-model input rate to compute the
    /// dollars-saved-vs-uncached number).
    pub fn pricing(&self) -> &PriceTable { &self.pricing }

    /// Extrapolate seconds remaining until the agent's context-window
    /// hits 95% (Claude Code's auto-compaction trigger), based on the
    /// per-pid context-history ring.  Returns `None` when there's
    /// less than 3 samples or growth is non-positive.
    pub fn time_to_compaction_secs(&self, pid: u32, limit: u64) -> Option<u64> {
        let ring = self.agent_ctx_hist.get(&pid)?;
        if ring.len() < 3 || limit == 0 { return None; }
        let (t0, c0) = *ring.front()?;
        let (t1, c1) = *ring.back()?;
        if t1 <= t0 || c1 <= c0 { return None; }
        let dt_s   = (t1 - t0) as f64 / 1000.0;
        let dctx   = (c1 - c0) as f64;
        let target = (limit as f64) * 0.95;
        if (c1 as f64) >= target { return Some(0); }
        let rate   = dctx / dt_s;        // tokens / second
        if rate <= 0.0 { return None; }
        let need   = target - (c1 as f64);
        Some((need / rate) as u64)
    }

    /// Per-tick growth rate of the agent's context-window in tokens
    /// per minute, computed from the same ring.  Used to render the
    /// `+28k/min` annotation alongside the time-to-compaction line.
    pub fn context_growth_per_min(&self, pid: u32) -> Option<u64> {
        let ring = self.agent_ctx_hist.get(&pid)?;
        if ring.len() < 3 { return None; }
        let (t0, c0) = *ring.front()?;
        let (t1, c1) = *ring.back()?;
        if t1 <= t0 || c1 <= c0 { return None; }
        let dt_min = (t1 - t0) as f64 / 60_000.0;
        let dctx   = (c1 - c0) as f64;
        if dt_min <= 0.0 { return None; }
        Some((dctx / dt_min) as u64)
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);

        if self.use_sysinfo {
            return self.snapshot_via_sysinfo(now);
        }

        let total_cpu = proc_::read_system_cpu_total();
        let total_delta = total_cpu.saturating_sub(self.prev_total).max(1);
        let mem = proc_::read_meminfo();

        let mut agents: Vec<Agent> = Vec::new();
        let mut agg_cpu = 0.0f64;
        let mut agg_mem = 0u64;
        // /proc/<pid>/net/tcp{,6} is a netns-wide table, identical for
        // every pid sharing the namespace (the common case: all of them)
        // — parse it once per namespace per tick, not once per agent.
        let mut net_by_ns: HashMap<String, u32> = HashMap::new();

        for pid in proc_::list_pids() {
            let stat = match proc_::read_stat(pid) { Some(s) => s, None => continue };
            let cmdline = proc_::read_cmdline(pid);
            if cmdline.is_empty() { continue; }
            let label = match classify(&cmdline, &self.builtins, &self.user) {
                Some(l) => l.to_string(),
                None => continue,
            };

            let cwd_path: PathBuf = proc_::read_cwd(pid).unwrap_or_else(|| PathBuf::from("?"));
            let exe_path = proc_::read_exe(pid).unwrap_or_else(|| PathBuf::from("?"));
            let io = proc_::read_io(pid).unwrap_or_default();
            let writing = proc_::read_writing_files(pid, 4);

            let proc_total = stat.utime.saturating_add(stat.stime);
            // PID-reuse guard: only trust the previous sample if its
            // recorded starttime matches the current /proc/<pid>/stat.
            // Otherwise a recycled pid would produce a fictitious delta.
            let prev_total = self.prev.get(&pid)
                .filter(|p| p.starttime == stat.starttime)
                .map(|p| p.total);
            let cpu_raw = match prev_total {
                Some(pt) => {
                    let proc_delta = proc_total.saturating_sub(pt) as f64;
                    (proc_delta / total_delta as f64) * self.num_cpus as f64 * 100.0
                }
                None => 0.0,
            }.max(0.0);
            self.prev.insert(pid, PrevCpu { total: proc_total, starttime: stat.starttime });

            let smoothed = if prev_total.is_some() {
                match self.cpu_smooth.get(&pid) {
                    Some(prev) => prev * 0.6 + cpu_raw * 0.4,
                    None => cpu_raw,
                }
            } else {
                // Reset smoothing when we discarded the previous sample
                // (first sight or pid-reuse).
                cpu_raw
            };
            self.cpu_smooth.insert(pid, smoothed);

            let rss_bytes = stat.rss_pages.saturating_mul(proc_::page_size());
            let started_at_sec = self.boot_time
                .saturating_add(stat.starttime / proc_::CLK_TCK);
            let now_sec = now / 1000;
            // boot_time == 0 means /proc/stat was unreadable; skip the
            // uptime computation rather than reporting a multi-decade value.
            let uptime_sec = if self.boot_time == 0 { 0 } else { now_sec.saturating_sub(started_at_sec) };

            // Every /proc-derived string is attacker-controlled (any
            // local user can `exec -a $'\x1b]0;...'`), so we strip
            // ANSI / control bytes at the collector boundary before
            // anything reaches stdout (--once / --json / TUI).
            let cwd     = crate::format::sanitize_control(&cwd_path.to_string_lossy());
            let exe     = crate::format::sanitize_control(&exe_path.to_string_lossy());
            let cmdline = crate::format::sanitize_control(&cmdline);
            let project = derive_project(&cwd, &exe, &cmdline, &label);
            let writing_files: Vec<String> = writing.iter()
                .map(|p| crate::format::sanitize_control(&p.to_string_lossy())).collect();
            let writing_dirs: Vec<String> = dedupe(
                writing.iter()
                    .filter_map(|p| p.parent())
                    .map(|p| crate::format::sanitize_control(&p.to_string_lossy())),
            );

            let agent = Agent {
                pid,
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
                time_to_compaction_secs: None,
                ctx_growth_per_min: None,
                loaded_skills: Vec::new(),
                loaded_plugins: Vec::new(),
                tool_counts: Vec::new(),
                ppid_name: proc_::read_comm(stat.ppid)
                    .map(|s| crate::format::sanitize_control(&s))
                    .unwrap_or_default(),
                session_started_ms: 0,
                dangerous_flag: dangerous_flag_for_cmdline(&cmdline),
                model: None,
                dangerous: is_dangerous_invocation(&cmdline),
                in_flight_subagents: Vec::new(),
                recent_activity: Vec::new(),
                cpu_history: Vec::new(),
                tokens_history: Vec::new(),
                cpu: smoothed,
                cpu_raw,
                rss: rss_bytes,
                vsize: stat.vsize,
                threads: stat.num_threads,
                state: stat.state.to_string(),
                ppid: stat.ppid,
                uptime_sec,
                cwd,
                exe,
                cmdline,
                read_bytes: io.read_bytes,
                write_bytes: io.write_bytes,
                writing_files,
                writing_dirs,
                // Background-activity surface (Linux-only).  Reading
                // FDs / spawned children / open TCP conns are the
                // signals that explain CPU usage when no tokens are
                // visibly flowing.  Caps are tight so a process with
                // thousands of open files / children / sockets
                // doesn't dominate one snapshot tick.
                reading_files: proc_::read_reading_files(pid, 6).iter()
                    .map(|p| crate::format::sanitize_control(&p.to_string_lossy())).collect(),
                children: proc_::read_children(pid, 8).into_iter()
                    .map(|(p, c)| (p, crate::format::sanitize_control(&c)))
                    .collect(),
                net_established: net_established_by_ns(pid, &mut net_by_ns),
                read_rate_bps: 0,    // filled in below from the per-pid prev snapshot
                write_rate_bps: 0,
                gpu_pct: 0.0,
                gpu_mem_bytes: 0,
                host: String::new(),
            };

            agg_cpu += smoothed;
            agg_mem += rss_bytes;
            agents.push(agent);
        }

        // Update per-agent CPU history & attach a copy onto the agent struct.
        self.refresh_agent_cpu_history(&mut agents);

        // Spawn / exit events.
        let live_pids: std::collections::HashSet<u32> = agents.iter().map(|a| a.pid).collect();
        for a in &agents {
            if let std::collections::hash_map::Entry::Vacant(e) = self.known_pids.entry(a.pid) {
                e.insert(a.label.clone());
                self.push_activity(ActivityEvent {
                    t: now,
                    kind: ActivityKind::Spawn,
                    label: a.label.clone(),
                    pid: a.pid,
                    cwd: Some(a.cwd.clone()),
                });
            }
        }
        let exited: Vec<(u32, String)> = self.known_pids.iter()
            .filter(|(pid, _)| !live_pids.contains(pid))
            .map(|(pid, label)| (*pid, label.clone()))
            .collect();
        let to_remove: Vec<u32> = exited.iter().map(|(p, _)| *p).collect();
        for (pid, label) in exited {
            self.push_activity(ActivityEvent {
                t: now, kind: ActivityKind::Exit,
                label, pid, cwd: None,
            });
        }
        for pid in &to_remove {
            self.known_pids.remove(pid);
            self.prev.remove(pid);
            self.cpu_smooth.remove(pid);
            self.agent_ctx_hist.remove(pid);
            self.agent_io_prev.remove(pid);
        }

        self.prev_total = total_cpu;

        let sessions = self.enrich_and_score(&mut agents, now);

        // Stable sort: status > project > cpu > rss > pid.
        agents.sort_by(|a, b| {
            a.status.rank().cmp(&b.status.rank())
                .then_with(|| a.project.cmp(&b.project))
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| b.rss.cmp(&a.rss))
                .then_with(|| a.pid.cmp(&b.pid))
        });

        // Per-project aggregates.
        let mut by_proj: HashMap<String, ProjectAgg> = HashMap::new();
        for a in &agents {
            let row = by_proj.entry(a.project.clone()).or_insert_with(|| ProjectAgg {
                project: a.project.clone(),
                cwd: a.cwd.clone(),
                ..Default::default()
            });
            row.agents += 1;
            row.cpu += a.cpu;
            row.rss += a.rss;
            row.subagents += a.subagents;
            row.tokens_total += a.tokens_total;
            row.cost_usd += a.cost_usd;
            *row.statuses.entry(status_key(a.status)).or_insert(0) += 1;
        }
        let mut projects: Vec<ProjectAgg> = by_proj.into_values().collect();
        projects.sort_by(|a, b| {
            let a_busy = *a.statuses.get("busy").unwrap_or(&0);
            let b_busy = *b.statuses.get("busy").unwrap_or(&0);
            b_busy.cmp(&a_busy)
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.project.cmp(&b.project))
        });

        let busy_count = agents.iter().filter(|a| matches!(a.status, Status::Busy | Status::Spawning)).count() as u32;
        let subagents_total: u32 = agents.iter().map(|a| a.subagents).sum();
        // Saturating sums — defensive against pathological cumulative
        // session counters; real totals never come close to u64::MAX.
        let tokens_input_total:  u64 = agents.iter().map(|a| a.tokens_input).fold(0u64, u64::saturating_add);
        let tokens_output_total: u64 = agents.iter().map(|a| a.tokens_output).fold(0u64, u64::saturating_add);
        let tokens_grand_total = tokens_input_total.saturating_add(tokens_output_total);
        let cost_grand_total: f64 = agents.iter().map(|a| a.cost_usd).sum();

        push_bounded(&mut self.history_total,  agents.len() as f64, HISTORY);
        push_bounded(&mut self.history_active, agents.len() as f64 + sessions.waiting as f64, HISTORY);
        push_bounded(&mut self.history_busy,   busy_count as f64, HISTORY);
        push_bounded(&mut self.history_cpu,    (agg_cpu * 10.0).round() / 10.0, HISTORY);
        push_bounded(&mut self.history_mem,    ((agg_mem as f64 / 1_048_576.0) * 10.0).round() / 10.0, HISTORY);
        // Token rate = tokens added since last tick. First tick yields 0
        // because we don't yet have a baseline.
        let tokens_delta = if self.prev_tokens_total == 0 {
            0.0
        } else {
            tokens_grand_total.saturating_sub(self.prev_tokens_total) as f64
        };
        self.prev_tokens_total = tokens_grand_total;
        push_bounded(&mut self.history_tokens_rate, tokens_delta, HISTORY);

        let project_count = projects.len() as u32;
        Snapshot {
            now,
            platform: "linux".into(),
            note: None,
            sys_cpus: self.num_cpus as u32,
            mem_total: mem.total,
            mem_available: mem.available,
            aggregates: Aggregates {
                cpu: agg_cpu,
                mem_bytes: agg_mem,
                active: agents.len() as u32,
                busy: busy_count,
                waiting: sessions.waiting,
                completed: sessions.completed,
                subagents: subagents_total,
                project_count,
                tokens_total:  tokens_grand_total,
                tokens_input:  tokens_input_total,
                tokens_output: tokens_output_total,
                cost_usd: cost_grand_total,
            },
            agents,
            projects,
            sessions,
            history: History {
                total:       self.history_total.iter().copied().collect(),
                active:      self.history_active.iter().copied().collect(),
                busy:        self.history_busy.iter().copied().collect(),
                cpu:         self.history_cpu.iter().copied().collect(),
                mem:         self.history_mem.iter().copied().collect(),
                tokens_rate: self.history_tokens_rate.iter().copied().collect(),
            },
            activity: self.activity.iter().rev().take(80).cloned().collect(),
        }
    }

    fn push_activity(&mut self, e: ActivityEvent) {
        if self.activity.len() >= MAX_ACTIVITY { self.activity.pop_front(); }
        self.activity.push_back(e);
    }

    /// Enriches `agents` in-place with vendor session info, applies the
    /// universal CPU% override, fills in cost from the price table, and
    /// returns the merged sessions block ready to put on the snapshot.
    fn enrich_and_score(&mut self, agents: &mut [Agent], now: u64) -> crate::model::Sessions {
        // GPU usage table.  Only re-query nvidia-smi when there is at
        // least one agent to attribute usage to, and only every
        // GPU_REFRESH_EVERY-th tick — the query is the priciest thing in
        // the loop and the numbers move slowly.  Between refreshes we
        // reuse the cached table.  On non-NVIDIA hosts `read_gpu_usage`
        // short-circuits without spawning anything.
        if !agents.is_empty() && self.gpu_tick % GPU_REFRESH_EVERY == 0 {
            self.gpu_cache = read_gpu_usage();
        }
        self.gpu_tick = self.gpu_tick.wrapping_add(1);
        let gpu_by_pid = &self.gpu_cache;

        // Compute per-pid IO rates against the previous tick's
        // (read_bytes, write_bytes, ts).  First sample for any pid
        // returns 0; subsequent samples produce bytes/sec.
        for a in agents.iter_mut() {
            if let Some(prev) = self.agent_io_prev.get(&a.pid) {
                let dt_ms = now.saturating_sub(prev.0);
                if dt_ms > 0 {
                    let dr = a.read_bytes.saturating_sub(prev.1);
                    let dw = a.write_bytes.saturating_sub(prev.2);
                    a.read_rate_bps  = (dr as u128 * 1000 / dt_ms as u128) as u64;
                    a.write_rate_bps = (dw as u128 * 1000 / dt_ms as u128) as u64;
                }
            }
            self.agent_io_prev.insert(a.pid, (now, a.read_bytes, a.write_bytes));
            if let Some((pct, mem)) = gpu_by_pid.get(&a.pid) {
                a.gpu_pct = *pct;
                a.gpu_mem_bytes = *mem;
            }
        }

        // Claude Code skills/plugins are re-derived per Claude row below.
        // Plugin enumeration is host-global (identical for every Claude
        // agent in a snapshot) and skills are cwd-scoped, so memoize both
        // for the duration of this tick instead of re-reading settings.json
        // and re-scanning skill dirs once per Claude agent.
        let mut plugins_cache: Option<Vec<String>> = None;
        let mut skills_cache: HashMap<String, Vec<String>> = HashMap::new();

        let live_refs: Vec<LiveAgentRef> = agents.iter()
            .map(|a| LiveAgentRef {
                pid: a.pid,
                cwd: a.cwd.as_str(),
                label: a.label.as_str(),
                uptime_sec: a.uptime_sec,
            })
            .collect();
        let merged = sessions::merge(vec![
            claude::summarise(&live_refs, now),
            codex::summarise(&live_refs, now),
            goose::summarise(&live_refs, now),
            gemini::summarise(&live_refs, now),
            aider::summarise(&live_refs, now),
            generic::summarise(agents, &live_refs, now),
        ]);

        for a in agents.iter_mut() {
            if let Some(s) = merged.by_pid.get(&a.pid) {
                a.status = s.status;
                a.current_tool = s.current_tool.clone();
                a.current_task = s.last_task.clone();
                a.subagents = s.in_flight_tasks;
                a.session_id = Some(s.id.clone());
                a.session_age_ms = Some(s.age_ms);
                a.tokens_input       = s.tokens_input;
                a.tokens_output      = s.tokens_output;
                a.tokens_total       = s.tokens_total;
                a.tokens_cache_read  = s.tokens_cache_read;
                a.tokens_cache_write = s.tokens_cache_write;
                a.cost_usd           = s.cost_usd;
                a.session_started_ms = s.session_started_ms;
                a.tool_counts        = s.tool_counts.clone();
                a.model = s.model.clone();
                a.in_flight_subagents = s.in_flight_subagents.clone();
                a.recent_activity = s.recent_activity.clone();
            } else {
                a.status = Status::Idle;
            }
            // Universal CPU% override.  Threshold calibrated against
            // observed Claude / Codex Node-process CPU during real turns
            // (5–15% is typical mid-turn on a modern CPU).
            if a.cpu >= 10.0 { a.status = Status::Busy; }
            else if a.cpu >= 3.0 && matches!(a.status, Status::Idle | Status::Stale) {
                a.status = Status::Active;
            }
            // Cost.  Always classify the basis (api / local / unknown)
            // so the UI can label local-runtime rows as `local` instead
            // of pretending they're free API calls.  Use the
            // cache-aware variant so prompt-cached tokens are billed
            // at Anthropic's discounted rate (0.1× for reads, 1.25×
            // for writes) instead of full input rate.
            if let Some(model) = &a.model {
                // Vendors that price at parse time (claude's per-model
                // accumulation) already set cost_usd via the session copy
                // above; recompute only when the parse side left it unset,
                // otherwise last-seen-model pricing would clobber the
                // mixed-model sum.
                if a.cost_usd == 0.0 {
                    a.cost_usd = self.pricing.cost_with_cache(
                        model, a.tokens_input, a.tokens_output,
                        a.tokens_cache_read, a.tokens_cache_write,
                    );
                }
                a.cost_basis = match crate::pricing::cost_basis(&self.pricing, model) {
                    crate::pricing::CostBasis::Api     => "api".into(),
                    crate::pricing::CostBasis::Local   => "local".into(),
                    crate::pricing::CostBasis::Unknown => "unknown".into(),
                };
                a.context_limit = self.pricing.context_limit(model);
            }
            // Context-window usage from the latest assistant turn (set
            // by the vendor enrichers above).  Defaults to 0 when
            // unknown.
            if let Some(s) = merged.by_pid.get(&a.pid) {
                a.context_used = s.context_used;
            }
            // Self-calibrating limit: if the observed prompt size on the
            // latest turn exceeds the table-derived limit, the model
            // must be running with a larger window than the table knew
            // about (e.g. an undeclared 1M-context variant).  Promote
            // the limit to the next standard window-size that contains
            // the observed value, with 5% headroom.  Prevents the
            // popup from ever displaying a >100% fill.
            if a.context_used > a.context_limit {
                let need = (a.context_used as f64 * 1.05) as u64;
                a.context_limit = STANDARD_WINDOWS.iter().copied()
                    .find(|w| *w >= need)
                    .unwrap_or(need);
            }
            // Per-pid context history — push the current sample, cap
            // at PER_AGENT_HISTORY entries.  Used by the popup's
            // time-to-compaction estimator (see `extrapolate_compaction`).
            if a.context_used > 0 {
                let ring = self.agent_ctx_hist.entry(a.pid).or_default();
                ring.push_back((now, a.context_used));
                while ring.len() > PER_AGENT_HISTORY { ring.pop_front(); }
            }
            // Fold the compaction extrapolation into the agent now, while we
            // still hold the collector, so the render side (which runs on a
            // different thread once collection is off the UI thread) needs no
            // live handle back to the collector's per-pid history.
            a.time_to_compaction_secs = self.time_to_compaction_secs(a.pid, a.context_limit);
            a.ctx_growth_per_min = self.context_growth_per_min(a.pid);
            // Claude Code skills loaded for this session — scan the
            // project-local + user-global skill roots.  npm-installed
            // Claude Code classifies as "claude-code", not "claude"
            // (the scoped-path matcher), and must get skills/plugins too.
            // Memoized per-tick (see caches above) so N Claude rows don't
            // trigger N readdirs + N settings.json parses.
            if a.label == "claude" || a.label == "claude-code" {
                a.loaded_skills = skills_cache.entry(a.cwd.clone())
                    .or_insert_with(|| crate::skills::skills_for_cwd(&a.cwd))
                    .clone();
                a.loaded_plugins = plugins_cache
                    .get_or_insert_with(crate::plugins::enabled_plugins)
                    .clone();
            }
        }

        // Per-pid token-rate history — must run now that a.tokens_total
        // has been populated by the enrichment loop above.
        self.refresh_agent_token_history(agents);

        // Mutate session entries inline so JSON output carries cost too.
        // Use the cache-aware variant so a session's cache-read tokens
        // (typically 90%+ of the input bucket) are billed at 0.1× rather
        // than full input rate — otherwise the sessions pane / --json
        // reported ~5-10× the per-agent cost for the same session.
        let mut sessions_block = merged.sessions;
        for s in sessions_block.sessions.iter_mut() {
            // Skip sessions already priced per-model at parse time —
            // recomputing from the grand totals with the last-seen model
            // would undo the mixed-model accounting.
            if s.cost_usd != 0.0 { continue; }
            if let Some(model) = &s.model {
                s.cost_usd = self.pricing.cost_with_cache(
                    model, s.tokens_input, s.tokens_output,
                    s.tokens_cache_read, s.tokens_cache_write,
                );
            }
        }
        sessions_block
    }

    fn refresh_agent_cpu_history(&mut self, agents: &mut [Agent]) {
        let live: std::collections::HashSet<u32> = agents.iter().map(|a| a.pid).collect();
        for a in agents.iter_mut() {
            let entry = self.agent_cpu_hist.entry(a.pid)
                .or_insert_with(|| VecDeque::with_capacity(PER_AGENT_HISTORY));
            if entry.len() >= PER_AGENT_HISTORY { entry.pop_front(); }
            entry.push_back(a.cpu);
            a.cpu_history = entry.iter().copied().collect();
        }
        // Drop entries for processes that disappeared.
        self.agent_cpu_hist.retain(|pid, _| live.contains(pid));
    }

    /// Per-pid token-rate history for the detail popup sparkline.  Must
    /// run *after* `enrich_and_score` has populated `a.tokens_total` —
    /// running it in `refresh_agent_cpu_history` (which executes before
    /// enrichment) computed every delta against a zero total, leaving the
    /// history permanently flat and the sparkline dead.
    fn refresh_agent_token_history(&mut self, agents: &mut [Agent]) {
        let live: std::collections::HashSet<u32> = agents.iter().map(|a| a.pid).collect();
        for a in agents.iter_mut() {
            // Delta vs the previous total for this pid.  First observation
            // seeds with 0 so the sparkline doesn't spike on the initial
            // sample.  saturating_sub defends against vendor enrichers
            // that occasionally re-emit a smaller running total (Codex
            // resumes a session; the header total is the new session
            // only).
            let delta = match self.prev_tokens_per_pid.get(&a.pid).copied() {
                Some(p) => a.tokens_total.saturating_sub(p) as f64,
                None    => 0.0,
            };
            self.prev_tokens_per_pid.insert(a.pid, a.tokens_total);
            let tok_entry = self.agent_tokens_hist.entry(a.pid)
                .or_insert_with(|| VecDeque::with_capacity(PER_AGENT_HISTORY));
            if tok_entry.len() >= PER_AGENT_HISTORY { tok_entry.pop_front(); }
            tok_entry.push_back(delta);
            a.tokens_history = tok_entry.iter().copied().collect();
        }
        self.agent_tokens_hist.retain(|pid, _| live.contains(pid));
        self.prev_tokens_per_pid.retain(|pid, _| live.contains(pid));
    }

    /// macOS / *BSD / Windows path: lean on sysinfo for process metadata.
    /// Session enrichment, sorting, charts, and aggregates work identically.
    fn snapshot_via_sysinfo(&mut self, now: u64) -> Snapshot {
        // self.use_sysinfo guarantees self.sys is Some by construction; no
        // unwrap/expect needed because the constructor populates them
        // together and there's no public API to mutate them apart.
        let sys = match self.sys.as_mut() {
            Some(s) => s,
            None => return Snapshot {
                now, platform: std::env::consts::OS.into(),
                note: Some("sysinfo backend not initialised".into()),
                ..Default::default()
            },
        };
        sys.refresh();
        let mut agents = sys.collect_agents(&self.builtins, &self.user);

        // Windows-only: pull live Linux agents from every running WSL
        // distro and append.  PIDs are pre-namespaced (top bit set,
        // distro idx in next 7 bits) inside wsl_backend so the
        // per-pid HashMaps below don't collide between Windows and
        // Linux PIDs that happen to share a numeric value.  CPU% is
        // computed by the same prev-vs-current delta logic the
        // Windows-native path uses.
        #[cfg(windows)]
        {
            agents.extend(crate::wsl_backend::collect(&self.builtins, &self.user));
        }

        // Spawn / exit events.
        let live_pids: std::collections::HashSet<u32> = agents.iter().map(|a| a.pid).collect();
        for a in &agents {
            if let std::collections::hash_map::Entry::Vacant(e) = self.known_pids.entry(a.pid) {
                e.insert(a.label.clone());
                self.push_activity(ActivityEvent {
                    t: now, kind: ActivityKind::Spawn,
                    label: a.label.clone(), pid: a.pid, cwd: Some(a.cwd.clone()),
                });
            }
        }
        let exited: Vec<(u32, String)> = self.known_pids.iter()
            .filter(|(p, _)| !live_pids.contains(p))
            .map(|(p, l)| (*p, l.clone())).collect();
        for (pid, label) in &exited {
            self.push_activity(ActivityEvent { t: now, kind: ActivityKind::Exit,
                label: label.clone(), pid: *pid, cwd: None });
            self.known_pids.remove(pid);
            self.cpu_smooth.remove(pid);
            self.agent_ctx_hist.remove(pid);
            self.agent_io_prev.remove(pid);
        }
        if self.activity.len() > MAX_ACTIVITY {
            let drop = self.activity.len() - MAX_ACTIVITY;
            self.activity.drain(0..drop);
        }

        self.refresh_agent_cpu_history(&mut agents);
        let sessions = self.enrich_and_score(&mut agents, now);

        let mut agg_cpu = 0.0;
        let mut agg_mem = 0u64;
        for a in &agents {
            agg_cpu += a.cpu;
            agg_mem += a.rss;
        }

        agents.sort_by(|a, b| {
            a.status.rank().cmp(&b.status.rank())
                .then_with(|| a.project.cmp(&b.project))
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| b.rss.cmp(&a.rss))
                .then_with(|| a.pid.cmp(&b.pid))
        });

        let mut by_proj: HashMap<String, ProjectAgg> = HashMap::new();
        for a in &agents {
            let row = by_proj.entry(a.project.clone()).or_insert_with(|| ProjectAgg {
                project: a.project.clone(), cwd: a.cwd.clone(), ..Default::default()
            });
            row.agents += 1;
            row.cpu += a.cpu;
            row.rss += a.rss;
            row.subagents += a.subagents;
            row.tokens_total += a.tokens_total;
            row.cost_usd += a.cost_usd;
            *row.statuses.entry(status_key(a.status)).or_insert(0) += 1;
        }
        let mut projects: Vec<ProjectAgg> = by_proj.into_values().collect();
        projects.sort_by(|a, b| {
            let a_busy = *a.statuses.get("busy").unwrap_or(&0);
            let b_busy = *b.statuses.get("busy").unwrap_or(&0);
            b_busy.cmp(&a_busy)
                .then_with(|| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.project.cmp(&b.project))
        });

        let busy_count = agents.iter().filter(|a| matches!(a.status, Status::Busy | Status::Spawning)).count() as u32;
        let subagents_total: u32 = agents.iter().map(|a| a.subagents).sum();
        // Saturating sums — defensive against pathological cumulative
        // session counters; real totals never come close to u64::MAX.
        let tokens_input_total:  u64 = agents.iter().map(|a| a.tokens_input).fold(0u64, u64::saturating_add);
        let tokens_output_total: u64 = agents.iter().map(|a| a.tokens_output).fold(0u64, u64::saturating_add);
        let tokens_grand_total = tokens_input_total.saturating_add(tokens_output_total);
        let cost_grand_total: f64 = agents.iter().map(|a| a.cost_usd).sum();

        push_bounded(&mut self.history_total,  agents.len() as f64, HISTORY);
        push_bounded(&mut self.history_active, agents.len() as f64 + sessions.waiting as f64, HISTORY);
        push_bounded(&mut self.history_busy,   busy_count as f64, HISTORY);
        push_bounded(&mut self.history_cpu,    (agg_cpu * 10.0).round() / 10.0, HISTORY);
        push_bounded(&mut self.history_mem,    ((agg_mem as f64 / 1_048_576.0) * 10.0).round() / 10.0, HISTORY);
        let tokens_delta = if self.prev_tokens_total == 0 { 0.0 }
                           else { tokens_grand_total.saturating_sub(self.prev_tokens_total) as f64 };
        self.prev_tokens_total = tokens_grand_total;
        push_bounded(&mut self.history_tokens_rate, tokens_delta, HISTORY);

        let project_count = projects.len() as u32;
        Snapshot {
            now,
            platform: std::env::consts::OS.to_string(),
            // sysinfo backend covers everything; writable-FD
            // enumeration is now native on macOS (libSystem FFI),
            // Windows (NtQuerySystemInformation), and FreeBSD
            // (libprocstat).  Only OpenBSD / NetBSD lack it
            // (kernel doesn't track per-fd paths).  Hide the note
            // entirely on platforms with full coverage.
            note: if cfg!(any(target_os = "openbsd", target_os = "netbsd")) {
                Some("running via sysinfo backend — writing-files unavailable on this OS".into())
            } else { None },
            sys_cpus: self.num_cpus as u32,
            mem_total: self.sys.as_ref().map(|s| s.total_memory()).unwrap_or(0),
            mem_available: self.sys.as_ref().map(|s| s.available_memory()).unwrap_or(0),
            aggregates: Aggregates {
                cpu: agg_cpu, mem_bytes: agg_mem,
                active: agents.len() as u32, busy: busy_count,
                waiting: sessions.waiting, completed: sessions.completed,
                subagents: subagents_total, project_count,
                tokens_total: tokens_grand_total,
                tokens_input: tokens_input_total,
                tokens_output: tokens_output_total,
                cost_usd: cost_grand_total,
            },
            agents, projects, sessions,
            history: History {
                total:       self.history_total.iter().copied().collect(),
                active:      self.history_active.iter().copied().collect(),
                busy:        self.history_busy.iter().copied().collect(),
                cpu:         self.history_cpu.iter().copied().collect(),
                mem:         self.history_mem.iter().copied().collect(),
                tokens_rate: self.history_tokens_rate.iter().copied().collect(),
            },
            activity: self.activity.iter().rev().take(80).cloned().collect(),
        }
    }
}

fn status_key(s: Status) -> &'static str {
    match s {
        Status::Busy => "busy",
        Status::Spawning => "spawning",
        Status::Active => "active",
        Status::Idle => "idle",
        Status::Waiting => "waiting",
        Status::Completed => "completed",
        Status::Stale => "stale",
    }
}

fn push_bounded(v: &mut VecDeque<f64>, x: f64, max: usize) {
    if v.len() >= max { v.pop_front(); }
    v.push_back(x);
}

/// Established-connection count for `pid`, memoized per network
/// namespace within one snapshot.  Keyed on the `/proc/<pid>/ns/net`
/// symlink target (`net:[inode]`); a pid whose ns link is unreadable
/// gets a private key so it still resolves without polluting the cache.
fn net_established_by_ns(pid: u32, cache: &mut HashMap<String, u32>) -> u32 {
    let key = std::fs::read_link(format!("/proc/{pid}/ns/net"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("pid:{pid}"));
    *cache.entry(key).or_insert_with(|| proc_::count_net_established(pid))
}

fn dedupe(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for x in it {
        if x.is_empty() { continue; }
        if seen.insert(x.clone()) {
            out.push(x);
        }
    }
    out
}

#[cfg(all(test, unix))]
mod run_capped_tests {
    use super::{run_capped, run_capped_max};
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn returns_stdout_on_clean_exit() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "printf hi"]);
        assert_eq!(run_capped(cmd, Duration::from_secs(5)), Some(b"hi".to_vec()));
    }

    #[test]
    fn kills_overrunning_child_within_deadline() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30"]);
        let t0 = Instant::now();
        assert_eq!(run_capped(cmd, Duration::from_millis(200)), None);
        // Deadline + residual grace + reap poll, with generous CI slack.
        assert!(t0.elapsed() < Duration::from_secs(3),
            "run_capped blocked past its deadline: {:?}", t0.elapsed());
    }

    #[test]
    fn descendant_holding_pipe_cannot_block_past_deadline() {
        // The shell exits immediately but its backgrounded child inherits
        // the stdout write end, so the pipe never reaches EOF — the
        // residual wait must give up instead of recv()ing forever.
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "sleep 30 & exit 0"]);
        let t0 = Instant::now();
        assert_eq!(run_capped(cmd, Duration::from_millis(300)), None);
        assert!(t0.elapsed() < Duration::from_secs(3),
            "residual pipe wait blocked past the deadline: {:?}", t0.elapsed());
    }

    #[test]
    fn output_over_cap_is_rejected() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "printf '0123456789'"]);
        assert_eq!(run_capped_max(cmd, Duration::from_secs(5), 4), None);
        let mut ok = Command::new("/bin/sh");
        ok.args(["-c", "printf '0123'"]);
        assert_eq!(run_capped_max(ok, Duration::from_secs(5), 4), Some(b"0123".to_vec()));
    }
}
