// oh-my-pi (`omp`) + pi coding-agent session enricher.
//
// Both agents store one JSONL transcript per session under a slugged
// project directory:
//
//   ~/.omp/agent/sessions/<slug>/<file>.jsonl   (oh-my-pi)
//   ~/.pi/agent/sessions/<slug>/<file>.jsonl    (pi)
//
// OMP is a fork of Pi, so the two share the exact same `version:3`
// transcript backbone.  OMP mostly just *added* fields (the
// `contextSnapshot` block, the subagent sibling dir); the few real
// divergences (e.g. `model_change` carries `model` in OMP vs `modelId`
// in Pi) are handled by probing both shapes.  One Value-probing reader
// therefore covers both; we tag each session with its vendor
// ("omp" / "pi") only so the right live PID gets paired.
//
// The shared `version:3` schema (camelCase keys, parsed defensively):
//   line 1   { type:"session", version:3, cwd, timestamp }   — cwd is authoritative
//   model_change                                             — fallback model only
//   { type:"message", message:{ role, ... } } branches on .message.role:
//     "assistant"  → { model (bare), stopReason, usage{input,output,
//                      cacheRead,cacheWrite,totalTokens,...}, content[],
//                      contextSnapshot{promptTokens}? (OMP only) }
//     "user"       → content[] text items hold the prompt
//     "toolResult" → { toolCallId, content[] }  (a ROLE, not a top type)
//   content[] items: text{text} (prose), thinking{thinking} (skip),
//   toolCall{ id, name, arguments(object) }.  A toolCall.id with no
//   matching toolResult.toolCallId is in-flight.
//
// Verified token identity: input + output + cacheRead + cacheWrite ==
// totalTokens, i.e. `input` is cache-EXCLUSIVE.  `reasoningTokens` is a
// subset of `output`, so we never add it.
//
// Why tail the JSONL and not `~/.omp/stats.db`: tailing keeps the
// existing `serde_json` dependency (no `rusqlite`), reads live data
// straight from the transcript the agent is writing right now, and
// matches every other vendor reader in this crate.  We report the bare
// model + token buckets; the collector prices cost uniformly from
// agtop's table.

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Sessions, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS: u64 = 30 * 1000;        // 30s — covers mid-turn tool waits
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;  // 5 minutes
const TAIL_BYTES: u64 = 256 * 1024;
const HEAD_BYTES: u64 = 4 * 1024; // the `session` header carrying cwd is line 1 (tiny)
// 64 MiB hard-cap on tail reads — defensive against pathological / symlinked
// session files.  All real call sites use <= 256 KiB.
const MAX_TAIL: u64 = 64 * 1024 * 1024;

/// One discovered transcript: `(path, mtime_ms, size_bytes)`.
type FileStat = (PathBuf, u64, u64);

/// Every existing `(<home>/.<vendor>/agent/sessions, vendor)` pair —
/// own home plus any extras (WSL `/mnt/c/Users/*`, `AGTOP_EXTRA_HOMES`),
/// for both omp and pi.  Deduped by path so a home that resolves the
/// same dir twice isn't walked twice.
fn roots() -> Vec<(PathBuf, &'static str)> {
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    for h in crate::paths::home_roots() {
        out.push((h.join(".omp").join("agent").join("sessions"), "omp"));
        out.push((h.join(".pi").join("agent").join("sessions"), "pi"));
    }
    out.retain(|(p, _)| p.exists());
    let mut seen = HashSet::new();
    out.retain(|(p, _)| seen.insert(p.clone()));
    out
}

fn read_tail(path: &Path, bytes: u64) -> String {
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

fn read_head(path: &Path, bytes: u64) -> String {
    let take = bytes.min(MAX_TAIL);
    let f = match File::open(path) { Ok(f) => f, Err(_) => return String::new() };
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

/// Substring after the last `/`, or the whole string if there's no `/`.
/// Used ONLY to bare-ify the provider-prefixed `model_change.model`
/// (e.g. "openai-codex/gpt-5.5" -> "gpt-5.5"); the assistant message's
/// own `model` field is already bare and is preferred.
fn strip_provider_prefix(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_string()
}

/// Pull the authoritative cwd from the `type:"session"` header line.
/// Slugged dir names are lossy, so we never decode them — we read the
/// real path the agent recorded at startup.
fn extract_cwd_from_header(head_text: &str) -> Option<String> {
    for r in parse_lines(head_text) {
        if r.get("type").and_then(|v| v.as_str()) == Some("session") {
            if let Some(cwd) = r.get("cwd").and_then(|v| v.as_str()) {
                if !cwd.is_empty() { return Some(cwd.to_string()); }
            }
        }
    }
    None
}

#[derive(Default, Debug, Clone)]
struct AnalysisOut {
    stop_reason: Option<String>,
    last_task: Option<String>,
    last_user_prompt: Option<String>,
    last_tool: Option<String>,
    current_tool: Option<String>,
    /// Tool calls with no matching toolResult — drives the busy decision.
    in_flight_tools: u32,
    /// Capped, prefix-tagged tail (`›` prose, `→` tool, `←` result) for
    /// the detail-popup live preview.
    recent_activity: Vec<String>,
    /// Cache-INCLUSIVE input total: sum of input + cacheRead + cacheWrite
    /// across the transcript (matches the claude reader's "tokens" column).
    tokens_input: u64,
    tokens_output: u64,
    tokens_cache_read: u64,
    tokens_cache_write: u64,
    /// Latest assistant turn's prompt size — current context fill.
    context_used: u64,
    /// First-record timestamp parsed from the JSONL.  Unix ms.
    session_started_ms: u64,
    /// Newest entry-level timestamp seen.  Unix ms.
    last_ts: u64,
    /// Tool-use counter — name -> call count.
    tool_counts: HashMap<String, u32>,
    model: Option<String>,
}

fn push_recent(buf: &mut Vec<String>, line: String) {
    if buf.last().map(|s| s == &line).unwrap_or(false) { return; }
    buf.push(line);
    if buf.len() > 12 { buf.remove(0); }
}

/// Collapse whitespace and clip to `n` chars — used for every preview
/// snippet so multi-line prose doesn't blow out the popup row.
fn snippet(s: &str, n: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(n).collect()
}

/// Join the text of a `content[]` array's `text`-typed items.
fn content_text(content: &Value) -> String {
    let mut out = String::new();
    if let Some(arr) = content.as_array() {
        for c in arr {
            if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() { out.push(' '); }
                    out.push_str(t);
                }
            }
        }
    }
    out
}

fn analyse(records: &[Value]) -> AnalysisOut {
    let mut out = AnalysisOut::default();
    let mut model_change_fallback: Option<String> = None;
    let mut all_tool_ids: Vec<String> = Vec::new();
    let mut completed: HashSet<String> = HashSet::new();

    for r in records {
        // Track wall-clock from the ENTRY-level `.timestamp` (an ISO
        // string in both vendors).  The inner `message.timestamp` is
        // numeric in Pi, so never use that one.
        if let Some(ts) = r.get("timestamp").and_then(|v| v.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                let ms = dt.timestamp_millis().max(0) as u64;
                if out.session_started_ms == 0 { out.session_started_ms = ms; }
                if ms > out.last_ts { out.last_ts = ms; }
            }
        }

        let kind = r.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "model_change" => {
                // OMP carries a provider-prefixed `model`; Pi carries a
                // bare `modelId`.  Last one wins; used only if no
                // assistant message reports a model.
                if let Some(m) = r.get("model").and_then(|v| v.as_str()) {
                    model_change_fallback = Some(strip_provider_prefix(m));
                } else if let Some(m) = r.get("modelId").and_then(|v| v.as_str()) {
                    model_change_fallback = Some(m.to_string());
                }
            }
            "message" => {
                let m = match r.get("message") { Some(m) => m, None => continue };
                let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
                match role {
                    "assistant" => {
                        // Bare model on the assistant turn is authoritative —
                        // most recent wins.
                        if let Some(model) = m.get("model").and_then(|v| v.as_str()) {
                            out.model = Some(model.to_string());
                        }
                        if let Some(sr) = m.get("stopReason").and_then(|v| v.as_str()) {
                            out.stop_reason = Some(sr.to_string());
                        }
                        // Token usage.  `input` is cache-EXCLUSIVE (verified:
                        // input+output+cacheRead+cacheWrite == totalTokens), so
                        // the rolled-up "input" column sums all three input-side
                        // buckets while the cache buckets are tracked separately
                        // for pricing.
                        if let Some(u) = m.get("usage") {
                            let it = u.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
                            let ot = u.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
                            let cr = u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0);
                            let cw = u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0);
                            out.tokens_input        = out.tokens_input.saturating_add(it.saturating_add(cr).saturating_add(cw));
                            out.tokens_output       = out.tokens_output.saturating_add(ot);
                            out.tokens_cache_read   = out.tokens_cache_read.saturating_add(cr);
                            out.tokens_cache_write  = out.tokens_cache_write.saturating_add(cw);
                            // Prefer OMP's explicit promptTokens; else the
                            // input-window sum.  Last assignment wins (records
                            // iterate oldest -> newest).
                            out.context_used = m.get("contextSnapshot")
                                .and_then(|c| c.get("promptTokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or_else(|| it.saturating_add(cr).saturating_add(cw));
                        }
                        if let Some(arr) = m.get("content").and_then(|v| v.as_array()) {
                            for c in arr {
                                match c.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                    "text" => {
                                        if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                                            let s = snippet(t, 120);
                                            if !s.is_empty() {
                                                out.last_task = Some(s.clone());
                                                push_recent(&mut out.recent_activity, format!("› {}", s));
                                            }
                                        }
                                    }
                                    "thinking" => { /* skip — internal reasoning */ }
                                    "toolCall" => {
                                        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                        if name.is_empty() { continue; }
                                        out.last_tool = Some(name.to_string());
                                        out.current_tool = Some(name.to_string());
                                        *out.tool_counts.entry(name.to_string()).or_insert(0) += 1;
                                        if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                                            all_tool_ids.push(id.to_string());
                                        }
                                        // Arg hint: probe the common string fields in
                                        // the `arguments` object; fall back to a clipped
                                        // compact dump of the whole object.
                                        let args = c.get("arguments");
                                        let hint = args.and_then(|a|
                                            a.get("command").and_then(|v| v.as_str())
                                                .or_else(|| a.get("file_path").and_then(|v| v.as_str()))
                                                .or_else(|| a.get("path").and_then(|v| v.as_str()))
                                                .or_else(|| a.get("query").and_then(|v| v.as_str()))
                                                .or_else(|| a.get("pattern").and_then(|v| v.as_str()))
                                                .or_else(|| a.get("subject").and_then(|v| v.as_str())))
                                            .map(|s| snippet(s, 120))
                                            .or_else(|| args.map(|a| {
                                                let dump = serde_json::to_string(a).unwrap_or_default();
                                                dump.chars().take(120).collect::<String>()
                                            }))
                                            .unwrap_or_default();
                                        let line = if hint.is_empty() { format!("→ {}", name) }
                                                   else { format!("→ {}: {}", name, hint) };
                                        push_recent(&mut out.recent_activity, line);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "user" => {
                        if let Some(content) = m.get("content") {
                            let s = snippet(&content_text(content), 120);
                            if !s.is_empty() {
                                out.last_user_prompt = Some(s.clone());
                                push_recent(&mut out.recent_activity, format!("› {}", s));
                            }
                        }
                    }
                    "toolResult" => {
                        if let Some(id) = m.get("toolCallId").and_then(|v| v.as_str()) {
                            completed.insert(id.to_string());
                        }
                        out.current_tool = None;
                        let preview = m.get("content")
                            .map(|c| snippet(&content_text(c), 120))
                            .unwrap_or_default();
                        let line = if preview.is_empty() { "← (ok)".to_string() }
                                   else { format!("← {}", preview) };
                        push_recent(&mut out.recent_activity, line);
                    }
                    _ => {}
                }
            }
            // thinking_level_change, mcp_tool_selection, custom_message,
            // compaction, session_info, session_init, ... — all ignored.
            _ => {}
        }
    }

    out.in_flight_tools = all_tool_ids.iter()
        .filter(|id| !completed.contains(*id)).count() as u32;
    // Prefer the assistant's prose subject; fall back to the last user prompt.
    if out.last_task.is_none() {
        out.last_task = out.last_user_prompt.clone();
    }
    // Assistant model wins; only fall back to the stripped model_change.
    if out.model.is_none() {
        out.model = model_change_fallback;
    }
    out
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
    if stop_reason.as_deref() == Some("stop") { return Status::Completed; }
    if age_ms < RECENT_WINDOW_MS { return Status::Waiting; }
    Status::Stale
}

/// OMP-only: scan the sibling subagent directory for live role
/// transcripts.  The dir is the session path with the `.jsonl`
/// extension dropped (built from parent + file_stem so a stray `.` in a
/// future stem can't strip too much).  Each role `.jsonl` whose mtime
/// is recent contributes one entry (`<Role>` or `<Role>: <task>`, the
/// task read from its `type:"session_init"` line).  Pi has no such dir,
/// so this returns empty without special-casing.
fn scan_subagents(session_path: &Path, now_ms: u64) -> Vec<String> {
    let dir = match (session_path.parent(), session_path.file_stem()) {
        (Some(parent), Some(stem)) => parent.join(stem),
        _ => return Vec::new(),
    };
    if !dir.is_dir() { return Vec::new(); }
    let rd = match fs::read_dir(&dir) { Ok(d) => d, Err(_) => return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for ent in rd.flatten() {
        let p = ent.path();
        // Only role transcripts: skip the `local/` subdir, `.md`, `.log`.
        if !p.is_file() { continue; }
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") { continue; }
        let md = match fs::metadata(&p) { Ok(m) => m, Err(_) => continue };
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        if now_ms.saturating_sub(mtime) >= ACTIVE_WINDOW_MS { continue; }
        let role = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if role.is_empty() { continue; }
        // Pull the assignment from the role file's `session_init` entry.
        // It sits a few lines in (after the `session` header) and the task
        // text alone can run to ~100 KB, so a 4 KB head would truncate the
        // line and fail to parse it.  TAIL_BYTES (256 KB) captures the whole
        // `session_init` line in practice; the cap keeps a pathological role
        // file from being slurped in full.
        let task = parse_lines(&read_head(&p, TAIL_BYTES)).iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("session_init"))
            .and_then(|r| r.get("task").and_then(|v| v.as_str()).map(|t| snippet(t, 80)));
        out.push(match task {
            Some(t) if !t.is_empty() => format!("{}: {}", role, t),
            _ => role,
        });
    }
    out
}

pub fn summarise(live_agents: &[LiveAgentRef], now_ms: u64) -> SessionsResult {
    let roots = roots();
    if roots.is_empty() {
        return SessionsResult::empty();
    }

    // Per-vendor live-PID map keyed on (vendor, cwd).  An omp PID and a
    // pi PID in the same cwd must not collide, hence the vendor in the key.
    let mut cwd_to_pid: HashMap<(String, String), u32> = HashMap::new();
    for a in live_agents {
        if a.label == "omp" || a.label == "pi" {
            cwd_to_pid.insert((a.label.to_string(), a.cwd.to_string()), a.pid);
        }
    }

    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();

    // Collect the per-session transcript files: only the `.jsonl` files
    // directly inside each slug dir (NOT recursing — that skips the OMP
    // subagent sibling dir and its `local/` subdir).  Cross-root dedupe
    // via canonicalize so a session reached through two mounts
    // (`/home/u/...` and `/mnt/c/Users/u/...`) appears once.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut files: Vec<(PathBuf, &'static str)> = Vec::new();
    for (root, vendor) in &roots {
        let slug_dirs = match fs::read_dir(root) { Ok(d) => d, Err(_) => continue };
        for slug in slug_dirs.flatten() {
            let sd = slug.path();
            if !sd.is_dir() { continue; }
            let inner = match fs::read_dir(&sd) { Ok(d) => d, Err(_) => continue };
            for f in inner.flatten() {
                let p = f.path();
                if !p.is_file() { continue; }
                if p.extension().and_then(|s| s.to_str()) != Some("jsonl") { continue; }
                let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
                if seen.insert(canon) { files.push((p, vendor)); }
            }
        }
    }

    // Group by (vendor, cwd) read from each file's header.  Files whose
    // header we can't parse become orphans surfaced as Waiting.
    let mut by_group: HashMap<(String, String), Vec<FileStat>> = HashMap::new();
    let mut orphan: Vec<FileStat> = Vec::new();

    for (path, vendor) in files {
        let md = match fs::metadata(&path) { Ok(m) => m, Err(_) => continue };
        let mtime = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let size = md.len();
        let age_ms = now_ms.saturating_sub(mtime);
        // Cheap-only: skip sessions older than the recent window unless a
        // live process might own one.
        if age_ms > RECENT_WINDOW_MS && live_agents.is_empty() { continue; }
        let head = read_head(&path, HEAD_BYTES);
        match extract_cwd_from_header(&head) {
            Some(cwd) => by_group.entry((vendor.to_string(), cwd)).or_default().push((path, mtime, size)),
            None => orphan.push((path, mtime, size)),
        }
    }

    for ((vendor, cwd), mut files) in by_group {
        files.sort_by_key(|f| std::cmp::Reverse(f.1));   // newest first
        let live_pid = cwd_to_pid.get(&(vendor.clone(), cwd.clone())).copied();
        for (i, (path, mtime, size)) in files.iter().enumerate() {
            let is_most_recent = i == 0;
            let age_ms = now_ms.saturating_sub(*mtime);
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let is_live = is_most_recent && live_pid.is_some();

            // Expensive tail+parse only for the live session or any
            // recently-touched one.
            let info = if is_live || age_ms < RECENT_WINDOW_MS {
                analyse(&parse_lines(&read_tail(path, TAIL_BYTES)))
            } else {
                AnalysisOut::default()
            };

            // OMP subagents — scan only for the live most-recent session.
            let subagents = if is_live { scan_subagents(path, now_ms) } else { Vec::new() };
            let in_flight_tasks = subagents.len() as u32;

            let status = classify_status(
                is_live,
                age_ms,
                &info.stop_reason,
                in_flight_tasks > 0,
                info.in_flight_tools > 0,
            );

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
                stop_reason: info.stop_reason.clone(),
                last_task:    info.last_task.as_deref().map(sanitize_control),
                last_tool:    info.last_tool.as_deref().map(sanitize_control),
                current_tool: info.current_tool.as_deref().map(sanitize_control),
                in_flight_tasks,
                in_flight_subagents: subagents.iter().map(|s| sanitize_control(s)).collect(),
                recent_activity: info.recent_activity.iter().map(|s| sanitize_control(s)).collect(),
                live_pid: if is_most_recent { live_pid } else { None },
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

            if is_most_recent {
                if let Some(pid) = live_pid {
                    by_pid.entry(pid).or_insert_with(|| sess.clone());
                }
            }

            if let Some(t) = &info.last_task {
                if age_ms < RECENT_WINDOW_MS {
                    recent_tasks.push(RecentTask {
                        project: cwd.clone(),
                        project_short: proj_short.clone(),
                        task: snippet(t, 120),
                        mtime_ms: *mtime,
                        status,
                    });
                }
            }

            sessions.push(sess);
        }
    }

    // Headerless transcripts — still surface their mtime as Waiting.
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

    sessions.sort_by_key(|s| std::cmp::Reverse(s.mtime_ms));
    recent_tasks.sort_by_key(|t| std::cmp::Reverse(t.mtime_ms));
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

    #[test]
    fn strips_provider_prefix() {
        assert_eq!(strip_provider_prefix("openai-codex/gpt-5.5"), "gpt-5.5");
        assert_eq!(strip_provider_prefix("gpt-5.5"), "gpt-5.5");
        assert_eq!(strip_provider_prefix("a/b/c"), "c");
    }
}
