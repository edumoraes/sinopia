# Brush and layers

A brush tool (`B`) in the Photoshop/Procreate mould — size, opacity,
hardness — and layers to paint on, with a panel on the right that
`Shift+L` shows and hides.

## Why this is not a small change

The pencil is the only ink so far: two world units wide, opaque, crisp,
one path per stroke. A brush is a stroke with a body — soft edges and a
translucency of its own — and the renderer cannot draw one today: a
stroke is a chain of round-capped segments, and wherever two overlap,
their alpha adds up. At full opacity with a crisp edge that overlap is a
pixel wide and invisible; with a soft edge or half opacity every joint
shows as a bead. So a brush stroke has to be composited as one shape,
which is a new pass in `gfx`.

Layers are new structure in the document: every element joins one, the
layers have an order and a visibility, and there is an active layer new
ink lands on. That touches the document, hit-testing, paint order and
the editor — and it needs the first panel that is not the dock or the
strip.

## Decisions

- **The brush writes a `path`.** Same element as the pencil, with two
  new optional fields, `opacity` and `hardness` (0–1, absent when 1).
  Selection, transforms and the future export treat both inks alike;
  the pencil is a brush with fixed settings.
- **A stroke is composited as one shape** when it is soft or
  translucent: its segments are drawn into an offscreen texture with a
  *max* blend — the union of their coverage — and that texture is laid
  on the frame once, at the stroke's opacity. Crisp opaque strokes keep
  the direct path; nothing changes for existing boards.
- **Brush settings are session state**, global to the window (as in
  Photoshop), adjusted from the keyboard: `[` `]` for size, `{` `}` for
  hardness, `1`–`9`, `0` for opacity. The pointer shows a ring the size
  of the brush.
- **Layers are additive to the schema.** `Document.layers` and a
  `layer` field on every element; both default, so schema 1 boards
  load unchanged and get one layer on the way in.
- **The panel does the four things a layer needs**: pick, show/hide,
  add/remove, move up/down. Renaming needs text input, which does not
  exist yet; layer opacity needs the same offscreen pass as a brush
  stroke, nested, and waits for it.

## Document

```json
{
  "schema": 1,
  "layers": [
    { "id": "01J…", "name": "Layer 1" },
    { "id": "01J…", "name": "Layer 2", "visible": false }
  ],
  "elements": [
    { "id": "el_04", "type": "path", "layer": "01J…",
      "curves": [...], "stroke": "#1f1f1f", "width": 16,
      "opacity": 0.5, "hardness": 0.5 }
  ]
}
```

- `layers` is bottom to top and never empty in memory. A board without
  the field gets one layer, `Layer 1`, with a fresh ULID.
- `visible` is absent when true.
- Every element names its layer. Absent — every board written before
  this — means the first layer. A name no layer carries is an error on
  load, as is a duplicate layer id: the parse stays closed.
- `opacity` and `hardness` are clamped to nothing: outside 0–1 they are
  an error.

Paint order is the layers' order, then document order within a layer.
`Document::painted()` yields `(index, &Element)` in that order, hidden
layers skipped; `scene` and `select` read the board through it, so the
renderer and the pointer agree on what is on top.

Layer operations live on `Document`: `add_layer(above)` names the new
one `Layer N` with N past the highest in use; `remove_layer(index)`
drops the layer and its elements and refuses the last one;
`move_layer(index, up)` swaps with the neighbour.

## Modules

### `brush.rs` — new, pure

```rust
pub struct Brush { pub size: f64, pub opacity: f64, pub hardness: f64 }
pub struct Tip { pub width: f64, pub opacity: f64, pub hardness: f64 }

impl Brush {
    pub fn grow(&mut self) / shrink(&mut self);      // Photoshop's steps
    pub fn harder(&mut self) / softer(&mut self);    // ±0.25
    pub fn set_opacity_digit(&mut self, digit: u8);  // 1..9 → 10%..90%, 0 → 100%
    pub fn tip(&self) -> Tip;
}
impl Tip { pub const PENCIL: Tip; pub fn of(path: &Path) -> Tip; pub fn is_direct(&self) -> bool; }
pub fn ring_prims(center, radius_px, scale, color) -> Vec<Prim>;
```

Defaults: size 16 (world units), opacity 1, hardness 0.5. Size runs
1–500 in Photoshop's steps (1 below 10, 10 below 100, 25 below 200, 50
below 300, then 100).

Hardness maps onto the SDF ramp: for a stroke of radius `r` px, feather
`f = (1 − h)·r` and geometry radius `r − f/2`, so coverage is 1 at the
centre, ½ at `r − f/2` and 0 at `r` — the stroke never grows past its
nominal width, it only softens inside it.

### `scene.rs` — frames, groups and passes

```rust
pub struct Frame { pub prims: Vec<Prim>, pub groups: Vec<Group> }
pub struct Group { pub start: u32, pub end: u32, pub opacity: f32, pub bounds: ScreenRect }

impl Frame {
    pub fn extend(&mut self, prims);
    pub fn group(&mut self, prims, opacity);      // bounds from the prims + ramp
    pub fn stroke(&mut self, prims, tip: &Tip);   // direct or grouped, by the tip
    pub fn append(&mut self, other: Frame);
}

pub fn document_prims(doc, view, images) -> Frame;
pub fn path_prims(curves, tip, color, view) -> Vec<Prim>;
pub fn stroke_prims(points, tip, color, view) -> Vec<Prim>;

pub enum Pass {
    Direct { composite: Option<u32>, start: u32, end: u32 },
    Offscreen { wipe: u32, start: u32, end: u32 },
}
pub fn passes(frame: &Frame, viewport: Viewport, scratch: u32) -> (Vec<Prim>, Vec<Pass>);
```

`passes` is the whole compositing plan, computed on the CPU where it can
be tested: it appends one *wipe* box and one *composite* box per group
to the prim list and cuts the list into passes. A group whose bounds
miss the viewport is dropped, prims and all. The composite box samples
the scratch texture over the group's bounds (`Prim::composite`) with
`[opacity; 4]` as its tint — the scratch holds premultiplied colour, so
opacity scales every channel.

### `gfx.rs` — one more texture, three more pipelines

- A viewport-sized *scratch* texture in the surface's format, rebuilt on
  resize, with its own slot like the atlas.
- Pipelines from the one shader: `direct` (today's, straight alpha
  over), `union` (`fs_premul`, all channels `max`), `wipe` (`fs_premul`,
  no blend — writes transparent black), `composite` (`fs_main`,
  premultiplied over).
- `render(background, frame)` runs `scene::passes` and executes them:
  a `Direct` pass draws the composite box first, then its runs; an
  `Offscreen` pass targets the scratch, wipes the bounds, then draws its
  runs with `union`. The first pass clears; every other loads.

### `select.rs` — the pointer respects layers

`element_at` walks `painted()` from the top; `elements_in` filters
through it too. A hidden layer is neither hit nor marqueed.

### `editor.rs` — the brush, the tip, the active layer

- `Tool::Brush`, hotkey `b`, between Pencil and Zoom in the dock.
- The stroke in progress remembers its `Tip`, taken at the press from
  the pencil's constant or the brush passed in: `press(button, view,
  screen, doc, brush)`. Release writes the tip into the path.
- The active layer is an id, `None` meaning the topmost. New ink and
  pastes land on it; picking an element with the select tool makes its
  layer active (Photoshop's auto-select).
- Layer operations wrap the document's and keep the session honest:
  `add_layer` activates the new one; `remove_layer` drops its elements
  from the selection and activates the neighbour; `toggle_layer`
  deselects what it hides; `move_layer`; `select_layer`.

### `layers.rs` — new, pure

The dock's chrome in a column on the right: `MARGIN` below the strip
and from the edge, `WIDTH` 200 logical px, a header row and one row per
layer, top layer first. Rows that do not fit above the bottom margin
are not shown (no scrolling yet).

```rust
pub enum PanelHit { Select(usize), Toggle(usize), Add, Remove, Up, Down, Panel }
pub struct Row { pub index: usize, pub rect: ScreenRect, pub eye: ScreenRect, pub label: String, pub label_x: f32 }
pub struct Panel { pub rect: ScreenRect, pub rows: Vec<Row>, /* buttons */ }

impl Panel {
    pub fn layout(viewport, scale, top, atlas, layers: &[Layer]) -> Panel;
    pub fn hit(&self, x, y) -> Option<PanelHit>;
    pub fn prims(&self, layers, active, atlas, slot, theme) -> Vec<Prim>;
}
```

Header: the `Layers` label on the left; up, down, add and remove buttons
on the right, as polyline icons. Row: an eye (open, or slashed and
dimmed when hidden), the name truncated to the room left, the active
row on `active_bg`. `index` is the layer's index in `Document.layers`.

### `dock.rs`

A brush icon on the 24-grid: a tapered handle and a bristle head.

### `app.rs`

- `brush: Brush` and `layers_shown: bool` on the window.
- Keys, without Ctrl/Alt/Super: `Shift+L` toggles the panel; with the
  brush tool selected, `[` `]` size, `{` `}` hardness, digits opacity.
- Hit order: strip, panel (when shown), dock, canvas. Panel hits become
  editor calls and go through `apply`.
- The frame is a `scene::Frame`: grid, document, the stroke in progress
  through `Frame::stroke`, selection, marquee, the brush ring when the
  pointer tool is Brush over the canvas, dock, panel, strip.
- Cursor: crosshair under the brush; arrow over the panel.

## Keys

| Key | Effect |
|---|---|
| `B` | Brush tool |
| `[` / `]` | Brush smaller / larger (brush selected) |
| `{` / `}` | Brush softer / harder (brush selected) |
| `1`–`9`, `0` | Brush opacity 10%–90%, 100% (brush selected) |
| `Shift` + `L` | Show / hide the layers panel |

## Testing

Pure modules carry it: `doc` (defaults, validation, `painted`, layer
ops and naming), `brush` (steps and bounds, tip mapping, ring), `scene`
(hardness → radius and feather, grouping by tip, group bounds,
`passes` layout and culling, composite prim), `select` (top layer wins,
hidden layers ignored), `editor` (brush stroke carries its tip, ink
lands on the active layer, auto-select, every layer operation and what
it does to the selection), `layers` (layout, hit, prims, rows that do
not fit), `dock` (five tools, an icon each). `gfx` and `app` stay the
untested shell, checked by the smoke run and a screenshot.

## Slices

Each one commit, tree building and green.

1. `doc` — layers, the element's layer, opacity and hardness, `painted`,
   layer operations.
2. `brush` — settings, tip, ring.
3. `scene` — soft strokes, `Frame`, groups, `passes`.
4. `gfx` — scratch, pipelines, executing passes; `app` renders a frame.
5. `select`, `editor`, `dock` — the pointer respects layers; the brush
   tool; the active layer and its operations.
6. `layers` — the panel.
7. `app` — keys, routing, the panel and the ring in the frame.
8. Docs.

## Out of scope

- Renaming layers, and dragging them to reorder.
- Layer opacity and blend modes: the nested offscreen pass.
- Pressure: winit has no tablet events on Wayland; `zwp_tablet_v2` would
  be a bridge like `gestures`.
- Brush colour other than the theme's ink; a colour picker is its own
  panel.
- Caching the composited document between frames.
- Scrolling the panel when the layers outnumber the rows.
