"use strict";

// Curated list of known AI coding-agent CLIs. Each entry maps a label
// (used as the canonical "name" column in the UI) to a regex that is matched
// against the process's argv[0] basename and full command line. Order matters:
// the first match wins, so put more specific patterns first.
//
// Users can extend this list at runtime via --match "label=regex" (repeatable)
// or via $AGTOP_MATCH (semicolon-separated). Built-ins are never overridden;
// user matches are appended.

// Word-prefix used by most patterns: start-of-string, slash, or whitespace.
// This catches both "/usr/bin/foo" and "python -m foo" invocations.
const P = "(^|[\\s/])";

const BUILTIN_AGENTS = [
  { label: "claude",       re: new RegExp(P + "claude(-code)?(\\s|$)") },
  { label: "claude-code",  re: /@anthropic-ai\/claude-code/ },
  { label: "codex",        re: new RegExp(P + "codex(\\s|$)") },
  { label: "openai-codex", re: /@openai\/codex/ },
  { label: "aider",        re: new RegExp(P + "aider(\\s|$|\\.)") },
  { label: "cursor-agent", re: new RegExp(P + "cursor-agent(\\s|$)") },
  { label: "gemini",       re: new RegExp(P + "gemini(-cli)?(\\s|$)") },
  { label: "goose",        re: new RegExp(P + "goose(\\s|$)") },
  { label: "continue",     re: new RegExp(P + "continue(-cli|-agent)?(\\s|$)") },
  { label: "opencode",     re: new RegExp(P + "opencode(\\s|$)") },
  { label: "copilot",      re: /gh[\s-]copilot|github-copilot-cli/ },
  { label: "cody",         re: new RegExp(P + "cody(\\s|$)") },
  { label: "amp",          re: new RegExp(P + "amp(\\s|$)|@sourcegraph/amp") },
  { label: "crush",        re: new RegExp(P + "crush(\\s|$)") },
  { label: "mods",         re: new RegExp(P + "mods(\\s|$)") },
  { label: "sgpt",         re: new RegExp(P + "sgpt(\\s|$)") },
  { label: "llm",          re: new RegExp(P + "llm(\\s|$)") },
  { label: "ollama",       re: new RegExp(P + "ollama\\s+(run|chat)") },
  { label: "fabric",       re: new RegExp(P + "fabric(\\s|$)") },
  { label: "block-goose",  re: new RegExp(P + "goose-server") },
];

function buildMatchers(extra) {
  const out = BUILTIN_AGENTS.slice();
  if (Array.isArray(extra)) {
    for (const spec of extra) {
      if (!spec) continue;
      const eq = spec.indexOf("=");
      if (eq < 1) continue;
      const label = spec.slice(0, eq).trim();
      const pattern = spec.slice(eq + 1).trim();
      if (!label || !pattern) continue;
      try {
        out.push({ label, re: new RegExp(pattern), user: true });
      } catch {
        // ignore malformed user pattern
      }
    }
  }
  return out;
}

function classify(cmdline, matchers) {
  const haystack = (cmdline || "").trim();
  if (!haystack) return null;
  for (const m of matchers) {
    if (m.re.test(haystack)) return m.label;
  }
  return null;
}

module.exports = { BUILTIN_AGENTS, buildMatchers, classify };
