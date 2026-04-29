"use strict";

// agtop TUI — sophisticated, project-centric layout.
//
//  ┌ agtop ── 11 active · 2 busy · 4 waiting · 1 done · cpu 32% · mem 3.9G ─┐
//  ├ Agents (grouped by project) ─────────────────────┬ CPU% ──────────────┤
//  │ ◆ agtop  1 agent · 17.3% · 0 subagents           │ legend + line      │
//  │   ● BUSY  claude  pid 384791  17.3%  17m  Edit…  ├ MEM (MB) ─────────┤
//  │ ◆ xsol   1 agent · 16.4% · 1 subagent            │ legend + line      │
//  │   ● BUSY  claude  pid 404872  16.4%  4d   Bash…  ├ Active vs Busy ────┤
//  │   └ 1 subagent: code-reviewer (running)          │ multi-series line  │
//  │ ◆ marinade …                                     │                    │
//  ├ Projects ──────┬ Recent activity ───────────────┴ Claude sessions ────┤
//  │ agtop  1  17%  │ 23:14:57  ● spawn  claude  ~/code/agtop              │
//  │ xsol   1  16%  │ 23:13:22  ◌ exit   codex   pid 98271                 │
//  └─────────────────────────────────────────────────────────────────────────┘
//
// Implementation notes:
//  * Agent list is a blessed.box with manually-formatted, tag-colored lines
//    (rather than contrib.table) so we can render project headers, sub-rows,
//    and per-status badges with full control.
//  * Selection tracks the agent's PID across refreshes so the cursor never
//    "jumps" when sort positions shift slightly between samples.
//  * Charts use blessed-contrib's line widget with showLegend, axis labels
//    and abbreviated tick formatting so they're actually legible.

const blessed = require("blessed");
const contrib = require("blessed-contrib");
const fmt = require("./format.js");

const STATUS_DECOR = {
  busy:      { glyph: "●", color: "{green-fg}{bold}",   label: "BUSY"  },
  spawning:  { glyph: "◆", color: "{cyan-fg}{bold}",    label: "SPAWN" },
  active:    { glyph: "●", color: "{green-fg}",         label: "ACTV"  },
  idle:      { glyph: "○", color: "{white-fg}",         label: "idle"  },
  waiting:   { glyph: "◌", color: "{yellow-fg}",        label: "WAIT"  },
  completed: { glyph: "✓", color: "{magenta-fg}",       label: "DONE"  },
  stale:     { glyph: "·", color: "{white-fg}",         label: "stale" },
};

const AGENT_COLOR = {
  "claude": "blue", "claude-code": "blue",
  "codex": "green", "openai-codex": "green",
  "aider": "red",
  "cursor-agent": "magenta",
  "gemini": "cyan",
  "goose": "yellow",
  "continue": "white",
  "opencode": "magenta",
  "copilot": "cyan",
  "cody": "magenta",
  "amp": "yellow",
  "crush": "red",
  "mods": "green",
  "sgpt": "blue",
  "llm": "cyan",
  "ollama": "yellow",
  "fabric": "white",
  "block-goose": "yellow",
};
const fallbackPalette = ["cyan", "magenta", "yellow", "red", "green", "blue"];
function colorFor(label) {
  if (AGENT_COLOR[label]) return AGENT_COLOR[label];
  let h = 0;
  for (let i = 0; i < (label || "").length; i++) h = (h * 31 + label.charCodeAt(i)) >>> 0;
  return fallbackPalette[h % fallbackPalette.length];
}

const SORTS = [
  { key: "smart",  label: "Smart"  }, // collector default — busy → project → cpu
  { key: "cpu",    label: "CPU",    fn: (a, b) => b.cpu - a.cpu },
  { key: "mem",    label: "MEM",    fn: (a, b) => b.rss - a.rss },
  { key: "uptime", label: "UPTIME", fn: (a, b) => b.uptimeSec - a.uptimeSec },
  { key: "agent",  label: "AGENT",  fn: (a, b) => a.label.localeCompare(b.label) },
];

function rank(s) {
  return ({ busy: 0, spawning: 1, active: 2, idle: 3, waiting: 4, completed: 5, stale: 6 })[s] ?? 9;
}

function run({ collector, intervalMs = 1500, version = "0.1.0", initialFilter = "" }) {
  const screen = blessed.screen({
    smartCSR: true,
    fullUnicode: true,
    title: "agtop — agent monitor",
    autoPadding: true,
  });

  const grid = new contrib.grid({ rows: 12, cols: 12, screen });

  const header = grid.set(0, 0, 1, 12, blessed.box, {
    tags: true,
    style: { fg: "white" },
    border: { type: "line", fg: "cyan" },
    label: " agtop ",
  });

  const agentList = grid.set(1, 0, 8, 8, blessed.box, {
    label: " Agents (grouped by project) ",
    tags: true,
    keys: false,
    mouse: false,
    scrollable: true,
    alwaysScroll: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
  });

  const cpuChart = grid.set(1, 8, 3, 4, contrib.line, {
    label: " CPU% (sum) ",
    showLegend: false,
    style: { line: "yellow", text: "white", baseline: "white" },
    minY: 0,
    border: { type: "line", fg: "cyan" },
    numYLabels: 4,
  });

  const memChart = grid.set(4, 8, 3, 4, contrib.line, {
    label: " MEM (MB) ",
    showLegend: false,
    style: { line: "magenta", text: "white", baseline: "white" },
    minY: 0,
    border: { type: "line", fg: "cyan" },
    numYLabels: 4,
    abbreviate: true,
  });

  const countChart = grid.set(7, 8, 2, 4, contrib.line, {
    label: " Agents over time ",
    showLegend: true,
    legend: { width: 12 },
    style: { text: "white", baseline: "white" },
    minY: 0,
    border: { type: "line", fg: "cyan" },
    numYLabels: 3,
    wholeNumbersOnly: true,
  });

  const projectsBox = grid.set(9, 0, 3, 4, blessed.box, {
    label: " Projects ",
    tags: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
    scrollable: true,
  });

  const activityBox = grid.set(9, 4, 3, 4, blessed.box, {
    label: " Recent activity ",
    tags: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
    scrollable: true,
  });

  const sessionsBox = grid.set(9, 8, 3, 4, blessed.box, {
    label: " Claude sessions ",
    tags: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
  });

  const help = blessed.box({
    parent: screen,
    top: "center",
    left: "center",
    width: 70,
    height: 22,
    border: { type: "line", fg: "cyan" },
    label: " Help ",
    tags: true,
    hidden: true,
    style: { bg: "black", fg: "white" },
    content:
      "{bold}agtop{/bold}  v" + version + " — agent monitor\n\n" +
      "  q, Ctrl-C   quit\n" +
      "  ?, h        toggle this help\n" +
      "  p           pause / resume refresh\n" +
      "  r           refresh now\n" +
      "  s           cycle sort (smart / cpu / mem / uptime / agent)\n" +
      "  f           filter agents by substring (Esc to clear)\n" +
      "  g           toggle project grouping\n" +
      "  j/k, ↓/↑    move selection\n\n" +
      "  Status legend:\n" +
      "    {green-fg}● BUSY{/}   process active and writing in last 5s\n" +
      "    {cyan-fg}◆ SPAWN{/}  Task subagents currently in flight\n" +
      "    {green-fg}● ACTV{/}   process running recently\n" +
      "    {white-fg}○ idle{/}   process up but quiet for >60s\n" +
      "    {yellow-fg}◌ WAIT{/}  no live process, recent session activity\n" +
      "    {magenta-fg}✓ DONE{/} session ended (stop_reason)\n",
  });

  const filterPrompt = blessed.textbox({
    parent: screen,
    bottom: 0,
    left: 0,
    height: 1,
    width: "100%",
    inputOnFocus: true,
    hidden: true,
    style: { fg: "white", bg: "blue" },
  });

  const state = {
    paused: false,
    sortIdx: 0,
    grouped: true,
    filter: initialFilter,
    selectedPid: null,
    rows: [],
    lastSnap: null,
    timer: null,
  };

  function setHeader(snap) {
    const a = snap.aggregates;
    const sortLabel = SORTS[state.sortIdx].label;
    const flt = state.filter ? `  filter:{yellow-fg}${escapeTags(state.filter)}{/yellow-fg}` : "";
    const paused = state.paused ? "  {red-bg}{black-fg} PAUSED {/}" : "";
    const grp = state.grouped ? "  group:{cyan-fg}on{/cyan-fg}" : "  group:off";
    header.setContent(
      ` {bold}agtop{/bold} {gray-fg}v${version}{/gray-fg}` +
      `   active:{green-fg}${a.active}{/green-fg}` +
      `   busy:{green-fg}{bold}${a.busy}{/}` +
      `   subagents:{cyan-fg}${a.subagents}{/cyan-fg}` +
      `   waiting:{yellow-fg}${a.waiting}{/yellow-fg}` +
      `   completed:{magenta-fg}${a.completed}{/magenta-fg}` +
      `   projects:{white-fg}${a.projectCount}{/white-fg}` +
      `   cpu:{yellow-fg}${fmt.pct(a.cpu)}{/yellow-fg}` +
      `   mem:{magenta-fg}${fmt.bytes(a.memBytes)}{/magenta-fg}` +
      `   sort:${sortLabel}${grp}${flt}${paused}`
    );
  }

  function visibleAgents(snap) {
    let rows = snap.agents.slice();
    if (state.sortIdx !== 0 && SORTS[state.sortIdx].fn) rows.sort(SORTS[state.sortIdx].fn);
    if (state.filter) {
      const f = state.filter.toLowerCase();
      rows = rows.filter(a =>
        (a.label && a.label.toLowerCase().includes(f)) ||
        (a.cmdline && a.cmdline.toLowerCase().includes(f)) ||
        (a.cwd && a.cwd.toLowerCase().includes(f)) ||
        (a.project && a.project.toLowerCase().includes(f)) ||
        String(a.pid) === f
      );
    }
    return rows;
  }

  function renderAgents(snap) {
    const list = visibleAgents(snap);
    const lines = [];
    const rows = [];
    if (list.length === 0) {
      agentList.setContent("\n  {gray-fg}(no agents detected — try `agtop --list-builtins` or set $AGTOP_MATCH){/gray-fg}");
      state.rows = rows;
      return;
    }

    if (state.grouped) {
      const byProj = new Map();
      for (const a of list) {
        const p = a.project || "?";
        if (!byProj.has(p)) byProj.set(p, []);
        byProj.get(p).push(a);
      }
      const projOrder = [...byProj.keys()].sort((p1, p2) => {
        const s1 = byProj.get(p1)[0].status;
        const s2 = byProj.get(p2)[0].status;
        return rank(s1) - rank(s2) || p1.localeCompare(p2);
      });
      for (const p of projOrder) {
        const ags = byProj.get(p);
        const totalCpu = ags.reduce((n, a) => n + a.cpu, 0);
        const totalSub = ags.reduce((n, a) => n + (a.subagents || 0), 0);
        const totalMem = ags.reduce((n, a) => n + a.rss, 0);
        const headerColor = "cyan";
        lines.push(
          ` {${headerColor}-fg}{bold}◆ ${escapeTags(p)}{/}` +
          ` {gray-fg}— ${ags.length} agent${ags.length === 1 ? "" : "s"}` +
          ` · ${fmt.pct(totalCpu)} cpu` +
          ` · ${fmt.bytes(totalMem)} mem` +
          (totalSub ? ` · {cyan-fg}${totalSub} subagent${totalSub === 1 ? "" : "s"}{/cyan-fg}{gray-fg}` : "") +
          `{/gray-fg}`
        );
        rows.push({ kind: "header" });
        for (const a of ags) {
          lines.push(formatAgentRow(a));
          rows.push({ kind: "agent", agent: a });
        }
        lines.push("");
        rows.push({ kind: "blank" });
      }
    } else {
      for (const a of list) {
        lines.push(formatAgentRow(a));
        rows.push({ kind: "agent", agent: a });
      }
    }

    agentList.setContent(lines.join("\n"));
    state.rows = rows;

    if (state.selectedPid != null) {
      const found = rows.find(r => r.kind === "agent" && r.agent.pid === state.selectedPid);
      if (!found) {
        const first = rows.find(r => r.kind === "agent");
        state.selectedPid = first ? first.agent.pid : null;
      }
    } else {
      const first = rows.find(r => r.kind === "agent");
      if (first) state.selectedPid = first.agent.pid;
    }
  }

  function formatAgentRow(a) {
    const d = STATUS_DECOR[a.status] || STATUS_DECOR.stale;
    const labelCol = colorFor(a.label);
    const badge = `${d.color}${d.glyph} ${d.label.padEnd(5)}{/}`;
    const labelChip = `{${labelCol}-fg}{bold}${pad(a.label, 12)}{/}`;
    const pid = `{gray-fg}pid{/gray-fg}{white-fg}${lpad(a.pid, 7)}{/white-fg}`;
    const cpuStr = colorPct(a.cpu);
    const memStr = `{magenta-fg}${lpad(fmt.bytes(a.rss), 7)}{/magenta-fg}`;
    const upStr = `{gray-fg}${lpad(fmt.dur(a.uptimeSec), 7)}{/gray-fg}`;
    const sub = a.subagents > 0 ? `  {cyan-fg}{bold}+${a.subagents}{/}` : "    ";
    const doing = describeDoing(a);
    return `   ${badge}  ${labelChip} ${pid} ${cpuStr} ${memStr} ${upStr}${sub}  ${doing}`;
  }

  function describeDoing(a) {
    if (a.currentTool) {
      const tool = `{cyan-fg}${escapeTags(a.currentTool)}{/cyan-fg}`;
      const subj = a.currentTask
        ? `: ${escapeTags(String(a.currentTask).slice(0, 60))}`
        : "";
      return `${tool}${subj}`;
    }
    if (a.currentTask) {
      return `{white-fg}${escapeTags(String(a.currentTask).slice(0, 70))}{/white-fg}`;
    }
    if (a.status === "idle" && a.sessionAgeMs != null) {
      return `{gray-fg}(idle ${fmt.dur(Math.floor(a.sessionAgeMs / 1000))}){/gray-fg}`;
    }
    if (a.status === "waiting") return `{yellow-fg}(awaiting input){/yellow-fg}`;
    if (a.status === "completed") return `{magenta-fg}(session ended){/magenta-fg}`;
    return `{gray-fg}${escapeTags(a.cmdshort || "")}{/gray-fg}`;
  }

  function colorPct(v) {
    const s = lpad(fmt.pct(v), 6);
    if (v >= 50) return `{red-fg}{bold}${s}{/}`;
    if (v >= 10) return `{yellow-fg}{bold}${s}{/}`;
    if (v >= 1)  return `{green-fg}${s}{/green-fg}`;
    return `{gray-fg}${s}{/gray-fg}`;
  }

  function setCharts(snap) {
    const xs = snap.history.cpu.map((_, i) => String(i));
    cpuChart.setData([{ title: "CPU%", style: { line: "yellow" }, x: xs, y: snap.history.cpu }]);
    memChart.setData([{ title: "MB",   style: { line: "magenta" }, x: xs, y: snap.history.mem }]);
    countChart.setData([
      { title: "active", style: { line: "green" }, x: xs, y: snap.history.active },
      { title: "busy",   style: { line: "red"   }, x: xs, y: snap.history.busy   },
    ]);
  }

  function setProjects(snap) {
    const projs = snap.projects || [];
    if (projs.length === 0) {
      projectsBox.setContent("\n  {gray-fg}(no projects){/gray-fg}");
      return;
    }
    const lines = [""];
    for (const p of projs.slice(0, 10)) {
      const busyN  = p.statuses.busy || 0;
      const spawnN = p.statuses.spawning || 0;
      const activeBadge =
        busyN  ? `{green-fg}{bold}● ${busyN} busy{/}`
        : spawnN ? `{cyan-fg}◆ ${spawnN} spawning{/cyan-fg}`
        : (p.statuses.active ? `{green-fg}● ${p.statuses.active} active{/green-fg}`
        : `{white-fg}○ idle{/white-fg}`);
      lines.push(
        `  {bold}${escapeTags(pad(p.project, 18))}{/bold}` +
        ` ${lpad(String(p.agents), 2)}a` +
        ` ${colorPct(p.cpu)}` +
        (p.subagents ? ` {cyan-fg}+${p.subagents}{/cyan-fg}` : "   ") +
        `  ${activeBadge}`
      );
    }
    projectsBox.setContent(lines.join("\n"));
  }

  function setActivity(snap) {
    const events = (snap.activity || []).slice(0, 30);
    const lines = events.map(e => {
      const t = `{gray-fg}${new Date(e.t).toTimeString().slice(0, 8)}{/gray-fg}`;
      const cwd = e.cwd ? `  {gray-fg}${escapeTags(fmt.tildeify(e.cwd))}{/gray-fg}` : "";
      if (e.kind === "spawn") {
        return `${t}  {green-fg}{bold}● spawn{/}  {${colorFor(e.label)}-fg}${pad(e.label, 12)}{/}` +
               ` {gray-fg}pid{/gray-fg}${lpad(e.pid, 7)}${cwd}`;
      }
      return `${t}  {gray-fg}◌ exit {/gray-fg}  {${colorFor(e.label || "")}-fg}${pad(e.label || "", 12)}{/}` +
             ` {gray-fg}pid{/gray-fg}${lpad(e.pid, 7)}`;
    });
    activityBox.setContent(lines.join("\n") || "  {gray-fg}(no recent events){/gray-fg}");
  }

  function setSessions(snap) {
    const s = snap.sessions || { sessions: [], recentTasks: [] };
    const a = snap.aggregates;
    const lines = [];
    lines.push(
      `  {green-fg}{bold}${a.busy || 0}{/} busy   ` +
      `{green-fg}${Math.max(0, (s.active || 0) - (a.busy || 0))}{/green-fg} active   ` +
      `{yellow-fg}${s.waiting || 0}{/yellow-fg} waiting   ` +
      `{magenta-fg}${s.completed || 0}{/magenta-fg} done`
    );
    if (a.subagents) {
      lines.push(`  {cyan-fg}{bold}${a.subagents}{/} Task subagent${a.subagents === 1 ? "" : "s"} in flight`);
    }
    lines.push("");
    lines.push(`  {bold}Recent tasks{/bold}`);
    const tasks = (s.recentTasks || []).slice(0, 6);
    if (tasks.length === 0) lines.push("  {gray-fg}(none in last 24h){/gray-fg}");
    for (const t of tasks) {
      const proj = t.projectShort || (t.project || "").split("/").pop();
      const dec = STATUS_DECOR[t.status] || STATUS_DECOR.stale;
      lines.push(`  ${dec.color}${dec.glyph}{/} {bold}${escapeTags(proj)}{/bold}  {gray-fg}${escapeTags(t.task)}{/gray-fg}`);
    }
    sessionsBox.setContent(lines.join("\n"));
  }

  function tick() {
    const snap = collector.snapshot();
    state.lastSnap = snap;
    setHeader(snap);
    renderAgents(snap);
    setCharts(snap);
    setProjects(snap);
    setActivity(snap);
    setSessions(snap);
    screen.render();
  }

  function start() {
    tick();
    state.timer = setInterval(() => { if (!state.paused) tick(); }, intervalMs);
  }

  function moveSelection(delta) {
    const idxs = state.rows
      .map((r, i) => r.kind === "agent" ? i : -1)
      .filter(i => i >= 0);
    if (idxs.length === 0) return;
    let pos = idxs.findIndex(i => state.rows[i].agent.pid === state.selectedPid);
    if (pos < 0) pos = 0;
    pos = Math.max(0, Math.min(idxs.length - 1, pos + delta));
    state.selectedPid = state.rows[idxs[pos]].agent.pid;
    if (state.lastSnap) renderAgents(state.lastSnap);
    screen.render();
  }

  screen.key(["q", "C-c"], () => { clearInterval(state.timer); screen.destroy(); process.exit(0); });
  screen.key(["?", "h"],   () => { help.hidden ? help.show() : help.hide(); screen.render(); });
  screen.key(["p"],        () => { state.paused = !state.paused; if (state.lastSnap) setHeader(state.lastSnap); screen.render(); });
  screen.key(["r"],        () => { tick(); });
  screen.key(["s"],        () => { state.sortIdx = (state.sortIdx + 1) % SORTS.length; tick(); });
  screen.key(["g"],        () => { state.grouped = !state.grouped; tick(); });
  screen.key(["f"],        () => {
    filterPrompt.show();
    filterPrompt.setValue(state.filter);
    filterPrompt.focus();
    filterPrompt.readInput((err, value) => {
      filterPrompt.hide();
      if (typeof value === "string") state.filter = value.trim();
      tick();
    });
    screen.render();
  });
  screen.key(["escape"], () => { state.filter = ""; tick(); });
  screen.key(["j", "down"], () => moveSelection(+1));
  screen.key(["k", "up"],   () => moveSelection(-1));

  start();
  return { screen, stop: () => { clearInterval(state.timer); screen.destroy(); } };
}

function escapeTags(s) {
  return String(s == null ? "" : s).replace(/\{/g, "\\{").replace(/\}/g, "\\}");
}
function pad(s, n)  { s = String(s == null ? "" : s); return s.length >= n ? s.slice(0, n) : s + " ".repeat(n - s.length); }
function lpad(s, n) { s = String(s == null ? "" : s); return s.length >= n ? s.slice(0, n) : " ".repeat(n - s.length) + s; }

module.exports = { run };
