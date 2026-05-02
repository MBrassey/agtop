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

use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

pub struct SysBackend {
    sys: System,
    /// Per-tick refresh spec: full process metadata (cpu, mem, cmd,
    /// cwd, exe) plus disk-usage IO so we can fill the read_bytes /
    /// write_bytes columns on macOS + Windows like the Linux /proc
    /// path does.
    refresh_kind: ProcessRefreshKind,
}

impl SysBackend {
    pub fn new() -> Self {
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet)
            .with_disk_usage()
            .with_tasks();
        let mut sys = System::new_with_specifics(
            RefreshKind::default()
                .with_processes(refresh_kind)
                .with_memory(MemoryRefreshKind::everything()),
        );
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
        sys.refresh_memory();
        Self { sys, refresh_kind }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, self.refresh_kind);
        self.sys.refresh_memory();
    }

    /// Total system memory in bytes.  0 if sysinfo couldn't read it.
    pub fn total_memory(&self) -> u64 { self.sys.total_memory() }

    /// Available (free + reclaimable) memory in bytes.  0 if unknown.
    pub fn available_memory(&self) -> u64 { self.sys.available_memory() }

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
            // Sanitise sysinfo-derived strings the same way the
            // Linux backend does — argv[0] / cwd / exe come from
            // attacker-influenced sources and could contain ANSI.
            let cwd = crate::format::sanitize_control(
                &proc.cwd().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
            let exe = crate::format::sanitize_control(
                &proc.exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
            let cmdline = crate::format::sanitize_control(&cmdline);
            let project = derive_project(&cwd, &exe, &cmdline, &label);
            let cpu = proc.cpu_usage() as f64;
            let started_at = proc.start_time();   // unix seconds
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let uptime_sec = now.saturating_sub(started_at);

            // Native writable-FD enumeration (libproc on macOS,
            // NtQuerySystemInformation on Windows, empty on *BSD).
            // Computed once per agent, then split into files +
            // unique-dirs to mirror the Linux /proc path.
            let writing = crate::writing_files::read(pid.as_u32(), 4);
            let writing_files: Vec<String> = writing.iter()
                .map(|p| crate::format::sanitize_control(&p.to_string_lossy()))
                .collect();
            let writing_dirs: Vec<String> = {
                let mut seen = std::collections::HashSet::new();
                writing.iter()
                    .filter_map(|p| p.parent().map(|d|
                        crate::format::sanitize_control(&d.to_string_lossy())))
                    .filter(|d| seen.insert(d.clone()))
                    .collect()
            };

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
                tokens_cache_read: 0,
                tokens_cache_write: 0,
                cost_usd: 0.0,
                cost_basis: "unknown".into(),
                context_used: 0,
                context_limit: 0,
                loaded_skills: Vec::new(),
                tool_counts: Vec::new(),
                ppid_name: proc.parent()
                    .and_then(|pp| self.sys.process(pp))
                    .map(|p| crate::format::sanitize_control(&p.name().to_string_lossy()))
                    .unwrap_or_default(),
                session_started_ms: 0,
                dangerous_flag: crate::collector::dangerous_flag_for_cmdline(&cmdline),
                model: None,
                dangerous: crate::collector::is_dangerous_for_cmdline(&cmdline),
                in_flight_subagents: Vec::new(),
                recent_activity: Vec::new(),
                cpu_history: Vec::new(),
                tokens_history: Vec::new(),
                cpu,
                cpu_raw: cpu,
                rss: proc.memory(),
                vsize: proc.virtual_memory(),
                threads: proc.tasks().map(|t| t.len() as u64).unwrap_or(1),
                state: format!("{:?}", proc.status()),
                ppid: proc.parent().map(|p| p.as_u32()).unwrap_or(0),
                uptime_sec,
                cwd,
                exe,
                cmdline,
                // sysinfo exposes per-process disk IO on Linux + macOS +
                // Windows (FreeBSD returns 0).  `total_*` are
                // cumulative since process start, which matches the
                // semantics of /proc/<pid>/io that the Linux backend
                // returns.
                read_bytes:  proc.disk_usage().total_read_bytes,
                write_bytes: proc.disk_usage().total_written_bytes,
                writing_files,
                writing_dirs,
                // sysinfo doesn't expose per-process FD lists or
                // child enumeration on macOS / Windows / *BSD;
                // these stay empty.  /proc/<pid>/net/tcp is also
                // Linux-only.
                reading_files: Vec::new(),
                children: Vec::new(),
                net_established: 0,
                read_rate_bps: 0,
                write_rate_bps: 0,
                gpu_pct: 0.0,
                gpu_mem_bytes: 0,
            });
        }
        out
    }

    pub fn num_cpus(&self) -> usize {
        // Fall back to logical core count via std::thread.
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    }
}
