# agtop — npm package

This is the npm publishing wrapper for **agtop**, a terminal UI for monitoring AI
coding agents on your system. The actual source lives at the repo root; this
folder only carries the npm build script and publish notes.

## Build

```sh
./build.sh
```

This produces `agtop-<version>.tgz` in this folder by running `npm pack` against
the repo root and copying the tarball here. The tarball is what `npm publish`
uploads — install it locally to test:

```sh
npm install -g ./agtop-0.1.0.tgz
agtop --help
```

## Publish

```sh
cd ../..               # repo root
npm publish --access public
```

The `package.json` `files` array restricts what's shipped to `bin/`, `src/`,
`README.md`, and `LICENSE` — no `node_modules`, tests, or packaging metadata.

## Install (end users)

```sh
npm install -g agtop
agtop                  # launch TUI
agtop --once --top 10  # one-shot snapshot
```
