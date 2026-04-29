"use strict";

// Lightweight smoke test. Exercises the library end-to-end without launching
// the TUI: builds matchers, takes a snapshot, formats it, runs the CLI parser,
// and asserts a few invariants. Exits non-zero on any failure so CI can pick
// it up.

const assert = require("assert");
const { Collector } = require("../src/collector.js");
const { BUILTIN_AGENTS, buildMatchers, classify } = require("../src/agents.js");
const fmt = require("../src/format.js");
const { buildProgram } = require("../src/cli.js");

function ok(name, fn) {
  try { fn(); console.log(`ok   ${name}`); }
  catch (e) { console.error(`FAIL ${name}: ${e.message}\n${e.stack}`); process.exit(1); }
}

ok("builtin matchers compile", () => {
  assert(BUILTIN_AGENTS.length > 5, "expected >5 builtins");
  for (const m of BUILTIN_AGENTS) {
    assert(m.label && m.re instanceof RegExp);
  }
});

ok("classify recognises known commands", () => {
  const m = buildMatchers([]);
  assert.strictEqual(classify("/usr/bin/claude --resume", m), "claude");
  assert.strictEqual(classify("node /opt/codex/bin/codex chat", m), "codex");
  assert.strictEqual(classify("python -m aider --no-git", m), "aider");
  assert.strictEqual(classify("/usr/bin/cursor-agent --watch", m), "cursor-agent");
  assert.strictEqual(classify("/usr/bin/bash", m), null);
});

ok("custom matchers append, don't override", () => {
  const m = buildMatchers(["myagent=python.*my_agent\\.py"]);
  assert.strictEqual(classify("python /home/x/my_agent.py --foo", m), "myagent");
  // Builtin still wins on its pattern.
  assert.strictEqual(classify("/usr/bin/claude", m), "claude");
});

ok("format helpers", () => {
  assert.strictEqual(fmt.bytes(0), "0B");
  assert.strictEqual(fmt.bytes(1024), "1.0K");
  assert.strictEqual(fmt.bytes(1024 * 1024), "1.0M");
  assert.strictEqual(fmt.dur(5), "5s");
  assert.match(fmt.dur(125), /^2m05s$/);
  assert.match(fmt.dur(3661), /^1h01m$/);
});

ok("collector snapshot has expected shape", () => {
  const c = new Collector();
  const snap = c.snapshot();
  assert(snap && typeof snap === "object");
  assert(Array.isArray(snap.agents));
  assert(snap.aggregates && "active" in snap.aggregates);
  assert(snap.history && Array.isArray(snap.history.cpu));
  // sessions block always present, even with empty Claude project dir
  assert(snap.sessions);
});

ok("cli --help works", () => {
  const program = buildProgram();
  const helpText = program.helpInformation();
  assert(/agtop/.test(helpText));
  assert(/--once/.test(helpText));
  assert(/--json/.test(helpText));
  assert(/--match/.test(helpText));
});

console.log("\nall smoke tests passed.");
