//! Session state that never enters the document (§6.2): the active tool
//! and the stroke being drawn. Pure — `app` feeds it pointer events in
//! world coordinates and renders whatever it holds.

use crate::curve;
use crate::doc::{Document, Element, Path, new_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Pencil,
}

impl Tool {
    /// Dock order.
    pub const ALL: [Tool; 2] = [Tool::Select, Tool::Pencil];

    pub fn hotkey(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Pencil => 'p',
        }
    }

    pub fn from_hotkey(c: char) -> Option<Tool> {
        let c = c.to_ascii_lowercase();
        Tool::ALL.into_iter().find(|t| t.hotkey() == c)
    }
}

/// Pen width in world units (logical px at zoom 1).
pub const PEN_WIDTH: f64 = 2.0;

/// How far the committed path may stray from the pointer, in screen px.
/// `app` converts it to world units for [`Editor::pointer_up`].
pub const FIT_TOLERANCE_PX: f64 = 1.0;

#[derive(Debug, Default)]
pub struct Editor {
    tool: Tool,
    /// Stroke in progress, world coordinates.
    stroke: Option<Vec<[f64; 2]>>,
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Switching tools abandons whatever was being drawn.
    pub fn set_tool(&mut self, tool: Tool) {
        self.cancel();
        self.tool = tool;
    }

    pub fn stroke(&self) -> Option<&[[f64; 2]]> {
        self.stroke.as_deref()
    }

    pub fn is_drawing(&self) -> bool {
        self.stroke.is_some()
    }

    /// Primary button pressed on the canvas. True when the scene changed.
    pub fn pointer_down(&mut self, world: [f64; 2]) -> bool {
        match self.tool {
            Tool::Pencil => {
                self.stroke = Some(vec![world]);
                true
            }
            Tool::Select => false,
        }
    }

    /// Pointer moved. Points closer than `min_step` (world units) to the
    /// last recorded one are dropped so jitter does not bloat the path.
    pub fn pointer_move(&mut self, world: [f64; 2], min_step: f64) -> bool {
        let Some(points) = &mut self.stroke else {
            return false;
        };
        let last = points[points.len() - 1];
        let (dx, dy) = (world[0] - last[0], world[1] - last[1]);
        if dx * dx + dy * dy < min_step * min_step {
            return false;
        }
        points.push(world);
        true
    }

    /// Primary button released: the stroke is simplified and fitted with
    /// cubics within `tolerance` (world units), then committed as a `path`
    /// in `ink`.
    pub fn pointer_up(&mut self, doc: &mut Document, ink: &str, tolerance: f64) -> bool {
        let Some(points) = self.stroke.take() else {
            return false;
        };
        let curves = curve::fit(&curve::simplify(&points, tolerance), tolerance);
        doc.elements.push(Element::Path(Path {
            id: new_id(),
            curves,
            stroke: ink.to_owned(),
            width: PEN_WIDTH,
        }));
        true
    }

    /// Drops the stroke in progress. True if there was one.
    pub fn cancel(&mut self) -> bool {
        self.stroke.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Element;

    fn pencil() -> Editor {
        let mut e = Editor::new();
        e.set_tool(Tool::Pencil);
        e
    }

    #[test]
    fn starts_on_select_with_nothing_in_progress() {
        let e = Editor::new();
        assert_eq!(e.tool(), Tool::Select);
        assert!(!e.is_drawing());
        assert_eq!(e.stroke(), None);
    }

    #[test]
    fn hotkeys_map_to_tools_case_insensitively() {
        assert_eq!(Tool::from_hotkey('p'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('P'), Some(Tool::Pencil));
        assert_eq!(Tool::from_hotkey('v'), Some(Tool::Select));
        assert_eq!(Tool::from_hotkey('x'), None);
        for t in Tool::ALL {
            assert_eq!(Tool::from_hotkey(t.hotkey()), Some(t));
        }
    }

    #[test]
    fn select_tool_ignores_pointer_presses() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        assert!(!e.pointer_down([1.0, 1.0]));
        assert!(!e.pointer_move([2.0, 2.0], 0.5));
        assert!(!e.pointer_up(&mut doc, "#000", 1.0));
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn pencil_records_a_stroke_and_commits_it_on_release() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        assert!(e.pointer_down([1.0, 2.0]));
        assert!(e.is_drawing());
        assert!(e.pointer_move([4.0, 2.0], 0.5));
        assert_eq!(e.stroke(), Some(&[[1.0, 2.0], [4.0, 2.0]][..]));
        assert!(e.pointer_up(&mut doc, "#1f1f1f", 0.5));
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
    fn pointer_move_drops_points_closer_than_min_step() {
        let mut e = pencil();
        assert!(e.pointer_down([0.0, 0.0]));
        assert!(!e.pointer_move([0.2, 0.0], 0.5));
        assert!(e.pointer_move([0.6, 0.0], 0.5));
        assert_eq!(e.stroke(), Some(&[[0.0, 0.0], [0.6, 0.0]][..]));
    }

    #[test]
    fn pointer_move_without_a_press_does_nothing() {
        let mut e = pencil();
        assert!(!e.pointer_move([1.0, 1.0], 0.5));
        assert!(!e.is_drawing());
    }

    #[test]
    fn a_click_without_motion_commits_a_dot() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        e.pointer_down([3.0, 4.0]);
        assert!(e.pointer_up(&mut doc, "#000", 1.0));
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        assert_eq!(p.curves, vec![[[3.0, 4.0]; 4]]);
    }

    #[test]
    fn release_simplifies_jitter_away_before_fitting() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        e.pointer_down([0.0, 0.0]);
        for i in 1..=10 {
            let wobble = if i % 2 == 0 { 0.1 } else { -0.1 };
            e.pointer_move([i as f64, if i == 10 { 0.0 } else { wobble }], 0.5);
        }
        assert_eq!(e.stroke().map(<[_]>::len), Some(11));
        assert!(e.pointer_up(&mut doc, "#000", 0.5));
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        // Wobble under the tolerance is noise: one straight cubic remains.
        assert_eq!(p.curves.len(), 1, "{:?}", p.curves);
        let c = p.curves[0];
        assert_eq!(c[0], [0.0, 0.0]);
        assert_eq!(c[3], [10.0, 0.0]);
        assert_eq!(c[1][1], 0.0);
        assert_eq!(c[2][1], 0.0);
    }

    #[test]
    fn cancel_discards_the_stroke() {
        let mut e = pencil();
        let mut doc = Document::new("t");
        e.pointer_down([0.0, 0.0]);
        e.pointer_move([9.0, 9.0], 0.5);
        assert!(e.cancel());
        assert!(!e.is_drawing());
        assert!(!e.cancel(), "nothing left to cancel");
        assert!(!e.pointer_up(&mut doc, "#000", 1.0));
        assert!(doc.elements.is_empty());
    }

    #[test]
    fn switching_tools_cancels_the_stroke() {
        let mut e = pencil();
        e.pointer_down([0.0, 0.0]);
        e.set_tool(Tool::Select);
        assert_eq!(e.tool(), Tool::Select);
        assert!(!e.is_drawing());
    }
}
