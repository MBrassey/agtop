// Aider session reader.  Aider keeps a per-project markdown chat history:
//
//   <cwd>/.aider.chat.history.md
//   <cwd>/.aider.input.history
//
// The format is plain markdown with `# 2026-04-29 12:34:56.789 UTC` headers
// and `> user prompt` blocks.  We extract:
//
//   * last user prompt  (last `>` block before the tail)
//   * last assistant prose (text between the prompt and the next header)
//   * mtime of the file → age → status
//
// Aider doesn't write structured token usage to disk; cost can still be
// computed if the user adds aider patterns + a per-model price.

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS:   u64 = 30 * 1000;
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;
const TAIL_BYTES:       u64 = 64 * 1024;
/// Hard-cap tail reads at 64 MiB (real call sites use 64 KiB).  Defensive
/// against pathological / symlinked aider history files.
const MAX_TAIL:         u64 = 64 * 1024 * 1024;

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

#[derive(Default)]
struct ParsedHistory {
    last_user: Option<String>,
    last_assistant: Option<String>,
    /// Per-turn activity: last few user prompts and assistant responses,
    /// oldest → newest.  Each line already prefixed with `›`.
    recent_activity: Vec<String>,
}

fn parse_md(text: &str) -> ParsedHistory {
    let mut out = ParsedHistory::default();
    let mut user_lines: Vec<&str> = Vec::new();
    let mut asst_lines: Vec<&str> = Vec::new();
    let mut in_user = false;
    let mut all_turns: Vec<(String, String)> = Vec::new();   // (user, assistant)
    let flush = |u: &mut Vec<&str>, a: &mut Vec<&str>, all: &mut Vec<(String, String)>| {
        let join = |v: &[&str]| v.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        let us = join(u); let asx = join(a);
        if !us.is_empty() || !asx.is_empty() { all.push((us, asx)); }
        u.clear(); a.clear();
    };
    for line in text.lines() {
        if line.starts_with("# ") {
            flush(&mut user_lines, &mut asst_lines, &mut all_turns);
            in_user = false;
        } else if let Some(rest) = line.strip_prefix("> ") {
            in_user = true;
            user_lines.push(rest);
        } else if in_user && line.is_empty() {
            in_user = false;
        } else if !in_user {
            asst_lines.push(line);
        }
    }
    flush(&mut user_lines, &mut asst_lines, &mut all_turns);

    if let Some((u, a)) = all_turns.last() {
        if !u.is_empty() { out.last_user = Some(u.chars().take(120).collect()); }
        if !a.is_empty() { out.last_assistant = Some(a.chars().take(120).collect()); }
    }
    // Take the last 4 turns (≤8 lines) for the preview tail.
    let take = all_turns.len().saturating_sub(4);
    for (u, a) in &all_turns[take..] {
        if !u.is_empty() {
            let s: String = u.chars().take(120).collect();
            out.recent_activity.push(format!("› {}", s));
        }
        if !a.is_empty() {
            let s: String = a.chars().take(120).collect();
            out.recent_activity.push(format!("› {}", s));
        }
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
    // Aider history lives in each project's cwd, so we walk the cwds of
    // live aider processes plus a small set of recently-seen ones.
    let mut cwds: Vec<(PathBuf, Option<u32>)> = Vec::new();
    for a in live_agents {
        if a.label == "aider" {
            cwds.push((PathBuf::from(a.cwd), Some(a.pid)));
        }
    }
    if cwds.is_empty() {
        return SessionsResult::empty();
    }

    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();

    for (cwd, pid) in cwds {
        let history = cwd.join(".aider.chat.history.md");
        let md = match fs::metadata(&history) { Ok(m) => m, Err(_) => continue };
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let age_ms = now_ms.saturating_sub(mtime);
        let parsed = parse_md(&read_tail(&history, TAIL_BYTES));
        let last_task = parsed.last_assistant.clone().or(parsed.last_user.clone());
        let status = classify(pid.is_some(), age_ms);
        let cwd_s = cwd.to_string_lossy().into_owned();
        let proj_short = project_basename(&cwd_s);
        let id = format!("aider-{}", pid.map(|p| p.to_string()).unwrap_or_else(|| proj_short.clone()));
        let sess = Session {
            id: id.clone(),
            project: cwd_s.clone(),
            project_short: proj_short.clone(),
            file: history.to_string_lossy().into_owned(),
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
            recent_activity: parsed.recent_activity.iter()
                .map(|s| sanitize_control(s)).collect(),
            live_pid: pid,
            is_most_recent: true,
            tokens_input: 0, tokens_output: 0, tokens_total: 0,
            cost_usd: 0.0,
            context_used: 0,
            model: None,
        };
        if let Some(p) = pid {
            by_pid.entry(p).or_insert_with(|| sess.clone());
        }
        if let Some(t) = &last_task {
            if age_ms < RECENT_WINDOW_MS {
                recent_tasks.push(RecentTask {
                    project: cwd_s, project_short: proj_short,
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
