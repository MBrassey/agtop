# Contributing

Thanks for considering a contribution.  agtop is a Rust + ratatui project;
there's no special setup beyond the Rust toolchain.

## Dev loop

```sh
cargo build                # debug, fast
cargo test                 # unit tests
cargo run                  # full TUI
cargo run -- --once        # one-shot snapshot
cargo run -- --json | jq   # JSON for scripting
cargo clippy               # lint
cargo fmt --all            # format
```

## Adding a built-in agent matcher

Edit `src/matchers.rs` and add a regex.  Add a classification test in the
same file's `#[cfg(test)]` block so future regex tweaks don't silently break
detection.

## Adding a new vendor's session reader

Model it on `src/codex.rs`.  Each module exports
`summarise(live_agents, now_ms) -> SessionsResult`.  Slot the call into
`src/collector.rs::enrich_and_score` alongside the existing readers.

The collector merges all readers with `sessions::merge()`, so order matters
only for tie-breaking the same PID's enrichment (first-wins).

## Releasing

```sh
# 1. bump version
sed -i 's/^version = .*/version = "X.Y.Z"/' Cargo.toml

# 2. local build + tests
cargo build --release && cargo test --release

# 3. rebuild distribution packages
packages/pacman/build.sh
packages/deb/build.sh
packages/npm/build.sh

# 4. tag + push
git commit -am "vX.Y.Z" && git tag vX.Y.Z && git push --tags

# 5. .github/workflows/release.yml will:
#    - build prebuilt binaries for linux x86_64 / linux aarch64 / macos x86_64
#      / macos aarch64 / windows x86_64
#    - attach them to the GitHub Release
#    - publish to crates.io if CRATES_IO_TOKEN secret is set
```

## Distribution

| Channel       | How                                                                |
| ------------- | ------------------------------------------------------------------ |
| **crates.io** | `cargo publish` (set `CRATES_IO_TOKEN` GH secret to automate)      |
| **AUR**       | Push `packages/pacman/PKGBUILD` to `ssh://aur@aur.archlinux.org/agtop.git` after `makepkg --printsrcinfo > .SRCINFO` |
| **Homebrew**  | Push `homebrew/agtop.rb` to a tap repo; bump `url` + `sha256` per release |
| **apt PPA**   | Upload `packages/deb/agtop_*.deb` to a Launchpad PPA               |
| **npm**       | `npm publish packages/npm/agtop-*.tgz`                             |

The npm package is a thin shim that downloads the prebuilt binary from
GitHub Releases (or falls back to `cargo install agtop`).
