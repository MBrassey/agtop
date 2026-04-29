"use strict";

// btop-style multi-panel TUI on top of blessed + blessed-contrib.
// Layout (12-row x 12-col grid):
//
//   row 0       header / summary  (full width)
//   rows 1-7    agent table       (cols 0-7)         |  cpu sparkline  (cols 8-11, rows 1-3)
//                                                    |  mem sparkline  (cols 8-11, rows 4-5)
//                                                    |  agents-over-time line (cols 8-11, rows 6-7)
//   rows 8-9    activity log      (cols 0-7)         |  sessions panel (cols 8-11, rows 8-9)
//   rows 10-11  details / writing-paths              (full width)
//
// Keys: q/Ctrl-C quit, ?/h help overlay, p pause, r refresh now,
//       s cycle sort, f filter, c toggle completed list, j/k or arrows move.

const blessed = require("blessed");
const contrib = require("blessed-contrib");
const fmt = require("./format.js");

const SORTS = [
  { key: "cpu",     label: "CPU",     fn: (a, b) => b.cpu - a.cpu },
  { key: "mem",     label: "MEM",     fn: (a, b) => b.rss - a.rss },
  { key: "uptime",  label: "UPTIME",  fn: (a, b) => b.uptimeSec - a.uptimeSec },
  { key: "pid",     label: "PID",     fn: (a, b) => a.pid - b.pid },
  { key: "label",   label: "AGENT",   fn: (a, b) => a.label.localeCompare(b.label) },
];

function run({ collector, intervalMs = 1500, version = "0.1.0", initialFilter = "" }) {
  const screen = blessed.screen({
    smartCSR: true,
    fullUnicode: true,
    title: "agtop — agent monitor",
  });

  const grid = new contrib.grid({ rows: 12, cols: 12, screen });

  const header = grid.set(0, 0, 1, 12, blessed.box, {
    tags: true,
    style: { fg: "white", bg: "black" },
    border: { type: "line", fg: "cyan" },
    label: " agtop ",
  });

  const table = grid.set(1, 0, 7, 8, contrib.table, {
    label: " Agents ",
    keys: true,
    fg: "white",
    selectedFg: "black",
    selectedBg: "cyan",
    interactive: true,
    columnSpacing: 2,
    columnWidth: [6, 12, 6, 7, 8, 22, 30],
    border: { type: "line", fg: "cyan" },
  });

  const cpuLine = grid.set(1, 8, 3, 4, contrib.line, {
    label: " CPU% (sum) ",
    style: { line: "yellow", text: "white", baseline: "white" },
    showLegend: false,
    minY: 0,
    border: { type: "line", fg: "cyan" },
  });

  const memLine = grid.set(4, 8, 2, 4, contrib.line, {
    label: " MEM (MB, sum) ",
    style: { line: "magenta", text: "white", baseline: "white" },
    showLegend: false,
    minY: 0,
    border: { type: "line", fg: "cyan" },
  });

  const countLine = grid.set(6, 8, 2, 4, contrib.line, {
    label: " Active agents ",
    style: { line: "green", text: "white", baseline: "white" },
    showLegend: false,
    minY: 0,
    border: { type: "line", fg: "cyan" },
  });

  const activity = grid.set(8, 0, 2, 8, contrib.log, {
    label: " Recent activity ",
    fg: "white",
    border: { type: "line", fg: "cyan" },
    bufferLength: 200,
  });

  const sessions = grid.set(8, 8, 2, 4, blessed.box, {
    label: " Claude sessions ",
    tags: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
  });

  const detail = grid.set(10, 0, 2, 12, blessed.box, {
    label: " Selected: writing paths · cmdline ",
    tags: true,
    border: { type: "line", fg: "cyan" },
    style: { fg: "white" },
  });

  const help = blessed.box({
    parent: screen,
    top: "center",
    left: "center",
    width: 64,
    height: 18,
    border: { type: "line", fg: "cyan" },
    label: " Help ",
    tags: true,
    hidden: true,
    style: { bg: "black", fg: "white" },
    content:
      "{bold}agtop{/bold} — agent monitor\n\n" +
      "  q, Ctrl-C   quit\n" +
      "  ?, h        toggle this help\n" +
      "  p           pause / resume refresh\n" +
      "  r           refresh now\n" +
      "  s           cycle sort (cpu/mem/uptime/pid/agent)\n" +
      "  f           filter agents by substring (Esc to clear)\n" +
      "  c           toggle completed-sessions view\n" +
      "  j/k, ↓/↑    move selection in the agent table\n" +
      "  Enter       focus details for the selected agent\n",
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

  let state = {
    paused: false,
    sortIdx: 0,
    filter: initialFilter,
    showCompleted: false,
    rows: [],
    selected: 0,
    lastSnap: null,
    timer: null,
  };

  function setHeader(snap) {
    const a = snap.aggregates;
    const sortLabel = SORTS[state.sortIdx].label;
    const flt = state.filter ? `  filter:{yellow-fg}${escapeTags(state.filter)}{/yellow-fg}` : "";
    const paused = state.paused ? "  {red-fg}PAUSED{/red-fg}" : "";
    const compMark = state.showCompleted ? "  {magenta-fg}+completed{/magenta-fg}" : "";
    const memUsedTotal = snap.memTotal
      ? ` / sys ${fmt.bytes(snap.memTotal - snap.memAvailable)} of ${fmt.bytes(snap.memTotal)}`
      : "";
    header.setContent(
      ` {bold}agtop{/bold} v${version}` +
      `   active:{green-fg}${a.active}{/green-fg}` +
      `   waiting:{yellow-fg}${a.waiting}{/yellow-fg}` +
      `   completed:{cyan-fg}${a.completed}{/cyan-fg}` +
      `   cpu:{yellow-fg}${fmt.pct(a.cpu)}{/yellow-fg}` +
      `   mem:{magenta-fg}${fmt.bytes(a.memBytes)}{/magenta-fg}${memUsedTotal}` +
      `   sort:${sortLabel}${flt}${compMark}${paused}`
    );
  }

  function visibleAgents(snap) {
    const sorter = SORTS[state.sortIdx].fn;
    let rows = snap.agents.slice().sort(sorter);
    if (state.filter) {
      const f = state.filter.toLowerCase();
      rows = rows.filter(a =>
        a.label.toLowerCase().includes(f) ||
        a.cmdline.toLowerCase().includes(f) ||
        (a.cwd && a.cwd.toLowerCase().includes(f)) ||
        String(a.pid) === f
      );
    }
    return rows;
  }

  function setTable(snap) {
    const rows = visibleAgents(snap);
    state.rows = rows;
    table.setData({
      headers: ["PID", "AGENT", "CPU%", "MEM", "UPTIME", "CWD", "TASK / CMD"],
      data: rows.map(a => [
        String(a.pid),
        a.label,
        a.cpu.toFixed(1),
        fmt.bytes(a.rss),
        fmt.dur(a.uptimeSec),
        fmt.shorten(fmt.tildeify(a.cwd), 22),
        fmt.shorten(a.cmdshort, 30),
      ]),
    });
    const tbl = table.rows;
    if (rows.length === 0) {
      state.selected = 0;
    } else {
      state.selected = Math.min(state.selected, rows.length - 1);
      try { tbl.select(state.selected); } catch {}
    }
  }

  function setCharts(snap) {
    const xs = snap.history.cpu.map((_, i) => String(i));
    cpuLine.setData([{ x: xs, y: snap.history.cpu }]);
    memLine.setData([{ x: xs, y: snap.history.mem }]);
    countLine.setData([{ x: xs, y: snap.history.active }]);
  }

  function setActivity(snap) {
    activity.logLines = activity.logLines || [];
    // We rewrite the contents each tick so paused state freezes.
    activity.setItems
      ? activity.setItems(formatActivity(snap.activity))
      : activity.setContent(formatActivity(snap.activity).join("\n"));
  }

  function formatActivity(events) {
    return events.map(e => {
      const t = new Date(e.t).toTimeString().slice(0, 8);
      if (e.kind === "spawn") {
        return `${t}  spawn  ${pad(e.label, 12)} pid=${e.pid}  ${fmt.tildeify(e.cwd || "")}`;
      } else if (e.kind === "exit") {
        return `${t}  exit   ${pad(e.label, 12)} pid=${e.pid}`;
      }
      return `${t}  ${e.kind}  ${e.label || ""}`;
    });
  }

  function setSessions(snap) {
    const s = snap.sessions || { sessions: [], recentTasks: [] };
    const lines = [];
    lines.push(`{bold}Active{/bold}    ${s.active || 0}`);
    lines.push(`{bold}Waiting{/bold}   ${s.waiting || 0}`);
    lines.push(`{bold}Completed{/bold} ${s.completed || 0}`);
    lines.push("");
    lines.push("{bold}Recent tasks{/bold}");
    const tasks = (s.recentTasks || []).slice(0, 8);
    if (tasks.length === 0) lines.push("  (none in last 24h)");
    for (const t of tasks) {
      lines.push(`  · ${escapeTags(t.task)}`);
      lines.push(`    {gray-fg}${escapeTags(fmt.tildeify(t.project))}{/gray-fg}`);
    }
    sessions.setContent(lines.join("\n"));
  }

  function setDetail(snap) {
    const a = state.rows[state.selected];
    if (!a) {
      detail.setContent("  (no selection)");
      return;
    }
    const writing = (a.writingFiles || []).slice(0, 4).map(f => fmt.tildeify(f));
    const dirs = (a.writingDirs || []).slice(0, 3).map(d => fmt.tildeify(d));
    const lines = [
      ` {bold}${a.label}{/bold}  pid={cyan-fg}${a.pid}{/cyan-fg}  threads=${a.threads}  state=${a.state}  uptime=${fmt.dur(a.uptimeSec)}`,
      ` exe : ${escapeTags(a.exe)}`,
      ` cwd : ${escapeTags(fmt.tildeify(a.cwd))}`,
      ` cmd : ${escapeTags(a.cmdline)}`,
      ` io  : read ${fmt.bytes(a.readBytes)}  write ${fmt.bytes(a.writeBytes)}`,
      ` writing : ${writing.length ? writing.map(escapeTags).join("  ") : "(no open writable files)"}`,
      ` dirs    : ${dirs.length ? dirs.map(escapeTags).join("  ") : ""}`,
    ];
    detail.setContent(lines.join("\n"));
  }

  function tick() {
    const snap = collector.snapshot();
    state.lastSnap = snap;
    setHeader(snap);
    setTable(snap);
    setCharts(snap);
    setActivity(snap);
    setSessions(snap);
    setDetail(snap);
    screen.render();
  }

  function start() {
    tick();
    state.timer = setInterval(() => { if (!state.paused) tick(); }, intervalMs);
  }

  // Key bindings.
  screen.key(["q", "C-c"], () => { clearInterval(state.timer); screen.destroy(); process.exit(0); });
  screen.key(["?", "h"], () => { help.hidden ? help.show() : help.hide(); screen.render(); });
  screen.key(["p"], () => { state.paused = !state.paused; if (state.lastSnap) setHeader(state.lastSnap); screen.render(); });
  screen.key(["r"], () => { tick(); });
  screen.key(["s"], () => { state.sortIdx = (state.sortIdx + 1) % SORTS.length; tick(); });
  screen.key(["c"], () => { state.showCompleted = !state.showCompleted; tick(); });
  screen.key(["f"], () => {
    filterPrompt.show();
    filterPrompt.setValue(state.filter);
    filterPrompt.focus();
    filterPrompt.readInput((err, value) => {
      filterPrompt.hide();
      if (typeof value === "string") state.filter = value.trim();
      table.focus();
      tick();
    });
    screen.render();
  });
  screen.key(["escape"], () => { state.filter = ""; tick(); });
  screen.key(["j", "down"], () => { state.selected = Math.min(state.rows.length - 1, state.selected + 1); try { table.rows.select(state.selected); } catch {} setDetail(state.lastSnap || { agents: [] }); screen.render(); });
  screen.key(["k", "up"],   () => { state.selected = Math.max(0, state.selected - 1); try { table.rows.select(state.selected); } catch {} setDetail(state.lastSnap || { agents: [] }); screen.render(); });

  table.focus();
  start();
  return { screen, stop: () => { clearInterval(state.timer); screen.destroy(); } };
}

function escapeTags(s) {
  return String(s == null ? "" : s).replace(/\{/g, "\\{").replace(/\}/g, "\\}");
}
function pad(s, n) { s = String(s == null ? "" : s); return s.length >= n ? s.slice(0, n) : s + " ".repeat(n - s.length); }

module.exports = { run };
