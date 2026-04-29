# agtop — npm package

The npm distribution of **agtop** wraps the static Rust binary. On install
the postinstall script either downloads the prebuilt binary from GitHub
Releases (once cut) or falls back to `cargo install agtop` if the user has
Rust on PATH.

## Build

```sh
./build.sh
```

Produces `agtop-<version>.tgz` here. Inspect it with:

```sh
tar -tzf agtop-0.2.0.tgz
```

## Publish

```sh
npm publish ./agtop-0.2.0.tgz --access public
```

## Install (end users)

```sh
npm install -g agtop
agtop                  # launch TUI
agtop --once --top 10  # one-shot snapshot
```

If you don't have a prebuilt binary yet (or are on an unusual platform), the
postinstall script will run `cargo install agtop` for you. Install Rust from
[rustup.rs](https://rustup.rs) if you don't have it.

## Long-term plan

Once we cut GitHub Releases (`v0.2.0`, …) with prebuilt artefacts named
`agtop-<platform>-<arch>` for the standard targets, the postinstall fetches
those over HTTPS and unpacks into `vendor/<platform>-<arch>/agtop`. The
`bin/agtop` shim already looks in that path first.
