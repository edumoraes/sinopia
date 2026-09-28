# The shape tool

Vector shapes on the board, as the design tools and the boards on the
market draw them: one Shape tool (`U`) and a row of predefined models —
rectangle, ellipse, triangle, diamond, polygon, star, line and arrow —
each dragged out on the canvas and set from a properties bar of its own,
in the middle over the canvas where the brush's and the text's stand.
Fizzy card #66, "Ferramenta Shape". Branch `shape-tool`, off `main`.
Test first, every time: a test that fails, then the production code that
makes it pass. Atomic, semantic commits.

## What "done" is

Parity with what Figma, Affinity, Excalidraw and tldraw do with shapes,
within one fill and one stroke per shape:

- The models: rectangle (with a corner radius), ellipse, triangle,
  diamond, polygon (its sides), star (its points and how deep it is cut
  in), line and arrow (a head at either end, open or filled).
- Drawing: a drag lays the model over the area dragged; `Shift` keeps it
  square — a circle, a regular polygon, a line at 15° steps — and `Alt`
  draws it from its centre; a click lays it at its default size, centred
  on the click. The shape lands on a vector layer of its own, named after
  its model ("Rectangle 1"), inside the frame the press landed in, and
  arrives selected. What is being dragged out shows as the shape itself.
- Keys: `U` takes the tool, `U` again steps through the models; `R`, `O`,
  `L` and `A` take it with the rectangle, the ellipse, the line or the
  arrow in hand (Figma's and the draft's §7.2).
- A closed shape is drawn as a distance field — exact, sharp at every
  zoom, antialiased — with its stroke laid inside its edge, so what it
  paints is its box; a line is a capsule, its heads two more or a
  triangle.
- Hit on what shows: a filled shape anywhere inside, a hollow one only on
  its stroke, a line on its ink. It moves, resizes, turns and flips like
  anything else; a lone line wears its two ends as handles, and dragging
  one moves that end.
- The bar: the eight models, the fill and the stroke — none or one of the
  dock's inks, from a menu that tries each on the shape as the pointer
  passes — the stroke's width, and what the model has of its own: the
  rectangle's radius, the polygon's sides, the star's points and inner
  radius, a line's two heads. It looks at the shapes selected, or at how
  the next one will be drawn; a change is one step of the history.
- The dock's ink colours the stroke of the shapes selected, as it colours
  a text.
- The layers panel shows a shape's layer as the vector layer it is; a
  merge keeps shapes as shapes; export, the thumbnails and a merge's
  picture draw them; `board.md` counts them by model; a fragment grafted
  by an agent may carry them, and the skill says how.

Deliberately not in this cut: several fills or strokes on one shape,
gradients, dashes, a colour picker past the dock's inks, on-canvas
handles for a radius or a star's depth, corner radius on polygons and
stars, lines with more than two points or curved, connectors that follow
what they join, labels inside shapes, boolean operations, and turning a
shape into a path.

## Tasks

Model and pure core:

- [x] `doc`: `Element::Shape` — a model fitted to a box, its fill, its
      stroke and width, a rectangle's radius, a polygon's sides and a
      star's inner radius — checked on the way in.
- [x] `shape`: the models' geometry — a polygon's and a star's unit
      vertices and their fit to the box, the distance to an ellipse, to a
      polygon and to a rounded box, what is inside.
- [x] `select`: a shape's frame, its hit (filled inside, hollow on the
      stroke) and its transform.
- [x] `scene` + `gfx`: an ellipse and a polygon as distance fields, and
      a ring — the stroke laid inside the edge — for them and the box.
- [x] `editor`: the Shape tool — the drag, Shift, Alt, the click, the
      layer named after the model, the shape selected, Esc.
- [x] `doc`: `Element::Line` — two ends, a stroke and its width, a head
      at either end.
- [x] `shape` + `select`: a line's shaft and heads; its frame, hit and
      transform.
- [x] `scene`: a line and its heads drawn.
- [x] `editor`: the Line and Arrow models; Shift at 15° steps.
- [x] `select` + `editor`: a lone line's ends are its handles.
- [x] `editor`: where the shape bar looks and what it changes — the
      style of the next shape, the shapes selected, the model switched.

Chrome:

- [x] `shapebar`: the bar — the models, the fill and stroke wells, the
      width, and each model's own sliders and buttons.
- [x] `dock`: the Shape tool, its illustration and its line icon, `U`.

Shell:

- [x] `app`: the tool on the canvas — the shape being dragged out drawn,
      the cursor, the keys, `Alt` held.
- [x] `app`: the shape bar, its colour menus tried as the pointer
      passes, a slider's drag as one step; the dock's ink on the shapes
      selected.

The door an agent asks through:

- [x] `export`: `board.md` counts shapes and lines by model.
- [x] `graft` + `skills/sinopia/SKILL.md`: a fragment may carry shapes
      and lines.

Wrap-up:

- [x] AGENTS.md, DEVELOPMENT.md, ARCHITECTURE.md (§6.1, §7.2).
- [x] `cargo build`, `cargo clippy --all-targets` with zero warnings;
      `cargo test`.
- [x] Seen working in a window: every model dragged out, Shift and Alt,
      a click; fill and stroke from the bar and the dock; width, radius,
      sides, points and depth; heads; a line's ends dragged; resize,
      turn and flip; undo; the layers panel; a deep zoom.
- [x] An independent review of the diff: two defects confirmed and one
      plausible, each fixed with a test, and two old patterns the new
      state inherited fixed with them.
- [x] Pushed (`shape-tool`); CI green — build, clippy, the suite and the
      release archive, and the plugin's own suite.
