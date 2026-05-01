// Google Gemini CLI session reader.
//
// The Gemini CLI keeps history under ~/.gemini/ — usually:
//   ~/.gemini/sessions/<id>.json
//   ~/.gemini/history.json
//   ~/.gemini/config.json
//
// Schema is one JSON object with a `messages` array and an optional
// `metadata.cwd`/`metadata.model`.  Best-effort parsing — if the format
// drifts we still surface the file's mtime as session activity.

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS:   u64 = 30 * 1000;
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;

fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini").join("sessions")
}

fn read_whole(path: &Path) -> String {
    let mut s = String::new();
    if let Ok(mut f) = File::open(path) { let _ = f.read_to_string(&mut s); }
    s
}

#[derive(Default)]
struct AnalysisOut {
    last_user: Option<String>,
    last_assistant: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    tokens_input: u64,
    tokens_output: u64,
    recent_activity: Vec<String>,
}

fn analyse(text: &str) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let v: Value = match serde_json::from_str(text) { Ok(v) => v, Err(_) => return out };
    if let Some(m) = v.get("metadata") {
        out.cwd = m.get("cwd").and_then(|x| x.as_str()).map(String::from);
        out.model = m.get("model").and_then(|x| x.as_str()).map(String::from);
    }
    if let Some(arr) = v.get("messages").and_then(|x| x.as_array()) {
        for m in arr {
            let role = m.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let content = m.get("content").and_then(|x| x.as_str())
                .or_else(|| m.get("text").and_then(|x| x.as_str()))
                .unwrap_or("");
            let normalised: String = content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
            if !normalised.is_empty() {
                if role == "user" {
                    out.last_user = Some(normalised.clone());
                    out.recent_activity.push(format!("› {}", normalised));
                } else if role == "model" || role == "assistant" {
                    out.last_assistant = Some(normalised.clone());
                    out.recent_activity.push(format!("› {}", normalised));
                }
            }
            if let Some(u) = m.get("usage").or_else(|| m.get("usageMetadata")) {
                out.tokens_input  += u.get("promptTokenCount").and_then(|x| x.as_u64())
                    .or_else(|| u.get("input_tokens").and_then(|x| x.as_u64())).unwrap_or(0);
                out.tokens_output += u.get("candidatesTokenCount").and_then(|x| x.as_u64())
                    .or_else(|| u.get("output_tokens").and_then(|x| x.as_u64())).unwrap_or(0);
            }
        }
    }
    // Cap to most recent 12 events.
    if out.recent_activity.len() > 12 {
        let drop = out.recent_activity.len() - 12;
        out.recent_activity.drain(0..drop);
    }
    out
}

fn classify(is_live: bool, age_ms: u64) -> Status {
    if is_live && age_ms < BUSY_WINDOW_MS   { return Status::Busy; }
    if is_live && age_ms < ACTIVE_WINDOW_MS { return Status::Active; }
    if is_live                              { return Status::Idle; }
    if age_ms < RECENT_WINDOW_MS            { return Status::Waiting; }
    Status::Stale
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let root = root();
    if !root.exists() { return SessionsResult::empty(); }

    let mut cwd_to_pid: HashMap<String, u32> = HashMap::new();
    for a in live_agents {
        if a.label == "gemini" { cwd_to_pid.insert(a.cwd.into(), a.pid); }
    }

    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();

    let rd = match fs::read_dir(&root) { Ok(d) => d, Err(_) => return SessionsResult::empty() };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let md = match fs::metadata(&p) { Ok(m) => m, Err(_) => continue };
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let age_ms = now_ms.saturating_sub(mtime);
        if age_ms > RECENT_WINDOW_MS && live_agents.is_empty() { continue; }
        let info = analyse(&read_whole(&p));
        let cwd = info.cwd.clone().unwrap_or_default();
        let proj_short = project_basename(&cwd);
        let live_pid = if !cwd.is_empty() { cwd_to_pid.get(&cwd).copied() } else { None };
        let status = classify(live_pid.is_some(), age_ms);
        let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let last_task = info.last_assistant.clone().or(info.last_user.clone());
        let sess = Session {
            id: id.clone(),
            project: cwd.clone(),
            project_short: proj_short.clone(),
            file: p.to_string_lossy().into_owned(),
            size_bytes: md.len(),
            mtime_ms: mtime,
            age_ms,
            status,
            stop_reason: None,
            last_task: last_task.as_deref().map(sanitize_control),
            last_tool: None,
            current_tool: None,
            in_flight_tasks: 0,
            in_flight_subagents: Vec::new(),
            recent_activity: info.recent_activity.iter()
                .map(|s| sanitize_control(s)).collect(),
            live_pid,
            is_most_recent: true,
            tokens_input: info.tokens_input,
            tokens_output: info.tokens_output,
            tokens_total: info.tokens_input + info.tokens_output,
            cost_usd: 0.0,
            context_used: 0,
            model: info.model.as_deref().map(sanitize_control),
        };
        if let Some(pid) = live_pid {
            by_pid.entry(pid).or_insert_with(|| sess.clone());
        }
        if let Some(t) = &last_task {
            if age_ms < RECENT_WINDOW_MS {
                recent_tasks.push(RecentTask {
                    project: cwd, project_short: proj_short,
                    task: t.clone(), mtime_ms: mtime, status,
                });
            }
        }
        sessions.push(sess);
    }

    sessions.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    recent_tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    let waiting   = sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = sessions.iter().filter(|s| matches!(s.status, Status::Active|Status::Busy|Status::Spawning|Status::Idle)).count() as u32;
    let busy      = sessions.iter().filter(|s| matches!(s.status, Status::Busy|Status::Spawning)).count() as u32;
    SessionsResult {
        sessions: Sessions { sessions, recent_tasks, active, busy, waiting, completed },
        by_pid,
    }
}
