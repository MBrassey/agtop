#!/usr/bin/env bash
# Regenerate the README screenshots from the live binary.
#
# Requires: aha, chromium, python3 with pyte (in a venv at /tmp/agtop-venv).
# Outputs:
#   docs/screenshot-once.png   one-shot table view
#   docs/screenshot-tui.png    full ratatui frame
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
bin="$root/target/release/agtop"

[ -x "$bin" ] || { echo "build the release binary first: cargo build --release"; exit 1; }
command -v aha       >/dev/null || { echo "aha not on PATH";       exit 1; }
command -v chromium  >/dev/null || { echo "chromium not on PATH";  exit 1; }
[ -x /tmp/agtop-venv/bin/python ] || {
  python3 -m venv /tmp/agtop-venv
  /tmp/agtop-venv/bin/pip install --quiet pyte
}

mkdir -p "$here"

# ── 1. --once snapshot ─────────────────────────────────────────────────────
"$bin" --once --top 14 --iterations 2 --interval 0.5 > /tmp/agtop-once.ansi
awk '/^---$/{p=1; next} p' /tmp/agtop-once.ansi > /tmp/agtop-once-real.ansi
aha --black --no-header < /tmp/agtop-once-real.ansi > /tmp/agtop-once.html

cat > /tmp/agtop-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0c0e12; }
  body { padding: 28px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 10px; padding: 18px 22px;
           box-shadow: 0 8px 30px rgba(0,0,0,0.6); color: #e1ded7;
           line-height: 1.36; font-size: 13px; width: fit-content; }
  .titlebar { padding-bottom: 14px; display: flex; gap: 8px; align-items: center; }
  .dot { width: 12px; height: 12px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 12px; color: #7d828e; font-size: 12px; }
  pre { margin: 0; white-space: pre; color: inherit; background: transparent; font: inherit; }
</style></head><body>
<div class="frame">
  <div class="titlebar">
    <div class="dot r"></div><div class="dot y"></div><div class="dot g"></div>
    <span class="label">agtop --once --top 14</span>
  </div>
<pre>
HEAD
cat /tmp/agtop-once.html >> /tmp/agtop-page.html
echo '</pre></div></body></html>' >> /tmp/agtop-page.html

chromium --headless --no-sandbox --disable-gpu \
  --window-size=1700,560 --hide-scrollbars \
  --force-device-scale-factor=1.5 \
  --screenshot="$here/screenshot-once.png" \
  "file:///tmp/agtop-page.html" 2>/dev/null

# ── 2. Full TUI snapshot (pyte replays the pty stream) ─────────────────────
cat > /tmp/capture_tui.py <<'PY'
import os, pty, select, struct, fcntl, termios, time, sys
import pyte
COLS, ROWS = 220, 56
screen = pyte.Screen(COLS, ROWS)
stream = pyte.ByteStream(screen)
pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.environ["TERM"] = "xterm-256color"
    os.environ["COLORTERM"] = "truecolor"
    os.execv(sys.argv[1], ["agtop", "--interval", "0.5"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
deadline = time.time() + 6
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.1)
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
    if len(c) == 6 and all(ch in "0123456789abcdef" for ch in c.lower()):
        try:
            r = int(c[0:2], 16); g = int(c[2:4], 16); b = int(c[4:6], 16)
            return f"\x1b[{base};2;{r};{g};{b}m"
        except ValueError: return ""
    names = {"black":30,"red":31,"green":32,"brown":33,"blue":34,"magenta":35,"cyan":36,"white":37}
    return f"\x1b[{names[c] + (10 if bg else 0)}m" if c in names else ""

out = []
for y in range(ROWS):
    bits = []; lf = lb = ""; lbold = False
    for x in range(COLS):
        ch = screen.buffer[y][x]
        fg = col(ch.fg, False); bg = col(ch.bg, True)
        if (fg, bg, ch.bold) != (lf, lb, lbold):
            bits.append("\x1b[0m")
            if ch.bold: bits.append("\x1b[1m")
            bits.append(fg + bg)
            lf, lb, lbold = fg, bg, ch.bold
        bits.append(ch.data if ch.data else " ")
    bits.append("\x1b[0m")
    out.append("".join(bits).rstrip())
print("\n".join(out))
PY

/tmp/agtop-venv/bin/python /tmp/capture_tui.py "$bin" > /tmp/agtop-tui.ansi
aha --black --no-header < /tmp/agtop-tui.ansi > /tmp/agtop-tui.html

cat > /tmp/agtop-tui-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0c0e12; }
  body { padding: 24px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 10px; padding: 16px 18px;
           box-shadow: 0 8px 30px rgba(0,0,0,0.6); color: #e1ded7;
           line-height: 1.18; font-size: 13px; width: fit-content; }
  .titlebar { padding-bottom: 12px; display: flex; gap: 8px; align-items: center; }
  .dot { width: 12px; height: 12px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 12px; color: #7d828e; font-size: 11px; }
  pre { margin: 0; white-space: pre; color: inherit; background: transparent; font: inherit; }
</style></head><body>
<div class="frame">
  <div class="titlebar">
    <div class="dot r"></div><div class="dot y"></div><div class="dot g"></div>
    <span class="label">agtop</span>
  </div>
<pre>
HEAD
cat /tmp/agtop-tui.html >> /tmp/agtop-tui-page.html
echo '</pre></div></body></html>' >> /tmp/agtop-tui-page.html

chromium --headless --no-sandbox --disable-gpu \
  --window-size=2400,1080 --hide-scrollbars \
  --force-device-scale-factor=1.5 \
  --screenshot="$here/screenshot-tui.png" \
  "file:///tmp/agtop-tui-page.html" 2>/dev/null

ls -lh "$here"/screenshot-*.png
