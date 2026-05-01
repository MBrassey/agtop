// Reads ~/.claude/projects/*/<session>.jsonl best-effort to surface live agent
// status, current tool, in-flight Task subagents, and the last task subject.

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
// 30s captures the typical mid-turn gap where Claude is waiting on a tool
// result (no JSONL writes for tens of seconds) but is still actively working.
pub const BUSY_WINDOW_MS: u64 = 30 * 1000;
pub const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;   // 5 minutes
pub const TAIL_BYTES: u64 = 256 * 1024;

pub fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("projects")
}

fn read_tail(path: &Path, bytes: u64) -> String {
    // Hard-cap below i64::MAX and below usize::MAX on 32-bit targets so the
    // subsequent casts can't wrap.  64 MiB is generous — real callers pass
    // ~256 KiB; this just keeps a malformed/symlinked file from triggering
    // a panic-via-cast on tiny architectures.
    const MAX_TAIL: u64 = 64 * 1024 * 1024;
    let mut f = match File::open(path) { Ok(f) => f, Err(_) => return String::new() };
    let len = match f.metadata() { Ok(m) => m.len(), Err(_) => return String::new() };
    if len == 0 { return String::new(); }
    let take = bytes.min(len).min(MAX_TAIL);
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
    /// Task / Agent subagent tool_uses without a matching tool_result.
    in_flight_tasks: u32,
    /// Human-readable descriptions for each in-flight Task subagent
    /// (`subagent_type: subject`).
    in_flight_subagents: Vec<String>,
    /// In-flight count for ANY tool (Bash, Edit, Read, Write, ...) — used by
    /// the busy-status decision so an agent mid-Bash also reads as busy.
    in_flight_tools: u32,
    /// Capped, prefix-tagged tail of session activity for the detail popup
    /// preview.  Each entry already starts with `› `, `→ `, or `← `.
    recent_activity: Vec<String>,
    /// Sum of input_tokens + cache_creation + cache_read across the
    /// whole transcript — used by the rough "tokens" column.  For
    /// accurate cost we track the three buckets separately below.
    tokens_input: u64,
    tokens_output: u64,
    /// Cumulative cache-read tokens (charged at ~10% of input rate
    /// under Anthropic's prompt-caching pricing).  Tracked separately
    /// so the cost calc doesn't bill cache hits at full input rate.
    tokens_cache_read: u64,
    /// Cumulative cache-creation tokens (charged at ~125% of input
    /// rate — the prompt-cache write surcharge).
    tokens_cache_write: u64,
    /// Latest assistant turn's input window size in tokens.  Computed
    /// as `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`
    /// of the *last* usage block in the transcript — represents the
    /// total prompt size on the next request, i.e. how full the
    /// model's context window is right now.  Drives the popup's
    /// "Context: X% used" indicator.
    context_used: u64,
    /// First-record timestamp parsed from the JSONL.  Unix ms.
    session_started_ms: u64,
    /// Tool-use counter — name → call count, summed across all
    /// `tool_use` records in the session.  Used to surface the
    /// "tools: Bash 47 · Edit 23 · …" line in the popup.
    tool_counts: HashMap<String, u32>,
    model: Option<String>,
}

fn push_recent(buf: &mut Vec<String>, line: String) {
    // Cheap dedup: skip consecutive duplicates so spammy retries don't
    // overflow the preview window.
    if buf.last().map(|s| s == &line).unwrap_or(false) { return; }
    buf.push(line);
}

fn analyse(records: &[Value]) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut task_use_ids: Vec<String> = Vec::new();
    let mut all_tool_use_ids: Vec<String> = Vec::new();
    let mut task_descr: HashMap<String, String> = HashMap::new();
    let mut completed: HashMap<String, ()> = HashMap::new();

    for r in records {
        // Capture the first parseable record timestamp as the
        // session's wall-clock start.  Useful when `claude --resume`
        // produces a process whose uptime != session age.
        if out.session_started_ms == 0 {
            if let Some(ts) = r.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                    out.session_started_ms = dt.timestamp_millis().max(0) as u64;
                }
            }
        }

        if let Some(sr) = r.get("stop_reason").and_then(|v| v.as_str()) {
            out.stop_reason = Some(sr.to_string());
        } else if let Some(sr) = r.get("message").and_then(|m| m.get("stop_reason")).and_then(|v| v.as_str()) {
            out.stop_reason = Some(sr.to_string());
        }

        // Token usage — Claude attaches a usage block to each assistant
        // message.  We track input / output / cache-read / cache-write
        // separately so the cost calc can apply Anthropic's distinct
        // rates: standard input @ 1×, cache-write @ 1.25×, cache-read
        // @ 0.1× (prompt caching).  `tokens_input` is the rolled-up
        // total displayed in the table.
        if let Some(usage) = r.get("message").and_then(|m| m.get("usage")) {
            let it = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let ot = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cr = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cc = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            out.tokens_input        = out.tokens_input.saturating_add(it.saturating_add(cr).saturating_add(cc));
            out.tokens_output       = out.tokens_output.saturating_add(ot);
            out.tokens_cache_read   = out.tokens_cache_read.saturating_add(cr);
            out.tokens_cache_write  = out.tokens_cache_write.saturating_add(cc);
            // The most recent usage block's input window IS the current
            // context fill (cumulative-prompt size on the next request).
            // Records iterate oldest → newest, so the last assignment
            // wins.
            out.context_used = it.saturating_add(cr).saturating_add(cc);
        }

        // Model — most recent assistant message wins.
        if let Some(m) = r.get("message").and_then(|m| m.get("model")).and_then(|v| v.as_str()) {
            out.model = Some(m.to_string());
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
                                *out.tool_counts.entry(name.to_string()).or_insert(0) += 1;
                                // Recent-activity preview entry.
                                let arg_hint = c.get("input").and_then(|i| {
                                    i.get("command").and_then(|v| v.as_str())
                                        .or_else(|| i.get("file_path").and_then(|v| v.as_str()))
                                        .or_else(|| i.get("subject").and_then(|v| v.as_str()))
                                        .or_else(|| i.get("description").and_then(|v| v.as_str()))
                                        .or_else(|| i.get("path").and_then(|v| v.as_str()))
                                }).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                                  .unwrap_or_default();
                                let hint: String = arg_hint.chars().take(120).collect();
                                let line = if hint.is_empty() {
                                    format!("→ {}", name)
                                } else {
                                    format!("→ {}: {}", name, hint)
                                };
                                push_recent(&mut out.recent_activity, line);
                            }
                            // Track every tool_use id so we can compute
                            // generic in-flight (Bash/Edit/Read/...) too.
                            if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                                all_tool_use_ids.push(id.to_string());
                            }
                            if name == "Task" || name == "Agent" {
                                let id_str = c.get("id").and_then(|v| v.as_str()).map(String::from);
                                if let Some(id) = &id_str {
                                    task_use_ids.push(id.clone());
                                }
                                let mut subj_opt = None::<String>;
                                let mut kind_opt = None::<String>;
                                if let Some(input) = c.get("input") {
                                    if let Some(s) = input.get("subject")
                                        .or_else(|| input.get("description"))
                                        .and_then(|v| v.as_str()) {
                                        out.last_task = Some(s.to_string());
                                        subj_opt = Some(s.to_string());
                                    }
                                    if let Some(k) = input.get("subagent_type").and_then(|v| v.as_str()) {
                                        kind_opt = Some(k.to_string());
                                    }
                                }
                                if let Some(id) = id_str {
                                    let kind = kind_opt.unwrap_or_else(|| "agent".into());
                                    let descr = match subj_opt {
                                        Some(s) => format!("{}: {}", kind, s),
                                        None => kind,
                                    };
                                    task_descr.insert(id, descr);
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
                            // Pull a short result preview when present.
                            let preview = c.get("content").and_then(|v| {
                                if let Some(s) = v.as_str() { return Some(s.to_string()); }
                                if let Some(arr) = v.as_array() {
                                    for x in arr {
                                        if let Some(s) = x.get("text").and_then(|t| t.as_str()) {
                                            return Some(s.to_string());
                                        }
                                    }
                                }
                                None
                            }).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                              .unwrap_or_default();
                            let hint: String = preview.chars().take(120).collect();
                            let line = if hint.is_empty() {
                                "← (ok)".to_string()
                            } else {
                                format!("← {}", hint)
                            };
                            push_recent(&mut out.recent_activity, line);
                        }
                        "text" => {
                            if r.get("type").and_then(|v| v.as_str()) == Some("assistant") {
                                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                                    let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                                    if !trimmed.is_empty() {
                                        let snippet: String = trimmed.chars().take(120).collect();
                                        out.last_task = Some(snippet.clone());
                                        push_recent(&mut out.recent_activity, format!("› {}", snippet));
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

    // Keep the activity buffer to the most recent 12 events.
    if out.recent_activity.len() > 12 {
        let drop = out.recent_activity.len() - 12;
        out.recent_activity.drain(0..drop);
    }
    out.in_flight_tasks = task_use_ids.iter()
        .filter(|id| !completed.contains_key(*id)).count() as u32;
    out.in_flight_subagents = task_use_ids.iter()
        .filter(|id| !completed.contains_key(*id))
        .filter_map(|id| task_descr.get(id).cloned())
        .collect();
    out.in_flight_tools = all_tool_use_ids.iter()
        .filter(|id| !completed.contains_key(*id)).count() as u32;
    out
}

/// Decode a Claude Code session-path-encoded project name back into the
/// original cwd.  Encoding rules differ by host OS:
///
///   POSIX:  `/home/u/code/proj` → `-home-u-code-proj`
///   Windows: `C:\Users\u\proj`  → `C--Users-u-proj`
///                                  ^─ bare drive letter, then `--`
///
/// Both have a single consistent separator (`-`) standing in for the
/// path separator, but Windows preserves the drive letter as a literal
/// prefix.  We detect the Windows shape via a leading `[A-Za-z]--` and
/// emit `C:\` (backslashes) so the project label is recognisable to
/// Windows users; otherwise we fall back to the POSIX rule.
///
/// Path-traversal hardening: refuse decoded paths that contain `..`
/// segments — a directory crafted as `-..--..--etc-passwd` would
/// otherwise surface `/../../etc/passwd` as the row label.
fn decode_project(name: &str) -> String {
    if name.is_empty() { return String::new(); }
    let decoded = if let Some(rest) = windows_drive_split(name) {
        let (drive, body) = rest;
        let mut s = String::with_capacity(name.len() + 2);
        s.push(drive);
        s.push(':');
        s.push('\\');
        s.push_str(&body.replace('-', "\\"));
        s
    } else if let Some(rest) = name.strip_prefix('-') {
        let mut s = String::with_capacity(rest.len() + 1);
        s.push('/');
        s.push_str(&rest.replace('-', "/"));
        s
    } else {
        name.to_string()
    };
    // Reject paths with `..` segments to prevent display-side traversal.
    let bad = decoded.split(['/', '\\']).any(|seg| seg == "..");
    if bad { String::new() } else { decoded }
}

/// Detect the Windows-encoded shape `<drive>--rest`, returning
/// `(drive, rest)` where rest is the path body with `-` separators.
fn windows_drive_split(name: &str) -> Option<(char, &str)> {
    let mut chars = name.chars();
    let drive = chars.next()?;
    if !drive.is_ascii_alphabetic() { return None; }
    let rest = chars.as_str();
    rest.strip_prefix("--").map(|body| (drive, body))
}

fn classify_status(
    is_live: bool, age_ms: u64,
    stop_reason: &Option<String>,
    has_in_flight_task: bool,
    has_in_flight_tool: bool,
) -> Status {
    if is_live && has_in_flight_task { return Status::Spawning; }
    if is_live && (age_ms < BUSY_WINDOW_MS || has_in_flight_tool) { return Status::Busy; }
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
                info.in_flight_tools > 0,
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
                last_task:    info.last_task.as_deref().map(sanitize_control),
                last_tool:    info.last_tool.as_deref().map(sanitize_control),
                current_tool: info.current_tool.as_deref().map(sanitize_control),
                in_flight_tasks: info.in_flight_tasks,
                in_flight_subagents: info.in_flight_subagents.iter()
                    .map(|s| crate::format::sanitize_control(s)).collect(),
                recent_activity: info.recent_activity.iter()
                    .map(|s| crate::format::sanitize_control(s)).collect(),
                live_pid,
                is_most_recent,
                tokens_input: info.tokens_input,
                tokens_output: info.tokens_output,
                tokens_total: info.tokens_input.saturating_add(info.tokens_output),
                tokens_cache_read:  info.tokens_cache_read,
                tokens_cache_write: info.tokens_cache_write,
                cost_usd: 0.0,
                context_used: info.context_used,
                session_started_ms: info.session_started_ms,
                tool_counts: {
                    let mut v: Vec<(String, u32)> = info.tool_counts.iter()
                        .map(|(k, v)| (k.clone(), *v)).collect();
                    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    v.truncate(8);
                    v
                },
                model: info.model.as_deref().map(sanitize_control),
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
