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
  "name": "@mbrassey/agtop",
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
  "os": ["linux", "darwin", "win32"]
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
const crypto = require("node:crypto");
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

const archiveExt = process.platform === "win32" ? ".zip" : ".tar.gz";
const target    = `agtop-${platform}-${arch}`;
const baseUrl   = `https://github.com/mbrassey/agtop/releases/download/v${pkg.version}`;
const url       = `${baseUrl}/${target}${archiveExt}`;
const sumsUrl   = `${baseUrl}/SHA256SUMS`;
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
    "    yay -S agtop                   # Arch / CachyOS",
    "    sudo apt install agtop         # Debian / Ubuntu (when wired up)",
    "    brew install mbrassey/tap/agtop  # macOS / Linux (when wired up)",
    ""
  ].join("\n"));
  // Soft-fail so `npm install` exits 0 instead of breaking the user's setup.
  process.exit(0);
}

// Follow up to 5 redirects, deliver final-response body to `cb(buf)`.
function fetchBuffer(u, cb, hops = 0) {
  if (hops > 5) return cb(null);
  https.get(u, { headers: { "User-Agent": "agtop-npm-installer" } }, (res) => {
    if ([301, 302, 307, 308].includes(res.statusCode) && res.headers.location) {
      res.resume();
      return fetchBuffer(res.headers.location, cb, hops + 1);
    }
    if (res.statusCode !== 200) {
      res.resume();
      return cb(null);
    }
    const chunks = [];
    res.on("data", (c) => chunks.push(c));
    res.on("end", () => cb(Buffer.concat(chunks)));
    res.on("error", () => cb(null));
  }).on("error", () => cb(null));
}

// Parse a `<sha256>  <filename>` SHA256SUMS file into a {file: sha} map.
function parseSums(text) {
  const out = {};
  for (const line of String(text).split(/\r?\n/)) {
    const m = line.match(/^([0-9a-fA-F]{64})\s+\*?(\S+)\s*$/);
    if (m) out[m[2]] = m[1].toLowerCase();
  }
  return out;
}

function verifyAndExtract(buf, expectedSha) {
  const got = crypto.createHash("sha256").update(buf).digest("hex");
  if (got !== expectedSha) {
    console.warn(`agtop: SHA256 mismatch for ${target}${archiveExt}`);
    console.warn(`        expected ${expectedSha}`);
    console.warn(`        got      ${got}`);
    return cargoFallback();
  }
  fs.mkdirSync(vendorDir, { recursive: true });
  const archivePath = path.join(os.tmpdir(), `agtop-${process.pid}${archiveExt}`);
  fs.writeFileSync(archivePath, buf);
  let r;
  if (archiveExt === ".zip") {
    // Use PowerShell on Windows; tar -xf works too on Windows 10+ but is
    // not always present on older builds.
    r = spawnSync("powershell", [
      "-NoProfile", "-Command",
      `Expand-Archive -Force -Path "${archivePath}" -DestinationPath "${vendorDir}"`,
    ], { stdio: "ignore" });
  } else {
    r = spawnSync("tar", [
      "-xzf", archivePath, "-C", vendorDir,
      "--strip-components=1",
      "--no-overwrite-dir",
      "--no-same-owner",
      "--no-same-permissions",
    ], { stdio: "ignore" });
  }
  try { fs.unlinkSync(archivePath); } catch {}
  if (r.status !== 0) {
    console.warn("agtop: archive extraction failed");
    return cargoFallback();
  }
  // Windows zip preserves the leading dir; flatten it.
  if (archiveExt === ".zip") {
    const inner = path.join(vendorDir, target);
    if (fs.existsSync(inner)) {
      for (const f of fs.readdirSync(inner)) {
        // Defense-in-depth: reject any entry name that contains
        // path separators or `..` segments before the rename, so
        // a malicious zip-with-traversal can't escape vendorDir
        // even if the upstream SHA256 verification is somehow
        // bypassed.  Names from a release zip are flat agtop
        // binaries; anything fancy is wrong.
        if (f === "" || f === "." || f === ".." ||
            f.includes("/") || f.includes("\\") ||
            f.includes("..")) {
          console.warn("agtop: rejecting suspicious archive entry " + JSON.stringify(f));
          return cargoFallback();
        }
        const dest = path.resolve(vendorDir, f);
        if (!dest.startsWith(path.resolve(vendorDir) + path.sep)) {
          console.warn("agtop: archive entry " + JSON.stringify(f) + " escapes vendorDir");
          return cargoFallback();
        }
        fs.renameSync(path.join(inner, f), dest);
      }
      try { fs.rmdirSync(inner); } catch {}
    }
  }
  const bin = path.join(vendorDir, "agtop" + ext);
  if (!fs.existsSync(bin)) {
    console.warn("agtop: binary not found at " + bin);
    return cargoFallback();
  }
  try { fs.chmodSync(bin, 0o755); } catch {}
  console.log(`agtop ${pkg.version} installed at ${bin}  (sha256 verified)`);
  process.exit(0);
}

// Two-phase: fetch SHA256SUMS first so we can verify the binary tarball
// before trusting any of its bytes.  Both fetches go to github.com over
// TLS; the SHA256SUMS file itself is signed implicitly by the GitHub
// Release tag (only the repo owner can publish a vX.Y.Z release).
fetchBuffer(sumsUrl, (sumsBuf) => {
  if (!sumsBuf) {
    console.warn("agtop: couldn't fetch SHA256SUMS; refusing to install unverified binary");
    return cargoFallback();
  }
  const sums = parseSums(sumsBuf.toString("utf8"));
  const expected = sums[`${target}${archiveExt}`];
  if (!expected) {
    console.warn(`agtop: no SHA entry for ${target}${archiveExt} in SHA256SUMS`);
    return cargoFallback();
  }
  fetchBuffer(url, (buf) => {
    if (!buf) {
      console.warn(`agtop: couldn't fetch ${url}`);
      return cargoFallback();
    }
    verifyAndExtract(buf, expected);
  });
});
POST

cp "$root/README.md" "$build/README.md" 2>/dev/null || true
cp "$root/LICENSE"   "$build/LICENSE"   2>/dev/null || true

cd "$build"
# Friendly user-facing tarball name regardless of whether the package is
# scoped (npm pack would write `blueprint.xyz-agtop-X.Y.Z.tgz` for a scoped
# name).  We capture whatever it actually wrote and rename to a stable name.
tar_name="agtop-${version}.tgz"
out_dir="$(mktemp -d)"
( cd "$out_dir" && npm pack "$build" --silent >/dev/null )
produced="$(ls -1 "$out_dir"/*.tgz | head -1)"
mv "$produced" "$here/$tar_name"
rm -rf "$out_dir"

echo "built $here/$tar_name"
echo
echo "verify contents:"
tar -tzf "$here/$tar_name"
