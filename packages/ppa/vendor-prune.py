#!/usr/bin/env python3
"""Patch every vendor/*/.cargo-checksum.json so the file list reflects
the actual files on disk, after pruning steps removed pre-rendered
docs / binary import-libs / etc.  cargo build --frozen otherwise
fails integrity check on missing entries.

Run from the staged source root (where ./vendor/ exists)."""
import json, glob, os, sys

if not os.path.isdir("vendor"):
    sys.exit("no ./vendor/ directory — run from the staged source root")

patched = 0
for ck in glob.glob("vendor/*/.cargo-checksum.json"):
    crate_dir = os.path.dirname(ck)
    with open(ck) as f:
        d = json.load(f)
    files = d.get("files", {})
    pruned = {k: v for k, v in files.items()
              if os.path.exists(os.path.join(crate_dir, k))}
    if pruned != files:
        d["files"] = pruned
        with open(ck, "w") as f:
            json.dump(d, f)
        patched += 1

print(f"patched {patched} crate checksum files")
