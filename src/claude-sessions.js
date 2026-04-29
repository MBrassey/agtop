"use strict";

// Read Claude Code session metadata under ~/.claude/projects/. Each project
// directory contains JSONL transcript files (one JSON object per line). We
// read the tail to surface: status (active/busy/waiting/completed/idle), the
// most recent task subject and tool the model is using, and how many Task
// tool calls are in-flight (== running subagents).
//
// Best-effort across schema versions — if files are missing, malformed, or
// the schema drifts, we degrade silently rather than throwing.

const fs = require("fs");
const path = require("path");
const os = require("os");

const HOME = os.homedir();
const ROOT = path.join(HOME, ".claude", "projects");
const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000;
const BUSY_WINDOW_MS = 5 * 1000;            // file written in last 5s = busy
const ACTIVE_WINDOW_MS = 60 * 1000;         // file written in last 60s = active
const TAIL_BYTES = 256 * 1024;              // read the last 256KB for in-flight Task scan

function safeStat(p) { try { return fs.statSync(p); } catch { return null; } }
function safeReaddir(p) { try { return fs.readdirSync(p); } catch { return []; } }

function readTail(file, bytes = TAIL_BYTES) {
  let fd;
  try { fd = fs.openSync(file, "r"); } catch { return ""; }
  try {
    const st = fs.fstatSync(fd);
    const size = st.size;
    if (size === 0) return "";
    const len = Math.min(size, bytes);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, size - len);
    return buf.toString("utf8");
  } finally {
    try { fs.closeSync(fd); } catch {}
  }
}

function parseLines(text) {
  const out = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    try { out.push(JSON.parse(line)); } catch { /* tolerate truncation/garbage */ }
  }
  return out;
}

// Walk JSONL records and produce a summary of what the session is doing.
// Returns:
//   { stopReason, lastTask, lastTool, currentTool, inFlightTasks, lastTs }
function analyseRecords(records) {
  const taskUses = new Map();   // tool_use_id -> { subject, subagent_type, ts }
  const completedTaskIds = new Set();
  let lastTask = null;
  let lastTool = null;
  let currentTool = null;
  let stopReason = null;
  let lastTs = 0;

  // Walk forward so we naturally see Task starts before their results.
  for (const r of records) {
    if (!r || typeof r !== "object") continue;
    const ts = r.timestamp ? Date.parse(r.timestamp) : 0;
    if (ts) lastTs = Math.max(lastTs, ts);
    stopReason = stopReason || r.stop_reason || (r.message && r.message.stop_reason) || null;

    const content = (r.message && r.message.content) || r.content;
    if (Array.isArray(content)) {
      for (const c of content) {
        if (!c || typeof c !== "object") continue;
        if (c.type === "tool_use") {
          lastTool = c.name || lastTool;
          currentTool = c.name || currentTool;
          if (c.name === "Task" || c.name === "Agent") {
            const subj = (c.input && (c.input.subject || c.input.description)) || null;
            const sub = (c.input && c.input.subagent_type) || "agent";
            taskUses.set(c.id, { subject: subj, subagent_type: sub, ts });
            if (subj) lastTask = subj;
          } else if (c.name === "TodoWrite" && c.input && Array.isArray(c.input.todos)) {
            const inProg = c.input.todos.find(t => t.status === "in_progress");
            if (inProg && inProg.content) lastTask = inProg.content;
          } else if (c.input && typeof c.input.subject === "string") {
            lastTask = c.input.subject;
          }
        } else if (c.type === "tool_result") {
          if (c.tool_use_id) completedTaskIds.add(c.tool_use_id);
          // tool results imply the prior tool finished — clear currentTool
          currentTool = null;
        } else if (c.type === "text" && typeof c.text === "string" && r.type === "assistant") {
          // Assistant prose is also a useful "what is it doing" signal.
          const t = c.text.replace(/\s+/g, " ").trim();
          if (t) lastTask = t.slice(0, 100);
        }
      }
    }
    // Older single-message format with string content.
    if (r.toolUseResult && r.toolUseResult.subject) lastTask = r.toolUseResult.subject;
  }

  let inFlightTasks = 0;
  for (const [id] of taskUses) {
    if (!completedTaskIds.has(id)) inFlightTasks++;
  }

  return { stopReason, lastTask, lastTool, currentTool, inFlightTasks, lastTs };
}

function decodeProjectDir(name) {
  if (!name) return name;
  if (name.startsWith("-")) return "/" + name.slice(1).replace(/-/g, "/");
  return name;
}

function projectName(p) {
  if (!p) return "?";
  const parts = p.replace(/\/+$/, "").split("/").filter(Boolean);
  return parts[parts.length - 1] || p;
}

function classifyStatus({ isLive, ageMs, stopReason, hasInFlight }) {
  if (isLive && ageMs < BUSY_WINDOW_MS)        return "busy";       // bright green
  if (isLive && hasInFlight)                   return "spawning";   // bright cyan (subagents running)
  if (isLive && ageMs < ACTIVE_WINDOW_MS)      return "active";     // green
  if (isLive)                                  return "idle";       // gray (process up but quiet)
  if (stopReason === "end_turn" || stopReason === "stop_sequence") return "completed"; // magenta
  if (ageMs < RECENT_WINDOW_MS)                return "waiting";    // yellow
  return "stale";
}

// Top-level summary used by the UI's sessions panel.
function summariseSessions({ liveAgents = [], now = Date.now() } = {}) {
  const root = ROOT;
  if (!safeStat(root)) {
    return { sessions: [], byPid: new Map(), waiting: 0, completed: 0, active: 0, busy: 0, recentTasks: [] };
  }
  const sessions = [];
  const recentTasks = [];
  // Map cwd -> live claude pid (so we can tag each session).
  const cwdToPid = new Map();
  for (const a of liveAgents) {
    if (a.label === "claude" || a.label === "claude-code") {
      cwdToPid.set(a.cwd, a.pid);
    }
  }

  for (const proj of safeReaddir(root)) {
    const projDir = path.join(root, proj);
    const projStat = safeStat(projDir);
    if (!projStat || !projStat.isDirectory()) continue;
    const decodedPath = decodeProjectDir(proj);
    const projShort = projectName(decodedPath);

    // Find the most recently modified JSONL in this project — that's "the" current session.
    let mostRecent = null;
    const jsonls = [];
    for (const entry of safeReaddir(projDir)) {
      if (!entry.endsWith(".jsonl")) continue;
      const file = path.join(projDir, entry);
      const st = safeStat(file);
      if (!st) continue;
      jsonls.push({ entry, file, st });
      if (!mostRecent || st.mtimeMs > mostRecent.st.mtimeMs) mostRecent = { entry, file, st };
    }

    for (const { entry, file, st } of jsonls) {
      const ageMs = now - st.mtimeMs;
      const sessionId = entry.replace(/\.jsonl$/, "");
      const isMostRecent = mostRecent && mostRecent.file === file;
      const livePid = isMostRecent ? cwdToPid.get(decodedPath) : undefined;
      // Only do the expensive tail+parse for sessions that matter (live or recent).
      let info = { stopReason: null, lastTask: null, lastTool: null, currentTool: null, inFlightTasks: 0, lastTs: 0 };
      if (livePid || ageMs < RECENT_WINDOW_MS) {
        info = analyseRecords(parseLines(readTail(file)));
      }
      const status = classifyStatus({
        isLive: !!livePid,
        ageMs,
        stopReason: info.stopReason,
        hasInFlight: info.inFlightTasks > 0,
      });

      const session = {
        id: sessionId,
        project: decodedPath,
        projectShort: projShort,
        file,
        sizeBytes: st.size,
        mtimeMs: st.mtimeMs,
        ageMs,
        status,
        stopReason: info.stopReason,
        lastTask: info.lastTask,
        lastTool: info.lastTool,
        currentTool: info.currentTool,
        inFlightTasks: info.inFlightTasks,
        livePid: livePid || null,
        isMostRecent,
      };
      sessions.push(session);

      if (info.lastTask && ageMs < RECENT_WINDOW_MS) {
        recentTasks.push({
          project: decodedPath,
          projectShort: projShort,
          task: String(info.lastTask).replace(/\s+/g, " ").slice(0, 120),
          mtimeMs: st.mtimeMs,
          status,
        });
      }
    }
  }
  sessions.sort((a, b) => b.mtimeMs - a.mtimeMs);
  recentTasks.sort((a, b) => b.mtimeMs - a.mtimeMs);

  // Build a fast pid -> session lookup so the agents table can attach session info.
  const byPid = new Map();
  for (const s of sessions) {
    if (s.livePid && !byPid.has(s.livePid)) byPid.set(s.livePid, s);
  }

  return {
    sessions,
    byPid,
    waiting:   sessions.filter(s => s.status === "waiting").length,
    completed: sessions.filter(s => s.status === "completed").length,
    active:    sessions.filter(s => ["active", "busy", "spawning", "idle"].includes(s.status)).length,
    busy:      sessions.filter(s => s.status === "busy" || s.status === "spawning").length,
    recentTasks: recentTasks.slice(0, 20),
  };
}

module.exports = { summariseSessions, classifyStatus, ROOT };
