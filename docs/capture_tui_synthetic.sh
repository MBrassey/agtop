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

# ── Spawner: execve(argv[0]=label) so /proc shows the agent name ───────────
cat > /tmp/agtop-spawn.py <<'PY'
import os, sys
cwd, label, *args = sys.argv[1:]
os.chdir(cwd)
argv = [label] + list(args) + ["86400"]
os.execve("/usr/bin/sleep", argv, os.environ)
PY

# ── Spinner: a tiny C program that ignores argv and burns CPU.  We launch
# it via `exec -a` so /proc/<pid>/cmdline reads as the spoofed label only,
# unlike a Python spinner which would show the interpreter + script path.
cat > /tmp/agtop-spin.c <<'C'
int main(int argc, char **argv) {
    volatile unsigned long x = 0;
    for (;;) { for (int i = 0; i < 5000000; i++) x += i*i; }
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
         /tmp/halo2-circuits /tmp/ollama-svc; do
  mkdir -p "$d"
done

# Three CPU-spinning agents (proper cmdline via `exec -a`) so the row
# reads non-zero and the busy/active visuals are realistic.
( cd /tmp/zk-rollup-prover && exec -a "claude --dangerously-skip-permissions" /tmp/agtop-spin ) &
( cd /tmp/mev-searcher     && exec -a "codex --resume"                         /tmp/agtop-spin ) &
( cd /tmp/eigen-restake    && exec -a "claude --dangerously-skip-permissions" /tmp/agtop-spin ) &

# Sleeping agents — appear active via fresh JSONL mtime, 0% CPU is fine.
spawn () { python3 /tmp/agtop-spawn.py "$@" & }
spawn /tmp/zk-rollup-prover  claude --dangerously-skip-permissions
spawn /tmp/amm-v4-hooks      aider  --no-git
spawn /tmp/kzg-blob-pipe     claude
spawn /tmp/erc4337-bundler   gemini --query
spawn /tmp/huff-fuzzer       claude
spawn /tmp/cosmos-ibc-relay  claude
spawn /tmp/polygon-cdk       claude
spawn /tmp/halo2-circuits    claude
spawn /tmp/ollama-svc        ollama serve
spawn /tmp/ollama-svc        ollama serve
sleep 1.2
HOME=/tmp/agtop-demo-home /tmp/agtop-venv/bin/python /tmp/agtop-tui-capture.py "$AGTOP_BIN"
SH
chmod +x /tmp/agtop-tui-driver.sh

cat > /tmp/agtop-tui-capture.py <<'PY'
import os, pty, select, struct, fcntl, termios, time, sys
import pyte
COLS, ROWS = 220, 56
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.environ["TERM"] = "xterm-256color"; os.environ["COLORTERM"] = "truecolor"
    os.execv(sys.argv[1], ["agtop", "--interval", "0.5"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
deadline = time.time() + 7
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

# Tightly fitted chromium frame — width based on 220-col content at 12px.
cat > /tmp/fake-tui-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0d1014; }
  body { padding: 8px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 10px; padding: 10px 14px;
           box-shadow: 0 4px 20px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.04) inset;
           color: #e1ded7; line-height: 1.16; font-size: 12px; width: fit-content; }
  .titlebar { padding-bottom: 8px; display: flex; gap: 6px; align-items: center;
              border-bottom: 1px solid rgba(255,255,255,0.06); margin-bottom: 10px; }
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

# Tight window — 220 cols × ~7.2 px/char @12px = 1584 + padding ≈ 1660; rows
# 56 × ~14 px line-height ≈ 784 + chrome ≈ 830.  scale-factor 2 → 2× DPI.
chromium --headless --no-sandbox --disable-gpu \
  --window-size=1700,830 --hide-scrollbars \
  --force-device-scale-factor=2 \
  --screenshot="$here/screenshot-tui.png" \
  "file:///tmp/fake-tui-page.html" 2>/dev/null

ls -lh "$here/screenshot-tui.png"
