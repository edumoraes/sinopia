#!/usr/bin/env python3
"""Turns Sketchbook `.skbrushes` sets into the library the binary ships.

A `.skbrushes` file is a zip of one `<brushPresets>` XML and a 40x40 icon
per brush (plus an @2x copy). This reads the XML, maps every brush onto
the body `brush.rs` keeps, and writes:

    assets/brushes/library.json   the sets, their brushes, and the body
    assets/brushes/icons.png      every icon @2x, one grid cell each

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
        "texture_depth": round(f(paper, "paperTextureDepthMax", 0.0), 3) if textured else 0.0,
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


def wants_stamp(body):
    """Whether the brush is told apart by a shape or a texture of its
    own. It describes the brush, not its body: the canvas cannot stamp
    yet, so this is what says the icon promises a mark the ink cannot
    make."""
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
        # Every set ships its brushes at their factory settings, so the
        # copy is only written when one of them does not — half the file
        # otherwise, saying the same thing twice.
        if shipped != brush:
            entry["factory"] = shipped
        out.append(entry)
    return group.get("name") or Path(xml_name).stem, out


def png_grid(icons, cols):
    """Every icon into one RGBA sheet, row by row. Written by hand: the
    binary decodes PNG already, and a build step should not need a
    library the app does not."""
    rows = (len(icons) + cols - 1) // cols
    w, h = cols * ICON_PX, rows * ICON_PX
    sheet = bytearray(w * h * 4)
    for i, rgba in enumerate(icons):
        ox, oy = (i % cols) * ICON_PX, (i // cols) * ICON_PX
        for y in range(ICON_PX):
            src = y * ICON_PX * 4
            dst = ((oy + y) * w + ox) * 4
            sheet[dst:dst + ICON_PX * 4] = rgba[src:src + ICON_PX * 4]
    raw = b"".join(b"\x00" + bytes(sheet[y * w * 4:(y + 1) * w * 4]) for y in range(h))

    def chunk(kind, data):
        c = kind + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b"")), w, h


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
            sets.append({"name": title.replace("+", " "), "brushes": brushes})
            print(f"  {title:20} {len(brushes):3} brushes")

    cols = 16
    sheet, w, h = png_grid(icons, cols)
    (dst / "icons.png").write_bytes(sheet)
    (dst / "library.json").write_text(json.dumps(
        {"icon_px": ICON_PX, "icon_cols": cols, "icons": len(icons), "sets": sets},
        separators=(",", ":")) + "\n")

    total = sum(len(s["brushes"]) for s in sets)
    print(f"\n{total} brushes in {len(sets)} sets")
    print(f"  library.json  {(dst / 'library.json').stat().st_size / 1024:.0f} KB")
    print(f"  icons.png     {(dst / 'icons.png').stat().st_size / 1024:.0f} KB  ({w}x{h})")
    if missing:
        print(f"  no icon: {len(missing)} — {', '.join(missing[:5])}")


if __name__ == "__main__":
    main()
