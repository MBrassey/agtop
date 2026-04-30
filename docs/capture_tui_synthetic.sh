#!/usr/bin/env bash
# Capture a real-binary ratatui TUI snapshot with synthetic agents.
#
# Strategy: open a user+pid namespace via `unshare -U -p -f --mount-proc -r`
# so /proc only shows processes spawned inside.  Spawn a curated set of fake
# agent processes (matching the built-in matchers via execve argv[0]).  Some
# spin for CPU% to show up as non-zero; others sleep.  Set HOME to a sandbox
# containing fake ~/.claude/projects JSONL so the session enrichers find
# the matching transcripts (rich: multiple tool_use, tool_result, prose,
# usage/model fields, some in-flight Task subagents).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
bin="$root/target/release/agtop"
[ -x "$bin" ] || { echo "build the release binary first: cargo build --release"; exit 1; }
command -v aha       >/dev/null || { echo "aha not on PATH"; exit 1; }
command -v chromium  >/dev/null || { echo "chromium not on PATH"; exit 1; }
[ -x /tmp/agtop-venv/bin/python ] || {
  python3 -m venv /tmp/agtop-venv
  /tmp/agtop-venv/bin/pip install --quiet pyte
}

# ── Sophisticated fake transcripts: ZK provers, MEV searchers, EigenLayer
# restaking, AMM hooks, KZG blob pipelines, ERC-4337 bundlers, Huff fuzzers,
# IBC relayers, CDK rollups, Halo2 circuits.  Each session has multiple
# tool calls and prose entries so the live-preview popup has real material.
fake_home="/tmp/agtop-demo-home"
rm -rf "$fake_home"
mkdir -p "$fake_home/.claude/projects"

# write a JSONL transcript with N events.
# usage: rich_session <encoded-cwd> <model> <user> <prose1> <tool> <tool_arg> <prose2> <tool2> <tool2_arg> [in_flight_task_subj]
rich_session () {
  local enc="$1" model="$2"
  local dir="$fake_home/.claude/projects/${enc}"
  mkdir -p "$dir"
  shift 2
  local user="$1" p1="$2" t1="$3" ta1="$4" p2="$5" t2="$6" ta2="$7" subagent="${8:-}"
  cat > "$dir/sess.jsonl" <<JSONL
{"type":"user","timestamp":"2026-04-29T00:00:00.000Z","message":{"role":"user","content":"${user}"}}
{"type":"assistant","timestamp":"2026-04-29T00:00:01.000Z","message":{"id":"msg_1","model":"${model}","content":[{"type":"text","text":"${p1}"}],"usage":{"input_tokens":18000,"output_tokens":1100,"cache_read_input_tokens":120000}}}
{"type":"assistant","timestamp":"2026-04-29T00:00:02.000Z","message":{"id":"msg_2","model":"${model}","content":[{"type":"tool_use","id":"toolu_a","name":"${t1}","input":{"command":"${ta1}","description":"${p1}"}}],"usage":{"input_tokens":21000,"output_tokens":80}}}
{"type":"user","timestamp":"2026-04-29T00:00:03.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a","content":"completed in 12.4s — 0 errors"}]}}
{"type":"assistant","timestamp":"2026-04-29T00:00:04.000Z","message":{"id":"msg_3","model":"${model}","content":[{"type":"text","text":"${p2}"}],"usage":{"input_tokens":24000,"output_tokens":900,"cache_read_input_tokens":180000}}}
{"type":"assistant","timestamp":"2026-04-29T00:00:05.000Z","message":{"id":"msg_4","model":"${model}","content":[{"type":"tool_use","id":"toolu_b","name":"${t2}","input":{"command":"${ta2}","description":"${p2}"}}],"usage":{"input_tokens":27000,"output_tokens":50}}}
JSONL
  if [ -n "$subagent" ]; then
    cat >> "$dir/sess.jsonl" <<JSONL
{"type":"assistant","timestamp":"2026-04-29T00:00:06.000Z","message":{"id":"msg_5","model":"${model}","content":[{"type":"tool_use","id":"toolu_task1","name":"Task","input":{"subject":"${subagent}","subagent_type":"code-reviewer","prompt":"Review the diff and report any concerns"}}],"usage":{"input_tokens":30000,"output_tokens":40}}}
JSONL
  fi
}

# Sophisticated blockchain workloads.  Each row: cwd-encoded · model ·
# user prompt · prose1 · tool1 · tool1_arg · prose2 · tool2 · tool2_arg ·
# (optional) in-flight subagent subject.
rich_session "-tmp-zk-rollup-prover"  "claude-sonnet-4-7" \
  "Generate the witness and prove the new circuits"  \
  "Building witness against srs_2_22 — Plonk proving system, ~140k constraints, MSM precomputation cached" \
  "Bash" "nargo prove --witness witness.tr --srs srs_2_22.bin"  \
  "Witness verified.  Now run the recursion proof to fold this batch with the previous epoch" \
  "Bash" "nargo prove-recursive --src 0x4a8c..7d --depth 3"  \
  "review constraint inflation in the new permutation argument"

rich_session "-tmp-mev-searcher"      "claude-sonnet-4-7" \
  "Audit atomic_arb_v3 hot path"  \
  "Profiled the bundle composer — 78% of latency is in pool_state.update().  Inlining and removing the BTreeMap layer" \
  "Edit" "src/searcher/atomic_arb_v3.rs"  \
  "Now wire in the Curve-pool variant — it hits a different reorg-risk profile so we keep the bundle separate" \
  "Edit" "src/searcher/curve_lp_arb.rs"  ""

rich_session "-tmp-eigen-restake"     "claude-opus-4-7"   \
  "Verify Fiat-Shamir transcript soundness"  \
  "Drafting the soundness reduction.  Adversary can't predict the challenge before committing to all polynomial coefficients" \
  "Read" "circuits/fiat_shamir_transcript.rs"  \
  "Now formalise the simulator: it must reproduce the prover's transcript without knowing the witness.  Sketching in Lean 4" \
  "Write" "proofs/fs_soundness.lean"  \
  "audit the BLS12-381 pairing security assumptions"

rich_session "-tmp-amm-v4-hooks"      "claude-sonnet-4-7" \
  "Wire the dynamic-fee hook into the v4 pool factory"  \
  "Implementing IHooks.beforeSwap and afterSwap.  Fee tier rotates via TWAP-derived volatility every 256 blocks" \
  "Edit" "contracts/HookV4.sol"  \
  "Reading the pool manager invariants — confirming the hook can't violate the conservation-of-liquidity check" \
  "Read" "contracts/PoolManager.sol"  ""

rich_session "-tmp-kzg-blob-pipe"     "claude-sonnet-4-7" \
  "Simulate the EIP-4844 blob carrier transaction"  \
  "Building the blob-carrier sim against the latest spec.  Each tx carries ≤6 blobs of 4096 field elements" \
  "Write" "src/blob_tx_simulator.rs"  \
  "KZG commitment math — using Arkworks' BLS12-381 pairing.  Need to bench multi-blob batch verification" \
  "Bash" "cargo bench --bench kzg_batch"  ""

rich_session "-tmp-erc4337-bundler"   "gpt-5"             \
  "Validate the UserOperation paymaster contract"  \
  "EntryPoint v0.7 paymaster validation: paymasterVerificationGasLimit + paymasterPostOpGasLimit must split cleanly" \
  "Read" "contracts/EntryPointV07.sol"  \
  "Tracing the bundler's simulateValidation revert paths.  Need to surface AA33 vs AA34 cleanly to the RPC client" \
  "Edit" "src/bundler/validator.ts"  ""

rich_session "-tmp-huff-fuzzer"       "claude-sonnet-4-7" \
  "Fuzz the batch settlement contract"  \
  "Running forge fuzz against the Huff-compiled batch settler.  100k runs, MIRROR mode, with 7 invariant assertions" \
  "Bash" "forge fuzz --runs 100000 --match-contract BatchSettlerHuff"  \
  "First counter-example landed: under-flow in the index calculation when N=0 — fixing the guard" \
  "Edit" "src/BatchSettler.huff"  ""

rich_session "-tmp-cosmos-ibc-relay"  "claude-sonnet-4-7" \
  "IBC relayer health check"  \
  "Verifying the channel-0 ↔ channel-32 packet flow.  Last 100 packets all relayed within 8s of source commit" \
  "Bash" "rly tx link --src-chain-id channel-0"  \
  "Tendermint light-client header lag is 4 blocks — within bounds.  Watching for unbonding-period drift" \
  "Bash" "rly q clients --light"  ""

rich_session "-tmp-polygon-cdk"       "claude-opus-4-7"   \
  "Validity-proof generator cadence"  \
  "Polygon CDK prover hit the 18234-batch checkpoint.  Each batch ≈420ms on the GPU pool, well under SLA" \
  "Bash" "polygon-cdk-prover --batch 18234 --device cuda"  \
  "Aggregating the last 32 batches into a recursive zkEVM proof — that's the L1 settlement bundle" \
  "Bash" "polygon-cdk-aggregator --window 32"  ""

rich_session "-tmp-halo2-circuits"    "claude-sonnet-4-7" \
  "Pause and re-tune the recursion gadget"  \
  "Halo2 IPA recursion: the inner-product argument lookup table is over-provisioned.  Trimming to 2^14 rows" \
  "Edit" "src/circuit/poseidon.rs"  \
  "Re-running the keygen so the verifier key matches.  Should drop the proof size by ~600 bytes" \
  "Bash" "halo2-cli keygen --circuit poseidon --rows 16384"  ""

rich_session "-tmp-uniswap-v4-core"   "claude-sonnet-4-7" \
  "Pool-manager invariant proof"  \
  "Drafting the invariant: total pool liquidity ≥ Σ position liquidity at any block.  Symbolic execution via halmos" \
  "Bash" "halmos --contract PoolManager --function unlock"  \
  "Hook permission bitmap audit — checking that flag 0x80 (afterInitialize) doesn't bypass slot0 init guard" \
  "Read" "contracts/types/Hooks.sol" "scan downstream callers of Hooks.callHook"

rich_session "-tmp-lido-csm"          "codex-mini"        \
  "Validator exit queue refactor"  \
  "Auditing the CSM exit queue: bond curve + leftover handling.  Edge case at queue head when stake delta is zero" \
  "Read" "contracts/CSModule.sol"  \
  "Tracing the keysOpIndex compaction.  Looks correct but the gas cost grows linearly with deletions — flagging" \
  "Bash" "forge test --match-test testExitQueueCompaction -vv" ""

rich_session "-tmp-optimism-bedrock"  "claude-opus-4-7"   \
  "Output-root proposer cadence"  \
  "Bedrock proposer is 8 blocks behind L1 finality — that's within the 12-epoch tolerance.  Nothing to do" \
  "Bash" "cast call \$L2OO finalizedL1Block"  \
  "Now sanity-check the dispute-game factory bond.  Should be ≥ 0.08 ETH for the cannon faultDispute type" \
  "Bash" "cast call \$DGF init_bonds" "verify the new fault-proof VM transition"

rich_session "-tmp-arbitrum-stylus"   "claude-sonnet-4-7" \
  "Stylus contract gas pricing"  \
  "Stylus pre-compiles charge ink, not gas.  100 ink ≈ 1 gas at base, but cold-storage is 2.1k ink = 21 gas" \
  "Read" "stylus/src/program.rs"  \
  "Adding a benchmark: native ERC-20 transfer in Solidity vs Rust+Stylus.  Expecting ~30% gas reduction on Rust path" \
  "Write" "bench/erc20_transfer.rs" ""

rich_session "-tmp-celestia-da"       "claude-sonnet-4-7" \
  "Blob namespace mapping"  \
  "Celestia DA: each namespace is 29 bytes.  Need to remap the rollup's appns to avoid collision with the Roots namespace" \
  "Edit" "rollup/da/celestia_client.go"  \
  "Confirm the 2D Reed-Solomon padding for non-power-of-two blob counts.  Working through the share-builder flow" \
  "Read" "celestia-app/pkg/shares/builder.go" ""

rich_session "-tmp-scroll-zkevm"      "claude-opus-4-7"   \
  "Recursion-depth tuning"  \
  "Scroll zkEVM: aggregator depth is 5.  Each layer adds ~6.4s prove time but shrinks final proof by 2.1x" \
  "Bash" "zkevm-prover --aggregator-depth 5 --batch 9412"  \
  "Sanity-checking the EIP-4844 blob inclusion in the L1 settlement transaction.  Side-car bytes match commitment" \
  "Bash" "scroll-cli batch verify --batch 9412" "review the new batch-header serialisation"

rich_session "-tmp-risc0-prover"      "codex-mini"        \
  "Reckon proof time on the new STARK"  \
  "RISC0 v2 hash function: Poseidon2 vs SHA-2.  Poseidon2 is 3.2× faster on the GPU but slightly larger proof" \
  "Bash" "cargo r --release --bin r0 -- prove --hash poseidon2"  \
  "Building the Bonsai CI workflow so every PR runs an end-to-end proof on the dev guest" \
  "Write" ".github/workflows/bonsai-ci.yml" ""

rich_session "-tmp-sp1-zkvm"          "claude-sonnet-4-7" \
  "RISC-V trace shrink"  \
  "SP1 trace: trimming the keccak precompile to a single permutation pass.  Saved 11k rows on the bridge program" \
  "Edit" "core/src/syscall/precompiles/keccak256.rs"  \
  "Recursion proof composition is back-running on the GPU.  Switching to the cuda-aware reduction kernel" \
  "Bash" "RUST_LOG=info cargo run --release --bin sp1-prover -- prove" ""

rich_session "-tmp-foundry-suite"     "claude-sonnet-4-7" \
  "Forge invariant test suite"  \
  "Adding three invariants for the lending market: collateral ≥ debt, totalShares == Σ shares, no double-borrow" \
  "Write" "test/invariant/Lending.t.sol"  \
  "Running the full suite at depth 256 with 50k runs.  Surfaced one underflow in liquidation when LTV == max" \
  "Bash" "forge test --invariant-runs 50000 --invariant-depth 256" ""

# ── Spawner: execve(argv[0]=label) so /proc shows the agent name ───────────
cat > /tmp/agtop-spawn.py <<'PY'
import os, sys
cwd, label, *args = sys.argv[1:]
os.chdir(cwd)
argv = [label] + list(args) + ["86400"]
os.execve("/usr/bin/sleep", argv, os.environ)
PY

# ── Spinner: a tiny C program that targets a duty-cycle CPU% and
# allocates a target RAM in MiB.  argv is ignored (we set it via
# `exec -a` so /proc/<pid>/cmdline reads as the spoofed label).  The
# spinner reads its target from environment so the duty cycle is
# stable across the whole capture window.  Output: per-agent CPU% in
# 3..85 range and RSS in 80..820 MiB range, varying per spawn.
cat > /tmp/agtop-spin.c <<'C'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
int main(int argc, char **argv) {
    const char *p = getenv("AGTOP_PCT");  int pct = p ? atoi(p) : 50;
    const char *m = getenv("AGTOP_MB");   int mb  = m ? atoi(m)  : 128;
    if (pct < 1) pct = 1; if (pct > 99) pct = 99;
    if (mb  < 8) mb  = 8;
    /* Allocate + touch RAM so RSS reflects the target. */
    size_t bytes = (size_t)mb * 1024 * 1024;
    char *buf = (char *)malloc(bytes);
    if (buf) memset(buf, 0xa5, bytes);
    /* Duty cycle: busy for `on` ms, sleep for `off` ms each 100 ms. */
    long on_us  = pct * 1000L;
    long off_us = (100 - pct) * 1000L;
    volatile unsigned long x = 0;
    struct timespec ts;
    while (1) {
        clock_gettime(CLOCK_MONOTONIC, &ts);
        long start_ns = ts.tv_nsec; long start_s = ts.tv_sec;
        long elapsed_us = 0;
        while (elapsed_us < on_us) {
            for (int i = 0; i < 200000; i++) x += i*i + (unsigned long)buf;
            clock_gettime(CLOCK_MONOTONIC, &ts);
            elapsed_us = (ts.tv_sec - start_s) * 1000000L + (ts.tv_nsec - start_ns) / 1000;
        }
        if (off_us > 0) usleep(off_us);
    }
    return 0;
}
C
cc -O2 -o /tmp/agtop-spin /tmp/agtop-spin.c

# ── Driver that runs inside the namespace ─────────────────────────────────
cat > /tmp/agtop-tui-driver.sh <<'SH'
#!/usr/bin/env bash
set -e
for d in /tmp/zk-rollup-prover /tmp/mev-searcher /tmp/eigen-restake \
         /tmp/amm-v4-hooks /tmp/kzg-blob-pipe /tmp/erc4337-bundler \
         /tmp/huff-fuzzer /tmp/cosmos-ibc-relay /tmp/polygon-cdk \
         /tmp/halo2-circuits /tmp/ollama-svc /tmp/uniswap-v4-core \
         /tmp/lido-csm /tmp/optimism-bedrock /tmp/arbitrum-stylus \
         /tmp/celestia-da /tmp/scroll-zkevm /tmp/risc0-prover \
         /tmp/sp1-zkvm /tmp/foundry-suite; do
  mkdir -p "$d"
done

# Spawn a CPU spinner with a target percent + RAM size, spoofed cmdline.
# usage: spin <cwd> <pct> <ram_mb> <argv0...>
spin () {
  local cwd="$1" pct="$2" mb="$3"; shift 3
  local label="$*"
  ( cd "$cwd" && AGTOP_PCT="$pct" AGTOP_MB="$mb" exec -a "$label" /tmp/agtop-spin ) &
}

# 16 CPU-active agents with varied CPU% (3–82) and RAM (90–780 MiB).
# Different vendors, different argv0 styles, mixed across project dirs.
spin /tmp/zk-rollup-prover  82 780 "claude --dangerously-skip-permissions"
spin /tmp/mev-searcher      67 512 "codex --resume"
spin /tmp/eigen-restake     74 640 "claude --dangerously-skip-permissions"
spin /tmp/amm-v4-hooks      31 256 "aider --no-git"
spin /tmp/kzg-blob-pipe     45 384 "claude"
spin /tmp/erc4337-bundler   58 320 "gemini --query"
spin /tmp/huff-fuzzer       38 220 "claude --dangerously-skip-permissions"
spin /tmp/uniswap-v4-core   22 180 "claude"
spin /tmp/lido-csm          14 140 "codex --resume"
spin /tmp/optimism-bedrock  41 290 "claude --dangerously-skip-permissions"
spin /tmp/arbitrum-stylus    9 110 "aider"
spin /tmp/celestia-da       28 200 "claude"
spin /tmp/scroll-zkevm      53 410 "claude --dangerously-skip-permissions"
spin /tmp/risc0-prover      71 540 "codex"
spin /tmp/sp1-zkvm          47 360 "claude"
spin /tmp/foundry-suite     16 130 "aider --no-git"

# Sleeping agents — fresh JSONL mtime makes them "waiting"/"active",
# CPU stays near 0.  Adds breadth without overwhelming the table.
spawn () { python3 /tmp/agtop-spawn.py "$@" & }
spawn /tmp/cosmos-ibc-relay   claude
spawn /tmp/polygon-cdk        claude
spawn /tmp/halo2-circuits     claude
spawn /tmp/ollama-svc         ollama serve
spawn /tmp/ollama-svc         ollama serve
sleep 2.5
HOME=/tmp/agtop-demo-home /tmp/agtop-venv/bin/python /tmp/agtop-tui-capture.py "$AGTOP_BIN"
SH
chmod +x /tmp/agtop-tui-driver.sh

cat > /tmp/agtop-tui-capture.py <<'PY'
import os, pty, select, struct, fcntl, termios, time, sys
import pyte
# Wider 16:9-ish aspect for the GitHub README (wraps to 100% of the
# README content column at github.com).  240×52 fits the agents panel,
# the per-pane charts, the projects strip, and the right-hand sessions
# rail without clipping any content.
COLS, ROWS = 240, 52
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.environ["TERM"] = "xterm-256color"; os.environ["COLORTERM"] = "truecolor"
    os.execv(sys.argv[1], ["agtop", "--interval", "0.4"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
# Run long enough for the 60-tick CPU/MEM/tokens history to fully
# populate (60 ticks × 0.4 s = 24 s, plus 4 s slack).
deadline = time.time() + 28
while time.time() < deadline:
    r,_,_ = select.select([fd],[],[],0.1)
    if r:
        try: data = os.read(fd, 65536)
        except OSError: break
        if not data: break
        stream.feed(data)
try: os.write(fd, b"q")
except OSError: pass
time.sleep(0.3)
try: os.kill(pid, 9)
except ProcessLookupError: pass
try: os.waitpid(pid, 0)
except ChildProcessError: pass

def col(c, bg=False):
    base = 48 if bg else 38
    if c == "default": return ""
    if isinstance(c, str) and len(c) == 6 and all(ch in "0123456789abcdef" for ch in c.lower()):
        try:
            r=int(c[0:2],16); g=int(c[2:4],16); b=int(c[4:6],16)
            return f"\x1b[{base};2;{r};{g};{b}m"
        except ValueError: return ""
    names = {"black":30,"red":31,"green":32,"brown":33,"blue":34,"magenta":35,"cyan":36,"white":37}
    return f"\x1b[{names[c] + (10 if bg else 0)}m" if c in names else ""

out=[]
for y in range(ROWS):
    bits=[]; lf=lb=""; lbold=False
    for x in range(COLS):
        ch=screen.buffer[y][x]
        fg=col(ch.fg,False); bg=col(ch.bg,True)
        if (fg,bg,ch.bold)!=(lf,lb,lbold):
            bits.append("\x1b[0m");
            if ch.bold: bits.append("\x1b[1m")
            bits.append(fg+bg)
            lf,lb,lbold=fg,bg,ch.bold
        bits.append(ch.data if ch.data else " ")
    bits.append("\x1b[0m")
    out.append("".join(bits).rstrip())
print("\n".join(out))
PY

AGTOP_BIN="$bin" unshare -U -p -f --mount-proc -r /tmp/agtop-tui-driver.sh \
   > /tmp/fake-tui.ansi
echo "captured $(wc -l < /tmp/fake-tui.ansi) lines / $(wc -c < /tmp/fake-tui.ansi) bytes"

aha --black --no-header < /tmp/fake-tui.ansi > /tmp/fake-tui.html

# Flex-centered frame: body fills the viewport, frame is centered both
# axes so chromium produces a screenshot with equal margins on all sides.
cat > /tmp/fake-tui-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; height: 100%; background: #0d1014; }
  body { display: flex; align-items: center; justify-content: center;
         font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 10px; padding: 12px 16px;
           box-shadow: 0 6px 22px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.04) inset;
           color: #e1ded7; line-height: 1.16; font-size: 12px; }
  .titlebar { padding-bottom: 9px; display: flex; gap: 6px; align-items: center;
              border-bottom: 1px solid rgba(255,255,255,0.06); margin-bottom: 11px; }
  .dot { width: 10px; height: 10px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 8px; color: #7d828e; font-size: 10.5px; letter-spacing: 0.02em; }
  pre { margin: 0; white-space: pre; color: inherit; background: transparent; font: inherit; }
</style></head><body>
<div class="frame">
  <div class="titlebar">
    <div class="dot r"></div><div class="dot y"></div><div class="dot g"></div>
    <span class="label">agtop  ·  full TUI</span>
  </div>
<pre>
HEAD
cat /tmp/fake-tui.html >> /tmp/fake-tui-page.html
echo '</pre></div></body></html>' >> /tmp/fake-tui-page.html

# Two-pass capture: render at a generous viewport, query the frame's
# bounding rect via headless devtools, then take a clipped screenshot
# that snaps tightly to the frame + a uniform 20 px margin on every
# side.  Eliminates the empty slack that comes from guessing pixel
# dimensions of monospace glyph rendering.
chromium --headless --no-sandbox --disable-gpu \
  --hide-scrollbars --force-device-scale-factor=2 \
  --window-size=2000,1200 \
  --virtual-time-budget=500 \
  --screenshot="$here/screenshot-tui.png" \
  --screenshot-format=png \
  "file:///tmp/fake-tui-page.html" 2>/dev/null

# Crop to content using ImageMagick: trim the uniform background, then
# add an even 24 px border in the same background colour.  Result: the
# frame fills the image with consistent padding on every side.
if command -v magick >/dev/null 2>&1; then
  magick "$here/screenshot-tui.png" \
    -bordercolor "#0d1014" -border 1 \
    -fuzz 2% -trim +repage \
    -bordercolor "#0d1014" -border 20x20 \
    "$here/screenshot-tui.png"
elif command -v convert >/dev/null 2>&1; then
  convert "$here/screenshot-tui.png" \
    -bordercolor "#0d1014" -border 1 \
    -fuzz 2% -trim +repage \
    -bordercolor "#0d1014" -border 20x20 \
    "$here/screenshot-tui.png"
fi

ls -lh "$here/screenshot-tui.png"
