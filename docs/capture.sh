#!/usr/bin/env bash
# Regenerate the README hero screenshot from the synthetic-data emitter.
#
# fake_once.py emits ANSI byte-identical to `agtop --once`'s formatter,
# but with synthetic agent rows so the screenshot ships publish-safe.
# Single screenshot — perfect alignment from the real Rust formatter,
# rendered through aha + chromium headless.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

command -v aha       >/dev/null || { echo "aha not on PATH"; exit 1; }
command -v chromium  >/dev/null || { echo "chromium not on PATH"; exit 1; }

python3 "$here/fake_once.py" > /tmp/fake-once.ansi
aha --black --no-header < /tmp/fake-once.ansi > /tmp/fake-once.html

cat > /tmp/fake-once-page.html <<'HEAD'
<!doctype html>
<html><head><meta charset="utf-8"><style>
  html, body { margin: 0; padding: 0; background: #0d1014; }
  body { padding: 14px; font-family: "JetBrains Mono","SF Mono","Menlo","DejaVu Sans Mono",monospace; }
  .frame { background: #14171c; border-radius: 12px; padding: 14px 22px 18px;
           box-shadow: 0 6px 28px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.04) inset;
           color: #e1ded7; line-height: 1.42; font-size: 14px; width: fit-content; }
  .titlebar { padding-bottom: 12px; display: flex; gap: 7px; align-items: center;
              border-bottom: 1px solid rgba(255,255,255,0.06); margin-bottom: 14px; }
  .dot { width: 11px; height: 11px; border-radius: 50%; }
  .r { background: #ff5f57; } .y { background: #febc2e; } .g { background: #28c840; }
  .label { margin-left: 10px; color: #7d828e; font-size: 12px; letter-spacing: 0.02em; }
  pre { margin: 0; white-space: pre; color: inherit; background: transparent; font: inherit; }
</style></head><body>
<div class="frame">
  <div class="titlebar">
    <div class="dot r"></div><div class="dot y"></div><div class="dot g"></div>
    <span class="label">agtop  ·  one-shot snapshot</span>
  </div>
<pre>
HEAD
cat /tmp/fake-once.html >> /tmp/fake-once-page.html
echo '</pre></div></body></html>' >> /tmp/fake-once-page.html

chromium --headless --no-sandbox --disable-gpu \
  --window-size=1900,720 --hide-scrollbars \
  --force-device-scale-factor=2 \
  --screenshot="$here/screenshot.png" \
  "file:///tmp/fake-once-page.html" 2>/dev/null

ls -lh "$here/screenshot.png"
