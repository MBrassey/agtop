# agtop

> Like `top`, but for AI coding agents.

`agtop` is a terminal UI that surfaces every AI coding agent running on your
system in one place: live PIDs, CPU%, RSS, working directory, files being
written, uptime, sparkline history, and recent activity — plus a panel for
Claude Code session state (active / waiting / completed) and the most recent
task subjects.

It detects the major agent CLIs out of the box — **Claude Code**, **OpenAI
Codex**, **Aider**, **Cursor Agent**, **Gemini CLI**, **Goose**, **Continue**,
**Opencode**, **GitHub Copilot CLI**, **Cody**, **Sourcegraph Amp**, **Crush**,
**Mods**, **sgpt**, **llm**, **Ollama**, **Fabric** — and you can teach it
about anything else with a one-line regex.

```
 agtop v0.1.0   active:11  waiting:4  completed:1  cpu:32.8%  mem:3.9G   sort:CPU
┌ Agents ────────────────────────────────────────────────┬ CPU% (sum) ────────────┐
│ PID    AGENT       CPU%   MEM  UPTIME  CWD       CMD   │  ╱╲    ╱╲     ╱╲       │
│ 404872 claude     16.4%  626M  4d0h    ~/code/xsol …   │ ╱  ╲__╱  ╲___╱  ╲___   │
│ 384791 claude     16.4%  414M  16m42s  ~/code/agtop …  ├ MEM (MB, sum) ─────────┤
│ 413598 claude      0.0%  466M  18h37m  …blueprint…    │  ▁▂▂▃▃▃▄▄▄▄▄▄▄         │
│ 417638 claude      0.0%  425M  8h47m   ~/code/marin… │                         │
│ ...                                                    ├ Active agents ────────┤
│                                                        │ ▁▂▃▄▅▆▆▇▇█████        │
├ Recent activity ───────────────────────────────────────┼ Claude sessions ──────┤
│ 23:14:57  spawn  claude       pid=384791 ~/code/agtop │ Active    11           │
│ 23:13:22  exit   codex        pid=98271              │ Waiting   4            │
│ 23:12:01  spawn  aider        pid=4421  ~/code/foo   │ Completed 1            │
└ Selected: writing paths · cmdline ─────────────────────┴───────────────────────┘
   claude  pid=384791  threads=14  state=S  uptime=16m42s
   exe : /home/matt/.local/share/claude/versions/2.1.121
   cwd : ~/code/agtop
   cmd : claude --dangerously-skip-permissions
```

## Install

### npm (any OS with Node ≥ 16)

```sh
npm install -g agtop
```

### Arch Linux / CachyOS / Manjaro

```sh
sudo pacman -U agtop-0.1.0-1-any.pkg.tar.zst
```

(Build the `.pkg.tar.zst` yourself with `packages/pacman/build.sh`, or install
from the AUR once published.)

### Debian / Ubuntu

```sh
sudo apt install ./agtop_0.1.0_all.deb
```

(Build the `.deb` yourself with `packages/deb/build.sh`, or install from the
PPA once published.)

### From source

```sh
git clone https://github.com/mbrassey/agtop.git
cd agtop
npm install
node bin/agtop
```

## Usage

```
agtop                       # full TUI
agtop --once                # one-shot snapshot, like `top -b -n 1`
agtop -1 --top 10           # top-10 active agents and exit
agtop --json | jq           # machine-readable JSON for scripting
agtop --interval 0.5        # half-second refresh
agtop --filter aider        # only show agents whose label/cmd/cwd matches
agtop --sort mem            # sort by RSS instead of CPU
agtop --list-builtins       # print built-in agent matcher list
agtop -m "myagent=python.*my_agent\.py"  # add custom matcher
```

### All flags

| Flag                          | Description                                          |
| ----------------------------- | ---------------------------------------------------- |
| `-V`, `--version`             | Print version and exit                               |
| `-h`, `--help`                | Print help and exit                                  |
| `-1`, `--once`                | Print a one-shot snapshot and exit (no TUI)          |
| `-j`, `--json`                | Machine-readable JSON; implies `--once`              |
| `-i`, `--interval <seconds>`  | Refresh interval (default `1.5`)                     |
| `-n`, `--iterations <count>`  | With `--once`, print N snapshots delimited by `---`  |
| `-f`, `--filter <substr>`     | Only show agents matching label / cmd / cwd          |
| `-s`, `--sort <key>`          | `cpu` \| `mem` \| `uptime` \| `pid` \| `agent`       |
| `-m`, `--match <label=regex>` | Add a custom agent matcher (repeatable)              |
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
| `s`            | Cycle sort (cpu → mem → uptime → pid → agent)     |
| `f`            | Filter (Esc to clear)                             |
| `c`            | Toggle completed-sessions view                    |
| `j` / `k`, ↓/↑ | Move selection in the agent table                 |

### Environment

| Variable      | Effect                                                          |
| ------------- | --------------------------------------------------------------- |
| `AGTOP_MATCH` | Semicolon-separated `label=regex` matchers, additive to builtins |

## What counts as an "agent"?

Any process whose command line matches one of the built-in patterns. Run
`agtop --list-builtins` to see the current set. The patterns target the
common installation shapes — both bare-binary invocations
(`/usr/bin/claude`, `/usr/local/bin/aider`) and module-style invocations
(`python -m aider`, `node /opt/codex/bin/codex`).

To add your own:

```sh
agtop -m "internal-bot=python.*src/agent\.py" \
      -m "rag-worker=node.*workers/rag\.js"
```

…or set `AGTOP_MATCH` in your shell profile so it's always on:

```sh
export AGTOP_MATCH="internal-bot=python.*src/agent\.py;rag-worker=node.*workers/rag\.js"
```

## What does each column mean?

| Column     | Source                                                                        |
| ---------- | ----------------------------------------------------------------------------- |
| `PID`      | `/proc/<pid>/stat` field 1                                                    |
| `AGENT`    | label of the matcher that matched                                             |
| `CPU%`     | (Δutime+Δstime) / Δsystem-cpu × N-cores × 100, top-style                      |
| `MEM`      | RSS (`stat` field 24 × page size)                                             |
| `UPTIME`   | wall-clock since process start (boot-time + `starttime`)                      |
| `CWD`      | `/proc/<pid>/cwd` symlink                                                     |
| `TASK/CMD` | `/proc/<pid>/cmdline`, NUL-joined and elided                                  |

The "Selected" panel additionally surfaces:

- `exe`: `/proc/<pid>/exe` symlink
- `io`: cumulative `read_bytes` / `write_bytes` from `/proc/<pid>/io`
- `writing`: open file descriptors with `O_WRONLY` / `O_RDWR` flags (filtered
  to ignore `/dev/*`, pipes, sockets, anonymous inodes)
- `dirs`: deduped directories of the writing files

## Claude Code sessions

`agtop` reads `~/.claude/projects/*/` to surface session state (this is the
data Claude Code itself writes for resume / `/sessions`). Each session is
classified as:

- **active** — there's a live `claude` process whose `cwd` matches the project
- **waiting** — recent activity (≤ 24h) but no live process
- **completed** — last transcript line had `stop_reason` `end_turn` or
  `stop_sequence`
- **idle** — neither

The "Recent tasks" panel lists the last task subject from each recently active
session, drawn from `toolUseResult.subject` / `tool_use.input.subject` /
the leading prefix of the last assistant message — best-effort across schema
versions.

## JSON output shape

`agtop --json` writes a single JSON object to stdout suitable for piping
into `jq`, dashboards, or alerting:

```json
{
  "now": 1777439481861,
  "platform": "linux",
  "sysCpus": 32,
  "memTotal": 132499206144,
  "memAvailable": 46721998848,
  "aggregates": { "cpu": 32.8, "memBytes": 4201226240,
                  "active": 11, "waiting": 4, "completed": 1 },
  "agents": [
    { "pid": 404872, "label": "claude", "cpu": 16.4, "rss": 626491392,
      "threads": 14, "state": "S", "ppid": 1, "uptimeSec": 345600,
      "cwd": "/home/matt/code/xsol",
      "exe": "/home/matt/.local/share/claude/versions/2.1.119",
      "cmdline": "claude --dangerously-skip-permissions",
      "readBytes": 1440100352, "writeBytes": 19001344,
      "writingFiles": [], "writingDirs": [] }
  ],
  "sessions": { "active": 11, "waiting": 4, "completed": 1,
                "sessions": [...], "recentTasks": [...] },
  "activity": [ { "t": 1777439481861, "kind": "spawn",
                  "label": "claude", "pid": 384791, "cwd": "..." } ],
  "history": { "cpu": [...], "mem": [...], "active": [...] }
}
```

## Repo layout

```
agtop/
├── bin/agtop              # node entrypoint shim (npm `bin`)
├── src/
│   ├── cli.js             # commander CLI + --once / --json paths
│   ├── ui.js              # blessed + blessed-contrib TUI
│   ├── collector.js       # snapshot orchestrator + CPU% deltas
│   ├── proc.js            # /proc parser
│   ├── claude-sessions.js # ~/.claude/projects reader
│   ├── agents.js          # built-in agent matcher list
│   └── format.js          # bytes / pct / dur / table formatters
├── test/smoke.js          # `npm test` — exercises the library, not the TUI
├── package.json
├── packages/
│   ├── npm/build.sh       # → agtop-<v>.tgz (npm pack)
│   ├── deb/               # → agtop_<v>_all.deb (dpkg-deb or pure ar+tar)
│   │   ├── DEBIAN/control
│   │   └── build.sh
│   └── pacman/            # → agtop-<v>-1-any.pkg.tar.zst (makepkg)
│       ├── PKGBUILD
│       └── build.sh
├── README.md
└── LICENSE
```

## Building the packages

```sh
# npm tarball (publish-ready)
packages/npm/build.sh

# Debian/Ubuntu .deb (no dpkg-deb required — falls back to ar+tar)
packages/deb/build.sh

# Arch .pkg.tar.zst
packages/pacman/build.sh
```

Each subfolder has its own README with submission notes for npm registry,
Debian PPAs, and the AUR.

## Platform support

- **Linux** — full feature set (relies on `/proc`).
- **macOS / others** — `agtop` runs but live process metrics are unavailable;
  it falls back to printing the Claude Code session summary. PRs welcome for
  a `libproc`-based collector.

## Contributing

1. `npm install`
2. `npm test` — runs the smoke suite (no TUI required)
3. `node bin/agtop` — full TUI
4. `node bin/agtop --once` — snapshot for quick iteration

When adding a new built-in agent, edit `src/agents.js` and add a smoke-test
case in `test/smoke.js` so future regex tweaks don't silently break it.

## License

MIT — see `LICENSE`.
