"use strict";

// Human-friendly formatters used both by the TUI and the --once / --json paths.

function bytes(n) {
  if (!n || n < 0) return "0B";
  const units = ["B", "K", "M", "G", "T"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return (n >= 100 ? n.toFixed(0) : n.toFixed(1)) + units[i];
}

function pct(n) {
  if (!n) return "0.0%";
  return n.toFixed(1) + "%";
}

function dur(sec) {
  if (sec < 60) return sec + "s";
  if (sec < 3600) return Math.floor(sec / 60) + "m" + (sec % 60).toString().padStart(2, "0") + "s";
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  if (h < 24) return h + "h" + m.toString().padStart(2, "0") + "m";
  const d = Math.floor(h / 24);
  return d + "d" + (h % 24) + "h";
}

function tildeify(p) {
  const home = require("os").homedir();
  if (p && p.startsWith(home)) return "~" + p.slice(home.length);
  return p || "";
}

function shorten(s, n) {
  if (!s) return "";
  if (s.length <= n) return s;
  return "…" + s.slice(s.length - n + 1);
}

function pad(s, n) {
  s = s == null ? "" : String(s);
  return s.length >= n ? s.slice(0, n) : s + " ".repeat(n - s.length);
}

function lpad(s, n) {
  s = s == null ? "" : String(s);
  return s.length >= n ? s.slice(0, n) : " ".repeat(n - s.length) + s;
}

const STATUS_COLOR = {
  busy: "grn", spawning: "cyn", active: "grn", idle: "dim",
  waiting: "yel", completed: "mag", stale: "dim",
};
const STATUS_GLYPH = {
  busy: "● BUSY ", spawning: "◆ SPAWN", active: "● ACTV ", idle: "○ idle ",
  waiting: "◌ WAIT ", completed: "✓ DONE ", stale: "· stale",
};

function snapshotTable(snap, { color = true, max = 0 } = {}) {
  const c = color ? ansi : nocolor;
  const list = snap.agents.slice();
  const n = max > 0 ? Math.min(max, list.length) : list.length;

  const out = [];
  out.push(
    c.bold(
      pad("STATUS", 8) + " " +
      pad("AGENT", 12) + " " +
      lpad("PID", 7) + " " +
      lpad("CPU%", 6) + " " +
      lpad("MEM", 8) + " " +
      lpad("UP", 8) + " " +
      lpad("SUB", 4) + "  " +
      pad("PROJECT", 14) + "  " +
      "DOING"
    )
  );

  for (let i = 0; i < n; i++) {
    const a = list[i];
    const sc = STATUS_COLOR[a.status] || "dim";
    const sg = STATUS_GLYPH[a.status] || "·";
    const doing = describeAgentText(a);
    // Pad-then-color: ANSI codes have zero visible width but still count
    // toward String.length, so pad must happen on the raw text first.
    out.push(
      c[sc](pad(sg, 8)) + " " +
      pad(a.label, 12) + " " +
      lpad(String(a.pid), 7) + " " +
      lpad(pct(a.cpu), 6) + " " +
      lpad(bytes(a.rss), 8) + " " +
      lpad(dur(a.uptimeSec), 8) + " " +
      (a.subagents > 0 ? c.cyn(lpad("+" + a.subagents, 4)) : lpad("-", 4)) + "  " +
      pad((a.project || "?").slice(0, 14), 14) + "  " +
      shorten(doing, 80)
    );
  }
  return out.join("\n");
}

function describeAgentText(a) {
  if (a.currentTool) {
    return a.currentTool + (a.currentTask ? ": " + String(a.currentTask).slice(0, 60) : "");
  }
  if (a.currentTask) return String(a.currentTask).slice(0, 80);
  if (a.status === "idle" && a.sessionAgeMs != null) return `(idle ${dur(Math.floor(a.sessionAgeMs / 1000))})`;
  if (a.status === "waiting") return "(awaiting input)";
  if (a.status === "completed") return "(session ended)";
  return a.cmdshort || "";
}

function summaryLine(snap) {
  const a = snap.aggregates;
  return `agtop  active=${a.active}  busy=${a.busy}  subagents=${a.subagents}  waiting=${a.waiting}  completed=${a.completed}  projects=${a.projectCount}  cpu=${pct(a.cpu)}  mem=${bytes(a.memBytes)}`;
}

const ansi = {
  bold: s => `\x1b[1m${s}\x1b[22m`,
  dim:  s => `\x1b[2m${s}\x1b[22m`,
  red:  s => `\x1b[31m${s}\x1b[39m`,
  yel:  s => `\x1b[33m${s}\x1b[39m`,
  grn:  s => `\x1b[32m${s}\x1b[39m`,
  cyn:  s => `\x1b[36m${s}\x1b[39m`,
  mag:  s => `\x1b[35m${s}\x1b[39m`,
};
const nocolor = {
  bold: s => s, dim: s => s, red: s => s, yel: s => s, grn: s => s, cyn: s => s, mag: s => s,
};

module.exports = { bytes, pct, dur, tildeify, shorten, pad, lpad, snapshotTable, summaryLine, ansi, nocolor };
