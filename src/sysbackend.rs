// Cross-platform process backend used when /proc isn't available (macOS,
// *BSD, Windows).  Powered by the `sysinfo` crate, which gives us PID,
// command line, working directory, executable path, RSS, virtual size,
// CPU%, and start time on every supported OS.
//
// Things /proc gives us that sysinfo can't:
//
//   - per-process IO (read_bytes / write_bytes)
//   - writable-FD enumeration (we surface this as "writing files" on Linux)
//   - per-pid clock-tick precision for CPU calculations
//
// On non-Linux we leave those fields zero/empty and rely on session readers
// + CPU% for status.  All other behavior — matchers, project derivation,
// session enrichment, sorting, charts — works identically.

use crate::format::derive_project;
use crate::matchers::{classify, Matcher, UserMatcher};
use crate::model::{Agent, Status};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

pub struct SysBackend {
    sys: System,
}

impl SysBackend {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::default().with_processes(ProcessRefreshKind::default()),
        );
        sys.refresh_processes(ProcessesToUpdate::All, true);
        Self { sys }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
    }

    /// Walk every process and return Agents that match a known matcher.
    pub fn collect_agents(
        &self,
        builtins: &[Matcher],
        user: &[UserMatcher],
    ) -> Vec<Agent> {
        let mut out = Vec::new();
        for (pid, proc) in self.sys.processes() {
            let cmdline_parts: Vec<&str> = proc.cmd().iter()
                .filter_map(|s| s.to_str()).collect();
            let cmdline = cmdline_parts.join(" ");
            if cmdline.is_empty() { continue; }
            let label = match classify(&cmdline, builtins, user) {
                Some(l) => l.to_string(),
                None => continue,
            };
            let cwd = proc.cwd().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let exe = proc.exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let project = derive_project(&cwd, &exe, &cmdline, &label);
            let cpu = proc.cpu_usage() as f64;
            let started_at = proc.start_time();   // unix seconds
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let uptime_sec = now.saturating_sub(started_at);

            out.push(Agent {
                pid: pid.as_u32(),
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
                cost_usd: 0.0,
                model: None,
                dangerous: crate::collector::is_dangerous_for_cmdline(&cmdline),
                in_flight_subagents: Vec::new(),
                cpu_history: Vec::new(),
                cpu,
                cpu_raw: cpu,
                rss: proc.memory(),
                vsize: proc.virtual_memory(),
                threads: 1,
                state: format!("{:?}", proc.status()),
                ppid: proc.parent().map(|p| p.as_u32()).unwrap_or(0),
                uptime_sec,
                cwd,
                exe,
                cmdline,
                read_bytes: 0,
                write_bytes: 0,
                writing_files: Vec::new(),
                writing_dirs: Vec::new(),
            });
        }
        out
    }

    pub fn num_cpus(&self) -> usize {
        // Fall back to logical core count via std::thread.
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    }
}
