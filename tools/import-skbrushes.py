#!/usr/bin/env python3
"""Turns Sketchbook `.skbrushes` sets into the library the binary ships.

A `.skbrushes` file is a zip of one `<brushPresets>` XML and a 40x40 icon
per brush (plus an @2x copy). This reads the XML, maps every brush onto
the body `brush.rs` keeps, and writes:

    assets/brushes/library.json   the sets, their brushes, and the body
    assets/brushes/icons.png      every icon @2x, one grid cell each
    assets/brushes/shapes.png     every nib and every paper

A brush with art of its own names a TIFF in the same zip, and the image
says its coverage in one of two places. The sets use both: one drawn
white on opaque black says it in its gray, and one drawn on transparency
says it in its alpha — the colour left there is whatever it happened to
be drawn in, and thirteen of the 114 nibs are drawn in black, so reading
the gray would take them for empty. Either way what comes out is a cell
of gray + alpha, the gray full and the alpha the coverage, exactly as
the glyph atlas is read.

Sketchbook has three kinds of art and one sheet carries all of them. Two
belong to the nib: a `shape` is a silhouette stamped in place of a round
dab, a `texture` is a grain worn over one, and the library says which a
brush names. The third is the `paperTexture`, and it belongs to the
canvas rather than to the nib — the nib is dragged over it, so it stands
still while the nib turns, and one tile of it covers hundreds of world
units instead of one dab. Forty-nine brushes turn one on, thirty of
those wear a nib as well, and a dab wearing both has one sheet to
sample.

Three things in the files are read and deliberately not carried, so
that nobody has to derive them twice:

  * `hardnessEdge`, off on 25 brushes and every one of them carrying
    art, says whether the Edge slider reaches the nib's own image. The
    flag is plain; what it *does* to a silhouette is stated nowhere,
    and the sets only hint — the brushes that turn it off park their
    Edge at one of two values instead of spreading it over the eight
    the rest use. Reading it wrong would change 96 of the 103 shape
    brushes on an invention.
  * `tiltFactor`, off 1.0 on 28 brushes, scales what the barrel's lean
    does to the nib. What this engine reads from a lean is its
    *direction* — which way the barrel points, which is what turns the
    nib — and a factor cannot scale a direction. It would have to
    multiply an effect that is not here.
  * `metaParameter`, on the 12 legacy brushes, is the band their size
    slider ran over. Brush Properties has one band for every brush; a
    per-brush one is a different control, not a missing number.

And one that is carried but not painted: `stampBlendStyle="glowBrush"`,
the Glow shelf's five. It is not like the other five styles, which read
the paint under the dab and so want another engine — an additive lay
needs no read-back and this one could do it. What stops it is the
board: ink that adds is invisible on a light one, and the shelf would
go from painting the wrong thing to painting nothing at all.

The originals are 34 MB and 438 MB of shape/texture TIFFs once opened, so
they stay out of the repo: only what this writes is committed. Run it
again with the sets in `brushes/` to rebuild.

    python3 tools/import-skbrushes.py brushes assets/brushes
"""

import json
import re
import struct
import sys
import zipfile
import zlib
from pathlib import Path

# The order the sets stand in the palette. Anything not named here comes
# after, alphabetically, so a new set added to `brushes/` still lands.
ORDER = [
    "Basic", "Legacy", "Markers", "Fine+Art", "Traditional", "Designer",
    "Artist", "Pastel", "Half+Tone", "Texture+Essentials", "Texture",
    "Shape", "Synthetic+Paint", "Splatter", "Glow", "Smudge", "Colorless",
]

ICON_PX = 80  # the @2x icon; the 1x copy is 40x40
# The shapes ship 128 to 1024 px square. A nib is a soft mask, not line
# art, and 128 is past what the widest brush anyone paints with resolves
# — the whole sheet is half a megabyte at it.
SHAPE_PX = 128
SHAPE_COLS = 12
# A paper is not a nib. One tile of it covers hundreds of world units
# rather than one dab, so a stroke crosses only a handful of its texels
# and a nib's 128 would read as blobs. It rides on the same sheet all
# the same — a dab wearing a nib and a paper at once has one texture to
# sample — in a band of its own under the nibs, at its own size, which
# has to divide the sheet's width.
#
# What is left on the cutting-room floor at 192 is noise. The seventeen
# photographic grains cost four fifths of the band and are the ones
# nobody can see a texel of; the geometric patterns, which are the whole
# of what the Half Tone shelf is, are line art that has to stay crisp
# and costs a tenth of it. Going to 256 keeps the grains at three world
# units to a texel instead of four and charges half a megabyte for it.
PAPER_PX = 192
# Where an image keeps its coverage. The two ways the sets are drawn do
# not blur into each other: every TIFF they ship is either opaque to
# within a hair — the alpha averages 0.998 and up, which is antialiasing
# at the border and nothing else — or plainly transparent, at 0.54 and
# down. So an image this opaque is read for its gray, and any other for
# its alpha.
OPAQUE = 0.99


def attrs(tag_body):
    return dict(re.findall(r'(\w+)="([^"]*)"', tag_body))


def tag(body, name):
    m = re.search(rf"<{name}\b([^>]*)/?>", body)
    return attrs(m.group(1)) if m else {}


def f(d, key, default=0.0):
    try:
        return float(d.get(key, default))
    except ValueError:
        return default


def dynamics(stroke):
    """Sketchbook's Rotation Dynamics: one choice, spread over two
    attributes. Following the stroke wins over the stylus's tilt, which
    is the order its own menu offers them in."""
    if stroke.get("rotateToStroke") == "true":
        return "ToStroke"
    return {"1": "Tilt", "2": "TiltAndRoll"}.get(stroke.get("tiltType", "0"), "None")


def driven(lo, hi):
    """How much of a property the pen's pressure drives: the gap between
    what a light touch gives and what a heavy one does, over the heavy
    one. A brush whose two ends agree is one pressure does not touch."""
    if hi <= 0.0:
        return 0.0
    return max(0.0, min(1.0, (hi - lo) / hi))


def read_brush(body):
    stroke = tag(body, "strokeParameters")
    params = tag(body, "brushParameters")
    paper = tag(body, "paperTexture")
    custom = tag(body, "customBrush")

    # The size is the radius the brush reaches at full pressure; the
    # document speaks diameters. `metaParameter` carries it for the six
    # legacy brushes only, so the radius is the one source that is
    # always there.
    max_radius = f(params, "maxRadius", 8.0)
    textured = paper.get("paperTextureEnabled") == "true"
    return {
        "size": round(max_radius * 2.0, 3),
        "opacity": round(f(stroke, "maxStrokeOpacity", 1.0), 4),
        "flow": round(f(params, "maxOpacity", 1.0), 4),
        "spacing": round(f(stroke, "spacingBias", 1.0), 3),
        "roundness": round(f(params, "squish", 1.0), 3),
        "rotation": round(f(params, "angle", 0.0), 2),
        "dynamics": dynamics(stroke),
        "hardness": round(f(stroke, "hardness", 1.0), 4),
        "profile": stroke.get("profile", "regularSolid"),
        # What one dab does to the ink already down. Ninety-one of the
        # 211 name something other than `normal`; only the eight
        # erasers are a thing the canvas can do without reading the
        # sheet back, and the rest are carried so the library can say
        # what it is not painting.
        "mark": stroke.get("stampBlendStyle", "normal"),
        "texture_depth": round(f(paper, "paperTextureDepthMax", 0.0), 3) if textured else 0.0,
        # What a dab does with the paint under it. Nothing reads the
        # canvas back yet, so these are carried and not painted — but
        # they are what the Smudge and Colorless shelves are made of,
        # and a library that did not say so would be lying.
        "strength": round(f(params, "strength"), 4),
        "blending": round(f(params, "blending"), 4),
        "dilution": round(f(params, "dilution"), 4),
        "jitter": {
            "size": round(f(stroke, "radiusJitter"), 3),
            "opacity": round(f(stroke, "opacityJitter"), 3),
            "flow": round(f(stroke, "strokeOpacityJitter"), 3),
            "rotation": round(f(stroke, "rotationJitter"), 2),
            "spacing": round(f(stroke, "spacingNoise"), 3),
        },
        "pressure": {
            "size": round(driven(f(params, "minRadius"), max_radius), 4),
            "opacity": round(driven(f(stroke, "minStrokeOpacity"),
                                    f(stroke, "maxStrokeOpacity", 1.0)), 4),
            "flow": round(driven(f(params, "minOpacity"),
                                 f(params, "maxOpacity", 1.0)), 4),
        },
    }


def art_of(body):
    """The nib image a brush carries: `("shape", name)` for a silhouette
    it stamps in place of a round dab, `("grain", name)` for one it
    wears over one, or None. The name is the TIFF's, without its
    extension, which is what the sheet and a saved board call it."""
    custom = tag(body, "customBrush")
    if custom.get("type", "off") == "off":
        return None
    kind = {"shape": "shape", "texture": "grain"}.get(custom.get("textureImageType"))
    name = custom.get("name")
    return (kind, name.rsplit(".", 1)[0]) if kind and name else None


def paper_of(body):
    """The paper a brush drags its nib over, or None for the 162 that
    turn it off: the TIFF it names, whether it is used inverted, and how
    far one tile of it is stretched.

    Sketchbook adjusts a paper by a brightness and a contrast as well,
    and those go where the three randomness amounts went. It does not say
    what its numbers mean, and the one band they could plainly be — the
    -100..100 every brightness and contrast control uses, and that
    ImageMagick's own takes — turns eight of the 49 papers solid black,
    one of them under a brush named Textured Pencil. A brush that paints
    nothing is not what anybody ships, so the reading is wrong, and the
    canvas does not guess. The invert is a flag and says exactly what it
    means, so it is baked into the cell instead of carried: the same TIFF
    used both ways is two cells under two names."""
    paper = tag(body, "paperTexture")
    if paper.get("paperTextureEnabled") != "true" or not paper.get("name"):
        return None
    return {
        "stem": paper["name"].rsplit(".", 1)[0],
        "invert": paper.get("paperTextureInvert") == "true",
        "scale": f(paper, "paperTextureScale", 1.0),
    }


def paper_key(paper):
    """What the sheet and a saved board call one paper: the TIFF's own
    name, and `~i` after it when the cell was baked inverted — so a name
    is one image and not one image under two readings."""
    return paper["stem"] + ("~i" if paper["invert"] else "")


def read_set(zf, xml_name):
    x = zf.read(xml_name).decode("utf-8")
    group = tag(x, "group")
    out = []
    # A `<brush>` carries its settings, then a `<default>` repeating what
    # it shipped as. Everything before `<default>` is the live brush.
    for m in re.finditer(r'<brush name="([^"]*)">(.*?)</brush>', x, re.S):
        name, body = m.group(1), m.group(2)
        cut = body.find("<default>")
        live = body[:cut] if cut >= 0 else body
        factory = body[cut:] if cut >= 0 else body
        person = tag(live, "personalized")
        brush, shipped = read_brush(live), read_brush(factory)
        entry = {
            "name": person.get("name") or name,
            "icon": person.get("icon", ""),
            "brush": brush,
        }
        art = art_of(live)
        if art:
            entry[art[0]] = art[1]
        # What the paper asks for. `main` holds the zip, and puts back
        # the cell it was baked into and the period it stretches to.
        paper = paper_of(live)
        if paper:
            entry["paper"] = paper
        # Every set ships its brushes at their factory settings, so the
        # copy is only written when one of them does not — half the file
        # otherwise, saying the same thing twice.
        if shipped != brush:
            entry["factory"] = shipped
        out.append(entry)
    return group.get("name") or Path(xml_name).stem, out


def encode_png(sheet, w, h, channels):
    """One sheet of texels as a PNG. `channels` is 4 for the icons, which
    carry their own colors, and 2 for the nibs and papers, which are a
    full gray under the coverage — the same white-with-alpha the glyph
    atlas is. Written by hand: the binary decodes PNG already, and a
    build step should not need a library the app does not."""
    color_type = {4: 6, 2: 4}[channels]
    stride = w * channels
    raw = b"".join(b"\x00" + bytes(sheet[y * stride:(y + 1) * stride]) for y in range(h))

    def chunk(kind, data):
        c = kind + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color_type, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def blit(sheet, w, channels, cell, px, x, y):
    """One `px`-square cell into the sheet, its corner at `(x, y)` px."""
    stride, run = w * channels, px * channels
    for r in range(px):
        dst = (y + r) * stride + x * channels
        sheet[dst:dst + run] = cell[r * run:(r + 1) * run]


def png_grid(cells, px, cols, channels):
    """Every cell into a sheet of its own, row by row."""
    rows = -(-len(cells) // cols)
    w, h = cols * px, rows * px
    sheet = bytearray(h * w * channels)
    for i, cell in enumerate(cells):
        blit(sheet, w, channels, cell, px, (i % cols) * px, (i // cols) * px)
    return encode_png(sheet, w, h, channels), w, h


def stamp_sheet(shapes, papers):
    """The one sheet the canvas stamps from: the nibs in a `SHAPE_PX`
    grid `SHAPE_COLS` across, and under them the papers in a band of
    their own, `PAPER_PX` a side and as many across as the sheet's width
    takes. Each is placed in the order it is given, so a name's cell is
    its place in its own list and nothing else has to be recorded.

    They share a sheet because a dab can wear both at once: 30 of the 49
    papered brushes carry a nib of their own as well, and two textures
    would be two draws of what is one dab."""
    w = SHAPE_COLS * SHAPE_PX
    if w % PAPER_PX:
        raise ValueError(f"{PAPER_PX} px papers do not divide a {w} px sheet")
    top = -(-len(shapes) // SHAPE_COLS) * SHAPE_PX
    across = w // PAPER_PX
    h = top + -(-len(papers) // across) * PAPER_PX
    sheet = bytearray(h * w * 2)
    for i, art in enumerate(shapes):
        blit(sheet, w, 2, art, SHAPE_PX,
             (i % SHAPE_COLS) * SHAPE_PX, (i // SHAPE_COLS) * SHAPE_PX)
    for i, art in enumerate(papers.values()):
        blit(sheet, w, 2, art, PAPER_PX,
             (i % across) * PAPER_PX, top + (i // across) * PAPER_PX)
    return encode_png(sheet, w, h, 2), w, h


def art_cell(data, px, through=()):
    """One TIFF into a `px` square of gray + alpha: whichever channel the
    image keeps its coverage in goes into the alpha, `through` on the
    way, and the color stays full — which is what makes `texel * ink` the
    mark, exactly as the glyph atlas works."""
    import subprocess
    opaque = float(subprocess.run(
        ["magick", "identify", "-format", "%[fx:mean.a]", "tif:-"],
        input=data, capture_output=True, check=True).stdout) >= OPAQUE
    read = ["-alpha", "off", "-colorspace", "gray"] if opaque else ["-alpha", "extract"]
    out = subprocess.run(
        ["magick", "tif:-", *read, *through,
         "-resize", f"{px}x{px}!", "-depth", "8", "gray:-"],
        input=data, capture_output=True, check=True).stdout
    if len(out) != px * px:
        raise ValueError(f"art is {len(out)} bytes, not {px * px}")
    return b"".join(b"\xff" + bytes([v]) for v in out)


def shape_cell(data):
    """A nib's own image. A shape and a grain are read the same way; what
    differs is whether the dab is the image or wears it."""
    return art_cell(data, SHAPE_PX)


def paper_cell(data, paper):
    """A paper's image, at a paper's size and inverted where the brush
    asks for it. Answers the source's own width beside it: that is what
    `paperTextureScale` stretches, so one tile covers the image's size
    and not the cell's."""
    import subprocess
    width = int(subprocess.run(
        ["magick", "identify", "-format", "%w", "tif:-"],
        input=data, capture_output=True, check=True).stdout)
    return art_cell(data, PAPER_PX, ["-negate"] if paper["invert"] else []), width


def decode_png(data):
    """The icons are 80x80 RGBA PNGs; this returns their texels. Only the
    shapes Sketchbook actually writes are handled, and anything else is
    reported rather than guessed at."""
    import subprocess
    out = subprocess.run(
        ["magick", "png:-", "-depth", "8", "RGBA:-"],
        input=data, capture_output=True, check=True).stdout
    want = ICON_PX * ICON_PX * 4
    if len(out) != want:
        raise ValueError(f"icon is {len(out)} bytes, not {want}")
    return out


def main():
    src = Path(sys.argv[1] if len(sys.argv) > 1 else "brushes")
    dst = Path(sys.argv[2] if len(sys.argv) > 2 else "assets/brushes")
    dst.mkdir(parents=True, exist_ok=True)

    found = {p.stem: p for p in sorted(src.glob("*.skbrushes"))}
    order = [n for n in ORDER if n in found] + sorted(set(found) - set(ORDER))
    if not order:
        sys.exit(f"no .skbrushes in {src}")

    sets, icons, missing = [], [], []
    # Name to cell, in the order the shapes are met. A shape is shared
    # by every brush that names it, and the name is what a saved board
    # keeps — an index would move the day a set is added.
    shapes, no_shape = {}, []
    # And the papers, keyed by the reading they were baked under: one
    # TIFF used both ways is two cells. `periods` is how wide the source
    # of each is, which is what the brush's own scale stretches.
    papers, periods, no_paper = {}, {}, []
    for stem in order:
        with zipfile.ZipFile(found[stem]) as zf:
            names = zf.namelist()
            xml = next(n for n in names if n.endswith(".xml"))
            title, brushes = read_set(zf, xml)
            for b in brushes:
                key = f"{b['icon']}@2x.png"
                if key in names:
                    b["icon"] = len(icons)
                    icons.append(decode_png(zf.read(key)))
                else:
                    missing.append(f"{title}/{b['name']}")
                    b["icon"] = None
                shape = b.get("shape") or b.get("grain")
                if shape and shape not in shapes:
                    # By stem, and only among the TIFFs: one set ships a
                    # shape named after the set itself, and its `.xml`
                    # would otherwise answer first.
                    tif = next((n for n in names
                                if n.lower().endswith(".tif")
                                and n.rsplit(".", 1)[0] == shape), None)
                    if tif is None:
                        no_shape.append(f"{title}/{b['name']}")
                        b.pop("shape", None)
                        b.pop("grain", None)
                    else:
                        shapes[shape] = shape_cell(zf.read(tif))
                paper = b.get("paper")
                if paper:
                    key = paper_key(paper)
                    if key not in papers:
                        tif = next((n for n in names
                                    if n.lower().endswith(".tif")
                                    and n.rsplit(".", 1)[0] == paper["stem"]),
                                   None)
                        if tif is not None:
                            papers[key], periods[key] = paper_cell(zf.read(tif),
                                                                   paper)
                    if key in papers:
                        # One tile in world units: the image's own size,
                        # stretched by what the brush asks for.
                        b["paper"] = {
                            "name": key,
                            "period": round(periods[key] * paper["scale"], 2),
                        }
                    else:
                        no_paper.append(f"{title}/{b['name']}")
                        b.pop("paper")
            sets.append({"name": title.replace("+", " "), "brushes": brushes})
            print(f"  {title:20} {len(brushes):3} brushes")

    cols = 16
    sheet, w, h = png_grid(icons, ICON_PX, cols, 4)
    (dst / "icons.png").write_bytes(sheet)
    nibs, sw, sh = stamp_sheet(list(shapes.values()), papers)
    (dst / "shapes.png").write_bytes(nibs)
    (dst / "library.json").write_text(json.dumps(
        {"icon_px": ICON_PX, "icon_cols": cols, "icons": len(icons),
         "shape_px": SHAPE_PX, "shape_cols": SHAPE_COLS, "shapes": list(shapes),
         "paper_px": PAPER_PX, "papers": list(papers),
         "sets": sets},
        separators=(",", ":")) + "\n")

    total = sum(len(s["brushes"]) for s in sets)
    stamping = sum(1 for s in sets for b in s["brushes"] if b.get("shape"))
    grained = sum(1 for s in sets for b in s["brushes"] if b.get("grain"))
    papered = sum(1 for s in sets for b in s["brushes"] if b.get("paper"))
    print(f"\n{total} brushes in {len(sets)} sets, "
          f"{stamping} stamping a shape, {grained} wearing a grain "
          f"and {papered} dragged over a paper")
    print(f"  library.json  {(dst / 'library.json').stat().st_size / 1024:.0f} KB")
    print(f"  icons.png     {(dst / 'icons.png').stat().st_size / 1024:.0f} KB  ({w}x{h})")
    print(f"  shapes.png    {(dst / 'shapes.png').stat().st_size / 1024:.0f} KB  "
          f"({sw}x{sh}, {len(shapes)} nibs and {len(papers)} papers)")
    if missing:
        print(f"  no icon: {len(missing)} — {', '.join(missing[:5])}")
    if no_shape:
        print(f"  no shape image: {len(no_shape)} — {', '.join(no_shape[:5])}")
    if no_paper:
        print(f"  no paper image: {len(no_paper)} — {', '.join(no_paper[:5])}")


if __name__ == "__main__":
    main()
