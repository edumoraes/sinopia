//! Session state that never enters the document (§6.2): the active tool,
//! the keys temporarily overriding it, the stroke being drawn and the
//! pan/zoom gesture in progress. Pure — `app` feeds it pointer events in
//! screen px together with the current [`View`] and stores whatever camera
//! comes back.

use crate::curve;
use crate::doc::{Camera, Document, Element, Path, new_id};
use crate::scene::View;

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

#[derive(Debug, Default)]
pub struct Editor {
    tool: Tool,
    space: bool,
    ctrl: bool,
    /// Stroke in progress, world coordinates.
    stroke: Option<Vec<[f64; 2]>>,
    nav: Option<Nav>,
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    /// The tool the dock shows.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Switching tools abandons whatever was in progress.
    pub fn set_tool(&mut self, tool: Tool) {
        self.cancel();
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
    /// buttons during a stroke or gesture are ignored.
    pub fn press(&mut self, button: Button, view: &View, screen: (f64, f64)) -> Change {
        if self.stroke.is_some() || self.nav.is_some() {
            return Change::None;
        }
        let world = view.screen_to_world(screen.0, screen.1);
        match (button, self.active_tool()) {
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

    /// The pointer moved to `screen`. Stroke points closer than one
    /// physical px to the last one are dropped so jitter does not bloat the
    /// path.
    pub fn moved(&mut self, view: &View, screen: (f64, f64)) -> Change {
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
            None => Change::None,
        }
    }

    /// A button came up. The left button commits the stroke — simplified
    /// and fitted with cubics within [`FIT_TOLERANCE_PX`] — as a `path` in
    /// `ink`; the button that started a gesture ends it, and a zoom click
    /// (no drag) zooms one unit in.
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
            }));
            return Change::Scene;
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

    /// Drops the stroke and the gesture in progress. True if there was one.
    pub fn cancel(&mut self) -> bool {
        let had_stroke = self.stroke.take().is_some();
        let had_nav = self.nav.take().is_some();
        had_stroke || had_nav
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;
    use crate::scene::Viewport;

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
        e.set_tool(t);
        e
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
    fn select_tool_ignores_pointer_presses() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let v = view();
        assert_eq!(e.press(Button::Left, &v, (1.0, 1.0)), Change::None);
        assert_eq!(e.moved(&v, (2.0, 2.0)), Change::None);
        assert_eq!(
            e.release(Button::Left, &v, (2.0, 2.0), &mut doc, "#000"),
            Change::None
        );
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn pencil_records_a_stroke_and_commits_it_on_release() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        assert_eq!(e.press(Button::Left, &v, (1.0, 2.0)), Change::Scene);
        assert!(e.is_drawing());
        assert_eq!(e.moved(&v, (4.0, 2.0)), Change::Scene);
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
        assert_eq!(e.press(Button::Left, &v, (50.0, 50.0)), Change::Scene);
        assert_eq!(e.moved(&v, (50.5, 50.0)), Change::None);
        assert_eq!(e.moved(&v, (51.25, 50.0)), Change::Scene);
        assert_eq!(e.stroke(), Some(&[[0.0, 0.0], [0.625, 0.0]][..]));
    }

    #[test]
    fn moved_without_a_press_does_nothing() {
        let mut e = pencil();
        assert_eq!(e.moved(&view(), (1.0, 1.0)), Change::None);
        assert!(!e.is_drawing());
    }

    #[test]
    fn a_click_without_motion_commits_a_dot() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        let v = view();
        let _ = e.press(Button::Left, &v, (3.0, 4.0));
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
        let _ = e.press(Button::Left, &v, (0.0, 10.0));
        for i in 1..=10 {
            let wobble = if i % 2 == 0 { 0.3 } else { -0.3 };
            let _ = e.moved(
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
        let _ = e.press(Button::Left, &v, (0.0, 0.0));
        let _ = e.moved(&v, (9.0, 9.0));
        assert!(e.cancel());
        assert!(!e.is_drawing());
        assert!(!e.cancel(), "nothing left to cancel");
        assert_eq!(
            e.release(Button::Left, &v, (9.0, 9.0), &mut doc, "#000"),
            Change::None
        );
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn switching_tools_cancels_the_stroke() {
        let mut e = pencil();
        let _ = e.press(Button::Left, &view(), (0.0, 0.0));
        e.set_tool(Tool::Select);
        assert_eq!(e.tool(), Tool::Select);
        assert!(!e.is_drawing());
    }

    #[test]
    fn hand_drag_keeps_the_grabbed_point_under_the_pointer() {
        let mut e = tool(Tool::Hand);
        let v = view();
        assert_eq!(e.press(Button::Left, &v, (20.0, 20.0)), Change::None);
        assert!(e.is_panning());
        // 10px right, 5px down: the camera goes the other way.
        let Change::Camera(c) = e.moved(&v, (30.0, 25.0)) else {
            panic!("pan should move the camera");
        };
        assert_eq!(c, cam(40.0, 45.0, 1.0));
        // Fed the moved view, the same world point stays under the pointer.
        let Change::Camera(c) = e.moved(&with(c), (40.0, 25.0)) else {
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
        assert_eq!(e.press(Button::Middle, &v, (10.0, 10.0)), Change::None);
        assert!(e.is_panning());
        assert!(!e.is_drawing());
        assert_eq!(
            e.moved(&v, (15.0, 10.0)),
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
        assert_eq!(e.press(Button::Left, &v, (20.0, 20.0)), Change::None);
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
        let Change::Camera(c) = e.press(Button::Right, &v, (70.0, 30.0)) else {
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
        let _ = e.press(Button::Left, &v, (20.0, 20.0));
        // ZOOM_DRAG_PX to the right is exactly one step in…
        let Change::Camera(c) = e.moved(&v, (20.0 + ZOOM_DRAG_PX, 20.0)) else {
            panic!("drag should zoom");
        };
        assert!((c.zoom - ZOOM_STEP).abs() < 1e-12, "{}", c.zoom);
        assert_close(with(c).screen_to_world(20.0, 20.0), (20.0, 20.0));
        // …and the same distance to the left of the press is one step out,
        // measured from the press, not from the previous position.
        let Change::Camera(c) = e.moved(&with(c), (20.0 - ZOOM_DRAG_PX, 20.0)) else {
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
        let _ = e.press(Button::Left, &v, (20.0, 20.0));
        assert_eq!(e.moved(&v, (22.0, 21.0)), Change::None);
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
        assert_eq!(e.press(Button::Left, &v, (20.0, 20.0)), Change::None);
        assert!(!e.is_drawing());
        assert!(matches!(e.moved(&v, (60.0, 20.0)), Change::Camera(_)));
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
        let _ = e.press(Button::Left, &v, (0.0, 0.0));
        assert_eq!(e.press(Button::Right, &v, (0.0, 0.0)), Change::None);
        assert!(e.is_panning());

        let mut e = pencil();
        let _ = e.press(Button::Left, &v, (0.0, 0.0));
        assert_eq!(e.press(Button::Middle, &v, (0.0, 0.0)), Change::None);
        assert!(!e.is_panning());
        assert!(e.is_drawing());
    }

    #[test]
    fn cancel_and_set_tool_drop_a_pan_in_progress() {
        let v = view();
        let mut e = tool(Tool::Hand);
        let _ = e.press(Button::Middle, &v, (0.0, 0.0));
        assert!(e.cancel());
        assert!(!e.is_panning());
        assert_eq!(e.moved(&v, (5.0, 5.0)), Change::None);

        let _ = e.press(Button::Left, &v, (0.0, 0.0));
        e.set_tool(Tool::Pencil);
        assert!(!e.is_panning());
    }
}
