# agtop

> Like `top`, but for AI coding agents.

A terminal UI that surfaces every AI coding agent running on your system in
one place: live PIDs, CPU%, RSS, working directory, files being written,
uptime, in-flight Task subagents, and what each one is currently doing —
plus a per-project rollup, real chart-widget history of CPU / MEM / agent
counts, and a Claude Code session panel (active / busy / waiting / done).

Detects the major agent CLIs out of the box — **Claude Code**, **OpenAI
Codex**, **Aider**, **Cursor Agent**, **Gemini CLI**, **Goose**, **Continue**,
**Opencode**, **GitHub Copilot CLI**, **Cody**, **Sourcegraph Amp**, **Crush**,
**Mods**, **sgpt**, **llm**, **Ollama**, **Fabric** — and you can teach it
about anything else with a one-line regex.

Implemented as a static **Rust** binary using **ratatui** + **crossterm** +
**clap**. ~3 MB, no runtime dependencies.

```
╭ agtop v0.2.0  [11 active] [1 busy] [0 subagents] [2 waiting] [5 done] [10 projects] [cpu 17%] [mem 4.0/132G]  sort:smart  group:on ╮
├ Agents ─────────────────────────────────────────────┬ CPU%   now 17.2%  peak 38.4% ─────────────────────────────────────────────┤
│ ◆ xsol     1 agent · 16.3% cpu · 626M mem  +1 sub   │  ▁▂▃▄▆█▇▆▄▂▁▂▄▆█████                                                       │
│   ● BUSY  claude    pid 404872  16.3%   626M    4d   │                                                                            │
│           Bash → cargo test  (running)              ├ MEM    now 4.0G ───────────────────────────────────────────────────────────┤
│   └ +1 sub: code-reviewer (running)                 │  ▁▁▁▂▂▂▃▃▃▄▄▄▄▄                                                            │
│                                                     │                                                                            │
│ ◆ agtop    1 agent · 3.8% cpu · 469M mem            │                                                                            │
│   ● ACTV  claude    pid 3847918  3.8%  469M  47m    ├ Active vs Busy ───────────────────────────────────────────────────────────┤
│           Edit src/ui.rs                            │  ━━━━━━━━━━━━━━━ active   ━━━━━━━━━━━ busy                                 │
│                                                     │                                                                            │
│ ◆ marinade   1 agent · 0.0% cpu · 425M mem  idle    ├ Claude sessions ───────────────────────────────────────────────────────────┤
│   ○ idle  claude    pid 4176380  0.0%  425M   9h    │  1 busy   10 active   2 waiting   5 done                                    │
│           (idle 21m)                                │  Recent tasks                                                              │
│                                                     │   ● xsol      Bash → cargo test                                             │
├ Projects ─────────────────────┬ Activity ──────────┴────────────────────────────────────────────────────────────────────────────┤
│ ● xsol      1  16.3% ████████▏ +1 │ 23:14:57  ● spawn  claude       pid 384791   agtop                                          │
│ ● agtop     1   3.8% ██▏        │ 23:13:22  ◌ exit   codex        pid 98271                                                    │
│ ○ marinade  1   0.0%             │                                                                                              │
╰─ q quit · ? help · s sort(smart) · g group(on) · / filter · p pause · r refresh · ↑↓ select ────────────────────────────────────╯
```

(ASCII approximation — the real thing has rounded borders, gradient-graded
chart lines, and per-agent accent colours.)

## Install

### Arch Linux / CachyOS / Manjaro

```sh
sudo pacman -U agtop-0.2.0-1-x86_64.pkg.tar.zst
```

(Build the `.pkg.tar.zst` yourself with `packages/pacman/build.sh`, or install
from the AUR once published.)

### Debian / Ubuntu

```sh
sudo apt install ./agtop_0.2.0_amd64.deb
```

(Build the `.deb` yourself with `packages/deb/build.sh`, or install from the
PPA once published.)

### Cargo (any platform with Rust)

```sh
cargo install agtop          # (once published to crates.io)
# or, locally:
cargo install --path .
```

### npm (wraps the Rust binary)

```sh
npm install -g agtop
```

The npm postinstall downloads the prebuilt binary from GitHub Releases when
available, and falls back to `cargo install agtop` if Rust is on PATH.

### From source

```sh
git clone https://github.com/mbrassey/agtop.git
cd agtop
cargo build --release
./target/release/agtop
```

## Usage

```
agtop                       # full TUI
agtop --once                # one-shot snapshot, like `top -b -n 1`
agtop -1 --top 10           # top-10 agents and exit
agtop --json | jq           # machine-readable JSON for scripting
agtop --interval 0.5        # half-second refresh
agtop --filter aider        # only show agents whose label/cmd/cwd matches
agtop --sort mem            # sort by RSS
agtop --list-builtins       # print built-in agent matcher list
agtop -m "myagent=python.*my_agent\.py"   # add a custom matcher
```

### All flags

| Flag                          | Description                                          |
| ----------------------------- | ---------------------------------------------------- |
| `-V`, `--version`             | Print version and exit                               |
| `-h`, `--help`                | Print help and exit                                  |
| `-1`, `--once`                | Print a one-shot snapshot and exit (no TUI)          |
| `-j`, `--json`                | Machine-readable JSON; implies `--once`              |
| `-i`, `--interval <SECONDS>`  | Refresh interval (default `1.5`)                     |
| `-n`, `--iterations <COUNT>`  | With `--once`, print N snapshots delimited by `---`  |
| `-f`, `--filter <SUBSTR>`     | Only show agents matching label / cmd / cwd / pid    |
| `-s`, `--sort <KEY>`          | `smart` \| `cpu` \| `mem` \| `uptime` \| `agent`     |
| `-m`, `--match <LABEL=REGEX>` | Add a custom agent matcher (repeatable)              |
| `--no-color`                  | Disable ANSI colors in `--once` output               |
| `--top <N>`                   | With `--once`, only show top N agents                |
| `--list-builtins`             | Print built-in matcher list and exit                 |

### TUI keybindings

| Key            | Action                                            |
| -------------- | ------------------------------------------------- |
| `q`, `Ctrl-C`  | Quit                                              |
| `?`, `h`       | Toggle help overlay                               |
| `p`            | Pause / resume refresh                            |
| `r`            | Refresh now                                       |
| `s`            | Cycle sort (smart → cpu → mem → uptime → agent)   |
| `g`            | Toggle project grouping                           |
| `/` or `f`     | Filter (Esc to clear)                             |
| `j` / `k`, ↓/↑ | Move selection                                    |

### Status legend

| Badge   | Meaning                                                         |
| ------- | --------------------------------------------------------------- |
| ● BUSY  | Process active and the JSONL transcript was written in last 5s, *or* CPU% ≥ 20 |
| ◆ SPWN  | One or more `Task` tool calls in flight (subagents running)     |
| ● ACTV  | Process running recently (CPU% ≥ 3, or transcript active in 60s) |
| ○ idle  | Process up but quiet for >60s and below CPU threshold           |
| ◌ WAIT  | No live process, but session activity in the last 24h           |
| ✓ DONE  | Session ended (`stop_reason: end_turn` / `stop_sequence`)       |

### Environment

| Variable      | Effect                                                          |
| ------------- | --------------------------------------------------------------- |
| `AGTOP_MATCH` | Semicolon-separated `label=regex` matchers, additive to builtins |

## How it works

### Live process metrics — `/proc`

Walks `/proc/[pid]/{stat,cmdline,cwd,exe,io,fdinfo,fd}` per tick. CPU% is
computed as `(Δutime + Δstime) / Δsystem-cpu × num-cpus × 100` (top-style)
and EWMA-smoothed per-pid so the table doesn't jitter. RSS comes from
`stat` field 24 × page size; uptime is `boot-time + starttime/CLK_TCK`.

`fdinfo` is scanned for FDs whose flags include `O_WRONLY` / `O_RDWR` —
these are the files an agent is *currently writing*, with `/dev/*`, pipes,
sockets, and anonymous inodes filtered out. Their parent directories
become the "writing dirs" surfaced in the JSON output.

### Claude Code session enrichment — `~/.claude/projects/`

For each Claude process we look up `~/.claude/projects/<encoded-cwd>/` and
read the tail of the most recently modified `.jsonl` transcript. From the
last ~256 KB of records we extract:

- **`current_tool`** — the most recent assistant `tool_use` whose result hasn't returned yet
- **`current_task`** — `TodoWrite` `in_progress` item, or the latest `Task`/`Agent` `subject`, or the leading 120 chars of the most recent assistant prose
- **`subagents`** — count of `Task` / `Agent` tool uses with no matching `tool_result` (= subagents currently spawned by this session)
- **`stop_reason`** — terminal `end_turn` / `stop_sequence` flags a "completed" session

Status is then a blend of process state and session activity, so an agent
mid-generation (transcript hasn't flushed in 60s) doesn't get tagged "idle".

### Project grouping & stable sort

Agents are sorted by status priority → project → CPU% → RSS → PID, so
positions are stable tick-to-tick. The TUI clusters them under per-project
headers showing total CPU / MEM / subagents. Selection tracks the agent's
PID across refreshes.

## JSON output shape

`agtop --json` writes a single JSON object to stdout, suitable for piping
into `jq`, dashboards, or alerting:

```json
{
  "now": 1777439481861,
  "platform": "linux",
  "sys_cpus": 32,
  "mem_total": 132499206144,
  "mem_available": 46721998848,
  "aggregates": {
    "cpu": 17.2, "mem_bytes": 4257710080,
    "active": 11, "busy": 1, "waiting": 2, "completed": 5,
    "subagents": 0, "project_count": 10
  },
  "agents": [
    {
      "pid": 404872, "label": "claude", "status": "busy",
      "project": "xsol",
      "current_tool": "Bash", "current_task": "running tests",
      "subagents": 0, "session_id": "abc-123", "session_age_ms": 3200,
      "cpu": 16.3, "rss": 626491392, "uptime_sec": 345600,
      "cwd": "/home/matt/code/xsol",
      "exe": "/home/matt/.local/share/claude/versions/2.1.119",
      "cmdline": "claude --dangerously-skip-permissions",
      "writing_files": [], "writing_dirs": []
    }
  ],
  "projects": [
    { "project": "xsol", "agents": 1, "cpu": 16.3, "rss": 626491392,
      "subagents": 0, "statuses": { "busy": 1 }, "cwd": "/home/matt/code/xsol" }
  ],
  "sessions": { "active": 11, "busy": 1, "waiting": 2, "completed": 5,
                "sessions": [...], "recent_tasks": [...] },
  "activity": [ { "t": 1777439481861, "kind": "spawn",
                  "label": "claude", "pid": 384791, "cwd": "..." } ],
  "history": { "cpu": [...], "mem": [...], "active": [...], "busy": [...] }
}
```

## Repo layout

```
agtop/
├── Cargo.toml
├── src/
│   ├── main.rs            entrypoint
│   ├── cli.rs             clap CLI + --once / --json paths
│   ├── ui.rs              ratatui TUI (header / agents / charts / projects / activity)
│   ├── theme.rs           color palette + per-agent accents
│   ├── collector.rs       snapshot orchestrator + CPU smoothing
│   ├── proc_.rs           /proc parser
│   ├── claude.rs          ~/.claude/projects JSONL reader
│   ├── matchers.rs        built-in + user agent matchers
│   ├── model.rs           Snapshot / Agent / Project / Session structs
│   └── format.rs          bytes / pct / dur formatters
├── README.md
├── LICENSE
└── packages/
    ├── npm/build.sh       → agtop-<v>.tgz (postinstall fetches binary)
    ├── deb/               → agtop_<v>_<arch>.deb
    │   ├── DEBIAN/control
    │   └── build.sh
    └── pacman/            → agtop-<v>-1-<arch>.pkg.tar.zst
        ├── PKGBUILD
        └── build.sh
```

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

## Platform support

- **Linux** — full feature set (relies on `/proc`).
- **macOS / BSD** — runs but live process metrics are unavailable; falls back
  to printing the Claude Code session summary. PRs welcome for a `libproc`
  / `kvm`-based collector.

## Contributing

```sh
cargo build                # debug build, fast
cargo test                 # unit tests (matchers, format, smoke)
cargo run                  # full TUI
cargo run -- --once        # snapshot for quick iteration
cargo run -- --json | jq   # JSON for scripting
```

When adding a new built-in agent, edit `src/matchers.rs` and add a
classification test in the same file's `#[cfg(test)]` block.

## License

MIT — see `LICENSE`.
