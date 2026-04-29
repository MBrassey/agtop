# agtop — Debian / Ubuntu package

This folder builds a `.deb` for **agtop** that ships the static Rust binary.

## Build

```sh
./build.sh
```

This will compile the release binary if needed (`cargo build --release`) and
produce `agtop_<version>_<arch>.deb` here. If `dpkg-deb` is on PATH it's used;
otherwise the script falls back to a pure `ar` + `tar` builder (a `.deb` is
just an `ar` archive containing `debian-binary`, `control.tar.gz`,
`data.tar.gz`).

## Install

```sh
sudo apt install ./agtop_0.2.0_amd64.deb
agtop --help
```

Removal:

```sh
sudo apt remove agtop
```

## Layout on disk

```
/usr/bin/agtop                # static Rust binary (~3 MB stripped)
/usr/share/doc/agtop/         # README, copyright
```

No runtime dependencies — the binary is statically linked against musl-friendly
crates and only needs a working terminal.

## Submitting upstream

To get this into the official Debian / Ubuntu repos you'll need to follow the
[Debian New Maintainer Guide](https://www.debian.org/doc/manuals/maint-guide/)
and rework the source under `debian/` with `debhelper` / `dh-cargo` and a
proper `debian/changelog`. The package built here is a binary `.deb` suitable
for direct distribution and PPA hosting.
