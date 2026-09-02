# Omawhite

Local-first whiteboard for Omarchy with export for the agent. Native Rust
engine (winit + wgpu). See [ARCHITECTURE.md](ARCHITECTURE.md) — a **draft**:
the real architecture emerges from development.

## Status

Scaffold (§15 items 1–2), the pencil (item 4), the brush and layers,
selection, navigation, pasted images, and projects in tabs:

- `cargo build` clean, `cargo test` with 339 tests.
- Wayland window + wgpu, one instanced pipeline of SDF primitives (rounded
  boxes and round-capped segments, analytic antialiasing) for everything
  on screen.
- Dotted background fixed in world space; light theme matching the
  reference look, re-derived from `op: theme`.
- Images: `Ctrl+V` pastes what the clipboard holds (PNG, JPEG, WebP) as an
  `image` element — centered on the pointer, one world unit per pixel,
  shrunk to 80% of the visible world if it would not fit, and selected.
  The original bytes are kept in `blobs/<sha256>`; the element only names
  the hash, so a board says nothing about the machine that wrote it.
  Images move, resize and turn like any other element, and paint in
  document order: one instanced draw per texture over the same buffer.
- Pencil: on release the stroke is simplified (Ramer–Douglas–Peucker) and
  fitted with cubic Béziers (Schneider), then saved as a `path` of
  self-contained `[a, c1, c2, b]` curves; rendering flattens them per
  frame at the current zoom.
- Brush (`B`): the same stroke with a body — a size, an opacity and a
  hardness, adjusted from the keyboard while the brush is selected:
  `[` `]` step the size (Photoshop's steps, 1–500 world units), `{` `}`
  the hardness by a quarter, `1`–`9` and `0` set the opacity to
  10%–90% and 100%. A ring the size of the brush follows the pointer.
  The stroke is saved as the same `path`, with `opacity` and `hardness`
  beside `width` (absent when 1), so it selects, moves and turns like a
  pencil stroke. A soft or translucent stroke is composited as one
  shape: its segments are drawn into an offscreen texture as the union
  of their coverage, then laid on the frame once at the stroke's
  opacity — so a stroke crossing itself does not darken, and a soft
  edge has no beads at the joints. Hardness spends `1 − hardness` of
  the radius on the edge ramp, inside the nominal width.
- Layers: every element is on one; the document lists them bottom to
  top, and paint order is the layers' order, then document order within
  a layer. `Shift+L` shows and hides the panel on the right: one row
  per layer, top first, with an eye to show or hide it and the active
  layer highlighted; the header has up, down, add and remove. New ink
  and pastes land on the active layer; picking an element with the
  select tool makes its layer active. A hidden layer paints nothing
  and cannot be hit or marqueed; hiding one deselects what was on it,
  removing one takes its elements along, and the last layer stays.
  Boards from before have no `layers`: they get `Layer 1` on load, and
  their elements join it.
- Select: click picks the topmost element, `Shift`+click toggles one in
  or out, dragging on empty canvas draws a marquee that selects whatever
  it overlaps (`Shift` adds to the selection). The selection shows its
  frame — a lone element's own box, turned with it; several elements get
  the axis-aligned box around every corner, which turns with them while a
  rotation lasts — with square handles on the corners and rings just past
  them. Dragging the selection moves it; a corner handle resizes with the
  opposite corner pinned — `Shift` hands both axes the wider factor, so
  the proportions hold, `Ctrl` pins the center instead of the corner, and
  held together they do both; a ring rotates about the frame center, in
  15° steps from the creation state while `Shift` is held. `Delete`/`Backspace`
  removes the selection; `Esc` cancels the drag in progress (putting things
  back), then clears the selection. Every element carries a `rotation` in
  degrees since it was created; paths still bake transforms into their
  curves, the field only turns their box and anchors the snap.
- Tool dock centered at the bottom — Select `V`, Hand `H`, Pencil `P`,
  Brush `B`, Zoom `Z`; `Esc` cancels the stroke, gesture or drag in
  progress.
- Pan: Hand tool, Space held or the middle button drag the canvas; the
  wheel and two-finger scroll pan (Shift: horizontally); a three-finger
  swipe pans on the trackpad.
- Zoom: Zoom tool or Ctrl held — everywhere but on a resize handle of
  the selection, which keeps the press and reads Ctrl as "from the
  center". Drag right/left to zoom in/out around the press point, click
  for one unit (25%) in, right-click for one out;
  the wheel zooms at the cursor while Zoom is active; pinch on the
  trackpad. Range 10%–1000%. The camera is saved with the board.
- Projects and tabs: several boards open at once, one tab each, with
  the board's name, a dot while it has unsaved changes, a close cross
  and a `+` for a new one. Every tab keeps its own tool, selection and
  camera. `Ctrl+S` saves — asking for a name the first time, unless the
  board came from the store, where it already has one; `Ctrl+Shift+S`
  always asks; `Ctrl+O` opens one or more `.omawhite` files, each in its
  own tab; `Ctrl+W` closes the tab, asking first if work would be lost.
  Nothing autosaves any more: a drawing change dirties the tab, a pan or
  a zoom does not. A project file holds exactly the document JSON;
  images stay in the store's `blobs/`, so a file carried to another
  machine shows placeholders.
- Text: a glyph atlas from a font shipped inside the binary, drawn by
  the same pipeline as the images — one white sheet, alpha for coverage,
  a UV cell per glyph. It dresses the tabs and the layers panel; the
  text *tool* is still ahead.
- Versioned JSON document (schema 1) + XDG persistence (0700/0600, atomic
  save). Project files chosen through the portal keep the umask instead.
- IPC protocol §5 (closed schema) + single instance via socket.
- CLI: `--new`, `--open <id>`, `--export <dir>`, `--shutdown`,
  `--socket <path>`.

Not yet: eraser, the text tool, shapes, undo, export, Omarchy plugin,
thumbnails, layer opacity and renaming, brush colour and pressure.

## Controls

| Input | Effect |
|---|---|
| `V` / `H` / `P` / `B` / `Z` | Select / Hand / Pencil / Brush / Zoom tool (also clickable in the dock) |
| `Esc` | Cancel the stroke, gesture or drag in progress; then clear the selection |
| Left drag (Brush) | Paint with the brush; the stroke is fitted to Béziers on release |
| `[` / `]` (Brush selected) | Brush smaller / larger |
| `{` / `}` (Brush selected) | Brush softer / harder |
| `1`–`9`, `0` (Brush selected) | Brush opacity 10%–90%, 100% |
| `Shift` + `L` | Show / hide the layers panel |
| Click a layer row / its eye | Make it the active layer / show or hide it |
| Panel `▲` `▼` `+` `🗑` | Move the active layer up / down, add a layer above it, remove it |
| Click (Select) | Select the topmost element under the pointer; empty canvas clears |
| `Shift` + click (Select) | Add the element to the selection, or remove it |
| Left drag on empty canvas (Select) | Marquee: selects what it overlaps (`Shift` adds) |
| Left drag on the selection | Move |
| Drag a corner handle | Resize, opposite corner pinned |
| `Shift` + drag a corner handle | Resize keeping the proportions |
| `Ctrl` + drag a corner handle | Resize about the center (`Shift` too: both) |
| Drag a ring past a corner | Rotate about the selection's center (`Shift`: 15° steps from the creation state) |
| `Ctrl` + `V` | Paste the clipboard image onto the board |
| `Ctrl` + `S` | Save the tab; asks for a name the first time |
| `Ctrl` + `Shift` + `S` | Save as — always asks |
| `Ctrl` + `O` | Open boards, one tab each |
| `Ctrl` + `W` | Close the tab; asks if there is unsaved work |
| Click a tab / its `✕` / the `+` | Switch / close / new board |
| `Delete` / `Backspace` | Delete the selection |
| Left drag (Pencil) | Draw; the stroke is fitted to Béziers on release |
| Left drag (Hand), `Space` + drag, middle drag | Pan |
| Wheel, two-finger scroll | Pan (`Shift`: horizontally) |
| Three-finger swipe | Pan (trackpad) |
| Left drag (Zoom) or `Ctrl` + drag | Zoom in/out around the press point (100 px per 25% step) |
| Click / right-click (Zoom or `Ctrl`) | One step (25%) in / out |
| Wheel (Zoom active), pinch | Zoom at the cursor / on the trackpad |

Zoom range is 10%–1000%. The camera is saved with the board.

## Run

```sh
cargo run                      # most recent board (or a new one)
cargo run -- --new             # new board
cargo test                     # full suite
```

Smoke test (opens the window, renders 3 frames, exits):

```sh
XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- \
  --socket /tmp/omawhite-smoke.sock --smoke-frames 3
```

## Layout

```
src/main.rs      CLI dispatch → forward to the live instance, or become it
src/cli.rs       flags (clap), mutually exclusive actions
src/doc.rs       document §6.1 (pure data, serde): layers, rect, path, image
src/store.rs     ~/.local/share/omawhite: boards/, blobs/, index.json, perms §9.3
src/ipc/         §5: proto (strict parser), client (forward), server (socket 0600)
src/bitmap.rs    decode PNG/JPEG/WebP to RGBA8, paste size (pure, tested)
src/curve.rs     simplify, cubic Bézier fit and flatten (pure, tested)
src/brush.rs     brush settings, the tip a stroke carries, the pointer's ring (pure, tested)
src/scene.rs     View (camera + viewport + scale), document → SDF prims, frames, groups and passes (pure, tested)
src/geom.rs      affine maps, corners and oriented frames (pure, tested)
src/select.rs    selection: element frames, hit-testing, handles, transforms, overlay prims (pure, tested)
src/grid.rs      dotted background (pure, tested)
src/theme.rs     palette: light default, derived from op: theme (pure, tested)
src/editor.rs    active tool, held keys, stroke and its tip, pan/zoom gesture, selection and its drag, the active layer (pure, tested)
src/dock.rs      bottom tool dock: layout, hit-test, icons (pure, tested)
src/layers.rs    layers panel on the right: layout, hit-test, rows, eyes and buttons (pure, tested)
src/tabs.rs      top tab strip: layout, hit-test, what a narrow tab drops (pure, tested)
src/text.rs      glyph atlas, measure, layout, ellipsis truncation (pure, tested)
src/project.rs   a document's origin (file, board, untitled) and dirty flag (pure, tested)
src/gfx.rs       wgpu 30: the instanced SDF pipelines, image textures, the scratch a group is composited in
src/app.rs       winit: window, input routing, socket → event loop bridge
src/gestures.rs  trackpad pinch/swipe (zwp_pointer_gestures_v1) → event loop bridge
src/clipboard.rs selection reads (wl_data_device) → event loop bridge
src/dialogs.rs   open/save-as/confirm over xdg-desktop-portal → event loop bridge
assets/fonts/    Liberation Sans (SIL OFL 1.1), compiled into the binary
```

Frame data flow: grid + document + live stroke + selection overlay +
brush ring + dock + layers panel + tab strip → `scene`/`select`/`brush`/
`layers`/`tabs` prims, gathered in a `scene::Frame` whose groups mark
the strokes composited as one shape → `scene::passes` plans the render
passes → `gfx` executes them.

User data: `~/.local/share/omawhite/`. Socket:
`$XDG_RUNTIME_DIR/omawhite.sock`. Project files go wherever the user
puts them.

Dialogs come from `xdg-desktop-portal` (any backend with a file
chooser). The unsaved-work question has no portal of its own and uses
`zenity`; without it the answer reads as a cancel, so a tab is never
closed by its absence.
