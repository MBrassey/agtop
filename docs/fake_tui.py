#!/usr/bin/env python3
"""
Render an ANSI representation of the agtop TUI with synthetic data.  The
output is a 220-column × 56-row grid that mirrors the live ratatui layout
(rounded borders, RGB pastel palette, project-grouped agents, four right
column panels, sessions / projects / activity, footer) so the screenshot
captures publish-safe fake content without invoking the real binary.

The dataset is sophisticated blockchain-engineering work — ZK provers,
MEV searchers, restaking primitives, EIP-4844 blob pipelines, AA bundlers,
substrate runtime upgrades — i.e. the workloads that warrant a tool like
agtop in the first place.
"""

W = 220
LEFT_W = 128         # left column (Agents on top, Projects/Activity on bottom)
RIGHT_W = W - LEFT_W # right column (CPU / Memory / Tokens / Status / Sessions)

# ── palette (matches src/theme.rs) ─────────────────────────────────────────
BORDER  = "\x1b[38;2;125;150;165m"
BORDER_DIM = "\x1b[38;2;70;85;95m"
FG      = "\x1b[38;2;225;222;215m"
FG_DIM  = "\x1b[38;2;140;140;140m"
BUSY    = "\x1b[1m\x1b[38;2;150;210;165m"
SPAWN   = "\x1b[1m\x1b[38;2;165;215;210m"
ACTIVE  = "\x1b[38;2;160;200;150m"
IDLE    = "\x1b[2m\x1b[38;2;155;155;155m"
WAIT    = "\x1b[38;2;225;195;140m"
DONE    = "\x1b[38;2;200;175;215m"
CHART_CPU = "\x1b[38;2;225;195;140m"
CHART_MEM = "\x1b[38;2;200;175;215m"
CHART_TOK = "\x1b[38;2;180;200;215m"
GAUGE_USED  = "\x1b[38;2;160;200;150m"
GAUGE_AGENT = "\x1b[38;2;225;195;140m"
GAUGE_FREE  = "\x1b[38;2;60;65;75m"
ACCENT_BLUE = "\x1b[38;2;160;180;215m"   # claude
ACCENT_GREEN= "\x1b[38;2;160;200;150m"   # codex / mods
ACCENT_ROSE = "\x1b[38;2;220;160;155m"   # aider
ACCENT_LAV  = "\x1b[38;2;200;170;210m"   # cursor-agent / cody
ACCENT_TEAL = "\x1b[38;2;165;215;210m"   # gemini / copilot / llm
ACCENT_PEACH= "\x1b[38;2;225;195;140m"   # goose / ollama / amp
BOLD = "\x1b[1m"
RST = "\x1b[0m"

def acc(label):
    return {
        "claude":      ACCENT_BLUE,
        "codex":       ACCENT_GREEN,
        "aider":       ACCENT_ROSE,
        "cursor-agent":ACCENT_LAV,
        "gemini":      ACCENT_TEAL,
        "goose":       ACCENT_PEACH,
        "ollama":      ACCENT_PEACH,
        "mods":        ACCENT_GREEN,
    }.get(label, FG)

def cpu_color(v):
    if v >= 50: return "\x1b[38;2;220;160;155m"
    if v >= 10: return CHART_CPU
    if v >= 1:  return ACTIVE
    return FG_DIM

# ── visible-width helpers (ANSI escapes are zero-width) ────────────────────
import re
ANSI = re.compile(r"\x1b\[[0-9;]*m")
def vlen(s): return len(ANSI.sub("", s))
def pad(s, w):
    return s + " " * max(0, w - vlen(s))
def lpad(s, w):
    return " " * max(0, w - vlen(s)) + s
def trunc(s, w):
    if vlen(s) <= w: return s
    out, count = [], 0
    i = 0
    while i < len(s) and count < w - 1:
        m = ANSI.match(s, i)
        if m:
            out.append(m.group()); i = m.end()
        else:
            out.append(s[i]); count += 1; i += 1
    out.append("…")
    return "".join(out)

# ── synthetic dataset ──────────────────────────────────────────────────────
agents = [
    # status, label, pid, cpu, mem_mb, uptime_sec, sub, tok_str, project, model, doing, dangerous
    ("busy",     "claude",       28471, 31.4, 642, 11820, 2, "9.2M", "zk-rollup-prover",  "claude-sonnet-4-7", "Bash: nargo prove --witness witness.tr"),
    ("busy",     "claude",       28473, 24.6, 538, 11820, 0, "8.4M", "zk-rollup-prover",  "claude-sonnet-4-7", "Edit: circuits/poseidon_t8.circom"),
    ("active",   "codex",        28480, 11.8, 220,  9660, 0, "3.1M", "zk-rollup-prover",  "gpt-5",             "Task: optimize MSM precomputation kernel"),
    ("busy",     "codex",        31802, 19.2, 412,  2832, 0, "4.1M", "mev-searcher",      "gpt-5",             "Edit: src/searcher/atomic_arb_v3.rs"),
    ("spawning", "claude",       19432,  7.8, 521, 13440, 1, "6.8M", "eigen-restake",     "claude-opus-4-7",   "Task: prove transcript Fiat-Shamir soundness"),
    ("active",   "aider",        24190,  4.2, 478,  4080, 0, "2.4M", "amm-v4-hooks",      "claude-sonnet-4-7", "applying SEARCH/REPLACE: contracts/HookV4.sol"),
    ("active",   "claude",       33561,  3.1, 412,  1102, 0, "3.7M", "kzg-blob-pipe",     "claude-sonnet-4-7", "Write: src/blob_tx_simulator.rs"),
    ("active",   "gemini",       22817,  2.0, 264, 18660, 0, "1.4M", "erc4337-bundler",   "gemini-2.0-flash",  "analysing UserOperation paymaster validation"),
    ("idle",     "claude",       14002,  0.0, 412,345600, 0, "5.9M", "cosmos-ibc-relay",  "claude-sonnet-4-7", "(idle 12m08s)"),
    ("idle",     "claude",       17293,  0.0, 389,237000, 0, "2.1M", "polygon-cdk-prov",  "claude-opus-4-7",   "(idle 47m22s)"),
    ("idle",     "goose",        28773,  0.0, 182, 23580, 0, "560k", "substrate-runtm",   "gpt-4o",            "applied 3 patches to pallet_eigenlayer"),
    ("idle",     "cursor-agent", 31102,  0.0, 287, 12120, 0, "320k", "cosmwasm-staking",  None,                "watching contracts/ for changes"),
    ("idle",     "ollama",       18234,  0.0,  16,  792000,0, "",     "ollama serve",     None,                "/usr/local/bin/ollama serve"),
    ("idle",     "claude",       29113,  0.0, 245, 42420, 0, "3.4M", "halo2-circuits",    "claude-sonnet-4-7", "(idle 2h17m)"),
    ("idle",     "ollama",       22091,  0.0,  14, 792000,0, "",     "ollama serve",     None,                "/bin/ollama serve"),
    ("idle",     "claude",       38221,  0.0, 198, 100800,0, "740k", "bridge-watchtower", "claude-opus-4-7",   "claude --resume"),
    ("waiting",  "claude",       12041,  0.0, 0,    7200, 0, "1.8M", "rust-stylus",       "claude-sonnet-4-7", "(awaiting input)"),
]

projects_set = {a[8] for a in agents}
project_count = len(projects_set)
busy_n  = sum(1 for a in agents if a[0] in ("busy","spawning"))
subs    = sum(a[6] for a in agents)
total_cpu = round(sum(a[3] for a in agents), 1)
total_mem_mb = sum(a[4] for a in agents)
total_tok_str = "38.7M"
cost_str = "$612.40"
sys_mem_used_g  = 12.4
sys_mem_total_g = 64.0

# ── status badge + decor ───────────────────────────────────────────────────
STATUS_GLYPH = {"busy":"●","spawning":"◆","active":"●","idle":"○","waiting":"◌","completed":"✓","stale":"·"}
STATUS_LABEL = {"busy":"BUSY","spawning":"SPWN","active":"ACTV","idle":"idle","waiting":"WAIT","completed":"DONE","stale":"stale"}
STATUS_COLOR = {"busy":BUSY,"spawning":SPAWN,"active":ACTIVE,"idle":IDLE,"waiting":WAIT,"completed":DONE,"stale":FG_DIM}

def fmt_dur(sec):
    if sec < 60: return f"{sec}s"
    if sec < 3600: return f"{sec//60}m{sec%60:02}s"
    if sec < 86400: return f"{sec//3600}h{(sec%3600)//60:02}m"
    return f"{sec//86400}d{(sec%86400)//3600:02}h"
def fmt_mem(mb):
    if mb >= 1024: return f"{mb/1024:.1f}G"
    return f"{mb:.0f}M" if mb >= 100 else f"{mb:.1f}M"

# ── 8-cell sparkline ──────────────────────────────────────────────────────
BLOCKS = " ▁▂▃▄▅▆▇█"
def spark(values, max_v, w):
    if not values: return " " * w
    step = max(1.0, len(values) / w)
    out = []
    for i in range(w):
        a = int(i*step); b = min(len(values), int((i+1)*step))
        avg = sum(values[a:b])/max(1, b-a) if a < b else 0
        idx = min(len(BLOCKS)-1, int((avg / max(max_v, 1)) * (len(BLOCKS)-1) + 0.5))
        out.append(BLOCKS[idx])
    return "".join(out)

# Synthetic CPU history (system aggregate) — realistic burst pattern.
cpu_hist = [3,5,9,18,32,48,67,82,84,79,68,55,44,38,42,55,69,76,71,62,55,48,52,61,72,81,78,68]
# Synthetic per-pid CPU history — approximates the bar in the agent row.
def per_agent_spark(cpu_now):
    if cpu_now == 0:    return spark([0]*8, 100, 8)
    if cpu_now < 5:     return spark([0,0,1,1,2,1,1,2], 100, 8)
    if cpu_now < 15:    return spark([2,3,4,8,12,10,8,11], 100, 8)
    if cpu_now < 25:    return spark([5,12,18,22,24,18,20,24], 100, 8)
    return spark([18,24,28,30,32,29,33,31], 100, 8)

# ── component renderers ───────────────────────────────────────────────────
def title_chips_row(width):
    chips = [
        (BUSY,  f" {sum(1 for a in agents if a[0] in ('busy','spawning','active','idle'))} "), (FG_DIM, "active "),
        (BUSY,  f" {busy_n} "), (FG_DIM, "busy "),
        (SPAWN, f" {subs} "), (FG_DIM, "subagents "),
        (WAIT,  " 4 "), (FG_DIM, "waiting "),
        (DONE,  " 8 "), (FG_DIM, "done "),
        (BOLD+FG, f" {project_count} "), (FG_DIM, "projects "),
        (CHART_CPU, f" {total_cpu:.1f}% "), (FG_DIM, "cpu "),
        (CHART_MEM, f" {sys_mem_used_g:.1f}G/{sys_mem_total_g:.0f}G "), (FG_DIM, "mem "),
        (CHART_TOK, f" {total_tok_str} "), (FG_DIM, "tokens "),
        (CHART_CPU, f" {cost_str} "), (FG_DIM, "cost "),
        (FG_DIM, " sort:smart  group:on  "),
    ]
    s = f"{BOLD}{BORDER} agtop {RST}{FG_DIM}v2.0.0  {RST}"
    for col, txt in chips:
        s += f"{col}{txt}{RST}"
    return s

def panel(title, body_lines, width, header_extra=""):
    """Rounded-border panel.  body_lines are pre-formatted, no border."""
    out = []
    title_str = f"{BOLD}{FG} {title} {RST}"
    if header_extra:
        title_str += f"{FG_DIM}{header_extra}{RST}"
    bar = pad(title_str, width - 2)
    out.append(f"{BORDER}╭ {RST}{bar}{BORDER} ╮{RST}"[: 200000])
    inner_w = width - 2
    for ln in body_lines:
        out.append(f"{BORDER}│{RST}" + pad(ln, inner_w) + f"{BORDER}│{RST}")
    out.append(f"{BORDER}╰{'─'*(width-2)}╯{RST}")
    return out

# Agents panel body.
def agents_body(width):
    inner = width - 4
    by_proj = {}
    for a in agents:
        by_proj.setdefault(a[8], []).append(a)
    rank = {"busy":0,"spawning":1,"active":2,"idle":3,"waiting":4,"completed":5,"stale":6}
    proj_order = sorted(by_proj.keys(), key=lambda p: (rank[by_proj[p][0][0]], p))
    out = []
    for proj in proj_order:
        rows = by_proj[proj]
        cpu_sum = sum(r[3] for r in rows); mem_sum = sum(r[4] for r in rows)
        sub_sum = sum(r[6] for r in rows)
        tok_sum_label = next((r[7] for r in rows if r[7]), "")
        header = (
            f" {BORDER}{BOLD}◆ {proj}{RST}"
            f"{FG_DIM}  {len(rows)} agent{'s' if len(rows)>1 else ''} · "
            f"{cpu_sum:.1f}% cpu · {fmt_mem(mem_sum)} mem"
            f"{RST}"
        )
        if sub_sum > 0:
            header += f"{SPAWN}  +{sub_sum}{RST}{FG_DIM} sub{RST}"
        out.append(header)
        for r in rows:
            status, label, pid, cpu, mem, up, sub, tok, _, model, doing = r
            # Heuristic: any agent with status=busy in the synthetic dataset
            # is treated as god-mode (so the screenshot shows the pulsating
            # GOD tag); real binary uses cmdline regex.
            dangerous = (status == "busy" and label == "claude")
            badge = f"{STATUS_COLOR[status]}{STATUS_GLYPH[status]} {STATUS_LABEL[status]}{RST}"
            cpu_str = f"{cpu_color(cpu)}{cpu:>5.1f}%{RST}"
            cpu_bar_n = max(0, min(6, int(cpu/100*6 + 0.5)))
            cpu_bar = f"{cpu_color(cpu)}{'█'*cpu_bar_n}{RST}{BORDER_DIM}{'·'*(6-cpu_bar_n)}{RST}"
            mem_str = f"{CHART_MEM}{fmt_mem(mem):>6}{RST}"
            up_str  = f"{FG_DIM}{up:>7}{RST}"  # not used — we use sec value directly
            up_str  = f"{FG_DIM}{fmt_dur(up):>7}{RST}"
            sub_str = f"{SPAWN}+{sub}{RST}{FG_DIM} sub{RST}" if sub > 0 else "       "
            tok_str = f"{CHART_TOK}{tok}{RST}" if tok else ""
            sp = f"{cpu_color(cpu)}{per_agent_spark(cpu)}{RST}"
            doing_col = SPAWN if any(t in doing for t in ("Bash:","Edit:","Task:","Write:")) else FG
            doing_clipped = trunc(doing, max(0, inner - 78))
            # Pulsating GOD tag for dangerous-mode agents.
            god_tag = ""
            label_padded = f"{label:<12}"
            if dangerous:
                god_tag = "\x1b[5m\x1b[7m\x1b[1m\x1b[31m GOD \x1b[0m "
                label_padded = f"{label:<7}"
            line = (
                f"   {badge}  "
                f"{god_tag}{acc(label)}{BOLD}{label_padded}{RST} "
                f"{FG_DIM}pid{RST}{FG}{pid:>7}{RST} "
                f"{cpu_str} {cpu_bar} "
                f"{mem_str} "
                f"{up_str} "
                f"{sub_str} "
                f"{tok_str:<5} "
                f"{sp}  "
                f"{doing_col}{doing_clipped}{RST}"
            )
            out.append(line)
            if sub > 0:
                out.append(f"           {FG_DIM}└ +{sub} sub: zk-circuit-reviewer, gas-optimizer{RST}")
        out.append("")
    return out

# CPU panel body.
def cpu_body(width):
    inner = width - 4
    out = []
    sl = spark(cpu_hist, 100, inner)
    out.append(f"{CHART_CPU}{sl}{RST}")
    out.append("")
    by_cpu = sorted([a for a in agents if a[3] > 0], key=lambda a: -a[3])
    bar_w = inner - 36
    max_cpu = by_cpu[0][3] if by_cpu else 1
    for r in by_cpu[:6]:
        status, label, pid, cpu, *_ , project, model, doing = r
        frac = cpu / max(1, max_cpu)
        filled = int(frac * bar_w + 0.5)
        bar = f"{cpu_color(cpu)}{'█'*filled}{RST}{BORDER_DIM}{'·'*(bar_w-filled)}{RST}"
        out.append(
            f" {STATUS_COLOR[status]}{STATUS_GLYPH[status]}{RST} "
            f"{FG}{project[:14]:<14}{RST} "
            f"{acc(label)}{label:<12}{RST} "
            f"{bar} "
            f"{cpu_color(cpu)}{BOLD}{cpu:>5.1f}%{RST}"
        )
    return out

def mem_body(width):
    inner = width - 4
    out = []
    by_mem = sorted([a for a in agents if a[4] > 0], key=lambda a: -a[4])
    bar_w = inner - 38
    max_mem = by_mem[0][4]
    for r in by_mem[:6]:
        status, label, pid, cpu, mem, *_ , project, model, doing = r
        frac = mem / max_mem
        filled = int(frac * bar_w + 0.5)
        bar = f"{CHART_MEM}{BOLD}{'█'*filled}{RST}{BORDER_DIM}{'·'*(bar_w-filled)}{RST}"
        out.append(
            f" {STATUS_COLOR[status]}{STATUS_GLYPH[status]}{RST} "
            f"{FG}{project[:14]:<14}{RST} "
            f"{acc(label)}{label:<12}{RST} "
            f"{bar} "
            f"{CHART_MEM}{fmt_mem(mem):>6}{RST}"
        )
    # System memory gauge.
    out.append("")
    cells = inner - 2
    agent_cells = int(cells * 0.06 + 0.5)
    other_cells = int(cells * 0.13 + 0.5)
    free_cells  = cells - agent_cells - other_cells
    out.append(
        f" {GAUGE_AGENT}{'█'*agent_cells}{GAUGE_USED}{'█'*other_cells}{GAUGE_FREE}{'░'*free_cells}{RST}"
    )
    out.append(
        f" {FG_DIM}agents{RST} {GAUGE_AGENT}{BOLD}4.8G{RST}"
        f" {FG_DIM} other{RST} {GAUGE_USED}{BOLD}7.6G{RST}"
        f" {FG_DIM} free{RST} {BOLD}51.6G{RST}"
        f" {FG_DIM}/ 64G{RST}"
    )
    return out

def tok_body(width):
    inner = width - 4
    out = []
    tok_hist = [400, 1200, 800, 3200, 6400, 4800, 2200, 1800, 7200, 9100, 6400, 5200, 3800, 4400, 6100, 8200, 7400, 5800]
    sl = spark(tok_hist, max(tok_hist), inner)
    out.append(f"{CHART_TOK}{sl}{RST}")
    out.append("")
    tok_rank = [a for a in agents if a[7]]
    def parse_tok(s):
        s = s.strip()
        if s.endswith("M"): return float(s[:-1])*1_000_000
        if s.endswith("k"): return float(s[:-1])*1_000
        return float(s)
    tok_rank.sort(key=lambda a: -parse_tok(a[7]))
    bar_w = inner - 36
    max_tok = parse_tok(tok_rank[0][7]) if tok_rank else 1
    for r in tok_rank[:6]:
        status, label, pid, cpu, mem, *_ , project, model, doing = r
        tok = r[7]
        frac = parse_tok(tok) / max_tok
        filled = int(frac * bar_w + 0.5)
        bar = f"{CHART_TOK}{BOLD}{'█'*filled}{RST}{BORDER_DIM}{'·'*(bar_w-filled)}{RST}"
        out.append(
            f" {STATUS_COLOR[status]}{STATUS_GLYPH[status]}{RST} "
            f"{FG}{project[:14]:<14}{RST} "
            f"{acc(label)}{label:<12}{RST} "
            f"{bar} "
            f"{CHART_TOK}{BOLD}{tok:>6}{RST}"
        )
    return out

def status_dist_body(width):
    inner = width - 4
    counts = {"busy":busy_n - sum(1 for a in agents if a[0]=="spawning"),
              "spawning":sum(1 for a in agents if a[0]=="spawning"),
              "active": sum(1 for a in agents if a[0]=="active"),
              "idle":   sum(1 for a in agents if a[0]=="idle"),
              "waiting":sum(1 for a in agents if a[0]=="waiting"),
              "completed": 0}
    total = sum(counts.values())
    bar_w = inner - 32
    out = []
    for st in ("busy","spawning","active","idle","waiting","completed"):
        c = counts[st]; pct = (c/total*100) if total else 0
        filled = int(pct/100 * bar_w + 0.5)
        out.append(
            f"   {STATUS_COLOR[st]}{STATUS_GLYPH[st]} {STATUS_LABEL[st][:5]:<5}{RST} "
            f"{BOLD}{FG}{c:>3}{RST} "
            f"{STATUS_COLOR[st]}{BOLD}{'█'*filled}{RST}{BORDER_DIM}{'·'*(bar_w-filled)}{RST} "
            f"{FG_DIM}{pct:>4.1f}%{RST}"
        )
    return out

def sessions_body(width):
    inner = width - 4
    out = [
        f" {SPAWN}{BOLD} {subs} {RST}{FG_DIM}Task subagents in flight{RST}",
        "",
        f" {BOLD}{FG}Recent tasks{RST}",
    ]
    recents = [
        ("busy",     "zk-rollup-prover",  "Bash: nargo prove --witness witness.tr"),
        ("busy",     "mev-searcher",      "Edit: src/searcher/atomic_arb_v3.rs"),
        ("spawning", "eigen-restake",     "Task: prove transcript Fiat-Shamir soundness"),
        ("active",   "amm-v4-hooks",      "applying SEARCH/REPLACE: contracts/HookV4.sol"),
        ("active",   "kzg-blob-pipe",     "Write: src/blob_tx_simulator.rs (EIP-4844)"),
        ("active",   "erc4337-bundler",   "analysing UserOperation paymaster validation"),
    ]
    for st, proj, task in recents:
        out.append(
            f"  {STATUS_COLOR[st]}{STATUS_GLYPH[st]}{RST} "
            f"{BOLD}{FG}{proj:<16}{RST}  "
            f"{FG_DIM}{trunc(task, max(0, inner - 22))}{RST}"
        )
    return out

def projects_body(width):
    inner = width - 4
    rows = [
        ("busy",     "zk-rollup-prover", 3, 67.8, "9.2M"),
        ("busy",     "mev-searcher",     1, 19.2, "4.1M"),
        ("spawning", "eigen-restake",    1,  7.8, "6.8M"),
        ("active",   "amm-v4-hooks",     1,  4.2, "2.4M"),
        ("active",   "kzg-blob-pipe",    1,  3.1, "3.7M"),
        ("active",   "erc4337-bundler",  1,  2.0, "1.4M"),
        ("idle",     "cosmos-ibc-relay", 1,  0.0, "5.9M"),
        ("idle",     "halo2-circuits",   1,  0.0, "3.4M"),
    ]
    out = [""]
    bar_w = 12
    max_cpu = max(r[3] for r in rows) or 1
    for st, proj, n, cpu, tok in rows:
        frac = cpu / max_cpu
        filled = int(frac * bar_w + 0.5)
        bar = f"{cpu_color(cpu)}{'█'*filled}{BORDER_DIM}{'·'*(bar_w-filled)}{RST}"
        out.append(
            f"  {STATUS_COLOR[st]}{STATUS_GLYPH[st]}{RST} "
            f"{FG}{BOLD}{proj[:18]:<18}{RST} "
            f"{FG_DIM}{n:>2}{RST} "
            f"{cpu_color(cpu)}{cpu:>5.1f}%{RST} "
            f"{bar} "
            f"{CHART_TOK}{tok:>5}{RST}"
        )
    return out

def activity_body(width):
    rows = [
        ("23:14:57", "spawn", "claude",       28471, "zk-rollup-prover"),
        ("23:14:32", "spawn", "claude",       28473, "zk-rollup-prover"),
        ("23:13:22", "exit",  "codex",        93217, ""),
        ("23:11:08", "spawn", "codex",        31802, "mev-searcher"),
        ("23:09:44", "spawn", "claude",       19432, "eigen-restake"),
        ("22:58:11", "spawn", "aider",        24190, "amm-v4-hooks"),
        ("22:42:33", "spawn", "claude",       33561, "kzg-blob-pipe"),
        ("22:14:02", "exit",  "ollama",       72844, ""),
        ("21:55:18", "spawn", "gemini",       22817, "erc4337-bundler"),
    ]
    out = []
    for t, kind, label, pid, cwd in rows:
        glyph = f"{BUSY}●{RST}" if kind == "spawn" else f"{FG_DIM}◌{RST}"
        kind_s = f"{FG_DIM}{kind:<5}{RST}"
        out.append(
            f"  {FG_DIM}{t}{RST}  {glyph} {kind_s}  "
            f"{acc(label)}{label:<12}{RST} "
            f"{FG_DIM}pid{RST} {pid:<7}"
            + (f"  {FG}{cwd}{RST}" if cwd else "")
        )
    return out

# ── lay out the screen ────────────────────────────────────────────────────
def render():
    rows = []
    # Header (3 rows): top border + chips line + bottom border.
    chips = title_chips_row(W)
    rows.append(f"{BORDER}╭{'─'*(W-2)}╮{RST}")
    rows.append(f"{BORDER}│{RST}" + pad(chips, W-2) + f"{BORDER}│{RST}")
    rows.append(f"{BORDER}├{'─'*(W-2)}┤{RST}")

    # Body: left and right columns rendered side-by-side row-by-row.
    agents_p   = panel("Agents (project-grouped)", agents_body(LEFT_W), LEFT_W)
    cpu_p      = panel("CPU", cpu_body(RIGHT_W), RIGHT_W,
                       header_extra="32 cores · now 81.8% · peak 96.0% · avg 47.3% ")
    mem_p      = panel("Memory by agent", mem_body(RIGHT_W), RIGHT_W,
                       header_extra="4.8G across 17 agents ")
    tok_p      = panel("Tokens", tok_body(RIGHT_W), RIGHT_W,
                       header_extra="total 38.7M  rate 142k/min ")
    sd_p       = panel("Status distribution", status_dist_body(RIGHT_W), RIGHT_W,
                       header_extra="17 live agents ")
    sess_p     = panel("Claude sessions — recent tasks", sessions_body(RIGHT_W), RIGHT_W)

    right_col = cpu_p + mem_p + tok_p + sd_p + sess_p

    # Bottom-left split: Projects | Activity
    proj_p = panel("Projects", projects_body(LEFT_W // 2), LEFT_W // 2)
    act_p  = panel("Activity", activity_body(LEFT_W - LEFT_W // 2), LEFT_W - LEFT_W // 2)

    # Stack agents_p with proj+act below to fill the same vertical extent as right_col.
    agents_h_target = len(right_col) - len(proj_p)
    while len(agents_p) < agents_h_target:
        agents_p.append(f"{BORDER}│{RST}" + " "*(LEFT_W-2) + f"{BORDER}│{RST}")
    if len(agents_p) > agents_h_target:
        # Truncate but keep the bottom border line.
        agents_p = agents_p[:agents_h_target-1] + [agents_p[-1]]

    # Compose left column = agents_p + (proj_p alongside act_p).
    left_col = agents_p + [a + b for a, b in zip(proj_p, act_p)]

    # Pair left + right rows.  If lengths differ, pad with blanks.
    while len(left_col) < len(right_col):
        left_col.append(" " * LEFT_W)
    while len(right_col) < len(left_col):
        right_col.append(" " * RIGHT_W)
    for l, r in zip(left_col, right_col):
        rows.append(pad(l, LEFT_W) + pad(r, RIGHT_W))

    # Footer.
    rows.append(
        f"{FG_DIM}  q quit · ? help · s sort(smart) · g group(on) · / filter · "
        f"p pause · r refresh · ↑↓ select · Enter detail{RST}"
    )
    return "\n".join(rows)

if __name__ == "__main__":
    print(render())
