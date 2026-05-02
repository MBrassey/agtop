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
( cd "$src_dir" && debuild -S -sa )

changes="${build_dir}/agtop_${ppa_version}_source.changes"
[ -f "$changes" ] || { echo "expected $changes — debuild output mismatch"; ls "$build_dir"; exit 1; }

echo "==> Uploading to ${ppa}"
dput "${ppa}" "${changes}"

echo "==> Done.  Watch the build at:"
echo "    https://launchpad.net/~mbrassey/+archive/ubuntu/agtop/+packages"
