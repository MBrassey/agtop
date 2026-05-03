#!/usr/bin/env bash
# Upload an already-signed agtop PPA source package via a relay
# host (e.g. an AWS EC2 instance) when the local network's path to
# ppa.launchpad.net:22 is blocked / shadow-banned.
#
# Usage:
#   ./packages/ppa/relay-upload.sh <ssh-host> [series]
#
# Examples:
#   ./packages/ppa/relay-upload.sh grafana-ca           # default series=noble
#   ./packages/ppa/relay-upload.sh ec2-user@1.2.3.4 noble
#   ./packages/ppa/relay-upload.sh grafana.example.com noble
#
# Steps performed:
#   1. If ./dist-ppa/agtop_*.changes already exists, reuse it.
#      Otherwise run `PPA_NO_UPLOAD=1 ./packages/ppa/build.sh <series>`
#      to produce a freshly-signed source package.
#   2. Verify the relay can reach ppa.launchpad.net:22 (banner check).
#   3. Install dput-ng on the relay if missing (apt).
#   4. scp the source-package files to ~/agtop-ppa/ on the relay.
#   5. ssh in and run dput.
#   6. Print Launchpad URL to watch the binary build.

set -euo pipefail

if [ $# -lt 1 ]; then
  cat >&2 <<USAGE
usage: $0 <ssh-host> [series]

  <ssh-host> = SSH alias OR user@host of a Linux box that can reach
               ppa.launchpad.net:22 (typically a cloud VM you already
               own — EC2, GCP, Hetzner, friend's VPS, etc.).
  [series]   = Ubuntu series to upload for; defaults to noble.

Run from the agtop repo root.  Reuses dist-ppa/ if present, else
builds a fresh signed source package via packages/ppa/build.sh.
USAGE
  exit 2
fi

relay="$1"
series="${2:-noble}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$root"

ppa_target="${PPA_TARGET:-ppa:mbrassey/agtop}"
remote_dir="${PPA_RELAY_REMOTE_DIR:-agtop-ppa}"

# 1. Source package — produce if absent.
shopt -s nullglob
changes=( dist-ppa/agtop_*_source.changes )
shopt -u nullglob
if [ ${#changes[@]} -eq 0 ]; then
  echo "==> No signed source pkg in dist-ppa/.  Building one now."
  echo "    (You'll be prompted for GPG passphrase via PPA_NO_UPLOAD=1)"
  PPA_NO_UPLOAD=1 ./packages/ppa/build.sh "$series"
  shopt -s nullglob
  changes=( dist-ppa/agtop_*_source.changes )
  shopt -u nullglob
  if [ ${#changes[@]} -eq 0 ]; then
    echo "build produced no .changes — aborting" >&2
    exit 1
  fi
fi
changes_path="${changes[0]}"
echo "==> Using ${changes_path}"

# 2. Probe the relay's network to Launchpad.
echo "==> Probing relay '${relay}' for SSH connectivity to ppa.launchpad.net:22"
banner_check=$(ssh -o ConnectTimeout=10 -o BatchMode=yes "$relay" \
  'timeout 6 bash -c "exec 3<>/dev/tcp/ppa.launchpad.net/22; head -c 30 <&3"' 2>&1)
if ! echo "$banner_check" | grep -q "SSH-"; then
  echo "    relay can't reach ppa.launchpad.net:22 either."
  echo "    captured: ${banner_check}"
  echo "    Try a different relay (us-east, eu-west) — Launchpad's PPA"
  echo "    SSH endpoint shadow-bans some ASNs."
  exit 3
fi
echo "    relay reaches Launchpad: ${banner_check}"

# 3. Ensure relay has dput-ng.  Skip if already present.
echo "==> Checking dput on relay"
if ! ssh -o BatchMode=yes "$relay" 'command -v dput >/dev/null 2>&1'; then
  echo "    installing dput-ng on relay (sudo apt)"
  ssh "$relay" 'sudo apt-get update -qq && sudo apt-get install -y -qq dput-ng python3-paramiko openssh-client'
fi

# 4. Copy artefacts to relay.
echo "==> Staging files on relay:${remote_dir}/"
ssh -o BatchMode=yes "$relay" "mkdir -p \"\$HOME/${remote_dir}\""
# scp every file referenced by the .changes plus the .changes itself.
files=( "$changes_path" )
while IFS= read -r f; do
  [ -n "$f" ] && files+=( "dist-ppa/$f" )
done < <(awk '/^Files:/{flag=1; next} /^[^ ]/{flag=0} flag{print $NF}' "$changes_path")
# dedupe
declare -A seen=()
unique_files=()
for f in "${files[@]}"; do
  if [ -z "${seen[$f]:-}" ]; then
    seen[$f]=1
    unique_files+=( "$f" )
  fi
done
echo "    files: ${#unique_files[@]}"
for f in "${unique_files[@]}"; do echo "      $(basename "$f") ($(stat -c %s "$f") bytes)"; done
scp -q "${unique_files[@]}" "${relay}:${remote_dir}/"

# 5. Run dput on the relay.
echo "==> Running dput ${ppa_target} on relay"
remote_changes=$(basename "$changes_path")
ssh "$relay" "dput \"${ppa_target}\" \"\$HOME/${remote_dir}/${remote_changes}\""

# 6. Done.
ppa_user="${ppa_target#ppa:}"; ppa_user="${ppa_user%/*}"
ppa_archive="${ppa_target##*/}"
cat <<DONE

==> Upload complete.

Watch the build at:
  https://launchpad.net/~${ppa_user}/+archive/ubuntu/${ppa_archive}/+packages

When the build turns green (~30-40 min), end-users can install with:
  sudo add-apt-repository ppa:${ppa_user}/${ppa_archive}
  sudo apt update
  sudo apt install agtop

Relay artefacts left at \${HOME}/${remote_dir}/ on '${relay}'; safe to
ssh in and delete when done:
  ssh ${relay} 'rm -rf ~/${remote_dir}'
DONE
