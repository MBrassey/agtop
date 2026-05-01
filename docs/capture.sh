#!/usr/bin/env bash
# Regenerate the README detail-popup screenshot.
#
# Same synthetic setup as capture_tui_synthetic.sh (PID-namespace,
# spoofed-cmdline spinners, rich JSONL transcripts), but the capture
# driver presses <Enter> after the TUI has populated so the agent
# detail popup is open in the screenshot — showing the per-agent
# header, current tool / task, in-flight subagents, recent activity,
# and the live transcript preview.
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

# Re-use the JSONL transcripts and spinner binary that
# capture_tui_synthetic.sh creates.  Run that script first so the
# /tmp/agtop-demo-home/ tree, /tmp/agtop-spin, /tmp/agtop-spawn.py
# and /tmp/agtop-tui-driver.sh are in place — but stash whatever
# screenshot-tui.png it produced, since we only want screenshot.png
# changed by this run.
[ -f "$here/screenshot-tui.png" ] && cp "$here/screenshot-tui.png" /tmp/screenshot-tui.bak
bash "$here/capture_tui_synthetic.sh" >/dev/null
[ -f /tmp/screenshot-tui.bak ] && mv /tmp/screenshot-tui.bak "$here/screenshot-tui.png"

# ── Capture driver: open agtop, wait for population, then send <Enter>
#    to open the detail popup for the (top-of-list) busiest agent.
cat > /tmp/agtop-dialog-capture.py <<'PY'
import os, pty, select, struct, fcntl, termios, time, sys
import pyte
# Smaller grid for the dialog screenshot so the centered detail
# popup (capped at 100×32 internally) dominates the rendered image
# instead of being a small inset in a wide TUI.
COLS, ROWS = 130, 40
screen = pyte.Screen(COLS, ROWS); stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.environ["TERM"] = "xterm-256color"; os.environ["COLORTERM"] = "truecolor"
    os.execv(sys.argv[1], ["agtop", "--interval", "0.4"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

# Phase 1: let the TUI populate for long enough that the per-pid
# context-history ring (driven by the background JSONL appender)
# has enough samples for the popup's time-to-compaction
# extrapolation to fire.  At 0.4s tick × 100 ticks ≈ 40s, with
# the appender adding a record every 4s, that's ~10 growth
# samples — well past the 3-sample minimum.
end_warmup = time.time() + 40
while time.time() < end_warmup:
    r,_,_ = select.select([fd],[],[],0.1)
    if r:
        try: data = os.read(fd, 65536)
        except OSError: break
        if not data: break
        stream.feed(data)

# Phase 2: drive selection to the highest-CPU agent (zk-rollup-
# prover at 82%):
#   1. `g` toggles project grouping OFF (default-on grouping puts
#      the alphabetical first project at top regardless of sort).
#   2. `s` cycles smart→cpu so CPU-desc is the active order.
#   3. `Home` jumps the selection to the new top row (selection
#      otherwise sticks to the same PID across re-sorts).
#   4. Enter opens the detail popup on that row.
for k in [b"g", b"s"]:
    try: os.write(fd, k)
    except OSError: pass
    time.sleep(0.6)
try: os.write(fd, b"\x1b[H")   # ESC[H == Home
except OSError: pass
time.sleep(0.6)
try: os.write(fd, b"\r")
except OSError: pass
end_capture = time.time() + 4
while time.time() < end_capture:
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

# Re-use the same namespace/spawn setup, but swap the capture script
# the driver runs from agtop-tui-capture.py to agtop-dialog-capture.py.
cat > /tmp/agtop-dialog-driver.sh <<'SH'
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

spin () {
  local cwd="$1" pct="$2" mb="$3"; shift 3
  local label="$*"
  # Redirect stdin/out/err to /dev/null so the spinner's inherited
  # FDs don't end up as `writing_files` noise (the parent driver
  # script's stdout is /tmp/fake-dialog.ansi, which would otherwise
  # show up in the popup).
  ( cd "$cwd" && \
      AGTOP_PCT="$pct" AGTOP_MB="$mb" \
      AGTOP_WRITE_FILE="${cwd}/circuits/main.rs" \
      exec -a "$label" /tmp/agtop-spin >/dev/null 2>&1 </dev/null ) &
}
for d in /tmp/zk-rollup-prover /tmp/mev-searcher /tmp/eigen-restake \
         /tmp/amm-v4-hooks /tmp/kzg-blob-pipe /tmp/erc4337-bundler \
         /tmp/huff-fuzzer /tmp/uniswap-v4-core /tmp/lido-csm \
         /tmp/optimism-bedrock /tmp/arbitrum-stylus /tmp/celestia-da \
         /tmp/scroll-zkevm /tmp/risc0-prover /tmp/sp1-zkvm /tmp/foundry-suite; do
  mkdir -p "$d/circuits"
done
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

spawn () { python3 /tmp/agtop-spawn.py "$@" & }
spawn /tmp/cosmos-ibc-relay   claude
spawn /tmp/polygon-cdk        claude
spawn /tmp/halo2-circuits     claude
spawn /tmp/ollama-svc         ollama serve
spawn /tmp/ollama-svc         ollama serve

# Background context-growth appender for the hero session — see the
# matching block in capture_tui_synthetic.sh for rationale.  Drives
# the popup's "≈ Xm to compaction (+N/min)" line.
HERO_JSONL=/tmp/agtop-demo-home/.claude/projects/-tmp-zk-rollup-prover/sess.jsonl
(
  ctx=510000
  i=20
  while sleep 4; do
    ts=$(date -u +"2026-02-14T09:14:%S.000Z" 2>/dev/null || echo "2026-02-14T09:14:30.000Z")
    ctx=$((ctx + 14000))
    cat >> "$HERO_JSONL" <<APPEND
{"type":"assistant","timestamp":"${ts}","message":{"id":"msg_${i}","model":"claude-opus-4-7","content":[{"type":"text","text":"Continuing the proof — building the next aggregation layer"}],"usage":{"input_tokens":4,"output_tokens":420,"cache_read_input_tokens":${ctx},"cache_creation_input_tokens":2400}}}
APPEND
    i=$((i + 1))
  done
) &

sleep 2.5
HOME=/tmp/agtop-demo-home /tmp/agtop-venv/bin/python /tmp/agtop-dialog-capture.py "$AGTOP_BIN"
SH
chmod +x /tmp/agtop-dialog-driver.sh

AGTOP_BIN="$bin" unshare -U -p -f --mount-proc -r /tmp/agtop-dialog-driver.sh \
   > /tmp/fake-dialog.ansi
echo "captured $(wc -l < /tmp/fake-dialog.ansi) lines / $(wc -c < /tmp/fake-dialog.ansi) bytes"

aha --black --no-header < /tmp/fake-dialog.ansi > /tmp/fake-dialog.html

cat > /tmp/fake-dialog-page.html <<'HEAD'
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
    <span class="label">agtop  ·  agent detail popup</span>
  </div>
<pre>
HEAD
cat /tmp/fake-dialog.html >> /tmp/fake-dialog-page.html
echo '</pre></div></body></html>' >> /tmp/fake-dialog-page.html

chromium --headless --no-sandbox --disable-gpu \
  --hide-scrollbars --force-device-scale-factor=2 \
  --window-size=2000,1200 \
  --virtual-time-budget=500 \
  --screenshot="$here/screenshot.png" \
  "file:///tmp/fake-dialog-page.html" 2>/dev/null

if command -v magick >/dev/null 2>&1; then
  magick "$here/screenshot.png" \
    -bordercolor "#0d1014" -border 1 \
    -fuzz 2% -trim +repage \
    -bordercolor "#0d1014" -border 20x20 \
    "$here/screenshot.png"
elif command -v convert >/dev/null 2>&1; then
  convert "$here/screenshot.png" \
    -bordercolor "#0d1014" -border 1 \
    -fuzz 2% -trim +repage \
    -bordercolor "#0d1014" -border 20x20 \
    "$here/screenshot.png"
fi

ls -lh "$here/screenshot.png"
