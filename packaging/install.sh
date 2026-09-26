#!/bin/sh
# Lays sinopia out under a package root. Every recipe calls this — the
# prebuilt package from inside the release archive, the source and git
# packages from inside the source tree — so the three cannot come to
# install different things: both trees keep these files at the same
# paths, and the archive is packed to (packaging/archive.sh).
#
#   packaging/install.sh <binary> <root> [<name>]
#
# <name> names the licence and doc directories, which Arch keeps per
# package: sinopia, sinopia-bin or sinopia-git.
#
# What lands under share/sinopia is opt-in, wired up by each person for
# themselves — the plugin, the theme hook, the agent skill. A package
# never writes into a home directory.

set -eu

if [ $# -lt 2 ]; then
	echo "usage: $0 <binary> <root> [<name>]" >&2
	exit 2
fi

bin=$1
share=$2/usr/share
name=${3:-sinopia}
here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

install -Dm755 "$bin" "$2/usr/bin/sinopia"
install -Dm644 "$here/packaging/applications/sinopia.desktop" \
	"$share/applications/sinopia.desktop"
install -Dm644 "$here/assets/logo/app-icon.svg" \
	"$share/icons/hicolor/scalable/apps/sinopia.svg"

install -Dm644 -t "$share/licenses/$name" "$here/LICENSE" \
	"$here/THIRD-PARTY.md" "$here/assets/fonts/LiberationSans-LICENSE.txt"
install -Dm644 -t "$share/doc/$name" "$here/README.md"

install -Dm755 -t "$share/sinopia/omarchy" "$here/contrib/omarchy/sinopia"
install -Dm644 -t "$share/sinopia/plugin" "$here"/plugin/*
install -Dm644 -t "$share/sinopia/skills/sinopia" "$here/skills/sinopia/SKILL.md"
install -Dm755 -t "$share/sinopia/skills/sinopia" "$here/skills/sinopia/install.sh"
