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

3. **Install host build tools** (Debian / Ubuntu / Mint / Pop!_OS)

   ```sh
   sudo apt install devscripts dput-ng debhelper dh-cargo \
        cargo rustc lintian build-essential
   ```

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

# Build + upload for one Ubuntu series.
./packages/ppa/build.sh noble        # 24.04 LTS

# Repeat for older series if you want broad coverage.
./packages/ppa/build.sh jammy        # 22.04 LTS
./packages/ppa/build.sh oracular     # 24.10
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
