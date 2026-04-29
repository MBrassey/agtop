// OpenAI Codex CLI session enricher.
//
// Codex stores rollouts (transcripts) at:
//
//   ~/.codex/sessions/<YYYY>/<MM>/<DD>/<rollout-id>.jsonl
//
// (older flat layout: ~/.codex/sessions/<rollout-id>.jsonl) — this module
// handles both via a shallow recursive walk (max depth 4).
//
// Each line is a JSON event. The schema has evolved; we probe defensively
// for the shapes that have actually shipped:
//
//   { "type": "session_meta", "payload": { "id": "...", "cwd": "...", ... } }
//   { "type": "response_item",
//     "payload": { "type": "function_call",
//                  "name": "shell", "arguments": "...", "call_id": "call_1" } }
//   { "type": "response_item",
//     "payload": { "type": "function_call_output", "call_id": "call_1", "output": "..." } }
//   { "type": "response_item",
//     "payload": { "type": "message", "role": "user|assistant",
//                  "content": [{"type":"input_text","text":"..."}] } }
//
// We also tolerate the flat shape (no `payload` nesting) and the older
// `function_call`/`tool_use` field names — anything that mentions a tool
// call_id we'll track as in-flight until a matching output arrives.

use crate::format::project_basename;
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS: u64 = 5 * 1000;
const ACTIVE_WINDOW_MS: u64 = 60 * 1000;
const TAIL_BYTES: u64 = 256 * 1024;
const HEAD_BYTES: u64 = 4 * 1024; // session_meta is at the top of the file

fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".codex").join("sessions")
}

fn read_tail(path: &Path, bytes: u64) -> String {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return String::new() };
    let len = match f.metadata() { Ok(m) => m.len(), Err(_) => return String::new() };
    if len == 0 { return String::new(); }
    let take = bytes.min(len);
    if f.seek(SeekFrom::End(-(take as i64))).is_err() {
        return String::new();
    }
    let mut buf = String::with_capacity(take as usize);
    let _ = f.take(take).read_to_string(&mut buf);
    buf
}

fn read_head(path: &Path, bytes: u64) -> String {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return String::new() };
    let mut buf = String::with_capacity(bytes as usize);
    let _ = f.take(bytes).read_to_string(&mut buf);
    buf
}

fn parse_lines(text: &str) -> Vec<Value> {
    text.split('\n')
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

/// Recursive walk capped at `max_depth` so we don't traverse the entire home
/// dir if the user has put junk under ~/.codex/sessions.
fn walk_jsonls(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn rec(dir: &Path, depth: usize, max: usize, out: &mut Vec<PathBuf>) {
        if depth > max { return; }
        let rd = match fs::read_dir(dir) { Ok(d) => d, Err(_) => return };
        for ent in rd.flatten() {
            let p = ent.path();
            let ft = match ent.file_type() { Ok(f) => f, Err(_) => continue };
            if ft.is_dir() {
                rec(&p, depth + 1, max, out);
            } else if ft.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    rec(root, 0, max_depth, &mut out);
    out
}

fn extract_cwd_from_meta(text: &str) -> Option<String> {
    for r in parse_lines(text) {
        // Probe a handful of plausible field names across schema versions.
        let candidates = [
            r.get("payload").and_then(|p| p.get("cwd")).and_then(|v| v.as_str()),
            r.get("cwd").and_then(|v| v.as_str()),
            r.get("payload").and_then(|p| p.get("workspace")).and_then(|v| v.as_str()),
            r.get("workspace").and_then(|v| v.as_str()),
        ];
        for c in candidates {
            if let Some(s) = c {
                if !s.is_empty() { return Some(s.to_string()); }
            }
        }
    }
    None
}

#[derive(Default)]
struct AnalysisOut {
    last_user_prompt: Option<String>,
    last_assistant: Option<String>,
    last_tool: Option<String>,
    current_tool: Option<String>,
    in_flight: u32,
    last_ts: u64,
    finished: bool,
}

fn analyse(records: &[Value]) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut tool_call_ids: Vec<String> = Vec::new();
    let mut completed: HashSet<String> = HashSet::new();

    for r in records {
        // Unwrap the optional `payload` envelope.
        let payload = r.get("payload").unwrap_or(r);
        let kind = payload.get("type").and_then(|v| v.as_str())
            .or_else(|| r.get("type").and_then(|v| v.as_str()))
            .unwrap_or("");

        match kind {
            "function_call" | "tool_use" | "local_shell_call" => {
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                out.last_tool = Some(name.to_string());
                out.current_tool = Some(name.to_string());
                if let Some(id) = payload.get("call_id").and_then(|v| v.as_str())
                    .or_else(|| payload.get("id").and_then(|v| v.as_str())) {
                    tool_call_ids.push(id.to_string());
                }
            }
            "function_call_output" | "tool_result" | "local_shell_call_output" => {
                if let Some(id) = payload.get("call_id").and_then(|v| v.as_str())
                    .or_else(|| payload.get("tool_use_id").and_then(|v| v.as_str())) {
                    completed.insert(id.to_string());
                }
                out.current_tool = None;
            }
            "message" | "response" => {
                let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let text = extract_text(payload);
                if !text.is_empty() {
                    if role == "user" || role == "human" {
                        out.last_user_prompt = Some(text.chars().take(120).collect());
                    } else if role == "assistant" || role == "model" {
                        out.last_assistant = Some(text.chars().take(120).collect());
                    }
                }
            }
            "session_end" | "stop" => {
                out.finished = true;
            }
            _ => {}
        }

        if let Some(t) = r.get("timestamp").and_then(|v| v.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
                let ms = dt.timestamp_millis() as u64;
                if ms > out.last_ts { out.last_ts = ms; }
            }
        }
    }

    let mut in_flight = 0u32;
    for id in &tool_call_ids {
        if !completed.contains(id) { in_flight += 1; }
    }
    out.in_flight = in_flight;
    out
}

fn extract_text(payload: &Value) -> String {
    // Codex content arrays use "input_text" / "output_text" / "text"; we also
    // accept a plain string for the simple shape.
    if let Some(s) = payload.get("content").and_then(|v| v.as_str()) {
        return s.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    if let Some(arr) = payload.get("content").and_then(|v| v.as_array()) {
        let mut out = String::new();
        for c in arr {
            let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if t == "input_text" || t == "output_text" || t == "text" {
                if let Some(s) = c.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() { out.push(' '); }
                    out.push_str(s);
                }
            }
        }
        return out.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    String::new()
}

fn classify_status(is_live: bool, age_ms: u64, finished: bool, has_in_flight: bool) -> Status {
    if is_live && age_ms < BUSY_WINDOW_MS { return Status::Busy; }
    if is_live && has_in_flight { return Status::Spawning; }
    if is_live && age_ms < ACTIVE_WINDOW_MS { return Status::Active; }
    if is_live { return Status::Idle; }
    if finished { return Status::Completed; }
    if age_ms < RECENT_WINDOW_MS { return Status::Waiting; }
    Status::Stale
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let root = root();
    if !root.exists() {
        return SessionsResult::empty();
    }
    // Map cwd -> live codex pid.
    let mut cwd_to_pid: HashMap<String, u32> = HashMap::new();
    for a in live_agents {
        if a.label == "codex" || a.label == "openai-codex" {
            cwd_to_pid.insert(a.cwd.to_string(), a.pid);
        }
    }

    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();

    let files = walk_jsonls(&root, 4);
    // Group files by their session_meta cwd. Some users have many rollouts
    // for the same project; the most recently modified one is "the" session
    // for that cwd.
    let mut by_cwd: HashMap<String, Vec<(PathBuf, u64, u64)>> = HashMap::new();
    let mut orphan: Vec<(PathBuf, u64, u64)> = Vec::new();

    for path in files {
        let md = match fs::metadata(&path) { Ok(m) => m, Err(_) => continue };
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let size = md.len();
        // Cheap-only: don't bother scanning sessions older than the recent
        // window unless they belong to a live process.
        let age_ms = now_ms.saturating_sub(mtime);
        if age_ms > RECENT_WINDOW_MS && live_agents.is_empty() { continue; }
        let head = read_head(&path, HEAD_BYTES);
        if let Some(cwd) = extract_cwd_from_meta(&head) {
            by_cwd.entry(cwd).or_default().push((path, mtime, size));
        } else {
            orphan.push((path, mtime, size));
        }
    }

    // Sort each cwd group by mtime desc and process.
    for (cwd, mut files) in by_cwd {
        files.sort_by(|a, b| b.1.cmp(&a.1));
        let live_pid = cwd_to_pid.get(&cwd).copied();
        for (i, (path, mtime, size)) in files.iter().enumerate() {
            let is_most_recent = i == 0;
            let age_ms = now_ms.saturating_sub(*mtime);
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

            // Only do the expensive tail+parse for live or recently-active.
            let info = if (is_most_recent && live_pid.is_some()) || age_ms < RECENT_WINDOW_MS {
                analyse(&parse_lines(&read_tail(path, TAIL_BYTES)))
            } else {
                AnalysisOut::default()
            };

            let status = classify_status(
                is_most_recent && live_pid.is_some(),
                age_ms,
                info.finished,
                info.in_flight > 0,
            );

            let last_task = info.last_assistant.clone()
                .or(info.last_user_prompt.clone());

            let proj_short = project_basename(&cwd);
            let sess = Session {
                id: id.clone(),
                project: cwd.clone(),
                project_short: proj_short.clone(),
                file: path.to_string_lossy().into_owned(),
                size_bytes: *size,
                mtime_ms: *mtime,
                age_ms,
                status,
                stop_reason: if info.finished { Some("session_end".to_string()) } else { None },
                last_task: last_task.clone(),
                last_tool: info.last_tool.clone(),
                current_tool: info.current_tool.clone(),
                in_flight_tasks: info.in_flight,
                live_pid: if is_most_recent { live_pid } else { None },
                is_most_recent,
            };

            if is_most_recent {
                if let Some(pid) = live_pid {
                    by_pid.entry(pid).or_insert_with(|| sess.clone());
                }
            }

            if let Some(t) = &last_task {
                if age_ms < RECENT_WINDOW_MS {
                    let task = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    recent_tasks.push(RecentTask {
                        project: cwd.clone(),
                        project_short: proj_short.clone(),
                        task: task.chars().take(120).collect(),
                        mtime_ms: *mtime,
                        status,
                    });
                }
            }

            sessions.push(sess);
        }
    }

    // Sessions whose meta line we couldn't parse — still surface their mtime
    // as "waiting" so they show up in the panel.
    for (path, mtime, size) in orphan {
        let age_ms = now_ms.saturating_sub(mtime);
        if age_ms > RECENT_WINDOW_MS { continue; }
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        sessions.push(Session {
            id, project: "?".into(), project_short: "?".into(),
            file: path.to_string_lossy().into_owned(),
            size_bytes: size, mtime_ms: mtime, age_ms,
            status: Status::Waiting,
            stop_reason: None, last_task: None, last_tool: None,
            current_tool: None, in_flight_tasks: 0, live_pid: None,
            is_most_recent: false,
        });
    }

    sessions.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    recent_tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    if recent_tasks.len() > 20 { recent_tasks.truncate(20); }

    let waiting   = sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = sessions.iter().filter(|s| matches!(s.status, Status::Active | Status::Busy | Status::Spawning | Status::Idle)).count() as u32;
    let busy      = sessions.iter().filter(|s| matches!(s.status, Status::Busy | Status::Spawning)).count() as u32;

    SessionsResult {
        sessions: Sessions { sessions, recent_tasks, active, busy, waiting, completed },
        by_pid,
    }
}
