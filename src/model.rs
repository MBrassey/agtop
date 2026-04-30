// Data shapes shared by the collector, the TUI, and the JSON output path.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Busy,
    Spawning,
    Active,
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
            Status::Idle => "idle",
            Status::Waiting => "WAIT",
            Status::Completed => "DONE",
            Status::Stale => "stale",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Busy | Status::Active => "●",
            Status::Spawning => "◆",
            Status::Idle => "○",
            Status::Waiting => "◌",
            Status::Completed => "✓",
            Status::Stale => "·",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
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
    pub tokens_input: u64,
    pub tokens_output: u64,
    pub cost_usd: f64,
    /// How `cost_usd` was computed.  One of:
    ///   - "api"     — known per-token rate looked up in the price table
    ///   - "local"   — model runs on user's hardware (Ollama / vLLM / llama.cpp); $0
    ///   - "unknown" — no model name, or no price-table match (treated as $0 but flagged)
    pub cost_basis: String,
    pub model: Option<String>,
    /// Process is running with elevated / unsafe permissions
    /// (e.g. `claude --dangerously-skip-permissions`, `--yolo`, `--no-permissions`).
    /// The TUI surfaces this as a pulsating "GOD" tag on the row.
    pub dangerous: bool,
    /// Recent CPU% samples for the inline sparkline (oldest → newest).
    pub cpu_history: Vec<f64>,
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
    pub cost_usd: f64,
    pub model: Option<String>,
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
