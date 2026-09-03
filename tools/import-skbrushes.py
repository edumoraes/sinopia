#!/usr/bin/env python3
"""Turns Sketchbook `.skbrushes` sets into the library the binary ships.

A `.skbrushes` file is a zip of one `<brushPresets>` XML and a 40x40 icon
per brush (plus an @2x copy). This reads the XML, maps every brush onto
the body `brush.rs` keeps, and writes:

    assets/brushes/library.json   the sets, their brushes, and the body
    assets/brushes/icons.png      every icon @2x, one grid cell each
    assets/brushes/shapes.png     every nib shape, one grid cell each

A brush with a nib of its own names a TIFF in the same zip — white on
black, the nib's own coverage. Those become the nib sheet: gray + alpha,
the gray full and the alpha the coverage, exactly as the glyph atlas is
read. Sketchbook has two kinds and the sheet carries both: a `shape` is
a silhouette stamped in place of a round dab, a `texture` is a grain
worn over one, and the library says which a brush names. A
`paperTexture` is neither — it belongs to the canvas, not to the nib —
and this passes over it, though its art is here too: 49 brushes turn a
paper on, they name 30 TIFFs between them, and every one of those is in
the sets.

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


def wants_stamp(body):
    """Whether the brush is told apart by a nib or a paper of its own.
    It describes the brush, not its body: with a `shape` or a `grain`
    beside it, it is the paper — which belongs to the canvas, and which
    nothing here reads yet — that says the icon promises a mark the ink
    cannot make."""
    custom = tag(body, "customBrush")
    paper = tag(body, "paperTexture")
    return (custom.get("type", "off") != "off"
            or custom.get("name") is not None
            or paper.get("paperTextureEnabled") == "true")


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
            "stamp": wants_stamp(live),
            "brush": brush,
        }
        art = art_of(live)
        if art:
            entry[art[0]] = art[1]
        # Every set ships its brushes at their factory settings, so the
        # copy is only written when one of them does not — half the file
        # otherwise, saying the same thing twice.
        if shipped != brush:
            entry["factory"] = shipped
        out.append(entry)
    return group.get("name") or Path(xml_name).stem, out


def png_grid(cells, px, cols, channels):
    """Every cell into one sheet, row by row. `channels` is 4 for the
    icons, which carry their own colors, and 2 for the shapes, which are
    a full gray under the coverage — the same white-with-alpha the glyph
    atlas is. Written by hand: the binary decodes PNG already, and a
    build step should not need a library the app does not."""
    color_type = {4: 6, 2: 4}[channels]
    rows = (len(cells) + cols - 1) // cols
    w, h = cols * px, rows * px
    stride, run = w * channels, px * channels
    sheet = bytearray(h * stride)
    for i, cell in enumerate(cells):
        ox, oy = (i % cols) * px, (i // cols) * px
        for y in range(px):
            src = y * run
            dst = (oy + y) * stride + ox * channels
            sheet[dst:dst + run] = cell[src:src + run]
    raw = b"".join(b"\x00" + bytes(sheet[y * stride:(y + 1) * stride]) for y in range(h))

    def chunk(kind, data):
        c = kind + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color_type, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b"")), w, h


def shape_cell(data):
    """One nib TIFF into a `SHAPE_PX` square of gray + alpha. The
    originals are white on black — the nib's own coverage — so the gray
    goes in the alpha and the color stays full, which is what makes
    `texel * ink` the mark. A shape and a grain are read the same way;
    what differs is whether the dab is the image or wears it."""
    import subprocess
    out = subprocess.run(
        ["magick", "tif:-", "-colorspace", "gray",
         "-resize", f"{SHAPE_PX}x{SHAPE_PX}!", "-depth", "8", "gray:-"],
        input=data, capture_output=True, check=True).stdout
    want = SHAPE_PX * SHAPE_PX
    if len(out) != want:
        raise ValueError(f"shape is {len(out)} bytes, not {want}")
    return b"".join(b"\xff" + bytes([v]) for v in out)


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
            sets.append({"name": title.replace("+", " "), "brushes": brushes})
            print(f"  {title:20} {len(brushes):3} brushes")

    cols = 16
    sheet, w, h = png_grid(icons, ICON_PX, cols, 4)
    (dst / "icons.png").write_bytes(sheet)
    nibs, sw, sh = png_grid(list(shapes.values()), SHAPE_PX, SHAPE_COLS, 2)
    (dst / "shapes.png").write_bytes(nibs)
    (dst / "library.json").write_text(json.dumps(
        {"icon_px": ICON_PX, "icon_cols": cols, "icons": len(icons),
         "shape_px": SHAPE_PX, "shape_cols": SHAPE_COLS, "shapes": list(shapes),
         "sets": sets},
        separators=(",", ":")) + "\n")

    total = sum(len(s["brushes"]) for s in sets)
    stamping = sum(1 for s in sets for b in s["brushes"] if b.get("shape"))
    grained = sum(1 for s in sets for b in s["brushes"] if b.get("grain"))
    print(f"\n{total} brushes in {len(sets)} sets, "
          f"{stamping} stamping a shape and {grained} wearing a grain")
    print(f"  library.json  {(dst / 'library.json').stat().st_size / 1024:.0f} KB")
    print(f"  icons.png     {(dst / 'icons.png').stat().st_size / 1024:.0f} KB  ({w}x{h})")
    print(f"  shapes.png    {(dst / 'shapes.png').stat().st_size / 1024:.0f} KB  "
          f"({sw}x{sh}, {len(shapes)} nibs)")
    if missing:
        print(f"  no icon: {len(missing)} — {', '.join(missing[:5])}")
    if no_shape:
        print(f"  no shape image: {len(no_shape)} — {', '.join(no_shape[:5])}")


if __name__ == "__main__":
    main()
