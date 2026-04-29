#!/usr/bin/env bash
# Build a Debian/Ubuntu .deb package for agtop.
#
# Layout produced:
#   /usr/lib/agtop/          # bundled JS + node_modules
#   /usr/bin/agtop           # launcher (symlink-ish wrapper)
#   /usr/share/doc/agtop/    # README, LICENSE, copyright
#
# Uses dpkg-deb when available; falls back to a pure ar+tar builder so the
# .deb can be produced on any Linux without installing dpkg.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
build="$here/build"
stage="$build/agtop"

version="$(node -p 'require("'"$root"'/package.json").version')"

rm -rf "$build"
mkdir -p "$stage/DEBIAN"
mkdir -p "$stage/usr/lib/agtop"
mkdir -p "$stage/usr/bin"
mkdir -p "$stage/usr/share/doc/agtop"

# Sync source.
cp -r "$root/src" "$stage/usr/lib/agtop/"
cp -r "$root/bin" "$stage/usr/lib/agtop/"
cp "$root/package.json" "$stage/usr/lib/agtop/"

# Production node_modules — don't ship dev deps or the .bin shims for them.
( cd "$root" && npm install --omit=dev --prefix "$stage/usr/lib/agtop" --no-audit --no-fund --loglevel=error \
    blessed blessed-contrib commander >/dev/null )
# npm leaves a package-lock.json under the prefix; remove it to keep the deb tight.
rm -f "$stage/usr/lib/agtop/package-lock.json"

# Launcher.
cat > "$stage/usr/bin/agtop" <<'LAUNCHER'
#!/bin/sh
exec /usr/bin/env node /usr/lib/agtop/bin/agtop "$@"
LAUNCHER
chmod 0755 "$stage/usr/bin/agtop"

# Docs.
cp "$root/LICENSE" "$stage/usr/share/doc/agtop/copyright" 2>/dev/null || true
[ -f "$root/README.md" ] && cp "$root/README.md" "$stage/usr/share/doc/agtop/README.md"

# Control file with patched Version.
sed "s/^Version: .*/Version: ${version}/" "$here/DEBIAN/control" > "$stage/DEBIAN/control"

# Compute installed-size (KB) for the control file.
size_kb=$(du -sk "$stage" --exclude=DEBIAN | awk '{print $1}')
printf 'Installed-Size: %s\n' "$size_kb" >> "$stage/DEBIAN/control"

# md5sums (optional but expected by lintian).
( cd "$stage" && find . -type f ! -path './DEBIAN/*' -print0 \
  | xargs -0 md5sum 2>/dev/null \
  | sed 's| \./| |' > DEBIAN/md5sums ) || true

deb_name="agtop_${version}_all.deb"
out="$here/$deb_name"
rm -f "$out"

if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "$stage" "$out" >/dev/null
  echo "built (via dpkg-deb): $out"
else
  # Manual builder: a .deb is an `ar` archive of three members in this order:
  #   debian-binary, control.tar.gz, data.tar.gz
  echo "dpkg-deb not found; building manually with ar+tar"
  tmp="$build/_build"
  rm -rf "$tmp" && mkdir -p "$tmp"
  echo "2.0" > "$tmp/debian-binary"
  ( cd "$stage/DEBIAN" && tar --owner=0 --group=0 -czf "$tmp/control.tar.gz" . )
  ( cd "$stage" && tar --owner=0 --group=0 --exclude='./DEBIAN' -czf "$tmp/data.tar.gz" . )
  ( cd "$tmp" && ar rc "$out" debian-binary control.tar.gz data.tar.gz )
  echo "built (manual ar): $out"
fi

echo
echo "package contents:"
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb -I "$out"
  echo
  dpkg-deb -c "$out" | head -20
else
  ar t "$out"
  echo "(install dpkg to inspect with dpkg-deb -I / -c)"
fi
