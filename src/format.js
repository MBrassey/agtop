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

function snapshotTable(snap, { color = true, max = 0 } = {}) {
  const c = color ? ansi : nocolor;
  const header = c.bold(
    pad("PID", 7) + pad("AGENT", 14) + lpad("CPU%", 6) + lpad("MEM", 8) +
    lpad("UPTIME", 9) + " " + pad("CWD", 32) + " TASK/CMD"
  );
  const rows = [header];
  const list = snap.agents.slice().sort((a, b) => b.cpu - a.cpu || b.rss - a.rss);
  const n = max > 0 ? Math.min(max, list.length) : list.length;
  for (let i = 0; i < n; i++) {
    const a = list[i];
    rows.push(
      lpad(a.pid, 6) + " " +
      pad(a.label, 14) +
      lpad(pct(a.cpu), 6) +
      lpad(bytes(a.rss), 8) +
      lpad(dur(a.uptimeSec), 9) + " " +
      pad(shorten(tildeify(a.cwd), 32), 32) + " " +
      shorten(a.cmdshort, 60)
    );
  }
  return rows.join("\n");
}

function summaryLine(snap) {
  const a = snap.aggregates;
  return `agtop  active=${a.active}  waiting=${a.waiting}  completed=${a.completed}  cpu=${pct(a.cpu)}  mem=${bytes(a.memBytes)}`;
}

const ansi = {
  bold: s => `\x1b[1m${s}\x1b[22m`,
  dim:  s => `\x1b[2m${s}\x1b[22m`,
  red:  s => `\x1b[31m${s}\x1b[39m`,
  yel:  s => `\x1b[33m${s}\x1b[39m`,
  grn:  s => `\x1b[32m${s}\x1b[39m`,
  cyn:  s => `\x1b[36m${s}\x1b[39m`,
};
const nocolor = {
  bold: s => s, dim: s => s, red: s => s, yel: s => s, grn: s => s, cyn: s => s,
};

module.exports = { bytes, pct, dur, tildeify, shorten, pad, lpad, snapshotTable, summaryLine, ansi, nocolor };
