#!/usr/bin/env bash
# Run this checkout's app here against an engine built on a host from the
# host's own checkout, for trying what was changed there: the commit that
# checkout is on is fetched and checked out here first, so both sides are
# the same source.
#
# A build run from source needs none of this to use a host: with only
# [remote] host set, `cargo run` builds perchd from this checkout for the
# host and installs it. This is for the host's checkout instead.
#
# Usage: scripts/dev-remote.sh <host> <checkout-on-host> [perch args...]
#   scripts/dev-remote.sh halo '~/code/perch'
#   scripts/dev-remote.sh chris@halo:2222 '~/code/perch'
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
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi
# The host as [remote] host names it: no scheme, a port after it or not.
host="${1#ssh://}"
host="${host%/}"
dir="${2%/}"
shift 2
# ssh takes a port only as an option.
dest="$host"
port=()
if [[ "$host" =~ ^([^:]+):([0-9]+)$ ]]; then
  dest="${BASH_REMATCH[1]}"
  port=(-p "${BASH_REMATCH[2]}")
fi

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
  ssh ${port[@]+"${port[@]}"} -o BatchMode=yes "$dest" "exec \"\$SHELL\" -lc 'cd \"$on_host\" && $1'"
}

echo "dev-remote: fetching $host:$from_home"
GIT_SSH_COMMAND="ssh ${port[*]-}" git fetch --quiet "$dest:$from_home" HEAD
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

# The app reads where its engine is from config.toml, so say when that is
# not the host and the build just made, rather than run against another.
config="${PERCH_HOME:-$HOME/.perch}/config.toml"
remote_value() {
  awk -v key="$1" '
    /^[[:space:]]*\[/ { inside = ($0 ~ /^[[:space:]]*\[remote\][[:space:]]*(#.*)?$/); next }
    inside && $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      sub(/^[^=]*=[[:space:]]*/, "")
      sub(/[[:space:]]*(#.*)?$/, "")
      gsub(/^["\047]|["\047]$/, "")
      print
      exit
    }' "$config" 2>/dev/null || true
}
wrong() {
  echo "dev-remote: $1" >&2
  if grep -q '^[[:space:]]*\[remote\]' "$config" 2>/dev/null; then
    echo "Under [remote] in $config, set:" >&2
  else
    echo "Add to $config:" >&2
    echo >&2
    echo "  [remote]" >&2
  fi
  cat >&2 <<NOTE

  host = "$host"
  perchd_on_host = "$built"

NOTE
  exit 1
}
set_host="$(remote_value host)"
set_host="${set_host#ssh://}"
set_host="${set_host%/}"
set_path="$(remote_value perchd_on_host)"
if [[ -n "$(remote_value command)" ]]; then
  wrong "[remote] command is set, and the app runs that instead of perchd_on_host; take it out."
elif [[ "$set_host" != "$host" ]]; then
  wrong "[remote] host is \"$set_host\", not \"$host\"."
elif [[ -z "$set_path" ]]; then
  wrong "[remote] names no perchd_on_host."
fi
# The same file, however each names it.
set_from_home="${set_path#\~/}"
if [[ "$set_from_home" =~ ^[A-Za-z0-9._+/-]+$ && "$built" =~ ^[A-Za-z0-9._+/-]+$ ]]; then
  case "$set_from_home" in
    /*) set_on_host="$set_from_home" ;;
    *) set_on_host="\$HOME/$set_from_home" ;;
  esac
  real="$(on_host_run "readlink -f \"$set_on_host\" \"$built\"" || true)"
  if [[ "$(sed -n 1p <<<"$real")" != "$(sed -n 2p <<<"$real")" ]]; then
    wrong "[remote] perchd_on_host is \"$set_path\", not the perchd just built."
  fi
fi

exec cargo run -p perch-app -- "$@"
