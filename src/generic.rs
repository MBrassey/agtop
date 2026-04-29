// Generic enricher for agents that don't have a vendor-specific session
// reader (yet). Looks at the process's open writable file descriptors and
// uses the most recently modified file as the "current task" hint.
//
// This is a best-effort, vendor-agnostic fallback: it gives any AI coding
// agent (aider, cursor-agent, gemini, goose, continue, opencode, copilot,
// cody, amp, crush, mods, sgpt, llm, ollama, fabric, ...) at least a "what
// is it touching right now?" line in the DOING column, even if we can't
// parse its transcript.

use crate::model::{Agent, Session, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use std::collections::HashMap;
use std::fs;
use std::path::Path;

// Vendor labels we already enrich in dedicated modules — generic skips these.
const SKIP_LABELS: &[&str] = &["claude", "claude-code", "codex", "openai-codex"];

pub fn summarise(agents: &[Agent], live_agents: &[LiveAgentRef], _now_ms: u64) -> SessionsResult {
    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    for a in agents {
        if SKIP_LABELS.contains(&a.label.as_str()) { continue; }
        // Only consider the labels that actually appear in live_agents (i.e.
        // process really exists).
        if !live_agents.iter().any(|l| l.pid == a.pid) { continue; }

        // Pick the most-recently modified file the agent has open for write
        // and present it as the "current task" hint.
        let recent = most_recent_writable(a);
        let last_task = recent.as_ref().map(|p| {
            let s = p.to_string_lossy();
            // Strip the cwd prefix to keep the hint short.
            if let Some(rest) = s.strip_prefix(a.cwd.as_str()) {
                let r = rest.trim_start_matches('/');
                if r.is_empty() { s.into_owned() } else { r.to_string() }
            } else {
                s.into_owned()
            }
        });

        if last_task.is_some() {
            // Synthetic Session record for the merge step. We don't have a
            // real session id so we use "{pid}".
            by_pid.insert(a.pid, Session {
                id: format!("{}", a.pid),
                project: a.cwd.clone(),
                project_short: a.project.clone(),
                file: String::new(),
                size_bytes: 0,
                mtime_ms: 0,
                age_ms: 0,
                status: classify(a),
                stop_reason: None,
                last_task,
                last_tool: None,
                current_tool: None,
                in_flight_tasks: 0,
                live_pid: Some(a.pid),
                is_most_recent: true,
                tokens_input: 0,
                tokens_output: 0,
                tokens_total: 0,
            });
        }
    }

    SessionsResult { sessions: crate::model::Sessions::default(), by_pid }
}

fn classify(a: &Agent) -> Status {
    if a.cpu >= 20.0 { Status::Busy }
    else if a.cpu >= 1.0 { Status::Active }
    else { Status::Idle }
}

fn most_recent_writable(a: &Agent) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
    for f in &a.writing_files {
        if f.is_empty() { continue; }
        let p = Path::new(f);
        if let Ok(md) = fs::metadata(p) {
            if let Ok(mt) = md.modified() {
                let take = best.as_ref().map(|(_, t)| mt > *t).unwrap_or(true);
                if take {
                    best = Some((p.to_path_buf(), mt));
                }
            }
        }
    }
    best.map(|(p, _)| p)
}
