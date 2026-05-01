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
    "spawning":  ("● SPWN ", BOLD + CYN),
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
    ("busy",     "claude",       28471, "82.4%", "780M", "3h17m",  2, "9.2M",  "zk-rollup",      "Bash: nargo prove --witness witness.tr --srs srs_2_22.bin"),
    ("busy",     "codex",        31802, "67.1%", "512M", "47m12s", 0, "4.1M",  "mev-searcher",   "Edit: src/searcher/atomic_arb_v3.rs (curve-pool variant)"),
    ("spawning", "claude",       19432, "74.6%", "640M", "3h44m",  1, "6.8M",  "eigen-restake",  "Task: prove transcript Fiat-Shamir soundness"),
    ("busy",     "claude",       17234, "71.2%", "540M", "1h22m",  1, "5.4M",  "risc0-prover",   "Bash: cargo r --release --bin r0 -- prove --hash poseidon2"),
    ("busy",     "codex",        29918, "58.3%", "320M", "2h41m",  0, "3.9M",  "erc4337-bundler","Edit: src/bundler/validator.ts (AA33/AA34 surfacing)"),
    ("busy",     "claude",       24812, "53.7%", "410M", "4h05m",  1, "4.7M",  "scroll-zkevm",   "Bash: zkevm-prover --aggregator-depth 5 --batch 9412"),
    ("active",   "claude",       30210, "47.4%", "360M", "55m02s", 0, "2.8M",  "sp1-zkvm",       "Edit: core/src/syscall/precompiles/keccak256.rs"),
    ("active",   "claude",       33561, "45.1%", "384M", "18m22s", 0, "3.7M",  "kzg-blob-pipe",  "Write: src/blob_tx_simulator.rs (EIP-4844 simulator)"),
    ("active",   "claude",       16778, "41.2%", "290M", "2h11m",  0, "2.3M",  "optimism-bedrock","Bash: cast call $DGF init_bonds (cannon dispute game)"),
    ("active",   "claude",       29774, "38.0%", "220M", "2h31m",  0, "1.1M",  "huff-fuzzer",    "Bash: forge fuzz --runs 100000 --match-contract"),
    ("active",   "aider",        24190, "31.4%", "256M", "1h08m",  0, "2.4M",  "amm-v4-hooks",   "applying SEARCH/REPLACE: contracts/HookV4.sol"),
    ("active",   "claude",       21338, "28.2%", "200M", "3h17m",  0, "1.8M",  "celestia-da",    "Edit: rollup/da/celestia_client.go (namespace remap)"),
    ("active",   "claude",       19984, "22.1%", "180M", "1h44m",  0, "1.2M",  "uniswap-v4-core","Bash: halmos --contract PoolManager --function unlock"),
    ("active",   "claude",       18820, "16.8%", "130M", "27m11s", 0, "880k",  "foundry-suite",  "Bash: forge test --invariant-runs 50000 --invariant-depth 256"),
    ("active",   "codex",        13127, "14.3%", "140M", "52m04s", 0, "1.4M",  "lido-csm",       "Read: contracts/CSModule.sol (exit queue compaction)"),
    ("active",   "aider",        15402,  "9.1%", "110M", "2h09m",  0, "640k",  "arbitrum-stylus","Write: bench/erc20_transfer.rs (Stylus vs Solidity)"),
    ("active",   "gemini",       22817,  "5.4%", "264M", "5h11m",  0, "1.4M",  "halo2-circuits", "analysing IPA recursion lookup table sizing"),
    ("idle",     "claude",       14002,  "0.0%", "412M", "4d02h",  0, "5.9M",  "cosmos-ibc",     "(idle 12m08s)"),
    ("idle",     "claude",       17293,  "0.0%", "389M", "2d18h",  0, "2.1M",  "polygon-cdk",    "(idle 47m22s)"),
    ("idle",     "goose",        28773,  "0.0%", "182M", "6h33m",  0, "560k",  "substrate-rt",   "applied 3 patches to pallet_eigenlayer"),
    ("idle",     "cursor-agent", 31102,  "0.0%", "287M", "3h22m",  0, "320k",  "cosmwasm",       "watching contracts/ for changes"),
    ("idle",     "ollama",       18234,  "0.0%", "16.2M","9d03h",  0, "",      "ollama serve",   "/usr/local/bin/ollama serve"),
    ("idle",     "ollama",       22091,  "0.0%", "14.2M","9d03h",  0, "",      "ollama serve",   "/bin/ollama serve"),
    ("idle",     "claude",       38221,  "0.0%", "198M", "1d04h",  0, "740k",  "bridge-watch",   "claude --resume --model opus"),
]

active_n   = sum(1 for r in agents if r[0] in ("busy","spawning","active","idle"))
busy_n     = sum(1 for r in agents if r[0] in ("busy","spawning"))
subs_total = sum(r[6] for r in agents)
projects   = {r[8] for r in agents}

total_cpu = sum(float(r[3].rstrip('%')) for r in agents)
print(
    f"agtop  active={active_n}  busy={busy_n}  subagents={subs_total}  "
    f"waiting=6  completed=11  projects={len(projects)}  "
    f"cpu={total_cpu:.1f}%  mem=6.4G  tokens=58.1M  cost=$894.32"
)
print(
    f"{BOLD}STATUS   AGENT          PID    CPU%      MEM       UP  SUB   TOK  "
    f"PROJECT         DOING{UNBOLD}"
)
for r in agents:
    print(row(*r))
