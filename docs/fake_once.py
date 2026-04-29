#!/usr/bin/env python3
"""
Emit ANSI output bit-for-bit identical to `agtop --once --top N` (per the
format-string in src/cli.rs::print_snapshot), but with synthetic agent rows
for the README screenshot.  No personal session content.
"""

RESET = "\x1b[0m"
BOLD  = "\x1b[1m"; UNBOLD = "\x1b[22m"
GRN   = "\x1b[32m"; YEL = "\x1b[33m"; CYN = "\x1b[36m"; MAG = "\x1b[35m"; DIM = "\x1b[2m"

STATUS_DECOR = {
    "busy":      ("● BUSY ", BOLD + GRN),
    "spawning":  ("◆ SPWN ", BOLD + CYN),
    "active":    ("● ACTV ", GRN),
    "idle":      ("○ idle ", DIM),
    "waiting":   ("◌ WAIT ", YEL),
    "completed": ("✓ DONE ", MAG),
}

def status_badge(s):
    glyph, color = STATUS_DECOR[s]
    return f"{color}{glyph}{RESET}"

def sub_chip(n):
    return f"{CYN}{('+' + str(n)):>4}{RESET}" if n > 0 else f"{'-':>4}"

def tok_chip(s):
    return f"{CYN}{s:>5}{RESET}" if s else f"{'-':>5}"

def row(status, agent, pid, cpu, mem, up, sub, tok, project, doing):
    visible_status = STATUS_DECOR[status][0]
    badge_padded = status_badge(status) + " " * max(0, 8 - len(visible_status))
    return (
        f"{badge_padded} "
        f"{agent:<12} "
        f"{pid:>7} "
        f"{cpu:>6} "
        f"{mem:>8} "
        f"{up:>8} "
        f"{sub_chip(sub)} "
        f"{tok_chip(tok)}  "
        f"{project[:14]:<14}  "
        f"{doing}"
    )

# Sophisticated fake data: zero-knowledge prover farms, MEV searchers,
# rollup sequencers, restaking primitives, account-abstraction bundlers,
# bridge watchtowers — typical heavy AI-coding-agent workloads.
agents = [
    ("busy",     "claude⚡",  28471, "31.4%", "642M", "3h17m",  2, "9.2M",  "zk-rollup",     "Bash: nargo prove --witness witness.tr --srs srs_2_22.bin  GOD MODE"),
    ("busy",     "codex",        31802, "24.6%", "538M", "47m12s", 0, "4.1M",  "mev-searcher",  "Edit: src/searcher/atomic_arb_v3.rs"),
    ("spawning", "claude",       19432, "11.8%", "521M", "3h44m",  1, "6.8M",  "eigen-restake", "Task: prove transcript Fiat-Shamir soundness"),
    ("active",   "aider",        24190,  "5.2%", "478M", "1h08m",  0, "2.4M",  "amm-v4-hooks",  "applying SEARCH/REPLACE: contracts/HookV4.sol"),
    ("active",   "claude",       33561,  "4.1%", "412M", "18m22s", 0, "3.7M",  "kzg-blob-pipe", "Write: src/blob_tx_simulator.rs (EIP-4844)"),
    ("active",   "gemini",       22817,  "2.9%", "264M", "5h11m",  0, "1.4M",  "erc4337-bundler","analysing UserOperation paymaster validation"),
    ("active",   "claude",       29774,  "1.8%", "395M", "2h31m",  0, "1.1M",  "huff-fuzzer",   "running: forge fuzz --runs 100000 --match-contract"),
    ("idle",     "claude",       14002,  "0.0%", "412M", "4d02h",  0, "5.9M",  "cosmos-ibc",    "(idle 12m08s)"),
    ("idle",     "claude",       17293,  "0.0%", "389M", "2d18h",  0, "2.1M",  "polygon-cdk",   "(idle 47m22s)"),
    ("idle",     "goose",        28773,  "0.0%", "182M", "6h33m",  0, "560k",  "substrate-rt",  "applied 3 patches to pallet_eigenlayer"),
    ("idle",     "cursor-agent", 31102,  "0.0%", "287M", "3h22m",  0, "320k",  "cosmwasm",      "watching contracts/ for changes"),
    ("idle",     "ollama",       18234,  "0.0%", "16.2M","9d03h",  0, "",      "ollama serve",  "/usr/local/bin/ollama serve"),
    ("idle",     "claude",       29113,  "0.0%", "245M", "11h47m", 0, "3.4M",  "halo2-circuits","(idle 2h17m)"),
    ("idle",     "ollama",       22091,  "0.0%", "14.2M","9d03h",  0, "",      "ollama serve",  "/bin/ollama serve"),
    ("idle",     "claude",       38221,  "0.0%", "198M", "1d04h",  0, "740k",  "bridge-watch",  "claude --resume --model opus"),
]

active_n   = sum(1 for r in agents if r[0] in ("busy","spawning","active","idle"))
busy_n     = sum(1 for r in agents if r[0] in ("busy","spawning"))
subs_total = sum(r[6] for r in agents)
projects   = {r[8] for r in agents}

print(
    f"agtop  active={active_n}  busy={busy_n}  subagents={subs_total}  "
    f"waiting=4  completed=8  projects={len(projects)}  "
    f"cpu=81.8%  mem=4.8G  tokens=41.6M  cost=$612.40"
)
print(
    f"{BOLD}STATUS   AGENT          PID    CPU%      MEM       UP  SUB   TOK  "
    f"PROJECT         DOING{UNBOLD}"
)
for r in agents:
    print(row(*r))
