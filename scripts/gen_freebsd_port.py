#!/usr/bin/env python3
"""
Generate the FreeBSD ports CARGO_CRATES list + distinfo entries by
walking Cargo.lock and fetching each crate's sha256 from crates.io.

This mirrors what `make cargo-crates` does inside FreeBSD ports'
USES=cargo framework, but runs from any OS with Python + network so
we don't need a FreeBSD machine to prepare the port submission.

Outputs to stdout:
  - CARGO_CRATES Makefile fragment (sorted, one crate per line)
  - distinfo block (SHA256 + SIZE per crate tarball)

Usage:
  python3 scripts/gen_freebsd_port.py > /tmp/freebsd-port-fragments.txt
"""
from __future__ import annotations
import hashlib, json, sys, urllib.request, pathlib, tomllib

# Crates.io publishes deterministic .crate tarballs at:
#   https://crates.io/api/v1/crates/<name>/<version>/download
# which 302-redirects to the static.crates.io CDN.  Same content is
# also at:
#   https://static.crates.io/crates/<name>/<name>-<version>.crate
# We use the static path directly to skip a redirect hop and get
# stable Content-Length headers.
CRATE_URL = "https://static.crates.io/crates/{name}/{name}-{version}.crate"


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "agtop-port-gen/1"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def main() -> int:
    here = pathlib.Path(__file__).resolve().parent
    lock_path = here.parent / "Cargo.lock"
    cargo = tomllib.loads(lock_path.read_text())

    pkgs = cargo.get("package", [])
    # Skip the workspace root (no `source` field) and any path-based
    # deps; only registry deps need to land in CARGO_CRATES.
    crates = []
    for p in pkgs:
        src = p.get("source", "")
        if not src.startswith("registry+"):
            continue
        crates.append((p["name"], p["version"]))
    crates.sort()

    cargo_crates_lines = []
    distinfo_lines     = []

    for name, version in crates:
        url = CRATE_URL.format(name=name, version=version)
        print(f"  fetching {name}-{version}", file=sys.stderr)
        try:
            blob = fetch(url)
        except Exception as e:
            print(f"  ERROR {name}-{version}: {e}", file=sys.stderr)
            return 2
        sha = hashlib.sha256(blob).hexdigest()
        size = len(blob)
        cargo_crates_lines.append(f"\t{name}-{version} \\")
        distinfo_lines.append(
            f"SHA256 (rust/crates/{name}-{version}.crate) = {sha}\n"
            f"SIZE   (rust/crates/{name}-{version}.crate) = {size}"
        )

    # Trim the trailing backslash from the last CARGO_CRATES line.
    if cargo_crates_lines:
        cargo_crates_lines[-1] = cargo_crates_lines[-1].rstrip(" \\")

    print("# ── CARGO_CRATES (paste into Makefile) ──────────────────────────────")
    print("CARGO_CRATES=\t\\")
    print("\n".join(cargo_crates_lines))
    print()
    print("# ── distinfo (paste into distinfo, in addition to the source tarball) ──")
    print("\n".join(distinfo_lines))
    print(f"\n# Total crates: {len(crates)}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
