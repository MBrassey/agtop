#!/usr/bin/env bash
# Capture a real-binary ratatui TUI snapshot with synthetic agents.
#
# Strategy: open a user+pid namespace via `unshare -U -p -f --mount-proc -r`
# so /proc only shows processes spawned inside.  Spawn a curated set of fake
# agent processes (matching the built-in matchers via execve argv[0]).  Set
# HOME to a sandbox containing fake ~/.claude/projects JSONL so the session
# enrichers find the matching transcripts.  Run agtop via pyte to capture
# the rendered output.  Output: docs/screenshot-tui.png.
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

# ── Fake home with synthetic Claude-Code transcripts ──────────────────────
fake_home="/tmp/agtop-demo-home"
rm -rf "$fake_home"
mkdir -p "$fake_home/.claude/projects"

make_session () {  # $1=cwd  $2=user  $3=asst  $4=tool  $5=tool_arg  $6=model
  local enc="${1//\//-}"
  local dir="$fake_home/.claude/projects/${enc}"
  mkdir -p "$dir"
  cat > "$dir/sess.jsonl" <<JSONL
{"type":"user","timestamp":"2026-04-29T00:00:00.000Z","message":{"role":"user","content":"$2"}}
{"type":"assistant","timestamp":"2026-04-29T00:00:01.000Z","message":{"id":"msg_1","model":"$6","content":[{"type":"text","text":"$3"}],"usage":{"input_tokens":12000,"output_tokens":800,"cache_read_input_tokens":40000}}}
{"type":"assistant","timestamp":"2026-04-29T00:00:02.000Z","message":{"id":"msg_2","model":"$6","content":[{"type":"tool_use","id":"toolu_$RANDOM","name":"$4","input":{"command":"$5","description":"$3"}}],"usage":{"input_tokens":15000,"output_tokens":50}}}
JSONL
}

# Each entry: project | cwd | user prompt | assistant prose | tool | tool input | model
make_session "/tmp/zk-rollup-prover" "Prove the new circuits"          "Running nargo prove on the latest witness, ~3 min"        "Bash"  "nargo prove --witness witness.tr"           "claude-sonnet-4-7"
make_session "/tmp/mev-searcher"     "Audit atomic_arb_v3"             "Refactoring src/searcher/atomic_arb_v3.rs hot path"        "Edit"  "src/searcher/atomic_arb_v3.rs"              "claude-sonnet-4-7"
make_session "/tmp/eigen-restake"    "Verify Fiat-Shamir"              "Drafting the soundness proof for the FS transcript"        "Task"  "prove transcript Fiat-Shamir soundness"     "claude-opus-4-7"
make_session "/tmp/amm-v4-hooks"     "Wire the hook"                   "Applying SEARCH/REPLACE blocks to contracts/HookV4.sol"    "Edit"  "contracts/HookV4.sol"                       "claude-sonnet-4-7"
make_session "/tmp/kzg-blob-pipe"    "EIP-4844 sim"                    "Writing src/blob_tx_simulator.rs against the latest spec"  "Write" "src/blob_tx_simulator.rs"                   "claude-sonnet-4-7"
make_session "/tmp/erc4337-bundler"  "Validate paymaster"              "analysing UserOperation paymaster validation"              "Read"  "contracts/EntryPointV07.sol"                "claude-sonnet-4-7"
make_session "/tmp/huff-fuzzer"      "Fuzz the batch settlement"       "running forge fuzz --runs 100000 --match-contract"         "Bash"  "forge fuzz --runs 100000"                   "claude-sonnet-4-7"
make_session "/tmp/cosmos-ibc-relay" "Relayer health"                  "(idle 12m08s)"                                              "Bash"  "rly tx link --src-chain-id channel-0"       "claude-sonnet-4-7"
make_session "/tmp/polygon-cdk"      "Validity-proof cadence"          "(idle 47m22s)"                                              "Bash"  "polygon-cdk-prover --batch 18234"           "claude-opus-4-7"
make_session "/tmp/halo2-circuits"   "Circuit prover paused"           "(idle 2h17m)"                                               "Read"  "src/circuit/poseidon.rs"                    "claude-sonnet-4-7"

# ── Spawn fake agent processes inside a user+pid namespace ─────────────────
cat > /tmp/agtop-spawn.py <<'PY'
import os, sys
cwd, label, *args = sys.argv[1:]
os.chdir(cwd)
argv = [label] + list(args) + ["86400"]
os.execve("/usr/bin/sleep", argv, os.environ)
PY

# Driver that runs inside the namespace.  It spawns the fake set, mounts a
# fresh /proc (already done by unshare --mount-proc), then runs agtop via
# pyte and prints the captured ANSI on stdout.
cat > /tmp/agtop-tui-driver.sh <<'SH'
#!/usr/bin/env bash
set -e
# Pre-create all project cwds inside the namespace.
for d in /tmp/zk-rollup-prover /tmp/mev-searcher /tmp/eigen-restake \
         /tmp/amm-v4-hooks /tmp/kzg-blob-pipe /tmp/erc4337-bundler \
         /tmp/huff-fuzzer /tmp/cosmos-ibc-relay /tmp/polygon-cdk \
         /tmp/halo2-circuits /tmp/ollama-svc; do
  mkdir -p "$d"
done
spawn () { python3 /tmp/agtop-spawn.py "$@" & }
spawn /tmp/zk-rollup-prover  claude --dangerously-skip-permissions
spawn /tmp/zk-rollup-prover  claude --dangerously-skip-permissions
spawn /tmp/mev-searcher      codex  --resume
spawn /tmp/eigen-restake     claude --dangerously-skip-permissions
spawn /tmp/amm-v4-hooks      aider  --no-git
spawn /tmp/kzg-blob-pipe     claude
spawn /tmp/erc4337-bundler   gemini --query
spawn /tmp/huff-fuzzer       claude
spawn /tmp/cosmos-ibc-relay  claude
spawn /tmp/polygon-cdk       claude
spawn /tmp/halo2-circuits    claude
spawn /tmp/ollama-svc        ollama serve
spawn /tmp/ollama-svc        ollama serve
sleep 0.5
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
deadline = time.time() + 6
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

# ── Run inside the user+pid namespace ──────────────────────────────────────
AGTOP_BIN="$bin" unshare -U -p -f --mount-proc -r /tmp/agtop-tui-driver.sh \
   > /tmp/fake-tui.ansi
echo "captured $(wc -l < /tmp/fake-tui.ansi) lines / $(wc -c < /tmp/fake-tui.ansi) bytes"

# ── ANSI → HTML → PNG ──────────────────────────────────────────────────────
aha --black --no-header < /tmp/fake-tui.ansi > /tmp/fake-tui.html

cat > /tmp/fake-tui-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0d1014; }
  body { padding: 14px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 12px; padding: 14px 18px;
           box-shadow: 0 6px 28px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.04) inset;
           color: #e1ded7; line-height: 1.18; font-size: 12px; width: fit-content; }
  .titlebar { padding-bottom: 10px; display: flex; gap: 7px; align-items: center;
              border-bottom: 1px solid rgba(255,255,255,0.06); margin-bottom: 12px; }
  .dot { width: 11px; height: 11px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 10px; color: #7d828e; font-size: 11px; letter-spacing: 0.02em; }
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

chromium --headless --no-sandbox --disable-gpu \
  --window-size=2400,1300 --hide-scrollbars \
  --force-device-scale-factor=2 \
  --screenshot="$here/screenshot-tui.png" \
  "file:///tmp/fake-tui-page.html" 2>/dev/null

ls -lh "$here/screenshot-tui.png"
