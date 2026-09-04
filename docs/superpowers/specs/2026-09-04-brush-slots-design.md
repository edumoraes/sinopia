# The brush strip, and the library beside it

Source: `docs/boards/brush-context/board.png`, a sketch of the left-hand
chrome the brush tool brings up.

Today one panel does two jobs: a preview block naming the brush in the
hand, and under it the whole of Sketchbook's Brush Library — 211 brushes
in seventeen shelves, scrolled. Picking a brush you already know means
walking that list. The sketch splits the two: a narrow **strip** that says
which brush is in the hand and keeps ten within reach, and the library as
a second panel that opens beside it when there is something to go looking
for.

## What the sketch says

- A header: the brush's own icon, its name, the shelf it came off.
- Under it two buttons: sliders open Brush Properties, a chevron opens
  the full library.
- Under those, ten slots in a column, numbered `1`–`9` and then `0`.
  Slots 1–9 are defaults; **slot 0 is the last brush used that is not in
  the default slots**.

## Decisions

**The two panels stand side by side.** The chevron opens the library as a
panel of its own to the right of the strip, with canvas between them.
Both have to be on show at once because a brush is put into a slot by
dragging it out of the library.

**Slots 1–9 ship with the first nine of Basic** — Textured Pencil,
Textured Marker, Pressure Airbrush, Technical Pen, 80% Inking Pen,
Textured Watercolor, Textured Inker, Natural Blur, Auto Eraser Soft.
That is the shelf Sketchbook puts first, and it covers pencil, marker,
airbrush, pen, ink, watercolour, blur and eraser — a whole hand without
anybody picking one out.

**`Shift`+digit takes a slot; the bare digits keep the opacity.**
`Shift+1` is a different character on every layout, so the key is read
through winit's `key_without_modifiers()` rather than the character that
arrives — layout-aware, the way the tools' own letters already are.

**The reset button moves to the properties bar**, beside the dot that
already says the brush is off what it shipped as. The header is left with
the two buttons the sketch draws.

## The model

A slot names its brush the way `Edits::held` does — by set and name,
never by index, because a set added to `brushes/` moves every index after
it and a file written last week would then dress the wrong brush.

- `SLOT_DEFAULTS`: the nine, as a const of `(set, name)`.
- `Library::select` gains one consequence: taking up a brush that is
  **not** in slots 1–9 writes it into slot 0. Taking up one that is
  leaves slot 0 alone.
- Assigning a brush to a slot 1–9 clears slot 0 if that was the brush
  sitting there — slot 0 holds a brush only while it is outside the
  defaults.
- `Edits.slots` is absent on disk when the slots are the shipped nine,
  the way a raster layer's kind is absent: a `brushes.json` written
  before slots existed opens meaning what it meant. A name this build no
  longer carries leaves that slot empty, exactly as `Library::apply`
  passes over a brush that has gone.

## The strip — `src/slots.rs`

Pure and tested, like every other panel: `app` asks where a click landed
and what to draw.

132 logical px wide. The header carries the icon at 34, the name and the
shelf beside it on two lines, then a line of what the brush actually
lays — the icon is Sketchbook's art and may promise a mark the canvas
cannot stamp; the dab is the part that cannot, and it is why the preview
had one. Under it the two buttons, sliders at the left and the chevron at
the right. Then ten cells of 32: the brush's icon, its number in the
corner in `theme.muted`, and the one in the hand wearing the same ring
the library's grid uses.

Ten is a fixed, small number, so the column does not scroll. What a short
window has no room for is cut by `Prim::clipped` — the keyboard reaches
every slot regardless.

## The library — `palette.rs`

Loses the preview block and its two buttons; keeps the shelves, the grid,
the band, the scroll and the thumb. It exists only while the chevron has
opened it, and it stands to the right of the strip. Hiding the strip
(`Shift+B`) hides it too: it is opened from inside the strip, so it does
not outlive it.

## Dragging a brush into a slot

The layers panel's pattern without its easing. A press on a library cell
may become a click or a drag; past the slop `app` carries the brush's
icon behind the pointer and the slot under it stands out. The release
writes it. Releasing outside, or over slot 0, writes nothing — slot 0 is
computed, not assigned.

## Testing

- `slots`: layout, hit, prims, the shipped nine, the clip.
- `brush`: seeding, `select` writing slot 0, an assignment clearing it,
  a round trip through `Edits`.
- `palette`: what it had, less the preview's tests.
- `props`: the reset button lays out and is hit.
