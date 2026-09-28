//! The Shape tool: a model dragged out over an area of the board.
//!
//! A press starts the drag and nothing lands until the release: what the
//! drag would lay is [`Editor::shaping`], which the window draws as the
//! shape itself. `Shift` gives both sides the longer one — a square, a
//! circle, a regular polygon — and `Alt` makes the press the middle of it
//! rather than a corner; a click lays the model at its default size,
//! centred on it. The shape lands on a vector layer of its own, named
//! after its model, in the frame the press landed in, and arrives
//! selected.

use super::{CLICK_SLOP_PX, Change, Editor, ROTATE_SNAP_DEG, Tool};
use crate::doc::{
    DEFAULT_INNER, DEFAULT_SHAPE_WIDTH, DEFAULT_SIDES, Document, Element, Head, Kind, Line, Shape, new_id,
};
use crate::geom::Point;
use crate::scene::View;
use crate::shape::Figure;

/// How many world units a click lays a figure: a shape this many a side,
/// Figma's own, and a line this long.
pub const DEFAULT_SIZE: f64 = 100.0;

/// How the next figure is drawn, and what the bar shows with nothing to
/// look at. A shape's stroke is the window's ink, read at the press as a
/// new text's colour is, and `outline` says whether it has one at all; a
/// line is nothing but its ink, and always has it. An arrow wears
/// `start` and `end`; a line, neither.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeStyle {
    pub fill: Option<String>,
    pub outline: bool,
    pub width: f64,
    pub radius: f64,
    pub sides: u32,
    pub inner: f64,
    pub start: Head,
    pub end: Head,
}

impl Default for ShapeStyle {
    /// An outline in the ink in the hand and nothing inside it, as a
    /// board's shapes are drawn: what is round them stays readable. An
    /// arrow's head is at its end, open.
    fn default() -> ShapeStyle {
        ShapeStyle {
            fill: None,
            outline: true,
            width: DEFAULT_SHAPE_WIDTH,
            radius: 0.0,
            sides: DEFAULT_SIDES,
            inner: DEFAULT_INNER,
            start: Head::None,
            end: Head::Arrow,
        }
    }
}

impl Editor {
    /// Takes the Shape tool up with `figure` in hand.
    pub fn choose_figure(&mut self, figure: Figure, doc: &mut Document) {
        self.set_tool(Tool::Shape, doc);
        self.figure = figure;
    }

    /// The next figure along, and round to the first after the last:
    /// what the tool's key does with the tool already in hand.
    pub(super) fn next_figure(&mut self) {
        let at = Figure::ALL.iter().position(|f| *f == self.figure).unwrap_or(0);
        self.figure = Figure::ALL[(at + 1) % Figure::ALL.len()];
    }

    /// What the drag in progress would lay — a shape or a line — stroked
    /// in `ink`; none while the drag is still a click, which lays a
    /// figure of its own size rather than one the hand is making.
    pub fn shaping(&self, view: &View, ink: &str) -> Option<Element> {
        let (from, to) = self.shaping?;
        (!is_click(view, from, to)).then(|| self.shaped(view, from, to, ink))
    }

    /// What a drag from `from` to `to` lays, as the figure, the held keys
    /// and the style say — with neither id nor layer yet.
    fn shaped(&self, view: &View, from: Point, to: Point, ink: &str) -> Element {
        let style = &self.shape_style;
        let Some(model) = self.figure.model() else {
            let (from, to) = self.dragged_ends(view, from, to);
            let arrow = self.figure == Figure::Arrow;
            return Element::Line(Line {
                id: String::new(),
                layer: String::new(),
                from,
                to,
                stroke: ink.to_owned(),
                width: style.width,
                start: if arrow { style.start } else { Head::None },
                end: if arrow { style.end } else { Head::None },
            });
        };
        let (lo, hi) = if is_click(view, from, to) {
            let half = DEFAULT_SIZE / 2.0;
            ([from[0] - half, from[1] - half], [from[0] + half, from[1] + half])
        } else {
            self.dragged_box(view, from, to)
        };
        Element::Shape(Shape {
            id: String::new(),
            layer: String::new(),
            model,
            x: lo[0],
            y: lo[1],
            w: hi[0] - lo[0],
            h: hi[1] - lo[1],
            rotation: 0.0,
            flip: false,
            fill: style.fill.clone(),
            stroke: style.outline.then(|| ink.to_owned()),
            width: style.width,
            radius: style.radius,
            sides: style.sides,
            inner: style.inner,
        })
    }

    /// The box a drag spans: from the press to the pointer, both sides the
    /// longer one under `Shift` — kept the way the drag went — and the
    /// press its middle under `Alt`. Never thinner than a pixel on screen,
    /// since a shape of no height is no shape.
    fn dragged_box(&self, view: &View, from: Point, to: Point) -> (Point, Point) {
        let mut d = [to[0] - from[0], to[1] - from[1]];
        if self.shift {
            let side = d[0].abs().max(d[1].abs());
            d = [side.copysign(d[0]), side.copysign(d[1])];
        }
        let least = 1.0 / view.px_per_world();
        let d = d.map(|v| if v.abs() < least { least.copysign(v) } else { v });
        if self.alt {
            let half = d.map(f64::abs);
            return (
                [from[0] - half[0], from[1] - half[1]],
                [from[0] + half[0], from[1] + half[1]],
            );
        }
        let other = [from[0] + d[0], from[1] + d[1]];
        (
            [from[0].min(other[0]), from[1].min(other[1])],
            [from[0].max(other[0]), from[1].max(other[1])],
        )
    }

    /// The ends a drag lays a line between: the press and the pointer,
    /// turned to the nearest fifteen degrees under `Shift` — the rotation
    /// ring's own step — and run as far past the press the other way
    /// under `Alt`. A click lays one across it, level.
    fn dragged_ends(&self, view: &View, from: Point, to: Point) -> (Point, Point) {
        if is_click(view, from, to) {
            let half = DEFAULT_SIZE / 2.0;
            return ([from[0] - half, from[1]], [from[0] + half, from[1]]);
        }
        let mut d = [to[0] - from[0], to[1] - from[1]];
        if self.shift {
            let step = ROTATE_SNAP_DEG.to_radians();
            let angle = (d[1].atan2(d[0]) / step).round() * step;
            let length = d[0].hypot(d[1]);
            d = [length * angle.cos(), length * angle.sin()];
        }
        let end = [from[0] + d[0], from[1] + d[1]];
        if self.alt {
            return ([from[0] - d[0], from[1] - d[1]], end);
        }
        (from, end)
    }

    /// Lays what the drag from `from` to `to` makes, stroked in `ink`, on
    /// a vector layer of its own named after its figure — in the frame
    /// the press landed in — and selects it.
    pub(super) fn lay_figure(&mut self, doc: &mut Document, view: &View, from: Point, to: Point, ink: &str) -> Change {
        let mut laid = self.shaped(view, from, to, ink);
        let born = doc.stack_at(from).map(str::to_owned);
        let layer = self.fresh_layer(doc, Kind::Vector, born.as_deref());
        let owner = doc.locate(&layer).and_then(|(owner, _)| owner.map(str::to_owned));
        let name = doc.next_name(owner.as_deref(), self.figure.name());
        if let Some(l) = doc.layer_mut(&layer) {
            l.name = name;
        }
        let id = new_id();
        laid.set_id(&id);
        laid.set_layer(&layer);
        self.selection = vec![id];
        doc.elements.push(laid);
        Change::Scene
    }
}

/// The figure a key takes the Shape tool up with: `R` the rectangle, `O`
/// the ellipse, `L` the line and `A` the arrow, as Figma's and every
/// board's own keys do.
pub fn figure_for_key(c: char) -> Option<Figure> {
    match c.to_ascii_lowercase() {
        'r' => Some(Figure::Rectangle),
        'o' => Some(Figure::Ellipse),
        'l' => Some(Figure::Line),
        'a' => Some(Figure::Arrow),
        _ => None,
    }
}

/// Whether a drag from `from` to `to` never left the click slop: a hand
/// that did not mean to drag, the same few pixels at every zoom.
fn is_click(view: &View, from: Point, to: Point) -> bool {
    let k = view.px_per_world() / view.scale;
    (to[0] - from[0]).abs() * k < CLICK_SLOP_PX && (to[1] - from[1]).abs() * k < CLICK_SLOP_PX
}

#[cfg(test)]
mod tests {
    use super::super::{Button, Tool};
    use super::*;
    use crate::brush::Tip;
    use crate::doc::Model;
    use crate::scene::Viewport;

    const INK: &str = "#112233";

    fn view() -> View {
        View {
            camera: crate::doc::Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            viewport: Viewport { w: 800, h: 600 },
            scale: 1.0,
        }
    }

    /// Screen px for world `(x, y)` in [`view`].
    fn at(x: f64, y: f64) -> (f64, f64) {
        view().world_to_screen(x, y)
    }

    fn shaping() -> (Editor, Document) {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Shape, &mut doc);
        (e, doc)
    }

    fn drag(e: &mut Editor, doc: &mut Document, from: (f64, f64), to: (f64, f64)) -> Change {
        let _ = e.press(Button::Left, &view(), at(from.0, from.1), doc, &Tip::PENCIL);
        let _ = e.moved(&view(), at((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0), doc);
        let _ = e.moved(&view(), at(to.0, to.1), doc);
        e.release(Button::Left, &view(), at(to.0, to.1), doc, INK)
    }

    fn lines(doc: &Document) -> Vec<&Line> {
        doc.elements
            .iter()
            .filter_map(|el| match el {
                Element::Line(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    fn only_line(doc: &Document) -> &Line {
        let all = lines(doc);
        assert_eq!(all.len(), 1, "{all:?}");
        all[0]
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    fn shapes(doc: &Document) -> Vec<&Shape> {
        doc.elements
            .iter()
            .filter_map(|el| match el {
                Element::Shape(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn only(doc: &Document) -> &Shape {
        let all = shapes(doc);
        assert_eq!(all.len(), 1, "{all:?}");
        all[0]
    }

    fn boxed(s: &Shape) -> (f64, f64, f64, f64) {
        (s.x, s.y, s.w, s.h)
    }

    #[test]
    fn a_drag_lays_the_model_over_the_area_it_crossed() {
        let (mut e, mut doc) = shaping();
        assert_eq!(drag(&mut e, &mut doc, (40.0, 30.0), (-60.0, 90.0)), Change::Scene);
        let s = only(&doc);
        assert_eq!(s.model, Model::Rectangle, "the tool starts on the rectangle");
        assert_eq!(boxed(s), (-60.0, 30.0, 100.0, 60.0), "from either corner");
        assert_eq!((s.fill.as_deref(), s.stroke.as_deref()), (None, Some(INK)));
        assert_eq!(s.width, crate::doc::DEFAULT_SHAPE_WIDTH);
        assert_eq!(e.selection(), std::slice::from_ref(&s.id), "it arrives selected");
        let layer = doc.layer(&s.layer).expect("its layer");
        assert_eq!(layer.kind, Kind::Vector, "a layer of its own, as a pencil line's");
        assert_eq!(e.active(&doc), s.layer);
    }

    #[test]
    fn its_layer_is_named_after_its_model() {
        let (mut e, mut doc) = shaping();
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (50.0, 50.0));
        e.choose_figure(Figure::Ellipse, &mut doc);
        let _ = drag(&mut e, &mut doc, (100.0, 0.0), (150.0, 50.0));
        e.choose_figure(Figure::Rectangle, &mut doc);
        let _ = drag(&mut e, &mut doc, (200.0, 0.0), (250.0, 50.0));
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = drag(&mut e, &mut doc, (300.0, 0.0), (350.0, 50.0));
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Layer 1", "Rectangle 1", "Ellipse 1", "Rectangle 2", "Arrow 1"]);
    }

    #[test]
    fn shift_gives_both_sides_the_longer_one_and_alt_draws_from_the_middle() {
        let (mut e, mut doc) = shaping();
        e.hold_shift(true);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (80.0, -30.0));
        assert_eq!(boxed(only(&doc)), (0.0, -80.0, 80.0, 80.0), "a square, the way it was dragged");
        doc.elements.clear();
        e.hold_shift(false);
        e.hold_alt(true);
        let _ = drag(&mut e, &mut doc, (100.0, 100.0), (130.0, 120.0));
        assert_eq!(boxed(only(&doc)), (70.0, 80.0, 60.0, 40.0), "the press is its middle");
        doc.elements.clear();
        e.hold_shift(true);
        let _ = drag(&mut e, &mut doc, (100.0, 100.0), (130.0, 120.0));
        assert_eq!(boxed(only(&doc)), (70.0, 70.0, 60.0, 60.0), "both at once");
    }

    #[test]
    fn a_click_lays_the_model_at_its_default_size_centred_on_it() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Star, &mut doc);
        let _ = e.press(Button::Left, &view(), at(10.0, 20.0), &mut doc, &Tip::PENCIL);
        let _ = e.moved(&view(), (at(10.0, 20.0).0 + 1.0, at(10.0, 20.0).1), &mut doc);
        let _ = e.release(Button::Left, &view(), at(10.0, 20.0), &mut doc, INK);
        let s = only(&doc);
        assert_eq!(s.model, Model::Star);
        let side = DEFAULT_SIZE;
        assert_eq!(boxed(s), (10.0 - side / 2.0, 20.0 - side / 2.0, side, side));
    }

    #[test]
    fn nothing_lands_until_the_release_and_esc_takes_the_drag_back() {
        let (mut e, mut doc) = shaping();
        let _ = e.press(Button::Left, &view(), at(0.0, 0.0), &mut doc, &Tip::PENCIL);
        assert!(e.shaping(&view(), INK).is_none(), "a press is not yet a drag");
        let _ = e.moved(&view(), at(60.0, 40.0), &mut doc);
        assert!(shapes(&doc).is_empty());
        assert!(e.busy() && e.is_drawing());
        let Some(Element::Shape(preview)) = e.shaping(&view(), INK) else {
            panic!("the shape being dragged out");
        };
        assert_eq!(boxed(&preview), (0.0, 0.0, 60.0, 40.0));
        assert_eq!(preview.stroke.as_deref(), Some(INK));
        assert!(e.cancel(&mut doc));
        assert!(e.shaping(&view(), INK).is_none() && !e.busy());
        let _ = e.release(Button::Left, &view(), at(60.0, 40.0), &mut doc, INK);
        assert!(shapes(&doc).is_empty(), "Esc took the drag back");
    }

    #[test]
    fn what_is_dragged_out_is_what_lands() {
        let (mut e, _) = shaping();
        e.hold_shift(true);
        for figure in [Figure::Polygon, Figure::Arrow] {
            let mut doc = Document::new("t");
            e.choose_figure(figure, &mut doc);
            let _ = e.press(Button::Left, &view(), at(5.0, 5.0), &mut doc, &Tip::PENCIL);
            let _ = e.moved(&view(), at(95.0, 60.0), &mut doc);
            let mut preview = e.shaping(&view(), INK).unwrap();
            let _ = e.release(Button::Left, &view(), at(95.0, 60.0), &mut doc, INK);
            let landed = &doc.elements[0];
            preview.set_id(landed.id());
            preview.set_layer(landed.layer());
            assert_eq!(preview, *landed, "{figure:?}");
        }
    }

    #[test]
    fn u_again_steps_through_the_figures() {
        let (mut e, mut doc) = shaping();
        assert_eq!(e.figure, Figure::Rectangle);
        e.choose_tool(Tool::Shape, &mut doc);
        assert_eq!(e.figure, Figure::Ellipse);
        for _ in 0..Figure::ALL.len() - 1 {
            e.choose_tool(Tool::Shape, &mut doc);
        }
        assert_eq!(e.figure, Figure::Rectangle, "and round again");
        e.choose_tool(Tool::Select, &mut doc);
        e.choose_tool(Tool::Shape, &mut doc);
        assert_eq!(e.figure, Figure::Rectangle, "taking the tool up keeps the figure");
    }

    #[test]
    fn r_o_l_and_a_take_the_tool_up_with_their_figures() {
        assert_eq!(figure_for_key('r'), Some(Figure::Rectangle));
        assert_eq!(figure_for_key('O'), Some(Figure::Ellipse));
        assert_eq!(figure_for_key('l'), Some(Figure::Line));
        assert_eq!(figure_for_key('a'), Some(Figure::Arrow));
        assert_eq!(figure_for_key('u'), None, "the tool's own key steps through them");
        for c in ['r', 'o', 'l', 'a'] {
            assert_eq!(Tool::from_hotkey(c), None, "{c} is no tool's");
        }
    }

    #[test]
    fn choosing_a_figure_takes_the_tool_up_with_it() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.choose_figure(Figure::Ellipse, &mut doc);
        assert_eq!((e.tool(), e.figure), (Tool::Shape, Figure::Ellipse));
    }

    #[test]
    fn a_line_runs_from_the_press_to_the_release() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Line, &mut doc);
        let _ = drag(&mut e, &mut doc, (10.0, 20.0), (-90.0, 60.0));
        let l = only_line(&doc);
        assert_eq!((l.from, l.to), ([10.0, 20.0], [-90.0, 60.0]));
        assert_eq!((l.stroke.as_str(), l.width), (INK, crate::doc::DEFAULT_SHAPE_WIDTH));
        assert_eq!((l.start, l.end), (Head::None, Head::None));
        assert_eq!(doc.layer(&l.layer).map(|l| l.name.as_str()), Some("Line 1"));
        assert_eq!(e.selection(), std::slice::from_ref(&l.id));
    }

    #[test]
    fn an_arrow_is_a_line_with_a_head_at_its_end() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 0.0));
        let l = only_line(&doc);
        assert_eq!((l.start, l.end), (Head::None, Head::Arrow));
        // A line drawn with no stroke in the style is still drawn: a line
        // is nothing but its ink.
        doc.elements.clear();
        e.shape_style.outline = false;
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 0.0));
        assert_eq!(only_line(&doc).stroke, INK);
    }

    #[test]
    fn shift_turns_a_line_in_fifteen_degree_steps_and_alt_draws_it_from_its_middle() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Line, &mut doc);
        e.hold_shift(true);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 20.0));
        let l = only_line(&doc);
        let length = 100f64.hypot(20.0);
        let a = 15f64.to_radians();
        assert!(close(l.to, [length * a.cos(), length * a.sin()]), "{:?}", l.to);
        doc.elements.clear();
        e.hold_shift(false);
        e.hold_alt(true);
        let _ = drag(&mut e, &mut doc, (50.0, 50.0), (80.0, 40.0));
        let l = only_line(&doc);
        assert_eq!((l.from, l.to), ([20.0, 60.0], [80.0, 40.0]), "the press is its middle");
    }

    #[test]
    fn a_click_lays_a_line_across_it() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = e.press(Button::Left, &view(), at(0.0, 0.0), &mut doc, &Tip::PENCIL);
        let _ = e.release(Button::Left, &view(), at(0.0, 0.0), &mut doc, INK);
        let l = only_line(&doc);
        let half = DEFAULT_SIZE / 2.0;
        assert_eq!((l.from, l.to), ([-half, 0.0], [half, 0.0]));
        assert_eq!(l.end, Head::Arrow);
    }

    #[test]
    fn a_shape_pressed_in_a_frame_lands_in_its_stack() {
        let (mut e, mut doc) = shaping();
        e.set_tool(Tool::Frame, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (200.0, 200.0));
        let frame_layer = doc.layers.last().unwrap().id.clone();
        e.set_tool(Tool::Shape, &mut doc);
        // Pressed inside, let go of outside: the press says where it goes.
        let _ = drag(&mut e, &mut doc, (150.0, 150.0), (300.0, 300.0));
        let s = only(&doc);
        assert_eq!(doc.context(&s.layer), Some(frame_layer.as_str()));
    }

    #[test]
    fn with_the_tool_in_hand_the_selections_handles_still_answer() {
        let (mut e, mut doc) = shaping();
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 60.0));
        // The bottom right handle of the shape just drawn resizes it.
        assert_eq!(e.pointer_tool(&doc, &view(), at(100.0, 60.0)), Tool::Select);
        assert_eq!(e.pointer_tool(&doc, &view(), at(50.0, 30.0)), Tool::Shape, "inside it draws another");
        let _ = e.press(Button::Left, &view(), at(100.0, 60.0), &mut doc, &Tip::PENCIL);
        let _ = e.moved(&view(), at(160.0, 90.0), &mut doc);
        let _ = e.release(Button::Left, &view(), at(160.0, 90.0), &mut doc, INK);
        assert_eq!(boxed(only(&doc)), (0.0, 0.0, 160.0, 90.0));
    }

    #[test]
    fn the_next_shape_is_drawn_as_the_style_says() {
        let (mut e, mut doc) = shaping();
        e.shape_style = ShapeStyle {
            fill: Some("#e5484d".into()),
            outline: false,
            width: 6.0,
            radius: 12.0,
            sides: 8,
            inner: 0.6,
            ..ShapeStyle::default()
        };
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (40.0, 40.0));
        let s = only(&doc);
        assert_eq!((s.fill.as_deref(), s.stroke.as_deref()), (Some("#e5484d"), None));
        assert_eq!((s.width, s.radius, s.sides, s.inner), (6.0, 12.0, 8, 0.6));
    }
}
