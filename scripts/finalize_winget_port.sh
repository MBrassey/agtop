#!/usr/bin/env bash
# Submit agtop to microsoft/winget-pkgs.
#
# Workflow:
#   1. Fetch the v${VER} GitHub Release SHA256SUMS, extract the
#      windows zip's hash.
#   2. Patch the local manifests in ~/code/agtop-winget-port/.
#   3. Fork microsoft/winget-pkgs if no fork exists.
#   4. Sparse-checkout JUST the manifests/ subtree we need
#      (winget-pkgs is enormous — ~400 K files).
#   5. Drop the three manifests at
#      manifests/m/MBrassey/agtop/${VER}/.
#   6. Commit, push to fork, open PR.
#
# Idempotent on re-run: existing fork / branch is reused.
#
# Required: gh (logged in), curl, git ≥ 2.27 for partial-clone.
set -euo pipefail

VER="${1:-2.3.2}"
MANIFEST_SRC="$HOME/code/agtop-winget-port"
WORK="$HOME/code/winget-pkgs-fork"
PUBLISHER="MBrassey"
PACKAGE="agtop"
PKG_PATH="manifests/m/${PUBLISHER}/${PACKAGE}/${VER}"

[ -d "$MANIFEST_SRC" ] || {
  echo "missing $MANIFEST_SRC — run agtop port-prep step first"; exit 2; }

echo "== Step 1: fetch SHA256SUMS for v${VER} =="
SUMS=$(curl -sfL "https://github.com/MBrassey/agtop/releases/download/v${VER}/SHA256SUMS")
# Strict end-anchored match so a sibling line for e.g.
# `agtop-windows-x86_64.zip.sig` couldn't be picked up by accident.
SHA=$(echo "$SUMS" | awk '$2 ~ /^\*?agtop-windows-x86_64\.zip$/ {print toupper($1); exit}')
[ -n "$SHA" ] || { echo "no windows zip in SHA256SUMS"; exit 2; }
echo "  windows zip sha256: $SHA"

echo "== Step 2: patch manifests =="
sed -i "s|^PackageVersion:.*|PackageVersion: ${VER}|" \
  "$MANIFEST_SRC/MBrassey.agtop.yaml" \
  "$MANIFEST_SRC/MBrassey.agtop.installer.yaml" \
  "$MANIFEST_SRC/MBrassey.agtop.locale.en-US.yaml"
sed -i \
  -e "s|/v[0-9.]\\+/agtop-windows|/v${VER}/agtop-windows|" \
  -e "s|^    InstallerSha256:.*|    InstallerSha256: ${SHA}|" \
  "$MANIFEST_SRC/MBrassey.agtop.installer.yaml"

echo "== Step 3: fork microsoft/winget-pkgs (or reuse) =="
if ! gh repo view "${PUBLISHER}/winget-pkgs" >/dev/null 2>&1; then
  gh repo fork microsoft/winget-pkgs --clone=false --default-branch-only
fi

echo "== Step 4: sparse-checkout fork (only needed paths) =="
if [ ! -d "$WORK/.git" ]; then
  # winget-pkgs is ~400K files / multi-GB; partial clone gives us a
  # working tree without the historical blobs.
  git clone --depth=1 --filter=blob:none --no-checkout \
    "https://github.com/${PUBLISHER}/winget-pkgs.git" "$WORK"
  cd "$WORK"
  git sparse-checkout init --cone
  git sparse-checkout set "manifests/m/${PUBLISHER}"
  git checkout master
else
  cd "$WORK"
  git fetch --depth=1 origin master
fi
git checkout -B "agtop-${VER}" origin/master

echo "== Step 5: drop manifests in =="
mkdir -p "$PKG_PATH"
cp "$MANIFEST_SRC/MBrassey.agtop.yaml"               "$PKG_PATH/"
cp "$MANIFEST_SRC/MBrassey.agtop.installer.yaml"     "$PKG_PATH/"
cp "$MANIFEST_SRC/MBrassey.agtop.locale.en-US.yaml"  "$PKG_PATH/"

echo "== Step 6: commit =="
git config user.name  "mbrassey"
git config user.email "matt@brassey.io"
git add "$PKG_PATH"
if git diff --cached --quiet; then
  echo "no changes — manifests already at v${VER}"
  exit 0
fi
git commit -m "New version: ${PUBLISHER}.${PACKAGE} version ${VER}"

echo "== Step 7: push branch =="
git push -u origin "agtop-${VER}" --force

echo "== Step 8: open PR =="
gh pr create --repo microsoft/winget-pkgs \
  --base master --head "${PUBLISHER}:agtop-${VER}" \
  --title "New version: ${PUBLISHER}.${PACKAGE} version ${VER}" \
  --body "$(cat <<BODY
## ${PUBLISHER}.${PACKAGE} ${VER}

Adds agtop, a process monitor for AI coding agents, to winget.

agtop walks /proc on Linux (sysinfo on Windows) plus the on-disk
session transcripts of Claude Code, OpenAI Codex, Block Goose,
Aider, and Google Gemini, and reports per-agent CPU, RSS, status,
current tool, in-flight subagents, token usage, estimated cost,
context-window fill, and loaded skills in a top-style TUI.

### Windows-specific notes

- Writable-FD enumeration uses \`NtQuerySystemInformation\`
  (\`SystemExtendedHandleInformation\`) + \`DuplicateHandle\` +
  \`GetFinalPathNameByHandleW\` to surface what files each agent
  has open. Source: \`src/writing_files.rs\`.
- Per-process IO byte counters come from sysinfo's
  \`Process::disk_usage().total_*\`.
- Build matrix runs \`cargo test --release\` on windows-latest
  every push; the writable-FD self-test exercises the native
  path against the test process's own open file descriptor.

### Source / metadata

- Upstream: https://github.com/MBrassey/agtop
- License: MIT
- Installer: portable zip from the official GitHub Release
- Architecture: x64
- SHA256 verified against the release's SHA256SUMS

#### Author / Manifest checklist

- [x] I have read the [contributing guide](https://github.com/microsoft/winget-pkgs/blob/master/CONTRIBUTING.md)
- [x] Manifests follow the v1.6.0 schema
- [x] Installer URL points at the official GitHub Release
- [x] InstallerSha256 matches the release's SHA256SUMS
- [x] Three manifest files: version, installer, locale.en-US
- [x] Maintainer is the upstream author

I'm the upstream maintainer (\`matt@brassey.io\`) — happy to iterate
on any review feedback.
BODY
)"

echo
echo "✓ PR opened.  Track with:"
echo "    gh pr list --repo microsoft/winget-pkgs --author MBrassey"
