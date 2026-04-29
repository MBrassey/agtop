#!/usr/bin/env bash
# Build a Debian/Ubuntu .deb for agtop, wrapping the static Rust binary.
#
# Layout produced:
#   /usr/bin/agtop                       # static binary
#   /usr/share/doc/agtop/README.md       # docs
#   /usr/share/doc/agtop/copyright       # MIT
#   /usr/share/man/...                   # (none yet — TODO)
#
# Uses dpkg-deb when available; falls back to a pure ar+tar builder so the
# .deb can be produced on any Linux without installing dpkg.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
build="$here/build"
stage="$build/agtop"

version="$(awk -F'"' '/^version[[:space:]]*=/{print $2; exit}' "$root/Cargo.toml")"

# Build the release binary if it isn't already there.
if [[ ! -x "$root/target/release/agtop" || "$root/Cargo.toml" -nt "$root/target/release/agtop" ]]; then
  ( cd "$root" && cargo build --release )
fi

rm -rf "$build"
mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/doc/agtop"

install -m 0755 "$root/target/release/agtop" "$stage/usr/bin/agtop"
install -m 0644 "$root/LICENSE" "$stage/usr/share/doc/agtop/copyright" 2>/dev/null || true
[ -f "$root/README.md" ] && install -m 0644 "$root/README.md" "$stage/usr/share/doc/agtop/README.md"

# Patch architecture in the control file to match the build host so
# the .deb installs cleanly without --force-architecture.
arch="$(dpkg --print-architecture 2>/dev/null || uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
sed -e "s/^Version: .*/Version: ${version}/" \
    -e "s/^Architecture: .*/Architecture: ${arch}/" \
    "$here/DEBIAN/control" > "$stage/DEBIAN/control"

size_kb=$(du -sk "$stage" --exclude=DEBIAN | awk '{print $1}')
printf 'Installed-Size: %s\n' "$size_kb" >> "$stage/DEBIAN/control"

( cd "$stage" && find . -type f ! -path './DEBIAN/*' -print0 \
  | xargs -0 md5sum 2>/dev/null \
  | sed 's| \./| |' > DEBIAN/md5sums ) || true

deb_name="agtop_${version}_${arch}.deb"
out="$here/$deb_name"
rm -f "$out"

if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "$stage" "$out" >/dev/null
  echo "built (via dpkg-deb): $out"
else
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
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb -I "$out"
  echo
  dpkg-deb -c "$out"
else
  ar t "$out"
fi
