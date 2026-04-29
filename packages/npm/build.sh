#!/usr/bin/env bash
# Build the npm tarball for agtop.
# Output: packages/npm/agtop-<version>.tgz
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

cd "$root"
version="$(node -p 'require("./package.json").version')"

# `npm pack` writes into the cwd, so build it inside this folder.
cd "$here"
npm pack "$root" --silent >/dev/null

tarball="agtop-${version}.tgz"
if [[ ! -f "$tarball" ]]; then
  echo "build.sh: expected $tarball but it was not produced" >&2
  exit 1
fi

echo "built $here/$tarball"
echo
echo "verify contents:"
tar -tzf "$tarball" | head -20
