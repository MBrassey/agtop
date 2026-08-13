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

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS: u64 = 30 * 1000;        // 30s — covers mid-turn tool waits
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;  // 5 minutes
const TAIL_BYTES: u64 = 256 * 1024;
const HEAD_BYTES: u64 = 4 * 1024; // session_meta is at the top of the file

/// Every `<home>/.codex/sessions` that exists on disk — own home
/// plus any extras (WSL `/mnt/c/Users/*`, `AGTOP_EXTRA_HOMES`).
fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::paths::home_roots().into_iter()
        .map(|h| h.join(".codex").join("sessions"))
        .filter(|p| p.exists())
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

fn read_tail(path: &Path, bytes: u64) -> String {
    crate::readfile::tail(path, bytes)
}

fn read_head(path: &Path, bytes: u64) -> String {
    // Lossy UTF-8 head read: the previous `read_to_string` returned "" when
    // the 4 KiB header boundary split a multibyte char, which *permanently*
    // orphaned that rollout (its session_meta cwd never parsed).
    crate::readfile::head(path, bytes)
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
            // Skip symlinks: a symlink inside ~/.codex/sessions pointing
            // at `/` would otherwise let the walker traverse the whole
            // filesystem (capped only by max_depth).
            if ft.is_symlink() { continue; }
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
        for s in candidates.into_iter().flatten() {
            // Sanitize at the parse boundary — this string comes from
            // session-file content, not a real process cwd, and flows
            // into the rendered project column.
            if !s.is_empty() { return Some(sanitize_control(s)); }
        }
    }
    None
}

/// A rollout's session_meta cwd is written once at file creation and never
/// changes, so cache the header parse per path instead of re-reading 4 KiB
/// per rollout per tick.  A `None` entry is retried only while the file is
/// still growing (header may not have been flushed on first sight).
type CwdCache = Mutex<HashMap<PathBuf, (u64, Option<String>)>>;
static CWD_CACHE: OnceLock<CwdCache> = OnceLock::new();

fn cwd_for(path: &Path, size: u64) -> Option<String> {
    let cache = CWD_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some((seen_size, cwd)) = map.get(path) {
            if cwd.is_some() || *seen_size >= size {
                return cwd.clone();
            }
        }
    }
    let cwd = extract_cwd_from_meta(&read_head(path, HEAD_BYTES));
    if let Ok(mut map) = cache.lock() {
        map.insert(path.to_path_buf(), (size, cwd.clone()));
    }
    cwd
}

#[derive(Default)]
struct AnalysisOut {
    last_user_prompt: Option<String>,
    last_assistant: Option<String>,
    last_tool: Option<String>,
    current_tool: Option<String>,
    in_flight: u32,           // task / agent subagents only
    in_flight_tools: u32,     // any tool, used for busy-status decision
    last_ts: u64,
    finished: bool,
    tokens_input: u64,
    tokens_output: u64,
    /// Prompt-cache read tokens — a subset of `tokens_input`, never an
    /// additional bucket.  Used for the cached-rate cost discount.
    tokens_cache_read: u64,
    /// Latest assistant turn's prompt size — see Claude::context_used.
    context_used: u64,
    model: Option<String>,
    /// Capped, prefix-tagged tail (`›` prose, `→` tool, `←` result) for
    /// the detail-popup live preview.
    recent_activity: Vec<String>,
}

fn push_recent(buf: &mut Vec<String>, line: String) {
    if buf.last().map(|s| s == &line).unwrap_or(false) { return; }
    buf.push(line);
    if buf.len() > 12 { buf.remove(0); }
}

fn analyse(records: &[Value]) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut tool_call_ids: Vec<String> = Vec::new();    // Task / Agent
    let mut all_tool_ids:  Vec<String> = Vec::new();    // any tool
    let mut completed: HashSet<String> = HashSet::new();
    // Set once a cumulative token_count event is seen; the per-event
    // legacy accumulator must not add on top of cumulative totals.
    let mut saw_cumulative = false;

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
                    all_tool_ids.push(id.to_string());
                    if name == "Task" || name == "Agent" {
                        tool_call_ids.push(id.to_string());
                    }
                }
                let arg_hint = payload.get("arguments").and_then(|v| v.as_str())
                    .or_else(|| payload.get("input").and_then(|i|
                        i.get("command").and_then(|v| v.as_str())
                            .or_else(|| i.get("file_path").and_then(|v| v.as_str()))
                            .or_else(|| i.get("subject").and_then(|v| v.as_str()))
                            .or_else(|| i.get("path").and_then(|v| v.as_str()))))
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let hint: String = arg_hint.chars().take(120).collect();
                let line = if hint.is_empty() { format!("→ {}", name) }
                           else { format!("→ {}: {}", name, hint) };
                push_recent(&mut out.recent_activity, line);
            }
            "function_call_output" | "tool_result" | "local_shell_call_output" => {
                if let Some(id) = payload.get("call_id").and_then(|v| v.as_str())
                    .or_else(|| payload.get("tool_use_id").and_then(|v| v.as_str())) {
                    completed.insert(id.to_string());
                }
                out.current_tool = None;
                let preview = payload.get("output").and_then(|v| v.as_str())
                    .or_else(|| payload.get("content").and_then(|v| v.as_str()))
                    .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let hint: String = preview.chars().take(120).collect();
                let line = if hint.is_empty() { "← (ok)".to_string() }
                           else { format!("← {}", hint) };
                push_recent(&mut out.recent_activity, line);
            }
            "message" | "response" => {
                let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let text = extract_text(payload);
                if !text.is_empty() {
                    let snippet: String = text.chars().take(120).collect();
                    if role == "user" || role == "human" {
                        out.last_user_prompt = Some(snippet.clone());
                        push_recent(&mut out.recent_activity, format!("› {}", snippet));
                    } else if role == "assistant" || role == "model" {
                        out.last_assistant = Some(snippet.clone());
                        push_recent(&mut out.recent_activity, format!("› {}", snippet));
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

        // Modern rollouts carry token data as event_msg/token_count with
        // payload.info.total_token_usage — CUMULATIVE per session, so the
        // totals are assigned (last record wins), never summed.
        if let Some(info) = payload.get("info") {
            if let Some(tot) = info.get("total_token_usage") {
                let it = tot.get("input_tokens").and_then(|v| v.as_u64());
                let ot = tot.get("output_tokens").and_then(|v| v.as_u64());
                if it.is_some() || ot.is_some() {
                    out.tokens_input  = it.unwrap_or(0);
                    out.tokens_output = ot.unwrap_or(0);
                    // cached_input_tokens is a subset of input_tokens.
                    out.tokens_cache_read = tot.get("cached_input_tokens")
                        .and_then(|v| v.as_u64()).unwrap_or(0);
                    saw_cumulative = true;
                }
            }
            if let Some(last) = info.get("last_token_usage") {
                if let Some(iu) = last.get("input_tokens").and_then(|v| v.as_u64()) {
                    // Latest turn's prompt size = current context fill
                    // (input_tokens already includes the cached portion).
                    out.context_used = iu;
                    saw_cumulative = true;
                }
            }
        }

        // Legacy usage blocks on response.completed and a few other events.
        // Probe both nested-payload and flat shapes.
        let usage = payload.get("usage")
            .or_else(|| r.get("usage"))
            .or_else(|| payload.get("response").and_then(|r| r.get("usage")));
        if let Some(u) = usage {
            if !saw_cumulative {
                // OpenAI uses input_tokens / output_tokens (sometimes prompt_tokens /
                // completion_tokens on older APIs). Accept either.
                let it = u.get("input_tokens").and_then(|v| v.as_u64())
                    .or_else(|| u.get("prompt_tokens").and_then(|v| v.as_u64()))
                    .unwrap_or(0);
                let ot = u.get("output_tokens").and_then(|v| v.as_u64())
                    .or_else(|| u.get("completion_tokens").and_then(|v| v.as_u64()))
                    .unwrap_or(0);
                // Cached reads are a subset of input_tokens — track them
                // for the cost discount, never add them on top.
                let cr = u.get("input_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(|v| v.as_u64()).unwrap_or(0);
                out.tokens_input  = out.tokens_input.saturating_add(it);
                out.tokens_output = out.tokens_output.saturating_add(ot);
                out.tokens_cache_read = out.tokens_cache_read.saturating_add(cr);
                // Latest-turn prompt size = current context fill.  Records
                // iterate oldest → newest so the last assignment wins.
                out.context_used = it;
            }
        }

        // Model — try every place codex might mention it.
        let model_str = payload.get("model").and_then(|v| v.as_str())
            .or_else(|| r.get("model").and_then(|v| v.as_str()))
            .or_else(|| payload.get("response").and_then(|r| r.get("model")).and_then(|v| v.as_str()));
        if let Some(m) = model_str {
            out.model = Some(m.to_string());
        }
    }

    out.in_flight = tool_call_ids.iter().filter(|id| !completed.contains(*id)).count() as u32;
    out.in_flight_tools = all_tool_ids.iter().filter(|id| !completed.contains(*id)).count() as u32;
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

fn classify_status(
    is_live: bool, age_ms: u64, finished: bool,
    has_in_flight_task: bool, has_in_flight_tool: bool,
) -> Status {
    if is_live && has_in_flight_task { return Status::Spawning; }
    if is_live && (age_ms < BUSY_WINDOW_MS || has_in_flight_tool) { return Status::Busy; }
    if is_live && age_ms < ACTIVE_WINDOW_MS { return Status::Active; }
    if is_live { return Status::Idle; }
    if finished { return Status::Completed; }
    if age_ms < RECENT_WINDOW_MS { return Status::Waiting; }
    Status::Stale
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let roots = roots();
    if roots.is_empty() {
        return SessionsResult::empty();
    }
    // Map cwd -> live codex pids.  A Vec because parallel codex sessions
    // can share a cwd — a single-pid map non-deterministically dropped
    // all-but-one of them (same race claude.rs fixed in 2.4.4).  Sorted
    // newest-pid-first (lowest uptime) so the freshest rollout below
    // pairs with the newest process.
    let mut cwd_to_pids: HashMap<String, Vec<(u32, u64)>> = HashMap::new();
    for a in live_agents {
        if a.label == "codex" || a.label == "openai-codex" {
            cwd_to_pids.entry(a.cwd.to_string()).or_default().push((a.pid, a.uptime_sec));
        }
    }
    for v in cwd_to_pids.values_mut() {
        v.sort_by_key(|(_pid, uptime)| *uptime);
    }

    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();

    // Multi-root walk: WSL build picks up Windows-side sessions and
    // vice versa.  Canonicalise per-file before insert to drop cross-mount
    // duplicates (same file at `/home/u/.codex/...` and
    // `/mnt/c/Users/u/.codex/...`).
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut files: Vec<PathBuf> = Vec::new();
    for r in &roots {
        for f in walk_jsonls(r, 4) {
            let canon = std::fs::canonicalize(&f).unwrap_or_else(|_| f.clone());
            if seen.insert(canon) { files.push(f); }
        }
    }
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
        // window unless a *Codex* process is live.  Gating on the all-vendor
        // live set meant any running agent re-scanned every historical Codex
        // rollout's header each tick.
        let age_ms = now_ms.saturating_sub(mtime);
        if age_ms > RECENT_WINDOW_MS && cwd_to_pids.is_empty() { continue; }
        if let Some(cwd) = cwd_for(&path, size) {
            by_cwd.entry(cwd).or_default().push((path, mtime, size));
        } else {
            orphan.push((path, mtime, size));
        }
    }

    // Sort each cwd group by mtime desc and process.
    for (cwd, mut files) in by_cwd {
        files.sort_by_key(|x| std::cmp::Reverse(x.1));
        let live_pids = cwd_to_pids.get(&cwd);
        for (i, (path, mtime, size)) in files.iter().enumerate() {
            let is_most_recent = i == 0;
            // Freshest rollout → newest pid, next rollout → next pid.
            let live_pid = live_pids.and_then(|v| v.get(i)).map(|(pid, _)| *pid);
            let age_ms = now_ms.saturating_sub(*mtime);
            let id = path.file_stem().map(|s| sanitize_control(&s.to_string_lossy())).unwrap_or_default();

            // Only do the expensive tail+parse for live or recently-active.
            let info = if live_pid.is_some() || age_ms < RECENT_WINDOW_MS {
                analyse(&parse_lines(&read_tail(path, TAIL_BYTES)))
            } else {
                AnalysisOut::default()
            };

            let status = classify_status(
                live_pid.is_some(),
                age_ms,
                info.finished,
                info.in_flight > 0,
                info.in_flight_tools > 0,
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
                last_task:    last_task.as_deref().map(sanitize_control),
                last_tool:    info.last_tool.as_deref().map(sanitize_control),
                current_tool: info.current_tool.as_deref().map(sanitize_control),
                in_flight_subagents: Vec::new(),
                recent_activity: info.recent_activity.iter()
                    .map(|s| sanitize_control(s)).collect(),
                in_flight_tasks: info.in_flight,
                live_pid,
                is_most_recent,
                tokens_input: info.tokens_input,
                tokens_output: info.tokens_output,
                tokens_total: info.tokens_input.saturating_add(info.tokens_output),
                tokens_cache_read: info.tokens_cache_read,
                tokens_cache_write: 0,
                cost_usd: 0.0,
                context_used: info.context_used,
                session_started_ms: 0,
                tool_counts: Vec::new(),
                model: info.model.as_deref().map(sanitize_control),
            };

            if let Some(pid) = live_pid {
                by_pid.entry(pid).or_insert_with(|| sess.clone());
            }

            if let Some(t) = &last_task {
                if age_ms < RECENT_WINDOW_MS {
                    let task = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    recent_tasks.push(RecentTask {
                        project: cwd.clone(),
                        project_short: proj_short.clone(),
                        task: sanitize_control(&task).chars().take(120).collect(),
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
        let id = path.file_stem().map(|s| sanitize_control(&s.to_string_lossy())).unwrap_or_default();
        sessions.push(Session {
            id, project: "?".into(), project_short: "?".into(),
            file: path.to_string_lossy().into_owned(),
            size_bytes: size, mtime_ms: mtime, age_ms,
            status: Status::Waiting,
            stop_reason: None, last_task: None, last_tool: None,
            current_tool: None, in_flight_tasks: 0,
            in_flight_subagents: Vec::new(),
            recent_activity: Vec::new(),
            live_pid: None,
            is_most_recent: false,
            tokens_input: 0, tokens_output: 0, tokens_total: 0,
            tokens_cache_read: 0, tokens_cache_write: 0,
            cost_usd: 0.0, context_used: 0,
            session_started_ms: 0, tool_counts: Vec::new(),
            model: None,
        });
    }

    sessions.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
    recent_tasks.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn recs(lines: &[&str]) -> Vec<Value> {
        parse_lines(&lines.join("\n"))
    }

    // Modern rollouts: event_msg/token_count carries cumulative totals —
    // last record wins, cached is a subset of input, context fill comes
    // from the last turn's input size.
    #[test]
    fn modern_token_count_events_are_cumulative() {
        let out = analyse(&recs(&[
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":50,"total_tokens":1050},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":50},"model_context_window":272000}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5000,"cached_input_tokens":4200,"output_tokens":300,"total_tokens":5300},"last_token_usage":{"input_tokens":4000,"cached_input_tokens":3400,"output_tokens":250},"model_context_window":272000}}}"#,
        ]));
        assert_eq!(out.tokens_input, 5000);
        assert_eq!(out.tokens_output, 300);
        assert_eq!(out.tokens_cache_read, 4200);
        assert_eq!(out.context_used, 4000);
    }

    // Legacy usage blocks: input_tokens already includes cached tokens —
    // a 100k-input turn with 90k cached must not count as 190k.
    #[test]
    fn legacy_usage_does_not_double_count_cached() {
        let out = analyse(&recs(&[
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","usage":{"input_tokens":100000,"output_tokens":500,"input_tokens_details":{"cached_tokens":90000}}}}"#,
        ]));
        assert_eq!(out.tokens_input, 100000);
        assert_eq!(out.tokens_output, 500);
        assert_eq!(out.tokens_cache_read, 90000);
        assert_eq!(out.context_used, 100000);
    }

    // Mixed files: once a cumulative record is seen, legacy per-event
    // usage must not add on top of it.
    #[test]
    fn legacy_events_do_not_add_on_top_of_cumulative() {
        let out = analyse(&recs(&[
            r#"{"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2000,"cached_input_tokens":0,"output_tokens":100}}}}"#,
            r#"{"payload":{"usage":{"input_tokens":500,"output_tokens":20}}}"#,
        ]));
        assert_eq!(out.tokens_input, 2000);
        assert_eq!(out.tokens_output, 100);
    }

    // The session_meta cwd is untrusted file content rendered in the
    // project column — bidi/format controls must be stripped at parse.
    #[test]
    fn meta_cwd_is_sanitized() {
        let cwd = extract_cwd_from_meta(
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/tmp/\u{202e}evil\"}}").unwrap();
        assert!(!cwd.contains('\u{202e}'));
        assert!(cwd.contains("evil"));
    }
}
