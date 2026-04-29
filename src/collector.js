"use strict";

// Stitches /proc parsing + Claude Code session reading + agent classification
// into a single snapshot. Holds a small amount of state across snapshots so
// it can compute CPU% deltas (top-style) and a sparkline of agent counts,
// and so we can emit lifecycle events when agents spawn / exit.

const path = require("path");
const proc = require("./proc.js");
const { buildMatchers, classify } = require("./agents.js");
const claudeSessions = require("./claude-sessions.js");

// Status display priority — lower is shown earlier. Drives the stable sort
// so the table stops jumping around between samples.
const STATUS_RANK = {
  busy: 0, spawning: 1, active: 2, idle: 3, waiting: 4, completed: 5, stale: 6,
};

class Collector {
  constructor({ extraMatchers = [], history = 60 } = {}) {
    this.matchers = buildMatchers(extraMatchers);
    this.history = history;
    this.prev = new Map();
    this.prevTotal = 0;
    this.cpuSmooth = new Map();      // pid -> EWMA-smoothed cpu%
    this.numCpus = require("os").cpus().length || 1;
    this.bootTime = proc.readBootTime();
    this.history_total = [];
    this.history_active = [];
    this.history_busy = [];
    this.history_cpu = [];
    this.history_mem = [];
    this.activity = [];
    this.knownPids = new Map();
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
      // EWMA-smooth CPU% to stop the row order from jittering on every tick.
      const prevSmooth = this.cpuSmooth.get(pid);
      const smoothed = prevSmooth == null ? cpuPct : prevSmooth * 0.6 + cpuPct * 0.4;
      this.cpuSmooth.set(pid, smoothed);
      const cpuRaw = cpuPct;
      cpuPct = smoothed;
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
        cpuRaw,
        uptimeSec,
        readBytes: io ? (io.read_bytes || 0) : 0,
        writeBytes: io ? (io.write_bytes || 0) : 0,
        writingFiles: writing,
        writingDirs: dedupe(writing.map(f => path.dirname(f))),
        user: status ? (status.Uid || "").split(/\s+/)[0] : "",
        // populated below from claude session data:
        status: "active",
        statusReason: null,
        project: cwd ? cwd.split("/").filter(Boolean).pop() : "?",
        currentTool: null,
        currentTask: null,
        subagents: 0,
        sessionId: null,
        sessionAgeMs: null,
      });
      aggCpu += cpuPct;
      aggMem += rssBytes;

      if (!this.knownPids.has(pid)) {
        this.knownPids.set(pid, label);
        this.activity.push({ t: now, kind: "spawn", label, pid, cwd: cwd || "" });
      }
    }

    // Exit detection.
    const livePidNums = new Set(agents.map(a => a.pid));
    for (const [pid, label] of this.knownPids) {
      if (!livePidNums.has(pid)) {
        this.activity.push({ t: now, kind: "exit", label, pid });
        this.knownPids.delete(pid);
        this.prev.delete(pid);
      }
    }
    if (this.activity.length > 300) this.activity.splice(0, this.activity.length - 300);

    this.prevTotal = totalCpu;

    // Claude session data — give it the live agents so it can match cwd→pid
    // and only do expensive parsing for relevant sessions.
    const sessions = claudeSessions.summariseSessions({ liveAgents: agents, now });

    // Attach session info to each Claude agent row. We blend the JSONL-based
    // status with CPU% so an agent that's clearly running (e.g. mid-generation,
    // when the transcript hasn't flushed in 60s) doesn't get mis-tagged "idle".
    for (const a of agents) {
      if (a.label === "claude" || a.label === "claude-code") {
        const sess = sessions.byPid.get(a.pid);
        if (sess) {
          a.status = sess.status;
          a.currentTool = sess.currentTool;
          a.currentTask = sess.lastTask;
          a.subagents = sess.inFlightTasks || 0;
          a.sessionId = sess.id;
          a.sessionAgeMs = sess.ageMs;
        } else {
          a.status = "idle";
        }
        // CPU override — process state always wins over flush-lag.
        if (a.cpu >= 20) a.status = "busy";
        else if (a.cpu >= 3 && (a.status === "idle" || a.status === "stale")) a.status = "active";
      } else {
        a.status = a.cpu >= 20 ? "busy" : a.cpu >= 1 ? "active" : "idle";
      }
    }

    // Counts that match the agent rows on screen.
    const busyCount = agents.filter(a => a.status === "busy" || a.status === "spawning").length;

    // Stable sort — same input order across ticks unless real activity changes:
    //   1. status priority (busy > spawning > active > idle > ...)
    //   2. project name (alphabetical → same-project rows cluster)
    //   3. CPU% desc (busiest first within a project)
    //   4. RSS desc, then PID asc as deterministic tiebreakers
    agents.sort((a, b) =>
      (STATUS_RANK[a.status] ?? 9) - (STATUS_RANK[b.status] ?? 9) ||
      (a.project || "").localeCompare(b.project || "") ||
      b.cpu - a.cpu ||
      b.rss - a.rss ||
      a.pid - b.pid
    );

    // Per-project aggregates for the projects panel.
    const projectMap = new Map();
    for (const a of agents) {
      const p = a.project || "?";
      let row = projectMap.get(p);
      if (!row) {
        row = { project: p, agents: 0, cpu: 0, rss: 0, subagents: 0, statuses: {}, cwd: a.cwd };
        projectMap.set(p, row);
      }
      row.agents += 1;
      row.cpu += a.cpu;
      row.rss += a.rss;
      row.subagents += a.subagents || 0;
      row.statuses[a.status] = (row.statuses[a.status] || 0) + 1;
    }
    const projects = [...projectMap.values()].sort((a, b) =>
      (b.statuses.busy || 0) - (a.statuses.busy || 0) ||
      b.cpu - a.cpu ||
      a.project.localeCompare(b.project)
    );

    // Clean up smoothed CPU entries for processes that no longer exist.
    for (const pid of [...this.cpuSmooth.keys()]) {
      if (!livePidNums.has(pid)) this.cpuSmooth.delete(pid);
    }

    pushBounded(this.history_total,  agents.length, this.history);
    pushBounded(this.history_active, agents.length + sessions.waiting, this.history);
    pushBounded(this.history_busy,   busyCount,     this.history);
    pushBounded(this.history_cpu,    Math.round(aggCpu * 10) / 10, this.history);
    pushBounded(this.history_mem,    Math.round((aggMem / (1024 * 1024)) * 10) / 10, this.history);

    return {
      now,
      platform: "linux",
      sysCpus: this.numCpus,
      memTotal: mem.total,
      memAvailable: mem.available,
      agents,
      projects,
      sessions,
      aggregates: {
        cpu: aggCpu,
        memBytes: aggMem,
        active: agents.length,
        busy: busyCount,
        waiting: sessions.waiting,
        completed: sessions.completed,
        subagents: agents.reduce((n, a) => n + (a.subagents || 0), 0),
        projectCount: projects.length,
      },
      history: {
        total: this.history_total.slice(),
        active: this.history_active.slice(),
        busy: this.history_busy.slice(),
        cpu: this.history_cpu.slice(),
        mem: this.history_mem.slice(),
      },
      activity: this.activity.slice(-80).reverse(),
    };
  }

  _emptySnapshot(now, note) {
    const sessions = claudeSessions.summariseSessions({ liveAgents: [], now });
    return {
      now,
      platform: require("os").platform(),
      note,
      sysCpus: this.numCpus,
      memTotal: 0,
      memAvailable: 0,
      agents: [],
      projects: [],
      sessions,
      aggregates: { cpu: 0, memBytes: 0, active: 0, busy: 0, waiting: sessions.waiting, completed: sessions.completed, subagents: 0, projectCount: 0 },
      history: { total: [], active: [], busy: [], cpu: [], mem: [] },
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
