#!/usr/bin/env sh
# Installs the omawhite skill where an agent will find it.
#
# Usage:
#   ./install.sh              # ~/.claude/skills/omawhite/
#   ./install.sh <directory>  # anywhere else a host reads skills from
#
# The skill is one markdown file with frontmatter. An agent that reads a
# directory of skills wants the directory; an agent that reads a single
# instructions file (AGENTS.md, GEMINI.md) wants a line pointing at the
# installed SKILL.md — the last thing this script prints is that line.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
to=${1:-"$HOME/.claude/skills/omawhite"}

if [ ! -f "$here/SKILL.md" ]; then
	echo "no SKILL.md beside $0" >&2
	exit 1
fi

mkdir -p "$to"
cp "$here/SKILL.md" "$to/SKILL.md"
echo "installed $to/SKILL.md"

if ! command -v omawhite >/dev/null 2>&1; then
	echo
	echo "note: 'omawhite' is not on PATH. The skill's commands need the"
	echo "      binary; put it on PATH or in ~/.local/bin."
fi

cat <<EOF

For an agent that reads one instructions file instead of a skills
directory, add this line to it:

    See $to/SKILL.md for reading and writing frames on the omawhite board.
EOF
