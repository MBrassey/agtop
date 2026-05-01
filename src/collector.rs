// Stitches /proc, matchers, and ~/.claude/projects into a single Snapshot.
// Holds smoothing state across snapshots so the TUI doesn't jitter.

use crate::sessions::{self, LiveAgentRef};
use crate::{aider, claude, codex, gemini, generic, goose};
use crate::format::derive_project;
use crate::pricing::PriceTable;
use crate::sysbackend::SysBackend;

/// Patterns in a process cmdline that indicate elevated / "god mode" agent
/// permissions — `--dangerously-skip-permissions`, `--yolo`, `--no-permissions`,
/// `--allow-dangerously-…`.  The collector flags these so the TUI can pulsate
/// the row.
/// Public re-export for sysbackend.rs which needs to compute dangerous-ness
/// without the collector context.
pub fn is_dangerous_for_cmdline(s: &str) -> bool { is_dangerous_invocation(s) }

fn is_dangerous_invocation(cmdline: &str) -> bool {
    let s = cmdline.to_ascii_lowercase();
    s.contains("--dangerously")
        || s.contains("--no-permissions")
        || s.contains("--no-permission-prompt")
        || s.contains("--allow-dangerous")
        || s.contains("--yolo")
        || s.starts_with("sudo claude") || s.contains(" sudo claude")
        || s.starts_with("sudo codex")  || s.contains(" sudo codex")
}
use crate::matchers::{builtin, classify, Matcher, UserMatcher};
use crate::model::{
    ActivityEvent, ActivityKind, Agent, Aggregates, History, ProjectAgg, Snapshot, Status,
};
use crate::proc_;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY: usize = 60;
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
    pricing: PriceTable,
    sys: Option<SysBackend>,
}

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
        let use_sysinfo = !proc_::is_linux();
        let sys = if use_sysinfo { Some(SysBackend::new()) } else { None };
        let num_cpus = sys.as_ref().map(|s| s.num_cpus()).unwrap_or_else(proc_::num_cpus);
        Self {
            builtins: builtin(),
            user,
            use_sysinfo,
            prev: HashMap::new(),
            prev_total: 0,
            cpu_smooth: HashMap::new(),
            agent_cpu_hist: HashMap::new(),
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
            pricing,
            sys,
        }
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

            let rss_bytes = stat.rss_pages * proc_::PAGE_SIZE;
            let started_at_sec = self.boot_time
                .saturating_add(stat.starttime / proc_::CLK_TCK);
            let now_sec = now / 1000;
            // boot_time == 0 means /proc/stat was unreadable; skip the
            // uptime computation rather than reporting a multi-decade value.
            let uptime_sec = if self.boot_time == 0 { 0 } else { now_sec.saturating_sub(started_at_sec) };

            let cwd = cwd_path.to_string_lossy().into_owned();
            let exe = exe_path.to_string_lossy().into_owned();
            let project = derive_project(&cwd, &exe, &cmdline, &label);
            let writing_files: Vec<String> = writing.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let writing_dirs: Vec<String> = dedupe(
                writing.iter()
                    .filter_map(|p| p.parent())
                    .map(|p| p.to_string_lossy().into_owned()),
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
                loaded_skills: Vec::new(),
                model: None,
                dangerous: is_dangerous_invocation(&cmdline),
                in_flight_subagents: Vec::new(),
                recent_activity: Vec::new(),
                cpu_history: Vec::new(),
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
    fn enrich_and_score(&self, agents: &mut [Agent], now: u64) -> crate::model::Sessions {
        let live_refs: Vec<LiveAgentRef> = agents.iter()
            .map(|a| LiveAgentRef { pid: a.pid, cwd: a.cwd.as_str(), label: a.label.as_str() })
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
                a.cost_usd = self.pricing.cost_with_cache(
                    model, a.tokens_input, a.tokens_output,
                    a.tokens_cache_read, a.tokens_cache_write,
                );
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
            // Claude Code skills loaded for this session — scan the
            // project-local + user-global skill roots.  Cheap (one
            // readdir per scope, no recursion) so safe to do every
            // tick; also gracefully empty for non-Claude vendors.
            if a.label == "claude" {
                a.loaded_skills = crate::skills::skills_for_cwd(&a.cwd);
            }
        }

        // Mutate session entries inline so JSON output carries cost too.
        let mut sessions_block = merged.sessions;
        for s in sessions_block.sessions.iter_mut() {
            if let Some(model) = &s.model {
                s.cost_usd = self.pricing.cost(model, s.tokens_input, s.tokens_output);
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
