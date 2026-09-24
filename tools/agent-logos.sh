#!/bin/sh
# Sheets the agents' own marks into assets/agents/logos.png: one CELL px
# square per agent, side by side in `agents::KNOWN`'s order, each mark
# kept whole and centred on transparency. The binary embeds the sheet
# and never the originals, so this is only run when a mark changes.
#
# The export dialog draws a mark at 20 logical px, so a 40 px cell is
# exactly twice what one physical px shows at scale 1 — a halving the
# GPU's bilinear sampler averages cleanly — and one to one at scale 2.
#
# An SVG is rasterised well past the cell and brought down by
# ImageMagick, whose filter antialiases better than a straight render
# at 40 px would.
#
#     sh tools/agent-logos.sh      # needs rsvg-convert and magick
set -eu

CELL=40
KNOWN="claude codex opencode crush gemini"

cd "$(dirname "$0")/../assets/agents"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cells=""
for name in $KNOWN; do
    src=$(ls "$name".svg "$name".png "$name".webp 2>/dev/null | head -n 1)
    [ -n "$src" ] || { echo "no mark for $name" >&2; exit 1; }
    big="$tmp/$name-big.png"
    case "$src" in
        *.svg) rsvg-convert -w $((CELL * 8)) -h $((CELL * 8)) --keep-aspect-ratio "$src" -o "$big" ;;
        *) magick "$src" -background none "$big" ;;
    esac
    magick "$big" -background none -resize "${CELL}x${CELL}" \
        -gravity center -extent "${CELL}x${CELL}" "$tmp/$name.png"
    cells="$cells $tmp/$name.png"
done

# shellcheck disable=SC2086 # the cells are one word each, by construction
magick $cells -background none +append -strip -define png:color-type=6 logos.png
echo "wrote assets/agents/logos.png"
