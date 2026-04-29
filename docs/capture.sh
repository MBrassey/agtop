#!/usr/bin/env bash
# Regenerate the README screenshots from the synthetic-data scripts.
#
# Both screenshots are publish-safe: they render output produced by the
# Python generators in this directory (fake_once.py, fake_tui.py), not the
# live binary, so no real session content / cwds / PIDs leak into git.
#
# Requires: aha (ANSI->HTML) and chromium (HTML->PNG headless render).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

command -v aha       >/dev/null || { echo "aha not on PATH"; exit 1; }
command -v chromium  >/dev/null || { echo "chromium not on PATH"; exit 1; }

# Common HTML chrome (window dots + monospace font + dark background).
make_page () {
  local title="$1" body_html="$2" font_size="$3" line_height="$4" out="$5"
  cat > "$out" <<HEAD
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0c0e12; }
  body { padding: 24px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 10px; padding: 16px 20px;
           box-shadow: 0 8px 30px rgba(0,0,0,0.6); color: #e1ded7;
           line-height: ${line_height}; font-size: ${font_size}; width: fit-content; }
  .titlebar { padding-bottom: 12px; display: flex; gap: 8px; align-items: center; }
  .dot { width: 12px; height: 12px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 12px; color: #7d828e; font-size: 11px; }
  pre { margin: 0; white-space: pre; color: inherit; background: transparent; font: inherit; }
</style></head><body>
<div class="frame">
  <div class="titlebar"><div class="dot r"></div><div class="dot y"></div><div class="dot g"></div>
  <span class="label">${title}</span></div>
<pre>
HEAD
  cat "$body_html" >> "$out"
  echo '</pre></div></body></html>' >> "$out"
}

# ── --once snapshot ────────────────────────────────────────────────────────
python3 "$here/fake_once.py" > /tmp/fake-once.ansi
aha --black --no-header < /tmp/fake-once.ansi > /tmp/fake-once.html
make_page "agtop --once --top 15" /tmp/fake-once.html 13px 1.36 /tmp/fake-once-page.html
chromium --headless --no-sandbox --disable-gpu \
  --window-size=1820,720 --hide-scrollbars \
  --force-device-scale-factor=1.5 \
  --screenshot="$here/screenshot-once.png" \
  "file:///tmp/fake-once-page.html" 2>/dev/null

# ── full TUI snapshot ──────────────────────────────────────────────────────
python3 "$here/fake_tui.py" > /tmp/fake-tui.ansi
aha --black --no-header < /tmp/fake-tui.ansi > /tmp/fake-tui.html
make_page "agtop" /tmp/fake-tui.html 13px 1.18 /tmp/fake-tui-page.html
chromium --headless --no-sandbox --disable-gpu \
  --window-size=2400,1300 --hide-scrollbars \
  --force-device-scale-factor=1.5 \
  --screenshot="$here/screenshot-tui.png" \
  "file:///tmp/fake-tui-page.html" 2>/dev/null

ls -lh "$here"/screenshot-*.png
