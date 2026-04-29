// Reads ~/.claude/projects/*/<session>.jsonl best-effort to surface live agent
// status, current tool, in-flight Task subagents, and the last task subject.

use crate::format::project_basename;
use crate::model::{RecentTask, Session, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
pub const BUSY_WINDOW_MS: u64 = 5 * 1000;
pub const ACTIVE_WINDOW_MS: u64 = 60 * 1000;
pub const TAIL_BYTES: u64 = 256 * 1024;

pub fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("projects")
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

fn parse_lines(text: &str) -> Vec<Value> {
    text.split('\n')
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[derive(Default, Debug, Clone)]
struct AnalysisOut {
    stop_reason: Option<String>,
    last_task: Option<String>,
    last_tool: Option<String>,
    current_tool: Option<String>,
    in_flight_tasks: u32,
    tokens_input: u64,
    tokens_output: u64,
}

fn analyse(records: &[Value]) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut task_use_ids: Vec<String> = Vec::new();
    let mut completed: HashMap<String, ()> = HashMap::new();

    for r in records {
        if let Some(sr) = r.get("stop_reason").and_then(|v| v.as_str()) {
            out.stop_reason = Some(sr.to_string());
        } else if let Some(sr) = r.get("message").and_then(|m| m.get("stop_reason")).and_then(|v| v.as_str()) {
            out.stop_reason = Some(sr.to_string());
        }

        // Token usage — Claude attaches a usage block to each assistant
        // message. Sum across the whole transcript.
        if let Some(usage) = r.get("message").and_then(|m| m.get("usage")) {
            let it = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let ot = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            // Cache-read counts as charged input under most pricing schemes.
            let cr = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cc = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            out.tokens_input  += it + cr + cc;
            out.tokens_output += ot;
        }

        let content_holder = r.get("message").and_then(|m| m.get("content")).cloned()
            .or_else(|| r.get("content").cloned());
        if let Some(content) = content_holder {
            if let Some(arr) = content.as_array() {
                for c in arr {
                    let kind = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match kind {
                        "tool_use" => {
                            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if !name.is_empty() {
                                out.last_tool = Some(name.to_string());
                                out.current_tool = Some(name.to_string());
                            }
                            if name == "Task" || name == "Agent" {
                                if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                                    task_use_ids.push(id.to_string());
                                }
                                if let Some(input) = c.get("input") {
                                    let subj = input.get("subject")
                                        .or_else(|| input.get("description"))
                                        .and_then(|v| v.as_str());
                                    if let Some(s) = subj {
                                        out.last_task = Some(s.to_string());
                                    }
                                }
                            } else if name == "TodoWrite" {
                                if let Some(todos) = c.get("input").and_then(|i| i.get("todos")).and_then(|v| v.as_array()) {
                                    if let Some(in_prog) = todos.iter().find(|t| t.get("status").and_then(|v| v.as_str()) == Some("in_progress")) {
                                        if let Some(t) = in_prog.get("content").and_then(|v| v.as_str()) {
                                            out.last_task = Some(t.to_string());
                                        }
                                    }
                                }
                            } else if let Some(subj) = c.get("input").and_then(|i| i.get("subject")).and_then(|v| v.as_str()) {
                                out.last_task = Some(subj.to_string());
                            }
                        }
                        "tool_result" => {
                            if let Some(id) = c.get("tool_use_id").and_then(|v| v.as_str()) {
                                completed.insert(id.to_string(), ());
                            }
                            out.current_tool = None;
                        }
                        "text" => {
                            // Treat assistant prose as a useful "doing" hint, but only if
                            // this record is the assistant speaking.
                            if r.get("type").and_then(|v| v.as_str()) == Some("assistant") {
                                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                                    let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                                    if !trimmed.is_empty() {
                                        out.last_task = Some(trimmed.chars().take(120).collect());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(subj) = r.get("toolUseResult").and_then(|tu| tu.get("subject")).and_then(|v| v.as_str()) {
            out.last_task = Some(subj.to_string());
        }
    }

    let mut in_flight = 0;
    for id in &task_use_ids {
        if !completed.contains_key(id) {
            in_flight += 1;
        }
    }
    out.in_flight_tasks = in_flight;
    out
}

fn decode_project(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    if name.starts_with('-') {
        let mut s = String::from("/");
        s.push_str(&name[1..].replace('-', "/"));
        s
    } else {
        name.to_string()
    }
}

fn classify_status(is_live: bool, age_ms: u64, stop_reason: &Option<String>, has_in_flight: bool) -> Status {
    if is_live && age_ms < BUSY_WINDOW_MS { return Status::Busy; }
    if is_live && has_in_flight { return Status::Spawning; }
    if is_live && age_ms < ACTIVE_WINDOW_MS { return Status::Active; }
    if is_live { return Status::Idle; }
    if matches!(stop_reason.as_deref(), Some("end_turn") | Some("stop_sequence")) {
        return Status::Completed;
    }
    if age_ms < RECENT_WINDOW_MS { return Status::Waiting; }
    Status::Stale
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let root = root();
    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();

    if !root.exists() {
        return SessionsResult::empty();
    }

    let mut cwd_to_pid: HashMap<String, u32> = HashMap::new();
    for a in live_agents {
        if a.label == "claude" || a.label == "claude-code" {
            cwd_to_pid.insert(a.cwd.to_string(), a.pid);
        }
    }

    let read_dir = match fs::read_dir(&root) {
        Ok(d) => d,
        Err(_) => return SessionsResult::empty(),
    };

    for ent in read_dir.flatten() {
        let proj_dir = ent.path();
        if !proj_dir.is_dir() { continue; }
        let raw_name = ent.file_name().to_string_lossy().into_owned();
        let decoded_path = decode_project(&raw_name);
        let proj_short = project_basename(&decoded_path);

        // Find all jsonl files + the most recent one.
        let mut jsonls: Vec<(PathBuf, u64, u64)> = Vec::new();
        let mut most_recent_path: Option<PathBuf> = None;
        let mut most_recent_mtime: u64 = 0;
        let inner = match fs::read_dir(&proj_dir) {
            Ok(d) => d, Err(_) => continue,
        };
        for f in inner.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let md = match fs::metadata(&p) { Ok(m) => m, Err(_) => continue };
            let mtime = md.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64).unwrap_or(0);
            let size = md.len();
            if mtime > most_recent_mtime {
                most_recent_mtime = mtime;
                most_recent_path = Some(p.clone());
            }
            jsonls.push((p, mtime, size));
        }

        for (path, mtime, size) in &jsonls {
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let age_ms = now_ms.saturating_sub(*mtime);
            let is_most_recent = most_recent_path.as_deref() == Some(path);
            let live_pid = if is_most_recent { cwd_to_pid.get(&decoded_path).copied() } else { None };

            // Only do the expensive tail+parse for live or recently-touched sessions.
            let info = if live_pid.is_some() || age_ms < RECENT_WINDOW_MS {
                analyse(&parse_lines(&read_tail(path, TAIL_BYTES)))
            } else {
                AnalysisOut::default()
            };

            let status = classify_status(
                live_pid.is_some(),
                age_ms,
                &info.stop_reason,
                info.in_flight_tasks > 0,
            );

            let sess = Session {
                id: id.clone(),
                project: decoded_path.clone(),
                project_short: proj_short.clone(),
                file: path.to_string_lossy().into_owned(),
                size_bytes: *size,
                mtime_ms: *mtime,
                age_ms,
                status,
                stop_reason: info.stop_reason.clone(),
                last_task: info.last_task.clone(),
                last_tool: info.last_tool.clone(),
                current_tool: info.current_tool.clone(),
                in_flight_tasks: info.in_flight_tasks,
                live_pid,
                is_most_recent,
                tokens_input: info.tokens_input,
                tokens_output: info.tokens_output,
                tokens_total: info.tokens_input + info.tokens_output,
            };

            if let Some(pid) = live_pid {
                by_pid.entry(pid).or_insert_with(|| sess.clone());
            }

            if let Some(t) = &info.last_task {
                if age_ms < RECENT_WINDOW_MS {
                    let task = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    recent_tasks.push(RecentTask {
                        project: decoded_path.clone(),
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

    sessions.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    recent_tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    if recent_tasks.len() > 20 { recent_tasks.truncate(20); }

    let waiting   = sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = sessions.iter().filter(|s| matches!(s.status, Status::Active | Status::Busy | Status::Spawning | Status::Idle)).count() as u32;
    let busy      = sessions.iter().filter(|s| matches!(s.status, Status::Busy | Status::Spawning)).count() as u32;

    SessionsResult {
        sessions: crate::model::Sessions { sessions, recent_tasks, active, busy, waiting, completed },
        by_pid,
    }
}
