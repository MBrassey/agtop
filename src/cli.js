"use strict";

const { Command, Option } = require("commander");
const fmt = require("./format.js");
const { Collector } = require("./collector.js");
const proc = require("./proc.js");
const pkg = require("../package.json");

function buildProgram() {
  const program = new Command();
  program
    .name("agtop")
    .version(pkg.version, "-V, --version", "print version and exit")
    .description(
      "Terminal UI for monitoring AI coding agents on your system.\n" +
      "Like top, but for Claude Code, Codex, Aider, Cursor, Gemini, Goose, and friends."
    )
    .addOption(new Option("-1, --once", "print a one-shot snapshot and exit (no TUI)"))
    .addOption(new Option("-j, --json",  "machine-readable JSON snapshot; implies --once"))
    .addOption(new Option("-i, --interval <seconds>", "TUI refresh interval").default("1.5"))
    .addOption(new Option("-n, --iterations <count>", "with --once, print N snapshots delimited by --- and exit").default("1"))
    .addOption(new Option("-f, --filter <substr>", "only show agents whose label/cmd/cwd matches"))
    .addOption(new Option("-s, --sort <key>", "sort by key").choices(["cpu", "mem", "uptime", "pid", "agent"]).default("cpu"))
    .addOption(new Option("-m, --match <label=regex...>", "additional agent matchers (repeatable)"))
    .addOption(new Option("--no-color", "disable ANSI colors in --once output"))
    .addOption(new Option("--top <N>", "with --once, only show top N agents").default("0"))
    .addOption(new Option("--list-builtins", "print the built-in agent matcher list and exit"))
    .addHelpText("after", `
Examples:
  $ agtop                      # full TUI
  $ agtop --once               # one-shot snapshot, like \`top -b -n 1\`
  $ agtop -1 --top 10          # top-10 active agents and exit
  $ agtop --json | jq          # JSON for scripting
  $ agtop -m "myagent=python.*my_agent\\\\.py"
                               # treat anything matching this regex as an agent

Keys (TUI):
  q / Ctrl-C  quit            ?, h    help overlay
  p           pause           r       refresh now
  s           cycle sort      f       filter
  j/k, ↑/↓    move selection  c       toggle completed view

Environment:
  AGTOP_MATCH   semicolon-separated list of "label=regex" matchers
                (additive to built-ins)

Project: https://github.com/mbrassey/agtop
`);
  return program;
}

function readEnvMatchers() {
  const raw = process.env.AGTOP_MATCH;
  if (!raw) return [];
  return raw.split(";").map(s => s.trim()).filter(Boolean);
}

function once(collector, opts) {
  const iters = Math.max(1, parseInt(opts.iterations, 10) || 1);
  const intervalMs = Math.max(100, Math.round(parseFloat(opts.interval) * 1000));
  const out = (s) => process.stdout.write(s + "\n");
  // Warm-up sample so CPU% has a real delta on the first printed snapshot.
  // Skip the warmup if the user is asking for many iterations — the first
  // sample naturally amortises in that case.
  const warmup = iters === 1;
  function go() {
    let printed = 0;
    function emit() {
      const snap = collector.snapshot();
      if (opts.json) {
        out(JSON.stringify(scrubForJson(snap), null, 2));
      } else {
        out(fmt.summaryLine(snap));
        out(fmt.snapshotTable(snap, { color: opts.color !== false, max: parseInt(opts.top, 10) || 0 }));
      }
      printed++;
      if (printed >= iters) process.exit(0);
      if (!opts.json) out("---");
      setTimeout(emit, intervalMs);
    }
    emit();
  }
  if (warmup) {
    collector.snapshot();
    setTimeout(go, 400);
  } else {
    go();
  }
}

function scrubForJson(snap) {
  return {
    now: snap.now,
    platform: snap.platform,
    note: snap.note,
    sysCpus: snap.sysCpus,
    memTotal: snap.memTotal,
    memAvailable: snap.memAvailable,
    aggregates: snap.aggregates,
    agents: snap.agents.map(a => ({
      pid: a.pid,
      label: a.label,
      status: a.status,
      project: a.project,
      currentTool: a.currentTool,
      currentTask: a.currentTask,
      subagents: a.subagents,
      sessionId: a.sessionId,
      sessionAgeMs: a.sessionAgeMs,
      cpu: round(a.cpu, 2),
      cpuRaw: a.cpuRaw != null ? round(a.cpuRaw, 2) : undefined,
      rss: a.rss,
      vsize: a.vsize,
      threads: a.threads,
      state: a.state,
      ppid: a.ppid,
      uptimeSec: a.uptimeSec,
      cwd: a.cwd,
      exe: a.exe,
      cmdline: a.cmdline,
      readBytes: a.readBytes,
      writeBytes: a.writeBytes,
      writingFiles: a.writingFiles,
      writingDirs: a.writingDirs,
    })),
    projects: snap.projects || [],
    sessions: snap.sessions ? {
      active: snap.sessions.active,
      busy: snap.sessions.busy,
      waiting: snap.sessions.waiting,
      completed: snap.sessions.completed,
      sessions: snap.sessions.sessions,
      recentTasks: snap.sessions.recentTasks,
    } : null,
    activity: snap.activity,
    history: snap.history,
  };
}
function round(n, p) { const k = Math.pow(10, p); return Math.round(n * k) / k; }

function listBuiltins() {
  const { BUILTIN_AGENTS } = require("./agents.js");
  for (const m of BUILTIN_AGENTS) {
    process.stdout.write(`${m.label.padEnd(16)} ${m.re}\n`);
  }
}

function main(argv) {
  const program = buildProgram();
  program.parse(argv);
  const opts = program.opts();

  if (opts.listBuiltins) { listBuiltins(); return; }

  const matchers = (opts.match || []).concat(readEnvMatchers());
  const collector = new Collector({ extraMatchers: matchers, history: 60 });

  if (opts.once || opts.json) {
    once(collector, opts);
    return;
  }

  if (!proc.isLinux()) {
    process.stderr.write("agtop: live process metrics require Linux /proc.\n");
    process.stderr.write("       Falling back to a single Claude-sessions snapshot.\n\n");
    const snap = collector.snapshot();
    process.stdout.write(JSON.stringify(snap.sessions, null, 2) + "\n");
    process.exit(0);
  }

  const ui = require("./ui.js");
  const intervalMs = Math.max(250, Math.round(parseFloat(opts.interval) * 1000));
  ui.run({
    collector,
    intervalMs,
    version: pkg.version,
    initialFilter: opts.filter || "",
  });
}

module.exports = { main, buildProgram };
