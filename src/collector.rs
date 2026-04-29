// Stitches /proc, matchers, and ~/.claude/projects into a single Snapshot.
// Holds smoothing state across snapshots so the TUI doesn't jitter.

use crate::sessions::{self, LiveAgentRef};
use crate::{claude, codex, generic};
use crate::format::project_basename;
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

pub struct Collector {
    builtins: Vec<Matcher>,
    user: Vec<UserMatcher>,
    prev: HashMap<u32, PrevCpu>,
    prev_total: u64,
    cpu_smooth: HashMap<u32, f64>,
    boot_time: u64,
    num_cpus: usize,
    known_pids: HashMap<u32, String>,
    activity: VecDeque<ActivityEvent>,
    history_total:  VecDeque<f64>,
    history_active: VecDeque<f64>,
    history_busy:   VecDeque<f64>,
    history_cpu:    VecDeque<f64>,
    history_mem:    VecDeque<f64>,
}

struct PrevCpu {
    total: u64,
}

impl Collector {
    pub fn new(user: Vec<UserMatcher>) -> Self {
        Self {
            builtins: builtin(),
            user,
            prev: HashMap::new(),
            prev_total: 0,
            cpu_smooth: HashMap::new(),
            boot_time: proc_::read_boot_time(),
            num_cpus: proc_::num_cpus(),
            known_pids: HashMap::new(),
            activity: VecDeque::with_capacity(MAX_ACTIVITY),
            history_total:  VecDeque::with_capacity(HISTORY),
            history_active: VecDeque::with_capacity(HISTORY),
            history_busy:   VecDeque::with_capacity(HISTORY),
            history_cpu:    VecDeque::with_capacity(HISTORY),
            history_mem:    VecDeque::with_capacity(HISTORY),
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);

        if !proc_::is_linux() {
            let merged = sessions::merge(vec![
                claude::summarise(&[], now),
                codex::summarise(&[], now),
            ]);
            let mut snap = Snapshot::default();
            snap.now = now;
            snap.platform = std::env::consts::OS.to_string();
            snap.note = Some("Live process metrics require Linux /proc — running in session-readers-only mode.".into());
            snap.sys_cpus = self.num_cpus as u32;
            snap.aggregates.waiting = merged.sessions.waiting;
            snap.aggregates.completed = merged.sessions.completed;
            snap.sessions = merged.sessions;
            return snap;
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

            let proc_total = stat.utime + stat.stime;
            let prev_total = self.prev.get(&pid).map(|p| p.total);
            let cpu_raw = match prev_total {
                Some(pt) => {
                    let proc_delta = proc_total.saturating_sub(pt) as f64;
                    (proc_delta / total_delta as f64) * self.num_cpus as f64 * 100.0
                }
                None => 0.0,
            }.max(0.0);
            self.prev.insert(pid, PrevCpu { total: proc_total });

            let smoothed = match self.cpu_smooth.get(&pid) {
                Some(prev) => prev * 0.6 + cpu_raw * 0.4,
                None => cpu_raw,
            };
            self.cpu_smooth.insert(pid, smoothed);

            let rss_bytes = stat.rss_pages * proc_::PAGE_SIZE;
            let started_at_sec = self.boot_time + stat.starttime / proc_::CLK_TCK;
            let now_sec = now / 1000;
            let uptime_sec = now_sec.saturating_sub(started_at_sec);

            let cwd = cwd_path.to_string_lossy().into_owned();
            let project = project_basename(&cwd);
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
                cpu: smoothed,
                cpu_raw,
                rss: rss_bytes,
                vsize: stat.vsize,
                threads: stat.num_threads,
                state: stat.state.to_string(),
                ppid: stat.ppid,
                uptime_sec,
                cwd,
                exe: exe_path.to_string_lossy().into_owned(),
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

        // Spawn / exit events.
        let live_pids: std::collections::HashSet<u32> = agents.iter().map(|a| a.pid).collect();
        for a in &agents {
            if !self.known_pids.contains_key(&a.pid) {
                self.known_pids.insert(a.pid, a.label.clone());
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

        // Per-vendor session enrichment + generic fallback for everyone else.
        let live_refs: Vec<LiveAgentRef> = agents.iter()
            .map(|a| LiveAgentRef { pid: a.pid, cwd: a.cwd.as_str(), label: a.label.as_str() })
            .collect();
        let claude_r  = claude::summarise(&live_refs, now);
        let codex_r   = codex::summarise(&live_refs, now);
        let generic_r = generic::summarise(&agents, &live_refs, now);
        let merged = sessions::merge(vec![claude_r, codex_r, generic_r]);

        for a in &mut agents {
            if let Some(s) = merged.by_pid.get(&a.pid) {
                a.status = s.status;
                a.current_tool = s.current_tool.clone();
                a.current_task = s.last_task.clone();
                a.subagents = s.in_flight_tasks;
                a.session_id = Some(s.id.clone());
                a.session_age_ms = Some(s.age_ms);
            } else {
                // No vendor-specific or generic enrichment — derive status
                // from process activity alone.
                a.status = Status::Idle;
            }
            // Universal CPU% override — process state always wins over any
            // flush-lag in the underlying session transcript.
            if a.cpu >= 20.0 { a.status = Status::Busy; }
            else if a.cpu >= 3.0 && matches!(a.status, Status::Idle | Status::Stale) {
                a.status = Status::Active;
            } else if a.cpu >= 1.0 && a.status == Status::Idle {
                a.status = Status::Active;
            }
        }
        let sessions = merged;

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

        push_bounded(&mut self.history_total,  agents.len() as f64, HISTORY);
        push_bounded(&mut self.history_active, agents.len() as f64 + sessions.sessions.waiting as f64, HISTORY);
        push_bounded(&mut self.history_busy,   busy_count as f64, HISTORY);
        push_bounded(&mut self.history_cpu,    (agg_cpu * 10.0).round() / 10.0, HISTORY);
        push_bounded(&mut self.history_mem,    ((agg_mem as f64 / 1_048_576.0) * 10.0).round() / 10.0, HISTORY);

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
                waiting: sessions.sessions.waiting,
                completed: sessions.sessions.completed,
                subagents: subagents_total,
                project_count,
            },
            agents,
            projects,
            sessions: sessions.sessions,
            history: History {
                total:  self.history_total.iter().copied().collect(),
                active: self.history_active.iter().copied().collect(),
                busy:   self.history_busy.iter().copied().collect(),
                cpu:    self.history_cpu.iter().copied().collect(),
                mem:    self.history_mem.iter().copied().collect(),
            },
            activity: self.activity.iter().rev().take(80).cloned().collect(),
        }
    }

    fn push_activity(&mut self, e: ActivityEvent) {
        if self.activity.len() >= MAX_ACTIVITY { self.activity.pop_front(); }
        self.activity.push_back(e);
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
