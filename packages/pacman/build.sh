#!/usr/bin/env bash
# Build an Arch Linux .pkg.tar.zst for agtop using the PKGBUILD in this folder.
# Stages a clean source tarball that includes Cargo.toml + Cargo.lock + src/
# so makepkg can do a real `cargo build --release` in fakeroot.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

version="$(awk -F'"' '/^version[[:space:]]*=/{print $2; exit}' "$root/Cargo.toml")"
pkgname="agtop"
tar_name="${pkgname}-${version}.tar.gz"

# Stage a clean source tree (no target/, no node_modules from the legacy run).
work="$here/.stage"
rm -rf "$work"
mkdir -p "$work/${pkgname}-${version}"
for f in src Cargo.toml Cargo.lock README.md LICENSE; do
  if [ -e "$root/$f" ]; then
    cp -r "$root/$f" "$work/${pkgname}-${version}/"
  fi
done

# Cargo.lock might not exist if the user only did `cargo check`. Generate one.
if [ ! -f "$work/${pkgname}-${version}/Cargo.lock" ]; then
  ( cd "$root" && cargo generate-lockfile >/dev/null 2>&1 )
  cp "$root/Cargo.lock" "$work/${pkgname}-${version}/"
fi

( cd "$work" && tar --owner=0 --group=0 -czf "$here/$tar_name" "${pkgname}-${version}" )
rm -rf "$work"

rm -rf "$here/src" "$here/pkg"
rm -f  "$here/${pkgname}-${version}-"*-*.pkg.tar.zst

# Patch pkgver in PKGBUILD to match Cargo.toml on every build.
sed -i "s/^pkgver=.*/pkgver=${version}/" "$here/PKGBUILD"

cd "$here"
# --nodeps because we don't want to require root for sudo pacman -S during
# local development; the maintainer is expected to have rust + cargo.
makepkg --force --nodeps --skipinteg --skipchecksums --noconfirm 2>&1 | tail -30

echo
echo "built artifact(s):"
ls -la "$here"/*.pkg.tar.zst 2>/dev/null || { echo "no pkg.tar.zst produced" >&2; exit 1; }

echo
echo "package contents (first 20):"
zst="$(ls -1t "$here"/*.pkg.tar.zst | head -1)"
tar -I zstd -tf "$zst" | head -20
echo "total entries: $(tar -I zstd -tf "$zst" | wc -l)"
