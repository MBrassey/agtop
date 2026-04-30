# Homebrew tap for agtop

This formula compiles agtop from source via `cargo install`.  Once published
to a tap repo (e.g. `mbrassey/homebrew-tap`), users can install with:

```sh
brew tap mbrassey/tap
brew install agtop
```

## Maintainer setup (one-time)

1. Create a public repo named `homebrew-tap` under your account.
2. Copy `agtop.rb` into `Formula/agtop.rb` in that repo.
3. After every tagged release on the `agtop` repo, update the formula's
   `url` and `sha256`:

   ```sh
   curl -L https://github.com/mbrassey/agtop/archive/refs/tags/vX.Y.Z.tar.gz | shasum -a 256
   ```

   …and commit + push to the tap.

For prebuilt-binary distribution (no Rust toolchain required), point `url`
at a release artifact from `.github/workflows/release.yml` instead and
switch the `install` block to `bin.install "agtop"`.
