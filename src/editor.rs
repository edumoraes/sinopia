//! Session state that never enters the document (§6.2): the active tool,
//! the keys temporarily overriding it, the stroke being drawn, the
//! pan/zoom gesture in progress, the selection and the drag reshaping it.
//! Pure — `app` feeds it pointer events in screen px together with the
//! current [`View`] and the document, and stores whatever comes back.

use crate::bitmap;
use crate::brush::{Dynamics, Tip};
use crate::curve::{self, Cubic};
use crate::doc::{Camera, Document, Element, Envelope, Image, Kind, Paint, Path, new_id};
use crate::geom::{Affine, Corner, Frame, Point};
use crate::scene::View;
use crate::select::{self, Handle};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Hand,
    Pencil,
    Brush,
    Zoom,
}

impl Tool {
    /// Dock order.
    pub const ALL: [Tool; 5] = [
        Tool::Select,
        Tool::Hand,
        Tool::Pencil,
        Tool::Brush,
        Tool::Zoom,
    ];

    pub fn hotkey(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Hand => 'h',
            Tool::Pencil => 'p',
            Tool::Brush => 'b',
            Tool::Zoom => 'z',
        }
    }

    pub fn from_hotkey(c: char) -> Option<Tool> {
        let c = c.to_ascii_lowercase();
        Tool::ALL.into_iter().find(|t| t.hotkey() == c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// What an input did, so `app` knows what to redraw and store.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub enum Change {
    None,
    /// The stroke or the document changed.
    Scene,
    /// The camera moved; the caller keeps it.
    Camera(Camera),
    /// Only session state changed (selection, marquee): redraw, nothing
    /// to save.
    Selection,
}

/// Trackpad gesture step. Deltas are the fingers' travel in logical px;
/// `factor` is the pinch scale relative to the previous step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Pinch {
        dx: f64,
        dy: f64,
        factor: f64,
    },
    Swipe {
        fingers: u32,
        dx: f64,
        dy: f64,
    },
    /// The fingers lifted (or the compositor cancelled the gesture).
    End,
}

/// Pen width in world units (logical px at zoom 1).
pub const PEN_WIDTH: f64 = 2.0;

/// How far the committed path may stray from the pointer, in screen px.
pub const FIT_TOLERANCE_PX: f64 = 1.0;

/// One zoom unit: a click of the zoom tool, one wheel notch.
pub const ZOOM_STEP: f64 = 1.25;

/// Logical px the zoom tool has to be dragged for one zoom unit.
pub const ZOOM_DRAG_PX: f64 = 100.0;

/// Logical px one wheel line scrolls — also the scroll distance for one
/// zoom unit, so a notch is a unit either way.
pub const SCROLL_LINE_PX: f64 = 40.0;

/// Pointer travel (logical px) under which a press and release is a click.
pub const CLICK_SLOP_PX: f64 = 3.0;

/// Logical px past an element's edge that still hit it.
pub const HIT_SLOP_PX: f64 = 4.0;

/// Rotation step, in degrees, while Shift is held.
pub const ROTATE_SNAP_DEG: f64 = 15.0;

/// How much of the visible world a paste may take up before it is shrunk
/// to fit: a 4K screenshot should not cover the board wall to wall.
pub const PASTE_ROOM: f64 = 0.8;

/// Pan or zoom gesture in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Nav {
    /// The world point grabbed stays under the pointer.
    Pan { button: Button, anchor: (f64, f64) },
    /// Scrubby zoom: horizontal travel from `origin` sets the zoom relative
    /// to `zoom`, keeping `anchor` (the world under `origin`) fixed there.
    Zoom {
        origin: (f64, f64),
        anchor: (f64, f64),
        zoom: f64,
        dragged: bool,
    },
}

/// The selected elements as they were when a drag started, with their
/// positions in the document.
type Snapshot = Vec<(usize, Element)>;

/// Selection drag in progress. Each step recomputes the transform (`map`)
/// from the press and applies it to the snapshot, so steps never
/// accumulate error and cancelling just puts the snapshot back; `frame` is
/// the selection frame at the press, shown through `map` while it lasts.
#[derive(Debug, Clone, PartialEq)]
enum Drag {
    /// The pointer went down on a selected element; the selection follows
    /// it once it travels past the click slop.
    Move {
        origin: Point,
        screen: (f64, f64),
        frame: Frame,
        map: Affine,
        snapshot: Snapshot,
        moved: bool,
    },
    Resize {
        corner: Corner,
        frame: Frame,
        map: Affine,
        snapshot: Snapshot,
    },
    Rotate {
        origin: Point,
        frame: Frame,
        map: Affine,
        snapshot: Snapshot,
    },
    /// Rubber band between two screen points; `base` is what Shift keeps
    /// selected underneath it.
    Marquee {
        origin: (f64, f64),
        current: (f64, f64),
        base: Vec<String>,
    },
}

/// What the input is doing besides being somewhere: how hard the tip is
/// pressed, 0–1, and which way it is held — the barrel's own lean, in
/// the tablet's degrees, and the roll of a pen that reports one. A
/// mouse is a stylus that presses all the way and stands straight up,
/// which is what this is until a pen says otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stylus {
    pub pressure: f64,
    pub tilt: (f64, f64),
    pub roll: f64,
}

impl Stylus {
    pub const MOUSE: Stylus = Stylus {
        pressure: 1.0,
        tilt: (0.0, 0.0),
        roll: 0.0,
    };

    /// The turn the stylus gives the nib, in degrees, under `dynamics`:
    /// the way the barrel leans, and — on a pen that reports one — the
    /// roll on top of it. The other two dynamics turn nothing here:
    /// standing still is a nib the stylus does not touch, and following
    /// the stroke is the stroke's own doing, not the hand's.
    pub fn twist(&self, dynamics: Dynamics) -> f64 {
        match dynamics {
            Dynamics::Tilt => self.lean(),
            Dynamics::TiltAndRoll => self.lean() + self.roll,
            Dynamics::None | Dynamics::ToStroke => 0.0,
        }
    }

    /// Which way the barrel leans, in degrees. A pen standing straight
    /// up leans nowhere.
    fn lean(&self) -> f64 {
        let (x, y) = self.tilt;
        if x == 0.0 && y == 0.0 {
            0.0
        } else {
            y.atan2(x).to_degrees()
        }
    }
}

impl Default for Stylus {
    fn default() -> Stylus {
        Stylus::MOUSE
    }
}

/// The stroke being drawn: raw pointer samples in world units, what the
/// stylus said at each of them, and the tip they were taken with — the
/// pencil's, or the brush's settings at the press, so a setting changed
/// mid-stroke does not change the ink.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub points: Vec<[f64; 2]>,
    /// One reading per point, in step with it.
    pub stylus: Vec<Stylus>,
    pub tip: Tip,
    /// The frame the press was in, if any. Read once, at the press, and
    /// never again: the stroke is painted from its first sample, so if
    /// the release read the geometry a second time the ink could be cut
    /// by one boundary while it was drawn and land under another. The
    /// tip is taken at the press for the same reason.
    pub born: Option<String>,
}

impl Stroke {
    /// What the pen said along the stroke, evened out onto stations of
    /// its own arc length: what the walk lays the dabs by while it is
    /// being drawn, and what the release writes down — the same
    /// envelope both times, so the ink does not change when the stroke
    /// is let go of.
    ///
    /// Only what the nib actually reads is kept. A brush no pressure
    /// drives, a nib the stylus does not turn, or a mouse — which
    /// presses all the way from end to end — leaves an empty envelope,
    /// and the board says nothing about a pen that was not there.
    pub fn envelope(&self) -> Envelope {
        // The pencil sweeps and has no nib to ask, so `Tip::drive` is
        // what answers for it: it thins with the hand like anything else.
        let pressure = self.readings(!self.tip.drive().is_none(), |s| s.pressure, 1.0);
        let twist = self.readings(true, |s| s.twist(self.tip.dynamics), 0.0);
        Envelope { pressure, twist }
    }

    /// One of the stylus's own numbers along the stroke, or nothing at
    /// all when the nib does not read it or when it never left `flat` —
    /// the value a stroke drawn without a pen has the whole way.
    fn readings(&self, wanted: bool, of: impl Fn(&Stylus) -> f64, flat: f64) -> Vec<f32> {
        if !wanted {
            return Vec::new();
        }
        let readings: Vec<f32> = self.stylus.iter().map(|s| of(s) as f32).collect();
        if readings.iter().all(|&r| r == flat as f32) {
            return Vec::new();
        }
        curve::resample(&self.points, &readings)
    }
}

#[derive(Debug, Default)]
pub struct Editor {
    tool: Tool,
    space: bool,
    ctrl: bool,
    shift: bool,
    /// What the pen last said, or a mouse's own. Physical, like the
    /// held keys: it belongs to the hand, not to the board, and a tab
    /// switch does not hand it on.
    stylus: Stylus,
    stroke: Option<Stroke>,
    nav: Option<Nav>,
    /// Ids of the selected elements, in selection order.
    selection: Vec<String>,
    drag: Option<Drag>,
    /// The layer new ink lands on, by id; `None` is the topmost. Kept as
    /// an id, not an index, so it survives the layers being reordered.
    layer: Option<String>,
    /// The frame whose stack is being worked in, by id; `None` is the
    /// board's own. Session state like the active layer and the
    /// selection: it belongs to a tab, and every use of it goes
    /// through [`Editor::inside_in`], so a frame that has been deleted
    /// cannot leave the editor pointing into it.
    inside: Option<String>,
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    /// The tool the dock shows.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Switching tools abandons whatever was in progress, selection
    /// included.
    pub fn set_tool(&mut self, tool: Tool, doc: &mut Document) {
        self.cancel(doc);
        if tool != self.tool {
            self.selection.clear();
        }
        self.tool = tool;
    }

    /// The tool inputs go to: Ctrl held is Zoom, Space held is Hand,
    /// otherwise the selected one.
    pub fn active_tool(&self) -> Tool {
        if self.ctrl {
            Tool::Zoom
        } else if self.space {
            Tool::Hand
        } else {
            self.tool
        }
    }

    /// The tool a press at `screen` (physical px) would use: the active
    /// one, except that Ctrl — Zoom everywhere else — hands a resize
    /// handle of the selection back to Select, where it means "resize from
    /// the center". The rotation rings keep zooming: a turn has no use for
    /// Ctrl.
    pub fn pointer_tool(&self, doc: &Document, view: &View, screen: (f64, f64)) -> Tool {
        let tool = self.active_tool();
        let over_resize = matches!(self.hover(doc, view, screen), Some(Handle::Resize(_)));
        if tool == Tool::Zoom && self.ctrl && over_resize {
            return Tool::Select;
        }
        tool
    }

    pub fn hold_space(&mut self, down: bool) {
        self.space = down;
    }

    pub fn hold_ctrl(&mut self, down: bool) {
        self.ctrl = down;
    }

    /// Shift makes clicks and the marquee add to the selection.
    pub fn hold_shift(&mut self, down: bool) {
        self.shift = down;
    }

    /// Test-only until the shell needs the ids themselves.
    #[cfg(test)]
    pub fn selection(&self) -> &[String] {
        &self.selection
    }

    /// The frame around the selection, if anything is selected. While a
    /// drag reshapes it, the frame from the press follows the drag's map,
    /// so a group's box turns with it instead of being re-wrapped.
    pub fn selection_frame(&self, doc: &Document) -> Option<Frame> {
        match &self.drag {
            Some(
                Drag::Move { frame, map, .. }
                | Drag::Resize { frame, map, .. }
                | Drag::Rotate { frame, map, .. },
            ) => Some(frame.transformed(map)),
            _ => select::frame_of(doc, &self.selection),
        }
    }

    /// The marquee's two screen corners while one is being dragged.
    pub fn marquee(&self) -> Option<((f64, f64), (f64, f64))> {
        match self.drag {
            Some(Drag::Marquee {
                origin, current, ..
            }) => Some((origin, current)),
            _ => None,
        }
    }

    /// The selection is being dragged around.
    pub fn is_moving(&self) -> bool {
        matches!(self.drag, Some(Drag::Move { moved: true, .. }))
    }

    /// The selection handle under `screen`, for the cursor.
    pub fn hover(&self, doc: &Document, view: &View, screen: (f64, f64)) -> Option<Handle> {
        let frame = self.selection_frame(doc)?;
        self.handle_at(doc, &frame, view, screen)
    }

    /// The handle under `screen`, less the ones the selection cannot
    /// honour: a frame does not turn, so its rings are not offered — the
    /// cursor must not promise a turn that will not happen.
    fn handle_at(
        &self,
        doc: &Document,
        frame: &Frame,
        view: &View,
        screen: (f64, f64),
    ) -> Option<Handle> {
        let handle = select::handle_at(frame, view, screen)?;
        match handle {
            Handle::Rotate(_) if self.selection_holds_a_frame(doc) => None,
            _ => Some(handle),
        }
    }

    pub fn stroke(&self) -> Option<&Stroke> {
        self.stroke.as_ref()
    }

    /// The frame being worked in, as last recorded. `app` reads this to
    /// notice a change between frames; everything with a document in
    /// hand asks [`Editor::inside_in`] instead.
    pub fn inside(&self) -> Option<&str> {
        self.inside.as_deref()
    }

    /// The frame being worked in, checked against `doc`: none once that
    /// frame is gone, which is what makes the field self-clearing.
    fn inside_in(&self, doc: &Document) -> Option<&str> {
        self.inside.as_deref().filter(|id| doc.frame(id).is_some())
    }

    /// The stack new ink goes to: the frame the stroke in progress was
    /// born in, or the one being worked in when no stroke is in flight.
    /// The panel and the ink can disagree — pressing inside a frame
    /// while the panel stands on the board paints in the frame — and
    /// this is where they are told apart.
    fn ink_stack(&self, doc: &Document) -> Option<&str> {
        match &self.stroke {
            Some(s) => s.born.as_deref().filter(|id| doc.frame(id).is_some()),
            None => self.inside_in(doc),
        }
    }

    /// The active layer's index in `stack`, which is not always the one
    /// the panel is showing: a press inside a frame paints there while
    /// the panel stands on the board.
    fn index_in(&self, doc: &Document, stack: Option<&str>) -> usize {
        self.layer
            .as_deref()
            .and_then(|id| doc.layer_index(stack, id))
            .unwrap_or(doc.stack(stack).len().saturating_sub(1))
    }

    /// Works inside `frame` from now on: the panel shows its stack and
    /// new ink lands there. The top of that stack becomes active.
    pub fn enter_frame(&mut self, doc: &Document, frame: &str) -> Change {
        if doc.frame(frame).is_none() {
            return Change::None;
        }
        self.inside = Some(frame.to_owned());
        self.layer = doc.stack(Some(frame)).last().map(|l| l.id.clone());
        Change::Selection
    }

    /// Back out to the board, with the frame's own layer active — the
    /// card the pointer came from.
    pub fn leave_frame(&mut self, doc: &Document) -> Change {
        let Some(frame) = self.inside.take() else {
            return Change::None;
        };
        self.layer = doc.frame(&frame).map(|f| f.layer.clone());
        Change::Selection
    }

    /// Index of the layer new ink lands on, in the stack being worked
    /// in: the one chosen, or the top one when none was, when the chosen
    /// one is gone, or when it is on another stack.
    pub fn active_layer(&self, doc: &Document) -> usize {
        let stack = self.inside_in(doc);
        self.layer
            .as_deref()
            .and_then(|id| doc.layer_index(stack, id))
            .unwrap_or(doc.stack(stack).len().saturating_sub(1))
    }

    /// Opens a layer of `kind` above the active one, makes it active and
    /// answers its id — what an element that comes with its own layer is
    /// stamped with.
    fn fresh_layer(&mut self, doc: &mut Document, kind: Kind, stack: Option<&str>) -> String {
        let stack = stack.filter(|id| doc.frame(id).is_some()).map(str::to_owned);
        let above = self.index_in(doc, stack.as_deref());
        let at = doc
            .add_layer(stack.as_deref(), above, kind)
            .unwrap_or_default();
        let id = doc.stack(stack.as_deref())[at].id.clone();
        self.layer = Some(id.clone());
        id
    }

    /// The layer a new element of `kind` lands on. Raster accumulates —
    /// it joins the active layer when that one takes pixels, and opens
    /// one above it when it does not, as Photoshop does when you paint on
    /// a shape. Vector does not accumulate: every object gets a layer of
    /// its own.
    /// The layer the stroke in progress would join, if there is one to
    /// join: the active layer when it takes pixels and the stroke is a
    /// brush's. A pencil opens a layer of its own and a brush over a
    /// vector layer opens one above it, so neither has anywhere to be
    /// painted yet — they are drawn over everything until they land.
    pub fn live_layer<'a>(&self, doc: &'a Document) -> Option<&'a str> {
        if self.tool == Tool::Pencil {
            return None;
        }
        let stack = self.ink_stack(doc);
        doc.stack(stack)
            .get(self.index_in(doc, stack))
            .filter(|l| l.kind == Kind::Raster)
            .map(|l| l.id.as_str())
    }

    fn ink_layer(&mut self, doc: &mut Document, kind: Kind, stack: Option<&str>) -> String {
        let stack = stack.filter(|id| doc.frame(id).is_some());
        if kind == Kind::Raster
            && let Some(layer) = doc.stack(stack).get(self.index_in(doc, stack))
            && layer.kind == Kind::Raster
        {
            return layer.id.clone();
        }
        let stack = stack.map(str::to_owned);
        self.fresh_layer(doc, kind, stack.as_deref())
    }

    /// Makes layer `index` the one new ink lands on.
    pub fn select_layer(&mut self, doc: &Document, index: usize) -> Change {
        match doc.stack(self.inside_in(doc)).get(index) {
            Some(layer) => {
                self.layer = Some(layer.id.clone());
                Change::Selection
            }
            None => Change::None,
        }
    }

    /// Adds a layer above the active one and makes it active.
    pub fn add_layer(&mut self, doc: &mut Document) -> Change {
        let stack = self.inside_in(doc).map(str::to_owned);
        let above = self.active_layer(doc);
        let at = doc
            .add_layer(stack.as_deref(), above, Kind::Raster)
            .unwrap_or_default();
        self.layer = Some(doc.stack(stack.as_deref())[at].id.clone());
        Change::Scene
    }

    /// Removes the active layer with everything on it — out of the
    /// selection too — and activates the layer that was above it, or
    /// the one below when it was on top. The last layer stays, but what
    /// it holds does not: a layer is its object, so the trash takes the
    /// object either way.
    pub fn remove_layer(&mut self, doc: &mut Document) -> Change {
        let stack = self.inside_in(doc).map(str::to_owned);
        let index = self.active_layer(doc);
        // A frame layer takes its frame with it, and the panel cannot go
        // on standing in a stack that is gone.
        let leaving = doc
            .stack(stack.as_deref())
            .get(index)
            .and_then(|l| doc.frame_on(&l.id))
            .map(|f| f.id.clone());
        if !doc.remove_layer(stack.as_deref(), index) {
            let Some(layer) = doc.stack(stack.as_deref()).get(index) else {
                return Change::None;
            };
            let last = layer.id.clone();
            if !doc.elements.iter().any(|el| el.layer() == last) {
                return Change::None;
            }
            doc.elements.retain(|el| el.layer() != last);
            self.drag = None;
            self.selection
                .retain(|id| doc.elements.iter().any(|el| el.id() == id));
            return Change::Scene;
        }
        if self.inside.as_deref() == leaving.as_deref() {
            self.inside = None;
        }
        self.drag = None;
        self.selection
            .retain(|id| doc.elements.iter().any(|el| el.id() == id));
        let stack = self.inside_in(doc).map(str::to_owned);
        let next = index.min(doc.stack(stack.as_deref()).len() - 1);
        self.layer = Some(doc.stack(stack.as_deref())[next].id.clone());
        Change::Scene
    }

    /// Shows or hides layer `index`. What is hidden cannot be seen, so
    /// it cannot stay selected either.
    pub fn toggle_layer(&mut self, doc: &mut Document, index: usize) -> Change {
        let stack = self.inside_in(doc).map(str::to_owned);
        let Some(layer) = doc
            .stack_mut(stack.as_deref())
            .and_then(|layers| layers.get_mut(index))
        else {
            return Change::None;
        };
        layer.visible = !layer.visible;
        if !layer.visible {
            let hidden = layer.id.clone();
            self.drag = None;
            self.selection.retain(|id| {
                doc.elements
                    .iter()
                    .any(|el| el.id() == id && el.layer() != hidden)
            });
        }
        Change::Scene
    }

    /// Moves the active layer one step up or down. It keeps its id, so
    /// it stays active.
    pub fn move_layer(&mut self, doc: &mut Document, up: bool) -> Change {
        let stack = self.inside_in(doc).map(str::to_owned);
        let index = self.active_layer(doc);
        match doc.move_layer(stack.as_deref(), index, up) {
            Some(_) => Change::Scene,
            None => Change::None,
        }
    }

    /// Drops the active layer at `index`, however far that is — what a
    /// row dragged in the panel does. It keeps its id, so it stays
    /// active wherever it lands.
    pub fn move_layer_to(&mut self, doc: &mut Document, index: usize) -> Change {
        let stack = self.inside_in(doc).map(str::to_owned);
        let from = self.active_layer(doc);
        match doc.reorder_layer(stack.as_deref(), from, index) {
            true => Change::Scene,
            false => Change::None,
        }
    }

    pub fn is_drawing(&self) -> bool {
        self.stroke.is_some()
    }

    pub fn is_panning(&self) -> bool {
        matches!(self.nav, Some(Nav::Pan { .. }))
    }

    /// A button went down on the canvas at `screen` (physical px). Extra
    /// buttons during a stroke, gesture or drag are ignored. `brush` is
    /// what the brush tool paints with, read now: the tip is the
    /// stroke's from here on.
    pub fn press(
        &mut self,
        button: Button,
        view: &View,
        screen: (f64, f64),
        doc: &mut Document,
        // What the brush tool would lay: the whole nib, body and
        // shape, which only the library knows both halves of.
        brush: &Tip,
    ) -> Change {
        if self.stroke.is_some() || self.nav.is_some() || self.drag.is_some() {
            return Change::None;
        }
        let world = view.screen_to_world(screen.0, screen.1);
        let tool = self.pointer_tool(doc, view, screen);
        match (button, tool) {
            (Button::Left, Tool::Select) => self.select_press(view, screen, doc),
            (Button::Middle, _) | (Button::Left, Tool::Hand) => {
                self.nav = Some(Nav::Pan {
                    button,
                    anchor: world,
                });
                Change::None
            }
            (Button::Left, Tool::Pencil) => {
                let born = doc.frame_at([world.0, world.1]).map(str::to_owned);
                self.start_stroke(world, Tip::PENCIL, born)
            }
            (Button::Left, Tool::Brush) => {
                let born = doc.frame_at([world.0, world.1]).map(str::to_owned);
                self.start_stroke(world, brush.clone(), born)
            }
            (Button::Left, Tool::Zoom) => {
                self.nav = Some(Nav::Zoom {
                    origin: screen,
                    anchor: world,
                    zoom: view.camera.zoom,
                    dragged: false,
                });
                Change::None
            }
            (Button::Right, Tool::Zoom) => Change::Camera(view.zoomed_at(1.0 / ZOOM_STEP, screen)),
            _ => Change::None,
        }
    }

    fn start_stroke(&mut self, world: (f64, f64), tip: Tip, born: Option<String>) -> Change {
        self.stroke = Some(Stroke {
            points: vec![[world.0, world.1]],
            stylus: vec![self.stylus],
            tip,
            born,
        });
        Change::Scene
    }

    /// What the pen is doing, read from here on. `app` sets it from the
    /// tablet's own frames and puts it back to [`Stylus::MOUSE`] the
    /// moment a mouse moves, so a pen lifted off the tablet cannot
    /// leave the mouse drawing at no pressure at all.
    pub fn set_stylus(&mut self, stylus: Stylus) {
        self.stylus = stylus;
    }

    /// Select tool, left button: a handle starts a resize or a rotation;
    /// an element becomes the selection (Shift toggles it in and out),
    /// makes its layer the active one, and starts a move; empty canvas
    /// starts a marquee, clearing the selection unless Shift keeps it as
    /// the base.
    fn select_press(&mut self, view: &View, screen: (f64, f64), doc: &mut Document) -> Change {
        let world = point(view.screen_to_world(screen.0, screen.1));
        if let Some(frame) = self.selection_frame(doc)
            && let Some(handle) = self.handle_at(doc, &frame, view, screen)
        {
            let snapshot = self.snapshot(doc);
            let map = Affine::IDENTITY;
            self.drag = Some(match handle {
                Handle::Resize(corner) => Drag::Resize {
                    corner,
                    frame,
                    map,
                    snapshot,
                },
                Handle::Rotate(_) => Drag::Rotate {
                    origin: world,
                    frame,
                    map,
                    snapshot,
                },
            });
            return Change::None;
        }
        let slop = HIT_SLOP_PX * view.scale / view.px_per_world();
        let Some(id) = select::element_at(doc, world, slop).map(str::to_owned) else {
            let base = if self.shift {
                self.selection.clone()
            } else {
                Vec::new()
            };
            self.selection = base.clone();
            self.drag = Some(Drag::Marquee {
                origin: screen,
                current: screen,
                base,
            });
            return Change::Selection;
        };
        if let Some(el) = doc.elements.iter().find(|el| el.id() == id) {
            let layer = el.layer().to_owned();
            // A layer belongs to a stack, so going to it means going to
            // that stack: picking something inside a frame goes in with
            // it, and picking something on the board comes back out.
            self.inside = doc
                .locate(&layer)
                .and_then(|(frame, _)| frame)
                .map(str::to_owned);
            self.layer = Some(layer);
        }
        if self.shift {
            if let Some(i) = self.selection.iter().position(|s| *s == id) {
                self.selection.remove(i);
                return Change::Selection;
            }
            self.selection.push(id);
        } else if !self.selection.contains(&id) {
            self.selection = vec![id];
        }
        let Some(frame) = select::frame_of(doc, &self.selection) else {
            return Change::Selection;
        };
        self.drag = Some(Drag::Move {
            origin: world,
            screen,
            frame,
            map: Affine::IDENTITY,
            snapshot: self.move_snapshot(doc),
            moved: false,
        });
        Change::Selection
    }

    /// Where the moved objects live now. An object's centre names its
    /// home — a frame's area, or the open board — and what moves is the
    /// object's **layer**, since a layer holds one object and the two go
    /// together: the element goes on naming its layer, and its layer
    /// changes stacks. A frame never rehomes, because it does not nest.
    fn rehome_moved(&self, doc: &mut Document) {
        let mut moves: Vec<(String, Option<String>)> = Vec::new();
        for id in &self.selection {
            let Some(el) = doc.elements.iter().find(|el| el.id() == id) else {
                continue;
            };
            if matches!(el, Element::Frame(_)) {
                continue;
            }
            let Some(f) = select::frame(el) else { continue };
            let home = doc.frame_at(f.center).map(str::to_owned);
            let layer = el.layer().to_owned();
            let was = doc.locate(&layer).and_then(|(f, _)| f).map(str::to_owned);
            if was != home {
                moves.push((layer, home));
            }
        }
        for (layer, home) in moves {
            doc.rehome_layer(&layer, home.as_deref());
        }
    }

    /// What a move carries: the selection, and everything held by any
    /// frame in it. Moving a frame moves what is inside — their relation
    /// to the area does not change — while a resize moves the boundary
    /// alone, which is why only this drag uses it.
    fn move_snapshot(&self, doc: &Document) -> Snapshot {
        let held: Vec<&str> = doc
            .elements
            .iter()
            .filter_map(|el| match el {
                Element::Frame(f) if self.selection.iter().any(|id| id == &f.id) => Some(f),
                _ => None,
            })
            .flat_map(|f| f.layers.iter().map(|l| l.id.as_str()))
            .collect();
        doc.elements
            .iter()
            .enumerate()
            .filter(|(_, el)| {
                self.selection.iter().any(|id| id == el.id())
                    || held.iter().any(|l| *l == el.layer())
            })
            .map(|(i, el)| (i, el.clone()))
            .collect()
    }

    /// Whether a frame is in the selection — which is what refuses a
    /// rotation, since the cut that makes a frame is axis-aligned.
    fn selection_holds_a_frame(&self, doc: &Document) -> bool {
        doc.elements.iter().any(|el| {
            matches!(el, Element::Frame(_)) && self.selection.iter().any(|id| id == el.id())
        })
    }

    fn snapshot(&self, doc: &Document) -> Snapshot {
        doc.elements
            .iter()
            .enumerate()
            .filter(|(_, el)| self.selection.iter().any(|id| id == el.id()))
            .map(|(i, el)| (i, el.clone()))
            .collect()
    }

    /// The pointer moved to `screen`. Stroke points closer than one
    /// physical px to the last one are dropped so jitter does not bloat the
    /// path.
    pub fn moved(&mut self, view: &View, screen: (f64, f64), doc: &mut Document) -> Change {
        if let Some(Stroke { points, stylus, .. }) = &mut self.stroke {
            let world = view.screen_to_world(screen.0, screen.1);
            let last = points[points.len() - 1];
            let (dx, dy) = (world.0 - last[0], world.1 - last[1]);
            let min_step = 1.0 / view.px_per_world();
            if dx * dx + dy * dy < min_step * min_step {
                return Change::None;
            }
            points.push([world.0, world.1]);
            stylus.push(self.stylus);
            return Change::Scene;
        }
        match &mut self.nav {
            Some(Nav::Pan { anchor, .. }) => {
                Change::Camera(view.showing(*anchor, screen, view.camera.zoom))
            }
            Some(Nav::Zoom {
                origin,
                anchor,
                zoom,
                dragged,
            }) => {
                let (dx, dy) = (
                    (screen.0 - origin.0) / view.scale,
                    (screen.1 - origin.1) / view.scale,
                );
                if !*dragged && dx.hypot(dy) < CLICK_SLOP_PX {
                    return Change::None;
                }
                *dragged = true;
                let target = *zoom * ZOOM_STEP.powf(dx / ZOOM_DRAG_PX);
                Change::Camera(view.showing(*anchor, *origin, target))
            }
            None => self.drag_moved(view, screen, doc),
        }
    }

    fn drag_moved(&mut self, view: &View, screen: (f64, f64), doc: &mut Document) -> Change {
        let world = point(view.screen_to_world(screen.0, screen.1));
        let shift = self.shift;
        // Shift keeps the proportions, Ctrl holds the center; read before
        // the drag borrows the editor.
        let constraints = select::Resize {
            uniform: self.shift,
            from_center: self.ctrl,
        };
        match &mut self.drag {
            Some(Drag::Move {
                origin,
                screen: start,
                map,
                snapshot,
                moved,
                ..
            }) => {
                if !*moved {
                    let (dx, dy) = (
                        (screen.0 - start.0) / view.scale,
                        (screen.1 - start.1) / view.scale,
                    );
                    if dx.hypot(dy) < CLICK_SLOP_PX {
                        return Change::None;
                    }
                    *moved = true;
                }
                *map = Affine::translate(world[0] - origin[0], world[1] - origin[1]);
                apply(doc, snapshot, map);
                Change::Scene
            }
            Some(Drag::Resize {
                corner,
                frame,
                map,
                snapshot,
            }) => {
                *map = select::resize_map(frame, *corner, world, constraints);
                apply(doc, snapshot, map);
                Change::Scene
            }
            Some(Drag::Rotate {
                origin,
                frame,
                map,
                snapshot,
            }) => {
                let mut delta = select::sweep(frame, *origin, world);
                if shift {
                    delta = select::snap_turn(frame.angle, delta, ROTATE_SNAP_DEG.to_radians());
                }
                *map = select::rotate_map(frame, delta);
                apply(doc, snapshot, map);
                Change::Scene
            }
            Some(Drag::Marquee {
                origin,
                current,
                base,
            }) => {
                *current = screen;
                let a = point(view.screen_to_world(origin.0, origin.1));
                let mut selection = base.clone();
                for id in select::elements_in(doc, a, world) {
                    if !selection.contains(&id) {
                        selection.push(id);
                    }
                }
                self.selection = selection;
                Change::Selection
            }
            None => Change::None,
        }
    }

    /// Deletes the selected elements. Whatever drag was reshaping them
    /// goes with them.
    /// Drops a pasted bitmap on the board. `blob` names it in the store
    /// and `px` is its pixel size; it lands centered on `screen` — the
    /// pointer — or in the middle of the view when the pointer is
    /// elsewhere, at one world unit per pixel, shrunk to [`PASTE_ROOM`] of
    /// what the view shows when it would not fit. It arrives selected
    /// under the select tool, so the next drag moves it.
    pub fn paste_image(
        &mut self,
        doc: &mut Document,
        view: &View,
        screen: Option<(f64, f64)>,
        blob: String,
        px: (u32, u32),
    ) -> Change {
        // Also drops whatever was in progress, and the old selection with
        // it: what lands is what is selected.
        self.set_tool(Tool::Select, doc);
        let (sx, sy) = screen.unwrap_or((
            f64::from(view.viewport.w) / 2.0,
            f64::from(view.viewport.h) / 2.0,
        ));
        let (cx, cy) = view.screen_to_world(sx, sy);
        let showing = |px: u32| f64::from(px) / view.px_per_world() * PASTE_ROOM;
        let (w, h) = bitmap::fit_size(px, (showing(view.viewport.w), showing(view.viewport.h)));
        let id = new_id();
        // Pixels of its own: a paste never joins what is already on a
        // layer, so a stroke after it paints over the image, not beside it.
        // The pointer names the stack, as a press does: an image pasted
        // over a frame's area lands in it.
        let born = doc.frame_at([cx, cy]).map(str::to_owned);
        let layer = self.fresh_layer(doc, Kind::Raster, born.as_deref());
        doc.elements.push(Element::Image(Image {
            id: id.clone(),
            layer,
            x: cx - w / 2.0,
            y: cy - h / 2.0,
            w,
            h,
            rotation: 0.0,
            blob,
        }));
        self.selection = vec![id];
        Change::Scene
    }

    pub fn delete_selection(&mut self, doc: &mut Document) -> Change {
        if self.selection.is_empty() {
            return Change::None;
        }
        self.drag = None;
        // A layer is the object it holds, so a layer the deletion empties
        // goes with it. The last layer stays, as it does everywhere else.
        let emptied: Vec<String> = doc
            .elements
            .iter()
            .filter(|el| self.selection.iter().any(|id| id == el.id()))
            .map(|el| el.layer().to_owned())
            .collect();
        doc.elements
            .retain(|el| !self.selection.iter().any(|id| id == el.id()));
        for id in emptied {
            if !doc.elements.iter().any(|el| el.layer() == id)
                && let Some((stack, i)) = doc.locate(&id)
            {
                let stack = stack.map(str::to_owned);
                doc.remove_layer(stack.as_deref(), i);
            }
        }
        self.selection.clear();
        Change::Scene
    }

    /// What the stroke's points become on disk, once the jitter under
    /// [`FIT_TOLERANCE_PX`] is gone.
    ///
    /// A pencil line is a vector object, and its curves are what it is:
    /// they are what a resize scales and what a later editor would take
    /// hold of, so it is fitted with cubics ([`curve::fit`]).
    ///
    /// Paint is pixels, and it owes nothing but the path the hand took.
    /// Fitting reads a curve into the samples, and a fast stroke leaves
    /// too few, too far apart, for there to be one: the exit tangent
    /// comes off the last two of them and carries a handle as long as
    /// the span, so the ink turns the other way at the end of a stroke
    /// that only ever turned one way. So a brush keeps the hand's own
    /// line ([`curve::polyline`]) — which is also what it was already
    /// painting while the stroke was live, so the ink no longer shifts
    /// under the pointer at the release.
    fn laid_curves(points: &[[f64; 2]], kind: Kind, view: &View) -> Vec<Cubic> {
        let tolerance = FIT_TOLERANCE_PX / view.px_per_world();
        let hand = curve::simplify(points, tolerance);
        match kind {
            Kind::Vector => curve::fit(&hand, tolerance),
            // A stroke is a pencil's or a brush's. A frame layer holds
            // an area and never asks for curves, so it can only mean
            // the brush's answer here.
            Kind::Raster | Kind::Frame => curve::polyline(&hand),
        }
    }

    /// A button came up. The left button commits the stroke as a `path`
    /// in `ink` with the stroke's tip, on the active layer — or ends the
    /// selection drag; the button that started a gesture ends it, and a
    /// zoom click (no drag) zooms one unit in.
    ///
    /// Either way the jitter goes first, at [`FIT_TOLERANCE_PX`]: what
    /// sits under a pixel of the chord it is on is the digitiser's, not
    /// the hand's. What is left is the hand's line, and what becomes of
    /// it is the tool's business — see [`Editor::laid_curves`].
    pub fn release(
        &mut self,
        button: Button,
        view: &View,
        screen: (f64, f64),
        doc: &mut Document,
        ink: &str,
    ) -> Change {
        let _ = screen;
        if button == Button::Left
            && let Some(live) = self.stroke.take()
        {
            let pen = live.envelope();
            let Stroke {
                points, tip, born, ..
            } = live;
            // The tool that started the stroke: switching tools cancels
            // whatever was in progress, so this is still that one.
            let kind = match self.tool {
                Tool::Pencil => Kind::Vector,
                _ => Kind::Raster,
            };
            let curves = Self::laid_curves(&points, kind, view);
            let layer = self.ink_layer(doc, kind, born.as_deref());
            let laid = crate::doc::Stroke {
                curves,
                stroke: ink.to_owned(),
                width: tip.width,
                opacity: tip.opacity,
                hardness: tip.hardness,
                stamp: tip.stamp,
                pen,
            };
            if kind == Kind::Vector {
                doc.elements.push(Element::Path(Path {
                    id: new_id(),
                    layer,
                    curves: laid.curves,
                    stroke: laid.stroke,
                    width: laid.width,
                    opacity: laid.opacity,
                    hardness: laid.hardness,
                    rotation: 0.0,
                    stamp: laid.stamp,
                    pen: laid.pen,
                }));
                return Change::Scene;
            }
            // A raster layer holds one painting: the stroke joins the
            // paint already on it, and opens one only when there is none.
            let onto = doc
                .elements
                .iter()
                .position(|el| matches!(el, Element::Paint(p) if p.layer == layer));
            if let Some(i) = onto
                && let Element::Paint(p) = &mut doc.elements[i]
            {
                p.strokes.push(laid);
            } else {
                doc.elements.push(Element::Paint(Paint {
                    id: new_id(),
                    layer,
                    strokes: vec![laid],
                    rotation: 0.0,
                }));
            }
            return Change::Scene;
        }
        if button == Button::Left
            && let Some(drag) = self.drag.take()
        {
            return match drag {
                Drag::Move { moved: false, .. } | Drag::Marquee { .. } => Change::Selection,
                Drag::Move { .. } => {
                    self.rehome_moved(doc);
                    Change::Scene
                }
                Drag::Resize { .. } | Drag::Rotate { .. } => Change::Scene,
            };
        }
        match self.nav {
            Some(Nav::Pan { button: b, .. }) if b == button => {
                self.nav = None;
                Change::None
            }
            Some(Nav::Zoom {
                origin, dragged, ..
            }) if button == Button::Left => {
                self.nav = None;
                if dragged {
                    Change::None
                } else {
                    Change::Camera(view.zoomed_at(ZOOM_STEP, origin))
                }
            }
            _ => Change::None,
        }
    }

    /// Wheel or two-finger scroll by `delta` physical px (positive = content
    /// moves right/down). Pans, or zooms at `cursor` while Zoom is active;
    /// Shift turns a vertical wheel horizontal.
    pub fn scroll(
        &self,
        view: &View,
        cursor: (f64, f64),
        delta: (f64, f64),
        shift: bool,
    ) -> Camera {
        if self.active_tool() == Tool::Zoom {
            let units = delta.1 / (SCROLL_LINE_PX * view.scale);
            return view.zoomed_at(ZOOM_STEP.powf(units), cursor);
        }
        let (dx, dy) = if shift && delta.0 == 0.0 {
            (delta.1, 0.0)
        } else {
            delta
        };
        view.panned_by(dx, dy)
    }

    /// A trackpad gesture step: pinch zooms at `cursor` and follows the
    /// fingers, a three-finger swipe pans. `None` when nothing moves.
    pub fn gesture(&self, view: &View, cursor: (f64, f64), gesture: Gesture) -> Option<Camera> {
        match gesture {
            Gesture::Pinch { dx, dy, factor } => {
                let zoomed = View {
                    camera: view.zoomed_at(factor, cursor),
                    ..*view
                };
                Some(zoomed.panned_by(dx * view.scale, dy * view.scale))
            }
            Gesture::Swipe { fingers: 3, dx, dy } => {
                Some(view.panned_by(dx * view.scale, dy * view.scale))
            }
            Gesture::Swipe { .. } | Gesture::End => None,
        }
    }

    /// Drops the stroke, the gesture and the drag in progress, putting
    /// back whatever the drag had moved. True if there was one. The
    /// selection stays.
    pub fn cancel(&mut self, doc: &mut Document) -> bool {
        let had_stroke = self.stroke.take().is_some();
        let had_nav = self.nav.take().is_some();
        let had_drag = match self.drag.take() {
            Some(
                Drag::Move { snapshot, .. }
                | Drag::Resize { snapshot, .. }
                | Drag::Rotate { snapshot, .. },
            ) => {
                for (i, el) in snapshot {
                    if let Some(slot) = doc.elements.get_mut(i) {
                        *slot = el;
                    }
                }
                true
            }
            Some(Drag::Marquee { base, .. }) => {
                self.selection = base;
                true
            }
            None => false,
        };
        had_stroke || had_nav || had_drag
    }

    /// Esc: cancels what is in progress, or else clears the selection.
    /// True if anything changed.
    pub fn escape(&mut self, doc: &mut Document) -> bool {
        if self.cancel(doc) {
            return true;
        }
        let had_selection = !self.selection.is_empty();
        self.selection.clear();
        had_selection
    }
}

fn point((x, y): (f64, f64)) -> Point {
    [x, y]
}

/// Writes the snapshot back through `m`.
fn apply(doc: &mut Document, snapshot: &Snapshot, m: &Affine) {
    for (i, el) in snapshot {
        if let Some(slot) = doc.elements.get_mut(*i) {
            let mut moved = el.clone();
            select::transform(&mut moved, m);
            *slot = moved;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Brush, Face, Tip};
    use crate::doc::{Element, Layer, Pressure, Rect};
    use crate::geom::Corner;
    use crate::scene::Viewport;
    use crate::select::Handle;

    fn brush() -> Brush {
        Brush::default()
    }

    fn points(e: &Editor) -> Option<&[[f64; 2]]> {
        e.stroke().map(|s| s.points.as_slice())
    }

    fn path_of(doc: &Document, i: usize) -> &Path {
        let Element::Path(p) = &doc.elements[i] else {
            panic!("expected a path at {i}");
        };
        p
    }

    fn paint_of(doc: &Document, i: usize) -> &Paint {
        let Element::Paint(p) = &doc.elements[i] else {
            panic!("expected a paint at {i}");
        };
        p
    }

    /// [`board`] with `b` moved onto a second layer, `L2`, above `L1`.
    fn layered_board() -> Document {
        let mut doc = board();
        doc.layers.push(Layer {
            id: "L2".into(),
            name: "Layer 2".into(),
            visible: true,
            kind: Kind::Raster,
        });
        doc.elements[1].set_layer("L2");
        doc
    }

    /// A pen pressing `p` of the way down, held straight up.
    fn pressing(p: f64) -> Stylus {
        Stylus {
            pressure: p,
            ..Stylus::MOUSE
        }
    }

    #[test]
    fn the_layer_a_live_stroke_would_join_is_the_one_it_is_painted_on() {
        let mut doc = board();
        let e = tool(Tool::Brush);
        assert_eq!(
            e.live_layer(&doc),
            Some(doc.layers[0].id.as_str()),
            "a brush joins the raster layer under it"
        );
        // A pencil opens a layer of its own, and a brush over a vector
        // layer opens one above it: neither has anywhere to be painted
        // until it lands.
        let mut e_pencil = tool(Tool::Pencil);
        assert_eq!(e_pencil.live_layer(&doc), None);
        let _ = e_pencil.escape(&mut doc);
        doc.layers[0].kind = Kind::Vector;
        assert_eq!(e.live_layer(&doc), None);
    }

    #[test]
    fn a_stroke_records_what_the_stylus_said_at_every_sample() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        e.set_stylus(pressing(0.25));
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        e.set_stylus(pressing(1.0));
        let _ = e.moved(&v, (40.0, 10.0), &mut doc);
        let s = e.stroke().unwrap();
        assert_eq!(s.points.len(), s.stylus.len(), "one reading per sample");
        assert_eq!(s.stylus[0].pressure, 0.25, "the press took the pen as it was");
        assert_eq!(s.stylus[1].pressure, 1.0);
    }

    #[test]
    fn the_ink_does_not_jump_when_the_stroke_is_let_go_of() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        e.set_stylus(pressing(0.2));
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        e.set_stylus(pressing(0.9));
        let _ = e.moved(&v, (60.0, 10.0), &mut doc);
        let live = e.stroke().unwrap().envelope();
        let _ = e.release(Button::Left, &v, (60.0, 10.0), &mut doc, "#000");
        let laid = &paint_of(&doc, 0).strokes[0];
        assert_eq!(laid.pen, live, "the release writes down what was drawn");
    }

    #[test]
    fn a_stroke_keeps_only_the_readings_its_nib_can_use() {
        let mut doc = Document::new("t");
        let v = view();
        // A brush no pressure drives keeps nothing, however the pen was
        // held: there is nothing on the nib for it to move.
        let deaf = Brush {
            pressure: Pressure::NONE,
            ..brush()
        };
        let mut e = tool(Tool::Brush);
        e.set_stylus(pressing(0.3));
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &deaf.tip(Face::Round));
        let _ = e.moved(&v, (60.0, 10.0), &mut doc);
        assert!(
            e.stroke().unwrap().envelope().is_empty(),
            "a nib the pen cannot lean on"
        );

        // And a mouse leaves nothing behind on a brush that would have
        // read it: it pressed all the way from end to end.
        let mut e = tool(Tool::Brush);
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, (60.0, 10.0), &mut doc);
        assert!(e.stroke().unwrap().envelope().pressure.is_empty());
    }

    #[test]
    fn only_a_nib_the_stylus_turns_keeps_the_way_it_was_held() {
        let mut doc = Document::new("t");
        let v = view();
        let leaning = Stylus {
            tilt: (0.0, 1.0),
            ..Stylus::MOUSE
        };
        for (dynamics, kept) in [
            (Dynamics::None, false),
            (Dynamics::ToStroke, false),
            (Dynamics::Tilt, true),
            (Dynamics::TiltAndRoll, true),
        ] {
            let b = Brush { dynamics, ..brush() };
            let mut e = tool(Tool::Brush);
            e.set_stylus(leaning);
            let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &b.tip(Face::Round));
            let _ = e.moved(&v, (60.0, 10.0), &mut doc);
            let twist = e.stroke().unwrap().envelope().twist;
            assert_eq!(
                !twist.is_empty(),
                kept,
                "{dynamics:?} either reads the hand or does not"
            );
            if kept {
                assert!((twist[0] - 90.0).abs() < 1e-3, "leaning down the y axis");
            }
        }
    }

    #[test]
    fn a_pen_left_on_the_desk_does_not_make_the_mouse_draw_nothing() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        e.set_stylus(pressing(0.0));
        e.set_stylus(Stylus::MOUSE);
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, (60.0, 10.0), &mut doc);
        assert!(
            e.stroke().unwrap().envelope().pressure.is_empty(),
            "the mouse presses all the way"
        );
    }

    #[test]
    fn brush_records_the_tip_at_the_press_and_writes_it_into_the_paint() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        let brush = Brush {
            size: 20.0,
            opacity: 0.5,
            hardness: 0.25,
            ..Brush::default()
        };
        assert_eq!(
            e.press(Button::Left, &v, (1.0, 2.0), &mut doc, &brush.tip(Face::Round)),
            Change::Scene
        );
        assert_eq!(e.stroke().map(|s| s.tip.clone()), Some(brush.tip(Face::Round)));
        let _ = e.moved(&v, (9.0, 2.0), &mut doc);
        assert_eq!(
            e.release(Button::Left, &v, (9.0, 2.0), &mut doc, "#000"),
            Change::Scene
        );
        let p = paint_of(&doc, 0);
        let laid = &p.strokes[0];
        assert_eq!((laid.width, laid.opacity, laid.hardness), (20.0, 0.5, 0.25));
        assert_eq!(laid.stroke, "#000");
        assert_eq!(p.layer, doc.layers[0].id);
    }

    #[test]
    fn pencil_strokes_carry_the_pencil_tip() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = e.press(Button::Left, &v, (1.0, 2.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.stroke().map(|s| s.tip.clone()), Some(Tip::PENCIL));
        let _ = e.release(Button::Left, &v, (1.0, 2.0), &mut doc, "#000");
        let p = path_of(&doc, 0);
        assert_eq!((p.width, p.opacity, p.hardness), (PEN_WIDTH, 1.0, 1.0));
    }

    #[test]
    fn active_layer_defaults_to_the_top_and_follows_the_id() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        doc.add_layer(None, 0, Kind::Raster);
        doc.add_layer(None, 1, Kind::Raster);
        assert_eq!(e.active_layer(&doc), 2);
        assert_eq!(e.select_layer(&doc, 0), Change::Selection);
        assert_eq!(e.active_layer(&doc), 0);
        assert_eq!(e.select_layer(&doc, 9), Change::None);
        assert_eq!(e.active_layer(&doc), 0);
        // The layer goes away under the editor: back to the top.
        assert!(doc.remove_layer(None, 0));
        assert_eq!(e.active_layer(&doc), 1);
    }

    /// The samples a fast brush stroke leaves: nine of them over ~450
    /// world units, each leg turning the same way as the one before.
    /// Taken off a real stroke — the hand outran the sampler.
    fn fast_arc() -> Vec<(f64, f64)> {
        [
            [-292.0, -170.0],
            [-223.0, -193.0],
            [-148.0, -201.0],
            [-74.0, -193.0],
            [-5.0, -170.0],
            [56.0, -132.0],
            [104.0, -82.0],
            [136.0, -23.0],
            [151.0, 40.0],
        ]
        .iter()
        // The test view is at zoom 1 with the camera on the middle of
        // the viewport, so screen and world coincide.
        .map(|p| (p[0], p[1]))
        .collect()
    }

    /// Which way the line through `pts` turns at each of its corners.
    fn turns(pts: &[[f64; 2]]) -> Vec<f64> {
        pts.windows(3)
            .map(|w| {
                let (u, v) = (
                    [w[1][0] - w[0][0], w[1][1] - w[0][1]],
                    [w[2][0] - w[1][0], w[2][1] - w[1][1]],
                );
                u[0] * v[1] - u[1] * v[0]
            })
            .collect()
    }

    #[test]
    fn a_brush_stroke_is_laid_where_the_hand_put_it() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        let hand = fast_arc();
        let _ = e.press(Button::Left, &v, hand[0], &mut doc, &brush().tip(Face::Round));
        for &at in &hand[1..] {
            let _ = e.moved(&v, at, &mut doc);
        }
        let _ = e.release(Button::Left, &v, hand[hand.len() - 1], &mut doc, "#000");

        let laid = &paint_of(&doc, 0).strokes[0];
        let ends: Vec<[f64; 2]> = std::iter::once(laid.curves[0][0])
            .chain(laid.curves.iter().map(|c| c[3]))
            .collect();
        let world: Vec<[f64; 2]> = hand.iter().map(|&(x, y)| [x, y]).collect();
        assert_eq!(ends, world, "the ink goes through the samples themselves");
        // The hand turned one way the whole stroke; so does the ink.
        assert!(turns(&world).iter().all(|&t| t > 0.0), "the hand turned one way");
        assert!(
            turns(&ends).iter().all(|&t| t > 0.0),
            "and the ink turns with it, right to the end"
        );
    }

    #[test]
    fn a_pencil_line_is_still_fitted_with_curves() {
        // Paint is pixels and keeps the hand's line; a pencil line is a
        // vector object, and its curves are what it is.
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let hand = fast_arc();
        let _ = e.press(Button::Left, &v, hand[0], &mut doc, &brush().tip(Face::Round));
        for &at in &hand[1..] {
            let _ = e.moved(&v, at, &mut doc);
        }
        let _ = e.release(Button::Left, &v, hand[hand.len() - 1], &mut doc, "#000");
        let p = path_of(&doc, 0);
        assert!(
            p.curves.len() < hand.len() - 1,
            "fitted, not leg by leg: {} curves for {} legs",
            p.curves.len(),
            hand.len() - 1
        );
    }

    #[test]
    fn a_brush_stroke_still_drops_the_jitter_under_a_pixel() {
        // Keeping the hand's line is not keeping the digitiser's noise:
        // what sits under a pixel of its own chord goes, as it always did.
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        let _ = e.press(Button::Left, &v, (0.0, 10.0), &mut doc, &brush().tip(Face::Round));
        for i in 1..=10 {
            let wobble = if i % 2 == 0 { 0.3 } else { -0.3 };
            let y = if i == 10 { 10.0 } else { 10.0 + wobble };
            let _ = e.moved(&v, (f64::from(i) * 2.0, y), &mut doc);
        }
        let _ = e.release(Button::Left, &v, (20.0, 10.0), &mut doc, "#000");
        let laid = &paint_of(&doc, 0).strokes[0];
        assert_eq!(laid.curves.len(), 1, "one straight leg left: {:?}", laid.curves);
        assert_eq!(laid.curves[0][0], [0.0, 10.0]);
        assert_eq!(laid.curves[0][3], [20.0, 10.0]);
    }

    #[test]
    fn brush_ink_lands_on_the_active_layer() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(e.active_layer(&doc), 1);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(paint_of(&doc, 0).layer, doc.layers[1].id);
        let _ = e.select_layer(&doc, 0);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 5.0), (9.0, 5.0));
        assert_eq!(paint_of(&doc, 1).layer, doc.layers[0].id, "another layer, another paint");
    }

    #[test]
    fn brush_strokes_join_the_paint_on_the_raster_layer() {
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        let sheet = doc.layers[0].id.clone();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        let _ = drag(&mut e, &v, &mut doc, (1.0, 5.0), (9.0, 5.0));
        assert_eq!(doc.layers.len(), 1, "pixels join what is already there");
        assert_eq!(doc.elements.len(), 1, "and so does the object they make");
        let p = paint_of(&doc, 0);
        assert_eq!(p.strokes.len(), 2, "one paint, two strokes");
        assert_eq!(p.layer, sheet);
    }

    #[test]
    fn a_stroke_laid_with_another_tip_joins_the_paint_all_the_same() {
        // The ink belongs to the stroke, not to the object, so changing
        // the brush mid-painting does not start a second one.
        let mut e = tool(Tool::Brush);
        let mut doc = Document::new("t");
        let v = view();
        let fine = Brush {
            size: 4.0,
            opacity: 1.0,
            hardness: 1.0,
            ..Brush::default()
        };
        let _ = e.press(Button::Left, &v, (1.0, 2.0), &mut doc, &fine.tip(Face::Round));
        let _ = e.release(Button::Left, &v, (9.0, 2.0), &mut doc, "#000");
        let soft = Brush {
            size: 20.0,
            opacity: 0.5,
            hardness: 0.25,
            ..Brush::default()
        };
        let _ = e.press(Button::Left, &v, (1.0, 5.0), &mut doc, &soft.tip(Face::Round));
        let _ = e.release(Button::Left, &v, (9.0, 5.0), &mut doc, "#f00");
        assert_eq!(doc.elements.len(), 1);
        let p = paint_of(&doc, 0);
        assert_eq!(p.strokes.len(), 2);
        assert_eq!((p.strokes[0].width, p.strokes[0].stroke.as_str()), (4.0, "#000"));
        assert_eq!((p.strokes[1].width, p.strokes[1].opacity), (20.0, 0.5));
        assert_eq!(p.strokes[1].stroke, "#f00");
    }

    #[test]
    fn a_pencil_stroke_opens_a_vector_layer_of_its_own() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let first = doc.layers[0].id.clone();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(doc.layers.len(), 2, "the object brought its own layer");
        assert_eq!(doc.layers[0].id, first, "which went in above the active one");
        assert_eq!(doc.layers[1].kind, Kind::Vector);
        assert_eq!(path_of(&doc, 0).layer, doc.layers[1].id);
        assert_eq!(e.active_layer(&doc), 1, "and is now the active one");
        // The next object gets one of its own too.
        let _ = drag(&mut e, &v, &mut doc, (1.0, 5.0), (9.0, 5.0));
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(path_of(&doc, 1).layer, doc.layers[2].id);
    }

    #[test]
    fn a_brush_stroke_over_a_vector_layer_opens_a_raster_one() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(doc.layers[1].kind, Kind::Vector, "the pencil left one active");
        e.set_tool(Tool::Brush, &mut doc);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 5.0), (9.0, 5.0));
        assert_eq!(doc.layers.len(), 3, "pixels cannot go on a vector layer");
        assert_eq!(doc.layers[2].kind, Kind::Raster);
        assert_eq!(paint_of(&doc, 1).layer, doc.layers[2].id);
        assert_eq!(e.active_layer(&doc), 2);
        // The one after it finds that paint and joins it.
        let _ = drag(&mut e, &v, &mut doc, (1.0, 8.0), (9.0, 8.0));
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(doc.elements.len(), 2);
        assert_eq!(paint_of(&doc, 1).strokes.len(), 2);
    }

    #[test]
    fn deleting_an_object_takes_its_layer_with_it() {
        // A layer is the object it holds: what deletes one deletes both.
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.selection(), ids(&["b"]));
        assert_eq!(e.delete_selection(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 1, "L2 held b and nothing else");
        assert_eq!(doc.layers[0].id, "L1");
        // The last layer stays, even once what it held is gone.
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        let _ = e.delete_selection(&mut doc);
        assert!(doc.elements.is_empty());
        assert_eq!(doc.layers.len(), 1);
    }

    #[test]
    fn a_layer_with_something_else_on_it_stays() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let v = view();
        // The paste opens a raster layer and leaves it active; the brush
        // paints onto that same layer, beside the image.
        let _ = e.paste_image(&mut doc, &v, None, BLOB.into(), (10, 10));
        e.set_tool(Tool::Brush, &mut doc);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.elements.len(), 2);
        e.set_tool(Tool::Select, &mut doc);
        let _ = click(&mut e, &v, &mut doc, (5.0, 2.0));
        assert_eq!(e.selection().len(), 1, "the paint, away from the image");
        let _ = e.delete_selection(&mut doc);
        assert_eq!(doc.layers.len(), 2, "the image is still on it");
        assert_eq!(doc.elements.len(), 1);
    }

    #[test]
    fn a_pasted_image_always_opens_its_own_raster_layer() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let v = view();
        let _ = e.paste_image(&mut doc, &v, None, BLOB.into(), (10, 10));
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.layers[1].kind, Kind::Raster);
        assert_eq!(doc.elements[0].layer(), doc.layers[1].id);
        assert_eq!(e.active_layer(&doc), 1, "so a brush stroke lands over it");
        // A second one does not join the first.
        let _ = e.paste_image(&mut doc, &v, None, BLOB.into(), (10, 10));
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(doc.elements[1].layer(), doc.layers[2].id);
    }

    #[test]
    fn the_layer_the_panel_adds_is_a_raster_one() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers[1].kind, Kind::Raster, "a blank sheet to paint on");
    }

    #[test]
    fn picking_an_element_activates_its_layer() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        assert_eq!(e.active_layer(&doc), 1);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(e.active_layer(&doc), 0, "a is on L1");
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.active_layer(&doc), 1, "b is on L2");
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(e.active_layer(&doc), 0, "adding to the selection too");
        e.hold_shift(false);
        // Empty canvas changes nothing.
        let _ = click(&mut e, &v, &mut doc, (50.0, 5.0));
        assert_eq!(e.active_layer(&doc), 0);
    }

    #[test]
    fn add_layer_goes_in_above_the_active_one() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let _ = e.select_layer(&doc, 0);
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(e.active_layer(&doc), 1);
        assert_eq!(doc.layers[1].name, "Layer 3");
        assert_eq!(doc.layers[2].id, "L2");
    }

    #[test]
    fn remove_layer_drops_its_elements_from_the_selection_and_activates_the_neighbour() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.selection(), ids(&["b"]));
        assert_eq!(e.active_layer(&doc), 1);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.elements.len(), 1, "b went with L2");
        assert!(e.selection().is_empty());
        assert_eq!(e.active_layer(&doc), 0);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene, "a takes its turn");
        assert_eq!(doc.layers.len(), 1, "the last layer stays");
        assert!(doc.elements.is_empty(), "though what it held does not");
        assert_eq!(e.remove_layer(&mut doc), Change::None, "nothing left to take");
    }

    #[test]
    fn binning_the_last_layer_empties_it() {
        // A layer is the object it holds, so the trash always takes the
        // object — even where the layer itself has to stay.
        let mut e = Editor::new();
        let mut doc = board();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 1, "a board keeps its last layer");
        assert!(doc.elements.is_empty(), "but not what was on it");
        assert_eq!(e.remove_layer(&mut doc), Change::None, "nothing left to take");
    }

    #[test]
    fn remove_layer_activates_the_one_that_was_above() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let _ = e.add_layer(&mut doc);
        let _ = e.add_layer(&mut doc);
        let top = doc.layers[2].id.clone();
        let _ = e.select_layer(&doc, 1);
        let _ = e.remove_layer(&mut doc);
        assert_eq!(e.active_layer(&doc), 1);
        assert_eq!(doc.layers[1].id, top);
    }

    #[test]
    fn toggle_layer_deselects_what_it_hides() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        e.hold_shift(false);
        assert_eq!(e.selection(), ids(&["a", "b"]));
        assert_eq!(e.toggle_layer(&mut doc, 1), Change::Scene);
        assert!(!doc.layers[1].visible);
        assert_eq!(e.selection(), ids(&["a"]), "b is hidden with its layer");
        assert_eq!(e.toggle_layer(&mut doc, 1), Change::Scene);
        assert!(doc.layers[1].visible);
        assert_eq!(e.selection(), ids(&["a"]));
        assert_eq!(e.toggle_layer(&mut doc, 7), Change::None);
    }

    #[test]
    fn a_hidden_layer_cannot_be_picked() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = e.toggle_layer(&mut doc, 1);
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert!(e.selection().is_empty());
    }

    #[test]
    fn move_layer_swaps_and_keeps_the_active_id() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let _ = e.select_layer(&doc, 0);
        assert_eq!(e.move_layer(&mut doc, true), Change::Scene);
        assert_eq!(e.active_layer(&doc), 1);
        assert_eq!(doc.layers[1].id, "L1");
        assert_eq!(e.move_layer(&mut doc, true), Change::None, "already on top");
        assert_eq!(e.move_layer(&mut doc, false), Change::Scene);
        assert_eq!(e.active_layer(&doc), 0);
        assert_eq!(e.move_layer(&mut doc, false), Change::None);
    }

    #[test]
    fn move_layer_to_drops_the_active_layer_at_an_index() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        doc.add_layer(None, 1, Kind::Raster);
        let top = doc.layers[2].id.clone();
        let _ = e.select_layer(&doc, 2);
        assert_eq!(e.move_layer_to(&mut doc, 0), Change::Scene);
        assert_eq!(e.active_layer(&doc), 0, "it stays active where it landed");
        assert_eq!(doc.layers[0].id, top);
        assert_eq!(doc.layers[1].id, "L1", "the others keep their order");
        assert_eq!(doc.layers[2].id, "L2");
        assert_eq!(e.move_layer_to(&mut doc, 0), Change::None, "already there");
        assert_eq!(e.move_layer_to(&mut doc, 9), Change::None, "no such place");
    }

    /// 100×100 viewport looking at (50, 50) at zoom 1: screen px == world.
    fn view() -> View {
        View {
            camera: Camera {
                x: 50.0,
                y: 50.0,
                zoom: 1.0,
            },
            viewport: Viewport { w: 100, h: 100 },
            scale: 1.0,
        }
    }

    fn with(camera: Camera) -> View {
        View { camera, ..view() }
    }

    fn cam(x: f64, y: f64, zoom: f64) -> Camera {
        Camera { x, y, zoom }
    }

    fn tool(t: Tool) -> Editor {
        let mut e = Editor::new();
        set_tool(&mut e, t);
        e
    }

    fn set_tool(e: &mut Editor, t: Tool) {
        e.set_tool(t, &mut Document::new("t"));
    }

    fn press(e: &mut Editor, button: Button, v: &View, at: (f64, f64)) -> Change {
        e.press(button, v, at, &mut Document::new("t"), &brush().tip(Face::Round))
    }

    fn moved(e: &mut Editor, v: &View, at: (f64, f64)) -> Change {
        e.moved(v, at, &mut Document::new("t"))
    }

    fn cancel(e: &mut Editor) -> bool {
        e.cancel(&mut Document::new("t"))
    }

    fn rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
        Element::Rect(Rect {
            id: id.into(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation: 0.0,
            stroke: Some("#000".into()),
            fill: None,
            text: None,
        })
    }

    fn rect_of(doc: &Document, id: &str) -> Rect {
        let Some(Element::Rect(r)) = doc.elements.iter().find(|e| e.id() == id) else {
            panic!("no rect {id}");
        };
        r.clone()
    }

    /// Two rects: `a` at (10, 10) 20×10 and `b` at (60, 60) 20×20.
    fn board() -> Document {
        let mut doc = Document::new("t");
        doc.elements = vec![
            rect("a", 10.0, 10.0, 20.0, 10.0),
            rect("b", 60.0, 60.0, 20.0, 20.0),
        ];
        // A fixed layer id, so two boards compare equal element by element.
        doc.layers[0].id = "L1".into();
        for el in &mut doc.elements {
            el.set_layer("L1");
        }
        doc
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Press, move and release the left button on the canvas.
    fn drag(
        e: &mut Editor,
        v: &View,
        doc: &mut Document,
        from: (f64, f64),
        to: (f64, f64),
    ) -> Change {
        let _ = e.press(Button::Left, v, from, doc, &brush().tip(Face::Round));
        let _ = e.moved(v, to, doc);
        e.release(Button::Left, v, to, doc, "#000")
    }

    fn click(e: &mut Editor, v: &View, doc: &mut Document, at: (f64, f64)) -> Change {
        drag(e, v, doc, at, at)
    }

    #[test]
    fn select_click_picks_the_element_under_the_pointer() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        assert_eq!(
            e.press(Button::Left, &v, (70.0, 70.0), &mut doc, &brush().tip(Face::Round)),
            Change::Selection
        );
        assert_eq!(
            e.release(Button::Left, &v, (70.0, 70.0), &mut doc, "#000"),
            Change::Selection
        );
        assert_eq!(e.selection(), ids(&["b"]));
        assert_eq!(click(&mut e, &v, &mut doc, (15.0, 15.0)), Change::Selection);
        assert_eq!(e.selection(), ids(&["a"]));
        assert_eq!(
            doc.elements,
            board().elements,
            "clicking changes nothing in the document"
        );
    }

    #[test]
    fn click_on_empty_canvas_clears_the_selection() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(
            e.press(Button::Left, &v, (50.0, 5.0), &mut doc, &brush().tip(Face::Round)),
            Change::Selection
        );
        assert!(e.selection().is_empty(), "cleared on press");
        assert_eq!(
            e.release(Button::Left, &v, (50.0, 5.0), &mut doc, "#000"),
            Change::Selection
        );
        assert!(e.selection().is_empty());
    }

    #[test]
    fn shift_click_toggles_membership() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.selection(), ids(&["a", "b"]));
        // Away from the group frame's top-left handle, which sits on (10, 10).
        let _ = click(&mut e, &v, &mut doc, (25.0, 15.0));
        assert_eq!(e.selection(), ids(&["b"]));
        // Shift-click on empty canvas keeps what is selected.
        let _ = click(&mut e, &v, &mut doc, (50.0, 5.0));
        assert_eq!(e.selection(), ids(&["b"]));
    }

    #[test]
    fn dragging_a_selected_element_moves_the_whole_selection() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        e.hold_shift(false);
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.moved(&v, (80.0, 75.0), &mut doc), Change::Scene);
        assert_eq!((rect_of(&doc, "a").x, rect_of(&doc, "a").y), (20.0, 15.0));
        assert_eq!((rect_of(&doc, "b").x, rect_of(&doc, "b").y), (70.0, 65.0));
        // Each step is measured from the press, not accumulated.
        let _ = e.moved(&v, (75.0, 70.0), &mut doc);
        assert_eq!((rect_of(&doc, "a").x, rect_of(&doc, "a").y), (15.0, 10.0));
        assert_eq!(
            e.release(Button::Left, &v, (75.0, 70.0), &mut doc, "#000"),
            Change::Scene
        );
        assert_eq!(e.selection(), ids(&["a", "b"]));
    }

    #[test]
    fn dragging_an_unselected_element_selects_and_moves_it_alone() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(
            drag(&mut e, &v, &mut doc, (70.0, 70.0), (60.0, 60.0)),
            Change::Scene
        );
        assert_eq!(e.selection(), ids(&["b"]));
        assert_eq!((rect_of(&doc, "b").x, rect_of(&doc, "b").y), (50.0, 50.0));
        assert_eq!(rect_of(&doc, "a"), rect_of(&board(), "a"));
    }

    #[test]
    fn a_move_within_the_click_slop_is_a_click() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.moved(&v, (71.0, 71.0), &mut doc), Change::None);
        assert_eq!(
            e.release(Button::Left, &v, (71.0, 71.0), &mut doc, "#000"),
            Change::Selection
        );
        assert_eq!(doc.elements, board().elements);
    }

    #[test]
    fn marquee_selects_what_it_overlaps_and_shift_adds() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        assert_eq!(
            e.press(Button::Left, &v, (5.0, 5.0), &mut doc, &brush().tip(Face::Round)),
            Change::Selection
        );
        assert_eq!(e.moved(&v, (12.0, 12.0), &mut doc), Change::Selection);
        assert_eq!(e.marquee(), Some(((5.0, 5.0), (12.0, 12.0))));
        assert_eq!(e.selection(), ids(&["a"]));
        let _ = e.moved(&v, (65.0, 65.0), &mut doc);
        assert_eq!(e.selection(), ids(&["a", "b"]));
        let _ = e.moved(&v, (40.0, 40.0), &mut doc);
        assert_eq!(e.selection(), ids(&["a"]), "shrinking drops what it left");
        assert_eq!(
            e.release(Button::Left, &v, (40.0, 40.0), &mut doc, "#000"),
            Change::Selection
        );
        assert_eq!(e.marquee(), None);
        assert_eq!(e.selection(), ids(&["a"]));
        // With Shift the box adds to the selection instead of replacing it.
        e.hold_shift(true);
        let _ = drag(&mut e, &v, &mut doc, (55.0, 55.0), (65.0, 65.0));
        assert_eq!(e.selection(), ids(&["a", "b"]));
        assert_eq!(doc.elements, board().elements);
    }

    #[test]
    fn resize_handle_scales_about_the_opposite_corner() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        // Bottom-right corner of `a` is (30, 20); drag it to (50, 30).
        let _ = e.press(Button::Left, &v, (30.0, 20.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.moved(&v, (50.0, 30.0), &mut doc), Change::Scene);
        let r = rect_of(&doc, "a");
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 10.0, 40.0, 20.0));
        assert_eq!(
            e.release(Button::Left, &v, (50.0, 30.0), &mut doc, "#000"),
            Change::Scene
        );
        assert_eq!(e.selection(), ids(&["a"]));
    }

    #[test]
    fn shift_resizes_proportionally() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        e.hold_shift(true);
        // `a` is 20 x 10 at (10, 10); dragging its bottom-right corner
        // (30, 20) to (50, 25) asks 2x across and 1.5x down. The wider
        // one takes both, so 20 x 10 becomes 40 x 20.
        let _ = e.press(Button::Left, &v, (30.0, 20.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.moved(&v, (50.0, 25.0), &mut doc), Change::Scene);
        let r = rect_of(&doc, "a");
        assert_eq!((r.x, r.y, r.w, r.h), (10.0, 10.0, 40.0, 20.0));
    }

    #[test]
    fn ctrl_resizes_from_the_center() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        // `a` is centered on (20, 15). Dragging its bottom-right corner
        // (30, 20) to (40, 22.5) grows it both ways out of that center.
        let _ = e.press(Button::Left, &v, (30.0, 20.0), &mut doc, &brush().tip(Face::Round));
        e.hold_ctrl(true);
        assert_eq!(e.moved(&v, (40.0, 22.5), &mut doc), Change::Scene);
        let r = rect_of(&doc, "a");
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 7.5, 40.0, 15.0));
    }

    #[test]
    fn shift_and_ctrl_resize_proportionally_from_the_center() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        e.hold_shift(true);
        e.hold_ctrl(true);
        let _ = e.press(Button::Left, &v, (30.0, 20.0), &mut doc, &brush().tip(Face::Round));
        // 2x across, 1.5x down about (20, 15): the 2x takes both axes.
        assert_eq!(e.moved(&v, (40.0, 22.5), &mut doc), Change::Scene);
        let r = rect_of(&doc, "a");
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 5.0, 40.0, 20.0));
    }

    #[test]
    fn ctrl_is_zoom_except_over_a_resize_handle() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        e.hold_ctrl(true);
        assert_eq!(e.pointer_tool(&doc, &v, (30.0, 20.0)), Tool::Select);
        assert_eq!(e.pointer_tool(&doc, &v, (50.0, 50.0)), Tool::Zoom);
        // Ctrl means nothing to a turn, so the rings still zoom.
        let d = f64::from(crate::select::ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        assert_eq!(e.pointer_tool(&doc, &v, (30.0 + d, 20.0 + d)), Tool::Zoom);
        // The Z tool is not the Ctrl override: it zooms anywhere.
        set_tool(&mut e, Tool::Zoom);
        e.hold_ctrl(false);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(e.pointer_tool(&doc, &v, (30.0, 20.0)), Tool::Zoom);
    }

    #[test]
    fn ctrl_on_a_resize_handle_resizes_instead_of_zooming() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        e.hold_ctrl(true);
        assert_eq!(
            e.press(Button::Left, &v, (30.0, 20.0), &mut doc, &brush().tip(Face::Round)),
            Change::None
        );
        // A zoom would answer with a camera; the handle reshapes instead.
        assert_eq!(e.moved(&v, (40.0, 22.5), &mut doc), Change::Scene);
        assert_eq!(rect_of(&doc, "a").w, 40.0);
    }

    #[test]
    fn rotate_handle_turns_about_the_center() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        // `b` is centered on (70, 70); its top-right rotation handle sits
        // past (80, 60) along the diagonal. Sweeping it a quarter turn
        // clockwise puts it past (80, 80).
        let d = f64::from(crate::select::ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        let _ = e.press(Button::Left, &v, (80.0 + d, 60.0 - d), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.moved(&v, (80.0 + d, 80.0 + d), &mut doc), Change::Scene);
        let r = rect_of(&doc, "b");
        assert!((r.rotation - 90.0).abs() < 1e-9, "{}", r.rotation);
        assert!((r.x - 60.0).abs() < 1e-9 && (r.y - 60.0).abs() < 1e-9);
        assert_eq!(
            e.release(Button::Left, &v, (80.0 + d, 80.0 + d), &mut doc, "#000"),
            Change::Scene
        );
    }

    /// Screen point past `b`'s top-right corner at `angle` degrees from its
    /// center (70, 70), on the rotation handle's radius.
    fn around_b(angle: f64) -> (f64, f64) {
        let d = f64::from(crate::select::ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        let radius = (10.0 + d) * std::f64::consts::SQRT_2;
        let (s, c) = angle.to_radians().sin_cos();
        (70.0 + radius * c, 70.0 + radius * s)
    }

    #[test]
    fn shift_snaps_the_turn_to_15_degrees_from_the_creation_state() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        // The top-right rotation handle sits at -45°; sweep to -8° (37°).
        let _ = e.press(Button::Left, &v, around_b(-45.0), &mut doc, &brush().tip(Face::Round));
        e.hold_shift(true);
        let _ = e.moved(&v, around_b(-8.0), &mut doc);
        assert_eq!(rect_of(&doc, "b").rotation, 30.0);
        // Without Shift the pointer is followed exactly.
        e.hold_shift(false);
        let _ = e.moved(&v, around_b(-8.0), &mut doc);
        assert!((rect_of(&doc, "b").rotation - 37.0).abs() < 1e-9);
        let _ = e.release(Button::Left, &v, around_b(-8.0), &mut doc, "#000");
        // Already at 37°, a 5° sweep with Shift lands on 45°, not on 42°:
        // the grid is anchored on the creation state.
        let _ = e.press(Button::Left, &v, around_b(-8.0), &mut doc, &brush().tip(Face::Round));
        e.hold_shift(true);
        let _ = e.moved(&v, around_b(-3.0), &mut doc);
        assert_eq!(rect_of(&doc, "b").rotation, 45.0);
    }

    #[test]
    fn selection_frame_follows_the_drag_live() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (25.0, 15.0));
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        e.hold_shift(false);
        // The group frame spans (10, 10)–(80, 80): its bottom-right rotation
        // handle sits past (80, 80) on the diagonal.
        let d = f64::from(crate::select::ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        let _ = e.press(Button::Left, &v, (80.0 + d, 80.0 + d), &mut doc, &brush().tip(Face::Round));
        // A quarter turn: the handle goes from 45° to 135° around (45, 45).
        let r = (35.0 + d) * std::f64::consts::SQRT_2;
        let to = (45.0 - r * 0.5f64.sqrt(), 45.0 + r * 0.5f64.sqrt());
        let _ = e.moved(&v, to, &mut doc);
        let live = e.selection_frame(&doc).unwrap();
        assert!(
            (live.angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
            "{live:?}"
        );
        assert!((live.center[0] - 45.0).abs() < 1e-9 && (live.center[1] - 45.0).abs() < 1e-9);
        assert!((live.half[0] - 35.0).abs() < 1e-9 && (live.half[1] - 35.0).abs() < 1e-9);
        // Once released, a group is wrapped by the unturned box again.
        let _ = e.release(Button::Left, &v, to, &mut doc, "#000");
        assert_eq!(e.selection_frame(&doc).unwrap().angle, 0.0);
    }

    #[test]
    fn escape_cancels_the_drag_then_clears_the_selection() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, (90.0, 90.0), &mut doc);
        assert_ne!(doc.elements, board().elements);
        assert!(e.escape(&mut doc));
        assert_eq!(doc.elements, board().elements);
        assert_eq!(
            e.selection(),
            ids(&["b"]),
            "cancelling a drag keeps the selection"
        );
        assert_eq!(
            e.release(Button::Left, &v, (90.0, 90.0), &mut doc, "#000"),
            Change::None
        );
        // With nothing in progress, Esc drops the selection instead.
        assert!(e.escape(&mut doc));
        assert!(e.selection().is_empty());
        assert!(!e.escape(&mut doc));
        // Losing focus cancels the drag but never the selection.
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert!(!e.cancel(&mut doc));
        assert_eq!(e.selection(), ids(&["b"]));
    }

    #[test]
    fn delete_removes_the_selection() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        assert_eq!(e.delete_selection(&mut doc), Change::None);
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.delete_selection(&mut doc), Change::Scene);
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].id(), "a");
        assert!(e.selection().is_empty());
    }

    #[test]
    fn switching_tools_drops_the_selection() {
        let mut e = Editor::new();
        let mut doc = board();
        let _ = click(&mut e, &view(), &mut doc, (70.0, 70.0));
        e.set_tool(Tool::Pencil, &mut doc);
        assert!(e.selection().is_empty());
    }

    #[test]
    fn other_tools_do_not_select() {
        let mut e = pencil();
        let mut doc = board();
        let v = view();
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc, &brush().tip(Face::Round));
        assert!(e.is_drawing());
        assert!(e.selection().is_empty());
    }

    #[test]
    fn hover_names_the_handle_under_the_pointer() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        assert_eq!(e.hover(&doc, &v, (30.0, 20.0)), None);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(
            e.hover(&doc, &v, (30.0, 20.0)),
            Some(Handle::Resize(Corner::BottomRight))
        );
        assert_eq!(e.hover(&doc, &v, (50.0, 50.0)), None);
    }

    fn pencil() -> Editor {
        tool(Tool::Pencil)
    }

    fn release(e: &mut Editor, button: Button, v: &View, at: (f64, f64)) -> Change {
        let mut doc = Document::new("t");
        e.release(button, v, at, &mut doc, "#000")
    }

    fn assert_close(a: (f64, f64), b: (f64, f64)) {
        assert!(
            (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn starts_on_select_with_nothing_in_progress() {
        let e = Editor::new();
        assert_eq!(e.tool(), Tool::Select);
        assert_eq!(e.active_tool(), Tool::Select);
        assert!(!e.is_drawing());
        assert!(!e.is_panning());
        assert!(e.stroke().is_none());
    }

    #[test]
    fn hotkeys_map_to_tools_case_insensitively() {
        assert_eq!(Tool::from_hotkey('p'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('P'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('v'), Some(Tool::Select));
        assert_eq!(Tool::from_hotkey('h'), Some(Tool::Hand));
        assert_eq!(Tool::from_hotkey('z'), Some(Tool::Zoom));
        assert_eq!(Tool::from_hotkey('b'), Some(Tool::Brush));
        assert_eq!(Tool::from_hotkey('x'), None);
        for t in Tool::ALL {
            assert_eq!(Tool::from_hotkey(t.hotkey()), Some(t));
        }
    }

    #[test]
    fn dock_order_is_select_hand_pencil_brush_zoom() {
        assert_eq!(
            Tool::ALL,
            [Tool::Select, Tool::Hand, Tool::Pencil, Tool::Brush, Tool::Zoom]
        );
    }

    #[test]
    fn held_keys_override_the_tool_ctrl_over_space() {
        let mut e = Editor::new();
        e.hold_space(true);
        assert_eq!(e.active_tool(), Tool::Hand);
        assert_eq!(e.tool(), Tool::Select, "the dock keeps showing the tool");
        e.hold_ctrl(true);
        assert_eq!(e.active_tool(), Tool::Zoom);
        e.hold_space(false);
        assert_eq!(e.active_tool(), Tool::Zoom);
        e.hold_ctrl(false);
        assert_eq!(e.active_tool(), Tool::Select);
    }

    #[test]
    fn pencil_records_a_stroke_and_commits_it_on_release() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        assert_eq!(press(&mut e, Button::Left, &v, (1.0, 2.0)), Change::Scene);
        assert!(e.is_drawing());
        assert_eq!(moved(&mut e, &v, (4.0, 2.0)), Change::Scene);
        assert_eq!(points(&e), Some(&[[1.0, 2.0], [4.0, 2.0]][..]));
        assert_eq!(
            e.release(Button::Left, &v, (4.0, 2.0), &mut doc, "#1f1f1f"),
            Change::Scene
        );
        assert!(!e.is_drawing());
        assert_eq!(doc.elements.len(), 1);
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        assert_eq!(p.id.len(), 26, "ULID id");
        assert_eq!(
            p.curves,
            vec![[[1.0, 2.0], [2.0, 2.0], [3.0, 2.0], [4.0, 2.0]]]
        );
        assert_eq!(p.stroke, "#1f1f1f");
        assert_eq!(p.width, PEN_WIDTH);
    }

    #[test]
    fn moved_drops_points_closer_than_one_physical_pixel() {
        let mut e = pencil();
        // Zoom 2: one physical px is half a world unit.
        let v = with(cam(0.0, 0.0, 2.0));
        assert_eq!(press(&mut e, Button::Left, &v, (50.0, 50.0)), Change::Scene);
        assert_eq!(moved(&mut e, &v, (50.5, 50.0)), Change::None);
        assert_eq!(moved(&mut e, &v, (51.25, 50.0)), Change::Scene);
        assert_eq!(points(&e), Some(&[[0.0, 0.0], [0.625, 0.0]][..]));
    }

    #[test]
    fn moved_without_a_press_does_nothing() {
        let mut e = pencil();
        assert_eq!(moved(&mut e, &view(), (1.0, 1.0)), Change::None);
        assert!(!e.is_drawing());
    }

    #[test]
    fn a_click_without_motion_commits_a_dot() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = press(&mut e, Button::Left, &v, (3.0, 4.0));
        assert_eq!(
            e.release(Button::Left, &v, (3.0, 4.0), &mut doc, "#000"),
            Change::Scene
        );
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        assert_eq!(p.curves, vec![[[3.0, 4.0]; 4]]);
    }

    #[test]
    fn release_simplifies_jitter_away_before_fitting() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = press(&mut e, Button::Left, &v, (0.0, 10.0));
        for i in 1..=10 {
            let wobble = if i % 2 == 0 { 0.3 } else { -0.3 };
            let _ = moved(
                &mut e,
                &v,
                (
                    f64::from(i) * 2.0,
                    if i == 10 { 10.0 } else { 10.0 + wobble },
                ),
            );
        }
        assert_eq!(points(&e).map(<[_]>::len), Some(11));
        assert_eq!(
            e.release(Button::Left, &v, (20.0, 10.0), &mut doc, "#000"),
            Change::Scene
        );
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        // Wobble under the fit tolerance is noise: one straight cubic remains.
        assert_eq!(p.curves.len(), 1, "{:?}", p.curves);
        let c = p.curves[0];
        assert_eq!(c[0], [0.0, 10.0]);
        assert_eq!(c[3], [20.0, 10.0]);
        assert_eq!(c[1][1], 10.0);
        assert_eq!(c[2][1], 10.0);
    }

    #[test]
    fn cancel_discards_the_stroke() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = press(&mut e, Button::Left, &v, (0.0, 0.0));
        let _ = moved(&mut e, &v, (9.0, 9.0));
        assert!(cancel(&mut e));
        assert!(!e.is_drawing());
        assert!(!cancel(&mut e), "nothing left to cancel");
        assert_eq!(
            e.release(Button::Left, &v, (9.0, 9.0), &mut doc, "#000"),
            Change::None
        );
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn switching_tools_cancels_the_stroke() {
        let mut e = pencil();
        let _ = press(&mut e, Button::Left, &view(), (0.0, 0.0));
        set_tool(&mut e, Tool::Select);
        assert_eq!(e.tool(), Tool::Select);
        assert!(!e.is_drawing());
    }

    #[test]
    fn hand_drag_keeps_the_grabbed_point_under_the_pointer() {
        let mut e = tool(Tool::Hand);
        let v = view();
        assert_eq!(press(&mut e, Button::Left, &v, (20.0, 20.0)), Change::None);
        assert!(e.is_panning());
        // 10px right, 5px down: the camera goes the other way.
        let Change::Camera(c) = moved(&mut e, &v, (30.0, 25.0)) else {
            panic!("pan should move the camera");
        };
        assert_eq!(c, cam(40.0, 45.0, 1.0));
        // Fed the moved view, the same world point stays under the pointer.
        let Change::Camera(c) = moved(&mut e, &with(c), (40.0, 25.0)) else {
            panic!("pan should move the camera");
        };
        assert_eq!(c, cam(30.0, 45.0, 1.0));
        assert_eq!(
            release(&mut e, Button::Left, &with(c), (40.0, 25.0)),
            Change::None
        );
        assert!(!e.is_panning());
    }

    #[test]
    fn middle_button_pans_with_any_tool_until_it_is_released() {
        let mut e = pencil();
        let v = view();
        assert_eq!(
            press(&mut e, Button::Middle, &v, (10.0, 10.0)),
            Change::None
        );
        assert!(e.is_panning());
        assert!(!e.is_drawing());
        assert_eq!(
            moved(&mut e, &v, (15.0, 10.0)),
            Change::Camera(cam(45.0, 50.0, 1.0))
        );
        assert_eq!(
            release(&mut e, Button::Left, &v, (15.0, 10.0)),
            Change::None
        );
        assert!(e.is_panning(), "another button does not end the pan");
        assert_eq!(
            release(&mut e, Button::Middle, &v, (15.0, 10.0)),
            Change::None
        );
        assert!(!e.is_panning());
    }

    #[test]
    fn zoom_click_steps_in_at_the_cursor() {
        let mut e = tool(Tool::Zoom);
        let v = view();
        assert_eq!(press(&mut e, Button::Left, &v, (20.0, 20.0)), Change::None);
        let Change::Camera(c) = release(&mut e, Button::Left, &v, (21.0, 20.0)) else {
            panic!("a click zooms one step");
        };
        assert_eq!(c.zoom, ZOOM_STEP);
        assert_close(with(c).screen_to_world(20.0, 20.0), (20.0, 20.0));
    }

    #[test]
    fn zoom_right_click_steps_out_at_the_cursor() {
        let mut e = tool(Tool::Zoom);
        let v = view();
        let Change::Camera(c) = press(&mut e, Button::Right, &v, (70.0, 30.0)) else {
            panic!("right press zooms out one step");
        };
        assert!((c.zoom - 1.0 / ZOOM_STEP).abs() < 1e-12);
        assert_close(with(c).screen_to_world(70.0, 30.0), (70.0, 30.0));
        assert_eq!(
            release(&mut e, Button::Right, &with(c), (70.0, 30.0)),
            Change::None
        );
    }

    #[test]
    fn zoom_drag_scrubs_the_zoom_around_the_press_point() {
        let mut e = tool(Tool::Zoom);
        let v = view();
        let _ = press(&mut e, Button::Left, &v, (20.0, 20.0));
        // ZOOM_DRAG_PX to the right is exactly one step in…
        let Change::Camera(c) = moved(&mut e, &v, (20.0 + ZOOM_DRAG_PX, 20.0)) else {
            panic!("drag should zoom");
        };
        assert!((c.zoom - ZOOM_STEP).abs() < 1e-12, "{}", c.zoom);
        assert_close(with(c).screen_to_world(20.0, 20.0), (20.0, 20.0));
        // …and the same distance to the left of the press is one step out,
        // measured from the press, not from the previous position.
        let Change::Camera(c) = moved(&mut e, &with(c), (20.0 - ZOOM_DRAG_PX, 20.0)) else {
            panic!("drag should zoom");
        };
        assert!((c.zoom - 1.0 / ZOOM_STEP).abs() < 1e-12, "{}", c.zoom);
        assert_close(with(c).screen_to_world(20.0, 20.0), (20.0, 20.0));
        // Releasing after a drag adds no click step.
        assert_eq!(
            release(&mut e, Button::Left, &with(c), (20.0 - ZOOM_DRAG_PX, 20.0)),
            Change::None
        );
    }

    #[test]
    fn zoom_drag_within_the_click_slop_is_still_a_click() {
        let mut e = tool(Tool::Zoom);
        let v = view();
        let _ = press(&mut e, Button::Left, &v, (20.0, 20.0));
        assert_eq!(moved(&mut e, &v, (22.0, 21.0)), Change::None);
        let Change::Camera(c) = release(&mut e, Button::Left, &v, (22.0, 21.0)) else {
            panic!("a click zooms one step");
        };
        assert_eq!(c.zoom, ZOOM_STEP);
    }

    #[test]
    fn ctrl_drag_zooms_from_any_tool() {
        let mut e = pencil();
        e.hold_ctrl(true);
        let v = view();
        assert_eq!(press(&mut e, Button::Left, &v, (20.0, 20.0)), Change::None);
        assert!(!e.is_drawing());
        assert!(matches!(moved(&mut e, &v, (60.0, 20.0)), Change::Camera(_)));
    }

    #[test]
    fn scroll_pans_the_content_with_the_delta() {
        let e = Editor::new();
        let v = view();
        assert_eq!(
            e.scroll(&v, (50.0, 50.0), (10.0, -4.0), false),
            cam(40.0, 54.0, 1.0)
        );
    }

    #[test]
    fn shift_scroll_turns_a_vertical_wheel_horizontal() {
        let e = Editor::new();
        let v = view();
        assert_eq!(
            e.scroll(&v, (50.0, 50.0), (0.0, -8.0), true),
            cam(58.0, 50.0, 1.0)
        );
        // A horizontal delta (touchpad) is left alone.
        assert_eq!(
            e.scroll(&v, (50.0, 50.0), (5.0, 0.0), true),
            cam(45.0, 50.0, 1.0)
        );
    }

    #[test]
    fn scroll_zooms_at_the_cursor_while_zoom_is_active() {
        let mut e = Editor::new();
        e.hold_ctrl(true);
        let v = view();
        // One wheel line up is one step in.
        let c = e.scroll(&v, (20.0, 20.0), (0.0, SCROLL_LINE_PX), false);
        assert!((c.zoom - ZOOM_STEP).abs() < 1e-12, "{}", c.zoom);
        assert_close(with(c).screen_to_world(20.0, 20.0), (20.0, 20.0));
        // Half a line down (touchpad) is half a step out.
        let c = e.scroll(&v, (20.0, 20.0), (0.0, -SCROLL_LINE_PX / 2.0), false);
        assert!((c.zoom - ZOOM_STEP.powf(-0.5)).abs() < 1e-12, "{}", c.zoom);
        // The Z tool does the same.
        let c = tool(Tool::Zoom).scroll(&v, (20.0, 20.0), (0.0, SCROLL_LINE_PX), false);
        assert!((c.zoom - ZOOM_STEP).abs() < 1e-12, "{}", c.zoom);
    }

    #[test]
    fn pinch_zooms_at_the_cursor_and_pans_with_the_fingers() {
        let e = Editor::new();
        let v = View {
            scale: 2.0,
            ..view()
        };
        let g = Gesture::Pinch {
            dx: 5.0,
            dy: -3.0,
            factor: 1.5,
        };
        let c = e
            .gesture(&v, (20.0, 20.0), g)
            .expect("pinch moves the camera");
        // Logical deltas become physical px on a 2x display.
        let expected = View {
            camera: v.zoomed_at(1.5, (20.0, 20.0)),
            ..v
        }
        .panned_by(10.0, -6.0);
        assert_eq!(c, expected);
        assert_eq!(c.zoom, 1.5);
    }

    #[test]
    fn three_finger_swipe_pans_and_other_counts_are_ignored() {
        let e = Editor::new();
        let v = view();
        let swipe = |fingers| Gesture::Swipe {
            fingers,
            dx: 10.0,
            dy: -4.0,
        };
        assert_eq!(
            e.gesture(&v, (50.0, 50.0), swipe(3)),
            Some(cam(40.0, 54.0, 1.0))
        );
        assert_eq!(e.gesture(&v, (50.0, 50.0), swipe(4)), None);
        assert_eq!(e.gesture(&v, (50.0, 50.0), Gesture::End), None);
    }

    #[test]
    fn presses_during_a_gesture_are_ignored() {
        let v = view();
        let mut e = tool(Tool::Hand);
        let _ = press(&mut e, Button::Left, &v, (0.0, 0.0));
        assert_eq!(press(&mut e, Button::Right, &v, (0.0, 0.0)), Change::None);
        assert!(e.is_panning());

        let mut e = pencil();
        let _ = press(&mut e, Button::Left, &v, (0.0, 0.0));
        assert_eq!(press(&mut e, Button::Middle, &v, (0.0, 0.0)), Change::None);
        assert!(!e.is_panning());
        assert!(e.is_drawing());
    }

    #[test]
    fn cancel_and_set_tool_drop_a_pan_in_progress() {
        let v = view();
        let mut e = tool(Tool::Hand);
        let _ = press(&mut e, Button::Middle, &v, (0.0, 0.0));
        assert!(cancel(&mut e));
        assert!(!e.is_panning());
        assert_eq!(moved(&mut e, &v, (5.0, 5.0)), Change::None);

        let _ = press(&mut e, Button::Left, &v, (0.0, 0.0));
        set_tool(&mut e, Tool::Pencil);
        assert!(!e.is_panning());
    }

    const BLOB: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn pasted(doc: &Document) -> &crate::doc::Image {
        let Element::Image(i) = doc.elements.last().expect("nothing pasted") else {
            panic!("last element is not an image");
        };
        i
    }

    #[test]
    fn a_paste_lands_centered_on_the_pointer() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        // The view looks at (50, 50) with a 100x100 viewport at zoom 1, so
        // screen (10, 20) is world (10, 20).
        let change = e.paste_image(&mut doc, &view(), Some((10.0, 20.0)), BLOB.into(), (40, 20));
        assert_eq!(change, Change::Scene);
        let i = pasted(&doc);
        assert_eq!((i.w, i.h), (40.0, 20.0));
        assert_eq!((i.x, i.y), (-10.0, 10.0));
        assert_eq!(i.blob, BLOB);
        assert_eq!(i.rotation, 0.0);
    }

    #[test]
    fn a_paste_with_the_pointer_away_lands_in_the_middle_of_the_view() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let _ = e.paste_image(&mut doc, &view(), None, BLOB.into(), (40, 20));
        let i = pasted(&doc);
        assert_eq!((i.x, i.y), (30.0, 40.0));
    }

    #[test]
    fn a_paste_larger_than_the_view_is_shrunk_to_fit_it() {
        // A 4K screenshot must not cover the board wall to wall.
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let _ = e.paste_image(&mut doc, &view(), None, BLOB.into(), (3840, 2160));
        let i = pasted(&doc);
        // 80% of the 100x100 the view shows, aspect kept.
        assert!((i.w - 80.0).abs() < 1e-9, "{}", i.w);
        assert!((i.h - 45.0).abs() < 1e-9, "{}", i.h);
    }

    #[test]
    fn the_room_a_paste_gets_follows_the_zoom() {
        // Zoomed out, the same viewport shows more world — so more fits.
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let mut v = view();
        v.camera.zoom = 0.5;
        let _ = e.paste_image(&mut doc, &v, None, BLOB.into(), (160, 80));
        assert_eq!((pasted(&doc).w, pasted(&doc).h), (160.0, 80.0));
    }

    #[test]
    fn a_pasted_image_becomes_the_selection() {
        // Paste then drag: the thing that just landed is what moves.
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let _ = e.paste_image(&mut doc, &view(), Some((50.0, 50.0)), BLOB.into(), (10, 10));
        assert_eq!(e.selection(), &[doc.elements[0].id().to_owned()]);
    }

    #[test]
    fn a_paste_cancels_whatever_was_in_progress() {
        // Ctrl+V mid-stroke must not leave half a pen stroke behind.
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Pencil, &mut doc);
        let _ = e.press(Button::Left, &view(), (10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&view(), (20.0, 20.0), &mut doc);
        assert!(e.is_drawing());
        let _ = e.paste_image(&mut doc, &view(), None, BLOB.into(), (10, 10));
        assert!(!e.is_drawing());
        assert_eq!(doc.elements.len(), 1, "only the image landed");
    }

    #[test]
    fn a_paste_switches_to_select_so_the_new_image_can_be_moved() {
        // Landing selected is only useful if the next drag moves it.
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Pencil, &mut doc);
        let _ = e.paste_image(&mut doc, &view(), None, BLOB.into(), (10, 10));
        assert_eq!(e.tool(), Tool::Select);
        assert_eq!(e.selection().len(), 1);
    }

    /// A board whose only layer is a frame's: the frame `fr` spans
    /// (0,0)–(100,100) and holds one layer, `in`.
    fn framed_editor_doc() -> Document {
        Document::from_json(
            r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [
                { "id": "fr", "type": "frame", "layer": "fl",
                  "x": 0, "y": 0, "w": 100, "h": 100, "background": "#fff",
                  "layers": [ { "id": "in", "name": "Layer 1" } ] }
            ]
        }"##,
        )
        .unwrap()
    }

    #[test]
    fn the_editor_starts_on_the_board_and_can_enter_a_frame() {
        let doc = framed_editor_doc();
        let mut e = Editor::new();
        assert_eq!(e.inside(), None);
        let _ = e.enter_frame(&doc, "fr");
        assert_eq!(e.inside(), Some("fr"));
        assert_eq!(e.active_layer(&doc), 0, "the top of the frame's stack");
        assert_eq!(doc.stack(e.inside())[e.active_layer(&doc)].id, "in");
        let _ = e.leave_frame(&doc);
        assert_eq!(e.inside(), None);
    }

    #[test]
    fn entering_a_frame_that_is_not_there_does_nothing() {
        let doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "nobody");
        assert_eq!(e.inside(), None);
    }

    #[test]
    fn a_layer_added_while_inside_a_frame_lands_in_the_frames_stack() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        let _ = e.add_layer(&mut doc);
        assert_eq!(doc.stack(Some("fr")).len(), 2);
        assert_eq!(doc.layers.len(), 1, "the board's stack did not grow");
    }

    #[test]
    fn leaving_a_frame_makes_its_own_layer_active() {
        let doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        let _ = e.leave_frame(&doc);
        assert_eq!(
            doc.stack(None)[e.active_layer(&doc)].id,
            "fl",
            "the card the pointer came from"
        );
    }

    #[test]
    fn inside_lets_go_when_its_frame_is_gone() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        doc.elements.retain(|el| el.id() != "fr");
        doc.layers.clear();
        doc.layers.push(crate::doc::Layer::new("Layer 1"));
        assert_eq!(
            e.active_layer(&doc),
            0,
            "it falls back to the board's top layer"
        );
    }

    /// A layer picked inside a frame is read against that frame's stack,
    /// not the board's.
    #[test]
    fn select_layer_reads_the_stack_being_worked_in() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        doc.add_layer(Some("fr"), 0, Kind::Raster).unwrap();
        let _ = e.select_layer(&doc, 0);
        assert_eq!(doc.stack(Some("fr"))[e.active_layer(&doc)].id, "in");
        let _ = e.select_layer(&doc, 1);
        assert_eq!(e.active_layer(&doc), 1);
    }

    /// Deleting the frame's own layer from the board takes the frame,
    /// its stack and everything on it — and the panel stops standing in
    /// a stack that is gone.
    #[test]
    fn removing_a_frame_layer_lets_go_of_the_frame() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, crate::doc::Layer::new("Layer 1"));
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        let _ = e.leave_frame(&doc);
        let _ = e.enter_frame(&doc, "fr");
        // Stand on the board, on the frame's own card, and take it out.
        e.inside = None;
        e.layer = Some("fl".into());
        let _ = e.remove_layer(&mut doc);
        assert!(!doc.elements.iter().any(|el| el.id() == "fr"));
        assert_eq!(e.inside(), None, "and the panel came back out");
    }

    /// A layer hidden inside a frame is the frame's, not the board's.
    #[test]
    fn toggling_a_layer_inside_a_frame_hides_that_one() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.enter_frame(&doc, "fr");
        let _ = e.toggle_layer(&mut doc, 0);
        assert!(!doc.stack(Some("fr"))[0].visible);
        assert!(doc.layers[0].visible, "the frame's own card is untouched");
    }

    /// A press at a world point, through the view the test is using.
    fn at(v: &View, x: f64, y: f64) -> (f64, f64) {
        v.world_to_screen(x, y)
    }

    /// Where the paint that landed sits: the frame holding its layer.
    fn paint_stack(doc: &Document) -> Option<String> {
        let paint = doc
            .elements
            .iter()
            .find(|el| matches!(el, Element::Paint(_)))
            .expect("a paint landed");
        doc.locate(paint.layer())
            .and_then(|(f, _)| f)
            .map(str::to_owned)
    }

    #[test]
    fn a_stroke_started_inside_a_frame_lands_in_its_stack() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        e.set_tool(Tool::Brush, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 20.0, 20.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 40.0, 40.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 40.0, 40.0), &mut doc, "#111");
        assert_eq!(paint_stack(&doc).as_deref(), Some("fr"));
    }

    /// The press decides, not the release: a stroke that wanders out of
    /// the frame it started in belongs to that frame and is cut by it.
    #[test]
    fn a_stroke_that_wanders_out_still_belongs_to_the_frame_it_started_in() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        e.set_tool(Tool::Brush, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 20.0, 20.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 500.0, 500.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 500.0, 500.0), &mut doc, "#111");
        assert_eq!(paint_stack(&doc).as_deref(), Some("fr"));
    }

    #[test]
    fn a_stroke_started_on_the_open_board_lands_on_the_board() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        let mut e = Editor::new();
        e.set_tool(Tool::Brush, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 500.0, 500.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 510.0, 510.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 510.0, 510.0), &mut doc, "#111");
        assert_eq!(paint_stack(&doc), None);
    }

    /// The live stroke and the stroke it becomes are under one boundary:
    /// live_layer answers for the stack the press was in, whatever the
    /// panel is standing in.
    #[test]
    fn the_live_stroke_and_the_landed_stroke_agree_on_the_stack() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        let mut e = Editor::new();
        e.set_tool(Tool::Brush, &mut doc);
        let v = view();
        // Standing on the board, but pressing inside the frame.
        let _ = e.press(Button::Left, &v, at(&v, 20.0, 20.0), &mut doc, &brush().tip(Face::Round));
        let live = e.live_layer(&doc).map(str::to_owned);
        assert_eq!(
            live.as_deref().and_then(|l| doc.locate(l)).and_then(|(f, _)| f),
            Some("fr"),
            "the live stroke is painted in the frame the press was in"
        );
        let _ = e.moved(&v, at(&v, 40.0, 40.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 40.0, 40.0), &mut doc, "#111");
        assert_eq!(paint_stack(&doc).as_deref(), Some("fr"));
    }

    /// A pencil opens a layer of its own, and it opens it in the frame
    /// the press was in.
    #[test]
    fn a_pencil_stroke_inside_a_frame_opens_its_layer_there() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        let mut e = Editor::new();
        e.set_tool(Tool::Pencil, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 20.0, 20.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 40.0, 40.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 40.0, 40.0), &mut doc, "#111");
        let path = doc
            .elements
            .iter()
            .find(|el| matches!(el, Element::Path(_)))
            .expect("a path landed");
        assert_eq!(doc.locate(path.layer()).and_then(|(f, _)| f), Some("fr"));
        assert_eq!(doc.stack(Some("fr")).len(), 2, "its own layer, in the frame");
    }

    /// A loose rect on a board layer of its own, well away from the
    /// frame, and the id of the layer it brought with it.
    fn loose_on_the_board(doc: &mut Document, id: &str, x: f64, y: f64) -> String {
        let layer = Layer::new("Layer 1");
        let layer_id = layer.id.clone();
        doc.layers.insert(0, layer);
        doc.elements.push(Element::Rect(Rect {
            id: id.into(),
            layer: layer_id.clone(),
            x,
            y,
            w: 10.0,
            h: 10.0,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }));
        layer_id
    }

    #[test]
    fn an_object_dragged_into_a_frame_joins_it() {
        let mut doc = framed_editor_doc();
        let layer = loose_on_the_board(&mut doc, "loose", 500.0, 500.0);
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 505.0, 505.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 55.0, 55.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 55.0, 55.0), &mut doc, "#111");
        assert_eq!(
            doc.locate(&layer).and_then(|(f, _)| f),
            Some("fr"),
            "the layer moved into the frame"
        );
        let el = doc.elements.iter().find(|el| el.id() == "loose").unwrap();
        assert_eq!(el.layer(), layer, "and the object still names its layer");
    }

    #[test]
    fn an_object_dragged_out_of_a_frame_leaves_it() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        doc.elements.push(Element::Rect(Rect {
            id: "held".into(),
            layer: "in".into(),
            x: 40.0,
            y: 40.0,
            w: 10.0,
            h: 10.0,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }));
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 45.0, 45.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 600.0, 600.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 600.0, 600.0), &mut doc, "#111");
        assert_eq!(doc.locate("in").and_then(|(f, _)| f), None, "the layer came out");
    }

    #[test]
    fn a_frame_itself_never_rehomes() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        doc.layers.push(Layer {
            id: "fl2".into(),
            name: "Frame 2".into(),
            visible: true,
            kind: Kind::Frame,
        });
        doc.elements.push(Element::Frame(crate::doc::Frame {
            id: "fr2".into(),
            layer: "fl2".into(),
            x: 400.0,
            y: 400.0,
            w: 200.0,
            h: 200.0,
            background: None,
            layers: vec![Layer::new("Layer 1")],
        }));
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 10.0, 10.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 460.0, 460.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 460.0, 460.0), &mut doc, "#111");
        assert_eq!(
            doc.locate("fl").and_then(|(f, _)| f),
            None,
            "a frame does not nest, however far it is dragged"
        );
    }

    /// A click that selects is not a move: nothing changes stacks.
    #[test]
    fn a_click_that_does_not_move_rehomes_nothing() {
        let mut doc = framed_editor_doc();
        let layer = loose_on_the_board(&mut doc, "loose", 20.0, 20.0);
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 25.0, 25.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.release(Button::Left, &v, at(&v, 25.0, 25.0), &mut doc, "#111");
        assert_eq!(doc.locate(&layer).and_then(|(f, _)| f), None);
    }

    /// The frame of `framed_editor_doc`, with a rect inside it and a
    /// board layer under everything.
    fn frame_holding_a_rect() -> Document {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        doc.elements.push(Element::Rect(Rect {
            id: "held".into(),
            layer: "in".into(),
            x: 40.0,
            y: 40.0,
            w: 10.0,
            h: 10.0,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }));
        doc
    }

    fn centre_of(doc: &Document, id: &str) -> [f64; 2] {
        let el = doc.elements.iter().find(|el| el.id() == id).unwrap();
        select::frame(el).unwrap().center
    }

    #[test]
    fn moving_a_frame_carries_what_it_holds() {
        let mut doc = frame_holding_a_rect();
        let mut e = Editor::new();
        let v = view();
        // Press on the frame's own ground, away from the rect.
        let _ = e.press(Button::Left, &v, at(&v, 90.0, 90.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.selection(), ["fr"]);
        let _ = e.moved(&v, at(&v, 190.0, 90.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 190.0, 90.0), &mut doc, "#111");
        assert_eq!(
            centre_of(&doc, "held"),
            [145.0, 45.0],
            "it travelled with the frame"
        );
    }

    #[test]
    fn resizing_a_frame_leaves_what_it_holds_where_it_is() {
        let mut doc = frame_holding_a_rect();
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 90.0, 90.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.release(Button::Left, &v, at(&v, 90.0, 90.0), &mut doc, "#111");
        let sel = e.selection_frame(&doc).unwrap();
        let (_, corner) = *select::handles(&sel, &v)
            .iter()
            .find(|(h, _)| matches!(h, Handle::Resize(Corner::BottomRight)))
            .unwrap();
        let _ = e.press(Button::Left, &v, (corner[0], corner[1]), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, (corner[0] + 50.0, corner[1] + 50.0), &mut doc);
        let _ = e.release(Button::Left, &v, (corner[0] + 50.0, corner[1] + 50.0), &mut doc, "#111");
        assert_eq!(
            centre_of(&doc, "held"),
            [45.0, 45.0],
            "the boundary moved and the contents did not"
        );
        let Element::Frame(f) = doc.elements.iter().find(|el| el.id() == "fr").unwrap() else {
            panic!("not a frame");
        };
        assert!(f.w > 100.0 && f.h > 100.0, "the area grew: {} by {}", f.w, f.h);
    }

    #[test]
    fn a_frame_in_the_selection_offers_no_rotation() {
        let mut doc = frame_holding_a_rect();
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 90.0, 90.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.release(Button::Left, &v, at(&v, 90.0, 90.0), &mut doc, "#111");
        let sel = e.selection_frame(&doc).unwrap();
        let (_, ring) = *select::handles(&sel, &v)
            .iter()
            .find(|(h, _)| matches!(h, Handle::Rotate(_)))
            .unwrap();
        assert_eq!(
            e.hover(&doc, &v, (ring[0], ring[1])),
            None,
            "the rings are not offered while a frame is selected"
        );
        // And pressing there does not start one.
        let _ = e.press(Button::Left, &v, (ring[0], ring[1]), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, (ring[0] + 30.0, ring[1]), &mut doc);
        let _ = e.release(Button::Left, &v, (ring[0] + 30.0, ring[1]), &mut doc, "#111");
        let Element::Frame(f) = doc.elements.iter().find(|el| el.id() == "fr").unwrap() else {
            panic!("not a frame");
        };
        assert_eq!((f.w, f.h), (100.0, 100.0), "nothing turned it");
    }

    /// Picking an element makes its layer active — and when that layer is
    /// a frame's, the panel goes in with it, or the active layer would
    /// name a stack nobody is standing in.
    #[test]
    fn picking_something_inside_a_frame_goes_in_with_it() {
        let mut doc = frame_holding_a_rect();
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 45.0, 45.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.selection(), ["held"]);
        assert_eq!(e.inside(), Some("fr"));
        assert_eq!(doc.stack(e.inside())[e.active_layer(&doc)].id, "in");
    }

    /// And picking something on the board comes back out.
    #[test]
    fn picking_something_on_the_board_comes_back_out() {
        let mut doc = frame_holding_a_rect();
        let layer = loose_on_the_board(&mut doc, "loose", 500.0, 500.0);
        let mut e = Editor::new();
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 45.0, 45.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.release(Button::Left, &v, at(&v, 45.0, 45.0), &mut doc, "#111");
        let _ = e.press(Button::Left, &v, at(&v, 505.0, 505.0), &mut doc, &brush().tip(Face::Round));
        assert_eq!(e.inside(), None);
        assert_eq!(doc.stack(None)[e.active_layer(&doc)].id, layer);
    }
}
