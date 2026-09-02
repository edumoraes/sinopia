//! Session state that never enters the document (§6.2): the active tool,
//! the keys temporarily overriding it, the stroke being drawn, the
//! pan/zoom gesture in progress, the selection and the drag reshaping it.
//! Pure — `app` feeds it pointer events in screen px together with the
//! current [`View`] and the document, and stores whatever comes back.

use crate::bitmap;
use crate::curve;
use crate::doc::{Camera, Document, Element, Image, Path, new_id};
use crate::geom::{Affine, Corner, Frame, Point};
use crate::scene::View;
use crate::select::{self, Handle};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Hand,
    Pencil,
    Zoom,
}

impl Tool {
    /// Dock order.
    pub const ALL: [Tool; 4] = [Tool::Select, Tool::Hand, Tool::Pencil, Tool::Zoom];

    pub fn hotkey(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Hand => 'h',
            Tool::Pencil => 'p',
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

#[derive(Debug, Default)]
pub struct Editor {
    tool: Tool,
    space: bool,
    ctrl: bool,
    shift: bool,
    /// Stroke in progress, world coordinates.
    stroke: Option<Vec<[f64; 2]>>,
    nav: Option<Nav>,
    /// Ids of the selected elements, in selection order.
    selection: Vec<String>,
    drag: Option<Drag>,
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
        select::handle_at(&frame, view, screen)
    }

    pub fn stroke(&self) -> Option<&[[f64; 2]]> {
        self.stroke.as_deref()
    }

    pub fn is_drawing(&self) -> bool {
        self.stroke.is_some()
    }

    pub fn is_panning(&self) -> bool {
        matches!(self.nav, Some(Nav::Pan { .. }))
    }

    /// A button went down on the canvas at `screen` (physical px). Extra
    /// buttons during a stroke, gesture or drag are ignored.
    pub fn press(
        &mut self,
        button: Button,
        view: &View,
        screen: (f64, f64),
        doc: &mut Document,
    ) -> Change {
        if self.stroke.is_some() || self.nav.is_some() || self.drag.is_some() {
            return Change::None;
        }
        let world = view.screen_to_world(screen.0, screen.1);
        match (button, self.active_tool()) {
            (Button::Left, Tool::Select) => self.select_press(view, screen, doc),
            (Button::Middle, _) | (Button::Left, Tool::Hand) => {
                self.nav = Some(Nav::Pan {
                    button,
                    anchor: world,
                });
                Change::None
            }
            (Button::Left, Tool::Pencil) => {
                self.stroke = Some(vec![[world.0, world.1]]);
                Change::Scene
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

    /// Select tool, left button: a handle starts a resize or a rotation;
    /// an element becomes the selection (Shift toggles it in and out) and
    /// starts a move; empty canvas starts a marquee, clearing the selection
    /// unless Shift keeps it as the base.
    fn select_press(&mut self, view: &View, screen: (f64, f64), doc: &mut Document) -> Change {
        let world = point(view.screen_to_world(screen.0, screen.1));
        if let Some(frame) = self.selection_frame(doc)
            && let Some(handle) = select::handle_at(&frame, view, screen)
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
            snapshot: self.snapshot(doc),
            moved: false,
        });
        Change::Selection
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
        if let Some(points) = &mut self.stroke {
            let world = view.screen_to_world(screen.0, screen.1);
            let last = points[points.len() - 1];
            let (dx, dy) = (world.0 - last[0], world.1 - last[1]);
            let min_step = 1.0 / view.px_per_world();
            if dx * dx + dy * dy < min_step * min_step {
                return Change::None;
            }
            points.push([world.0, world.1]);
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
                *map = select::resize_map(frame, *corner, world);
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
        doc.elements.push(Element::Image(Image {
            id: id.clone(),
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
        doc.elements
            .retain(|el| !self.selection.iter().any(|id| id == el.id()));
        self.selection.clear();
        Change::Scene
    }

    /// A button came up. The left button commits the stroke — simplified
    /// and fitted with cubics within [`FIT_TOLERANCE_PX`] — as a `path` in
    /// `ink` — or ends the selection drag; the button that started a
    /// gesture ends it, and a zoom click (no drag) zooms one unit in.
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
            && let Some(points) = self.stroke.take()
        {
            let tolerance = FIT_TOLERANCE_PX / view.px_per_world();
            let curves = curve::fit(&curve::simplify(&points, tolerance), tolerance);
            doc.elements.push(Element::Path(Path {
                id: new_id(),
                curves,
                stroke: ink.to_owned(),
                width: PEN_WIDTH,
                rotation: 0.0,
            }));
            return Change::Scene;
        }
        if button == Button::Left
            && let Some(drag) = self.drag.take()
        {
            return match drag {
                Drag::Move { moved: false, .. } | Drag::Marquee { .. } => Change::Selection,
                Drag::Move { .. } | Drag::Resize { .. } | Drag::Rotate { .. } => Change::Scene,
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
    use crate::doc::{Element, Rect};
    use crate::geom::Corner;
    use crate::scene::Viewport;
    use crate::select::Handle;

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
        e.press(button, v, at, &mut Document::new("t"))
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
        let _ = e.press(Button::Left, v, from, doc);
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
            e.press(Button::Left, &v, (70.0, 70.0), &mut doc),
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
            e.press(Button::Left, &v, (50.0, 5.0), &mut doc),
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
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc);
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
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc);
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
            e.press(Button::Left, &v, (5.0, 5.0), &mut doc),
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
        let _ = e.press(Button::Left, &v, (30.0, 20.0), &mut doc);
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
    fn rotate_handle_turns_about_the_center() {
        let mut e = Editor::new();
        let mut doc = board();
        let v = view();
        let _ = click(&mut e, &v, &mut doc, (70.0, 70.0));
        // `b` is centered on (70, 70); its top-right rotation handle sits
        // past (80, 60) along the diagonal. Sweeping it a quarter turn
        // clockwise puts it past (80, 80).
        let d = f64::from(crate::select::ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        let _ = e.press(Button::Left, &v, (80.0 + d, 60.0 - d), &mut doc);
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
        let _ = e.press(Button::Left, &v, around_b(-45.0), &mut doc);
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
        let _ = e.press(Button::Left, &v, around_b(-8.0), &mut doc);
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
        let _ = e.press(Button::Left, &v, (80.0 + d, 80.0 + d), &mut doc);
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
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc);
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
        let _ = e.press(Button::Left, &v, (70.0, 70.0), &mut doc);
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
        assert_eq!(e.stroke(), None);
    }

    #[test]
    fn hotkeys_map_to_tools_case_insensitively() {
        assert_eq!(Tool::from_hotkey('p'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('P'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('v'), Some(Tool::Select));
        assert_eq!(Tool::from_hotkey('h'), Some(Tool::Hand));
        assert_eq!(Tool::from_hotkey('z'), Some(Tool::Zoom));
        assert_eq!(Tool::from_hotkey('x'), None);
        for t in Tool::ALL {
            assert_eq!(Tool::from_hotkey(t.hotkey()), Some(t));
        }
    }

    #[test]
    fn dock_order_is_select_hand_pencil_zoom() {
        assert_eq!(
            Tool::ALL,
            [Tool::Select, Tool::Hand, Tool::Pencil, Tool::Zoom]
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
        assert_eq!(e.stroke(), Some(&[[1.0, 2.0], [4.0, 2.0]][..]));
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
        assert_eq!(e.stroke(), Some(&[[0.0, 0.0], [0.625, 0.0]][..]));
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
        assert_eq!(e.stroke().map(<[_]>::len), Some(11));
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
        let _ = e.press(Button::Left, &view(), (10.0, 10.0), &mut doc);
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
}
