# agtop — Arch Linux package

This folder builds an Arch Linux `.pkg.tar.zst` for **agtop** using `makepkg`.

## Build

```sh
./build.sh
```

The script:

1. stages a clean source tarball `agtop-<version>.tar.gz` in this folder
   (so `makepkg` has something to verify, even for local builds);
2. patches `pkgver` in `PKGBUILD` to match `package.json`;
3. runs `makepkg --force --nodeps --skipinteg`.

Output: `agtop-<version>-1-any.pkg.tar.zst`.

## Install

```sh
sudo pacman -U ./agtop-0.1.0-1-any.pkg.tar.zst
agtop --help
```

Removal:

```sh
sudo pacman -R agtop
```

## Layout on disk

```
/usr/bin/agtop                # launcher: exec node /usr/lib/agtop/bin/agtop
/usr/lib/agtop/bin/agtop      # node entrypoint
/usr/lib/agtop/src/...        # JS source
/usr/lib/agtop/node_modules/  # bundled production deps
/usr/share/licenses/agtop/    # LICENSE
/usr/share/doc/agtop/         # README
```

## Submitting to the AUR

This `PKGBUILD` is a starting point. To publish on the AUR you'll want to
swap the local `source=()` for a real release tarball, e.g.:

```sh
source=("agtop-${pkgver}.tar.gz::https://github.com/mbrassey/agtop/archive/v${pkgver}.tar.gz")
sha256sums=('<sha256 of the release tarball>')
```

Then create the AUR package with `mksrcinfo > .SRCINFO` and push to
`ssh://aur@aur.archlinux.org/agtop.git`.
