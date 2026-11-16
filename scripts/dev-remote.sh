#!/usr/bin/env bash
# Run this checkout's app on the Mac against an engine built from the same
# commit on a Linux host, for working on Perch where the source and the
# agents are on the host.
#
# Usage: scripts/dev-remote.sh <host> <checkout-on-host> [perch args...]
#   scripts/dev-remote.sh halo '~/code/perch'
#
# It fetches the host checkout's HEAD into this clone and checks it out
# detached, builds perchd in the host's checkout, and runs the app here.
# The app finds the host's build through config.toml on the Mac:
#
#   [remote]
#   host = "<host>"
#   perchd_on_host = "<checkout-on-host>/target/debug/perchd"
#
# Only what is committed on the host comes over: commit there first. A
# rebuilt perchd replaces the host's engine when nothing is running in it,
# and the app offers a Restart when something is.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi
host="$1"
dir="${2%/}"
shift 2

# The same characters the app allows in a path it sends to the host.
from_home="${dir#\~/}"
if [[ ! "$from_home" =~ ^[A-Za-z0-9._+/-]+$ || "$from_home" == -* ]]; then
  echo "dev-remote: the checkout's path can hold only letters, digits and . _ + - /" >&2
  exit 2
fi
case "$from_home" in
  /*) on_host="$from_home" ;;
  *) on_host="\$HOME/$from_home" ;;
esac

cd "$(git rev-parse --show-toplevel)"
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  echo "dev-remote: this checkout has changes; commit or set them aside first" >&2
  exit 1
fi

# In the host's login shell, for the PATH its profile sets (cargo's).
on_host_run() {
  ssh -o BatchMode=yes "$host" "exec \"\$SHELL\" -lc 'cd \"$on_host\" && $1'"
}

echo "dev-remote: fetching $host:$from_home"
git fetch --quiet "$host:$from_home" HEAD
git checkout --quiet --detach FETCH_HEAD
commit="$(git rev-parse --short=12 HEAD)"
echo "dev-remote: at $commit $(git log -1 --format=%s | cut -c1-72)"

if [[ -n "$(on_host_run 'git status --porcelain --untracked-files=no')" ]]; then
  echo "dev-remote: warning: $host has uncommitted changes there; its engine will have them and this app won't" >&2
fi

echo "dev-remote: building perchd on $host"
on_host_run 'cargo build -p perchd'
target="$(on_host_run 'cargo metadata --format-version 1 --no-deps' |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
built="${target:-$dir/target}/debug/perchd"

config="${PERCH_HOME:-$HOME/.perch}/config.toml"
if ! grep -q '^[[:space:]]*perchd_on_host[[:space:]]*=' "$config" 2>/dev/null; then
  cat >&2 <<NOTE
dev-remote: $config names no perchd_on_host. Add:

  [remote]
  host = "$host"
  perchd_on_host = "$built"

NOTE
  exit 1
fi

exec cargo run -p perch-app -- "$@"
