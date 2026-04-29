#!/usr/bin/env bash
# Build the npm tarball for agtop.
#
# As of v0.2.0 agtop is a Rust binary, so the npm package is a thin shim that
# downloads the right prebuilt binary from GitHub Releases at install time.
# Until we cut a real GitHub Release, the postinstall falls back to running
# `cargo install agtop` if cargo is on PATH.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

version="$(awk -F'"' '/^version[[:space:]]*=/{print $2; exit}' "$root/Cargo.toml")"

build="$here/build"
rm -rf "$build"
mkdir -p "$build"

cat > "$build/package.json" <<EOF
{
  "name": "agtop",
  "version": "${version}",
  "description": "Terminal UI for monitoring AI coding agents — like top, but for agents.",
  "keywords": ["agent","ai","monitor","tui","terminal","top","htop","btop","claude","codex","aider","cursor","observability"],
  "homepage": "https://github.com/mbrassey/agtop",
  "bugs": "https://github.com/mbrassey/agtop/issues",
  "repository": "github:mbrassey/agtop",
  "license": "MIT",
  "author": "Matt Brassey <matt@brassey.io>",
  "bin": { "agtop": "bin/agtop" },
  "files": ["bin/", "scripts/", "README.md", "LICENSE"],
  "scripts": { "postinstall": "node scripts/install.js" },
  "engines": { "node": ">=16" },
  "os": ["linux", "darwin"]
}
EOF

mkdir -p "$build/bin" "$build/scripts"
# bin shim: locate the platform binary installed by the postinstall, and exec it.
cat > "$build/bin/agtop" <<'SHIM'
#!/usr/bin/env node
"use strict";
const { spawnSync } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");
const candidates = [
  path.join(__dirname, "..", "vendor", process.platform + "-" + process.arch, "agtop"),
  path.join(process.env.HOME || "", ".cargo", "bin", "agtop"),
  "/usr/local/bin/agtop",
  "/usr/bin/agtop",
];
for (const c of candidates) {
  try { if (fs.existsSync(c)) { const r = spawnSync(c, process.argv.slice(2), { stdio: "inherit" }); process.exit(r.status ?? 1); } } catch {}
}
console.error("agtop: binary not found. Run `cargo install agtop` or download a release from\nhttps://github.com/mbrassey/agtop/releases");
process.exit(127);
SHIM
chmod +x "$build/bin/agtop"

# postinstall: prefer prebuilt binary from GH Releases (once published);
# otherwise try `cargo install agtop` and fall back with instructions.
cat > "$build/scripts/install.js" <<'POST'
"use strict";
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const pkg = require(path.join(__dirname, "..", "package.json"));

function which(cmd) {
  const probe = spawnSync(process.platform === "win32" ? "where" : "which", [cmd], { stdio: "pipe" });
  return probe.status === 0 ? String(probe.stdout || "").trim().split("\n")[0] : null;
}

// 1) Prebuilt binary from GitHub Releases would go here once we cut one.
// const url = `https://github.com/mbrassey/agtop/releases/download/v${pkg.version}/agtop-${process.platform}-${process.arch}`;
// (left as a TODO — no code that runs without a real release)

// 2) Fall back to `cargo install`.
if (which("cargo")) {
  const r = spawnSync("cargo", ["install", "--locked", "agtop"], { stdio: "inherit" });
  if (r.status === 0) process.exit(0);
}

console.error([
  "",
  "  agtop's npm distribution requires either a prebuilt GitHub Release",
  "  (coming soon) or a working Rust toolchain so it can `cargo install agtop`.",
  "",
  "  Install Rust from https://rustup.rs and re-run, or use:",
  "    pacman -U agtop-*.pkg.tar.zst   (Arch)",
  "    apt install ./agtop_*.deb       (Debian / Ubuntu)",
  ""
].join("\n"));
process.exit(0); // soft-fail so npm install doesn't error out hard
POST

cp "$root/README.md" "$build/README.md" 2>/dev/null || true
cp "$root/LICENSE"   "$build/LICENSE"   2>/dev/null || true

cd "$build"
tar_name="agtop-${version}.tgz"
( cd .. && rm -f "$tar_name" && npm pack "$build" --silent >/dev/null )
mv "../$tar_name" "$here/$tar_name"

echo "built $here/$tar_name"
echo
echo "verify contents:"
tar -tzf "$here/$tar_name"
