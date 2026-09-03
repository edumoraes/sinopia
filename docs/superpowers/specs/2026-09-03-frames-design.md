# Frames

An area on the board that holds objects, with a layer stack of its own,
and which is itself a layer — of kind `frame` — on the board's stack.

There will be several kinds of frame. This spec covers the first, the
**basic frame**: it takes the objects that already exist, raster and
vector, it can carry a background, and what is inside it is cut by its
boundary.

## Why this is not a small change

Every walk over the document today assumes one stack: `Document.layers`
is a flat `Vec<Layer>`, an element names its layer by id, and
`Document::painted()` yields `(index, &Element)` in the layers' order.
The renderer, the pointer and the panel all read that one iterator, and
they agree because there is only one thing to agree about.

A frame breaks the assumption in three places at once. There is now more
than one stack, so a layer id alone no longer says where a layer sits.
There is a boundary, so what is painted and what is hit are no longer
the same question as what exists. And membership is decided by
**geometry** — where a stroke was born, where an object was let go of —
so the document changes shape under a drag, not only under a click in
the panel.

The seam is narrow, though: `painted()` has three real callers, one in
`scene` and two in `select`. Widening that one iterator is what carries
most of the feature.

## Decisions

Five were asked and answered; two follow from the engine.

- **Geometry decides membership.** A stroke born inside a frame's area
  belongs to it. An object let go of inside one joins it; dragged out,
  it leaves. The panel reflects what the geometry decided — it does not
  decide anything itself.
- **A Frame tool (`F`), dragged.** The gesture that places the area is
  the gesture that creates the frame.
- **The panel drills in.** Clicking a frame card's chevron replaces the
  panel's contents with that frame's own stack, with a breadcrumb back.
  The panel keeps showing exactly one flat stack, so its drag, lift,
  scroll and reorder machinery is untouched.
- **Born with the theme's surface; the dock's inks repaint it.** A new
  frame carries the panel colour, which is what makes it read as a frame
  against the dotted board. With a frame selected, clicking an ink in the
  dock paints its background. No new chrome.
- **Move carries the contents; resize re-cuts.** Dragging the frame
  carries what is inside it — their relation to the area does not change.
  Resizing moves the boundary only: the contents stay where they are and
  more or less of them shows.
- **A frame does not turn.** The cut is `Prim.clip`, an axis-aligned box
  already in the shader. An oriented cut is a shader change, not a line;
  so the rotation rings are not offered while a frame is in the
  selection, and `Frame` carries no `rotation` field — a field that
  cannot be honoured is worse than no field.
- **A frame does not nest.** A frame layer never appears inside a frame's
  own stack, and the parse refuses one. The Frame tool inside an existing
  frame still makes a top-level frame. Nesting is what approach C below
  buys, and it is a different cut.

## Three ways to model it, and why the first

**A — a frame is an `Element` on a layer of `kind: frame`.** `elements`
stays flat; an object inside a frame names one of that frame's own
layers, and since layer ids are unique across the whole document, the
`layer` an element already carries says where it is. Chosen.

It keeps the invariant the codebase is built on — *a layer is the object
it holds, and the two go together*: a vector layer holds its one path, a
frame layer holds its one frame. And it makes reparenting fall out:
moving an object into a frame is moving **its layer** from one stack to
the other. Nothing has to decide which inner layer an object lands on,
because it brings its own.

**B — the frame's data on the `Layer`** (`Layer { kind: Frame, frame:
Option<…> }`). Literally "the frame is a layer", but it breaks the split
that `select` rests on: geometry lives in elements, layers are
organisation. `select::frame`, `hits`, `transform` and `elements_in`
would each need an exception where today there is none.

**C — recursive containment** (`Element::Frame { children: Vec<Element> }`).
Nesting for free, but `painted()` yields an index into `doc.elements`;
recursion replaces that index with a path and rewrites all three callers,
the editor's selection, and the store. The right engine three cuts from
now, not this one.

### On the name

`Frame` is already taken twice: `geom::Frame` is the oriented box a
selection is drawn around, and `scene::Frame` is everything on screen.
The document type is still `Frame`, written `doc::Frame` at the two call
sites that already have one in scope. Renaming the domain to `Artboard`
would make the code disagree with the disk and with the UI, which is a
worse confusion than a qualified path.

## The model — `doc`

```rust
pub enum Kind {
    #[default]
    Raster,
    Vector,
    /// The layer a frame is the object of. Never inside a frame.
    Frame,
}

pub enum Element {
    Rect(Rect),
    Path(Path),
    Paint(Paint),
    Image(Image),
    Frame(Frame),
}

/// An area that holds objects, with a stack of its own. `x, y, w, h` is
/// the boundary in world units; it does not turn, because the cut that
/// makes it a frame is axis-aligned. `background` is the colour under
/// everything inside it, absent when the frame is clear. `layers` is its
/// own stack, bottom to top, never empty and never holding a frame.
pub struct Frame {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}
```

On disk:

```json
{ "id": "el_05", "type": "frame", "layer": "01J…",
  "x": -200, "y": -140, "w": 400, "h": 300,
  "background": "#fbfbfa",
  "layers": [ { "id": "01J…", "name": "Layer 1" } ] }
```

`background` is an unvalidated hex string, exactly as `Rect.stroke` and
`Rect.fill` are: `scene::parse_color` has a fallback and the document
does not own colour parsing.

Schema stays **1**. A new element type is what `paint` already did; a
board holding frames simply cannot be opened by a build that predates
them, which is what a closed schema means.

### Addressing two stacks

```rust
impl Document {
    /// The layers of the frame `frame` names, or the board's own when it
    /// names none.
    pub fn stack(&self, frame: Option<&str>) -> &[Layer];
    pub fn stack_mut(&mut self, frame: Option<&str>) -> &mut Vec<Layer>;

    /// Where a layer lives: the frame holding it (none — the board) and
    /// its index in that stack.
    pub fn locate(&self, layer: &str) -> Option<(Option<&str>, usize)>;

    pub fn frame(&self, id: &str) -> Option<&Frame>;
    pub fn frame_mut(&mut self, id: &str) -> Option<&mut Frame>;

    /// The topmost frame whose area holds `p`, or none for the open
    /// board. Hidden frames do not claim anything.
    pub fn frame_at(&self, p: [f64; 2]) -> Option<&str>;

    /// The frame element on `layer`, when that layer is a frame's.
    pub fn frame_on(&self, layer: &str) -> Option<&Frame>;
}
```

`add_layer`, `remove_layer`, `move_layer` and `reorder_layer` each take
a `frame: Option<&str>` ahead of their index, so an index is never read
against the wrong stack. `next_layer_name` counts within the stack it is
naming into, so a frame's first layer is `Layer 1` however many the board
has.

One more, the one that makes membership cheap:

```rust
/// Moves the layer `layer` — and so the object on it — to the top of
/// the stack `to` names. False when the layer is a frame's own, when it
/// is already there, or when either end does not exist.
pub fn rehome_layer(&mut self, layer: &str, to: Option<&str>) -> bool;
```

### Validation

`settle_layers` grows to walk both stacks:

- Every layer id is unique **across the whole document**, board and
  frames alike.
- A `Kind::Frame` layer is named by exactly one `Element::Frame`, and an
  `Element::Frame` sits only on a `Kind::Frame` layer. Neither half alone
  is a document.
- A frame's own stack holds no `Kind::Frame` layer.
- An empty frame stack gets `Layer 1`, exactly as the board's does — so
  every accessor's assumption that there is a layer to paint on holds in
  both stacks, and the panel's `+` and trash mean the same thing inside a
  frame as outside.
- Every element's `layer` exists in some stack; an element with an empty
  `layer` joins the board's first, as today.
- `w` and `h` are finite and greater than zero.

A board written before frames existed parses unchanged: `Kind` still
defaults to raster, `layers` still defaults, and nothing it holds is a
frame.

## Paint order — `painted()`

```rust
/// An element in paint order, and the frame whose boundary cuts it.
pub struct Painted<'a> {
    pub index: usize,
    pub element: &'a Element,
    pub within: Option<&'a Frame>,
}

pub fn painted(&self) -> impl DoubleEndedIterator<Item = Painted<'_>>;
```

The walk is the old one with one step inside it. For each visible layer
of the board's stack, in order:

- a **frame** layer yields its `Element::Frame` first (`within: None` —
  the frame is not inside itself), then, for each visible layer of its
  own stack, that layer's elements with `within: Some(frame)`;
- any other layer yields its elements with `within: None`, as before.

A hidden frame layer takes its whole contents with it, which is what
hiding a layer has always meant. The iterator stays double-ended, since
`Once` and `Chain` both are; a `Vec<Painted>` is an acceptable fallback
if the borrows fight, at the cost of an allocation per hit-test.

`index` is still an index into the flat `doc.elements`, for elements
inside a frame as much as outside — which is the point of keeping
`elements` flat, and what lets `select` go on answering with ids and
`editor` go on holding a selection of them.

## Rendering — `scene`

`document_prims` gains one derived value per element: the frame's
boundary in screen px, `within.map(|f| view.world_to_screen(…))`.

- **The frame itself** paints its background, when it has one, and then a
  hairline border in `theme.muted` — four thin rects, the way `Rect`'s
  outline is already drawn. The border is what makes an area with no
  background visible and hittable at all, and it goes down before the
  contents so ink can cover it.
- **Everything inside** is cut: every prim the element produces goes
  through `Prim::clipped(boundary)` before it reaches the frame — before
  `Frame::stroke` and before `Frame::sheet`, so a grouped stroke carries
  the cut into the scratch and a sheet carries it too.
- **The live stroke** is cut by the frame holding `live.layer`, found
  through `Document::locate`. This covers both paths: the stroke joining
  a `Paint` on its layer, and the one on a layer that holds no paint yet,
  which is still drawn last over everything — clipped to its own frame,
  though above the contents of any other. That last is a known wrinkle of
  the "drawn last" rule and is left as it is.

`Prim::clipped` overwrites `clip` rather than intersecting, and nothing
in `document_prims` sets a clip today, so the overwrite is safe here. The
plan notes it so a later cut inside a cut does not land silently wrong.

`Prim::painted_bounds` starts honouring the clip — intersecting the
painted box with it, and answering an empty box when they miss. A group's
bounds are the union of those, so a stroke mostly cut away no longer
wipes and composites the scratch over its uncut extent. This is a
correctness-neutral, work-saving change, and it is what keeps the frame
from making the compositor pay for ink nobody sees.

## Selection and hit-testing — `select`

- `frame(el)` answers `geom::Frame::spanning` over a frame's box, angle
  zero.
- `hits` for a frame is its box — it is an area with a surface, not an
  outline. Because the frame is yielded **before** its contents and
  `element_at` walks `.rev()`, the contents win wherever there are any:
  the frame is picked on its own background, which is the rule a person
  expects without a double-click gesture to teach.
- `element_at` and `elements_in` refuse an element whose `within` does
  not hold the point (or does not overlap the marquee). Ink cut away is
  ink that is not there for the pointer either — the renderer and the
  pointer keep agreeing, which is the property `painted()` exists to
  protect.
- `transform` maps a frame's box like a rect's and **drops the turn**: a
  frame does not rotate. `select::handles` still hands back eight, and
  the editor is what refuses to start a rotation while a frame is in the
  selection.

## Membership — `editor`

Geometry decides, at four moments.

- **Where new ink is born.** The frame is decided at the **press** and
  written on the stroke in progress — `Stroke.born: Option<String>`, from
  `doc.frame_at(world)`. It has to be: the live stroke is painted from
  its first sample, so if the release re-read the geometry the ink could
  be cut by one frame while it was being drawn and land in another. This
  is the same reason the release writes the tip taken at the press.

  Both halves then read `born` rather than the geometry.
  `Editor::live_layer` resolves the active layer **within that stack** —
  the stroke joins the paint it is going to join, or is drawn last when
  there is none, as today. `Editor::ink_layer` takes the same `born`: the
  active layer when it is in that stack and takes the kind, a fresh layer
  at the top of that stack otherwise. On the open board (`born: None`)
  both behave exactly as they do now. A stroke that wanders out of the
  frame it started in belongs to that frame and is cut by it.
- **A paste.** `paste_image` already places the image under the pointer;
  its fresh layer is opened in the stack the pointer is over.
- **The release of a move.** For each moved object that is not a frame,
  the centre of its box names its home through `frame_at`. Changed home,
  `rehome_layer`. The object keeps its own layer; the layer changes
  stacks.
- **The creation of a frame.** Every board-level layer whose object's
  centre falls in the new area moves into the frame, keeping their
  relative order. A frame drawn over things captures them — the
  alternative, a frame born empty over objects that visibly sit inside it
  and are not cut, contradicts the boundary the moment it appears.

Rehoming always moves the **layer**, never the element alone. A layer
holds one object in practice — that is the invariant `fresh_layer` keeps
— so the two are the same thing; where a layer somehow holds more, it
moves whole, because a layer is not a thing that can be in two stacks.

Deleting a frame takes its layer, its stack and everything on it, which
is what deleting a layer has always meant. The panel's `+` inside a frame
makes a raster layer in that frame's stack; it never makes a frame layer,
here or anywhere — only the Frame tool does.

With a frame in the selection, clicking an ink in the dock writes that
hex into its `background` rather than changing the ink new strokes are
laid in. The dock is a colour the window is holding; a selected frame is
the thing that colour has to go on.

Moving a frame carries its contents: the move drag applies its map to the
selection **and** to the elements of any selected frame's stacks. A
resize applies to the frame alone. The selection's own box is still built
from the selection proper, so the handles sit on the frame, not on a
union with what it holds.

`Editor::active_layer` becomes stack-relative: an id resolved through
`Document::locate`, and an index read against the stack the panel is
showing. `Editor::inside: Option<String>` is the frame the panel has
drilled into — session state on the editor, so it belongs to a tab the
way the selection and the active layer do, and it clears when its frame
is deleted.

## The Frame tool

`Tool::Frame`, hotkey `F`, sixth in `Tool::ALL` and in the dock. A press
anchors, a move previews the area as a hairline outline in
`theme.selection`, a release creates it. A drag under `MIN_FRAME_PX` in
either axis creates nothing — a click is not an area. The floor is in
screen px on purpose: what it refuses is a hand that did not mean to
drag, and that is the same few pixels at every zoom.

Creating means: a `Kind::Frame` layer above the active board layer, an
`Element::Frame` on it holding one `Layer 1`, `background` set to the
theme's surface, capture of what its area covers, the frame selected and
its layer made active.

## The panel — drilling in

`Panel::layout` keeps taking one `&[Layer]`; `app` hands it
`doc.stack(editor.inside())`. Everything the panel does — the lift, the
slides, `drop_index`, the scroll and its thumb — is untouched, because it
is still one flat stack.

What is added:

- A frame's card carries a chevron `›` at the end opposite the eye, where
  a raster or vector layer carries its mark. Clicking the chevron enters;
  clicking the rest of the card selects the layer as any card does. Two
  clicks, two meanings, on two targets. It is drawn in `theme.icon` and
  not in the `theme.muted` a mark gets: a mark is read and the chevron is
  pressed, and the panel already says that difference in colour.
- Inside a frame, the panel's header carries a breadcrumb — `‹` and the
  frame's layer name — and clicking it leaves. `PanelHit::Enter(index)`
  and `PanelHit::Leave` join the hits.
- Entering makes the top of the inner stack active; leaving makes the
  frame's own layer active.

## The theme

`Theme` gains `panel_hex: String` beside `ink_hex` — the panel colour as
a hex a document can hold, which is what a new frame's background is set
to. It needs the inverse of `parse_color`:

```rust
/// A colour as `#rrggbb`, the form a document holds.
pub fn to_hex(c: Rgba) -> String;
```

The existing theme test that asserts `ink_hex` parses back to `ink`
covers `panel_hex` the same way.

## What is out of this slice

- Nesting a frame in a frame.
- Turning a frame.
- Frame kinds beyond the basic one.
- Renaming a frame anywhere but through its layer's name.
- Capturing on **resize**: growing a frame over an object does not claim
  it. Only creation and the release of a move do.
- A frame in the export, thumbnails, or the plugin's index — none of them
  read element types.

## Testing

Every module touched is a pure one, and the suite is inline
`#[cfg(test)]`, so each of these is a test written before the code.

- `doc`: a frame round-trips; a frame layer with no frame is refused, and
  a frame on a raster layer too; a duplicate layer id across a board and a
  frame is refused; a frame inside a frame is refused; an empty frame
  stack gets `Layer 1`; a board written before frames parses unchanged;
  `locate`, `stack`, `frame_at` (topmost wins, hidden claims nothing),
  `rehome_layer`.
- `doc::painted`: order is board layer, frame, its stack, next board
  layer; a hidden frame layer takes its contents; `within` names the
  frame for the contents and none for the frame.
- `scene`: contents carry the boundary as their clip; the background and
  the border go down before them; a grouped stroke inside a frame carries
  the clip into its group; `painted_bounds` shrinks to the clip.
- `select`: a point on a frame's background picks the frame, a point on
  its content picks the content; a point over ink that is cut away picks
  nothing; a marquee misses cut-away ink; `transform` drops a frame's
  turn.
- `editor`: ink born inside a frame lands in its stack; born outside, on
  the board; a stroke that starts inside and ends outside still lands in
  the frame it started in, and `live_layer` and `ink_layer` answer for
  the same stack throughout it; an object let go of inside a frame
  rehomes, and out of one rehomes back; moving a frame carries its
  contents, resizing does not; a new frame captures what its area covers;
  deleting a frame takes its stack with it; an ink picked with a frame
  selected paints its background and leaves the stroke ink alone;
  `inside` clears when its frame goes.
- `layers`: the frame card's chevron hits as `Enter`, the rest of the card
  as the row; the breadcrumb hits as `Leave`; the panel laid out from a
  frame's stack behaves as it does from the board's.
- `theme`: `panel_hex` parses back to `panel`.
