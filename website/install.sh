#!/bin/sh
# Install or update Perch from the latest release:
#   curl -fsSL https://raw.githubusercontent.com/perch-code/perch/main/install.sh | sh
#
# On a Mac this puts Perch.app in /Applications. On Linux it unpacks Perch
# into ~/.local/share/perch. Either way it links ~/.local/bin/perch to the
# app, so `perch context` runs from a terminal or an agent's shell.
# It adds ~/.local/bin to PATH in your shell's startup file unless it is on
# PATH already or PERCH_NO_MODIFY_PATH is set to something other than 0.
# PERCH_INSTALL_DIR overrides where Perch.app or the perch folder goes.

set -eu

repo=perch-code/perch

fail() {
	echo "install.sh: $*" >&2
	exit 1
}

case "$(uname -s)" in
Darwin) os=macos ;;
Linux) os=linux ;;
*) fail "Perch runs on macOS and Linux, not $(uname -s)" ;;
esac

case "$(uname -m)" in
arm64 | aarch64) arch=arm64 ;;
x86_64 | amd64) arch=x86_64 ;;
*) fail "no Perch build for $(uname -m)" ;;
esac

# Rosetta reports x86_64 to a translated shell; Apple Silicon wants arm64.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] &&
	[ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
	arch=arm64
fi

if [ "$os" = linux ]; then
	glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
	case "$glibc" in
	"") fail "Perch needs glibc 2.35 or newer" ;;
	2.[0-9] | 2.[0-9].* | 2.[12][0-9] | 2.[12][0-9].* | 2.3[0-4] | 2.3[0-4].*)
		fail "Perch needs glibc 2.35 or newer; this system has $glibc" ;;
	esac
fi

tarball=Perch-$os-$arch.tar.gz
url=https://github.com/$repo/releases/latest/download/$tarball

if [ "$os" = macos ]; then
	dest=${PERCH_INSTALL_DIR:-/Applications}
	bundle=Perch.app
else
	dest=${PERCH_INSTALL_DIR:-$HOME/.local/share}
	bundle=perch
fi

mkdir -p "$dest" 2>/dev/null || true
[ -w "$dest" ] || fail "can't write to $dest; set PERCH_INSTALL_DIR to a folder you own"
# Absolute, so the link to it from ~/.local/bin resolves.
dest=$(CDPATH= cd -- "$dest" && pwd)

# Unpack beside the old copy, then swap, so a failed download leaves it be.
tmp=$(mktemp -d "$dest/.perch-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $tarball"
curl -fsSL "$url" -o "$tmp/$tarball" ||
	fail "couldn't download $url; the latest release may not have a $os-$arch build yet"
tar -xzf "$tmp/$tarball" -C "$tmp"
[ -e "$tmp/$bundle" ] || fail "$tarball has no $bundle in it"

rm -rf "${dest:?}/$bundle"
mv "$tmp/$bundle" "$dest/$bundle"
echo "Installed $dest/$bundle"

bin=$HOME/.local/bin
mkdir -p "$bin"
if [ "$os" = macos ]; then
	ln -sf "$dest/$bundle/Contents/MacOS/perch" "$bin/perch"
else
	ln -sf "$dest/perch/perch" "$bin/perch"
fi
echo "Linked $bin/perch"

# Put ~/.local/bin on PATH in the shell's startup file, as rustup and uv do:
# a Mac has no ~/.local/bin on PATH, and some Linux shells don't either.
# The line checks PATH first, so a startup file read twice adds it once.
on_path() {
	case ":$PATH:" in
	*":$bin:"*) return 0 ;;
	*) return 1 ;;
	esac
}
add_to_path() {
	line='case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac'
	case "$(basename "${SHELL:-sh}")" in
	# .zshenv, not .zshrc: every zsh reads it, including the non-interactive
	# `zsh -c` and `zsh -lc` an agent started from the Dock runs commands in.
	zsh) rc=${ZDOTDIR:-$HOME}/.zshenv ;;
	bash)
		rc=$HOME/.bashrc
		# Terminal.app starts login shells, which read the first of these
		# there is, not .bashrc: a new .bash_profile would hide a .profile.
		if [ "$os" = macos ]; then
			rc=$HOME/.bash_profile
			for f in .bash_profile .bash_login .profile; do
				if [ -e "$HOME/$f" ]; then
					rc=$HOME/$f
					break
				fi
			done
		fi
		;;
	fish)
		rc=${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/perch.fish
		line='contains $HOME/.local/bin $PATH; or set -gx PATH $HOME/.local/bin $PATH'
		;;
	*) rc=$HOME/.profile ;;
	esac
	if grep -qsF "$line" "$rc"; then
		echo "$rc puts $bin on PATH; open a new terminal to run perch"
		return
	fi
	mkdir -p "$(dirname "$rc")" || return 1
	printf '\n# Added by the Perch installer\n%s\n' "$line" >>"$rc" || return 1
	echo "Added $bin to PATH in $rc; open a new terminal to run perch"
}
if ! on_path; then
	case ${PERCH_NO_MODIFY_PATH:-0} in
	0) add_to_path || echo "Add $bin to your PATH to run perch" ;;
	*) echo "Add $bin to your PATH to run perch" ;;
	esac
fi

if ! command -v gh >/dev/null 2>&1; then
	echo "Perch needs the GitHub CLI: install it from https://cli.github.com, then run gh auth login"
elif ! gh auth status >/dev/null 2>&1; then
	echo "Sign in to GitHub before starting Perch: gh auth login"
fi
