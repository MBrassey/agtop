# agtop — Launchpad PPA

This folder ships the source-only build script that uploads agtop
to the Ubuntu PPA at `ppa:mbrassey/agtop`.  PPAs build the binary
themselves on Launchpad's farm — we only push a signed source
package; Launchpad rebuilds it for every active Ubuntu series we
target.

## One-time setup

1. **Activate the PPA on Launchpad**

   Browse to <https://launchpad.net/~mbrassey/+activate-ppa> and
   create a PPA named `agtop`.  The full URL becomes
   `https://launchpad.net/~mbrassey/+archive/ubuntu/agtop`.

2. **Register your GPG signing key with Launchpad**

   ```sh
   gpg --list-secret-keys --keyid-format LONG
   gpg --send-keys <KEYID>            # publish to keyserver
   ```

   Paste the KEYID at
   <https://launchpad.net/~mbrassey/+editpgpkeys>.  Launchpad
   sends a test email encrypted with that key — decrypt it
   (`gpg -d`) and follow the confirmation link.  After that, your
   key can sign uploads.

3. **Install host build tools**

   On Debian / Ubuntu / Mint / Pop!_OS:

   ```sh
   sudo apt install devscripts dput-ng debhelper \
        cargo rustc lintian build-essential
   ```

   On Arch / CachyOS:

   ```sh
   yay -S devscripts dput-ng           # AUR — packages aren't in extra
   sudo pacman -S dpkg lintian          # extra
   # rustc + cargo: already installed if you build agtop locally.
   ```

   Alternatively, install `podman` (or `docker`) and skip native
   tooling — `packages/ppa/build.sh` auto-detects missing `dch` and
   re-runs itself inside an `ubuntu:${series}` container that has
   everything pre-installed.  Force this path explicitly with
   `PPA_FORCE_CONTAINER=1`.

   `dh-cargo` is intentionally NOT installed.  Launchpad PPA builders
   have no network access during the build phase, so dh-cargo's
   default behaviour (resolve crates against crates.io / Debian's
   `librust-*-dev` archive) doesn't work for a private rust app.
   `packages/ppa/build.sh` instead vendors every crate into the
   `orig.tar.gz` via `cargo vendor`, and `debian/rules` drives
   `cargo build --release --frozen --offline` directly.

4. **Set maintainer identity**

   ```sh
   echo 'export DEBEMAIL="matt@brassey.io"'       >> ~/.bashrc
   echo 'export DEBFULLNAME="Matt Brassey"'       >> ~/.bashrc
   ```

## Per-release

```sh
# Bump debian/changelog (only if you want a fresh debian rev,
# not strictly necessary — build.sh mints a per-series stanza
# automatically).
dch -i
git add debian/changelog && git commit -m "debian: changelog 2.4.x"

# Build + upload for one Ubuntu series.  Only noble (24.04) and
# newer ship a recent-enough rustc for our crate tree (ratatui
# 0.30 + crossterm 0.29 need rust >= 1.74).  jammy (22.04) ships
# rust 1.61 and won't build — skip it.
./packages/ppa/build.sh noble        # 24.04 LTS — supported
./packages/ppa/build.sh oracular     # 24.10     — supported
./packages/ppa/build.sh plucky       # 25.04     — supported
```

The script:
- builds an `orig.tar.gz` from the current `HEAD` (no `debian/`)
- overlays `debian/` for the package build
- mints a per-series stanza in `debian/changelog`
  (`<upstream>-<rev>~<series>1`)
- runs `debuild -S -sa` to produce a signed source package
- runs `dput ppa:mbrassey/agtop` to upload

Launchpad emails build status when each series finishes — green
means the package is in `https://ppa.launchpad.net/mbrassey/agtop/ubuntu/`
and `apt install agtop` works for any user who's added the PPA.

## End-user install

```sh
sudo add-apt-repository ppa:mbrassey/agtop
sudo apt update
sudo apt install agtop
```

PPA users get automatic updates through `apt upgrade` once the
PPA source is added — no separate signing-key dance like the
self-hosted apt repo at `mbrassey.github.io/apt`.
