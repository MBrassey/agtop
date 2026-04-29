#!/usr/bin/env bash
# Build an Arch Linux .pkg.tar.zst for agtop using the PKGBUILD in this folder.
# Stages a fresh source tarball so makepkg has something to verify, then
# invokes makepkg in this directory.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

version="$(node -p 'require("'"$root"'/package.json").version')"
pkgname="agtop"
tar_name="${pkgname}-${version}.tar.gz"

# Create a clean source tree (no node_modules, no build artefacts).
work="$here/.stage"
rm -rf "$work"
mkdir -p "$work/${pkgname}-${version}"
for f in src bin package.json README.md LICENSE .gitignore; do
  if [ -e "$root/$f" ]; then
    cp -r "$root/$f" "$work/${pkgname}-${version}/"
  fi
done

( cd "$work" && tar --owner=0 --group=0 -czf "$here/$tar_name" "${pkgname}-${version}" )
rm -rf "$work"

# Make sure makepkg's own scratch dirs don't pile up between runs.
rm -rf "$here/src" "$here/pkg"
rm -f "$here/${pkgname}-${version}-"*-*.pkg.tar.zst

# Patch pkgver in PKGBUILD to match package.json on every build.
sed -i "s/^pkgver=.*/pkgver=${version}/" "$here/PKGBUILD"

cd "$here"
makepkg --force --nodeps --skipinteg --syncdeps --noconfirm 2>&1 | tail -20 || {
  # If --syncdeps prompts (sudo), retry without it — assume deps are present.
  makepkg --force --nodeps --skipinteg --noconfirm
}

echo
echo "built artifact(s):"
ls -la "$here"/*.pkg.tar.zst 2>/dev/null || { echo "no pkg.tar.zst produced" >&2; exit 1; }

echo
echo "package contents (first 30):"
zst="$(ls -1t "$here"/*.pkg.tar.zst | head -1)"
tar -I zstd -tf "$zst" | head -30
echo "total entries: $(tar -I zstd -tf "$zst" | wc -l)"
