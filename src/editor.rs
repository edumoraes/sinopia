//! Session state that never enters the document (§6.2): the active tool,
//! the keys temporarily overriding it, the stroke being drawn, the
//! pan/zoom gesture in progress, the selection and the drag reshaping it.
//! Pure — `app` feeds it pointer events in screen px together with the
//! current [`View`] and the document, and stores whatever comes back.

use serde::{Deserialize, Serialize};

use crate::bitmap;
use crate::brush::{Dynamics, Tip};
use crate::curve::{self, Cubic};
use crate::doc::{BlendMode, Camera, Document, Element, Envelope, Image, Kind, Layer, Paint, Path, Tag, new_id};
use crate::geom::{Affine, Corner, Frame, Point};
use crate::merge::{Merge, Run};
use crate::scene::View;
use crate::select::{self, Handle};
use crate::tree::{self, Arrange, Filter, Place};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Hand,
    Pencil,
    Brush,
    Frame,
    Zoom,
}

impl Tool {
    /// Dock order.
    pub const ALL: [Tool; 6] = [
        Tool::Select,
        Tool::Hand,
        Tool::Pencil,
        Tool::Brush,
        Tool::Frame,
        Tool::Zoom,
    ];

    pub fn hotkey(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Hand => 'h',
            Tool::Pencil => 'p',
            Tool::Brush => 'b',
            Tool::Frame => 'f',
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
    /// The frame the press was in, if any, by its layer — the stack the
    /// ink lands in. Read once, at the press, and never again: the stroke is painted from its first sample, so if
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

/// Where the hand is standing: what is selected, the layer new ink
/// lands on, and the layers picked in the panel. Session state (§6.2) — it
/// never enters the document — and exactly the part of it that belongs
/// beside a board rather than to the window.
///
/// The tool, the keys held down and what the pen last said are not
/// here: those are physical, and a step backwards does not move a hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spot {
    /// Ids of the selected elements, in selection order.
    pub selection: Vec<String>,
    pub layer: Option<String>,
    pub picked: Vec<String>,
}

/// How a press on a row picks its layer: alone, in or out of what is
/// already picked (`Ctrl`), or every row between the anchor and it
/// (`Shift`) — Photoshop's three, and every file manager's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Only,
    Toggle,
    Range,
}

#[derive(Debug, Default, Clone)]
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
    /// The area the Frame tool is dragging out: where the press was and
    /// where the pointer is, in world units.
    framing: Option<(Point, Point)>,
    /// What a new frame's background is set to — the theme's surface,
    /// handed over by `app` so the editor never sees a `Theme`.
    surface: String,
    /// The layers picked in the panel, by id, in the order they were
    /// picked. Empty is the active layer alone. The active layer is the
    /// anchor a range is taken from, and where ink goes.
    picked: Vec<String>,
    /// The pick was made in the panel, not on the canvas: what `Delete`
    /// takes is the layers, then, and not the objects.
    in_panel: bool,
    /// The groups and frames whose rows the panel shows open, by id.
    /// Session state of the panel's and nothing else's: it is not where
    /// the hand stands, so a step back does not shut what was opened.
    open: Vec<String>,
    /// The panel's filter, and whether its bar is open: shut, the tree is
    /// shown whole and the filter waits as it was left.
    filter: Filter,
    filtering: bool,
}

/// One layer as the command line lists it: where it stands in the tree,
/// what it is and how it draws, and whether it is the active layer or
/// among the picked. `visible` is its own eye; `shown` is whether it is
/// on show at all, which every layer holding it has a say in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The layer holding its stack; none on the board's root.
    pub owner: Option<String>,
    pub depth: usize,
    pub visible: bool,
    pub shown: bool,
    pub locked: bool,
    pub opacity: f64,
    pub blend: BlendMode,
    pub color: Tag,
    pub active: bool,
    pub picked: bool,
    /// How many objects stand on it.
    pub elements: usize,
}

/// What a lock keeps a layer from being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keeps {
    /// How it draws — its strength and its mode — which its own lock and
    /// every holder's keep.
    Look,
    /// Where it stands in its stack, which only a locked holder keeps: a
    /// layer's own lock is about what it holds, not about its place.
    Place,
}

/// The first of `ids` a lock keeps from what `keeps` names, if any: what
/// the command line refuses rather than answering done while nothing
/// changed.
pub fn locked_among(doc: &Document, ids: &[String], keeps: Keeps) -> Option<String> {
    ids.iter()
        .find(|id| match keeps {
            Keeps::Look => doc.locked(id),
            Keeps::Place => doc.locate(id).is_some_and(|(owner, _)| doc.fixed(owner)),
        })
        .cloned()
}

/// A command on the picked layers: what a shortcut, a row's menu and
/// the command line all ask for, so the three cannot come to disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Group,
    Ungroup,
    Duplicate,
    /// Takes the picked layers away, with everything on them.
    Remove,
    /// Locks them all while any is open, and opens them all once every
    /// one is locked; `Show` does the same with what shows.
    Lock,
    Show,
    Arrange(Arrange),
    /// The picked siblings into one, a group alone into a layer, or one
    /// layer down into the one under it: `Ctrl+Alt+E`.
    Merge,
    /// The active layer — a group too — into the one under it, as the
    /// command line asks by name.
    MergeDown,
    /// Every visible sibling of every stack into one: `Ctrl+Shift+E`.
    MergeVisible,
    /// Merge visible, and what is hidden goes.
    Flatten,
}

impl Command {
    /// Whether it merges: those wait on pictures `app` takes of what is
    /// not exact, and go through [`Editor::merge`].
    pub fn merges(self) -> bool {
        matches!(
            self,
            Command::Merge | Command::MergeDown | Command::MergeVisible | Command::Flatten
        )
    }
}

/// What a new frame is born with until `app` says otherwise: white, the
/// colour a sheet of paper is.
const DEFAULT_SURFACE: &str = "#ffffff";

/// The smallest drag that is an area rather than a click, in screen px.
/// What it refuses is a hand that did not mean to drag, and that is the
/// same few pixels at every zoom.
const MIN_FRAME_PX: f64 = 8.0;

impl Editor {
    pub fn new() -> Editor {
        Editor {
            surface: DEFAULT_SURFACE.to_owned(),
            ..Editor::default()
        }
    }

    /// The colour a new frame's ground is laid in.
    pub fn set_surface(&mut self, hex: &str) {
        hex.clone_into(&mut self.surface);
    }

    /// The area being dragged out, while there is one.
    pub fn framing(&self) -> Option<(Point, Point)> {
        self.framing
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

    /// The selected elements, by id. The shell reads them to answer a
    /// question a frame is the only one that asks: an ink picked with a
    /// frame selected paints its ground rather than the next stroke.
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

    /// The layer new ink is aimed at, by id: the one chosen while it is
    /// still on the board, or the top of the board's root otherwise.
    pub fn active<'a>(&self, doc: &'a Document) -> &'a str {
        self.layer
            .as_deref()
            .and_then(|id| doc.layer(id))
            .or_else(|| doc.layers.last())
            .map_or("", |l| l.id.as_str())
    }

    /// The layer ink pressed into `born` — a frame, by its layer, or the
    /// board — is aimed at: the active layer when it stands where the
    /// press landed, and the top of that stack when it does not. Pressing
    /// inside a frame while another layer is active paints in the frame.
    fn aim<'a>(&self, doc: &'a Document, born: Option<&str>) -> Option<&'a Layer> {
        let a = self.active(doc);
        if doc.context(a) == born {
            return doc.layer(a);
        }
        doc.stack(born).last()
    }

    /// Where a layer opened for ink pressed into `born` goes: the stack,
    /// and the index it is laid above — inside the layer aimed at, on
    /// top, when that is a group; right above it otherwise.
    fn opening(&self, doc: &Document, born: Option<&str>) -> (Option<String>, usize) {
        let top = |owner: Option<&str>| doc.stack(owner).len().saturating_sub(1);
        let (owner, above) = match self.aim(doc, born) {
            Some(g) if g.kind == Kind::Group => (Some(g.id.clone()), top(Some(&g.id))),
            Some(l) => match doc.locate(&l.id) {
                Some((owner, i)) => (owner.map(str::to_owned), i),
                None => (born.map(str::to_owned), top(born)),
            },
            None => (born.map(str::to_owned), top(born)),
        };
        clear_of_locks(doc, owner, above)
    }

    /// Whether a press at `screen` would lay ink that has nowhere to go:
    /// a brush aimed at a raster layer that is locked, or held by a locked
    /// one. The press is refused, and the cursor says so before it.
    pub fn refuses_ink(&self, doc: &Document, view: &View, screen: (f64, f64)) -> bool {
        if self.pointer_tool(doc, view, screen) != Tool::Brush {
            return false;
        }
        let (x, y) = view.screen_to_world(screen.0, screen.1);
        let born = doc.stack_at([x, y]);
        self.aim(doc, born)
            .is_some_and(|l| l.kind == Kind::Raster && doc.locked(&l.id))
    }

    /// The raster layer ink pressed into `born` joins, when there is one
    /// to join: the layer aimed at, when it takes pixels.
    fn joins<'a>(&self, doc: &'a Document, born: Option<&str>) -> Option<&'a str> {
        self.aim(doc, born)
            .filter(|l| l.kind == Kind::Raster)
            .map(|l| l.id.as_str())
    }

    /// Opens a layer of `kind` for ink pressed into `born`, where
    /// [`Editor::opening`] says, makes it active and answers its id —
    /// what an element that comes with its own layer is stamped with.
    fn fresh_layer(&mut self, doc: &mut Document, kind: Kind, born: Option<&str>) -> String {
        let (owner, above) = self.opening(doc, born);
        let at = doc
            .add_layer(owner.as_deref(), above, kind)
            .unwrap_or_default();
        let id = doc.stack(owner.as_deref())[at].id.clone();
        self.reveal(doc, &id);
        self.layer = Some(id.clone());
        self.picked.clear();
        id
    }

    /// The layer the stroke in progress would join, if there is one to
    /// join: the active layer when it takes pixels, stands where the press
    /// landed, and the stroke is a brush's. A pencil opens a layer of its
    /// own and a brush over a vector layer opens one above it, so neither
    /// has anywhere to be painted yet — they are drawn over everything
    /// until they land.
    pub fn live_layer<'a>(&self, doc: &'a Document) -> Option<&'a str> {
        if self.tool == Tool::Pencil {
            return None;
        }
        let born = self.stroke.as_ref().and_then(|s| s.born.as_deref());
        self.joins(doc, born.filter(|id| doc.frame_on(id).is_some()))
    }

    /// The layer a new element of `kind` lands on. Raster accumulates —
    /// it joins the active layer when that one takes pixels, and opens
    /// one above it when it does not, as Photoshop does when you paint on
    /// a shape. Vector does not accumulate: every object gets a layer of
    /// its own.
    fn ink_layer(&mut self, doc: &mut Document, kind: Kind, born: Option<&str>) -> String {
        let born = born.filter(|id| doc.frame_on(id).is_some()).map(str::to_owned);
        if kind == Kind::Raster
            && let Some(id) = self.joins(doc, born.as_deref())
        {
            return id.to_owned();
        }
        self.fresh_layer(doc, kind, born.as_deref())
    }

    /// The layers picked in the panel, in the order they were picked —
    /// or the active layer alone, when none are. One gone from the board
    /// is left out.
    pub fn picked<'a>(&self, doc: &'a Document) -> Vec<&'a str> {
        let mut out: Vec<&str> = self
            .picked
            .iter()
            .filter_map(|id| doc.layer(id))
            .map(|l| l.id.as_str())
            .collect();
        if out.is_empty() {
            out.push(self.active(doc));
        }
        out
    }

    /// A press on `id`'s row, picking it as `how` says. `rows` is the
    /// order the panel lists its rows in, top first — what a range is
    /// taken across. With the Select tool in hand the canvas follows:
    /// what the picked layers hold becomes the selection.
    pub fn pick_layer(&mut self, doc: &Document, id: &str, how: Pick, rows: &[&str]) -> Change {
        if doc.layer(id).is_none() {
            return Change::None;
        }
        let anchor = self.active(doc).to_owned();
        let mut picked: Vec<String> = self.picked(doc).into_iter().map(str::to_owned).collect();
        let range = || {
            let a = rows.iter().position(|r| *r == anchor)?;
            let b = rows.iter().position(|r| *r == id)?;
            Some(rows[a.min(b)..=a.max(b)].iter().map(|r| (*r).to_owned()).collect())
        };
        match how {
            Pick::Toggle => match picked.iter().position(|p| p == id) {
                // The last one stays: there is always a layer to paint on.
                Some(_) if picked.len() == 1 => return Change::None,
                Some(i) => {
                    picked.remove(i);
                    if anchor == id {
                        self.layer = picked.last().cloned();
                    }
                }
                None => {
                    picked.push(id.to_owned());
                    self.layer = Some(id.to_owned());
                }
            },
            Pick::Range if let Some(between) = range() => picked = between,
            Pick::Only | Pick::Range => {
                picked = vec![id.to_owned()];
                self.layer = Some(id.to_owned());
            }
        }
        self.picked = picked;
        self.in_panel = true;
        self.follow_the_pick(doc);
        Change::Selection
    }

    /// With the Select tool in hand, what the picked layers hold becomes
    /// the selection; with any other the canvas is left alone.
    fn follow_the_pick(&mut self, doc: &Document) {
        if self.tool == Tool::Select {
            self.drag = None;
            self.selection = self.held_by(doc, &self.picked(doc));
        }
    }

    /// The picked layers' ids, owned: what the layer operations act on.
    fn picked_ids(&self, doc: &Document) -> Vec<String> {
        self.picked(doc).into_iter().map(str::to_owned).collect()
    }

    /// Wraps the picked layers in a new group standing where the topmost
    /// of them did — `Ctrl+G` — and picks the group.
    pub fn group_layers(&mut self, doc: &mut Document) -> Change {
        let picked = self.picked_ids(doc);
        let Some(group) = doc.group_layers(&picked) else {
            return Change::None;
        };
        self.reveal(doc, &group);
        self.layer = Some(group);
        self.picked.clear();
        self.follow_the_pick(doc);
        Change::Scene
    }

    /// Lets the active group's layers out where it stood — `Ctrl+Shift+G`
    /// — and picks them.
    pub fn ungroup(&mut self, doc: &mut Document) -> Change {
        let group = self.active(doc).to_owned();
        let Some(out) = doc.ungroup(&group) else {
            return Change::None;
        };
        self.layer = out.last().cloned();
        self.picked = out;
        self.follow_the_pick(doc);
        Change::Scene
    }

    /// A copy of every picked layer right above its original — `Ctrl+J`
    /// — and the copies picked.
    pub fn duplicate_layers(&mut self, doc: &mut Document) -> Change {
        let picked = self.picked_ids(doc);
        let made = doc.duplicate_layers(&picked);
        if made.is_empty() {
            return Change::None;
        }
        self.layer = made.last().cloned();
        self.picked = made;
        self.follow_the_pick(doc);
        Change::Scene
    }

    /// Removes every picked layer, with everything under it and on it —
    /// the bin, and `Delete` on a pick made in the panel. One alone keeps
    /// [`Editor::remove_layer`]'s ways; several go, and the board and a
    /// frame left empty get a fresh layer.
    pub fn remove_layers(&mut self, doc: &mut Document) -> Change {
        let picked = self.picked_ids(doc);
        if let [one] = picked.as_slice() {
            self.layer = Some(one.clone());
            return self.remove_layer(doc);
        }
        // Where the topmost of them stood is where the hand goes back to.
        let top = picked
            .last()
            .and_then(|id| doc.locate(id))
            .map(|(o, i)| (o.map(str::to_owned), i));
        let mut gone = false;
        for id in picked.iter().rev() {
            if let Some((owner, i)) = doc.locate(id).map(|(o, i)| (o.map(str::to_owned), i))
                && !doc.fixed(owner.as_deref())
                && let Some(layer) = doc.stack(owner.as_deref()).get(i)
            {
                let held = doc.subtree(layer);
                if let Some(stack) = doc.stack_mut(owner.as_deref()) {
                    stack.remove(i);
                }
                doc.elements
                    .retain(|el| !held.iter().any(|h| h == el.layer()));
                gone = true;
            }
        }
        if !gone {
            return Change::None;
        }
        doc.fill_empty_stacks();
        self.drag = None;
        self.selection
            .retain(|id| doc.elements.iter().any(|el| el.id() == id));
        self.picked.clear();
        self.layer = top.and_then(|(owner, i)| {
            let stack = doc.stack(owner.as_deref());
            match stack.len() {
                0 => owner,
                n => Some(stack[i.min(n - 1)].id.clone()),
            }
        });
        Change::Scene
    }

    /// `Delete`: the picked layers when the pick was made in the panel,
    /// the selected objects when it was made on the canvas.
    pub fn delete(&mut self, doc: &mut Document) -> Change {
        if self.in_panel {
            self.remove_layers(doc)
        } else {
            self.delete_selection(doc)
        }
    }

    /// Moves the picked layers within their own stacks — to the top, a
    /// step up, a step down, to the bottom: `Ctrl+Shift+]`, `Ctrl+]`,
    /// `Ctrl+[`, `Ctrl+Shift+[`.
    pub fn arrange(&mut self, doc: &mut Document, how: Arrange) -> Change {
        let picked = self.picked_ids(doc);
        match doc.arrange(&picked, how) {
            true => Change::Scene,
            false => Change::None,
        }
    }

    /// Locks the picked layers — all of them while any is open, and opens
    /// them all once every one is locked: `Ctrl+/`. What a lock takes is
    /// no longer held.
    pub fn toggle_lock(&mut self, doc: &mut Document) -> Change {
        let picked = self.picked_ids(doc);
        let lock = picked
            .iter()
            .any(|id| doc.layer(id).is_some_and(|l| !l.locked));
        self.set_locked(doc, &picked, lock)
    }

    /// Locks the picked layers, or opens them — as asked, not toggled.
    pub fn lock_layers(&mut self, doc: &mut Document, locked: bool) -> Change {
        let picked = self.picked_ids(doc);
        self.set_locked(doc, &picked, locked)
    }

    /// Opens a lock, or closes it, on `id` alone: the lock on its row.
    pub fn toggle_lock_of(&mut self, doc: &mut Document, id: &str) -> Change {
        let Some(locked) = doc.layer(id).map(|l| l.locked) else {
            return Change::None;
        };
        self.set_locked(doc, &[id.to_owned()], !locked)
    }

    fn set_locked(&mut self, doc: &mut Document, ids: &[String], lock: bool) -> Change {
        let mut changed = false;
        for id in ids {
            if let Some(l) = doc.layer_mut(id)
                && l.locked != lock
            {
                l.locked = lock;
                changed = true;
            }
        }
        if !changed {
            return Change::None;
        }
        if lock {
            self.drag = None;
            self.selection.retain(|sel| {
                doc.painted()
                    .any(|p| p.element.id() == sel && !p.locked)
            });
        }
        Change::Scene
    }

    /// Gives the picked layers a strength, 0 to 1 — the bar's slider and,
    /// with a tool that does not paint, the digits. A locked layer keeps
    /// how it draws; what is not a number changes nothing.
    pub fn set_opacity(&mut self, doc: &mut Document, opacity: f64) -> Change {
        if opacity.is_nan() {
            return Change::None;
        }
        let opacity = opacity.clamp(0.0, 1.0);
        let mut changed = false;
        for id in self.picked_ids(doc) {
            if doc.locked(&id) {
                continue;
            }
            if let Some(l) = doc.layer_mut(&id)
                && l.opacity != opacity
            {
                l.opacity = opacity;
                changed = true;
            }
        }
        if changed { Change::Scene } else { Change::None }
    }

    /// Gives the picked layers a blend mode — the bar's menu. Only a
    /// group passes through, and a locked layer keeps how it draws.
    pub fn set_blend(&mut self, doc: &mut Document, mode: BlendMode) -> Change {
        let mut changed = false;
        for id in self.picked_ids(doc) {
            if doc.locked(&id) {
                continue;
            }
            if let Some(l) = doc.layer_mut(&id)
                && l.blend != mode
                && (mode != BlendMode::PassThrough || l.kind == Kind::Group)
            {
                l.blend = mode;
                changed = true;
            }
        }
        if changed { Change::Scene } else { Change::None }
    }

    /// Hides the picked layers — all of them while any shows, and shows
    /// them all once every one is hidden: `Ctrl+,`. What is hidden is no
    /// longer held.
    pub fn toggle_shown(&mut self, doc: &mut Document) -> Change {
        let hide = self
            .picked_ids(doc)
            .iter()
            .any(|id| doc.layer(id).is_some_and(|l| l.visible));
        self.show_layers(doc, !hide)
    }

    /// Shows the picked layers, or hides them — as asked, not toggled.
    /// What is hidden is no longer held.
    pub fn show_layers(&mut self, doc: &mut Document, visible: bool) -> Change {
        let mut changed = false;
        for id in self.picked_ids(doc) {
            if let Some(l) = doc.layer_mut(&id)
                && l.visible != visible
            {
                l.visible = visible;
                changed = true;
            }
        }
        if !changed {
            return Change::None;
        }
        if !visible {
            self.drag = None;
            self.selection.retain(|sel| {
                doc.elements
                    .iter()
                    .any(|el| el.id() == sel && doc.shown(el.layer()))
            });
        }
        Change::Scene
    }

    /// What `layers` hold, in paint order: a group's are everything
    /// under it, a frame's is the frame — which carries what it holds.
    /// What is hidden or locked is left out: it cannot be seen, or it
    /// cannot be moved.
    fn held_by(&self, doc: &Document, layers: &[&str]) -> Vec<String> {
        let mut wanted: Vec<String> = Vec::new();
        for l in layers.iter().filter_map(|id| doc.layer(id)) {
            match l.kind {
                Kind::Group => wanted.extend(doc.subtree(l)),
                _ => wanted.push(l.id.clone()),
            }
        }
        doc.painted()
            .filter(|p| !p.locked && wanted.iter().any(|w| w == p.element.layer()))
            .map(|p| p.element.id().to_owned())
            .collect()
    }

    /// The canvas said what is selected: the panel picks the layers it
    /// stands on, in the order it was picked.
    fn pick_what_is_selected(&mut self, doc: &Document) {
        let mut layers: Vec<String> = Vec::new();
        for id in &self.selection {
            if let Some(el) = doc.elements.iter().find(|el| el.id() == id)
                && !layers.iter().any(|l| l == el.layer())
            {
                layers.push(el.layer().to_owned());
            }
        }
        self.picked = layers;
        self.in_panel = false;
    }

    /// Adds a raster layer and makes it active: inside the active layer,
    /// on top, when that is a group or a frame; above it in its own
    /// stack otherwise.
    pub fn add_layer(&mut self, doc: &mut Document) -> Change {
        self.add(doc, Kind::Raster)
    }

    /// Adds an empty group where [`Editor::add_layer`] would add a layer.
    pub fn add_group(&mut self, doc: &mut Document) -> Change {
        self.add(doc, Kind::Group)
    }

    fn add(&mut self, doc: &mut Document, kind: Kind) -> Change {
        let a = self.active(doc).to_owned();
        let (owner, above) = match doc.layer(&a) {
            Some(l) if matches!(l.kind, Kind::Group | Kind::Frame) => {
                (Some(a.clone()), doc.stack(Some(&a)).len().saturating_sub(1))
            }
            _ => match doc.locate(&a) {
                Some((owner, i)) => (owner.map(str::to_owned), i),
                None => return Change::None,
            },
        };
        let (owner, above) = clear_of_locks(doc, owner, above);
        let Some(at) = doc.add_layer(owner.as_deref(), above, kind) else {
            return Change::None;
        };
        let id = doc.stack(owner.as_deref())[at].id.clone();
        self.reveal(doc, &id);
        self.layer = Some(id);
        self.picked.clear();
        Change::Scene
    }

    /// The picked layers as a clip — `Ctrl+C` — or none to copy.
    pub fn copy(&self, doc: &Document) -> Option<Document> {
        doc.clip(&self.picked_ids(doc))
    }

    /// The picked layers as a clip, taken off the board — `Ctrl+X`.
    pub fn cut(&mut self, doc: &mut Document) -> (Option<Document>, Change) {
        let Some(clip) = self.copy(doc) else {
            return (None, Change::None);
        };
        (Some(clip), self.remove_layers(doc))
    }

    /// Plants `clip` above the active layer and picks what it planted —
    /// `Ctrl+V`. It lands in place when that place is in `seen`, the part
    /// of the world on show, and in the middle of it otherwise: a paste
    /// nobody can see did not happen, as far as the hand can tell.
    pub fn paste(&mut self, doc: &mut Document, clip: &Document, seen: (Point, Point)) -> Change {
        let ids: Vec<String> = clip.elements.iter().map(|el| el.id().to_owned()).collect();
        let (lo, hi) = seen;
        let by = match select::frame_of(clip, &ids).map(|f| f.aabb()) {
            Some((a, b)) if b[0] < lo[0] || a[0] > hi[0] || b[1] < lo[1] || a[1] > hi[1] => {
                Affine::translate(
                    (lo[0] + hi[0] - a[0] - b[0]) / 2.0,
                    (lo[1] + hi[1] - a[1] - b[1]) / 2.0,
                )
            }
            _ => Affine::IDENTITY,
        };
        let above = self.active(doc).to_owned();
        let planted = doc.paste(clip, &above, &by);
        let Some(top) = planted.last().cloned() else {
            return Change::None;
        };
        for id in &planted {
            self.reveal(doc, id);
        }
        self.picked = planted;
        self.layer = Some(top);
        self.follow_the_pick(doc);
        Change::Scene
    }

    /// Picks exactly `ids`, the last of them active, as the panel picks —
    /// what a command from the command line acts on — and opens the rows
    /// holding them. Every id has to be a layer of the board: a name that
    /// is not picks nothing at all.
    pub fn pick_ids(&mut self, doc: &Document, ids: &[String]) -> Result<(), String> {
        let Some(last) = ids.last() else {
            return Err("no layer named".into());
        };
        if let Some(missing) = ids.iter().find(|id| doc.layer(id).is_none()) {
            return Err(format!("no layer {missing:?} on the board"));
        }
        self.picked = ids.to_vec();
        self.layer = Some(last.clone());
        self.in_panel = true;
        for id in ids {
            self.reveal(doc, id);
        }
        self.follow_the_pick(doc);
        Ok(())
    }

    /// A new raster layer — or group — above `above`, or above the
    /// active layer when none is named, called `name` when there is one.
    pub fn add_layer_as(
        &mut self,
        doc: &mut Document,
        group: bool,
        name: Option<&str>,
        above: Option<&str>,
    ) -> Result<Change, String> {
        if let Some(above) = above {
            self.pick_ids(doc, &[above.to_owned()])?;
        }
        let change = if group {
            self.add_group(doc)
        } else {
            self.add_layer(doc)
        };
        if change == Change::None {
            return Err("nothing can be added there: a locked group takes nothing in".into());
        }
        if let Some(name) = name {
            let id = self.active(doc).to_owned();
            let _ = self.rename_layer(doc, &id, name);
        }
        Ok(change)
    }

    /// Every layer of the board, the whole tree top first whatever the
    /// panel has open, with where it stands and what it is.
    pub fn listing(&self, doc: &Document) -> Vec<Listed> {
        let active = self.active(doc);
        let picked = self.picked(doc);
        doc.rows(|_| true)
            .iter()
            .map(|r| Listed {
                id: r.layer.id.clone(),
                name: r.layer.name.clone(),
                kind: r.layer.kind,
                owner: r.owner.map(str::to_owned),
                depth: r.depth,
                visible: r.layer.visible,
                shown: r.shown,
                locked: r.layer.locked,
                opacity: r.layer.opacity,
                blend: r.layer.blend,
                color: r.layer.color,
                active: r.layer.id == active,
                picked: picked.contains(&r.layer.id.as_str()),
                elements: doc.elements.iter().filter(|el| el.layer() == r.layer.id).count(),
            })
            .collect()
    }

    /// Does `command` to the picked layers.
    pub fn run(&mut self, doc: &mut Document, command: Command) -> Change {
        match command {
            Command::Group => self.group_layers(doc),
            Command::Ungroup => self.ungroup(doc),
            Command::Duplicate => self.duplicate_layers(doc),
            Command::Remove => self.remove_layers(doc),
            Command::Lock => self.toggle_lock(doc),
            Command::Show => self.toggle_shown(doc),
            Command::Arrange(how) => self.arrange(doc, how),
            Command::Merge | Command::MergeDown | Command::MergeVisible | Command::Flatten => {
                self.merge(doc, command, &[])
            }
        }
    }

    /// What [`Command::Merge`] does to the pick, as a menu names it.
    pub fn merge_name(&self, doc: &Document) -> &'static str {
        match self.merge_request(doc, Command::Merge) {
            Some(Merge::Layers(ids)) if ids.len() > 1 => "Merge Layers",
            Some(Merge::Layers(_)) => "Merge Group",
            _ => "Merge Down",
        }
    }

    fn merge_request(&self, doc: &Document, command: Command) -> Option<Merge> {
        match command {
            Command::Merge => {
                let picked = self.picked_ids(doc);
                let active = self.active(doc).to_owned();
                Some(if picked.len() > 1 {
                    Merge::Layers(picked)
                } else if doc.layer(&active).is_some_and(|l| l.kind == Kind::Group) {
                    Merge::Layers(vec![active])
                } else {
                    Merge::Down(active)
                })
            }
            Command::MergeDown => Some(Merge::Down(self.active(doc).to_owned())),
            Command::MergeVisible => Some(Merge::Visible),
            Command::Flatten => Some(Merge::Flatten),
            _ => None,
        }
    }

    /// The merges `command` asks of the board, in the order `merge` takes
    /// the pictures of the ones that are not exact.
    pub fn merges(&self, doc: &Document, command: Command) -> Vec<Run> {
        self.merge_request(doc, command)
            .map_or_else(Vec::new, |m| doc.merges(&m))
    }

    /// Carries out `command`'s merges: every run that is exact by moving
    /// what it shows onto the layer it keeps, and every one that is not by
    /// the picture `drawn` holds for it, at its place in [`Editor::merges`].
    /// A run with neither refuses the whole merge, and the board stays as
    /// it was. What is left of the pick is picked: the layer a run kept,
    /// or — once its layer went — the top of the board.
    pub fn merge(&mut self, doc: &mut Document, command: Command, drawn: &[Option<Image>]) -> Change {
        let Some(request) = self.merge_request(doc, command) else {
            return Change::None;
        };
        let runs = doc.merges(&request);
        let pictured = |i: usize| drawn.get(i).and_then(Option::as_ref);
        if !runs.iter().enumerate().all(|(i, r)| doc.exact(r) || pictured(i).is_some()) {
            return Change::None;
        }
        let active = self.active(doc).to_owned();
        let home = runs.iter().find(|r| {
            r.members
                .iter()
                .filter_map(|m| doc.layer(m))
                .any(|m| doc.subtree(m).contains(&active))
        });
        let landed = home.map(|r| r.keep.clone());
        for (i, run) in runs.iter().enumerate() {
            match pictured(i).filter(|_| !doc.exact(run)) {
                Some(picture) => doc.merge_raster(run, picture.clone()),
                None => doc.merge_structural(run),
            }
        }
        let discarded = request == Merge::Flatten && doc.discard_hidden();
        if runs.is_empty() && !discarded {
            return Change::None;
        }
        self.layer = landed.or_else(|| doc.layer(&active).map(|l| l.id.clone()));
        self.picked.clear();
        self.selection.retain(|id| doc.elements.iter().any(|el| el.id() == id));
        self.follow_the_pick(doc);
        Change::Scene
    }

    /// Whether `command` would change anything, found out by doing it to
    /// copies: the one answer that cannot disagree with the doing. A
    /// merge is asked by its runs instead — one that is not exact waits
    /// on a picture `app` can always take.
    pub fn can(&self, doc: &Document, command: Command) -> bool {
        if command.merges() {
            let hidden = command == Command::Flatten && doc.rows(|_| true).iter().any(|r| !r.layer.visible);
            return hidden || !self.merges(doc, command).is_empty();
        }
        let mut doc = doc.clone();
        self.clone().run(&mut doc, command) != Change::None
    }

    /// Tags layer `id` with `tag` — and every picked layer with it, when
    /// it is one of them, as a menu on a picked row speaks for the pick.
    /// A lock does not keep a tag off: it is about the layer, and not
    /// about what the layer holds.
    pub fn set_tag(&mut self, doc: &mut Document, id: &str, tag: Tag) -> Change {
        let picked = self.picked_ids(doc);
        let ids = if picked.iter().any(|p| p == id) {
            picked
        } else {
            vec![id.to_owned()]
        };
        let mut changed = false;
        for id in ids {
            if let Some(l) = doc.layer_mut(&id)
                && l.color != tag
            {
                l.color = tag;
                changed = true;
            }
        }
        if changed { Change::Scene } else { Change::None }
    }

    /// Gives layer `id` the name it was typed. A name that is nothing but
    /// space is not a name, and the layer keeps the one it had — a card
    /// with no word on it can be neither read nor exported under.
    pub fn rename_layer(&mut self, doc: &mut Document, id: &str, name: &str) -> Change {
        let name = name.trim();
        let Some(layer) = doc.layer_mut(id) else {
            return Change::None;
        };
        if name.is_empty() || layer.name == name {
            return Change::None;
        }
        layer.name = name.to_owned();
        Change::Scene
    }

    /// Removes the active layer with everything on it and under it — out
    /// of the selection too — and activates the layer that was above it,
    /// or the one below when it was on top, or the group it leaves empty.
    /// The board and a frame keep their last layer, but not what it holds:
    /// a layer is its object, so the trash takes the object either way.
    pub fn remove_layer(&mut self, doc: &mut Document) -> Change {
        let a = self.active(doc).to_owned();
        let Some((owner, index)) = doc.locate(&a).map(|(o, i)| (o.map(str::to_owned), i)) else {
            return Change::None;
        };
        let owner = owner.as_deref();
        if doc.fixed(owner) {
            return Change::None;
        }
        if !doc.remove_layer(owner, index) {
            match doc.stack(owner).get(index).map(|l| l.kind) {
                // A layer that holds objects stays, emptied.
                Some(Kind::Raster | Kind::Vector) => {
                    if !doc.elements.iter().any(|el| el.layer() == a) {
                        return Change::None;
                    }
                    doc.elements.retain(|el| el.layer() != a);
                    self.drag = None;
                    self.selection
                        .retain(|id| doc.elements.iter().any(|el| el.id() == id));
                    return Change::Scene;
                }
                // One that holds layers goes, and a fresh one keeps the
                // stack from standing empty — as the parse would.
                Some(Kind::Group | Kind::Frame) => {
                    let name = doc.next_layer_name(owner, Kind::Raster);
                    if let Some(layers) = doc.stack_mut(owner) {
                        layers.insert(index + 1, crate::doc::Layer::new(&name));
                    }
                    doc.remove_layer(owner, index);
                }
                None => return Change::None,
            }
        }
        self.drag = None;
        self.selection
            .retain(|id| doc.elements.iter().any(|el| el.id() == id));
        let stack = doc.stack(owner);
        self.picked.clear();
        self.layer = match stack.len() {
            0 => owner.map(str::to_owned),
            n => Some(stack[index.min(n - 1)].id.clone()),
        };
        Change::Scene
    }

    /// Shows or hides layer `id`. What is hidden cannot be seen, so it
    /// cannot stay selected either — hiding a group hides what it holds.
    pub fn toggle_layer(&mut self, doc: &mut Document, id: &str) -> Change {
        let Some(layer) = doc.layer_mut(id) else {
            return Change::None;
        };
        layer.visible = !layer.visible;
        if !layer.visible {
            self.drag = None;
            self.selection.retain(|sel| {
                doc.elements
                    .iter()
                    .any(|el| el.id() == sel && doc.shown(el.layer()))
            });
        }
        Change::Scene
    }

    /// Drops the picked layers at `place` — what a card let go of over
    /// the panel does — keeping their order. They stay picked, and what
    /// holds them now is shown open.
    pub fn drop_layers(&mut self, doc: &mut Document, place: &Place) -> Change {
        let picked: Vec<String> = self.picked(doc).into_iter().map(str::to_owned).collect();
        let Some((owner, index)) = doc.place(place).map(|(o, i)| (o.map(str::to_owned), i)) else {
            return Change::None;
        };
        if !doc.move_layers(&picked, owner.as_deref(), index) {
            return Change::None;
        }
        for id in &picked {
            self.reveal(doc, id);
        }
        Change::Scene
    }

    /// The rows the panel shows: the tree as far as it is open, narrowed
    /// by the filter while its bar is.
    pub fn rows<'a>(&self, doc: &'a Document) -> Vec<tree::Row<'a>> {
        match self.filtering() {
            Some(filter) => doc.rows_matching(|id| self.is_open(id), filter),
            None => doc.rows(|id| self.is_open(id)),
        }
    }

    /// The filter in force: the bar's, while it is open.
    pub fn filtering(&self) -> Option<&Filter> {
        self.filtering.then_some(&self.filter)
    }

    pub fn filter_mut(&mut self) -> &mut Filter {
        &mut self.filter
    }

    /// Opens the filter's bar, or shuts it.
    pub fn toggle_filter(&mut self) -> Change {
        self.filtering = !self.filtering;
        Change::Selection
    }

    /// Whether the panel shows `holder`'s layers under its row.
    pub fn is_open(&self, holder: &str) -> bool {
        self.open.iter().any(|id| id == holder)
    }

    /// Opens `holder`'s row, or shuts it, as asked.
    pub fn set_open(&mut self, holder: &str, open: bool) -> Change {
        if self.is_open(holder) != open {
            return self.toggle_open(holder);
        }
        Change::None
    }

    /// Opens `holder`'s row, or shuts it.
    pub fn toggle_open(&mut self, holder: &str) -> Change {
        match self.open.iter().position(|id| id == holder) {
            Some(i) => {
                self.open.remove(i);
            }
            None => self.open.push(holder.to_owned()),
        }
        Change::Selection
    }

    /// Opens every row holding `layer`, so its own can be seen.
    pub fn reveal(&mut self, doc: &Document, layer: &str) {
        for holder in doc.ancestors(layer) {
            if !self.is_open(&holder.id) {
                self.open.push(holder.id.clone());
            }
        }
    }

    pub fn is_drawing(&self) -> bool {
        self.stroke.is_some() || self.framing.is_some()
    }

    /// Where the hand is standing now.
    pub fn at(&self) -> Spot {
        Spot {
            selection: self.selection.clone(),
            layer: self.layer.clone(),
            picked: self.picked.clone(),
        }
    }

    /// Stands there instead. Nothing physical moves: a spot is where the
    /// work is, and the tool and the held keys belong to the window.
    pub fn go(&mut self, spot: Spot) {
        let Spot {
            selection,
            layer,
            picked,
        } = spot;
        self.selection = selection;
        self.layer = layer;
        self.picked = picked;
    }

    /// Something is in the middle of happening: a stroke, an area being
    /// dragged out, a navigation or a drag. What the document says
    /// mid-gesture is not a state anybody meant to arrive at, which is
    /// why the history does not write it down.
    ///
    /// Wider than [`Editor::is_moving`], which only ever meant a
    /// `Drag::Move` that had already passed the click slop.
    pub fn busy(&self) -> bool {
        self.stroke.is_some()
            || self.framing.is_some()
            || self.nav.is_some()
            || self.drag.is_some()
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
        if self.stroke.is_some()
            || self.nav.is_some()
            || self.drag.is_some()
            || self.framing.is_some()
        {
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
                let born = doc.stack_at([world.0, world.1]).map(str::to_owned);
                self.start_stroke(world, Tip::PENCIL, born)
            }
            (Button::Left, Tool::Brush) if self.refuses_ink(doc, view, screen) => Change::None,
            (Button::Left, Tool::Brush) => {
                let born = doc.stack_at([world.0, world.1]).map(str::to_owned);
                self.start_stroke(world, brush.clone(), born)
            }
            (Button::Left, Tool::Frame) => {
                self.framing = Some(([world.0, world.1], [world.0, world.1]));
                Change::Selection
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
            // A layer belongs to a stack, and its row may be inside a
            // group or a frame that is shut: picking the object opens
            // them, so the panel can show where it is.
            self.reveal(doc, &layer);
            self.layer = Some(layer);
        }
        if self.shift {
            if let Some(i) = self.selection.iter().position(|s| *s == id) {
                self.selection.remove(i);
                self.pick_what_is_selected(doc);
                return Change::Selection;
            }
            self.selection.push(id);
        } else if !self.selection.contains(&id) {
            self.selection = vec![id];
        }
        self.pick_what_is_selected(doc);
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

    /// Lays a frame over the area dragged from `from` to `to`, and
    /// claims what that area covers. A frame drawn over things takes
    /// them: an object visibly inside an area, uncut, would contradict
    /// the boundary the moment it appeared. The centre decides, as it
    /// does on a move.
    fn lay_frame(&mut self, doc: &mut Document, view: &View, from: Point, to: Point) -> Change {
        let lo = [from[0].min(to[0]), from[1].min(to[1])];
        let hi = [from[0].max(to[0]), from[1].max(to[1])];
        let px = view.px_per_world();
        if (hi[0] - lo[0]) * px < MIN_FRAME_PX || (hi[1] - lo[1]) * px < MIN_FRAME_PX {
            return Change::Selection;
        }
        // A frame is always the board's: it does not nest. It goes in
        // above whatever holds the active layer on the board's root.
        let active = self.active(doc).to_owned();
        let root = doc
            .ancestors(&active)
            .last()
            .map_or(active.clone(), |l| l.id.clone());
        let above = doc
            .layers
            .iter()
            .position(|l| l.id == root)
            .unwrap_or(doc.layers.len().saturating_sub(1));
        let Some(at) = doc.add_layer(None, above, Kind::Frame) else {
            return Change::None;
        };
        let layer = doc.layers[at].id.clone();
        let id = new_id();
        doc.elements.push(Element::Frame(crate::doc::Frame {
            id: id.clone(),
            layer: layer.clone(),
            x: lo[0],
            y: lo[1],
            w: hi[0] - lo[0],
            h: hi[1] - lo[1],
            background: Some(self.surface.clone()),
            layers: vec![crate::doc::Layer::new("Layer 1")],
        }));
        let claimed: Vec<String> = doc
            .painted()
            .filter(|p| p.within.is_none() && !matches!(p.element, Element::Frame(_)))
            .filter_map(|p| {
                let c = select::frame(p.element)?.center;
                (c[0] >= lo[0] && c[0] <= hi[0] && c[1] >= lo[1] && c[1] <= hi[1])
                    .then(|| p.element.layer().to_owned())
            })
            .collect();
        for l in claimed {
            doc.rehome_layer(&l, Some(&layer));
        }
        self.selection = vec![id];
        self.layer = Some(layer);
        self.picked.clear();
        Change::Scene
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
            let home = doc.stack_at(f.center).map(str::to_owned);
            let layer = el.layer().to_owned();
            let was = doc.context(&layer).map(str::to_owned);
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
        if let Some((_, to)) = &mut self.framing {
            let world = view.screen_to_world(screen.0, screen.1);
            *to = [world.0, world.1];
            return Change::Selection;
        }
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
                self.pick_what_is_selected(doc);
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
        let born = doc.stack_at([cx, cy]).map(str::to_owned);
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
        self.picked.clear();
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
            // an area and a group holds layers; neither asks for curves,
            // so either can only mean the brush's answer here.
            Kind::Raster | Kind::Frame | Kind::Group => curve::polyline(&hand),
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
            && let Some((from, to)) = self.framing.take()
        {
            return self.lay_frame(doc, view, from, to);
        }
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
            // A raster layer accumulates: the stroke joins the paint on
            // top of it — where it was painted while it was drawn — and
            // opens one on top when the top of the layer is not a paint.
            let top = doc.elements.iter().rposition(|el| el.layer() == layer);
            if let Some(i) = top
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
        let had_area = self.framing.take().is_some();
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
                self.pick_what_is_selected(doc);
                true
            }
            None => false,
        };
        had_stroke || had_area || had_nav || had_drag
    }

    /// Esc: cancels what is in progress, or else clears the selection.
    /// True if anything changed.
    pub fn escape(&mut self, doc: &mut Document) -> bool {
        if self.cancel(doc) {
            return true;
        }
        let had_selection = !self.selection.is_empty();
        self.selection.clear();
        self.picked.clear();
        self.in_panel = false;
        had_selection
    }
}

/// Where a new layer goes once it is kept out of every locked holder:
/// right above the outermost one it would have gone into. A locked group
/// or frame is a stack nothing new goes into.
fn clear_of_locks(doc: &Document, mut owner: Option<String>, mut above: usize) -> (Option<String>, usize) {
    while let Some(o) = owner.clone() {
        if !doc.locked(&o) {
            break;
        }
        match doc.locate(&o) {
            Some((up, i)) => {
                owner = up.map(str::to_owned);
                above = i;
            }
            None => break,
        }
    }
    (owner, above)
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

    /// Where the active layer stands in its own stack — on a board of one
    /// stack, its index in the board's.
    fn active_at(e: &Editor, doc: &Document) -> usize {
        doc.locate(e.active(doc)).expect("the active layer is on the board").1
    }

    /// Picks the board's layer `i`, by its id.
    fn select_at(e: &mut Editor, doc: &Document, i: usize) -> Change {
        let id = doc.layers.get(i).map_or(String::new(), |l| l.id.clone());
        e.pick_layer(doc, &id, Pick::Only, &[])
    }

    /// Shows or hides the board's layer `i`, by its id.
    fn toggle_at(e: &mut Editor, doc: &mut Document, i: usize) -> Change {
        let id = doc.layers.get(i).map_or(String::new(), |l| l.id.clone());
        e.toggle_layer(doc, &id)
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
            ..Layer::of("Layer 2", Kind::Raster)
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
        assert_eq!(active_at(&e, &doc), 2);
        assert_eq!(select_at(&mut e, &doc, 0), Change::Selection);
        assert_eq!(active_at(&e, &doc), 0);
        assert_eq!(e.pick_layer(&doc, "nobody", Pick::Only, &[]), Change::None);
        assert_eq!(active_at(&e, &doc), 0);
        // The layer goes away under the editor: back to the top.
        assert!(doc.remove_layer(None, 0));
        assert_eq!(active_at(&e, &doc), 1);
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
        assert_eq!(active_at(&e, &doc), 1);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(paint_of(&doc, 0).layer, doc.layers[1].id);
        let _ = select_at(&mut e, &doc, 0);
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
        assert_eq!(active_at(&e, &doc), 1, "and is now the active one");
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
        assert_eq!(active_at(&e, &doc), 2);
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
        assert_eq!(active_at(&e, &doc), 1, "so a brush stroke lands over it");
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
        assert_eq!(active_at(&e, &doc), 1);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(active_at(&e, &doc), 0, "a is on L1");
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(active_at(&e, &doc), 1, "b is on L2");
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert_eq!(active_at(&e, &doc), 0, "adding to the selection too");
        e.hold_shift(false);
        // Empty canvas changes nothing.
        let _ = click(&mut e, &v, &mut doc, (50.0, 5.0));
        assert_eq!(active_at(&e, &doc), 0);
    }

    #[test]
    fn add_layer_goes_in_above_the_active_one() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let _ = select_at(&mut e, &doc, 0);
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(active_at(&e, &doc), 1);
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
        assert_eq!(active_at(&e, &doc), 1);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.elements.len(), 1, "b went with L2");
        assert!(e.selection().is_empty());
        assert_eq!(active_at(&e, &doc), 0);
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
        let _ = select_at(&mut e, &doc, 1);
        let _ = e.remove_layer(&mut doc);
        assert_eq!(active_at(&e, &doc), 1);
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
        assert_eq!(toggle_at(&mut e, &mut doc, 1), Change::Scene);
        assert!(!doc.layers[1].visible);
        assert_eq!(e.selection(), ids(&["a"]), "b is hidden with its layer");
        assert_eq!(toggle_at(&mut e, &mut doc, 1), Change::Scene);
        assert!(doc.layers[1].visible);
        assert_eq!(e.selection(), ids(&["a"]));
        assert_eq!(toggle_at(&mut e, &mut doc, 7), Change::None);
    }

    #[test]
    fn a_hidden_layer_cannot_be_picked() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = toggle_at(&mut e, &mut doc, 1);
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert!(e.selection().is_empty());
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

    fn ed_default() -> Editor {
        Editor::default()
    }

    #[test]
    fn renaming_a_layer_writes_the_name_and_asks_for_a_save() {
        let mut doc = Document::new("t");
        let mut ed = Editor::default();
        let id = doc.layers[0].id.clone();
        assert_eq!(ed.rename_layer(&mut doc, &id, "  auth flow  "), Change::Scene);
        assert_eq!(doc.layers[0].name, "auth flow");
    }

    #[test]
    fn a_name_of_nothing_but_space_leaves_the_layer_as_it_was() {
        let mut doc = Document::new("t");
        let was = doc.layers[0].name.clone();
        let id = doc.layers[0].id.clone();
        assert_eq!(ed_default().rename_layer(&mut doc, &id, "   "), Change::None);
        assert_eq!(doc.layers[0].name, was);
    }

    #[test]
    fn renaming_a_layer_that_is_not_there_changes_nothing() {
        let mut doc = Document::new("t");
        assert_eq!(ed_default().rename_layer(&mut doc, "nobody", "x"), Change::None);
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
    fn dock_order_is_select_hand_pencil_brush_frame_zoom() {
        assert_eq!(
            Tool::ALL,
            [
                Tool::Select,
                Tool::Hand,
                Tool::Pencil,
                Tool::Brush,
                Tool::Frame,
                Tool::Zoom
            ]
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
    fn a_layer_inside_a_frame_is_picked_by_its_id_like_any_other() {
        let doc = framed_editor_doc();
        let mut e = Editor::new();
        assert_eq!(e.active(&doc), "fl", "the top of the board's root");
        assert_eq!(e.pick_layer(&doc, "in", Pick::Only, &[]), Change::Selection);
        assert_eq!(e.active(&doc), "in");
        assert_eq!(e.pick_layer(&doc, "nobody", Pick::Only, &[]), Change::None);
        assert_eq!(e.active(&doc), "in");
    }

    #[test]
    fn a_layer_added_with_a_frame_active_goes_into_its_stack_on_top() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "fl", Pick::Only, &[]);
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.stack(Some("fl")).len(), 2);
        assert_eq!(doc.layers.len(), 1, "the board's root did not grow");
        assert_eq!(doc.stack(Some("fl"))[1].id, e.active(&doc), "on top, and active");
        assert!(e.is_open("fl"), "and its row can be seen");
    }

    #[test]
    fn a_layer_added_with_a_layer_in_a_frame_active_goes_right_above_it() {
        let mut doc = framed_editor_doc();
        doc.add_layer(Some("fl"), 0, Kind::Raster).unwrap();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "in", Pick::Only, &[]);
        let _ = e.add_layer(&mut doc);
        let stack = doc.stack(Some("fl"));
        assert_eq!(stack.len(), 3);
        assert_eq!(stack[0].id, "in");
        assert_eq!(stack[1].id, e.active(&doc));
    }

    #[test]
    fn a_group_added_with_a_group_active_goes_inside_it() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "H", Pick::Only, &[]);
        assert_eq!(e.add_group(&mut doc), Change::Scene);
        let new = e.active(&doc).to_owned();
        assert_eq!(doc.locate(&new), Some((Some("H"), 1)), "on top of H's own");
        let group = doc.layer(&new).unwrap();
        assert_eq!(group.kind, Kind::Group);
        assert_eq!(group.name, "Group 3", "numbered across the board's tree");
    }

    #[test]
    fn the_active_layer_falls_back_to_the_top_when_its_layer_is_gone() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "in", Pick::Only, &[]);
        doc.elements.retain(|el| el.id() != "fr");
        doc.layers.clear();
        doc.layers.push(crate::doc::Layer::new("Layer 1"));
        assert_eq!(e.active(&doc), doc.layers[0].id);
    }

    /// The frame's own layer takes the frame, its stack and everything on
    /// it.
    #[test]
    fn removing_a_frame_layer_takes_the_frame_and_what_it_holds() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, crate::doc::Layer::new("Layer 1"));
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "fl", Pick::Only, &[]);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert!(doc.frame("fr").is_none());
        assert!(doc.layer("in").is_none());
        assert_eq!(e.active(&doc), doc.layers[0].id, "the one left below");
    }

    /// A board keeps a layer: the only one it has going, when that one
    /// holds layers, leaves a fresh one behind rather than a frame layer
    /// with no frame on it.
    #[test]
    fn removing_the_boards_only_frame_leaves_a_fresh_layer() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "fl", Pick::Only, &[]);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert!(doc.frame("fr").is_none());
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].kind, Kind::Raster);
        Document::from_json(&doc.to_json().unwrap()).expect("and it is still a board");
    }

    #[test]
    fn removing_a_groups_last_layer_leaves_the_group_empty_and_active() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "C", Pick::Only, &[]);
        assert_eq!(e.remove_layer(&mut doc), Change::Scene);
        assert!(doc.layer("H").unwrap().layers.is_empty(), "a group may stand empty");
        assert_eq!(e.active(&doc), "H");
    }

    #[test]
    fn toggling_a_layer_inside_a_frame_hides_that_one() {
        let mut doc = framed_editor_doc();
        let mut e = Editor::new();
        assert_eq!(e.toggle_layer(&mut doc, "in"), Change::Scene);
        assert!(!doc.layer("in").unwrap().visible);
        assert!(doc.layers[0].visible, "the frame's own card is untouched");
    }

    #[test]
    fn hiding_a_group_lets_go_of_what_it_holds() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        e.go(Spot {
            selection: vec!["c".into(), "a".into()],
            ..Spot::default()
        });
        let _ = e.toggle_layer(&mut doc, "G");
        assert_eq!(e.selection(), ["a"], "c is in G's group H");
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
        doc.context(paint.layer()).map(str::to_owned)
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
        assert_eq!(paint_stack(&doc).as_deref(), Some("fl"));
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
        assert_eq!(paint_stack(&doc).as_deref(), Some("fl"));
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
            live.as_deref().and_then(|l| doc.context(l)),
            Some("fl"),
            "the live stroke is painted in the frame the press was in"
        );
        let _ = e.moved(&v, at(&v, 40.0, 40.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 40.0, 40.0), &mut doc, "#111");
        assert_eq!(paint_stack(&doc).as_deref(), Some("fl"));
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
        assert_eq!(doc.context(path.layer()), Some("fl"));
        assert_eq!(doc.stack(Some("fl")).len(), 2, "its own layer, in the frame");
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
            doc.context(&layer),
            Some("fl"),
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
        assert_eq!(doc.context("in"), None, "the layer came out");
    }

    #[test]
    fn a_frame_itself_never_rehomes() {
        let mut doc = framed_editor_doc();
        doc.layers.insert(0, Layer::new("Layer 1"));
        doc.layers.push(Layer {
            id: "fl2".into(),
            ..Layer::of("Frame 2", Kind::Frame)
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
            doc.context("fl"),
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
        assert_eq!(doc.context(&layer), None);
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
        assert_eq!(e.active(&doc), "in", "its layer, in the frame");
        assert!(e.is_open("fl"), "and the frame's row is open to show it");
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
        assert_eq!(e.active(&doc), layer);
    }

    #[test]
    fn the_frame_tool_is_in_the_dock_and_answers_to_f() {
        assert_eq!(Tool::from_hotkey('f'), Some(Tool::Frame));
        assert_eq!(Tool::from_hotkey('F'), Some(Tool::Frame));
        assert!(Tool::ALL.contains(&Tool::Frame));
    }

    fn the_frame(doc: &Document) -> &crate::doc::Frame {
        doc.elements
            .iter()
            .find_map(|el| match el {
                Element::Frame(f) => Some(f),
                _ => None,
            })
            .expect("a frame landed")
    }

    #[test]
    fn dragging_the_frame_tool_makes_a_frame_with_its_own_stack() {
        let mut doc = Document::new("t");
        let mut e = Editor::new();
        e.set_surface("#fbfbfa");
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 200.0, 150.0), &mut doc);
        assert!(e.framing().is_some(), "the area shows while it is dragged");
        let _ = e.release(Button::Left, &v, at(&v, 200.0, 150.0), &mut doc, "#111");

        let f = the_frame(&doc);
        assert_eq!((f.x, f.y, f.w, f.h), (0.0, 0.0, 200.0, 150.0));
        assert_eq!(f.background.as_deref(), Some("#fbfbfa"));
        assert_eq!(f.layers.len(), 1, "with a stack of its own");
        assert_eq!(doc.layers.last().unwrap().kind, Kind::Frame);
        assert_eq!(e.selection(), std::slice::from_ref(&f.id), "and it is selected");
        assert!(e.framing().is_none());
    }

    /// Dragged the other way, it is the same area.
    #[test]
    fn a_frame_dragged_up_and_left_spans_the_same_area() {
        let mut doc = Document::new("t");
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 200.0, 150.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 0.0, 0.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, "#111");
        let f = the_frame(&doc);
        assert_eq!((f.x, f.y, f.w, f.h), (0.0, 0.0, 200.0, 150.0));
    }

    #[test]
    fn a_click_with_the_frame_tool_makes_nothing() {
        let mut doc = Document::new("t");
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.release(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, "#111");
        assert!(
            !doc.elements.iter().any(|el| matches!(el, Element::Frame(_))),
            "a click is not an area"
        );
    }

    #[test]
    fn a_new_frame_claims_what_its_area_covers() {
        let mut doc = Document::new("t");
        let board = doc.layers[0].id.clone();
        doc.elements.push(Element::Rect(Rect {
            id: "under".into(),
            layer: board.clone(),
            x: 40.0,
            y: 40.0,
            w: 20.0,
            h: 20.0,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }));
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 200.0, 150.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 200.0, 150.0), &mut doc, "#111");
        let layer = the_frame(&doc).layer.clone();
        assert_eq!(
            doc.context(&board),
            Some(layer.as_str()),
            "an object visibly inside the area is in it"
        );
    }

    #[test]
    fn a_new_frame_leaves_what_is_outside_it_alone() {
        let mut doc = Document::new("t");
        let board = doc.layers[0].id.clone();
        doc.elements.push(Element::Rect(Rect {
            id: "away".into(),
            layer: board.clone(),
            x: 900.0,
            y: 900.0,
            w: 20.0,
            h: 20.0,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }));
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 200.0, 150.0), &mut doc);
        let _ = e.release(Button::Left, &v, at(&v, 200.0, 150.0), &mut doc, "#111");
        assert_eq!(doc.context(&board), None);
    }

    #[test]
    fn escape_drops_the_area_being_dragged() {
        let mut doc = Document::new("t");
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 200.0, 150.0), &mut doc);
        assert!(e.escape(&mut doc));
        assert!(e.framing().is_none());
        assert!(!doc.elements.iter().any(|el| matches!(el, Element::Frame(_))));
    }

    /// Switching tools cancels what is in progress, as it does a stroke.
    #[test]
    fn taking_another_tool_drops_the_area_being_dragged() {
        let mut doc = Document::new("t");
        let mut e = Editor::new();
        e.set_tool(Tool::Frame, &mut doc);
        let v = view();
        let _ = e.press(Button::Left, &v, at(&v, 0.0, 0.0), &mut doc, &brush().tip(Face::Round));
        let _ = e.moved(&v, at(&v, 200.0, 150.0), &mut doc);
        e.set_tool(Tool::Select, &mut doc);
        assert!(e.framing().is_none());
        assert!(!doc.elements.iter().any(|el| matches!(el, Element::Frame(_))));
    }

    #[test]
    fn a_spot_is_what_is_selected_and_the_ink_layer() {
        let mut e = Editor::new();
        e.selection = vec!["a".into()];
        e.layer = Some("L1".into());
        let spot = e.at();
        assert_eq!(spot.selection, ["a"]);
        assert_eq!(spot.layer.as_deref(), Some("L1"));
    }

    #[test]
    fn going_to_a_spot_puts_it_all_back() {
        let mut e = Editor::new();
        e.selection = vec!["a".into()];
        e.layer = Some("L1".into());
        let there = e.at();

        let mut other = Editor::new();
        other.selection = vec!["z".into(), "y".into()];
        other.layer = Some("L9".into());
        other.go(there.clone());
        assert_eq!(other.at(), there);
    }

    #[test]
    fn going_to_a_spot_leaves_the_hand_alone() {
        // A spot is where the work is, not what the hand is doing: the
        // tool and the keys held down are physical and belong to the
        // window, so a step backwards must not move them.
        let mut e = tool(Tool::Brush);
        e.hold_shift(true);
        e.go(Spot::default());
        assert_eq!(e.tool(), Tool::Brush);
        assert!(e.shift);
    }

    /// The rows of [`crate::tree::tests::nested`] with `G` open, top
    /// first: what `Shift` ranges over.
    const ROWS: [&str; 5] = ["F", "G", "H", "B", "A"];

    fn picked(e: &Editor, doc: &Document) -> Vec<String> {
        e.picked(doc).into_iter().map(str::to_owned).collect()
    }

    #[test]
    fn a_plain_pick_takes_one_layer_and_makes_it_active() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        assert_eq!(picked(&e, &doc), ["F"], "nothing picked is the active layer alone");
        assert_eq!(e.pick_layer(&doc, "B", Pick::Only, &ROWS), Change::Selection);
        assert_eq!(picked(&e, &doc), ["B"]);
        assert_eq!(e.active(&doc), "B");
        assert_eq!(e.pick_layer(&doc, "nobody", Pick::Only, &ROWS), Change::None);
        assert_eq!(picked(&e, &doc), ["B"]);
    }

    #[test]
    fn ctrl_takes_a_layer_in_and_out_of_the_pick() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "C", Pick::Toggle, &ROWS);
        assert_eq!(picked(&e, &doc), ["A", "C"]);
        assert_eq!(e.active(&doc), "C", "the one just taken is where ink goes");
        let _ = e.pick_layer(&doc, "A", Pick::Toggle, &ROWS);
        assert_eq!(picked(&e, &doc), ["C"]);
        // The last one stays: there is always a layer to paint on.
        let _ = e.pick_layer(&doc, "C", Pick::Toggle, &ROWS);
        assert_eq!(picked(&e, &doc), ["C"]);
    }

    #[test]
    fn shift_takes_the_rows_between_the_anchor_and_the_press() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "B", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "F", Pick::Range, &ROWS);
        assert_eq!(picked(&e, &doc), ["F", "G", "H", "B"], "in row order, both ends in");
        assert_eq!(e.active(&doc), "B", "the anchor stays where it was");
        // Again from the same anchor, the other way: the range is
        // measured from it, not added to.
        let _ = e.pick_layer(&doc, "A", Pick::Range, &ROWS);
        assert_eq!(picked(&e, &doc), ["B", "A"]);
    }

    #[test]
    fn with_the_select_tool_a_picked_layer_selects_its_objects() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        // A group's are everything under it, in paint order.
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert_eq!(e.selection(), ["b", "c"]);
        // A frame's is the frame, which carries what it holds.
        let _ = e.pick_layer(&doc, "F", Pick::Only, &ROWS);
        assert_eq!(e.selection(), ["fr"]);
        let _ = e.pick_layer(&doc, "A", Pick::Toggle, &ROWS);
        assert_eq!(e.selection(), ["a", "fr"]);
        // What is hidden or locked is not there to be selected.
        doc.layer_mut("H").unwrap().visible = false;
        doc.layer_mut("B").unwrap().locked = true;
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert!(e.selection().is_empty(), "{:?}", e.selection());
    }

    #[test]
    fn with_a_painting_tool_a_pick_leaves_the_canvas_alone() {
        let mut doc = crate::tree::tests::nested();
        let mut e = tool(Tool::Brush);
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        assert!(e.selection().is_empty());
        assert_eq!(e.active(&doc), "A", "the layer the next stroke joins");
        let _ = &mut doc;
    }

    #[test]
    fn picking_objects_on_the_canvas_picks_their_layers() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        e.hold_shift(true);
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        e.hold_shift(false);
        assert_eq!(picked(&e, &doc), ["L1", "L2"]);
        assert_eq!(e.active(&doc), "L2", "the last one picked");
        // A marquee picks what it catches.
        let _ = e.escape(&mut doc);
        let _ = drag(&mut e, &v, &mut doc, (1.0, 1.0), (99.0, 99.0));
        assert_eq!(picked(&e, &doc), ["L1", "L2"]);
    }

    #[test]
    fn letting_go_of_the_selection_keeps_the_active_layer() {
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (15.0, 15.0));
        assert!(e.escape(&mut doc));
        assert_eq!(picked(&e, &doc), ["L1"], "the active layer, and nothing else");
    }

    #[test]
    fn a_spot_keeps_the_pick() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "C", Pick::Toggle, &ROWS);
        let there = e.at();
        let mut other = Editor::new();
        other.go(there);
        assert_eq!(picked(&other, &doc), ["A", "C"]);
    }

    #[test]
    fn dropping_the_picked_layers_moves_them_all_and_opens_where_they_went() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "B", Pick::Toggle, &ROWS);
        assert_eq!(e.drop_layers(&mut doc, &Place::Into("F".into())), Change::Scene);
        assert_eq!(doc.context("A"), Some("F"));
        assert_eq!(doc.context("B"), Some("F"));
        assert_eq!(picked(&e, &doc), ["A", "B"], "still picked where they landed");
        assert!(e.is_open("F"), "and the frame shows them");
        assert_eq!(
            e.drop_layers(&mut doc, &Place::Into("A".into())),
            Change::None,
            "a layer holds none"
        );
    }

    #[test]
    fn grouping_the_picked_layers_picks_the_group() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "B", Pick::Toggle, &ROWS);
        assert_eq!(e.group_layers(&mut doc), Change::Scene);
        let g = e.active(&doc).to_owned();
        assert_eq!(doc.layer(&g).unwrap().kind, Kind::Group);
        assert_eq!(picked(&e, &doc), [g]);
        assert!(e.is_open("G"), "where it stands can be seen");
        // A frame is never grouped.
        let _ = e.pick_layer(&doc, "F", Pick::Only, &ROWS);
        assert_eq!(e.group_layers(&mut doc), Change::None);
    }

    #[test]
    fn ungrouping_picks_what_the_group_held() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert_eq!(e.ungroup(&mut doc), Change::Scene);
        assert_eq!(picked(&e, &doc), ["B", "H"]);
        assert_eq!(e.active(&doc), "H");
        assert_eq!(e.ungroup(&mut doc), Change::Scene, "H is a group too");
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        assert_eq!(e.ungroup(&mut doc), Change::None, "a layer is no group");
    }

    #[test]
    fn duplicating_picks_the_copies() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        assert_eq!(e.duplicate_layers(&mut doc), Change::Scene);
        let copy = e.active(&doc).to_owned();
        assert_ne!(copy, "A");
        assert_eq!(doc.locate(&copy), Some((None, 1)), "right above A");
        assert_eq!(picked(&e, &doc), [copy]);
        assert_eq!(e.selection().len(), 1, "and, with the Select tool, what it holds");
    }

    #[test]
    fn removing_the_picked_layers_takes_them_all() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "H", Pick::Toggle, &ROWS);
        assert_eq!(e.remove_layers(&mut doc), Change::Scene);
        assert!(doc.layer("A").is_none() && doc.layer("H").is_none() && doc.layer("C").is_none());
        assert!(!doc.elements.iter().any(|el| el.id() == "a" || el.id() == "c"));
        assert!(e.selection().is_empty());
        assert!(doc.layer(e.active(&doc)).is_some(), "the active layer is one that is left");
        Document::from_json(&doc.to_json().unwrap()).expect("still a board");
    }

    #[test]
    fn delete_takes_what_was_picked_where_it_was_picked() {
        // Picked in the panel: the layers go, holders and all.
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert_eq!(e.delete(&mut doc), Change::Scene);
        assert!(doc.layer("G").is_none(), "the group went, not only what it held");
        // Picked on the canvas: the objects go, and a layer they empty.
        let mut e = Editor::new();
        let mut doc = layered_board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        assert_eq!(e.delete(&mut doc), Change::Scene);
        assert!(doc.layer("L2").is_none());
        assert!(doc.layer("L1").is_some(), "the layer of what was not selected stays");
    }

    #[test]
    fn arranging_moves_the_picked_layers_in_their_stacks() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        assert_eq!(e.arrange(&mut doc, Arrange::Front), Change::Scene);
        assert_eq!(doc.locate("A"), Some((None, 2)));
        assert_eq!(e.arrange(&mut doc, Arrange::Forward), Change::None, "already on top");
        assert_eq!(e.arrange(&mut doc, Arrange::Backward), Change::Scene);
        assert_eq!(doc.locate("A"), Some((None, 1)));
    }

    #[test]
    fn a_brush_stroke_on_a_locked_layer_is_refused_before_it_starts() {
        let mut doc = grouped_board();
        doc.layer_mut("R").unwrap().locked = true;
        let mut e = tool(Tool::Brush);
        let _ = e.pick_layer(&doc, "R", Pick::Only, &[]);
        let v = view();
        assert!(e.refuses_ink(&doc, &v, (5.0, 5.0)), "the cursor can say so first");
        let tip = brush().tip(Face::Round);
        assert_eq!(e.press(Button::Left, &v, (5.0, 5.0), &mut doc, &tip), Change::None);
        assert!(e.stroke().is_none());
        // A locked group locks the layer it holds all the same.
        doc.layer_mut("R").unwrap().locked = false;
        doc.layer_mut("G").unwrap().locked = true;
        assert!(e.refuses_ink(&doc, &v, (5.0, 5.0)));
    }

    #[test]
    fn new_ink_never_opens_a_layer_inside_a_locked_group() {
        let mut doc = grouped_board();
        doc.layer_mut("G").unwrap().locked = true;
        let mut e = pencil();
        let _ = e.pick_layer(&doc, "R", Pick::Only, &[]);
        let v = view();
        assert!(!e.refuses_ink(&doc, &v, (5.0, 5.0)), "a pencil opens a layer of its own");
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(doc.layer("G").unwrap().layers.len(), 1, "nothing went into the locked group");
        assert_eq!(doc.locate(e.active(&doc)), Some((None, 1)), "it opened right above it");
    }

    #[test]
    fn a_layer_added_with_a_locked_group_active_opens_above_it() {
        let mut doc = grouped_board();
        doc.layer_mut("G").unwrap().locked = true;
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "G", Pick::Only, &[]);
        assert_eq!(e.add_layer(&mut doc), Change::Scene);
        assert_eq!(doc.locate(e.active(&doc)), Some((None, 1)));
    }

    #[test]
    fn locking_the_picked_layers_lets_go_of_what_they_hold() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        assert_eq!(e.selection(), ["a"]);
        assert_eq!(e.toggle_lock(&mut doc), Change::Scene);
        assert!(doc.layer("A").unwrap().locked);
        assert!(e.selection().is_empty(), "a locked object is not held");
        let _ = e.toggle_lock(&mut doc);
        assert!(!doc.layer("A").unwrap().locked);
        // Several at once: all locked while any is open, all opened once
        // every one is locked.
        let _ = e.pick_layer(&doc, "B", Pick::Toggle, &ROWS);
        doc.layer_mut("B").unwrap().locked = true;
        let _ = e.toggle_lock(&mut doc);
        assert!(doc.layer("A").unwrap().locked && doc.layer("B").unwrap().locked);
        let _ = e.toggle_lock(&mut doc);
        assert!(!doc.layer("A").unwrap().locked && !doc.layer("B").unwrap().locked);
    }

    #[test]
    fn hiding_the_picked_layers_hides_them_all_or_shows_them_all() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "B", Pick::Toggle, &ROWS);
        doc.layer_mut("B").unwrap().visible = false;
        assert_eq!(e.toggle_shown(&mut doc), Change::Scene);
        assert!(!doc.layer("A").unwrap().visible && !doc.layer("B").unwrap().visible);
        assert!(e.selection().is_empty(), "what is hidden is not held");
        let _ = e.toggle_shown(&mut doc);
        assert!(doc.layer("A").unwrap().visible && doc.layer("B").unwrap().visible);
    }

    #[test]
    fn a_layer_in_a_locked_group_is_not_removed() {
        let mut doc = grouped_board();
        doc.layer_mut("G").unwrap().locked = true;
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "R", Pick::Only, &[]);
        assert_eq!(e.remove_layers(&mut doc), Change::None);
        assert!(doc.layer("R").is_some());
        // The locked group itself goes, from a stack that is free.
        let _ = e.pick_layer(&doc, "G", Pick::Only, &[]);
        assert_eq!(e.remove_layers(&mut doc), Change::Scene);
        assert!(doc.layer("G").is_none());
    }

    #[test]
    fn a_brush_stroke_joins_the_paint_on_top_of_its_layer_or_goes_on_top() {
        // A layer holding a paint under an image: the stroke lands over
        // the image, where it was painted while it was drawn — not in the
        // paint under it.
        let mut doc = Document::new("t");
        let layer = doc.layers[0].id.clone();
        let mut e = tool(Tool::Brush);
        let v = view();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        doc.elements.push(Element::Image(Image {
            id: "img".into(),
            layer: layer.clone(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            rotation: 0.0,
            blob: BLOB.into(),
        }));
        let _ = drag(&mut e, &v, &mut doc, (1.0, 5.0), (9.0, 5.0));
        let on: Vec<&Element> = doc.elements.iter().filter(|el| el.layer() == layer).collect();
        assert_eq!(on.len(), 3, "a paint of its own, over the image");
        assert!(matches!(on[2], Element::Paint(p) if p.strokes.len() == 1));
        assert!(matches!(on[0], Element::Paint(p) if p.strokes.len() == 1));
        // And the next stroke joins that one, the paint on top.
        let _ = drag(&mut e, &v, &mut doc, (1.0, 8.0), (9.0, 8.0));
        let on: Vec<&Element> = doc.elements.iter().filter(|el| el.layer() == layer).collect();
        assert!(matches!(on[2], Element::Paint(p) if p.strokes.len() == 2));
    }

    #[test]
    fn the_picked_layers_take_a_strength_and_a_locked_one_keeps_its_own() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "G", Pick::Toggle, &ROWS);
        doc.layer_mut("G").unwrap().locked = true;
        assert_eq!(e.set_opacity(&mut doc, 0.4), Change::Scene);
        assert_eq!(doc.layer("A").unwrap().opacity, 0.4);
        assert_eq!(doc.layer("G").unwrap().opacity, 1.0, "a lock keeps how a layer draws");
        assert_eq!(e.set_opacity(&mut doc, 0.4), Change::None, "already so");
        let _ = e.set_opacity(&mut doc, 7.0);
        assert_eq!(doc.layer("A").unwrap().opacity, 1.0, "a strength is a fraction");
        let _ = e.set_opacity(&mut doc, f64::NAN);
        assert_eq!(doc.layer("A").unwrap().opacity, 1.0, "and a number");
    }

    #[test]
    fn the_picked_layers_take_a_blend_mode_and_only_a_group_passes_through() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "G", Pick::Toggle, &ROWS);
        assert_eq!(e.set_blend(&mut doc, BlendMode::Screen), Change::Scene);
        assert_eq!(doc.layer("A").unwrap().blend, BlendMode::Screen);
        assert_eq!(doc.layer("G").unwrap().blend, BlendMode::Screen);
        assert_eq!(e.set_blend(&mut doc, BlendMode::PassThrough), Change::Scene);
        assert_eq!(doc.layer("G").unwrap().blend, BlendMode::PassThrough);
        assert_eq!(doc.layer("A").unwrap().blend, BlendMode::Screen, "a layer does not pass through");
        doc.layer_mut("A").unwrap().locked = true;
        let _ = e.set_blend(&mut doc, BlendMode::Multiply);
        assert_eq!(doc.layer("A").unwrap().blend, BlendMode::Screen, "a lock keeps how it draws");
        assert_eq!(e.set_blend(&mut doc, BlendMode::Multiply), Change::None, "already so");
    }

    #[test]
    fn a_command_runs_and_can_says_whether_it_would_change_anything() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "B", Pick::Only, &ROWS);
        let (before, at) = (doc.clone(), e.at());
        assert!(e.can(&doc, Command::Group));
        assert!(e.can(&doc, Command::Duplicate));
        assert!(!e.can(&doc, Command::Ungroup), "B is no group");
        assert_eq!((&doc, e.at()), (&before, at), "asking changes nothing");
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert!(e.can(&doc, Command::Ungroup));
        assert_eq!(e.run(&mut doc, Command::Ungroup), Change::Scene);
        assert!(doc.layer("G").is_none());
        // Inside a locked group nothing comes or goes.
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("H").unwrap().locked = true;
        let _ = e.pick_layer(&doc, "C", Pick::Only, &ROWS);
        assert!(!e.can(&doc, Command::Duplicate));
        assert!(!e.can(&doc, Command::Group));
        assert!(e.can(&doc, Command::Show), "what shows is still the layer's");
        assert_eq!(e.run(&mut doc, Command::Show), Change::Scene);
        assert!(!doc.layer("C").unwrap().visible);
        assert_eq!(e.run(&mut doc, Command::Lock), Change::Scene);
        assert!(doc.layer("C").unwrap().locked);
        assert_eq!(e.run(&mut doc, Command::Arrange(Arrange::Front)), Change::None, "alone in its stack");
    }

    #[test]
    fn copy_cut_and_paste_carry_the_pick_and_land_where_they_are_seen() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "B", Pick::Only, &ROWS);
        let clip = e.copy(&doc).expect("B to copy");
        assert_eq!(clip.layers.len(), 1);
        let (cut, change) = e.cut(&mut doc);
        assert_eq!(change, Change::Scene);
        assert!(doc.layer("B").is_none(), "a cut takes it away");
        let cut = cut.expect("and keeps it");
        // What the clip covers is on show: it lands in place.
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let seen = ([-100.0, -100.0], [100.0, 100.0]);
        assert_eq!(e.paste(&mut doc, &cut, seen), Change::Scene);
        let active = e.active(&doc).to_owned();
        assert_eq!(e.picked(&doc), [active.as_str()], "what came in is picked");
        assert_eq!(doc.layers[1].id, active, "above the active layer");
        let rect_on = |doc: &Document, layer: &str| -> (f64, f64) {
            match doc.elements.iter().find(|el| el.layer() == layer) {
                Some(Element::Rect(r)) => (r.x + r.w / 2.0, r.y + r.h / 2.0),
                other => panic!("a rect, not {other:?}"),
            }
        };
        assert_eq!(rect_on(&doc, &active), (0.5, 0.5), "in place");
        // Out of sight: into the middle of what is on show.
        let far = ([1000.0, 1000.0], [1200.0, 1100.0]);
        let _ = e.paste(&mut doc, &cut, far);
        let active = e.active(&doc).to_owned();
        assert_eq!(rect_on(&doc, &active), (1100.0, 1050.0), "centred");
        assert_eq!(e.copy(&Document::new("t")).map(|c| c.layers.len()), Some(1), "the lone layer");
    }

    #[test]
    fn merging_goes_down_or_takes_the_picked_or_a_group_and_picks_what_is_left() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        assert_eq!(e.merge_name(&doc), "Merge Group");
        assert!(e.can(&doc, Command::Merge));
        assert_eq!(e.run(&mut doc, Command::Merge), Change::Scene);
        let g = doc.layer("G").unwrap();
        assert_eq!((g.kind, g.layers.len()), (Kind::Raster, 0));
        assert_eq!(e.active(&doc), "G");
        // A layer alone goes down into the one under it.
        assert_eq!(e.merge_name(&doc), "Merge Down");
        assert_eq!(e.run(&mut doc, Command::Merge), Change::Scene);
        assert!(doc.layer("G").is_none());
        assert_eq!(e.active(&doc), "A", "what is left is picked");
        let on_a: Vec<&str> = doc.elements.iter().filter(|el| el.layer() == "A").map(|el| el.id()).collect();
        assert_eq!(on_a, ["a", "b", "c"]);
        // Several picked: they merge into the topmost.
        let mut doc = crate::tree::tests::nested();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "G", Pick::Toggle, &ROWS);
        assert_eq!(e.merge_name(&doc), "Merge Layers");
        assert_eq!(e.run(&mut doc, Command::Merge), Change::Scene);
        assert_eq!(doc.layers.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["G", "F"]);
        // What is not exact wants a picture: without one nothing is done,
        // though it can be — `app` takes the picture.
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("A").unwrap().opacity = 0.5;
        let _ = e.pick_layer(&doc, "G", Pick::Only, &ROWS);
        let _ = e.run(&mut doc, Command::Merge);
        let before = doc.clone();
        assert_eq!(e.run(&mut doc, Command::Merge), Change::None);
        assert_eq!(doc, before);
        assert!(e.can(&doc, Command::Merge));
        let runs = e.merges(&doc, Command::Merge);
        assert_eq!(runs.len(), 1);
        assert!(!doc.exact(&runs[0]));
        let picture = crate::doc::Image {
            id: "pic".into(),
            layer: String::new(),
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            rotation: 0.0,
            blob: "a".repeat(64),
        };
        assert_eq!(e.merge(&mut doc, Command::Merge, &[Some(picture)]), Change::Scene);
        let a = doc.layer("A").unwrap();
        assert_eq!(a.opacity, 1.0, "the picture already is what it drew");
        let on_a: Vec<&str> = doc.elements.iter().filter(|el| el.layer() == "A").map(|el| el.id()).collect();
        assert_eq!(on_a, ["pic"]);
        assert_eq!(e.active(&doc), "A");
    }

    #[test]
    fn flatten_merges_what_shows_and_drops_what_is_hidden() {
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("B").unwrap().visible = false;
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "B", Pick::Only, &ROWS);
        assert!(e.can(&doc, Command::MergeVisible));
        assert_eq!(e.run(&mut doc, Command::Flatten), Change::Scene);
        assert_eq!(doc.layers.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["G", "F"]);
        assert_eq!(doc.stack(Some("F")).iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["K"]);
        assert!(doc.elements.iter().all(|el| el.id() != "b"), "hidden, and gone");
        assert_eq!(e.active(&doc), "G", "its layer went with the group it stood in, which is left");
        assert!(!e.can(&doc, Command::Flatten), "flat already");
    }

    #[test]
    fn layers_are_picked_by_id_and_every_id_has_to_be_there() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        assert_eq!(e.pick_ids(&doc, &["C".into(), "A".into()]), Ok(()));
        assert_eq!(e.picked(&doc), ["C", "A"]);
        assert_eq!(e.active(&doc), "A", "the last one named leads");
        let before = e.at();
        assert!(e.pick_ids(&doc, &["A".into(), "nope".into()]).unwrap_err().contains("nope"));
        assert_eq!(e.at(), before, "a refusal picks nothing");
        assert!(e.pick_ids(&doc, &[]).is_err(), "nothing named is nothing to act on");
    }

    #[test]
    fn a_listing_is_the_whole_tree_top_first_with_the_pick() {
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("H").unwrap().visible = false;
        doc.layer_mut("A").unwrap().opacity = 0.25;
        let mut e = Editor::new();
        e.pick_ids(&doc, &["B".into(), "C".into()]).unwrap();
        let list = e.listing(&doc);
        let ids: Vec<&str> = list.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["F", "K", "E", "D", "G", "H", "C", "B", "A"], "shut rows too");
        let c = list.iter().find(|l| l.id == "C").unwrap();
        assert_eq!((c.owner.as_deref(), c.depth, c.visible, c.shown), (Some("H"), 2, true, false));
        assert!(c.active && c.picked);
        let b = list.iter().find(|l| l.id == "B").unwrap();
        assert!(!b.active && b.picked);
        assert_eq!(list.iter().find(|l| l.id == "A").unwrap().opacity, 0.25);
        assert_eq!(list.iter().find(|l| l.id == "F").unwrap().elements, 1, "its frame");
    }

    #[test]
    fn the_picked_layers_are_shown_hidden_locked_and_opened_as_asked() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        e.pick_ids(&doc, &["A".into(), "B".into()]).unwrap();
        assert_eq!(e.show_layers(&mut doc, false), Change::Scene);
        assert!(!doc.layer("A").unwrap().visible && !doc.layer("B").unwrap().visible);
        assert_eq!(e.show_layers(&mut doc, false), Change::None, "already so: not a toggle");
        assert_eq!(e.show_layers(&mut doc, true), Change::Scene);
        assert_eq!(e.lock_layers(&mut doc, true), Change::Scene);
        assert!(doc.layer("A").unwrap().locked && doc.layer("B").unwrap().locked);
        assert_eq!(e.lock_layers(&mut doc, true), Change::None);
        assert!(e.is_open("G"), "it holds what was picked");
        assert!(!e.is_open("H"));
        let _ = e.set_open("H", true);
        let _ = e.set_open("H", true);
        assert!(e.is_open("H"), "opened, and not shut again");
        let _ = e.set_open("H", false);
        assert!(!e.is_open("H"));
    }

    #[test]
    fn what_a_lock_keeps_is_named_before_anything_is_asked_of_it() {
        let mut doc = crate::tree::tests::nested();
        doc.layer_mut("H").unwrap().locked = true;
        let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // How it draws: a lock of its own or a holder's keeps it.
        assert_eq!(locked_among(&doc, &ids(&["A", "C"]), Keeps::Look), Some("C".into()));
        assert_eq!(locked_among(&doc, &ids(&["A", "H"]), Keeps::Look), Some("H".into()));
        assert_eq!(locked_among(&doc, &ids(&["A", "B"]), Keeps::Look), None);
        // Where it stands: only a locked holder keeps that.
        assert_eq!(locked_among(&doc, &ids(&["H"]), Keeps::Place), None, "its own lock does not");
        assert_eq!(locked_among(&doc, &ids(&["C"]), Keeps::Place), Some("C".into()));
    }

    #[test]
    fn a_layer_is_added_where_it_is_asked_under_the_name_it_is_given() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        assert_eq!(e.add_layer_as(&mut doc, false, Some("Sky"), Some("A")), Ok(Change::Scene));
        let sky = e.active(&doc).to_owned();
        assert_eq!(doc.layer(&sky).unwrap().name, "Sky");
        assert_eq!(doc.layers[1].id, sky, "right above A");
        assert_eq!(e.add_layer_as(&mut doc, true, None, None), Ok(Change::Scene));
        let group = e.active(&doc).to_owned();
        assert_eq!(doc.layer(&group).unwrap().kind, Kind::Group);
        assert!(e.add_layer_as(&mut doc, false, None, Some("nope")).is_err());
    }

    #[test]
    fn a_tag_goes_on_the_row_and_on_every_picked_layer_with_it() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let _ = e.pick_layer(&doc, "A", Pick::Only, &ROWS);
        let _ = e.pick_layer(&doc, "B", Pick::Toggle, &ROWS);
        doc.layer_mut("B").unwrap().locked = true;
        assert_eq!(e.set_tag(&mut doc, "A", Tag::Red), Change::Scene);
        assert_eq!(doc.layer("A").unwrap().color, Tag::Red);
        assert_eq!(doc.layer("B").unwrap().color, Tag::Red, "a lock does not keep a tag off");
        // A row that is not picked is tagged alone.
        assert_eq!(e.set_tag(&mut doc, "C", Tag::Blue), Change::Scene);
        assert_eq!(doc.layer("C").unwrap().color, Tag::Blue);
        assert_eq!(doc.layer("A").unwrap().color, Tag::Red);
        assert_eq!(e.set_tag(&mut doc, "C", Tag::Blue), Change::None, "already so");
        assert_eq!(e.set_tag(&mut doc, "nope", Tag::Blue), Change::None);
    }

    #[test]
    fn the_filter_narrows_the_rows_while_its_bar_is_open() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let ids = |e: &Editor| -> Vec<String> {
            e.rows(&doc).iter().map(|r| r.layer.id.clone()).collect()
        };
        let whole = ids(&e);
        assert!(e.filtering().is_none());
        e.filter_mut().name = "Layer 3".into();
        assert_eq!(ids(&e), whole, "the bar is shut");
        assert_eq!(e.toggle_filter(), Change::Selection);
        assert!(e.filtering().is_some());
        assert_eq!(ids(&e), ["G", "H", "C"]);
        let _ = e.toggle_filter();
        assert_eq!(ids(&e), whole);
        let _ = e.toggle_filter();
        assert_eq!(ids(&e), ["G", "H", "C"], "the filter waited as it was left");
    }

    /// A board of one group holding one raster layer: `G[R]`.
    fn grouped_board() -> Document {
        let mut doc = Document::new("t");
        doc.layers = vec![Layer {
            id: "G".into(),
            layers: vec![Layer {
                id: "R".into(),
                ..Layer::of("Layer 1", Kind::Raster)
            }],
            ..Layer::of("Group 1", Kind::Group)
        }];
        doc
    }

    #[test]
    fn a_brush_stroke_joins_a_raster_layer_inside_a_group() {
        let mut doc = grouped_board();
        let mut e = tool(Tool::Brush);
        let _ = e.pick_layer(&doc, "R", Pick::Only, &[]);
        let v = view();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        assert_eq!(paint_of(&doc, 0).layer, "R");
        assert_eq!(doc.layer("G").unwrap().layers.len(), 1, "nothing opened");
    }

    #[test]
    fn a_brush_stroke_with_a_group_active_opens_a_layer_inside_it_on_top() {
        let mut doc = grouped_board();
        let mut e = tool(Tool::Brush);
        let _ = e.pick_layer(&doc, "G", Pick::Only, &[]);
        let v = view();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        let group = doc.layer("G").unwrap();
        assert_eq!(group.layers.len(), 2);
        assert_eq!(paint_of(&doc, 0).layer, group.layers[1].id, "on top, inside");
        assert_eq!(e.active(&doc), group.layers[1].id);
        assert!(e.is_open("G"), "and its row can be seen");
    }

    #[test]
    fn a_pencil_stroke_opens_its_layer_right_above_the_active_one_in_its_group() {
        let mut doc = grouped_board();
        doc.stack_mut(Some("G"))
            .unwrap()
            .push(Layer::of("Layer 2", Kind::Raster));
        let mut e = pencil();
        let _ = e.pick_layer(&doc, "R", Pick::Only, &[]);
        let v = view();
        let _ = drag(&mut e, &v, &mut doc, (1.0, 2.0), (9.0, 2.0));
        let kids = &doc.layer("G").unwrap().layers;
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[0].id, "R");
        assert_eq!(kids[1].kind, Kind::Vector, "right above R, still in G");
        assert_eq!(path_of(&doc, 0).layer, kids[1].id);
    }

    #[test]
    fn a_holder_opens_and_shuts_and_starts_shut() {
        let mut e = Editor::new();
        assert!(!e.is_open("G"), "every holder starts shut");
        assert_eq!(e.toggle_open("G"), Change::Selection);
        assert!(e.is_open("G"));
        let _ = e.toggle_open("G");
        assert!(!e.is_open("G"));
    }

    #[test]
    fn revealing_a_layer_opens_everything_holding_it_and_nothing_else() {
        let doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        e.reveal(&doc, "E");
        assert!(e.is_open("K") && e.is_open("F"));
        assert!(!e.is_open("G") && !e.is_open("H"));
        e.reveal(&doc, "C");
        assert!(e.is_open("H") && e.is_open("G"));
    }

    #[test]
    fn opening_a_holder_is_not_where_the_hand_stands() {
        // Undo does not shut what was opened: looking inside a group is
        // not work, as panning is not.
        let mut e = Editor::new();
        let before = e.at();
        let _ = e.toggle_open("G");
        assert_eq!(e.at(), before);
    }

    #[test]
    fn picking_an_object_on_the_canvas_reveals_its_layer() {
        let mut doc = crate::tree::tests::nested();
        let mut e = Editor::new();
        let v = view();
        // The top of the pile at the origin is the rect in the group in
        // the frame.
        let _ = click(&mut e, &v, &mut doc, at(&v, 0.5, 0.5));
        assert_eq!(e.selection(), ["e"]);
        assert!(e.is_open("K") && e.is_open("F"), "its row can be seen");
    }

    #[test]
    fn an_editor_at_rest_is_not_busy() {
        assert!(!Editor::new().busy());
    }

    #[test]
    fn every_gesture_makes_the_editor_busy() {
        let v = view();
        let tip = brush().tip(Face::Round);

        // A stroke.
        let mut doc = board();
        let mut e = tool(Tool::Pencil);
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &tip);
        assert!(e.busy(), "a stroke in progress");
        e.cancel(&mut doc);
        assert!(!e.busy(), "and not once it is cancelled");

        // An area being dragged out.
        let mut e = tool(Tool::Frame);
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &tip);
        assert!(e.busy(), "an area being dragged out");

        // A navigation.
        let mut e = tool(Tool::Hand);
        let _ = e.press(Button::Left, &v, (10.0, 10.0), &mut doc, &tip);
        assert!(e.busy(), "a pan");

        // A move, which `is_moving` only admits to after the click slop.
        let mut e = tool(Tool::Select);
        let _ = e.press(Button::Left, &v, (15.0, 15.0), &mut doc, &tip);
        assert!(e.busy(), "a press on an element is already a drag");
        assert!(!e.is_moving(), "though nothing has moved yet");

        // A marquee, which `is_moving` never covered at all.
        let mut e = tool(Tool::Select);
        let _ = e.press(Button::Left, &v, (95.0, 5.0), &mut doc, &tip);
        assert!(e.busy(), "a marquee is a drag too");
        assert!(!e.is_moving(), "and is not a move");
    }
}
