// Reads ~/.claude/projects/*/<session>.jsonl best-effort to surface live agent
// status, current tool, in-flight Task subagents, and the last task subject.

use crate::format::{project_basename, sanitize_control};
use crate::model::{RecentTask, Session, Status};
use crate::sessions::{LiveAgentRef, SessionsResult};

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const RECENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
// 30s captures the typical mid-turn gap where Claude is waiting on a tool
// result (no JSONL writes for tens of seconds) but is still actively working.
pub const BUSY_WINDOW_MS: u64 = 30 * 1000;
pub const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;   // 5 minutes
// Byte budgets for transcript parsing.  Steady-state ticks parse only
// freshly appended bytes, but the first sight of a large transcript (or a
// rewritten file) needs a whole-file catch-up; these caps spread that
// catch-up across ticks so a multi-hundred-MB transcript can't stall the
// collector for seconds in a single tick.
const PARSE_BUDGET_PER_FILE: u64 = 8 * 1024 * 1024;
const PARSE_BUDGET_PER_TICK: u64 = 32 * 1024 * 1024;

/// Every `<home>/.claude/projects` that exists on disk — own home
/// plus any extras (WSL `/mnt/c/Users/*`, `AGTOP_EXTRA_HOMES`).
/// Used by `summarise` so the Linux WSL build picks up Windows-side
/// Claude sessions and vice versa.
pub fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::paths::home_roots().into_iter()
        .map(|h| h.join(".claude").join("projects"))
        .filter(|p| p.exists())
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// One bounded read attempt: parse complete newline-terminated JSONL
/// records starting at `offset`, scanning at most `window` bytes.
/// Returns the records, the offset just past the last complete line
/// consumed, and the file length observed at open.  A trailing line with
/// no `\n` yet (still being written) is left for a later tick.  Mirrors
/// readfile.rs's hardening — O_NONBLOCK open plus a regular-file check on
/// the opened descriptor, so a `*.jsonl` FIFO can't wedge the synchronous
/// collector — because this streaming reader needs a seekable handle the
/// shared tail/head helpers don't expose.  UTF-8 is decoded lossily per
/// line.
fn read_lines_window(path: &Path, offset: u64, window: u64) -> Option<(Vec<Value>, u64, u64)> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
    #[cfg(unix)]
    let f = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let f = fs::File::open(path).ok()?;
    let md = f.metadata().ok()?;
    if !md.is_file() { return None; }
    let len = md.len();
    if offset >= len { return Some((Vec::new(), offset, len)); }
    let mut f = f;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let take = (len - offset).min(window).min(crate::readfile::HARD_CAP);
    let mut rdr = BufReader::with_capacity(64 * 1024, f.take(take));
    let mut records = Vec::new();
    let mut consumed = offset;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let n = match rdr.read_until(b'\n', &mut buf) { Ok(n) => n, Err(_) => break };
        if n == 0 { break; }
        if buf[n - 1] != b'\n' { break; }   // partial line at window edge / EOF
        consumed += n as u64;
        let line = String::from_utf8_lossy(&buf);
        let t = line.trim();
        if t.is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<Value>(t) { records.push(v); }
    }
    Some((records, consumed, len))
}

/// Budget-aware read: retries once at the hard cap when a single line is
/// larger than the per-tick window (huge inline tool results), and
/// force-skips a line that exceeds even the hard cap so the parser can
/// never livelock on it — the JSONL framing self-heals at the next
/// newline (the partial remainder fails to parse and is dropped).
fn read_records_from(path: &Path, offset: u64, budget: u64) -> (Vec<Value>, u64) {
    let budget = budget.clamp(1, crate::readfile::HARD_CAP);
    let Some((recs, new_off, len)) = read_lines_window(path, offset, budget) else {
        return (Vec::new(), offset);
    };
    if new_off > offset || offset.saturating_add(budget) >= len {
        return (recs, new_off);
    }
    let Some((recs, new_off, len)) = read_lines_window(path, offset, crate::readfile::HARD_CAP) else {
        return (Vec::new(), offset);
    };
    if new_off > offset || offset.saturating_add(crate::readfile::HARD_CAP) >= len {
        return (recs, new_off);
    }
    (Vec::new(), offset + crate::readfile::HARD_CAP)
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
    /// Token buckets keyed by model id.  Sessions routinely mix models
    /// (opus ↔ fable switches, subagent records on a different SKU), so
    /// cost must be summed per-model at each line's own rate — pricing
    /// the grand total at the last-seen model mis-bills every other
    /// model's share.
    model_tokens: HashMap<String, TokBuckets>,
    model: Option<String>,
}

/// Per-model share of the token buckets.  `input` includes cache_read +
/// cache_write, mirroring `AnalysisOut::tokens_input`.
#[derive(Default, Debug, Clone, Copy)]
struct TokBuckets {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

fn push_recent(buf: &mut Vec<String>, line: String) {
    // Cheap dedup: skip consecutive duplicates so spammy retries don't
    // overflow the preview window.
    if buf.last().map(|s| s == &line).unwrap_or(false) { return; }
    buf.push(line);
}

/// Streaming transcript accumulator.  Ingests JSONL records one at a
/// time so the incremental file cache can feed it only freshly appended
/// bytes; all cumulative fields (tokens, tool counts, per-model buckets,
/// first-record timestamp) therefore cover the whole transcript rather
/// than a fixed tail window.
#[derive(Default, Debug, Clone)]
struct ParseState {
    out: AnalysisOut,
    // Claude Code writes one JSONL line per assistant *content block*, and
    // every line belonging to the same assistant message repeats a
    // byte-identical `usage` object.  Summing per-line therefore
    // multi-counts a message's tokens (measured 3-7× on real transcripts).
    // Dedupe usage on `message.id` so each assistant message is counted
    // exactly once (the same key ccusage uses).
    usage_seen: HashSet<String>,
    /// tool_use ids (any tool) with no tool_result yet.
    pending_tools: HashSet<String>,
    /// Task / Agent tool_use ids with no tool_result yet, in transcript
    /// order.
    pending_tasks: Vec<String>,
    /// Descriptions for the pending Task/Agent subagents.
    task_descr: HashMap<String, String>,
}

impl ParseState {
    fn ingest(&mut self, r: &Value) {
        let out = &mut self.out;
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

        // Model — most recent *real* assistant message wins.  Skip Claude
        // Code's locally-generated placeholder records (`<synthetic>`) and
        // empty ids: pricing a whole session at `<synthetic>` yields $0
        // because it matches nothing in the price table.  Updated before
        // the usage block below so a record's tokens are attributed to its
        // own model.
        if let Some(m) = r.get("message").and_then(|m| m.get("model")).and_then(|v| v.as_str()) {
            if !m.is_empty() && m != "<synthetic>" {
                out.model = Some(m.to_string());
            }
        }

        // Token usage — Claude attaches a usage block to each assistant
        // message.  We track input / output / cache-read / cache-write
        // separately so the cost calc can apply Anthropic's distinct
        // rates: standard input @ 1×, cache-write @ 1.25×, cache-read
        // @ 0.1× (prompt caching).  `tokens_input` is the rolled-up
        // total displayed in the table.
        if let Some(msg) = r.get("message") {
            if let Some(usage) = msg.get("usage") {
                let it = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let ot = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cr = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cc = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                // Count this message's usage only the first time its id is
                // seen.  Records lacking a `message.id` (rare) are always
                // counted — under-counting a real message is worse than the
                // rare duplicate an id-less record could introduce.
                let first_time = match msg.get("id").and_then(|v| v.as_str()) {
                    Some(id) => self.usage_seen.insert(id.to_string()),
                    None => true,
                };
                if first_time {
                    out.tokens_input        = out.tokens_input.saturating_add(it.saturating_add(cr).saturating_add(cc));
                    out.tokens_output       = out.tokens_output.saturating_add(ot);
                    out.tokens_cache_read   = out.tokens_cache_read.saturating_add(cr);
                    out.tokens_cache_write  = out.tokens_cache_write.saturating_add(cc);
                    // Per-model share — the record's own model (synthetic /
                    // model-less records fall back to the last real one).
                    if let Some(model) = out.model.clone() {
                        let b = out.model_tokens.entry(model).or_default();
                        b.input       = b.input.saturating_add(it.saturating_add(cr).saturating_add(cc));
                        b.output      = b.output.saturating_add(ot);
                        b.cache_read  = b.cache_read.saturating_add(cr);
                        b.cache_write = b.cache_write.saturating_add(cc);
                    }
                }
                // The most recent usage block's input window IS the current
                // context fill (cumulative-prompt size on the next request).
                // Records iterate oldest → newest, so the last assignment
                // wins.  Duplicates carry the same value, so this is stable
                // whether or not the line was a counted duplicate.
                out.context_used = it.saturating_add(cr).saturating_add(cc);
            }
        }

        let content_holder = r.get("message").and_then(|m| m.get("content")).cloned()
            .or_else(|| r.get("content").cloned());

        // A fresh user prompt — a `user` record carrying no tool_result
        // and not one of Claude Code's `isMeta` bookkeeping lines — means
        // any still-unresolved tool_use was interrupted: a turn only
        // resumes via tool_results.  Without this, one orphaned tool_use
        // deep in a long transcript would pin the generic in-flight count
        // (and Busy status) forever, now that accumulation covers the
        // whole transcript instead of a tail window.  Task/Agent
        // subagents stay pending: background subagents legitimately keep
        // running across user turns.
        if r.get("type").and_then(|v| v.as_str()) == Some("user")
            && !r.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            let carries_result = content_holder.as_ref()
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().any(|c|
                    c.get("type").and_then(|v| v.as_str()) == Some("tool_result")))
                .unwrap_or(false);
            if !carries_result {
                self.pending_tools.clear();
            }
        }

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
                                self.pending_tools.insert(id.to_string());
                            }
                            if name == "Task" || name == "Agent" {
                                let id_str = c.get("id").and_then(|v| v.as_str()).map(String::from);
                                if let Some(id) = &id_str {
                                    self.pending_tasks.push(id.clone());
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
                                    self.task_descr.insert(id, descr);
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
                                self.pending_tools.remove(id);
                                self.pending_tasks.retain(|t| t != id);
                                self.task_descr.remove(id);
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
                        "text" if r.get("type").and_then(|v| v.as_str()) == Some("assistant") => {
                            if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                                let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                                if !trimmed.is_empty() {
                                    let snippet: String = trimmed.chars().take(120).collect();
                                    out.last_task = Some(snippet.clone());
                                    push_recent(&mut out.recent_activity, format!("› {}", snippet));
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

        // Keep the activity buffer to the most recent 12 events.
        if out.recent_activity.len() > 12 {
            let drop = out.recent_activity.len() - 12;
            out.recent_activity.drain(0..drop);
        }
    }

    /// Materialise the current analysis: cumulative fields cloned, the
    /// in-flight counts derived from the pending sets.
    fn snapshot(&self) -> AnalysisOut {
        let mut out = self.out.clone();
        out.in_flight_tools = self.pending_tools.len() as u32;
        out.in_flight_tasks = self.pending_tasks.len() as u32;
        out.in_flight_subagents = self.pending_tasks.iter()
            .filter_map(|id| self.task_descr.get(id).cloned())
            .collect();
        out
    }
}

/// Incremental per-transcript parse cache entry.  Holds the streaming
/// accumulator plus the (mtime, size, inode, offset) stamp of the last
/// parse, so an unchanged file costs no I/O at all and a grown file
/// costs only its appended bytes.
struct CachedFile {
    mtime_ms: u64,
    /// Size at the last fully-caught-up parse.  A budget-cut parse
    /// stores the parse offset here instead, so the next tick takes the
    /// growth path and resumes the catch-up.
    size: u64,
    ino: u64,
    /// Byte offset just past the last complete line ingested.
    offset: u64,
    state: ParseState,
}

/// Keyed on canonical session path.  `summarise` runs on the single
/// collector thread; the mutex is for safety, not contention.
fn session_cache() -> &'static Mutex<HashMap<PathBuf, CachedFile>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFile>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inode_of(md: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    { use std::os::unix::fs::MetadataExt; md.ino() }
    #[cfg(not(unix))]
    { let _ = md; 0 }
}

/// Cached analysis of one transcript.  Reuses the accumulated state on
/// an unchanged (mtime, size, inode) stamp, parses only appended bytes
/// on growth, and re-parses from scratch on shrink / same-size rewrite /
/// inode swap.  `tick_budget` bounds the bytes parsed across all files
/// in one tick; leftovers are picked up on later ticks via the stored
/// offset.
fn analyse_file_with_cache(
    cache: &mut HashMap<PathBuf, CachedFile>,
    path: &Path, mtime_ms: u64, size: u64, ino: u64,
    tick_budget: &mut u64,
) -> AnalysisOut {
    use std::collections::hash_map::Entry;
    let c = match cache.entry(path.to_path_buf()) {
        Entry::Occupied(e) => {
            let c = e.into_mut();
            if mtime_ms == c.mtime_ms && size == c.size && ino == c.ino {
                return c.state.snapshot();
            }
            let appended_only = ino == c.ino && size > c.size && size >= c.offset;
            if !appended_only {
                // Shrink, same-size rewrite, or inode swap — the bytes we
                // already ingested can no longer be trusted.
                c.state = ParseState::default();
                c.offset = 0;
            }
            c
        }
        // Sentinel stamp (mtime 0 / size 0) so a budget-starved first
        // tick can't be mistaken for "unchanged" on the next one.
        Entry::Vacant(v) => v.insert(CachedFile {
            mtime_ms: 0, size: 0, ino, offset: 0, state: ParseState::default(),
        }),
    };
    c.ino = ino;
    let want = size.saturating_sub(c.offset);
    let file_budget = PARSE_BUDGET_PER_FILE.min(*tick_budget);
    if want > 0 && file_budget > 0 {
        let window = want.min(file_budget);
        let (records, new_off) = read_records_from(path, c.offset, window);
        for r in &records {
            c.state.ingest(r);
        }
        let spent = new_off.saturating_sub(c.offset).max(window);
        *tick_budget = tick_budget.saturating_sub(spent);
        c.offset = new_off;
        c.mtime_ms = mtime_ms;
        c.size = if window >= want { size } else { c.offset };
    } else if want == 0 {
        c.mtime_ms = mtime_ms;
        c.size = size;
    }
    // want > 0 with no budget left: stamp stays stale so the next tick
    // retries the parse.
    c.state.snapshot()
}

static PRICE_TABLE: OnceLock<crate::pricing::PriceTable> = OnceLock::new();

/// Install the merged price table (builtin + user `--prices` overrides)
/// used for per-model session costing.  Call before the first
/// `summarise`; the builtin table is used when never called.
#[allow(dead_code)]
pub fn install_price_table(table: crate::pricing::PriceTable) {
    let _ = PRICE_TABLE.set(table);
}

fn price_table() -> &'static crate::pricing::PriceTable {
    PRICE_TABLE.get_or_init(crate::pricing::PriceTable::builtin)
}

/// Sum session cost per model at each model's own rate — see
/// `AnalysisOut::model_tokens`.
fn cost_from_model_tokens(
    table: &crate::pricing::PriceTable,
    model_tokens: &HashMap<String, TokBuckets>,
) -> f64 {
    model_tokens.iter()
        .map(|(m, b)| table.cost_with_cache(m, b.input, b.output, b.cache_read, b.cache_write))
        .sum()
}

/// Popup-ready top-8 tool counter.  Keys come straight from JSONL tool
/// names — sanitize like `last_tool`/`current_tool` so a crafted name
/// can't smuggle bidi overrides into the popup's tools line.
fn top_tool_counts(counts: &HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = counts.iter()
        .map(|(k, n)| (sanitize_control(k), *n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(8);
    v
}

/// Forward-encode a live-process cwd into the dir-name shape Claude
/// Code uses under `~/.claude/projects/`.  POSIX rule: every `/`
/// (including the leading one) becomes `-`; hyphens in path segments
/// stay as hyphens.  Windows: drive-letter prefix `C:\` becomes
/// `C--` and backslashes become `-`.  Used for matching live PIDs to
/// session JSONLs (decoding the other direction is ambiguous because
/// the encoding is lossy).
fn encode_cwd(cwd: &str) -> String {
    if cwd.is_empty() { return String::new(); }
    // Sysinfo on Windows hands back paths with a trailing `\` for many
    // processes (`C:\workspace\proj1\`); without trimming, the encoded
    // dir name picks up a stray trailing `-` and never matches Claude
    // Code's own encoding (`C--workspace-proj1`).  Preserve bare-root
    // paths (`/`, `C:\`) by keeping the original when the trim would
    // empty the string.
    let trimmed = cwd.trim_end_matches(&['/', '\\'][..]);
    let cwd = if trimmed.is_empty() { cwd } else { trimmed };
    let mut chars = cwd.chars();
    let first = chars.next().unwrap();
    // Windows drive letter — `C:\Users\u\proj` → `C--Users-u-proj`
    if first.is_ascii_alphabetic() {
        if let Some(':') = chars.clone().next() {
            let _ = chars.next();   // the ':'
            let rest = chars.collect::<String>();
            let body = rest.replace(['/', '\\'], "-");
            let body = body.strip_prefix('-').unwrap_or(&body);
            return format!("{}--{}", first, body);
        }
    }
    // POSIX
    cwd.replace('/', "-")
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
    // The directory name is attacker-choosable (any process can mkdir
    // under ~/.claude/projects); strip control/bidi characters before it
    // reaches the sessions pane.
    if bad { String::new() } else { sanitize_control(&decoded) }
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
    let roots = roots();
    let mut sessions: Vec<Session> = Vec::new();
    let mut recent_tasks: Vec<RecentTask> = Vec::new();
    let mut by_pid: HashMap<u32, Session> = HashMap::new();

    if roots.is_empty() {
        return SessionsResult::empty();
    }

    // Build the live-cwd → pids map keyed on the *forward-encoded*
    // cwd (slashes → hyphens).  Claude Code's project-dir encoding
    // is lossy — `/home/u/foo-bar` and `/home/u/foo/bar` both
    // produce `-home-u-foo-bar`, so reverse-decoding is ambiguous.
    // Encoding-forward is the only correct match.
    //
    // The map carries a Vec because multiple live `claude` PIDs
    // can share a cwd (parallel sessions in the same project from
    // different terminals).  Pre-2.4.4 agtop used a single-pid
    // HashMap which non-deterministically dropped all-but-one PID
    // depending on iteration order — the popup's "no Claude
    // session found" message was a race condition, not a real
    // missing session.  We sort each Vec by uptime ascending so the
    // newest-spawned pid pairs with the newest-touched JSONL below.
    let mut encoded_cwd_to_pids: HashMap<String, Vec<(u32, u64)>> = HashMap::new();
    for a in live_agents {
        if a.label == "claude" || a.label == "claude-code" {
            let enc = encode_cwd(a.cwd);
            if !enc.is_empty() {
                encoded_cwd_to_pids.entry(enc).or_default().push((a.pid, a.uptime_sec));
            }
        }
    }
    for v in encoded_cwd_to_pids.values_mut() {
        // Sort newest-pid-first (lowest uptime first).
        v.sort_by_key(|(_pid, uptime)| *uptime);
    }

    let mut seen_session_files: HashSet<PathBuf> = HashSet::new();
    // Incremental parse cache — held for the whole scan; evicted down to
    // this tick's working set at the end.
    let mut cache = session_cache().lock().unwrap_or_else(|p| p.into_inner());
    let mut tick_budget: u64 = PARSE_BUDGET_PER_TICK;
    for root in &roots {
    let read_dir = match fs::read_dir(root) {
        Ok(d) => d,
        Err(_) => continue,
    };

    for ent in read_dir.flatten() {
        let proj_dir = ent.path();
        if !proj_dir.is_dir() { continue; }
        let raw_name = ent.file_name().to_string_lossy().into_owned();
        let decoded_path = decode_project(&raw_name);
        let proj_short = project_basename(&decoded_path);

        // Find all jsonl files + the most recent one.
        let mut jsonls: Vec<(PathBuf, u64, u64, u64)> = Vec::new();
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
            jsonls.push((p, mtime, size, inode_of(&md)));
        }

        // Pair JSONL files to live PIDs for this project dir.  When
        // there are N pids and M jsonls in the same cwd, we line up
        // the freshest pid with the freshest-touched jsonl, and so
        // on.  Stronger than the old "only-most-recent jsonl gets
        // the only-pid" rule — supports parallel sessions correctly.
        let mut by_path_pid: HashMap<PathBuf, u32> = HashMap::new();
        if let Some(pids) = encoded_cwd_to_pids.get(&raw_name) {
            // Sort jsonls by mtime descending.
            let mut sorted_paths: Vec<&(PathBuf, u64, u64, u64)> = jsonls.iter().collect();
            sorted_paths.sort_by_key(|x| std::cmp::Reverse(x.1));
            for (i, (jp, _, _, _)) in sorted_paths.iter().enumerate() {
                if let Some((pid, _)) = pids.get(i) {
                    by_path_pid.insert(jp.clone(), *pid);
                } else {
                    break;
                }
            }
        }

        for (path, mtime, size, ino) in &jsonls {
            let age_ms = now_ms.saturating_sub(*mtime);
            let live_pid = by_path_pid.get(path).copied();
            // Historical transcripts outside the recent window with no
            // live process would only ever render as bare STAL rows with
            // no analysis — skip building them entirely.  Saves a
            // canonicalize syscall plus a Session allocation per stale
            // file per tick (thousands on long-lived installs), and keeps
            // every Snapshot from carting the full session history
            // through the channel.
            if live_pid.is_none() && age_ms >= RECENT_WINDOW_MS { continue; }
            // Cross-root dedupe: same session file accessed via two
            // mount paths (`/home/u/.claude/...` and
            // `/mnt/c/Users/u/.claude/...`) would otherwise appear
            // twice.  Canonicalise + insert; skip on duplicate.
            let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if !seen_session_files.insert(canon.clone()) { continue; }
            // The filename stem is attacker-writable (the monitored agent
            // owns the sessions dir) — sanitize before it reaches the
            // popup's session line.
            let id = path.file_stem()
                .map(|s| sanitize_control(&s.to_string_lossy()))
                .unwrap_or_default();
            let is_most_recent = most_recent_path.as_deref() == Some(path);

            let info = analyse_file_with_cache(
                &mut cache, &canon, *mtime, *size, *ino, &mut tick_budget,
            );

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
                cost_usd: cost_from_model_tokens(price_table(), &info.model_tokens),
                context_used: info.context_used,
                session_started_ms: info.session_started_ms,
                tool_counts: top_tool_counts(&info.tool_counts),
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
                        // Sanitize like Session.last_task — this parallel
                        // field otherwise reaches the renderer raw.
                        task: sanitize_control(&task).chars().take(120).collect(),
                        mtime_ms: *mtime,
                        status,
                    });
                }
            }

            sessions.push(sess);
        }
    }
    } // end outer roots loop

    // Evict cache entries for transcripts not analysed this tick —
    // deleted files, sessions aged past the recent window, unmounted
    // roots.  Bounds the cache to the current working set.
    cache.retain(|k, _| seen_session_files.contains(k));
    drop(cache);

    sessions.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
    recent_tasks.sort_by_key(|x| std::cmp::Reverse(x.mtime_ms));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One-shot analysis over an in-memory record slice — the test-side
    /// equivalent of what the incremental cache does per appended chunk.
    fn analyse(records: &[Value]) -> AnalysisOut {
        let mut st = ParseState::default();
        for r in records { st.ingest(r); }
        st.snapshot()
    }

    fn stat_of(path: &Path) -> (u64, u64) {
        let md = std::fs::metadata(path).unwrap();
        (md.len(), inode_of(&md))
    }

    #[test]
    fn encode_posix_cwd() {
        assert_eq!(encode_cwd("/home/u/code/proj"), "-home-u-code-proj");
        assert_eq!(encode_cwd("/home/u/code/proj/"), "-home-u-code-proj");
        assert_eq!(encode_cwd("/"), "-");
    }

    #[test]
    fn encode_windows_cwd() {
        assert_eq!(encode_cwd(r"C:\Users\u\proj"), "C--Users-u-proj");
        // Trailing backslash from Windows sysinfo must NOT leak into the
        // encoded dir name — Claude Code's own encoding produces no
        // trailing dash, and a mismatched key dropped agents off the
        // session-attached pipeline (jakeagtop.png repro).
        assert_eq!(encode_cwd(r"C:\workspace\proj1\"), "C--workspace-proj1");
        assert_eq!(encode_cwd(r"C:\workspace\proj1"), "C--workspace-proj1");
        // Mixed separators (some shells normalise to forward slash).
        assert_eq!(encode_cwd(r"C:/workspace/proj1"), "C--workspace-proj1");
        assert_eq!(encode_cwd(""), "");
    }

    #[test]
    fn usage_deduped_by_message_id() {
        // Claude Code emits one JSONL line per content block; each line for
        // the same message repeats identical usage.  Two lines, one id →
        // count once.
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "id": "msg_abc",
                "model": "claude-fable-5",
                "usage": {
                    "input_tokens": 100, "output_tokens": 50,
                    "cache_read_input_tokens": 10, "cache_creation_input_tokens": 5
                }
            }
        });
        let out = analyse(&[line.clone(), line.clone(), line]);
        assert_eq!(out.tokens_output, 50, "output must not be multi-counted");
        assert_eq!(out.tokens_input, 115, "input (incl. cache) must not be multi-counted");
        assert_eq!(out.tokens_cache_read, 10);
        assert_eq!(out.tokens_cache_write, 5);
        assert_eq!(out.model.as_deref(), Some("claude-fable-5"));
    }

    #[test]
    fn distinct_message_ids_accumulate() {
        let a = serde_json::json!({"type":"assistant","message":{"id":"m1","usage":{"output_tokens":10}}});
        let b = serde_json::json!({"type":"assistant","message":{"id":"m2","usage":{"output_tokens":7}}});
        assert_eq!(analyse(&[a, b]).tokens_output, 17);
    }

    #[test]
    fn synthetic_model_ignored_for_pricing() {
        // A session ending on a `<synthetic>` record must still be priced
        // at the last real model, not fall through to $0.
        let real = serde_json::json!({"type":"assistant","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":1}}});
        let synth = serde_json::json!({"type":"assistant","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":1}}});
        assert_eq!(analyse(&[real, synth]).model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn encode_idempotent_with_or_without_trailing_sep() {
        assert_eq!(
            encode_cwd(r"C:\workspace\proj1\"),
            encode_cwd(r"C:\workspace\proj1"));
        assert_eq!(
            encode_cwd("/home/u/code/proj/"),
            encode_cwd("/home/u/code/proj"));
    }

    #[test]
    fn mixed_model_sessions_cost_per_model() {
        // 1M input on fable-5 ($10/M) + 1M input on opus-4-8 ($5/M) must
        // bill $15 — last-seen-model pricing would say $10 (2M @ opus).
        let a = serde_json::json!({"type":"assistant","message":{"id":"m1","model":"claude-fable-5","usage":{"input_tokens":1_000_000u64,"output_tokens":0}}});
        let b = serde_json::json!({"type":"assistant","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":1_000_000u64,"output_tokens":0}}});
        let out = analyse(&[a, b]);
        assert_eq!(out.model.as_deref(), Some("claude-opus-4-8"), "display model stays last-seen");
        let t = crate::pricing::PriceTable::builtin();
        let c = cost_from_model_tokens(&t, &out.model_tokens);
        assert!((c - 15.0).abs() < 1e-6, "expected $15, got ${}", c);
    }

    #[test]
    fn synthetic_usage_attributed_to_last_real_model() {
        let real = serde_json::json!({"type":"assistant","message":{"id":"m1","model":"claude-fable-5","usage":{"input_tokens":100u64}}});
        let synth = serde_json::json!({"type":"assistant","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":50u64}}});
        let out = analyse(&[real, synth]);
        assert_eq!(out.model_tokens.len(), 1);
        assert_eq!(out.model_tokens["claude-fable-5"].input, 150);
    }

    #[test]
    fn tool_count_keys_sanitized() {
        // RTL-override in a tool name (the spoofing vector sanitize_control
        // defends against elsewhere) must not reach the popup's tools line.
        let rec = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"\u{202e}hsab","id":"t1"}]}});
        let out = analyse(&[rec]);
        let v = top_tool_counts(&out.tool_counts);
        assert_eq!(v, vec![("hsab".to_string(), 1)]);
    }

    #[test]
    fn fresh_user_prompt_clears_stale_pending_tools() {
        let bash_use = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"Bash","id":"t1"}]}});
        let task_use = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"Task","id":"t2","input":{"subject":"explore"}}]}});
        let prompt = serde_json::json!({"type":"user","message":{"content":"try something else"}});
        let out = analyse(&[bash_use.clone(), task_use, prompt.clone()]);
        // The orphaned Bash call is gone; the background Task survives.
        assert_eq!(out.in_flight_tools, 0);
        assert_eq!(out.in_flight_tasks, 1);
        assert_eq!(out.in_flight_subagents, vec!["agent: explore".to_string()]);
        // A tool_result-carrying user record must NOT clear pendings.
        let result = serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"other"}]}});
        let out = analyse(&[bash_use, result]);
        assert_eq!(out.in_flight_tools, 1);
    }

    // ---- incremental cache -------------------------------------------

    const L1: &str = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m1","model":"claude-fable-5","usage":{"input_tokens":100,"output_tokens":10}}}"#;
    const L2: &str = r#"{"type":"assistant","message":{"id":"m2","model":"claude-fable-5","usage":{"input_tokens":200,"output_tokens":20}}}"#;

    fn cache_tmpfile(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("agtop_claude_cache_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn cache_whole_file_then_incremental_append() {
        let f = cache_tmpfile("append.jsonl");
        std::fs::write(&f, format!("{L1}\n")).unwrap();
        let (size, ino) = stat_of(&f);
        let mut cache = HashMap::new();
        let mut budget = u64::MAX;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 100);
        assert_eq!(out.tokens_output, 10);
        assert_eq!(out.session_started_ms, 1_767_225_600_000);

        // Unchanged stamp → served from cache, no parse budget needed.
        let mut no_budget = 0u64;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut no_budget);
        assert_eq!(out.tokens_input, 100);

        // Append → only new bytes parsed; totals accumulate and the
        // first-record timestamp survives.
        use std::io::Write;
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{L2}").unwrap();
        drop(fh);
        let (size, ino) = stat_of(&f);
        let mut budget = u64::MAX;
        let out = analyse_file_with_cache(&mut cache, &f, 2000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 300);
        assert_eq!(out.tokens_output, 30);
        assert_eq!(out.session_started_ms, 1_767_225_600_000);

        // A duplicate message id appended later must not double-count —
        // the usage-dedup set persists across chunks.
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{L2}").unwrap();
        drop(fh);
        let (size, ino) = stat_of(&f);
        let out = analyse_file_with_cache(&mut cache, &f, 3000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 300);

        // Shrink/replace → full re-parse of the new content only.
        std::fs::write(&f, format!("{L2}\n")).unwrap();
        let (size, ino) = stat_of(&f);
        let out = analyse_file_with_cache(&mut cache, &f, 4000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 200);
        assert_eq!(out.session_started_ms, 0, "old first-record ts must not survive a replace");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn cache_waits_for_complete_lines() {
        let f = cache_tmpfile("partial.jsonl");
        // No trailing newline — the line is still being written.
        std::fs::write(&f, L1).unwrap();
        let (size, ino) = stat_of(&f);
        let mut cache = HashMap::new();
        let mut budget = u64::MAX;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 0);
        use std::io::Write;
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        fh.write_all(b"\n").unwrap();
        drop(fh);
        let (size, ino) = stat_of(&f);
        let out = analyse_file_with_cache(&mut cache, &f, 2000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 100);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn cache_budget_spreads_catchup_across_ticks() {
        let f = cache_tmpfile("budget.jsonl");
        std::fs::write(&f, format!("{L1}\n{L2}\n")).unwrap();
        let (size, ino) = stat_of(&f);
        let mut cache = HashMap::new();
        // First tick's budget only covers line 1.
        let mut budget = (L1.len() + 1) as u64;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 100);
        // Next tick finishes the catch-up despite an unchanged file.
        let mut budget = u64::MAX;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut budget);
        assert_eq!(out.tokens_input, 300);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn cache_resolves_in_flight_across_chunks() {
        let f = cache_tmpfile("inflight.jsonl");
        let use_rec = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","id":"t1"}]}}"#;
        std::fs::write(&f, format!("{use_rec}\n")).unwrap();
        let (size, ino) = stat_of(&f);
        let mut cache = HashMap::new();
        let mut budget = u64::MAX;
        let out = analyse_file_with_cache(&mut cache, &f, 1000, size, ino, &mut budget);
        assert_eq!(out.in_flight_tools, 1);
        // The matching tool_result lands in a later chunk.
        use std::io::Write;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#;
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{result}").unwrap();
        drop(fh);
        let (size, ino) = stat_of(&f);
        let out = analyse_file_with_cache(&mut cache, &f, 2000, size, ino, &mut budget);
        assert_eq!(out.in_flight_tools, 0);
        let _ = std::fs::remove_file(&f);
    }
}
