// Data shapes shared by the collector, the TUI, and the JSON output path.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Busy,
    Spawning,
    Active,
    // Default keeps `Agent: Default` derivable (UI test fixtures).
    #[default]
    Idle,
    Waiting,
    Completed,
    Stale,
}

impl Status {
    pub fn rank(self) -> u8 {
        match self {
            Status::Busy => 0,
            Status::Spawning => 1,
            Status::Active => 2,
            Status::Idle => 3,
            Status::Waiting => 4,
            Status::Completed => 5,
            Status::Stale => 6,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Busy => "BUSY",
            Status::Spawning => "SPWN",
            Status::Active => "ACTV",
            Status::Idle => "IDLE",
            Status::Waiting => "WAIT",
            Status::Completed => "DONE",
            Status::Stale => "STAL",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Busy | Status::Active => "●",
            Status::Spawning => "●",
            Status::Idle => "○",
            Status::Waiting => "◌",
            Status::Completed => "✓",
            Status::Stale => "·",
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Agent {
    pub pid: u32,
    pub label: String,
    pub status: Status,
    pub project: String,
    pub current_tool: Option<String>,
    pub current_task: Option<String>,
    pub subagents: u32,
    /// Human-readable descriptions of in-flight Task / Agent tool calls
    /// (e.g. "code-reviewer: review the auth refactor").  Populated by the
    /// session reader; one entry per element in `subagents`.
    pub in_flight_subagents: Vec<String>,
    /// Last ~6–10 readable events from the session transcript, oldest →
    /// newest.  Each line is already prefixed with one of:
    ///   `› `  user / assistant prose
    ///   `→ `  tool call
    ///   `← `  tool result
    /// Surfaced in the detail popup as a live-preview box.
    pub recent_activity: Vec<String>,
    pub session_id: Option<String>,
    pub session_age_ms: Option<u64>,
    /// Sum of input + output (+ cache) tokens charged to this agent's session.
    pub tokens_total: u64,
    /// Total input bucket: standard_input + cache_read + cache_creation.
    /// (Use `tokens_input - tokens_cache_read - tokens_cache_write` if
    /// you specifically want the *uncached* portion.)
    pub tokens_input: u64,
    pub tokens_output: u64,
    /// Cache-read tokens, charged at ~10% of the standard input rate
    /// under Anthropic prompt-caching pricing.  Tracked separately so
    /// `cost_usd` applies the discount instead of over-billing cache
    /// hits at the full input rate.
    #[serde(default)]
    pub tokens_cache_read: u64,
    /// Cache-creation / cache-write tokens, charged at ~125% of the
    /// standard input rate.
    #[serde(default)]
    pub tokens_cache_write: u64,
    pub cost_usd: f64,
    /// How `cost_usd` was computed.  One of:
    ///   - "api"     — known per-token rate looked up in the price table
    ///   - "local"   — model runs on user's hardware (Ollama / vLLM / llama.cpp); $0
    ///   - "unknown" — no model name, or no price-table match (treated as $0 but flagged)
    pub cost_basis: String,
    pub model: Option<String>,
    /// Latest-turn input window size in tokens (input_tokens +
    /// cache_read_input_tokens for Anthropic, prompt_tokens for
    /// OpenAI / Codex).  Drives the per-agent context-fill bar in the
    /// detail popup so the user can see how close they are to
    /// auto-compaction.  0 if unknown.
    pub context_used: u64,
    /// Maximum input window in tokens for this agent's model, looked
    /// up via the bundled price table (LiteLLM-derived).  0 if unknown.
    pub context_limit: u64,
    /// Extrapolated seconds until this agent's context window hits 95%
    /// (auto-compaction), from the per-pid context-growth trend.  `None`
    /// when there isn't enough history or the trend is flat/negative.
    /// Computed in the collector so the render side needs no live handle
    /// to it.
    #[serde(default)]
    pub time_to_compaction_secs: Option<u64>,
    /// Context-window growth rate in tokens/minute, same source ring as
    /// `time_to_compaction_secs`.  `None` when history is insufficient.
    #[serde(default)]
    pub ctx_growth_per_min: Option<u64>,
    /// Names of Claude Code agent skills loaded for this session.
    /// Populated by scanning `~/.claude/skills/<name>/SKILL.md` plus
    /// `<cwd>/.claude/skills/<name>/SKILL.md`.  Empty for non-Claude
    /// vendors.
    pub loaded_skills: Vec<String>,
    /// Names of Claude Code plugins enabled for this session.
    /// Resolved from `~/.claude/settings.json`'s `enabledPlugins` map
    /// cross-referenced against `~/.claude/plugins/installed_plugins.json`.
    /// Plugins are user-global (vs skills, which can be project-local
    /// too), so this is the same set for every Claude session on a
    /// given host.  Empty for non-Claude vendors.
    #[serde(default)]
    pub loaded_plugins: Vec<String>,
    /// Tool-use frequency over the lifetime of this session — top
    /// tools by call count.  Populated by the vendor enricher;
    /// surfaced in the detail popup as `tools: Bash 47 · Edit 23 · …`.
    #[serde(default)]
    pub tool_counts: Vec<(String, u32)>,
    /// Parent process command name (e.g. `zsh`, `bash`, `fish`,
    /// `tmux`, `code`).  Resolved from `/proc/<ppid>/comm` on Linux
    /// or `sysinfo::Process::name()` elsewhere.  Useful for tracing
    /// how the agent was launched.  Empty if unresolvable.
    #[serde(default)]
    pub ppid_name: String,
    /// Wall-clock start time of the session itself (vs `uptime_sec`
    /// which is process start).  These diverge when the agent was
    /// invoked with `claude --resume` against an older session.
    /// Unix milliseconds; 0 if unknown.
    #[serde(default)]
    pub session_started_ms: u64,
    /// When `dangerous = true`, the specific flag that triggered the
    /// classifier (`--dangerously-skip-permissions`, `--yolo`, etc.)
    /// so the user knows what's actually in play.  Empty otherwise.
    #[serde(default)]
    pub dangerous_flag: String,
    /// Process is running with elevated / unsafe permissions
    /// (e.g. `claude --dangerously-skip-permissions`, `--yolo`, `--no-permissions`).
    /// The TUI surfaces this as a pulsating "GOD" tag on the row.
    pub dangerous: bool,
    /// Recent CPU% samples for the inline sparkline (oldest → newest).
    pub cpu_history: Vec<f64>,
    /// Recent per-tick token deltas (tokens consumed since previous
    /// snapshot, oldest → newest).  Drives a per-agent token-rate
    /// sparkline in the detail popup so users can see a session's
    /// burn pattern over time without leaving the TUI.  Always
    /// length `HISTORY` once the agent has been observed for that
    /// many ticks; padded with leading zeros otherwise.
    #[serde(default)]
    pub tokens_history: Vec<f64>,
    pub cpu: f64,
    pub cpu_raw: f64,
    pub rss: u64,
    pub vsize: u64,
    pub threads: u64,
    pub state: String,
    pub ppid: u32,
    pub uptime_sec: u64,
    pub cwd: String,
    pub exe: String,
    pub cmdline: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub writing_files: Vec<String>,
    pub writing_dirs: Vec<String>,
    /// Files the process currently has open in read-only mode.
    /// Surfaces what the agent is reading right now (project files
    /// during context indexing, MCP server configs, hook scripts,
    /// etc.) — useful when CPU is up but no tokens are flowing
    /// because Claude Code is doing background work.  Linux-only;
    /// empty on other platforms (sysinfo doesn't expose foreign-
    /// process file descriptors).
    #[serde(default)]
    pub reading_files: Vec<String>,
    /// Immediate child processes spawned by the agent — `(pid, comm)`
    /// pairs from /proc/<pid>/task/*/children.  Captures hook
    /// invocations (`hook-pre-commit`, `bash`), MCP server processes,
    /// and any shell commands the agent ran.  Linux-only.
    #[serde(default)]
    pub children: Vec<(u32, String)>,
    /// Count of established TCP connections (v4 + v6).  Linux-only;
    /// 0 when the count can't be read.  Non-zero indicates the
    /// agent is talking to an API / MCP server / network resource
    /// even when no tokens are visibly flowing.
    #[serde(default)]
    pub net_established: u32,

    /// Bytes/sec read by this agent over the last tick interval.
    /// Computed as Δ(read_bytes) ÷ Δt; 0 on the first sample for
    /// any pid.  Linux-only.
    #[serde(default)]
    pub read_rate_bps: u64,
    /// Bytes/sec written by this agent over the last tick interval.
    /// Counterpart to `read_rate_bps`.  Linux-only.
    #[serde(default)]
    pub write_rate_bps: u64,

    /// GPU utilisation (0-100%) attributed to this PID by NVIDIA
    /// drivers.  Populated by parsing `nvidia-smi --query-compute-apps`
    /// once per snapshot.  0 when the host has no NVIDIA GPU,
    /// when nvidia-smi isn't on PATH, or when this PID isn't using
    /// the GPU.  AMD GPUs aren't yet supported.
    #[serde(default)]
    pub gpu_pct: f64,
    /// VRAM used by this PID in bytes.  See `gpu_pct`.
    #[serde(default)]
    pub gpu_mem_bytes: u64,

    /// Host the process lives on.  Empty (default) means the same OS
    /// agtop is running on.  For Windows-side agtop, WSL agents pulled
    /// in via `wsl_backend` carry `"wsl:<distro>"` here so the UI can
    /// disambiguate.  PID is then namespaced (top bit set, distro idx
    /// in next 7 bits, real Linux PID in low 24) so the collector's
    /// per-pid HashMaps stay collision-free across hosts.
    #[serde(default)]
    pub host: String,
}

impl Agent {
    /// PID as the user expects to see it.  WSL-hosted agents have
    /// their PID namespaced into the upper bits of u32; this strips
    /// the namespace and returns the real Linux PID inside the
    /// guest.  For native agents returns the stored PID unchanged.
    pub fn display_pid(&self) -> u32 {
        if self.host.starts_with("wsl:") {
            self.pid & 0x00FFFFFF
        } else {
            self.pid
        }
    }
}

/// Upper bit reserved for non-native (WSL) PIDs so the collector's
/// per-pid HashMaps (cpu_smooth, agent_cpu_hist, prev, …) can't
/// collide between a Windows PID and a Linux-inside-WSL PID that
/// happens to share the same numeric value.
#[allow(dead_code)] // only referenced from the Windows-only wsl_backend module
pub const WSL_PID_BASE: u32 = 0x8000_0000;

/// Encode `(distro_idx, linux_pid)` into a single namespaced u32.
/// 128 distros × 16 M PIDs — far exceeds any realistic install.
#[allow(dead_code)] // only referenced from the Windows-only wsl_backend module
pub fn encode_wsl_pid(distro_idx: u8, linux_pid: u32) -> u32 {
    WSL_PID_BASE | ((distro_idx as u32 & 0x7F) << 24) | (linux_pid & 0x00FF_FFFF)
}

/// Reverse of [`encode_wsl_pid`] — strips the namespace bits and
/// returns the real Linux PID inside the guest.  Used by UI display
/// sites that only have a raw PID (e.g. the activity event log)
/// rather than the full Agent struct.
pub fn display_pid(pid: u32) -> u32 {
    if pid & WSL_PID_BASE != 0 { pid & 0x00FF_FFFF } else { pid }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectAgg {
    pub project: String,
    pub agents: u32,
    pub cpu: f64,
    pub rss: u64,
    pub subagents: u32,
    pub tokens_total: u64,
    pub cost_usd: f64,
    pub statuses: HashMap<&'static str, u32>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub project: String,
    pub project_short: String,
    pub file: String,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    pub age_ms: u64,
    pub status: Status,
    pub stop_reason: Option<String>,
    pub last_task: Option<String>,
    pub last_tool: Option<String>,
    pub current_tool: Option<String>,
    pub in_flight_tasks: u32,
    pub in_flight_subagents: Vec<String>,
    pub recent_activity: Vec<String>,
    pub live_pid: Option<u32>,
    pub is_most_recent: bool,
    pub tokens_total: u64,
    pub tokens_input: u64,
    pub tokens_output: u64,
    /// See `Agent::tokens_cache_read`.
    #[serde(default)]
    pub tokens_cache_read: u64,
    /// See `Agent::tokens_cache_write`.
    #[serde(default)]
    pub tokens_cache_write: u64,
    pub cost_usd: f64,
    pub model: Option<String>,
    /// Latest-turn input window size (see `Agent::context_used`).
    pub context_used: u64,
    /// First-record timestamp from the JSONL transcript.  Unix ms.
    /// 0 when not parseable.
    #[serde(default)]
    pub session_started_ms: u64,
    /// Tool-use counter over the session.  Top entries surfaced in
    /// the popup.
    #[serde(default)]
    pub tool_counts: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Sessions {
    pub sessions: Vec<Session>,
    pub recent_tasks: Vec<RecentTask>,
    pub active: u32,
    pub busy: u32,
    pub waiting: u32,
    pub completed: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentTask {
    pub project: String,
    pub project_short: String,
    pub task: String,
    pub mtime_ms: u64,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEvent {
    pub t: u64,
    pub kind: ActivityKind,
    pub label: String,
    pub pid: u32,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    Spawn,
    Exit,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Aggregates {
    pub cpu: f64,
    pub mem_bytes: u64,
    pub active: u32,
    pub busy: u32,
    pub waiting: u32,
    pub completed: u32,
    pub subagents: u32,
    pub project_count: u32,
    pub tokens_total: u64,
    pub tokens_input: u64,
    pub tokens_output: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct History {
    pub total: Vec<f64>,
    pub active: Vec<f64>,
    pub busy: Vec<f64>,
    pub cpu: Vec<f64>,
    pub mem: Vec<f64>,
    /// Per-tick *delta* in cumulative tokens — gives a true activity pulse.
    pub tokens_rate: Vec<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub now: u64,
    pub platform: String,
    pub note: Option<String>,
    pub sys_cpus: u32,
    pub mem_total: u64,
    pub mem_available: u64,
    pub aggregates: Aggregates,
    pub agents: Vec<Agent>,
    pub projects: Vec<ProjectAgg>,
    pub sessions: Sessions,
    pub history: History,
    pub activity: Vec<ActivityEvent>,
}
