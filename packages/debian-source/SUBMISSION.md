# Debian source-package submission for agtop

## What's here

Debian-policy-compliant source packaging for agtop, ready for upload
to Debian's `unstable` archive via a Debian Developer sponsor.

  control            Source + binary package definitions, build-deps
  copyright          MIT license declaration in machine-readable format
  rules              dh-cargo build rules
  changelog          Initial-release entry
  source/format      `3.0 (quilt)` source format

## Submission flow

The user (= upstream maintainer) cannot upload directly to Debian
without Debian Maintainer (DM) status.  Steps:

1. **File the ITP bug** at https://bugs.debian.org/wnpp:
   ```
   Subject: ITP: agtop -- process monitor for AI coding agents
   ```
   The bug number that comes back replaces `#ITP-PENDING` in the
   `changelog` entry above.

2. **Find a sponsor** via:
   - https://lists.debian.org/debian-mentors/
   - #debian-mentors on OFTC (IRC)
   - https://mentors.debian.net (after step 3, upload there)

3. **Build a source tarball + dsc** locally:
   ```sh
   cd /home/matt/code/agtop
   ./packages/debian-source/build-source-pkg.sh   # see below; produces .dsc + tarball
   dput mentors agtop_2.3.5-1_source.changes
   ```

4. **The sponsor reviews + uploads to unstable**.  Lintian must be
   clean.  Reproducible-build hash should match.

5. After the package lands in `unstable`, the auto-sync to Ubuntu
   `universe` happens at the start of each Ubuntu release cycle;
   Linux Mint inherits from Ubuntu.  ETA: weeks to months for the
   first sync, days for subsequent updates.

## Embedded sources / vendored crates

agtop has ~250 transitive Rust crates.  Two paths Debian accepts:

- **Per-crate packaging**: every crate gets its own `librust-foo-dev`
  binary package.  `control`'s Build-Depends already lists the
  *direct* deps; the transitive set is roughly 10x larger and
  many already exist in Debian's archive.  `cargo-debstatus` will
  show what's missing.

- **Embedded sources**: bundle the vendored crates into the source
  tarball.  Debian's `cargo-vendor-filterer` produces a minimal
  `vendor/` directory.  Document in `debian/missing-sources` and
  argue for an exemption to the "no embedded code" policy on the
  basis that the crates are pinned in `Cargo.lock` and reviewed by
  upstream.  Common path for Rust applications without 250
  pre-existing librust-* packages.

The packaging in this directory is configured for the per-crate path
(via dh-cargo).  Switch to embedded sources by:
- Adding `vendor/` to the source tarball
- Adding `vendor.d/agtop.json` cargo config that points cargo at
  the vendored deps
- Removing the `librust-*-dev` build-deps from `control`

## Status of this submission

- Files prepared: 2026-05-01
- ITP bug: not yet filed
- Sponsor: not yet found
- mentors.debian.net upload: not yet done
- Status: **awaiting upstream maintainer to file the ITP**

## Why this is on hold

The ITP filing requires a debian.org Bugzilla account interaction
that needs the upstream maintainer's identity (email + GPG signature
on the report).  Cannot be automated from CI without delegating the
maintainer's identity, so it lives as a manual step the user runs
once.  After the ITP bug number is assigned, the rest of the
workflow (build source tarball, upload to mentors.debian.net) can
be scripted and added to release.yml as a reminder.
