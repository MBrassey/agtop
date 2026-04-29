"use strict";

// Stitches /proc parsing + Claude Code session reading + agent classification
// into a single snapshot. Holds a small amount of state across snapshots so
// it can compute CPU% deltas (top-style) and a sparkline of agent counts.

const path = require("path");
const proc = require("./proc.js");
const { buildMatchers, classify } = require("./agents.js");
const claudeSessions = require("./claude-sessions.js");

class Collector {
  constructor({ extraMatchers = [], history = 60 } = {}) {
    this.matchers = buildMatchers(extraMatchers);
    this.history = history;
    this.prev = new Map();           // pid -> { utime, stime, total }
    this.prevTotal = 0;
    this.numCpus = require("os").cpus().length || 1;
    this.bootTime = proc.readBootTime();
    this.history_total = [];         // [{ t, count }]
    this.history_active = [];
    this.history_cpu = [];
    this.history_mem = [];
    this.activity = [];              // recent agent lifecycle events
    this.knownPids = new Map();      // pid -> label, for spawn/exit events
  }

  snapshot() {
    const now = Date.now();
    if (!proc.isLinux()) {
      return this._emptySnapshot(now, "Live process metrics require Linux /proc — running in Claude-sessions-only mode.");
    }
    const totalCpu = proc.readSystemCpuTotal();
    const totalDelta = Math.max(1, totalCpu - this.prevTotal);
    const mem = proc.readMemInfo();
    const pids = proc.listPids();
    const agents = [];
    const liveSet = new Set();
    let aggCpu = 0;
    let aggMem = 0;

    const fs = require("fs");
    for (const pid of pids) {
      let statRaw;
      try { statRaw = fs.readFileSync(`/proc/${pid}/stat`, "utf8"); }
      catch { continue; }
      const st = proc.parseStat(statRaw);
      if (!st) continue;
      const cmdline = proc.readCmdline(pid);
      if (!cmdline) continue;
      const label = classify(cmdline, this.matchers);
      if (!label) continue;

      const cwd = proc.readCwd(pid);
      const exe = proc.readExe(pid);
      const status = proc.readStatus(pid);
      const io = proc.readIo(pid);
      const writing = proc.readWritingFiles(pid, 4);
      const procTotal = st.utime + st.stime;
      const prev = this.prev.get(pid);
      let cpuPct = 0;
      if (prev) {
        const procDelta = procTotal - prev.total;
        cpuPct = Math.max(0, (procDelta / totalDelta) * this.numCpus * 100);
      }
      this.prev.set(pid, { utime: st.utime, stime: st.stime, total: procTotal });
      const rssBytes = st.rss_pages * proc.PAGE_SIZE;
      const startedAtSec = this.bootTime + (st.starttime / proc.CLK_TCK);
      const uptimeSec = Math.max(0, Math.floor(Date.now() / 1000 - startedAtSec));

      agents.push({
        pid,
        label,
        comm: st.comm,
        state: st.state,
        ppid: st.ppid,
        cwd: cwd || "?",
        exe: exe || "?",
        cmdline,
        cmdshort: shortenCmd(cmdline, 80),
        threads: st.num_threads,
        rss: rssBytes,
        vsize: st.vsize,
        cpu: cpuPct,
        uptimeSec,
        readBytes: io ? (io.read_bytes || 0) : 0,
        writeBytes: io ? (io.write_bytes || 0) : 0,
        writingFiles: writing,
        writingDirs: dedupe(writing.map(f => path.dirname(f))),
        user: status ? (status.Uid || "").split(/\s+/)[0] : "",
      });
      liveSet.add({ pid, cwd: cwd || "" });
      aggCpu += cpuPct;
      aggMem += rssBytes;

      // Spawn detection.
      if (!this.knownPids.has(pid)) {
        this.knownPids.set(pid, label);
        this.activity.push({ t: now, kind: "spawn", label, pid, cwd: cwd || "" });
      }
    }

    // Exit detection: any pid we knew about that disappeared.
    const livePidNums = new Set(agents.map(a => a.pid));
    for (const [pid, label] of this.knownPids) {
      if (!livePidNums.has(pid)) {
        this.activity.push({ t: now, kind: "exit", label, pid });
        this.knownPids.delete(pid);
        this.prev.delete(pid);
      }
    }
    if (this.activity.length > 200) this.activity.splice(0, this.activity.length - 200);

    this.prevTotal = totalCpu;

    // Claude Code sessions (waiting/completed counts).
    const sessions = claudeSessions.summariseSessions({ livePids: liveSet, now });

    // Push history samples.
    pushBounded(this.history_total, agents.length, this.history);
    pushBounded(this.history_active, agents.length + sessions.active, this.history);
    pushBounded(this.history_cpu, Math.round(aggCpu * 10) / 10, this.history);
    pushBounded(this.history_mem, Math.round((aggMem / (1024 * 1024)) * 10) / 10, this.history);

    return {
      now,
      platform: "linux",
      sysCpus: this.numCpus,
      memTotal: mem.total,
      memAvailable: mem.available,
      agents,
      sessions,
      aggregates: {
        cpu: aggCpu,
        memBytes: aggMem,
        active: agents.length,
        waiting: sessions.waiting,
        completed: sessions.completed,
      },
      history: {
        total: this.history_total.slice(),
        active: this.history_active.slice(),
        cpu: this.history_cpu.slice(),
        mem: this.history_mem.slice(),
      },
      activity: this.activity.slice(-50).reverse(),
    };
  }

  _emptySnapshot(now, note) {
    const sessions = claudeSessions.summariseSessions({ livePids: new Set(), now });
    return {
      now,
      platform: require("os").platform(),
      note,
      sysCpus: this.numCpus,
      memTotal: 0,
      memAvailable: 0,
      agents: [],
      sessions,
      aggregates: { cpu: 0, memBytes: 0, active: 0, waiting: sessions.waiting, completed: sessions.completed },
      history: { total: [], active: [], cpu: [], mem: [] },
      activity: [],
    };
  }
}

function pushBounded(arr, v, max) {
  arr.push(v);
  if (arr.length > max) arr.shift();
}

function dedupe(arr) {
  const s = new Set();
  const out = [];
  for (const x of arr) {
    if (!x || s.has(x)) continue;
    s.add(x);
    out.push(x);
  }
  return out;
}

function shortenCmd(cmd, n) {
  if (cmd.length <= n) return cmd;
  return cmd.slice(0, n - 1) + "…";
}

module.exports = { Collector };
