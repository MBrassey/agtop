#!/usr/bin/env bash
# Build a source-only Debian package and upload it to Launchpad PPA.
#
# Usage:
#   ./packages/ppa/build.sh [SERIES]
#
# SERIES defaults to `noble` (Ubuntu 24.04 LTS).  Pass any active
# Ubuntu series to publish to that pocket: `noble`, `jammy`,
# `oracular`, etc.  Run multiple times for a multi-series PPA.
#
# Requirements (one-time, on your packaging host):
#   sudo apt install devscripts dput-ng debhelper dh-cargo \
#        cargo rustc lintian build-essential
#   gpg --list-secret-keys           # confirm a signing key exists
#   echo "DEBEMAIL=matt@brassey.io"  >> ~/.bashrc
#   echo "DEBFULLNAME=\"Matt Brassey\"" >> ~/.bashrc
#
# Launchpad PPA (one-time):
#   1. Create the PPA at https://launchpad.net/~mbrassey/+activate-ppa
#      Name: agtop  Display: agtop
#   2. Upload your GPG public key to Launchpad:
#      gpg --send-keys <KEYID>           # to keyserver
#      Then paste the keyid at https://launchpad.net/~mbrassey/+editpgpkeys
#   3. Confirm the email Launchpad sends — that activates the key
#      for source uploads.
#
# Each release thereafter: bump debian/changelog (or run this with
# a fresh `dch -i`), commit, then `./packages/ppa/build.sh noble`.
# `dput` will refuse to re-upload an already-published version, so
# re-runs are safe.

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$root"

series="${1:-noble}"
ppa="${PPA_TARGET:-ppa:mbrassey/agtop}"

# Auto-fall-back to a podman/docker container on hosts that don't
# ship the Debian packaging toolchain natively (e.g. Arch / CachyOS
# / Fedora).  PPA_NO_CONTAINER=1 forces native execution; PPA_FORCE_CONTAINER=1
# forces container even if `dch` is on PATH.
if [ "${PPA_FORCE_CONTAINER:-0}" = "1" ] || \
   { [ "${PPA_NO_CONTAINER:-0}" != "1" ] && ! command -v dch >/dev/null 2>&1; }; then
  if [ "${IN_PPA_CONTAINER:-0}" = "1" ]; then
    echo "==> Already inside the container but dch still missing — bug in this script."
    exit 1
  fi
  engine=""
  for cand in podman docker; do
    if command -v "$cand" >/dev/null 2>&1; then engine="$cand"; break; fi
  done
  if [ -z "$engine" ]; then
    cat >&2 <<EOF
==> Debian packaging tools (dch, debuild, dpkg-buildpackage) not on PATH and no container engine found.

Install on Arch / CachyOS:
  yay -S devscripts dput-ng           # AUR

Install on Debian / Ubuntu / Mint / Pop!_OS:
  sudo apt install devscripts dput-ng debhelper cargo rustc lintian build-essential

Or set PPA_FORCE_CONTAINER=1 after installing podman or docker — the
script will run the build inside an ubuntu:${series} image with
all tooling preinstalled.
EOF
    exit 1
  fi
  echo "==> Running build inside ${engine} ubuntu:${series} container"
  # Strategy: export the host's secret key for ${DEBEMAIL} into a
  # tempfile, mount it read-only into the container, and `gpg
  # --import` it inside.  Pre-2.4.10 we tried `cp -a ~/.gnupg`, but
  # gpg's split keybox + agent + dirmngr layout differs subtly
  # between Arch (gnupg 2.4.x) and Ubuntu (gnupg 2.4.x but
  # different distro defaults), and trustdb files copied across
  # are read but not always indexed → 'No secret key' even when
  # the .key file is present.  Export+import sidesteps every
  # variant.
  email="${DEBEMAIL:-matt@brassey.io}"
  if ! gpg --list-secret-keys "$email" >/dev/null 2>&1; then
    echo
    echo "==> No secret key for '$email' in your host GPG."
    echo "    Available secret keys:"
    gpg --list-secret-keys --keyid-format LONG | sed 's/^/      /'
    echo
    echo "    Either set DEBEMAIL to a UID gpg knows, or generate /"
    echo "    import the missing key first."
    exit 1
  fi
  secret_export="$(mktemp -t agtop-ppa-key.XXXXXX.gpg)"
  pubkey_export="$(mktemp -t agtop-ppa-pub.XXXXXX.gpg)"
  trap 'rm -f "$secret_export" "$pubkey_export"' EXIT
  # Drop --batch so gpg-agent can prompt the user for the key
  # passphrase via the host's pinentry (graphical or curses).  Once
  # entered, the agent caches the passphrase for subsequent uses
  # in this run + the configured cache-ttl window.
  echo "==> Exporting signing key for '$email' from host gpg"
  gpg --yes --export-secret-keys "$email" > "$secret_export"
  gpg --yes --export "$email" > "$pubkey_export"
  if ! [ -s "$secret_export" ]; then
    echo "==> gpg --export-secret-keys produced an empty file for '$email'."
    echo "    Likely: the key has no passphrase set OR a smartcard /"
    echo "    yubikey holds the private material (we can't extract that)."
    exit 1
  fi

  uid=$(id -u); gid=$(id -g)
  exec "$engine" run --rm -it \
    -v "$root":/work \
    -v "$secret_export":/secret-key.gpg:ro \
    -v "$pubkey_export":/public-key.gpg:ro \
    -e DEBEMAIL="$email" \
    -e DEBFULLNAME="${DEBFULLNAME:-Matt Brassey}" \
    -e PPA_TARGET="${PPA_TARGET:-ppa:mbrassey/agtop}" \
    -e PPA_REVISION="${PPA_REVISION:-1}" \
    -e IN_PPA_CONTAINER=1 \
    -w /work \
    "ubuntu:${series}" \
    bash -c "
      set -e
      apt-get update -qq >/dev/null
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
        devscripts dput-ng debhelper lintian build-essential \
        gnupg ca-certificates git python3 curl >/dev/null
      # Ubuntu noble ships cargo 1.75; agtop's Cargo.lock is v4.
      # rustup gives us a current toolchain.
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal --no-modify-path >/dev/null
      export PATH=\"/root/.cargo/bin:\$PATH\"
      git config --global --add safe.directory /work
      # Fresh GPG homedir owned by root with loopback pinentry so
      # debsign can sign non-interactively (or interactively if
      # the key has a passphrase).
      mkdir -p /root/.gnupg
      chmod 700 /root/.gnupg
      cat > /root/.gnupg/gpg.conf <<'GPG'
pinentry-mode loopback
GPG
      cat > /root/.gnupg/gpg-agent.conf <<'AGENT'
allow-loopback-pinentry
AGENT
      gpg --batch --import /public-key.gpg >/dev/null 2>&1
      gpg --batch --pinentry-mode loopback --import /secret-key.gpg >/dev/null 2>&1
      # Mark the imported key ultimately trusted so debsign doesn't
      # bail on trust depth.
      keyfp=\$(gpg --list-secret-keys --with-colons '${email_lit:-${DEBEMAIL:-matt@brassey.io}}' | awk -F: '/^fpr:/{print \$10; exit}')
      if [ -n \"\$keyfp\" ]; then
        echo \"\$keyfp:6:\" | gpg --import-ownertrust >/dev/null 2>&1
      fi
      gpg --list-secret-keys
      ./packages/ppa/build.sh '${series}'
    "
fi

# Resolve current version from Cargo.toml so the PPA build always
# tracks the upstream tag, regardless of debian/changelog drift.
version="$(awk -F'"' '/^version[[:space:]]*=/{print $2; exit}' Cargo.toml)"
revision="${PPA_REVISION:-1}"
ppa_version="${version}-${revision}~${series}1"

echo "==> Building source-only PPA upload"
echo "    upstream version : ${version}"
echo "    debian revision  : ${revision}"
echo "    target series    : ${series}"
echo "    full version     : ${ppa_version}"
echo "    PPA              : ${ppa}"

# Prepare a build dir that contains an unpacked upstream tarball
# named agtop-<version>/ alongside the debian/ tree, exactly as
# debuild expects.
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT
src_dir="${build_dir}/agtop-${version}"

echo "==> Staging upstream + debian/ into ${src_dir}"
git archive --format=tar HEAD | (mkdir -p "$src_dir" && cd "$src_dir" && tar -xf -)
# Pristine upstream tarball — must NOT contain debian/.
( cd "$src_dir" && rm -rf debian )

# Vendor every Cargo dep into the upstream tree.  Launchpad PPA
# build farms are network-isolated; without vendored crates the
# build fails with `cargo: failed to fetch` on the first dep.
# The `.cargo/config.toml` redirects all crate fetches to the
# vendor/ tree.  Both files are part of the orig.tar.gz so the
# PPA builder sees a fully self-contained source.
echo "==> Vendoring crates ($(grep -c '^\[\[package\]\]' Cargo.lock 2>/dev/null || echo '?') deps) into source"
# NOTE: do NOT use --quiet — it suppresses the config-toml output
# cargo vendor writes to stdout, which is exactly what we capture
# into .cargo/config.toml.  Stderr stays unredirected so a failure
# (old cargo, lockfile-version mismatch, network issue) shows the
# user the actual error instead of a silent script exit.
( cd "$src_dir" && \
    mkdir -p .cargo && \
    cargo vendor --locked vendor/ > .cargo/config.toml ) || {
  echo
  echo "==> cargo vendor failed.  Common causes:"
  echo "    - apt cargo is older than Cargo.lock's lockfile version"
  echo "      (containerised path now installs rustup; native path"
  echo "       needs cargo >= 1.78 on the host)."
  echo "    - no network access (cargo vendor must reach crates.io)."
  exit 1
}

# Prune vendored crate cruft that lintian flags as DFSG /
# source-is-missing violations:
#   - vendor/*/docs        — pre-rendered rustdoc HTML / JS without source
#   - vendor/*/target      — leftover build artefacts (rare but possible)
#   - vendor/*/benches     — same docs cruft pattern in some crates
#   - vendor/*/lib/lib*.a  — winapi precompiled import libs (binary
#                            archives lintian can't reverse to source)
# After pruning, every crate's .cargo-checksum.json still lists the
# now-deleted files; cargo build would fail integrity check.  Walk
# every checksum file and drop entries whose target no longer exists.
echo "==> Pruning vendored crate cruft + patching checksums"
( cd "$src_dir" && \
    find vendor -type d \( -name docs -o -name target -o -name benches \) -prune -exec rm -rf {} + 2>/dev/null
  # winapi-*-pc-windows-gnu/lib/*.a files are precompiled binary
  # import libraries — lintian rejects them in source packages.
  find vendor -type f -path '*/lib/lib*.a' -delete 2>/dev/null
  python3 -c '
import json, glob, os
for ck in glob.glob("vendor/*/.cargo-checksum.json"):
    crate = os.path.dirname(ck)
    with open(ck) as f: d = json.load(f)
    files = d.get("files", {})
    files = {k: v for k, v in files.items() if os.path.exists(os.path.join(crate, k))}
    d["files"] = files
    with open(ck, "w") as f: json.dump(d, f)
'
)

( cd "$build_dir" && tar --owner=0 --group=0 --numeric-owner -czf "agtop_${version}.orig.tar.gz" "agtop-${version}" )
# Now overlay debian/ for the package build.
git archive --format=tar HEAD debian | (cd "$src_dir" && tar -xf -)

# Mint a per-series changelog stanza so sequential uploads to
# noble + jammy + oracular don't collide.  `dch -v` adds a new top
# entry; we then rewrite the distribution to ${series}.
( cd "$src_dir" && \
  DEBEMAIL="${DEBEMAIL:-matt@brassey.io}" \
  DEBFULLNAME="${DEBFULLNAME:-Matt Brassey}" \
  dch --newversion "${ppa_version}" \
      --distribution "${series}" \
      --force-bad-version \
      "Build for ${series}." || true )

# Source-only build, sign with GPG.  -sa forces inclusion of the
# .orig.tar.gz on every upload (Launchpad rejects subsequent
# uploads that omit it for a brand-new ${ppa_version}).
( cd "$src_dir" && debuild -S -sa -d )

changes="${build_dir}/agtop_${ppa_version}_source.changes"
[ -f "$changes" ] || { echo "expected $changes — debuild output mismatch"; ls "$build_dir"; exit 1; }

echo "==> Uploading to ${ppa}"
dput "${ppa}" "${changes}"

echo "==> Done.  Watch the build at:"
echo "    https://launchpad.net/~mbrassey/+archive/ubuntu/agtop/+packages"
