#!/usr/bin/env python3
"""Draw the Omawhite marks.

Every mark is one monoline geometry on a 24-grid: round caps and joins, the
grid line icons are drawn on. The geometry lives here and only here --
assets/logo/*.svg is written by this, never edited by hand.

The weight is 1.8 and not the 2 that grid usually carries: at 2 the white
inside the board closed up and the legs went stubby, which the 128px sheet
showed and the 16px one did not.

The proportions come from docs/boards/frame-1/board.json, the sketch this
started as. Measured across its 172 strokes: a rounded container 219x185
at radius 32, a board 125x76 centred in it, a tray overhanging the board
by a tenth of its width each side, legs half the board's height crossing a
fifth of the way down. Two things the sketch says that are not carried:
the easel sits right of the board's centre and the strokes are a hand's,
not a grid's -- so the legs are symmetric here and the passes are one line.

    python3 tools/logo.py
"""

import os

GRID = 24
STROKE = 1.8
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "logo")

# Which of the explorations is the mark the app actually wears. Change this
# and the icon and both lockups follow, since a mark carries its own extent.
PRIMARY = "03-board-is-frame"


# --- the pen ---------------------------------------------------------------

def n(v):
    """A number as short as it can be written."""
    s = f"{float(v):.4f}".rstrip("0").rstrip(".")
    return "0" if s in ("", "-0") else s


def rect(x, y, w, h, r=None, fill=False):
    a = f'<rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}"'
    if r:
        a += f' rx="{n(r)}"'
    if fill:
        a += ' fill="currentColor" stroke="none"'
    return a + "/>"


def line(x1, y1, x2, y2):
    return f'<path d="M{n(x1)} {n(y1)}L{n(x2)} {n(y2)}"/>'


def path(d):
    return f'<path d="{d}"/>'


def circle(cx, cy, r, fill=False):
    a = f'<circle cx="{n(cx)}" cy="{n(cy)}" r="{n(r)}"'
    if fill:
        a += ' fill="currentColor" stroke="none"'
    return a + "/>"


def reaches(x0, x1, y0, y1):
    """What a mark occupies of its grid, centreline to centreline.

    A lockup has to know where the mark stops to stand the word beside it,
    and the answer is different for every one of them -- so it is written
    on the drawing rather than beside the name of whichever is primary.
    """
    def carry(draw):
        draw.reaches = (x0, x1, y0, y1)
        return draw
    return carry


# --- the parts the sketch is made of ---------------------------------------

def legs_crossed(tray_y, foot_y, top_l, top_r, foot_l, foot_r):
    """Two legs that cross, as drawn: each starts under the far side."""
    return [line(top_r, tray_y, foot_l, foot_y), line(top_l, tray_y, foot_r, foot_y)]


# --- the explorations ------------------------------------------------------

@reaches(2, 22, 3.5, 20.5)
def v01_faithful():
    """The sketch cleaned up, at the size it was drawn for.

    The container is 17 tall and the board 7, so the white between them is
    a unit and a bit -- six pixels at 128 and one at 16, which is why this
    one is a header mark and 02 or 03 is the icon.
    """
    return [
        rect(2, 3.5, 20, 17, 3),
        rect(6.3, 6.5, 11.4, 6.95),
        line(5.15, 13.45, 18.85, 13.45),
        *legs_crossed(13.45, 16.85, 10.06, 13.94, 6.6, 17.35),
    ]


@reaches(2.5, 21.5, 4.5, 19.5)
def v02_easel():
    """The container dropped -- the easel alone, at the size it wants."""
    return [
        rect(4, 4.5, 16, 10),
        line(2.5, 14.5, 21.5, 14.5),
        *legs_crossed(14.5, 19.5, 9.3, 14.7, 4.5, 19.5),
    ]


@reaches(2, 22, 4, 20.4)
def v03_board_is_frame():
    """The container becomes the board: one rectangle less, and it reads small."""
    return [
        rect(3, 4, 18, 11, 3),
        line(2, 15, 22, 15),
        *legs_crossed(15, 20.4, 8.95, 15.05, 3.55, 20.45),
    ]


@reaches(2, 22, 2.5, 21)
def v04_solid():
    """The board as a slab -- a silhouette for the dock, not an outline.

    The tray stands off it rather than running along its edge, since a line
    laid on a filled shape is a line nobody sees.
    """
    return [
        rect(2.5, 2.5, 19, 11.5, 4, fill=True),
        line(2, 15.8, 22, 15.8),
        *legs_crossed(15.8, 21, 8.95, 15.05, 4, 20),
    ]


@reaches(2, 22, 4, 20.4)
def v05_drawn_on():
    """A stroke laid on the board: an empty board says whiteboard, not draw."""
    return [
        rect(3, 4, 18, 11, 3),
        path("M6.5 11.6Q12 5.6 17.5 9.6"),
        line(2, 15, 22, 15),
        *legs_crossed(15, 20.4, 8.95, 15.05, 3.55, 20.45),
    ]


@reaches(3, 21, 2.2, 20.8)
def v06_monogram_o():
    """The board closed into the name's own O, resting on the tray."""
    return [
        circle(12, 8.2, 6),
        line(3, 15.6, 21, 15.6),
        *legs_crossed(15.6, 20.8, 9.3, 14.7, 5, 19),
    ]


@reaches(2, 22, 2.5, 21)
def v07_double_u():
    """The legs folded into a W -- the half of the name the board is not."""
    return [
        rect(3, 2.5, 18, 10, 2.5),
        line(2, 12.5, 22, 12.5),
        path("M5 15L8.5 21L12 17L15.5 21L19 15"),
    ]


@reaches(2, 22, 4, 20.4)
def v08_tripod():
    """The leg that stands behind, which is what makes an easel an easel."""
    return [
        rect(3, 4, 18, 11, 3),
        line(2, 15, 22, 15),
        line(10, 15, 4.5, 20.4),
        line(14, 15, 19.5, 20.4),
        line(12, 15, 12, 19.8),
    ]


EXPLORATIONS = [
    ("01-faithful", v01_faithful),
    ("02-easel", v02_easel),
    ("03-board-is-frame", v03_board_is_frame),
    ("04-solid", v04_solid),
    ("05-drawn-on", v05_drawn_on),
    ("06-monogram-o", v06_monogram_o),
    ("07-double-u", v07_double_u),
    ("08-tripod", v08_tripod),
]


# --- the word --------------------------------------------------------------
#
# Drawn in the mark's own grammar rather than set in a face: circles and
# straights at the same weight, so the two halves are one drawing and the
# file leans on no font being installed anywhere.
#
# Baseline 20, x-height 10, cap and ascender 6 -- centrelines, not edges.

BASE, CAP = 20, 6

LETTERS = {
    "O": (12, [circle(6, 13, 6)]),
    "m": (14, [path("M0 20V13.5A3.5 3.5 0 0 1 7 13.5V20"),
               path("M7 20V13.5A3.5 3.5 0 0 1 14 13.5V20")]),
    "a": (10, [circle(5, 15, 5), line(10, 10, 10, 20)]),
    "w": (14, [path("M0 10L3.5 20L7 12L10.5 20L14 10")]),
    "h": (7,  [line(0, 6, 0, 20), path("M0 13.5A3.5 3.5 0 0 1 7 13.5V20")]),
    "i": (0,  [line(0, 10, 0, 20), circle(0, 6, STROKE / 2, fill=True)]),
    "t": (6,  [path("M3 6.5V17C3 19 4.3 20 6 20"), line(0.5, 10, 6, 10)]),
    "e": (10, [path("M10 15A5 5 0 1 0 8.53 18.54"), line(0, 15, 10, 15)]),
}

WORD = "Omawhite"

# The gap after each pair, centre to centre. A round letter beside a round one
# needs less air than two flats do or the word reads spotty, so the spacing is
# a table and not one number -- eight letters is small enough to say it plainly.
GAP = 3.4
KERN = {"Om": 3.0, "ma": 3.4, "aw": 2.9, "wh": 2.9, "hi": 3.9, "it": 3.9, "te": 3.2}


def wordmark(dx=0, dy=0):
    """The name, laid out left to right. Returns (elements, width)."""
    out, x = [], 0.0
    for i, ch in enumerate(WORD):
        w, parts = LETTERS[ch]
        out.append(f'<g transform="translate({n(x + dx)} {n(dy)})">' + "".join(parts) + "</g>")
        x += w
        if i < len(WORD) - 1:
            x += KERN.get(WORD[i:i + 2], GAP)
    return out, x


# --- the page --------------------------------------------------------------

def svg(view, body, color):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view}" fill="none" '
        f'color="{color}" stroke="{color}" stroke-width="{n(STROKE)}" '
        f'stroke-linecap="round" stroke-linejoin="round">\n'
        "<!-- generated by tools/logo.py; edit the geometry there -->\n"
        + "\n".join(body)
        + "\n</svg>\n"
    )


def write(name, view, body):
    for tone, color in (("black", "#000000"), ("white", "#ffffff")):
        p = os.path.join(OUT, f"{name}-{tone}.svg")
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(svg(view, body, color))


# --- the lockups -----------------------------------------------------------
#
# The mark is scaled to about one and a half cap heights and the group's own
# stroke-width undoes the scale, so both halves are laid at exactly 2.

def lockup_horizontal(draw):
    """The mark left of the name, standing on the baseline the word does."""
    s, gap = 1.3, 8.5
    x0, x1, y0, y1 = draw.reaches
    span = (x1 - x0) * s
    word, w = wordmark(dx=span + gap)
    return _page(draw, s, -x0 * s, BASE - y1 * s, word, 0, span + gap + w)


def lockup_vertical(draw):
    """The mark over the name, centred on it."""
    s, gap = 2.2, 9
    x0, x1, y0, y1 = draw.reaches
    span = (x1 - x0) * s
    _, w = wordmark()
    left = min(0.0, (w - span) / 2)
    lift = y1 * s + gap - CAP
    word, _ = wordmark(dy=lift)
    return _page(draw, s, (w - span) / 2 - x0 * s, -y0 * s,
                 word, left, max(w, (w + span) / 2), dy=lift)


def _page(draw, s, tx, ty, word, left, right, dy=0.0):
    """One viewBox round a mark and a word, with the same margin on all four.

    The group's own stroke-width undoes its scale, so the two halves are laid
    at exactly one weight however big the mark is set.
    """
    x0, x1, y0, y1 = draw.reaches
    icon = (f'<g transform="translate({n(tx)} {n(ty)}) scale({n(s)})" '
            f'stroke-width="{n(STROKE / s)}">' + "".join(draw()) + "</g>")
    pad = STROKE / 2 + 1
    top = min(y0 * s + ty, CAP + dy) - pad
    bottom = max(y1 * s + ty, BASE + dy) + pad
    return (f"{n(left - pad)} {n(top)} {n(right - left + 2 * pad)} {n(bottom - top)}",
            [icon] + word)


# --- write it out ----------------------------------------------------------

def main():
    view = f"0 0 {GRID} {GRID}"
    for name, draw in EXPLORATIONS:
        write(os.path.join("explore", name), view, draw())

    primary = dict(EXPLORATIONS)[PRIMARY]
    write("icon", view, primary())
    write("wordmark-horizontal", *lockup_horizontal(primary))
    write("wordmark-vertical", *lockup_vertical(primary))
    print(f"wrote {len(os.listdir(OUT)) - 1} files + {len(os.listdir(os.path.join(OUT, 'explore')))} explorations in {OUT}")


if __name__ == "__main__":
    main()
