# Layers at parity — design

The layers panel was a flat list of cards: one stack at a time, a frame
"entered" by rewriting the list, add and remove in the header, a card
that lifted the moment it was pressed. This cut brings the model and the
panel to what Photoshop, Affinity and Krita users expect, without giving
up what makes this board what it is: every layer is the object it holds,
and ink is curves, not pixels.

## The model

A `Layer` gains five fields, all absent on disk at their default, so every
board written before them opens meaning what it meant:

| field | default | meaning |
|---|---|---|
| `opacity` | `1` | the layer composited as one, at this strength |
| `blend` | `normal` | how it meets what is under it (27 modes + `passThrough`) |
| `locked` | `false` | its content cannot be drawn on, picked, moved or merged |
| `color` | none | a tag: red, orange, yellow, green, blue, violet, gray |
| `layers` | `[]` | a **group**'s children, bottom to top |

`kind: "group"` is the fourth kind. A group holds layers, never an
element; it nests. A **frame stands only at the board's root** — never
inside a group, never inside a frame — which is Photoshop's rule for
artboards and keeps "frames do not nest" a one-line truth. A frame's own
stack stays on the frame element, as before, and may hold groups.

`passThrough` is valid only on a group, and is what a new group is born
with: its children blend with what is under the group as if the group
were not there. Every other mode isolates the group — its children are
composited together first, then laid on what is under them.

## Addressing

A stack is named by the layer that holds it: `None` is the board's root,
a group's id is its children, a frame layer's id is its frame's stack.
One scheme for all three, so the panel's tree, the editor and the CLI
all speak layer ids. `inside` — the frame the panel "stood in" — is gone:
the panel shows the whole tree, and where work happens is decided by the
active layer.

## The panel

A tree. Every row is a card, indented by its depth; a group or a frame
carries a chevron that **expands it in place**. Expansion is per tab
session state and is not undone. Picking a layer on the canvas reveals
its row, expanding what hides it.

- **Rows** show, left to right: the eye (its cell tinted by the colour
  tag), the chevron, a thumbnail (raster and vector) or the kind's icon
  (a folder, a frame), the name, and a lock when locked. Hidden rows are
  muted.
- **Kinds read apart**: a thumbnail with a pixel badge (raster) or a pen
  badge (vector); a folder, open or closed (group); a frame glyph on a
  tinted card (frame).
- **Header**: the title, and a filter toggle. The filter bar searches by
  name, and narrows by kind and by colour tag; a match brings its
  ancestors with it, so it is never shown out of context.
- **Properties bar**: blend mode (a menu, previewed on hover), opacity
  (a slider), lock — for the picked layers.
- **Footer**: new group, new layer, delete — the bottom of the panel, as
  everywhere else.
- **Multi-select**: click picks one, `Ctrl`+click toggles one in or out,
  `Shift`+click takes the range from the anchor, in row order.
- **Drag**: a card lifts only once the pointer has travelled past the
  slop — never on a click, never on the double click that renames. The
  drop is shown as a line between rows at the depth it lands at, or an
  outline around the container it goes into; it is applied on release,
  and the cards slide to their new rows. Every picked layer travels.
- **Context menu** (right click on a row): rename, duplicate, copy, cut,
  paste, delete, group, ungroup, merge, merge visible, flatten, lock,
  colour tag.

## Selection is one selection

Picking layers in the panel, with the Select tool in hand, selects their
objects on the canvas — a group's are all its descendants', a frame's is
the frame — leaving out what is hidden or locked. Picking objects on the
canvas picks their layers. The **active** layer is the anchor of the
pick: new ink lands there, and `Shift` ranges from it.

## Where ink lands

A stroke belongs to the frame its press landed in, as before. Then: the
active layer, when it is a raster layer in that same frame (or on the
board for a board press); otherwise a fresh layer — above the active one
when the active layer is in that frame, inside it at the top when it is a
group, at the top of that frame's stack when it is elsewhere. A stroke
that would join a locked layer, or open one inside a locked container, is
refused at the press, and the cursor says so before the press.

## Lock

Locked content cannot be drawn on, picked, marqueed, moved, transformed,
deleted from the canvas, merged, or have its opacity or blend changed. A
locked group locks everything in it. What stays open is what is about the
layer and not its content: its name, its colour tag, its visibility, its
place in the stack, and deleting it from the panel.

## Compositing

A layer, a frame or an isolated group whose opacity is below 1 or whose
mode is not normal is drawn onto a surface of its own and laid on what
is under it once. A group whose children blend, isolated, is too. Nothing
else is: a board with no opacity and no modes renders exactly as before,
through the same passes.

- Surfaces nest (a group in a group), one per depth.
- **Blend modes read the backdrop**: before a surface is laid with a mode,
  the region under it is copied out, and the lay samples both. The math
  is the W3C Compositing and Blending formulas (separable and
  non-separable), plus Photoshop's own (linear burn/dodge, vivid, linear
  and pin light, hard mix, subtract, divide, darker/lighter colour,
  dissolve), evaluated on gamma-encoded values the way Photoshop does.
- **Pass-through** at full opacity is no surface at all; below it, the
  surface opens on a copy of the backdrop and is laid back as a mix.
- An eraser still rubs out its own paint and nothing else.

## Merging

Only siblings merge — layers of one stack — which is what keeps the
result where it was. Frames never merge.

- **Merge down**: the active layer into the one under it; the result keeps
  the lower one's name and id.
- **Merge layers**: the picked siblings into one, named after the topmost.
- **Merge visible**: every visible sibling in each stack; on the board's
  root, frames are barriers — what is under a frame stays under it.
- **Flatten**: merge visible, then hidden layers are discarded.

A merge is **structural** when that is exact: every layer involved is
normal at full opacity, so moving the objects onto one layer, in paint
order, draws the same picture — and the curves stay curves. When it is
not exact the merged siblings are **rasterized**: their composite is drawn
offscreen and they become one layer holding that image, which is what a
pixel editor's merge is.

## Copy, cut, paste, duplicate

A clip is a document fragment — the picked layers with their subtrees and
what stands on them — validated by the board's own parse on the way back.
It is offered on the system clipboard under its own MIME type, so `Ctrl+V`
pastes whatever was copied last: the board's layers, or an image from
elsewhere. Every id is minted anew; the paste lands above the active
layer, in place when that place is on screen and in the middle of the view
otherwise. Duplicate is copy and paste in one step, in place, above each
original.

## The CLI

`omawhite layer …` drives every one of these on the open board: list,
add, remove, rename, move, show/hide, lock/unlock, opacity, blend, color,
group, ungroup, duplicate, merge, merge-down, merge-visible, flatten,
select, expand/collapse. Each is its own op on the closed schema, answered
by the loop like an agent's three, and each is one undo step. The CLI
resolves a name to an id the way `agent read` does; the protocol speaks
ids.

## Shortcuts

`Ctrl+G` group, `Ctrl+Shift+G` ungroup, `Ctrl+J` duplicate,
`Ctrl+Shift+N` new layer, `Ctrl+[`/`Ctrl+]` down/up,
`Ctrl+Shift+[`/`]` bottom/top, `Ctrl+Alt+E` merge (down, or the picked
layers), `Ctrl+Shift+E` merge visible, `Ctrl+/` lock, `Ctrl+,` show/hide,
`F2` rename, `Ctrl+C`/`X`/`V` copy/cut/paste layers, and with a tool that
does not paint, the digits set the picked layers' opacity (`1` 10% … `0`
100%). `Ctrl+E` stays the export to the agent.
