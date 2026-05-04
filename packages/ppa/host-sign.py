#!/usr/bin/env python3
"""Host-side debsign substitute.

Inside the docker container we build the source package unsigned
(`debuild -S -us -uc`) because the container has no clean way to
talk to the host's gpg-agent — and forcing the user to type the
key passphrase a second time inside the container is fragile.

This script takes the unsigned .dsc + .changes from dist-ppa/ and:

  1. Cleartext-signs the .dsc with the host's gpg (which uses the
     host's gpg-agent's already-cached passphrase, no prompt).
  2. Recomputes md5/sha1/sha256/size for the now-larger .dsc inside
     the .changes file's Files: / Checksums-Sha1: / Checksums-Sha256:
     stanzas.
  3. Cleartext-signs the .changes with the same key.

Result: dist-ppa/*.dsc and dist-ppa/*_source.changes are now valid
signed artefacts identical in form to what `debsign` would have
produced inside the container, ready for upload via host-upload.py.

Usage:
    python3 packages/ppa/host-sign.py
    python3 packages/ppa/host-sign.py --dir dist-ppa
"""
from __future__ import annotations
import argparse, hashlib, os, re, subprocess, sys
from pathlib import Path

DIGESTS = {"md5": "md5", "sha1": "sha1", "sha256": "sha256"}


def gpg_clearsign(path: Path, key: str) -> None:
    """Replace `path` with its cleartext-signed equivalent."""
    out = path.with_suffix(path.suffix + ".signed")
    subprocess.run(
        ["gpg", "--batch", "--yes", "--local-user", key,
         "--clearsign", "--output", str(out), str(path)],
        check=True,
    )
    out.replace(path)


def file_digests(path: Path) -> tuple[str, str, str, int]:
    data = path.read_bytes()
    return (
        hashlib.md5(data).hexdigest(),
        hashlib.sha1(data).hexdigest(),
        hashlib.sha256(data).hexdigest(),
        len(data),
    )


def update_changes_for_dsc(changes: Path, dsc: Path) -> None:
    """Patch .changes Files / Checksums-* stanzas to reference the
    signed .dsc's new hashes + size.  Stanza format is ``\\n " hash size [section priority] filename"``."""
    md5, sha1, sha256, size = file_digests(dsc)
    name = dsc.name

    body = changes.read_text()
    out_lines: list[str] = []
    section: str | None = None
    for line in body.splitlines():
        # Section header.  Must allow digits so `Checksums-Sha1:` and
        # `Checksums-Sha256:` are detected — earlier `[A-Za-z-]*`
        # bailed at the digit and dropped both stanzas through the
        # default `out_lines.append(line)` branch unmodified.
        if re.match(r"^[A-Z][A-Za-z0-9-]*:", line):
            section = line.split(":", 1)[0]
            out_lines.append(line)
            continue
        # str.split() drops the leading whitespace, so tok[0] is the
        # hash itself (not an empty string).  Files: layout is
        # `<md5> <size> <section> <priority> <filename>` (5 cols);
        # Checksums-Sha{1,256}: layout is `<hash> <size> <filename>`
        # (3 cols).  Preserve every column except the hash + size we
        # need to replace.
        if section == "Files" and line.startswith(" ") and line.rstrip().endswith(name):
            tok = line.split()
            tok[0] = md5
            tok[1] = str(size)
            out_lines.append(" " + " ".join(tok))
            continue
        if section == "Checksums-Sha1" and line.startswith(" ") and line.rstrip().endswith(name):
            tok = line.split()
            tok[0] = sha1
            tok[1] = str(size)
            out_lines.append(" " + " ".join(tok))
            continue
        if section == "Checksums-Sha256" and line.startswith(" ") and line.rstrip().endswith(name):
            tok = line.split()
            tok[0] = sha256
            tok[1] = str(size)
            out_lines.append(" " + " ".join(tok))
            continue
        out_lines.append(line)
    changes.write_text("\n".join(out_lines) + "\n")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="dist-ppa", help="directory holding unsigned .dsc + .changes (default: dist-ppa)")
    ap.add_argument("--key", default=os.environ.get("DEBEMAIL", "matt@brassey.io"),
                    help="GPG signing identity (default: $DEBEMAIL or matt@brassey.io)")
    args = ap.parse_args()

    base = Path(args.dir)
    if not base.is_dir():
        sys.exit(f"no such directory: {base}")

    dscs = sorted(base.glob("*.dsc"))
    changes = sorted(base.glob("*_source.changes"))
    if not dscs or not changes:
        sys.exit(f"no .dsc / .changes in {base}")
    if len(dscs) != 1 or len(changes) != 1:
        sys.exit(f"expected exactly one .dsc + one .changes, got {dscs} {changes}")
    dsc = dscs[0]
    chg = changes[0]
    print(f"==> Signing {dsc.name} (key: {args.key})")
    gpg_clearsign(dsc, args.key)

    print(f"==> Updating {chg.name} hashes for signed .dsc")
    update_changes_for_dsc(chg, dsc)

    print(f"==> Signing {chg.name}")
    gpg_clearsign(chg, args.key)

    print("==> Done.  Signed artefacts:")
    for f in sorted(base.iterdir()):
        print(f"      {f.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
