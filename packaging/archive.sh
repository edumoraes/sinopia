#!/bin/sh
# Packs a release build into the archive a release carries and the
# prebuilt package installs from: the binary at the top, and beside it
# every file packaging/install.sh lays out, at the path it has in the
# source tree.
#
#   packaging/archive.sh <tag> <binary> <outdir>
#
# Writes <outdir>/omawhite-<tag>-x86_64-unknown-linux-gnu.tar.gz, then
# unpacks it and runs install.sh from inside, so an archive missing
# something the install needs fails here rather than in somebody's
# makepkg.

set -eu

if [ $# -ne 3 ]; then
	echo "usage: $0 <tag> <binary> <outdir>" >&2
	exit 2
fi

tag=$1
bin=$2
out=$3
name=omawhite-$tag-x86_64-unknown-linux-gnu
here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/$name" "$out"
install -m755 "$bin" "$work/$name/omawhite"
(cd "$here" && cp --parents -R \
	LICENSE THIRD-PARTY.md README.md \
	packaging/install.sh packaging/applications/omawhite.desktop \
	assets/logo/app-icon.svg assets/fonts/LiberationSans-LICENSE.txt \
	contrib/omarchy/omawhite plugin skills/omawhite \
	"$work/$name/")

# Owned by nobody in particular, in a fixed order: the archive says
# nothing about the machine that packed it.
tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner \
	-czf "$out/$name.tar.gz" "$name"

mkdir "$work/check"
tar -C "$work/check" -xzf "$out/$name.tar.gz"
sh "$work/check/$name/packaging/install.sh" \
	"$work/check/$name/omawhite" "$work/check/root"
echo "$out/$name.tar.gz"
