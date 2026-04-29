"use strict";

// Read Claude Code session metadata under ~/.claude/projects/. Each project
// directory contains JSONL transcript files (one JSON object per line) and
// possibly a tasks/ directory for background tasks. We read just enough to
// summarise: which sessions look "waiting" (recent activity but no live PID),
// "completed" (terminal stop_reason), and what task names were last seen.
//
// Everything in here is best-effort: if files are missing, malformed, or the
// schema drifts, we degrade silently rather than throwing.

const fs = require("fs");
const path = require("path");
const os = require("os");

const HOME = os.homedir();
const ROOT = path.join(HOME, ".claude", "projects");
const RECENT_WINDOW_MS = 24 * 60 * 60 * 1000; // 24h

function safeStat(p) { try { return fs.statSync(p); } catch { return null; } }
function safeReaddir(p) { try { return fs.readdirSync(p); } catch { return []; } }

function readLastJsonLine(file, maxBytes = 65536) {
  let fd;
  try { fd = fs.openSync(file, "r"); } catch { return null; }
  try {
    const st = fs.fstatSync(fd);
    const size = st.size;
    if (size === 0) return null;
    const len = Math.min(size, maxBytes);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, size - len);
    const text = buf.toString("utf8");
    const lines = text.split("\n").filter(l => l.trim().length > 0);
    if (lines.length === 0) return null;
    // last line may be partial if we cut mid-record; try parsing from the end.
    for (let i = lines.length - 1; i >= 0; i--) {
      try { return JSON.parse(lines[i]); } catch { /* keep walking */ }
    }
    return null;
  } finally {
    try { fs.closeSync(fd); } catch {}
  }
}

function decodeProjectDir(name) {
  // ~/.claude/projects encodes the project path with - as separator.
  // Reverse-decode best-effort: replace leading - with /, leave the rest intact.
  if (!name) return name;
  if (name.startsWith("-")) return "/" + name.slice(1).replace(/-/g, "/");
  return name;
}

function summariseSessions({ livePids = new Set(), now = Date.now() } = {}) {
  const root = ROOT;
  if (!safeStat(root)) {
    return { sessions: [], waiting: 0, completed: 0, recentTasks: [] };
  }
  const sessions = [];
  const recentTasks = [];
  for (const proj of safeReaddir(root)) {
    const projDir = path.join(root, proj);
    const projStat = safeStat(projDir);
    if (!projStat || !projStat.isDirectory()) continue;
    const decodedPath = decodeProjectDir(proj);
    for (const entry of safeReaddir(projDir)) {
      if (!entry.endsWith(".jsonl")) continue;
      const file = path.join(projDir, entry);
      const st = safeStat(file);
      if (!st) continue;
      const ageMs = now - st.mtimeMs;
      const sessionId = entry.replace(/\.jsonl$/, "");
      const last = readLastJsonLine(file);
      let lastTask = null;
      let stopReason = null;
      if (last && typeof last === "object") {
        // Best-effort field probing across schema versions.
        stopReason = last.stop_reason || (last.message && last.message.stop_reason) || null;
        lastTask =
          (last.toolUseResult && last.toolUseResult.subject) ||
          (last.tool_use && last.tool_use.input && last.tool_use.input.subject) ||
          (last.message && typeof last.message.content === "string" && last.message.content.slice(0, 80)) ||
          null;
      }
      // Heuristic for "live" sessions: a process whose cwd matches the decoded
      // project path. Without that, fall back to mtime within recent window.
      const isLive = [...livePids].some(p => p.cwd === decodedPath);
      let status;
      if (isLive) status = "active";
      else if (stopReason === "end_turn" || stopReason === "stop_sequence") status = "completed";
      else if (ageMs < RECENT_WINDOW_MS) status = "waiting";
      else status = "idle";

      sessions.push({
        id: sessionId,
        project: decodedPath,
        file,
        sizeBytes: st.size,
        mtimeMs: st.mtimeMs,
        ageMs,
        status,
        stopReason,
        lastTask,
      });
      if (lastTask && ageMs < RECENT_WINDOW_MS) {
        recentTasks.push({ project: decodedPath, task: String(lastTask).replace(/\s+/g, " ").slice(0, 100), mtimeMs: st.mtimeMs });
      }
    }
  }
  sessions.sort((a, b) => b.mtimeMs - a.mtimeMs);
  recentTasks.sort((a, b) => b.mtimeMs - a.mtimeMs);
  return {
    sessions,
    waiting: sessions.filter(s => s.status === "waiting").length,
    completed: sessions.filter(s => s.status === "completed").length,
    active: sessions.filter(s => s.status === "active").length,
    recentTasks: recentTasks.slice(0, 20),
  };
}

module.exports = { summariseSessions, ROOT };
