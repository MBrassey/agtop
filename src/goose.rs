// Block goose session reader.  Goose stores conversations as JSON or JSONL
// under one of:
//
//   ~/.config/goose/sessions/<id>.jsonl
//   ~/.local/share/goose/sessions/<id>.jsonl
//   ~/.config/goose/sessions/<id>.json
//
// The schema has shipped in a few shapes; we probe defensively.  Token usage
// is parsed when the goose client surfaces it (recent versions do via
// usage{input_tokens,output_tokens}).

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS:   u64 = 30 * 1000;
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;
const TAIL_BYTES:       u64 = 256 * 1024;

fn candidate_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".config").join("goose").join("sessions"));
        v.push(h.join(".local").join("share").join("goose").join("sessions"));
    }
    v
}

/// Tail-read cap (real callers use 64 KiB).  Hard-clamped to keep `take as
/// i64` and `take as usize` safe on 32-bit + against pathological inputs.
const MAX_TAIL: u64 = 64 * 1024 * 1024;

fn read_tail(path: &Path, bytes: u64) -> String {
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return String::new() };
    let len = match f.metadata() { Ok(m) => m.len(), Err(_) => return String::new() };
    if len == 0 { return String::new(); }
    let take = bytes.min(len).min(MAX_TAIL);
    if f.seek(SeekFrom::End(-(take as i64))).is_err() { return String::new(); }
    let mut buf = String::with_capacity(take as usize);
    let _ = f.take(take).read_to_string(&mut buf);
    buf
}

fn read_whole(path: &Path) -> String {
    let mut buf = String::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_string(&mut buf);
    }
    buf
}

#[derive(Default)]
struct AnalysisOut {
    last_user: Option<String>,
    last_assistant: Option<String>,
    last_tool: Option<String>,
    current_tool: Option<String>,
    in_flight: u32,
    finished: bool,
    tokens_input: u64,
    tokens_output: u64,
    model: Option<String>,
    cwd: Option<String>,
    recent_activity: Vec<String>,
}

fn push_recent(buf: &mut Vec<String>, line: String) {
    if buf.last().map(|s| s == &line).unwrap_or(false) { return; }
    buf.push(line);
    if buf.len() > 12 { buf.remove(0); }
}

fn analyse_jsonl(text: &str) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut tool_ids: Vec<String> = Vec::new();
    let mut completed: HashMap<String, ()> = HashMap::new();

    for line in text.split('\n') {
        let line = line.trim();
        if line.is_empty() { continue; }
        let r: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        if let Some(c) = r.get("cwd").and_then(|v| v.as_str()) { out.cwd = Some(c.into()); }
        if let Some(c) = r.get("workspace").and_then(|v| v.as_str()) { out.cwd = Some(c.into()); }
        if let Some(m) = r.get("model").and_then(|v| v.as_str()) { out.model = Some(m.into()); }
        if let Some(u) = r.get("usage") {
            out.tokens_input  += u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            out.tokens_output += u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        }
        let kind = r.get("type").and_then(|v| v.as_str())
            .or_else(|| r.get("role").and_then(|v| v.as_str()))
            .unwrap_or("");
        match kind {
            "tool_request" | "tool_use" | "tool_call" | "function_call" => {
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                out.last_tool = Some(name.into());
                out.current_tool = Some(name.into());
                if let Some(id) = r.get("id").and_then(|v| v.as_str()) { tool_ids.push(id.into()); }
                let arg = r.get("arguments").and_then(|v| v.as_str())
                    .or_else(|| r.get("input").and_then(|i| i.as_str()))
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let hint: String = arg.chars().take(120).collect();
                let line = if hint.is_empty() { format!("→ {}", name) } else { format!("→ {}: {}", name, hint) };
                push_recent(&mut out.recent_activity, line);
            }
            "tool_response" | "tool_result" | "function_call_output" => {
                if let Some(id) = r.get("id").and_then(|v| v.as_str())
                    .or_else(|| r.get("tool_use_id").and_then(|v| v.as_str())) {
                    completed.insert(id.into(), ());
                }
                out.current_tool = None;
                let preview = r.get("output").and_then(|v| v.as_str())
                    .or_else(|| r.get("content").and_then(|v| v.as_str()))
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let hint: String = preview.chars().take(120).collect();
                let line = if hint.is_empty() { "← (ok)".into() } else { format!("← {}", hint) };
                push_recent(&mut out.recent_activity, line);
            }
            "user" | "human" => {
                if let Some(t) = r.get("content").and_then(|v| v.as_str()) {
                    let snip: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
                    out.last_user = Some(snip.clone());
                    push_recent(&mut out.recent_activity, format!("› {}", snip));
                }
            }
            "assistant" | "model" => {
                if let Some(t) = r.get("content").and_then(|v| v.as_str()) {
                    let snip: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
                    out.last_assistant = Some(snip.clone());
                    push_recent(&mut out.recent_activity, format!("› {}", snip));
                }
            }
            "session_end" | "stop" => out.finished = true,
            _ => {}
        }
    }
    out.in_flight = tool_ids.iter().filter(|id| !completed.contains_key(*id)).count() as u32;
    out
}

fn analyse_json(text: &str) -> AnalysisOut {
    // Single-object format: { "messages": [...], "metadata": {...} }
    let mut out = AnalysisOut::default();
    let v: Value = match serde_json::from_str(text) { Ok(v) => v, Err(_) => return out };
    if let Some(m) = v.get("metadata") {
        if let Some(c) = m.get("cwd").and_then(|x| x.as_str()) { out.cwd = Some(c.into()); }
        if let Some(c) = m.get("working_dir").and_then(|x| x.as_str()) { out.cwd = Some(c.into()); }
        if let Some(c) = m.get("model").and_then(|x| x.as_str()) { out.model = Some(c.into()); }
    }
    if let Some(arr) = v.get("messages").and_then(|x| x.as_array()) {
        for m in arr {
            let role = m.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let content = m.get("content").and_then(|x| x.as_str()).unwrap_or("");
            if role == "user" {
                out.last_user = Some(content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect());
            } else if role == "assistant" {
                out.last_assistant = Some(content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect());
            }
            if let Some(u) = m.get("usage") {
                out.tokens_input  += u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                out.tokens_output += u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            }
        }
    }
    out
}

fn classify(is_live: bool, age_ms: u64, finished: bool, in_flight: bool) -> Status {
    if is_live && age_ms < BUSY_WINDOW_MS   { return Status::Busy; }
    if is_live && in_flight                 { return Status::Spawning; }
    if is_live && age_ms < ACTIVE_WINDOW_MS { return Status::Active; }
    if is_live                              { return Status::Idle; }
    if finished                             { return Status::Completed; }
    if age_ms < RECENT_WINDOW_MS            { return Status::Waiting; }
    Status::Stale
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let roots: Vec<PathBuf> = candidate_roots().into_iter().filter(|p| p.exists()).collect();
    if roots.is_empty() { return SessionsResult::empty(); }

    let mut cwd_to_pid: HashMap<String, u32> = HashMap::new();
    for a in live_agents {
        if a.label == "goose" || a.label == "block-goose" {
            cwd_to_pid.insert(a.cwd.into(), a.pid);
        }
    }

    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();

    for root in roots {
        let rd = match fs::read_dir(&root) { Ok(d) => d, Err(_) => continue };
        for ent in rd.flatten() {
            let p = ent.path();
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "jsonl" && ext != "json" { continue; }
            let md = match fs::metadata(&p) { Ok(m) => m, Err(_) => continue };
            let mtime = md.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64).unwrap_or(0);
            let size = md.len();
            let age_ms = now_ms.saturating_sub(mtime);
            if age_ms > RECENT_WINDOW_MS && live_agents.is_empty() { continue; }

            let info = if ext == "jsonl" {
                analyse_jsonl(&read_tail(&p, TAIL_BYTES))
            } else {
                analyse_json(&read_whole(&p))
            };

            let cwd = info.cwd.clone().unwrap_or_default();
            let proj_short = if !cwd.is_empty() { project_basename(&cwd) } else { String::new() };
            let live_pid = if !cwd.is_empty() { cwd_to_pid.get(&cwd).copied() } else { None };
            let status = classify(live_pid.is_some(), age_ms, info.finished, info.in_flight > 0);
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let last_task = info.last_assistant.clone().or(info.last_user.clone());

            let sess = Session {
                id: id.clone(),
                project: cwd.clone(),
                project_short: proj_short.clone(),
                file: p.to_string_lossy().into_owned(),
                size_bytes: size,
                mtime_ms: mtime,
                age_ms,
                status,
                stop_reason: if info.finished { Some("session_end".into()) } else { None },
                last_task:    last_task.as_deref().map(sanitize_control),
                last_tool:    info.last_tool.as_deref().map(sanitize_control),
                current_tool: info.current_tool.as_deref().map(sanitize_control),
                in_flight_tasks: info.in_flight,
                in_flight_subagents: Vec::new(),
                recent_activity: info.recent_activity.iter()
                    .map(|s| sanitize_control(s)).collect(),
                live_pid,
                is_most_recent: true,
                tokens_input: info.tokens_input,
                tokens_output: info.tokens_output,
                tokens_total: info.tokens_input + info.tokens_output,
                tokens_cache_read: 0,
                tokens_cache_write: 0,
                cost_usd: 0.0,
                context_used: 0,
                session_started_ms: 0,
                tool_counts: Vec::new(),
                model: info.model.as_deref().map(sanitize_control),
            };
            if let Some(pid) = live_pid {
                by_pid.entry(pid).or_insert_with(|| sess.clone());
            }
            if let Some(t) = &last_task {
                if age_ms < RECENT_WINDOW_MS {
                    recent_tasks.push(RecentTask {
                        project: cwd.clone(), project_short: proj_short.clone(),
                        task: t.clone(), mtime_ms: mtime, status,
                    });
                }
            }
            sessions.push(sess);
        }
    }

    sessions.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    recent_tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    if recent_tasks.len() > 20 { recent_tasks.truncate(20); }
    let waiting   = sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = sessions.iter().filter(|s| matches!(s.status, Status::Active|Status::Busy|Status::Spawning|Status::Idle)).count() as u32;
    let busy      = sessions.iter().filter(|s| matches!(s.status, Status::Busy|Status::Spawning)).count() as u32;
    SessionsResult {
        sessions: Sessions { sessions, recent_tasks, active, busy, waiting, completed },
        by_pid,
    }
}
