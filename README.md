# Omawhite

Local-first whiteboard for Omarchy with export for the agent. Native Rust
engine (winit + wgpu). See [ARCHITECTURE.md](ARCHITECTURE.md) — a **draft**:
the real architecture emerges from development.

## Status

Scaffold (§15 items 1–2), the pencil (item 4), the brush and layers,
selection, navigation, pasted images, and projects in tabs:

- `cargo build` clean, `cargo test` with 431 tests.
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
  hardness — and, as in Sketchbook, a name and a shelf to live on. The
  binary ships **Sketchbook's own seventeen sets, 211 brushes**: Basic,
  Legacy, Markers, Fine Art, Traditional, Designer, Artist, Pastel,
  Half Tone, Texture Essentials, Texture, Shape, Synthetic Paint,
  Splatter, Glow, Smudge and Colorless, converted from the
  `.skbrushes` files by `tools/import-skbrushes.py`. One brush is in
  the hand at a time and an edit belongs to it: `[` `]` step the size (Photoshop's steps, 1–500 world
  units), `{` `}` the hardness by a quarter, `1`–`9` and `0` set the
  opacity to 10%–90% and 100% — all of them writing into the brush that
  is painting, which keeps the change when another is picked up and
  put down again. A brush is a **nib stamped along the stroke**, never
  a swept line: its `spacing`, `roundness` and `rotation` are what one
  dab is and how far apart they sit, and the canvas paints with all
  three, and `flow` with them — what one dab lays. **103 of the 211
  stamp a nib of their own**: Sketchbook's own shape images, converted
  into one sheet of 90 that the binary carries, and a stroke names the
  nib it was laid with. A nib is also thrown off true dab by dab: its
  radius, its angle and the gap before it, each by an amount in its own
  unit, which is how Sketchbook states randomness. The other two
  amounts it states — on opacity and flow — are not in those
  properties' own unit (a fraction thrown by five, or by twenty), so
  the canvas does not guess at them. The rest of the body is read off
  the real sets and described truthfully while the engine grows into it
  — dynamics, the tip's profile, texture depth, and what the pen's
  pressure drives. What is left promising a mark the ink cannot make is
  the brush told apart by a *grain* — a texture nib, or the canvas's
  own paper — and the library says so per brush. A ring the size of the
  brush follows the pointer.
  The stroke joins the `paint` on the layer it lands on — one object
  per raster layer, however many strokes went into it — each stroke
  keeping the ink it was laid with: `stroke`, `width`, and `opacity`
  and `hardness` when they are not 1, and the `stamp` — the nib — when
  it was stamped rather than swept. The paint selects, moves and turns
  as one. A stroke that does not cover with one dab is composited as
  one shape, into an offscreen texture and then onto the frame once at
  the stroke's opacity. How its pieces meet there is the difference
  between the two kinds of stroke: a **swept** one unions — every
  channel a max, so a soft edge has no beads at the joints and a
  stroke crossing itself does not darken — while a **stamped** one
  **builds**, one dab over the next. Flow is what a single dab lays and
  opacity the ceiling the pile reaches, so a stroke crossing itself is
  darker for it, as paint is. Hardness spends `1 − hardness` of the
  radius on the edge ramp, inside the nominal width.
- Brush palette: picking the brush brings up Sketchbook's Brush Library
  on the left. Every set stands one under the next in a single scroll —
  its name, then its brushes as a grid of icons six across, drawn with
  Sketchbook's own art — and the brush in the hand wears a ring. Above
  them a preview says which brush it is: its icon, its name, the set it
  came off, and a dab of what it actually lays. The icon is the art and
  may promise a mark the canvas cannot stamp yet; the dab is the part
  that cannot. Two buttons there open Brush Properties and put the
  brush back the way it shipped. 211 brushes fit no window, so the
  panel is a scroll area with a thumb: the wheel over it walks the
  list, and taking up a brush brings its cell into sight. It is the
  tool's own chrome: it comes and goes with the brush, and `Shift+B`
  shuts it without putting the brush down.
- Brush properties: a bar floats under the tab strip while the brush is
  in hand — the brush's name, a dot after it while it is off the
  settings it shipped with, and the pair a brush is judged by, Size and
  Opacity, each a slider lying flat with its number beside it. The
  chevron at its end drops Sketchbook's Advanced layout underneath, in
  two columns: Pressure (Size, Opacity, Flow), Stamp (Spacing,
  Roundness, Rotation), Nib (Edge, Depth) and Randomness (Size,
  Opacity, Flow, Rotation, Spacing), each section under its own
  heading. Closed and open never show the same slider twice, and the
  line above does not move or change width when the panel drops. The
  ones the canvas actually paints with — Size, Opacity, Flow, Edge, the
  whole Stamp section and three of the five under Randomness — are
  drawn in ink; every other slider is muted.
  It still moves, and it still writes the brush's own value, but the
  muting is the bar saying it does not promise paint yet. The
  palette's `≡` opens the same panel.
- Layers: every element is on one; the document lists them bottom to
  top, and paint order is the layers' order, then document order within
  a layer. A handle on the header's line pulls the panel out and puts it
  back — closed it stands on end off the window's right edge: a chevron
  pointing the way the panel comes, the word `Layers` turned a quarter
  turn counter-clockwise so it reads up the tab with its letters facing
  the canvas, and under it `Shift+L` on a key of its own, so the
  shortcut is taught by the thing it makes unnecessary. Open it steps
  aside to the panel's left, a chevron alone — the header behind it
  already says the word, and a shut door is the only one worth a
  shortcut. A window too short to letter the tab gives up the key first,
  then the word. The panel is one card per layer, top first — a hairline
  border and a light shadow each, the active one filled — with an eye to
  show or hide it; the header has up, down, add and remove. A card
  dragged by its name leaves the stack and follows the pointer instead
  of stepping from row to row: it grows a little, turns a couple of
  degrees clockwise, leans toward the canvas, takes a blue outline and a
  shadow with further to fall, and draws over the cards it passes. The
  lift eases in over about a seventh of a second and runs backwards when
  the card is let go, so nothing snaps. The stack reorders live under
  the pointer — the cards it passes slide out of its way rather than
  jumping — and the layer is left where the button comes up. The panel
  is a scroll area: it grows to the room the window has, cuts the card
  at its edge, and shows a thumb for how much of the stack is in view;
  the wheel over it walks the list. Picking an element on the canvas
  makes its layer active, and the panel glides to bring that card into
  sight. A layer holds one of two things, and its card says which — a
  grid of pixels or a curve, at the end opposite the eye. A raster
  layer accumulates: every brush stroke joins the `paint` already on
  the active one instead of becoming an element of its own, so the
  layer holds one painting however many strokes went into it, and a
  pasted image opens a layer of its own so the next stroke paints over
  the picture instead of beside it. A vector layer holds the one object
  it was made for: every pencil stroke opens its own, above the active
  layer, and leaves it active. Painting on a vector layer is not
  possible, so a brush stroke over one opens a raster layer above it —
  Photoshop's answer to the same question. The panel's `+` makes a
  raster layer: a blank sheet to paint on. A layer is the object it
  holds, and the two go together: deleting the object takes the layer
  with it, and the trash takes the object — on the last layer, which
  always stays, it empties it instead. A hidden layer paints
  nothing and cannot be hit or marqueed; hiding one deselects what was
  on it, removing one takes its elements along, and the last layer
  stays. Boards from before have no `layers`: they get `Layer 1` on
  load, and their elements join it. A layer written before kinds
  existed is a stack that accumulates, so it reads back as raster —
  the kind is on disk only when it is `vector`.
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
- Tablet: the pen draws. winit has no tablet events on Wayland, so a
  bridge of its own binds `zwp_tablet_v2` on the window's connection —
  the shape `gestures` uses — and hands the loop the tool's movement and
  the touch of its tip. They go down the same funnel as the mouse, so the
  pen picks a tool in the dock and drags a layer card as well as it
  draws. The protocol sends the axes and the tip of one hardware event
  one at a time, closed by a `frame`; the bridge holds them and sends the
  movement before the touch, because a press has to land where its own
  frame put the tool. Pressure arrives and is dropped: a stroke carries
  one width, and nowhere yet to keep more.
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
thumbnails, layer opacity and renaming, brush colour and pressure, a
library that outlives the process, and the grain the muted sliders that
are left still wait on — a texture nib, the canvas's own paper, and the
tip profile that shapes a round nib's falloff.

## Controls

| Input | Effect |
|---|---|
| `V` / `H` / `P` / `B` / `Z` | Select / Hand / Pencil / Brush / Zoom tool (also clickable in the dock) |
| `Esc` | Cancel the stroke, gesture or drag in progress; then clear the selection |
| Left drag (Brush) | Paint with the brush; the stroke is fitted to Béziers on release |
| `Shift` + `B` | Show / hide the brush palette |
| Palette `≡` / bar chevron | Open / fold Brush Properties |
| Drag a slider in the bar | That property of the brush in hand |
| Click an icon in the palette | Take up that brush |
| Wheel over the palette | Walk the library |
| Palette `↺` | Put the brush back the way it shipped |
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
src/brush.rs     the brush library: sets, presets, a brush's body, its properties, the tip a stroke carries, the pointer's ring (pure, tested)
tools/import-skbrushes.py  Sketchbook `.skbrushes` -> assets/brushes/ (parameters + icon sheet)
assets/brushes/  library.json (17 sets, 211 brushes) and icons.png (211 cells), built into the binary
src/scene.rs     View (camera + viewport + scale), document → SDF prims, frames, groups and passes (pure, tested)
src/geom.rs      affine maps, corners and oriented frames (pure, tested)
src/select.rs    selection: element frames, hit-testing, handles, transforms, overlay prims (pure, tested)
src/grid.rs      dotted background (pure, tested)
src/theme.rs     palette: light default, derived from op: theme (pure, tested)
src/editor.rs    active tool, held keys, stroke and its tip, pan/zoom gesture, selection and its drag, the active layer (pure, tested)
src/dock.rs      bottom tool dock: layout, hit-test, icons (pure, tested)
src/layers.rs    layers panel on the right: layout, hit-test, rows, eyes and buttons (pure, tested)
src/palette.rs   brush library on the left: the shelves, the grid of icons, the preview (pure, tested)
src/props.rs     brush properties bar under the strip: the basic pair, and the Advanced layout it drops (pure, tested)
src/tabs.rs      top tab strip: layout, hit-test, what a narrow tab drops (pure, tested)
src/text.rs      glyph atlas, measure, layout, ellipsis truncation (pure, tested)
src/project.rs   a document's origin (file, board, untitled) and dirty flag (pure, tested)
src/gfx.rs       wgpu 30: the instanced SDF pipelines, image textures, the scratch a group is composited in
src/app.rs       winit: window, input routing, socket → event loop bridge
src/gestures.rs  trackpad pinch/swipe (zwp_pointer_gestures_v1) → event loop bridge
src/tablet.rs    the tablet's pen (zwp_tablet_v2) → event loop bridge; its frame is tested
src/clipboard.rs selection reads (wl_data_device) → event loop bridge
src/dialogs.rs   open/save-as/confirm over xdg-desktop-portal → event loop bridge
assets/fonts/    Liberation Sans (SIL OFL 1.1), compiled into the binary
```

Frame data flow: grid + document + live stroke + selection overlay +
brush ring + dock + brush palette + properties bar + layers panel + tab
strip → `scene`/`select`/`brush`/`palette`/`props`/`layers`/`tabs`
prims, gathered in a `scene::Frame` whose groups mark
the strokes composited as one shape → `scene::passes` plans the render
passes → `gfx` executes them.

User data: `~/.local/share/omawhite/`. Socket:
`$XDG_RUNTIME_DIR/omawhite.sock`. Project files go wherever the user
puts them.

Dialogs come from `xdg-desktop-portal` (any backend with a file
chooser). The unsaved-work question has no portal of its own and uses
`zenity`; without it the answer reads as a cancel, so a tab is never
closed by its absence.
