# agtop — Arch Linux package

This folder builds an Arch Linux `.pkg.tar.zst` for **agtop** using `makepkg`
and `cargo build --release`.

## Build

```sh
./build.sh
```

The script:

1. stages a clean source tarball `agtop-<version>.tar.gz` (Cargo.toml,
   Cargo.lock, src/, README, LICENSE);
2. patches `pkgver` in `PKGBUILD` to match `Cargo.toml`;
3. runs `makepkg --force --nodeps --skipinteg`.

Output: `agtop-<version>-1-x86_64.pkg.tar.zst`.

## Install

```sh
sudo pacman -U ./agtop-0.2.0-1-x86_64.pkg.tar.zst
agtop --help
```

Removal:

```sh
sudo pacman -R agtop
```

## Layout on disk

```
/usr/bin/agtop                # static Rust binary
/usr/share/licenses/agtop/    # LICENSE
/usr/share/doc/agtop/         # README
```

## Submitting to the AUR

For AUR publishing, swap the local `source=()` for a real release tarball:

```sh
source=("agtop-${pkgver}.tar.gz::https://github.com/mbrassey/agtop/archive/v${pkgver}.tar.gz")
sha256sums=('<sha256 of the release tarball>')
```

Then create the AUR package with `makepkg --printsrcinfo > .SRCINFO` and push
to `ssh://aur@aur.archlinux.org/agtop.git`.
