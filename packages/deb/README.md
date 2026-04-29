# agtop — Debian / Ubuntu package

This folder builds a `.deb` for **agtop**.

## Build

```sh
./build.sh
```

Produces `agtop_<version>_all.deb` here. The script uses `dpkg-deb` when
available and falls back to a pure `ar` + `tar` builder otherwise (a `.deb`
is just an `ar` archive containing `debian-binary`, `control.tar.gz`,
`data.tar.gz`).

## Install

```sh
sudo apt install ./agtop_0.1.0_all.deb
agtop --help
```

Removal:

```sh
sudo apt remove agtop
```

## Layout on disk

```
/usr/bin/agtop                # launcher: exec node /usr/lib/agtop/bin/agtop
/usr/lib/agtop/bin/agtop      # node entrypoint
/usr/lib/agtop/src/...        # JS source
/usr/lib/agtop/node_modules/  # bundled production deps (blessed, commander, …)
/usr/share/doc/agtop/         # README, copyright
```

## Submitting upstream

To get this into the official Debian / Ubuntu repos you'll need to follow the
[Debian New Maintainer Guide](https://www.debian.org/doc/manuals/maint-guide/)
and rework the source under `debian/` with `debhelper` and a proper
`debian/changelog`. The package built here is a binary `.deb` suitable for
direct distribution and PPA hosting — it isn't a source package.
