# Omawhite

Local-first whiteboard for Omarchy with export for the agent. Native Rust
engine (winit + wgpu). See [ARCHITECTURE.md](ARCHITECTURE.md) — a **draft**:
the real architecture emerges from development.

## Status

Scaffold (§15 items 1–2), the pencil (item 4), the brush and layers,
selection, navigation, pasted images, projects in tabs, frames,
export to the agent and the CLI an agent asks the board through:

- `cargo build` clean, `cargo test` with 781 tests.
- Wayland window + wgpu, one instanced pipeline of SDF primitives (rounded
  boxes and round-capped segments, analytic antialiasing) for everything
  on screen.
- Dotted background fixed in world space; light theme matching the
  reference look, re-derived from `op: theme` — or from the theme the
  desktop is actually wearing, which is the whole of what Omarchy
  changes when a theme is set (see **The Omarchy theme** below).
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
  put down again — and which outlives the window: what was changed is
  kept in `brushes.json` beside the boards, as a list of exceptions, so
  a brush nobody touched is still whatever the shipped sets say. A brush is a **nib stamped along the stroke**, never
  a swept line: its `spacing`, `roundness` and `rotation` are what one
  dab is and how far apart they sit — the gap in Sketchbook's own
  spacing units, each a quarter of the nib's width, which is what makes
  its documented default Pencil draw solid — and the canvas paints with
  all
  three, and `flow` with them — what one dab lays. **103 of the 211
  stamp a nib of their own**: Sketchbook's own shape images, converted
  into one sheet that the binary carries, and a stroke names the
  nib it was laid with, and **turns with the stroke** when the brush
  says it should — 67 of them do, and 59 of those stamp a shape, so a
  bristle nib runs along the curve instead of pointing one way through
  it. Two more of them are the stylus's own — 101 brushes turn the nib
  by the way the pen is held and 34 of those by its roll on top of the
  lean. **42 more wear a grain** rather than stamping a shape:
  Sketchbook's other kind of nib image, and the opposite thing — the
  dab stays round and keeps its own edge, and the grain eats into what
  that edge covers, which is what breaks up a hard watercolour edge.
  Both kinds ride on the one sheet the binary carries, now 114 nibs —
  and reading it got a correction: an image says its coverage in its
  gray or in its alpha depending on how it was drawn, and thirteen
  nibs drawn in black on transparency had been read for empty, so the
  brushes naming them stamped nothing at all.
  A nib is also thrown off true dab by dab:
  its radius and its angle, each by an amount in its own unit, which is
  how Sketchbook states randomness. Three of its five amounts stay out,
  because its own sets contradict their scale: opacity and flow are
  thrown by five and by twenty on a property that is a fraction, and
  seventeen of the thirty brushes that throw the *gap* name an amount
  larger than the gap itself — a throw either side of it would land
  negative more often than not. The canvas does not guess at any of the
  three. The **tip's profile** paints too: Sketchbook picks one of four
  falloffs per brush — 61 airbrush, 40 sharp, 26 hard solid, the rest
  the plain ramp — and it is what the edge does over the width the
  Edge slider gives it, so an airbrush fades over its whole band
  instead of ramping straight across. The rest of the body is read off
  the real sets and described truthfully while the engine grows into
  it. **Forty-nine brushes are dragged over a paper**, and that is
  Sketchbook's third kind of art — the one that is not the nib's. A
  grain turns with the dab because it *is* the dab's; a paper belongs
  to the board, so the nib is dragged over it: it stands still while
  the nib turns, and two strokes crossing one place meet the same
  fibres. Thirty of the forty-nine wear a nib as well, and one dab
  wears both, so the papers ride on the same sheet in a band under the
  nibs. How wide one tile of a paper is is the brush's own — a
  pencil's spans two hundred world units, a halftone's fifty — which
  is what turns the Half Tone shelf into the dots and grids its icons
  promise. **Depth** paints with it: how deep the paper bites, and at
  the bottom of its track the paper is not there at all. What the
  canvas will not read is Sketchbook's brightness and contrast on a
  paper — nothing says what those numbers mean, and the one band they
  could plainly be turns eight of the 49 papers solid black, one of
  them under a brush named Textured Pencil. A ring follows the
  pointer, and it is the outline of the nib the next press would lay:
  a circle when the nib is round, the same flattened capsule when it
  is squished, leaning the way the ink will.
  **The pencil thins with the hand too.** A swept stroke used to have
  one width end to end whatever the pen said; it is now laid as a run
  of capsules, cut fine enough that the outline is as true to the hand
  as the flattened line is to the curve. How much a lighter touch takes
  away is the build's, since the pencil has no sliders to record — half
  the width, which is the middle of what Sketchbook's own Fine Art
  pencils ask for, none of its 211 brushes driving size fully.
  **The eight erasers erase.** Sketchbook gives every brush a stamp
  blend style and 91 of the 211 name something other than plain ink;
  the erasers used to paint. A raster layer is now built on a sheet of
  its own whenever one of its strokes rubs the others out, and the
  eraser's dabs are taken back out of that sheet — so it clears the
  layer it is on and leaves the board, the grid and every other layer
  alone. It rubs while it is still being drawn, because the stroke in
  progress is painted where it is going to land rather than over
  everything. The other six styles read the paint underneath, which is
  a different engine: those brushes lay plain ink, and the library says
  which they are rather than promising a mixture.
  **The pen's pressure drives the ink.** 126 of the brushes narrow
  with a lighter touch, 143 lay less, 33 fade — Sketchbook names the
  two ends of each and the gap between them is how much the hand is
  worth, which is what the importer reads. A dab is that much narrower
  and that much thinner where the press was lighter, and the gap after
  it closes with the nib, since the spacing is a share of its width —
  so a stroke tapers instead of ending flat, and stays as solid as it
  was. What the hand did is kept with the stroke: readings from end to
  end, evenly spaced along its own length, so a saved board reopens as
  the line that was drawn and not as a line of one width. A brush no
  pressure drives, and a mouse — which presses all the way — leave the
  board saying nothing about a pen.
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
- Brush strip: picking the brush stands a narrow strip up on the left.
  Its head is the brush in the hand — Sketchbook's own icon for it, its
  name, the shelf it came off, and a dab of what it actually lays. The
  icon is the art and may promise a mark the canvas cannot stamp yet;
  the dab is the part that cannot. Under it two buttons: the sliders
  open Brush Properties, the chevron opens the library beside the strip.
  Then the seats — nine numbered `1`–`9`, and slot `0` at the foot.
  `Shift` and a digit takes the brush in that seat, and a brush is put
  in one by dragging it out of the library. Nine ship filled with the
  first of Basic — pencil, marker, airbrush, pen, ink, watercolour,
  inker, blur and eraser — and slot `0` nobody fills: it follows the
  hand, holding whatever was last reached for that none of the nine
  already keeps, so a brush taken off a far shelf stays one key away for
  as long as it is wanted. What is moved is kept in `brushes.json`
  beside the boards; seats nobody rearranged are not written down. It is
  the tool's own chrome: it comes and goes with the brush, and
  `Shift+B` shuts it without putting the brush down.
- Brush library: the chevron opens Sketchbook's Brush Library as a panel
  to the right of the strip, with canvas between them — both on show at
  once, because that is what a brush is dragged across. Every set stands
  one under the next in a single scroll — its name, then its brushes as
  a grid of icons six across, drawn with Sketchbook's own art — and the
  brush in the hand wears a ring. 211 brushes fit no window, so the
  panel is a scroll area with a thumb: the wheel over it walks the list,
  and taking up a brush brings its cell into sight. It is opened from
  inside the strip, so hiding the strip hides it too.
- Brush properties: a bar floats under the tab strip while the brush is
  in hand — the brush's name, a dot after it while it is off the
  settings it shipped with, and the pair a brush is judged by, Size and
  Opacity, each a slider lying flat with its number beside it. The
  chevron at its end drops Sketchbook's Advanced layout underneath, in
  two columns: Pressure (Size, Opacity, Flow), Stamp (Spacing,
  Roundness, Rotation), Nib (Edge, Depth), Randomness (Size,
  Opacity, Flow, Rotation, Spacing) and Paint (Strength, Blending,
  Dilution), each section under its own heading. Closed and open never show the same slider twice, and the
  line above does not move or change width when the panel drops. The
  ones the canvas actually paints with — Size, Opacity, Flow, Edge, the
  whole Stamp section and two of the five under Randomness — are
  drawn in ink; every other slider is muted. The whole Paint section is
  muted, and for a reason worth naming: Strength, Blending and Dilution
  say what a dab does with the paint *under* it, and every stroke here
  is redrawn from its curves each frame, so there is nothing to read
  back. The Smudge and Colorless shelves are made of them. Smearing
  would mean keeping a raster layer as pixels — on an infinite board
  that pans, that means tiles in world space and an invalidation story:
  a different engine rather than a missing line, and the bar says so by
  not promising.
  It still moves, and it still writes the brush's own value, but the
  muting is the bar saying it does not promise paint yet. The strip's
  `≡` opens the same panel. Beside the dot that says the brush was
  moved off its factory settings stands the arrow that puts it back —
  both in a run reserved after the name, so neither travels with the
  length of a brush's name.
- Ink: the dock carries the colours a stroke is laid in, after the
  tools and a divider — the theme's own near-black first, then red,
  amber, green, blue and violet. The chosen one wears a ring, as the
  brush in the hand does in the palette. It is the window's, like the
  tool: the pencil and every brush lay it, and the brush's ring shows
  it before the press. Every stroke has always named its colour on
  disk; what was missing was somewhere to pick one.
- Undo: `Ctrl+Z` puts the board back the way it was before the last
  change and the hand back where it was standing then; `Ctrl+Shift+Z`
  puts it forward again. A step is a change that came to
  **rest** — a whole stroke, a whole drag, a delete, a paste, a layer
  carried across the stack — never a sample of one, which is why a
  stroke undoes as a stroke and a card dragged over two rows comes back
  in a single step. Undoing a delete brings the objects back
  *selected*, because what is kept is the board **and** the spot the
  hand was on: they are kept together, so the ids one names are ids the
  other has. The view is not part of it — panning between two strokes
  is not work, and a step back does not move it. The history is a tab's
  own, capped by a depth and by a memory budget, so a heavy board gets
  fewer steps rather than a quarter of a gigabyte of them.
- Export to the agent: `Ctrl+E` sends what is selected to an AI agent
  running on this machine. A panel lists the agents found — herdr says
  where each one is working, which is focused and what it is doing;
  tmux says the pane and its path; a `/proc` walk finds the ones under
  neither and, having no way to talk to them, writes them muted. The
  page lands in that agent's own working directory as three files under
  `docs/boards/<name>/` — `board.png`, the picture; `board.json`, the
  same objects in the board's own schema; and `board.md`, an inventory
  under a preface saying it is a diagram and not an order. A frame
  brings the name it carries on its card; a loose selection is asked
  for one, prefilled with a name the folder does not already hold, and
  whatever is typed becomes exactly one directory. The instruction is
  the person's own line, typed into the panel and submitted into the
  agent's live session — never the board's text, which is inventory.
  With nothing selected, or no agent running, the key opens nothing and
  says which half is missing. A layer's name is now the person's to
  give: a second press on a card opens it for editing, and a frame is
  born `Frame 1` rather than `Layer 3`.
- The CLI an agent asks the board through: `Ctrl+E` pushes a page at an
  agent; this is the way back, and a **frame** is the whole of what
  travels either way. `omawhite agent frames` lists what the open board
  has — id, name, box and how much is standing in each.
  `omawhite agent read <frame> --to <dir>` writes one of them into that
  directory as the same three files `Ctrl+E` writes, so an agent
  exports a picture to itself; a name is resolved against the listing
  and an ambiguous one is refused naming both ids, because the protocol
  itself speaks only ids. `omawhite agent add <fragment.json>` puts a
  frame **on** the board: the fragment is exactly what `read` writes —
  one frame plus what stands on its own layers — so a page read off the
  board can be handed straight back, and it arrives as a new frame
  rather than over the one it came from, since every id is minted
  again. The board picks where it lands, to the right of everything,
  because an agent cannot see the board it is drawing on; the frame's
  size is the agent's. What it can draw is what the canvas already
  draws — rects, ink paths, and images whose bytes ride in `blobs/`
  beside the json — so a label today is a picture the agent rendered,
  the canvas having no text of its own yet. It lands as one undo step:
  `Ctrl+Z` takes an agent's frame back off. All three need the board to
  be **open** — it is the live document, unsaved work included, and the
  tab in front where several are — and
  their answer is real work rather than an ack, so they go over the
  socket on a path of their own and wait for the loop to do it. The
  skill that teaches an agent all of this is `skills/omawhite/`, one
  markdown file and an installer that puts it where each host on the
  machine looks — `~/.claude/skills/`, `~/.codex/skills/` and
  `~/.config/opencode/skills/` all read a directory holding a
  `SKILL.md`, and the installer says which hosts it found.
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
- Frames: an area that holds objects, drawn with the Frame tool (`F`) by
  dragging it out. What is inside it is cut to its boundary — ink that
  runs past the edge stops there, and stops being clickable there too. It
  has a layer stack of its own, and it is itself a layer on the board:
  its card ends in a chevron that goes in, and inside it the panel's
  header says the frame's name and leads back out. It is born the
  theme's surface, and clicking an ink with a frame selected paints its
  ground. What is inside is decided by geometry rather than by the panel:
  a stroke belongs to the frame the press landed in, whatever it does
  afterwards, and an object let go of inside a frame joins it while one
  dragged out leaves — its layer moves with it, because a layer is the
  object it holds. A frame drawn over things claims what its area
  covers. Moving it carries what it holds; resizing it moves the
  boundary and shows more or less of them. It does not turn — the cut is
  an axis-aligned box in the shader — and it does not nest.
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
  Brush `B`, Frame `F`, Zoom `Z` — with six original RGBA illustrations
  cut from one 80 px-per-cell sheet: matte retro-industrial controls in
  warm ivory, charcoal and restrained orange/green. The former line icons
  remain the load-failure fallback. `Esc` cancels the stroke, gesture or
  drag in progress.
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
  frame put the tool. It reads the pen's pressure, the barrel's tilt and
  the roll of a pen that reports one, and hands them over ahead of the
  movement they came with — the axes stand from frame to frame, since
  the protocol only sends the ones that changed. Every mouse event puts
  them back to a mouse's own, so a pen left on the desk cannot leave the
  mouse painting nothing.
- Projects and tabs: several boards open at once, one tab each, with
  the board's name, a dot while it has unsaved changes, a close cross
  and a `+` for a new one. Every tab keeps its own tool, selection and
  camera. `Ctrl+S` saves — asking for a name the first time, unless the
  board came from the store, where it already has one; `Ctrl+Shift+S`
  always asks; `Ctrl+O` opens one or more `.omawhite` files, each in its
  own tab; `Ctrl+W` closes the tab, asking first if work would be lost.
  A drawing change dirties the tab, a pan or a zoom does not. A **draft**
  — a board with no name of its own — keeps itself: its changes reach the
  store once the hand has been still for a moment, so `Ctrl+S` is what
  gives a project a *name*, not what saves it from being lost, and
  closing a dirty draft no longer asks. A file the person named is the
  opposite: it is written only when asked, because writing into it behind
  their back would empty `Ctrl+S`, the dot and the question of all their
  meaning — so a dirty one still asks on the way out. A new board is
  written nowhere until it is drawn on, which is what keeps every `+`
  from leaving an empty board behind. A project file holds exactly the
  document JSON;
  images stay in the store's `blobs/`, so a file carried to another
  machine shows placeholders.
- Text: a glyph atlas from a font shipped inside the binary, drawn by
  the same pipeline as the images — one white sheet, alpha for coverage,
  a UV cell per glyph. It dresses the tabs and the layers panel; the
  text *tool* is still ahead.
- Versioned JSON document (schema 1) + XDG persistence (0700/0600, atomic
  save). Project files chosen through the portal keep the umask instead.
- IPC protocol §5 (closed schema) + single instance via socket.
- Recent projects: `index.json` lists both kinds, newest first — a draft
  by id, a named file by path — and is the one file the plugin reads. A
  project moving home stops being two entries; one file stays one entry
  whatever wrote it. Reaching for a file that has gone is the only thing
  that drops one, so a project on a drive nobody has mounted keeps its
  place.
- CLI: `--new`, `--open <id>`, `--open-file <path>`, `--export <dir>`,
  `--shutdown`, `--socket <path>`, and the `agent` verb — `agent
  frames`, `agent read <frame> [--to <dir>]`, `agent add <file>` —
  which answers one line of JSON and exits non-zero on a refusal.
- Omarchy plugin (`plugin/`, §10.2): a bar widget whose popout holds the
  two ways in — `n` for a new board, `o` for the recent projects, newest
  first, opened by `Enter` or a click. The list shows twenty and marks
  which kind each is; typing filters the whole index, so the fortieth
  project is a word away rather than a scroll. A file whose path is not
  there right now is dimmed and says so rather than being dropped. It is
  a shell and
  nothing more: it reads `index.json`, which is the one file it is
  allowed to read, drops any entry naming an id the engine would refuse,
  and launches the binary detached, so the board owns its own window and
  killing it cannot take the shell down. The engine is looked for on the
  `PATH`, in `~/.local/bin`, at an `enginePath` set in `shell.json`, and
  finally in `target/` beside a checkout — and when none of them answers
  the popout says so instead of swallowing the click. Enter and Space act
  only once the arrows have raised a cursor that can be seen: the two
  menu rows carry their own letters, and a selection nobody is shown is
  one nobody meant.

Not yet: the text tool, shapes, export,
thumbnails, layer opacity and renaming, frames that nest or turn or come
in more than the one basic kind, and the six sliders that stay
muted — three randomness amounts whose scale the sets contradict, and
the whole of Paint, which asks the canvas to read back the ink it has
already laid.

## The Omarchy theme

The board wears what the desktop wears. `omarchy-theme-set` stages a
theme whole into `~/.local/state/omarchy/current/theme/`, and the board
reads the same files every other app on the desktop reads:

| what | where it comes from |
|---|---|
| the palette | `colors.toml` — `mode`, `accent`, `muted`, the grounds and the foregrounds, by name |
| borders and fills | `shell.toml` `[controls]`, with `~/.config/omarchy/shell.toml` laid over it |
| the type | fontconfig's `monospace` (what `omarchy-font-set` writes) at `[font] base-size` |
| the corner | Hyprland's `decoration:rounding` |

Omarchy paints every surface in `background` and separates it with a
border. That does not close on a board, whose canvas *is* the ground, so
the desktop's own darker ground is the canvas and the panels stand on it
in `background` — which on the `white` theme lands on exactly the
off-white-under-near-white this board's reference look was drawn as. The
rest follows the section each surface belongs to: the strip is `[bar]`,
the dock and the panels are `[menu]`, and what is inside them is
`[controls]`, down to the alpha a selected control is filled at.

Corners scale by `rounding / 8`, the 8 the chrome was drawn to: at
Omarchy's own 0 the board goes square like every window around it —
the layers handle with the rest, since a tab standing beside a squared
panel is a surface and not a knob. A capsule is not a corner, though:
a slider's track, a scrollbar's thumb and an ink dot keep their own
shape.

The **dock's inks do not move**. Black, white and five colours, a
constant no part of a theme reaches: they are what a board is marked up
in, not what the interface is painted with, and ink picked today has to
be the same colour tomorrow. The canvas answers one question about
them — which of the two neutrals a board is *born* holding, since a
black pencil on a dark ground draws a line nobody can see. It is asked
once, at birth; a theme set later leaves the ink in the hand alone.
Every swatch wears the chrome's hairline, because on any theme one
neutral is the colour of the panel under it.

A change arrives two ways. The board re-reads the theme whenever the
window comes back into focus — which is the moment the theme switcher
gives the keyboard back, and asks nothing of anybody. For a board left
in sight, `contrib/omarchy/omawhite` is a hook to symlink into
`~/.config/omarchy/hooks/theme-set.d/` (and `font-set.d/`); it runs
`omawhite --theme`, which never opens a window. Nothing on disk changes
either way: a theme is what the window is painted with.

The split is worth stating: the palette and the text size come from the
theme, so they need one — without theme files the board keeps the
colours and the size it has always had. The face and the corner come
from the session, so they follow wherever the session answers: on a
machine with fontconfig the board letters itself in `monospace`, and on
Hyprland it cuts its corners to the desktop's. Nothing is guessed
anywhere.

## Controls

| Input | Effect |
|---|---|
| `V` / `H` / `P` / `B` / `F` / `Z` | Select / Hand / Pencil / Brush / Frame / Zoom tool (also clickable in the dock) |
| `Esc` | Cancel the stroke, gesture or drag in progress; then clear the selection |
| Left drag (Brush) | Paint with the brush; the stroke is fitted to Béziers on release |
| `Shift` + `B` | Show / hide the brush strip (and the library with it) |
| Strip `≡` / bar chevron | Open / fold Brush Properties |
| Strip `>` | Open / shut the brush library beside the strip |
| `Shift` + `1`–`9`, `0` (Brush) | Take up the brush in that seat |
| Click a seat | The same, by hand |
| Drag a brush onto a seat | Put it there; slot `0` refuses, it is computed |
| Drag a slider in the bar | That property of the brush in hand |
| Click an icon in the library | Take up that brush |
| Wheel over the library | Walk the shelves |
| Bar `↺` | Put the brush back the way it shipped |
| `[` / `]` (Brush selected) | Brush smaller / larger |
| `{` / `}` (Brush selected) | Brush softer / harder |
| `1`–`9`, `0` (Brush selected) | Brush opacity 10%–90%, 100% |
| `Shift` + `L` | Show / hide the layers panel |
| Left drag (Frame) | Drag out a frame; what its area covers joins it |
| Click a frame card's `›` / the panel's title | Work inside that frame / back out to the board |
| Click an ink with a frame selected | Paint the frame's ground |
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
| `Ctrl` + `Z` | Undo: back to the state before the last change |
| `Ctrl` + `Shift` + `Z` | Redo |
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
cargo run -- --theme           # re-read the desktop's theme (needs a live board)
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
src/doc.rs       document §6.1 (pure data, serde): layers and frames, rect, path, image
src/store.rs     ~/.local/share/omawhite: boards/, blobs/, index.json, perms §9.3
src/ipc/         §5: proto (strict parser), client (forward), server (socket 0600)
src/bitmap.rs    decode PNG/JPEG/WebP to RGBA8, paste size (pure, tested)
src/curve.rs     simplify, cubic Bézier fit and flatten (pure, tested)
src/brush.rs     the brush library: sets, presets, a brush's body, its properties, the tip a stroke carries, the pointer's ring (pure, tested)
tools/import-skbrushes.py  Sketchbook `.skbrushes` -> assets/brushes/ (parameters + icon sheet)
assets/brushes/  library.json (17 sets, 211 brushes) and icons.png (211 cells), built into the binary
assets/dock/     six RGBA tool illustrations and their 6 x 1 icon sheet, built into the binary
src/scene.rs     View (camera + viewport + scale), document → SDF prims, frames, groups and passes (pure, tested)
src/geom.rs      affine maps, corners and oriented frames (pure, tested)
src/select.rs    selection: element frames, hit-testing, handles, transforms, overlay prims (pure, tested)
src/grid.rs      dotted background (pure, tested)
src/theme.rs     palette: light default, derived from op: theme or from the desktop's own theme (pure, tested)
src/omarchy.rs   the desktop's look: colors.toml, shell.toml, the monospace face, Hyprland's rounding (parsing pure, tested)
contrib/omarchy/omawhite  a theme-set / font-set hook, installed by hand
src/editor.rs    active tool, held keys, stroke and its tip, pan/zoom gesture, selection and its drag, the active layer (pure, tested)
src/dock.rs      bottom tool dock: layout, hit-test, illustrated icons + line fallback (pure, tested)
src/layers.rs    layers panel on the right: layout, hit-test, rows, eyes and buttons (pure, tested)
src/slots.rs     brush strip on the left: the brush in the hand, the two buttons, the ten seats (pure, tested)
src/palette.rs   brush library beside it: the shelves, the grid of icons, the scroll (pure, tested)
src/props.rs     brush properties bar under the strip: the basic pair, and the Advanced layout it drops (pure, tested)
src/tabs.rs      top tab strip: layout, hit-test, what a narrow tab drops (pure, tested)
src/text.rs      glyph atlas, measure, layout, ellipsis truncation (pure, tested)
src/project.rs   a document's origin (file, board, untitled) and dirty flag (pure, tested)
src/field.rs     a one-line editable value: the string, the caret, the keys that move it (pure, tested)
src/export.rs    what leaves the board: the scope, its box, its sub-document, the slug, the inventory, the write (pure, tested)
src/graft.rs     what comes back: the fragment, the ids minted anew, the free spot, the graft (pure, tested)
src/agents.rs    the agents running here: herdr, tmux and /proc parsed into one list, and reaching one (pure, tested)
src/send.rs      the export panel: the targets, the folder, the instruction (pure, tested)
src/gfx.rs       wgpu 30: the instanced SDF pipelines, image textures, the scratch a group is composited in
src/app.rs       winit: window, input routing, socket → event loop bridge
src/gestures.rs  trackpad pinch/swipe (zwp_pointer_gestures_v1) → event loop bridge
src/tablet.rs    the tablet's pen (zwp_tablet_v2) → event loop bridge; its frame is tested
src/clipboard.rs selection reads (wl_data_device) → event loop bridge
src/dialogs.rs   open/save-as/confirm over xdg-desktop-portal → event loop bridge
assets/fonts/    Liberation Sans (SIL OFL 1.1), compiled into the binary
plugin/          the Omarchy bar widget (QML): manifest, BarWidget, install notes
skills/omawhite/ the skill an agent installs to read and write frames: SKILL.md and an installer
```

Frame data flow: grid + document + live stroke + selection overlay +
brush ring + dock + brush strip + brush library + properties bar +
layers panel + tab strip →
`scene`/`select`/`brush`/`slots`/`palette`/`props`/`layers`/`tabs`
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
