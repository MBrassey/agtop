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
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS:   u64 = 30 * 1000;
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;
/// Cap on a single whole-file read of a Gemini session JSON — a 10 GB
/// (or adversarially grown) file must not be slurped into one String.
const MAX_WHOLE: u64 = 16 * 1024 * 1024;

/// Every `<home>/.gemini/sessions` that exists — own home plus any
/// extras (WSL `/mnt/c/Users/*`, `AGTOP_EXTRA_HOMES`).
fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::paths::home_roots().into_iter()
        .map(|h| h.join(".gemini").join("sessions"))
        .filter(|p| p.exists())
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}


#[derive(Default, Clone)]
struct AnalysisOut {
    last_user: Option<String>,
    last_assistant: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    tokens_input: u64,
    tokens_output: u64,
    recent_activity: Vec<String>,
}

/// (mtime, size)-keyed parse cache.  A Gemini session JSON can be many MB
/// and used to be slurped + DOM-parsed for every file every tick; the
/// whole-file parse now runs only when the file actually changed.
type ParseCache = Mutex<HashMap<PathBuf, (u64, u64, AnalysisOut)>>;
static PARSE_CACHE: OnceLock<ParseCache> = OnceLock::new();

fn analyse_cached(p: &Path, mtime: u64, size: u64) -> AnalysisOut {
    let cache = PARSE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some((m, s, out)) = map.get(p) {
            if *m == mtime && *s == size { return out.clone(); }
        }
    }
    let out = analyse(&crate::readfile::whole_capped(p, MAX_WHOLE));
    if let Ok(mut map) = cache.lock() {
        map.insert(p.to_path_buf(), (mtime, size, out.clone()));
    }
    out
}

fn analyse(text: &str) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let v: Value = match serde_json::from_str(text) { Ok(v) => v, Err(_) => return out };
    if let Some(m) = v.get("metadata") {
        // Sanitize at the parse boundary — metadata.cwd is file content,
        // not a real process cwd, and flows into the project column.
        out.cwd = m.get("cwd").and_then(|x| x.as_str()).map(sanitize_control);
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
                let it = u.get("promptTokenCount").and_then(|x| x.as_u64())
                    .or_else(|| u.get("input_tokens").and_then(|x| x.as_u64())).unwrap_or(0);
                let ot = u.get("candidatesTokenCount").and_then(|x| x.as_u64())
                    .or_else(|| u.get("output_tokens").and_then(|x| x.as_u64())).unwrap_or(0);
                // saturating: a crafted u64::MAX token count in the JSON
                // would otherwise panic (debug) / wrap (release).
                out.tokens_input  = out.tokens_input.saturating_add(it);
                out.tokens_output = out.tokens_output.saturating_add(ot);
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
    let roots = roots();
    if roots.is_empty() { return SessionsResult::empty(); }

    // cwd -> live gemini pids, newest-pid-first (lowest uptime).  A Vec
    // because parallel sessions can share a cwd — a single-pid map
    // non-deterministically dropped all-but-one of them.
    let mut cwd_to_pids: HashMap<String, Vec<(u32, u64)>> = HashMap::new();
    for a in live_agents {
        // Both the bare `gemini` binary and the npm-scoped `gemini-cli`
        // classification belong to the same vendor.
        if a.label == "gemini" || a.label == "gemini-cli" {
            cwd_to_pids.entry(a.cwd.into()).or_default().push((a.pid, a.uptime_sec));
        }
    }
    for v in cwd_to_pids.values_mut() {
        v.sort_by_key(|(_pid, uptime)| *uptime);
    }

    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    let mut seen_files: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();

    // Enumerate first, then process newest-first so the freshest session
    // file pairs with the newest live pid in its cwd.
    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();   // (path, mtime, size)
    for root in &roots {
        let rd = match fs::read_dir(root) { Ok(d) => d, Err(_) => continue };
        for ent in rd.flatten() {
            // Cross-mount dedupe: same JSON visible at both `~/.gemini/...`
            // and `/mnt/c/Users/u/.gemini/...` would surface twice.
            let canon = std::fs::canonicalize(ent.path()).unwrap_or_else(|_| ent.path());
            if !seen_files.insert(canon) { continue; }
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
            let md = match fs::metadata(&p) { Ok(m) => m, Err(_) => continue };
            let mtime = md.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64).unwrap_or(0);
            let age_ms = now_ms.saturating_sub(mtime);
            // Skip files older than the recent window when no *Gemini* agent
            // is live.  The old gate (`live_agents.is_empty()`) tested every
            // vendor's live set, so any running agent of any kind forced a
            // full re-read + JSON parse of every historical Gemini session
            // every tick.
            if age_ms > RECENT_WINDOW_MS && cwd_to_pids.is_empty() { continue; }
            entries.push((p, mtime, md.len()));
        }
    }
    entries.sort_by_key(|x| std::cmp::Reverse(x.1));
    // Next unpaired pid per cwd, in newest-first order.
    let mut next_pid_idx: HashMap<String, usize> = HashMap::new();

    for (p, mtime, size) in entries {
        let age_ms = now_ms.saturating_sub(mtime);
        let info = analyse_cached(&p, mtime, size);
        let cwd = info.cwd.clone().unwrap_or_default();
        let proj_short = project_basename(&cwd);
        let live_pid = if !cwd.is_empty() {
            cwd_to_pids.get(&cwd).and_then(|v| {
                let i = next_pid_idx.entry(cwd.clone()).or_insert(0);
                let pid = v.get(*i).map(|(pid, _)| *pid);
                if pid.is_some() { *i += 1; }
                pid
            })
        } else { None };
        let status = classify(live_pid.is_some(), age_ms);
        let id = p.file_stem().map(|s| sanitize_control(&s.to_string_lossy())).unwrap_or_default();
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
            tokens_total: info.tokens_input.saturating_add(info.tokens_output),
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
                    project: cwd, project_short: proj_short,
                    task: sanitize_control(t), mtime_ms: mtime, status,
                });
            }
        }
        sessions.push(sess);
    }

    sessions.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
    recent_tasks.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
    let waiting   = sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = sessions.iter().filter(|s| matches!(s.status, Status::Active|Status::Busy|Status::Spawning|Status::Idle)).count() as u32;
    let busy      = sessions.iter().filter(|s| matches!(s.status, Status::Busy|Status::Spawning)).count() as u32;
    SessionsResult {
        sessions: Sessions { sessions, recent_tasks, active, busy, waiting, completed },
        by_pid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // metadata.cwd is untrusted file content rendered in the project
    // column — bidi/format controls must be stripped at parse.
    #[test]
    fn metadata_cwd_is_sanitized() {
        let out = analyse("{\"metadata\":{\"cwd\":\"/p/\u{202e}x\"},\"messages\":[]}");
        assert!(!out.cwd.unwrap().contains('\u{202e}'));
    }

    #[test]
    fn parse_cache_skips_reread_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("agtop_gem_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.json");
        std::fs::write(&f, "{\"metadata\":{\"cwd\":\"/proj\"},\"messages\":[]}").unwrap();
        let first = analyse_cached(&f, 111, 42);
        assert_eq!(first.cwd.as_deref(), Some("/proj"));
        // Delete the file: an unchanged (mtime, size) key must be served
        // from cache without touching the filesystem.
        std::fs::remove_file(&f).unwrap();
        let second = analyse_cached(&f, 111, 42);
        assert_eq!(second.cwd.as_deref(), Some("/proj"));
        // A changed key forces a re-read (file gone → default output).
        let third = analyse_cached(&f, 222, 42);
        assert!(third.cwd.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
