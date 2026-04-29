# agtop

> Like `top`, but for AI coding agents.

A terminal UI that surfaces every AI coding agent running on your system in
one place — live PIDs, CPU%, RSS, working directory, files being written,
uptime, in-flight tool calls, and what each agent is *currently doing* —
plus per-project rollups, smooth braille charts, and a multi-vendor
sessions panel.

Detects the major agent CLIs out of the box and you can teach it about
anything else with a one-line regex.

Implemented as a static **Rust** binary (~3 MB) on **ratatui** + **crossterm**
+ **clap**. No runtime dependencies.

```
╭ agtop  active 13 · busy 1 · subagents 2 · waiting 4 · done 5 · projects 11 · cpu 17.2% · mem 4.0/132G ─╮
├ Agents (project-grouped) ─────────────────────────────┬ CPU  32 cores · now 17.2% · peak 38.4% · avg 4.2% ┤
│ ◆ xsol     1 agent · 16.3% cpu · 626M mem  +1 sub      │ ▁▂▃▄▆█▇▆▄▂▁▂▄▆█████                                  │
│   ● BUSY  claude   pid 404872  16.3% ████░  626M  4d   │ ● xsol      claude  ████████████  16.3%             │
│           Bash: cargo test                             │ ● agtop     claude  ███           3.8%              │
│   └ +1 sub: code-reviewer                              │ ○ marinade  claude  ·             0.0%              │
│                                                        │ ○ ollama    ollama  ·             0.0%              │
│ ◆ agtop    1 agent · 3.8% cpu · 469M mem               ├ Memory by agent  4.0G across 13 agents ─────────────┤
│   ● ACTV  claude   pid 3847918  3.8% █░░░  469M  47m   │ ● xsol      claude  ████████████  626M               │
│           Edit src/ui.rs                               │ ● blueprint claude  █████████     464M               │
│                                                        │ ● marinade  claude  █████████     425M               │
│ ◆ marinade 1 agent · 0.0% cpu · 425M mem               │ agents 4.0G  other 76.6G  free 42.8G / 132G          │
│   ○ idle  claude   pid 4176380  0.0% ····  425M  10h   ├ Status distribution  13 live agents ─────────────────┤
│           (idle 21m)                                   │   ● BUSY    1 ████░░░░░░░░░░  9%                     │
├ Projects ──────────────┬ Activity ─────────────────────┤   ● ACTV    1 ████░░░░░░░░░░  9%                     │
│ ● xsol      1  16% ████ │ 23:14:57 ● spawn claude xsol  │   ○ idle    9 ███████████░░░ 70%                    │
│ ● agtop     1   4% █▏   │ 23:13:22 ◌ exit  codex 98271  ├ Claude sessions — recent tasks ──────────────────────┤
│ ○ marinade  1   0%      │                               │   ● xsol      End of turn: Subliminal monetization … │
╰─ q quit · ? help · s sort(smart) · g group(on) · / filter · p pause · r refresh · ↑↓ select ────────────────╯
```

(ASCII approximation — the real thing has rounded borders, RGB chart colors,
and per-agent accent chips.)

---

## Install

### Arch Linux / CachyOS / Manjaro

```sh
git clone https://github.com/mbrassey/agtop.git && cd agtop
packages/pacman/build.sh
sudo pacman -U packages/pacman/agtop-0.4.0-1-x86_64.pkg.tar.zst
```

`pacman -Q agtop` will then show the package as managed (Explicitly installed,
MIT, owns `/usr/bin/agtop`). Upgrade in-place by re-running `build.sh` after
`git pull` and `sudo pacman -U` the new `.pkg.tar.zst`. Remove with
`sudo pacman -R agtop`.

### Debian / Ubuntu

```sh
git clone https://github.com/mbrassey/agtop.git && cd agtop
packages/deb/build.sh
sudo apt install ./packages/deb/agtop_0.4.0_amd64.deb
```

### Cargo (any platform with Rust)

```sh
cargo install --path .          # installs into ~/.cargo/bin/agtop
```

### npm (wraps the Rust binary)

```sh
npm install -g agtop
```

The npm postinstall downloads the prebuilt binary from GitHub Releases when
available, and falls back to `cargo install agtop` if Rust is on `PATH`.

### From source

```sh
git clone https://github.com/mbrassey/agtop.git && cd agtop
cargo build --release
./target/release/agtop
```

---

## Usage

```sh
agtop                       # full TUI
agtop --once                # one-shot snapshot, like `top -b -n 1`
agtop -1 --top 10           # top-10 agents and exit
agtop --json | jq           # machine-readable JSON for scripting
agtop --interval 0.5        # half-second refresh
agtop --filter aider        # only show agents whose label/cmd/cwd matches
agtop --sort mem            # sort by RSS
agtop --list-builtins       # print built-in matcher list
agtop -m "myagent=python.*my_agent\.py"   # add a custom matcher
```

### CLI flags (`agtop --help`)

| Flag                          | Default | Description                                             |
| ----------------------------- | ------- | ------------------------------------------------------- |
| `-V`, `--version`             |         | Print version and exit                                  |
| `-h`, `--help`                |         | Print help and exit                                     |
| `-1`, `--once`                |         | Print a one-shot snapshot and exit (no TUI)             |
| `-j`, `--json`                |         | Machine-readable JSON; implies `--once`                 |
| `-i`, `--interval <SECONDS>`  | `1.5`   | TUI / iteration refresh interval                        |
| `-n`, `--iterations <COUNT>`  | `1`     | With `--once`, print N snapshots delimited by `---`     |
| `-f`, `--filter <SUBSTR>`     |         | Only show agents matching label / cmdline / cwd / project |
| `-s`, `--sort <KEY>`          | `smart` | `smart` \| `cpu` \| `mem` \| `uptime` \| `agent`        |
| `-m`, `--match <LABEL=REGEX>` |         | Add a custom agent matcher (repeatable)                 |
| `--no-color`                  |         | Disable ANSI colors in `--once` output                  |
| `--top <N>`                   | `0`     | With `--once`, only show top N agents (`0` = all)       |
| `--list-builtins`             |         | Print built-in matcher list and exit                    |

### TUI keybindings

| Key            | Action                                            |
| -------------- | ------------------------------------------------- |
| `q`, `Ctrl-C`  | Quit                                              |
| `?`, `h`       | Toggle help overlay                               |
| `p`            | Pause / resume refresh                            |
| `r`            | Refresh now                                       |
| `s`            | Cycle sort (`smart` → `cpu` → `mem` → `uptime` → `agent`) |
| `g`            | Toggle project grouping                           |
| `/`, `f`       | Filter (Esc to clear)                             |
| `j` / `k`, ↓/↑ | Move selection (tracks the agent's PID across refreshes) |
| `Esc`          | Clear filter / dismiss prompt                     |

### Environment

| Variable      | Effect                                                          |
| ------------- | --------------------------------------------------------------- |
| `AGTOP_MATCH` | Semicolon-separated `label=regex` matchers, additive to builtins |

---

## What it shows

### Status badges

Every agent row carries one of seven status badges. The collector blends each
session reader's verdict with the live process's CPU% so an agent mid-generation
(transcript hasn't flushed yet) doesn't get mis-tagged `idle`.

| Badge   | Trigger                                                                                |
| ------- | -------------------------------------------------------------------------------------- |
| ● BUSY  | Live process **and** transcript written in last 5s, **or** CPU% ≥ 20 (universal override) |
| ◆ SPWN  | Live process with one or more in-flight tool calls (subagents currently running)       |
| ● ACTV  | Live process with transcript activity in last 60s, **or** CPU% ≥ 3 (universal override), **or** CPU% ≥ 1 if otherwise idle |
| ○ idle  | Live process up but quiet for >60s and CPU% below threshold                            |
| ◌ WAIT  | No live process, but session activity in the last 24h                                  |
| ✓ DONE  | Session ended (Claude `stop_reason: end_turn`/`stop_sequence`, Codex `session_end`)    |
| · stale | None of the above — last activity older than 24h                                       |

### Layout

- **Header** — totals chip strip, sort label, group toggle, filter, pause indicator
- **Left top: Agents (grouped by project)** — per-project header with totals (agent count, total CPU, total MEM, in-flight subagents), then the agent rows clustered below. Each row: status badge · agent label chip · pid · CPU% with a 6-cell mini-bar · RSS · uptime · `+N sub` chip when subagents are spawned · "DOING" — current tool, current task subject, or friendly idle/waiting/done label.
- **Left bottom: Projects** — per-project rollup as a horizontal bar list, dominant-status glyph on the left
- **Left bottom: Activity** — recent spawn/exit events with timestamps and per-agent accent colors
- **Right top: CPU** — single-line braille `Sparkline` showing system CPU history, then a per-agent CPU bar list sorted desc
- **Right middle: Memory by agent** — horizontal bar list of every agent by RSS, plus a 3-segment system memory gauge showing `agents | other | free`
- **Right middle: Status distribution** — six htop-style segment bars (BUSY, SPWN, ACTV, idle, WAIT, DONE) with count + proportional bar + %
- **Right bottom: Claude sessions — recent tasks** — in-flight Task subagents banner + recent task subjects, color-coded by session status
- **Footer** — keybinding cheatsheet

---

## How it works

### Live process metrics — `/proc`

Every tick, the collector walks `/proc/[pid]/{stat,cmdline,cwd,exe,io,fdinfo,fd}`:

- **CPU%** = `(Δutime + Δstime) / Δsystem-cpu × num-cpus × 100` (top-style),
  EWMA-smoothed per-pid (`cpu_t = 0.6 × cpu_{t-1} + 0.4 × current`) so the
  table doesn't jitter sample-to-sample.
- **RSS** from `/proc/<pid>/stat` field 24 × page size.
- **Uptime** = `boot-time + starttime/CLK_TCK`.
- **Writable open files** scanned from `/proc/<pid>/fdinfo` (any FD with
  `O_WRONLY` or `O_RDWR` flags), filtered to skip `/dev/*`, pipes, sockets,
  anonymous inodes.

### Stable sort

Agents are sorted by:

1. status priority (`busy` → `spawning` → `active` → `idle` → `waiting` → `completed` → `stale`)
2. project name (alphabetical — same-project rows cluster)
3. CPU% desc
4. RSS desc
5. PID asc

So row positions are stable tick-to-tick. The TUI selection tracks the agent's
PID across refreshes, so the cursor doesn't jump when sort positions shift.

### Multi-vendor session enrichment

Each per-vendor module exposes a `summarise(live_agents, now_ms) → SessionsResult`.
The collector calls all of them and merges via `sessions::merge()`. Each agent
row then either picks up its enrichment (current tool, current task,
in-flight count, session id) or falls back to the universal CPU% override.

| Module           | Source                                                | Pulls                                                                                                                  |
| ---------------- | ----------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `src/claude.rs`  | `~/.claude/projects/<encoded-cwd>/<session>.jsonl`    | current `tool_use` name, `TodoWrite` in-progress / `Task` subject / latest assistant prose, in-flight `Task`/`Agent` subagents (count of `tool_use` IDs without matching `tool_result`), `stop_reason` |
| `src/codex.rs`   | `~/.codex/sessions/<YYYY>/<MM>/<DD>/<rollout>.jsonl`  | current `function_call` name, last user prompt, last assistant text, in-flight `function_call` (no matching `function_call_output`). Walks date-partitioned tree (max depth 4), tolerates both nested `payload` envelope and flat schema, and the `local_shell_call` / `tool_use` aliases |
| `src/generic.rs` | `/proc/<pid>/fdinfo` writable FDs                     | most recently modified file the agent has open for write, surfaced as a relative path under cwd in the DOING column. Status from CPU%. Applies to every label that doesn't have a dedicated module |

The collector then applies a universal CPU% override on top:

- CPU% ≥ 20 → `Busy`
- CPU% ≥ 3 and current status is `Idle` or `Stale` → `Active`
- CPU% ≥ 1 and current status is `Idle` → `Active`

So process state always wins over flush-lag in any session reader.

### Project grouping

Each agent's project name is `basename(cwd)`. The collector builds per-project
aggregates (`agents` count, total `cpu`, total `rss`, total `subagents`, status
counts) which power the "Projects" panel and the "Agents (grouped by project)"
view. Projects are sorted by busy-count desc, then total CPU desc, then name.

---

## Built-in agent matchers

20 patterns ship out of the box. `agtop --list-builtins` always prints the
canonical list; this is the current set:

```
claude            (^|[\s/])claude(-code)?(\s|$)
claude-code       @anthropic-ai/claude-code
codex             (^|[\s/])codex(\s|$)
openai-codex      @openai/codex
aider             (^|[\s/])aider(\s|$|\.)
cursor-agent      (^|[\s/])cursor-agent(\s|$)
gemini            (^|[\s/])gemini(-cli)?(\s|$)
goose             (^|[\s/])goose(\s|$)
continue          (^|[\s/])continue(-cli|-agent)?(\s|$)
opencode          (^|[\s/])opencode(\s|$)
copilot           gh[\s-]copilot|github-copilot-cli
cody              (^|[\s/])cody(\s|$)
amp               (^|[\s/])amp(\s|$)|@sourcegraph/amp
crush             (^|[\s/])crush(\s|$)
mods              (^|[\s/])mods(\s|$)
sgpt              (^|[\s/])sgpt(\s|$)
llm               (^|[\s/])llm(\s|$)
ollama            (^|[\s/])ollama(\s+(run|chat|serve)|$)
fabric            (^|[\s/])fabric(\s|$)
block-goose       (^|[\s/])goose-server
```

The word-boundary prefix `(^|[\s/])` catches both bare-binary invocations
(`/usr/bin/claude`) and module-style invocations (`python -m aider`).

### Custom matchers

```sh
# repeatable -m flag
agtop -m "internal-bot=python.*src/agent\.py" \
      -m "rag-worker=node.*workers/rag\.js"

# or set $AGTOP_MATCH so it's always on
export AGTOP_MATCH="internal-bot=python.*src/agent\.py;rag-worker=node.*workers/rag\.js"
```

Custom matchers are appended to (not replacing) the built-in list.

---

## JSON output

`agtop --json` (or `agtop -1 --json`) writes a single JSON object to stdout,
suitable for `jq`, dashboards, or alerting. All field names are `snake_case`
and the schema below matches the live shape:

```json
{
  "now": 1777439481861,
  "platform": "linux",
  "note": null,
  "sys_cpus": 32,
  "mem_total": 132499206144,
  "mem_available": 46721998848,
  "aggregates": {
    "cpu": 17.2, "mem_bytes": 4257710080,
    "active": 13, "busy": 1, "waiting": 4, "completed": 5,
    "subagents": 2, "project_count": 11
  },
  "agents": [
    {
      "pid": 404872, "label": "claude", "status": "busy",
      "project": "xsol",
      "current_tool": "Bash", "current_task": "running tests",
      "subagents": 1, "session_id": "abc-123", "session_age_ms": 3200,
      "cpu": 16.3, "cpu_raw": 14.8,
      "rss": 626491392, "vsize": 75879088128,
      "threads": 14, "state": "S", "ppid": 1453022, "uptime_sec": 345600,
      "cwd": "/home/matt/code/xsol",
      "exe": "/home/matt/.local/share/claude/versions/2.1.119",
      "cmdline": "claude --dangerously-skip-permissions",
      "read_bytes": 1440100352, "write_bytes": 19001344,
      "writing_files": [],
      "writing_dirs": []
    }
  ],
  "projects": [
    {
      "project": "xsol", "agents": 1, "cpu": 16.3, "rss": 626491392,
      "subagents": 1, "statuses": { "busy": 1 },
      "cwd": "/home/matt/code/xsol"
    }
  ],
  "sessions": {
    "sessions": [
      {
        "id": "...", "project": "...", "project_short": "xsol",
        "file": "...", "size_bytes": 12345, "mtime_ms": 1777439481861,
        "age_ms": 3200, "status": "busy",
        "stop_reason": null, "last_task": "running tests", "last_tool": "Bash",
        "current_tool": "Bash", "in_flight_tasks": 1,
        "live_pid": 404872, "is_most_recent": true
      }
    ],
    "recent_tasks": [
      { "project": "...", "project_short": "xsol",
        "task": "running tests", "mtime_ms": 1777439481861, "status": "busy" }
    ],
    "active": 13, "busy": 1, "waiting": 4, "completed": 5
  },
  "history": {
    "total":  [...],
    "active": [...],
    "busy":   [...],
    "cpu":    [...],
    "mem":    [...]
  },
  "activity": [
    { "t": 1777439481861, "kind": "spawn",
      "label": "claude", "pid": 384791, "cwd": "/home/matt/code/agtop" }
  ]
}
```

History arrays carry the last 60 samples (CPU%, MEM in MB, total agents,
active+waiting count, busy+spawning count). `kind` is `"spawn"` or `"exit"`.

---

## Repo layout

```
agtop/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs            entrypoint  (33 lines — installs SIGPIPE handler)
│   ├── cli.rs             clap CLI + --once / --json paths   (254)
│   ├── ui.rs              ratatui TUI                        (961)
│   ├── theme.rs           palette + per-agent accents         (98)
│   ├── collector.rs       snapshot orchestrator              (337)
│   ├── proc_.rs           /proc parser                       (211)
│   ├── claude.rs          Claude Code session reader         (290)
│   ├── codex.rs           OpenAI Codex session reader        (358)
│   ├── generic.rs         vendor-agnostic fallback            (90)
│   ├── sessions.rs        shared types + merge()              (63)
│   ├── matchers.rs        built-in + user matchers + tests   (117)
│   ├── model.rs           Snapshot / Agent / Session etc.    (181)
│   └── format.rs          bytes / pct / dur / shorten         (81)
├── README.md
├── LICENSE                MIT
└── packages/
    ├── npm/build.sh       → agtop-<v>.tgz (postinstall fetches binary)
    ├── deb/               → agtop_<v>_<arch>.deb
    │   ├── DEBIAN/control
    │   └── build.sh
    └── pacman/            → agtop-<v>-1-<arch>.pkg.tar.zst
        ├── PKGBUILD
        └── build.sh
```

13 source files, ~3,074 lines of Rust.

---

## Building the packages

```sh
# Arch .pkg.tar.zst (uses makepkg + cargo build --release)
packages/pacman/build.sh

# Debian/Ubuntu .deb (no dpkg-deb required — falls back to ar+tar)
packages/deb/build.sh

# npm tarball (thin shim, postinstall fetches binary)
packages/npm/build.sh
```

Each subfolder has its own README with submission notes for the npm registry,
Debian PPAs, and the AUR.

---

## Platform support

|                          | Live process metrics | Claude sessions | Codex sessions | Generic fallback |
| ------------------------ | -------------------- | --------------- | -------------- | ---------------- |
| **Linux** (x86_64/arm64) | ✓                    | ✓               | ✓              | ✓                |
| **macOS / *BSD**         | falls back to sessions-only | ✓        | ✓              | (no `/proc`)     |
| **Windows**              | not supported        | ✓               | ✓              | (no `/proc`)     |

The live process scanner relies on `/proc`. On non-Linux platforms agtop
prints a session summary and exits — PRs welcome for `libproc` (macOS/BSD)
and `NtQuerySystemInformation` (Windows) backends.

The Claude session reader has been validated against the actual transcript
format on this maintainer's machine. The Codex session reader is implemented
against the documented OpenAI Codex CLI rollout schema with defensive probing
for both the nested-`payload` envelope and the flat shape; if your Codex
version writes a schema agtop doesn't understand, the agent still appears in
the table via the generic enricher.

---

## Contributing

```sh
cargo build                # debug build, fast
cargo test                 # unit tests (matchers, format, smoke)
cargo run                  # full TUI
cargo run -- --once        # snapshot for quick iteration
cargo run -- --json | jq   # JSON for scripting
cargo clippy               # lint
```

When adding a new built-in agent, edit `src/matchers.rs` and add a
classification test in the same file's `#[cfg(test)]` block. When adding
a new vendor's session reader, model it on `src/codex.rs` and slot it into
`collector.rs` alongside the existing calls — it just needs to return a
`SessionsResult`.

---

## License

MIT — see `LICENSE`.
