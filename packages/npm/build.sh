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

# postinstall: download the prebuilt binary from the GitHub Release matching
# the package version, falling back to `cargo install` if download fails.
cat > "$build/scripts/install.js" <<'POST'
"use strict";
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const https = require("node:https");
const os = require("node:os");
const pkg = require(path.join(__dirname, "..", "package.json"));

const platMap = { linux: "linux", darwin: "macos", win32: "windows" };
const archMap = { x64: "x86_64", arm64: "aarch64" };
const platform = platMap[process.platform];
const arch = archMap[process.arch];
const ext = process.platform === "win32" ? ".exe" : "";

if (!platform || !arch) {
  console.warn(`agtop: no prebuilt for ${process.platform}/${process.arch}; trying cargo`);
  return cargoFallback();
}

const target = `agtop-${platform}-${arch}`;
const url = `https://github.com/mbrassey/agtop/releases/download/v${pkg.version}/${target}.tar.gz`;
const vendorDir = path.join(__dirname, "..", "vendor", `${platform}-${arch}`);

function which(cmd) {
  const probe = spawnSync(process.platform === "win32" ? "where" : "which", [cmd], { stdio: "pipe" });
  return probe.status === 0 ? String(probe.stdout || "").trim().split("\n")[0] : null;
}

function cargoFallback() {
  if (which("cargo")) {
    const r = spawnSync("cargo", ["install", "--locked", "agtop"], { stdio: "inherit" });
    if (r.status === 0) process.exit(0);
  }
  console.error([
    "",
    "  agtop: couldn't fetch the prebuilt binary and cargo isn't available.",
    "  Install Rust (https://rustup.rs) and re-run, or use:",
    "    sudo pacman -U agtop-*.pkg.tar.zst",
    "    sudo apt   install ./agtop_*.deb",
    "    brew install agtop  (after `brew tap mbrassey/tap`)",
    ""
  ].join("\n"));
  // Soft-fail so `npm install` exits 0 instead of breaking the user's setup.
  process.exit(0);
}

function download(u, hops = 0) {
  if (hops > 5) return cargoFallback();
  https.get(u, { headers: { "User-Agent": "agtop-npm-installer" } }, (res) => {
    if ([301, 302, 307, 308].includes(res.statusCode) && res.headers.location) {
      res.resume();
      return download(res.headers.location, hops + 1);
    }
    if (res.statusCode !== 200) {
      console.warn(`agtop: HTTP ${res.statusCode} fetching ${u}`);
      res.resume();
      return cargoFallback();
    }
    fs.mkdirSync(vendorDir, { recursive: true });
    const tarPath = path.join(os.tmpdir(), `agtop-${process.pid}.tar.gz`);
    const w = fs.createWriteStream(tarPath);
    res.pipe(w);
    w.on("finish", () => {
      const r = spawnSync("tar", ["-xzf", tarPath, "-C", vendorDir, "--strip-components=1"], { stdio: "ignore" });
      try { fs.unlinkSync(tarPath); } catch {}
      if (r.status !== 0) {
        console.warn("agtop: tar extraction failed");
        return cargoFallback();
      }
      const bin = path.join(vendorDir, "agtop" + ext);
      if (!fs.existsSync(bin)) {
        console.warn("agtop: binary not found at " + bin);
        return cargoFallback();
      }
      try { fs.chmodSync(bin, 0o755); } catch {}
      console.log(`agtop ${pkg.version} installed at ${bin}`);
      process.exit(0);
    });
    w.on("error", () => cargoFallback());
  }).on("error", () => cargoFallback());
}

download(url);
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
