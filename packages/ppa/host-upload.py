#!/usr/bin/env python3
"""Upload an already-signed PPA source package to Launchpad from
the maintainer's host directly, bypassing docker.  Useful when the
container's getaddrinfo only returns v4 (no v6 routing on docker's
default bridge) and Launchpad's v4 endpoint is intermittently
rate-limited.

Usage:
    python3 packages/ppa/host-upload.py
    python3 packages/ppa/host-upload.py --changes dist-ppa/agtop_*_source.changes
    python3 packages/ppa/host-upload.py --target ppa:other-user/other-archive

Reads:
    ~/.ssh/id_ed25519        SSH key registered with Launchpad
    dist-ppa/*.changes       signed source-package manifest
"""
from __future__ import annotations
import argparse, getpass, glob, os, re, socket, sys, time
from pathlib import Path

try:
    import paramiko
except ImportError:
    sys.exit("paramiko not installed.  pip install --user --break-system-packages paramiko")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--changes", help="path to .changes file (default: auto-discover under dist-ppa/)")
    ap.add_argument("--target", default="ppa:mbrassey/agtop", help="dput-style target ppa:<user>/<archive>")
    ap.add_argument("--key", default=str(Path.home() / ".ssh" / "id_ed25519"),
                    help="SSH private key (default ~/.ssh/id_ed25519)")
    ap.add_argument("--key-passphrase", default=os.environ.get("SSH_PASSPHRASE", ""),
                    help="passphrase for SSH key, or set SSH_PASSPHRASE env var")
    args = ap.parse_args()

    # Locate the .changes file.
    if args.changes:
        changes_path = Path(args.changes)
    else:
        candidates = sorted(glob.glob("dist-ppa/*_source.changes"))
        if not candidates:
            sys.exit("no dist-ppa/*_source.changes found.  Run packages/ppa/build.sh with PPA_NO_UPLOAD=1 first.")
        changes_path = Path(candidates[-1])
    print(f"==> changes file: {changes_path}")
    base = changes_path.parent

    # Parse Files: stanza for the list of artefacts.
    body = changes_path.read_text()
    files: list[str] = []
    in_files = False
    for line in body.splitlines():
        if line.startswith("Files:"):
            in_files = True; continue
        if in_files:
            if line and not line.startswith(" "):
                in_files = False; continue
            tok = line.split()
            if len(tok) >= 5: files.append(tok[-1])
    files.append(changes_path.name)
    files = sorted(set(files))
    print(f"    {len(files)} files: {files}")

    # Verify all files exist.
    missing = [f for f in files if not (base / f).exists()]
    if missing:
        sys.exit(f"missing artefacts: {missing}")

    # Parse target.
    m = re.match(r"^ppa:([^/]+)/([^/]+)$", args.target)
    if not m:
        sys.exit(f"bad target: {args.target}")
    user, archive = m.group(1), m.group(2)

    # Resolve all addresses, prefer v6 (often less rate-limited).
    host = "ppa.launchpad.net"
    print(f"==> resolving {host}")
    candidates: list[tuple[int, tuple]] = []
    for fam in (socket.AF_INET6, socket.AF_INET):
        try:
            for ai in socket.getaddrinfo(host, 22, fam, socket.SOCK_STREAM):
                candidates.append((fam, ai[4]))
        except socket.gaierror:
            pass
    print(f"    candidates: {[c[1] for c in candidates]}")
    if not candidates:
        sys.exit("no DNS results for ppa.launchpad.net")

    # Try each address, banner-peek before committing.  Launchpad's
    # PPA SSH endpoint is intermittent today — banner returns one
    # tick, times out the next.  Retry up to 12 times with a short
    # backoff between attempts before giving up.
    max_attempts = int(os.environ.get("PPA_RETRIES", "12"))
    sock = None
    last_err: Exception | None = None
    for attempt in range(1, max_attempts + 1):
        for fam, addr in candidates:
            try:
                print(f"  [{attempt}/{max_attempts}] trying {addr}...")
                s = socket.socket(fam, socket.SOCK_STREAM)
                s.settimeout(20)
                s.connect(addr)
                s.settimeout(25)
                peek = s.recv(20, socket.MSG_PEEK)
                if peek.startswith(b"SSH-"):
                    print(f"    banner OK, using {addr}")
                    sock = s
                    break
                print(f"      no banner (peek={peek!r}), trying next")
                s.close()
                last_err = RuntimeError(f"no banner from {addr}")
            except (OSError, socket.timeout) as e:
                print(f"      {type(e).__name__}: {e}")
                last_err = e
        if sock is not None:
            break
        # backoff: 5s, 10s, 15s, 20s, then 30s capped
        delay = min(5 * attempt, 30)
        print(f"    all candidates failed; sleeping {delay}s before retry")
        time.sleep(delay)
    if sock is None:
        sys.exit(f"no usable Launchpad SSH endpoint after {max_attempts} attempts: {last_err}")
    sock.settimeout(120)

    # Start SSH transport.
    t = paramiko.Transport(sock)
    t.banner_timeout = 60
    t.start_client(timeout=60)
    print(f"    remote: {t.remote_version}")

    # Load key.
    key_path = args.key
    pp = args.key_passphrase or None
    try:
        key = paramiko.Ed25519Key.from_private_key_file(key_path, password=pp)
    except paramiko.ssh_exception.PasswordRequiredException:
        if not sys.stdin.isatty():
            sys.exit("SSH key has passphrase; set SSH_PASSPHRASE env var")
        pp = getpass.getpass(f"Passphrase for {key_path}: ")
        key = paramiko.Ed25519Key.from_private_key_file(key_path, password=pp)
    except paramiko.ssh_exception.SSHException as e:
        sys.exit(f"can't load {key_path}: {e}.  Try `ssh-keygen -p -f {key_path}` to remove passphrase.")

    t.auth_publickey(user, key)
    print("    authenticated")

    sftp = paramiko.SFTPClient.from_transport(t)
    print(f"    remote pwd: {sftp.normalize('.')}")

    # Upload each file with a progress callback.
    for f in files:
        local = base / f
        size = local.stat().st_size
        print(f"    -> {f} ({size:,} bytes)", end="", flush=True)
        last = [time.time(), 0]
        def cb(done, total, last=last):
            now = time.time()
            if now - last[0] > 1 or done == total:
                pct = (done / total * 100) if total else 0
                print(f"\r    -> {f} ({size:,} bytes) {pct:5.1f}%", end="", flush=True)
                last[0] = now
        # confirm=False: Launchpad's PPA incoming/ is upload-only,
        # sftp.stat() on a just-uploaded file returns no size and
        # paramiko raises 'size mismatch in put! None != N'.  We
        # still know the upload succeeded because put() returns
        # without raising on transport errors.
        sftp.put(str(local), f, callback=cb, confirm=False)
        print()

    sftp.close()
    t.close()
    sock.close()
    print(f"==> Upload complete.  Watch the build at:")
    print(f"    https://launchpad.net/~{user}/+archive/ubuntu/{archive}/+packages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
