#!/usr/bin/env sh
# Installs the sinopia skill where each agent on this machine looks for
# one. A skill is a directory named after it holding a `SKILL.md`, and
# three hosts agree on that shape — they only disagree on where the
# directory goes:
#
#   Claude Code  ${CLAUDE_CONFIG_DIR:-~/.claude}/skills/sinopia/
#   Codex        ${CODEX_HOME:-~/.codex}/skills/sinopia/
#   OpenCode     ${XDG_CONFIG_HOME:-~/.config}/opencode/skills/sinopia/
#
# Usage:
#   ./install.sh              # every host found on this machine
#   ./install.sh <directory>  # one place, for a host not listed above
#
# A host is "found" when its command is on PATH or its config directory
# already exists; anything else is skipped and said out loud, because an
# install that quietly wrote nowhere is worse than one that reports it.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
skill="$here/SKILL.md"

if [ ! -f "$skill" ]; then
	echo "no SKILL.md beside $0" >&2
	exit 1
fi

put() {
	mkdir -p "$1"
	cp "$skill" "$1/SKILL.md"
	echo "  installed  $1/SKILL.md"
}

# One directory named on the command line: install there and stop.
if [ $# -gt 0 ]; then
	put "$1"
	exit 0
fi

found=0

# $1 host, $2 command, $3 config directory, $4 skills directory
host() {
	if command -v "$2" >/dev/null 2>&1 || [ -d "$3" ]; then
		printf '%-12s' "$1"
		put "$4"
		found=$((found + 1))
	else
		echo "  skipped    $1 — not installed here"
	fi
}

echo "sinopia skill:"
host "Claude Code" claude "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" \
	"${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills/sinopia"
host "Codex" codex "${CODEX_HOME:-$HOME/.codex}" \
	"${CODEX_HOME:-$HOME/.codex}/skills/sinopia"
host "OpenCode" opencode "${XDG_CONFIG_HOME:-$HOME/.config}/opencode" \
	"${XDG_CONFIG_HOME:-$HOME/.config}/opencode/skills/sinopia"

if [ "$found" -eq 0 ]; then
	echo
	echo "nothing installed: no host found. Name a directory instead:" >&2
	echo "  $0 <directory>" >&2
	exit 1
fi

# The binary has to be new enough to answer the verb, not merely be
# there: a skill whose every command errors is worse than none, and an
# old build on PATH is the likeliest way to get one.
if ! command -v sinopia >/dev/null 2>&1; then
	echo
	echo "note: 'sinopia' is not on PATH. The skill's commands need the"
	echo "      binary; put it on PATH or in ~/.local/bin."
elif ! sinopia agent --help >/dev/null 2>&1; then
	echo
	echo "note: the sinopia on PATH does not know the 'agent' verb —"
	echo "      $(command -v sinopia) predates it. Build and install a"
	echo "      newer one, or the skill's commands will all fail:"
	echo "        cargo build --release && install -m755 \\"
	echo "          target/release/sinopia ~/.local/bin/sinopia"
fi

cat <<EOF

A host that reads one instructions file rather than a skills directory
wants a line pointing at the installed copy instead:

    See ~/.claude/skills/sinopia/SKILL.md for reading and writing
    frames on the Sinopia board.
EOF
