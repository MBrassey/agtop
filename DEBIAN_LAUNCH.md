# Getting agtop into the official Debian archive + Ubuntu PPA

This doc lists exactly what *you* need to do, in order, to ship
agtop through both channels.  Everything that can be prepared in
the repo has been: `debian/`, `packages/ppa/build.sh`, the ITP
draft at `debian/ITP.txt`, and the `ppa.yml` CI workflow that
sanity-checks the source package on every push.

The two routes are independent.  PPA is fast (days) and you
control it.  Official Debian is slow (weeks) and goes through
sponsor + ftpmaster review.  Run them in parallel — the same
`debian/` tree drives both.

---

## Route A — Ubuntu PPA  (fast, ~1 day, you control it)

### A1.  Install the build host once

On any Ubuntu / Debian machine you'll be uploading from:

```sh
sudo apt install devscripts dput-ng debhelper dh-cargo \
     cargo rustc lintian build-essential
echo 'export DEBEMAIL="matt@brassey.io"' >> ~/.bashrc
echo 'export DEBFULLNAME="Matt Brassey"'  >> ~/.bashrc
source ~/.bashrc
```

### A2.  Activate the Launchpad PPA

1. Log in to <https://launchpad.net> with your account
   (`mbrassey` is assumed; if your Launchpad handle is different,
   change `ppa:mbrassey/agtop` references in
   `packages/ppa/build.sh` and `packages/ppa/README.md`).
2. Browse to <https://launchpad.net/~mbrassey/+activate-ppa>.
3. Name: `agtop`.  Display name: `agtop`.  Click **Activate**.
4. The PPA URL becomes
   `https://launchpad.net/~mbrassey/+archive/ubuntu/agtop`.

### A3.  Register your GPG signing key with Launchpad

```sh
gpg --list-secret-keys --keyid-format LONG
# pick the long key id of the key you'll sign with, e.g.
# `FC8BF673587134A114B205A0632F0658B478942A`
gpg --send-keys FC8BF673587134A114B205A0632F0658B478942A
```

Then on Launchpad, paste the key fingerprint at
<https://launchpad.net/~mbrassey/+editpgpkeys>.  Launchpad emails
a test message encrypted to that key; decrypt and click the
confirmation link.  This usually takes 5 minutes.

### A4.  First upload

```sh
cd ~/code/agtop
./packages/ppa/build.sh noble       # 24.04 LTS
./packages/ppa/build.sh jammy       # 22.04 LTS  — optional
./packages/ppa/build.sh oracular    # 24.10      — optional
```

Each call:
- builds an unsigned `.orig.tar.gz` from `HEAD`
- mints a per-series stanza in `debian/changelog`
- signs the source package with your GPG key
- `dput`s it to `ppa:mbrassey/agtop`

Launchpad emails build status when each series finishes (typically
20-40 min on their farm).  Green ⇒ the package is live.

### A5.  Tell users how to install

After the first green build:

```sh
sudo add-apt-repository ppa:mbrassey/agtop
sudo apt update
sudo apt install agtop
```

Add this to README.md's install table once Launchpad confirms.

---

## Route B — Official Debian  (slow, ~4-8 weeks, lots of waiting)

### B1.  File the ITP bug

The "Intent To Package" bug at <https://bugs.debian.org/wnpp> is
the public announcement that you intend to maintain agtop in
Debian.  Required before anything else.

```sh
sudo apt install reportbug
reportbug wnpp
```

When prompted, paste the body of `debian/ITP.txt`.  Once the
bug is filed:
- you get a number, e.g. `#1099999`
- replace `Closes: #NNNNNN` in `debian/changelog` with that number
- commit the changelog edit and push

Expected wait: instant — Debian's BTS auto-acknowledges within a
minute.

### B2.  Get a sponsor

Debian uploads must be signed by a Debian Developer (DD).  You
aren't one yet, so you need a sponsor — typically the rust-team
on `#debian-rust` (OFTC IRC) or the rust-team mailing list at
<debian-rust@lists.debian.org>.

Self-introductions go in
`https://lists.debian.org/debian-rust/`.  Subject line example:

```
Subject: Sponsor request: agtop (ITP #1099999)
```

Body should link to:
- the ITP bug
- this repo
- a link to the source package on mentors.debian.net (next step)

Expected wait: 1-3 weeks for someone to pick it up.

### B3.  Upload to mentors.debian.net

mentors is the staging area where non-DDs publish source packages
for sponsor review.

```sh
# One-time
sudo apt install dput
echo "[mentors]"                                    >> ~/.dput.cf
echo "fqdn = mentors.debian.net"                    >> ~/.dput.cf
echo "incoming = /upload"                           >> ~/.dput.cf
echo "method = https"                               >> ~/.dput.cf
echo "allow_unsigned_uploads = 0"                   >> ~/.dput.cf
echo "progress_indicator = 2"                       >> ~/.dput.cf
echo "allowed_distributions = .*"                   >> ~/.dput.cf

# Per upload
cd ~/code/agtop
debuild -S -sa                                      # signed source build
dput mentors ../agtop_2.4.2-1_source.changes
```

Sign up at <https://mentors.debian.net> first, paste your GPG
fingerprint there too (similar dance to Launchpad).

### B4.  Run lintian and tighten the package

```sh
lintian --pedantic --info --display-info \
        ../agtop_2.4.2-1_source.changes
lintian --pedantic --info --display-info \
        ../agtop_2.4.2-1_amd64.deb
```

Fix every `E:` (error) and as many `W:` (warning) as practical
before pinging a sponsor.  Common rust-package gotchas:
- `missing-debian-watch-file` → already handled (`debian/watch`)
- `dh-cargo-not-using-cargo-config` → only matters if we add a
  `.cargo/config.toml`; we don't
- `extended-description-line-too-long` → keep `debian/control`
  Description lines under 80 cols
- `no-manual-page` → already handled (`debian/agtop.1`)
- `package-uses-old-debhelper-compat-version` → we use compat 13,
  current

### B5.  Sponsor uploads to NEW

Once the sponsor signs off, they upload your `.changes` to
`ftp.upload.debian.org`.  First upload of a brand-new package
goes into the **NEW queue** for ftpmaster legal/copyright review.

Expected wait: 4-6 weeks (tracker at
<https://ftp-master.debian.org/new.html>).

### B6.  Once accepted

- Package lands in `unstable` automatically.
- Migrates to `testing` after 5-10 days assuming no autopkgtest
  failures and no RC bugs.
- Ubuntu syncs from `unstable` into the next Ubuntu release's
  pocket (or earlier, manually, via a Universe contributor).

### B7.  Ongoing maintenance

Each upstream tag:

```sh
# bump debian/changelog
DEBEMAIL=matt@brassey.io DEBFULLNAME="Matt Brassey" \
  dch --newversion 2.4.3-1 --distribution unstable \
      "New upstream release."

# build + sign
debuild -S -sa
dput ftp-master ../agtop_2.4.3-1_source.changes
```

After NEW, subsequent uploads skip the queue and land in unstable
directly.

---

## What's already prepared in this repo

| File / dir                        | Purpose |
| --------------------------------- | ------- |
| `debian/control`                  | Source + binary stanzas, `dh-cargo` build-deps |
| `debian/changelog`                | First entry, `Closes: #NNNNNN` placeholder |
| `debian/copyright`                | DEP-5 machine-readable, MIT license body |
| `debian/rules`                    | `dh $@ --buildsystem=cargo` one-liner |
| `debian/source/format`            | `3.0 (quilt)` |
| `debian/watch`                    | uscan target for upstream tags on GitHub |
| `debian/upstream/metadata`        | YAML pointers (homepage, repo, BTS) |
| `debian/agtop.1`                  | Manual page, ~140 lines |
| `debian/agtop.manpages`           | dh hook to install the man page |
| `debian/gbp.conf`                 | git-buildpackage layout |
| `debian/ITP.txt`                  | Ready-to-file ITP report body |
| `packages/ppa/build.sh`           | Local PPA upload driver |
| `packages/ppa/README.md`          | PPA setup walk-through |
| `.github/workflows/ppa.yml`       | CI: source-package + lintian on every push |

## Estimated calendar timeline

| Step                                  | Active work | Wait |
| ------------------------------------- | ----------- | ---- |
| PPA activation + key registration     | 30 min      | —    |
| First PPA upload + Launchpad build    | 5 min       | ~30 min |
| ITP filing + sponsor request          | 30 min      | 1-3 weeks |
| mentors upload + lintian polish       | 1-2 hrs     | —    |
| NEW queue review                      | —           | 4-6 weeks |
| Migration to testing                  | —           | 5-10 days |

Total to "Debian users `apt install agtop` from the official archive":
**~6-9 weeks**, of which ~2 hrs is your time and the rest is
waiting on volunteers.

PPA users get it within an hour of step A4.
