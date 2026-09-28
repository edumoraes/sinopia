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
    DEFAULT_INNER, DEFAULT_SHAPE_WIDTH, DEFAULT_SIDES, Document, Element, Head, Kind, Line, MAX_SIDES,
    MIN_SIDES, Shape, new_id,
};
use crate::geom::Point;
use crate::scene::View;
use crate::shape::{Figure, MAX_INNER, MIN_INNER};

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

/// A change the shape bar asks for: to the shapes and lines selected, as
/// far as each can take it, or to how the next one is drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Restyle {
    Fill(Option<String>),
    /// A colour, or none; a line keeps its ink whatever it is asked, and
    /// for the next figure the colour is the window's ink, set apart.
    Stroke(Option<String>),
    Width(f64),
    Radius(f64),
    Sides(u32),
    Inner(f64),
    Start(Head),
    End(Head),
}

impl Restyle {
    /// The change held to what a board can hold — sides and depth to
    /// their ranges, a radius to none below nothing — or none at all for
    /// a number that is not one, or a width of nothing.
    fn held(self) -> Option<Restyle> {
        Some(match self {
            Restyle::Width(w) if !(w.is_finite() && w > 0.0) => return None,
            Restyle::Radius(r) if !r.is_finite() => return None,
            Restyle::Inner(i) if !i.is_finite() => return None,
            Restyle::Radius(r) => Restyle::Radius(r.max(0.0)),
            Restyle::Sides(n) => Restyle::Sides(n.clamp(MIN_SIDES, MAX_SIDES)),
            Restyle::Inner(i) => Restyle::Inner(i.clamp(MIN_INNER, MAX_INNER)),
            other => other,
        })
    }

    /// Makes it on `el`, as far as `el` can take it: true when anything
    /// changed.
    fn onto(&self, el: &mut Element) -> bool {
        fn set<T: PartialEq>(at: &mut T, to: T) -> bool {
            let changed = *at != to;
            *at = to;
            changed
        }
        match (el, self) {
            (Element::Shape(s), Restyle::Fill(c)) => set(&mut s.fill, c.clone()),
            (Element::Shape(s), Restyle::Stroke(c)) => set(&mut s.stroke, c.clone()),
            (Element::Shape(s), Restyle::Width(w)) => set(&mut s.width, *w),
            (Element::Shape(s), Restyle::Radius(r)) => set(&mut s.radius, *r),
            (Element::Shape(s), Restyle::Sides(n)) => set(&mut s.sides, *n),
            (Element::Shape(s), Restyle::Inner(i)) => set(&mut s.inner, *i),
            (Element::Line(l), Restyle::Stroke(Some(c))) => set(&mut l.stroke, c.clone()),
            (Element::Line(l), Restyle::Width(w)) => set(&mut l.width, *w),
            (Element::Line(l), Restyle::Start(h)) => set(&mut l.start, *h),
            (Element::Line(l), Restyle::End(h)) => set(&mut l.end, *h),
            _ => false,
        }
    }
}

/// What the shape bar shows: the figure lit, and how the first shape or
/// line it is looking at is drawn — or how the next one will be.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeLook {
    pub figure: Figure,
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub width: f64,
    pub radius: f64,
    pub sides: u32,
    pub inner: f64,
    pub start: Head,
    pub end: Head,
}

impl Editor {
    /// The shapes and lines the bar is looking at: every one selected
    /// that no lock keeps.
    fn shape_targets(&self, doc: &Document) -> Vec<String> {
        self.selection
            .iter()
            .filter(|id| {
                doc.elements.iter().any(|el| {
                    el.id() == id.as_str()
                        && matches!(el, Element::Shape(_) | Element::Line(_))
                        && !doc.locked(el.layer())
                })
            })
            .cloned()
            .collect()
    }

    /// Whether the bar is looking at a shape or a line rather than at how
    /// the next one is drawn.
    pub fn shape_targeted(&self, doc: &Document) -> bool {
        !self.shape_targets(doc).is_empty()
    }

    /// What the bar shows: the first shape or line it is looking at, or
    /// the next figure as the style says, stroked in `ink` — the ink in
    /// the hand, which the next one is born in.
    pub fn shape_look(&self, doc: &Document, ink: &str) -> ShapeLook {
        let style = &self.shape_style;
        let first = self
            .shape_targets(doc)
            .first()
            .and_then(|id| doc.elements.iter().find(|el| el.id() == id.as_str()));
        match first {
            Some(Element::Shape(s)) => ShapeLook {
                figure: Figure::of(s.model),
                fill: s.fill.clone(),
                stroke: s.stroke.clone(),
                width: s.width,
                radius: s.radius,
                sides: s.sides,
                inner: s.inner,
                start: style.start,
                end: style.end,
            },
            Some(Element::Line(l)) => ShapeLook {
                figure: Figure::of_line(l),
                fill: None,
                stroke: Some(l.stroke.clone()),
                width: l.width,
                radius: style.radius,
                sides: style.sides,
                inner: style.inner,
                start: l.start,
                end: l.end,
            },
            _ => {
                let line = self.figure == Figure::Line;
                ShapeLook {
                    figure: self.figure,
                    fill: style.fill.clone(),
                    stroke: (style.outline || self.figure.model().is_none()).then(|| ink.to_owned()),
                    width: style.width,
                    radius: style.radius,
                    sides: style.sides,
                    inner: style.inner,
                    start: if line { Head::None } else { style.start },
                    end: if line { Head::None } else { style.end },
                }
            }
        }
    }

    /// Makes `change` where the bar is looking: on every shape and line
    /// selected, as far as each can take it — a line keeps its ink and
    /// has no fill, a shape has no heads — or, with none selected, on how
    /// the next one is drawn.
    pub fn restyle_shapes(&mut self, doc: &mut Document, change: Restyle) -> Change {
        let Some(change) = change.held() else {
            return Change::None;
        };
        let targets = self.shape_targets(doc);
        if targets.is_empty() {
            let style = &mut self.shape_style;
            match change {
                Restyle::Fill(c) => style.fill = c,
                Restyle::Stroke(c) => style.outline = c.is_some(),
                Restyle::Width(w) => style.width = w,
                Restyle::Radius(r) => style.radius = r,
                Restyle::Sides(n) => style.sides = n,
                Restyle::Inner(i) => style.inner = i,
                Restyle::Start(h) => style.start = h,
                Restyle::End(h) => style.end = h,
            }
            // A head asked of the next line makes it an arrow, and an
            // arrow left with none is a line: the figure is what it wears.
            let bare = style.start == Head::None && style.end == Head::None;
            match (self.figure, bare) {
                (Figure::Line, false) => self.figure = Figure::Arrow,
                (Figure::Arrow, true) => self.figure = Figure::Line,
                _ => {}
            }
            return Change::Selection;
        }
        let mut changed = false;
        for el in doc.elements.iter_mut().filter(|el| targets.iter().any(|t| t == el.id())) {
            changed |= change.onto(el);
        }
        if changed { Change::Scene } else { Change::Selection }
    }

    /// What every shape or line the bar is looking at wears as its fill —
    /// or its stroke — by its id: what a menu trying colours on them puts
    /// back when the pointer leaves it. A line has no fill to keep.
    pub fn paints(&self, doc: &Document, stroke: bool) -> Vec<(String, Option<String>)> {
        let targets = self.shape_targets(doc);
        doc.elements
            .iter()
            .filter(|el| targets.iter().any(|t| t == el.id()))
            .filter_map(|el| match (el, stroke) {
                (Element::Shape(s), false) => Some((s.id.clone(), s.fill.clone())),
                (Element::Shape(s), true) => Some((s.id.clone(), s.stroke.clone())),
                (Element::Line(l), true) => Some((l.id.clone(), Some(l.stroke.clone()))),
                _ => None,
            })
            .collect()
    }

    /// Puts back what [`Editor::paints`] kept.
    pub fn repaint(&self, doc: &mut Document, was: &[(String, Option<String>)], stroke: bool) {
        for (id, paint) in was {
            let Some(el) = doc.elements.iter_mut().find(|el| el.id() == id.as_str()) else {
                continue;
            };
            let change = if stroke {
                Restyle::Stroke(paint.clone())
            } else {
                Restyle::Fill(paint.clone())
            };
            change.onto(el);
        }
    }

    /// The bar's figure: the tool draws it from now on, and what is
    /// selected becomes it within its kind — a shape another model, a
    /// line an arrow or a bare line again. A shape is not made a line,
    /// nor a line a shape.
    pub fn set_figure(&mut self, figure: Figure, doc: &mut Document) -> Change {
        self.take_figure(figure);
        let targets = self.shape_targets(doc);
        let mut changed = false;
        for el in doc.elements.iter_mut().filter(|el| targets.iter().any(|t| t == el.id())) {
            match (el, figure.model()) {
                (Element::Shape(s), Some(model)) if s.model != model => {
                    s.model = model;
                    changed = true;
                }
                (Element::Line(l), None) => {
                    let bare = l.start == Head::None && l.end == Head::None;
                    let heads = match figure {
                        Figure::Line => (Head::None, Head::None),
                        _ if bare => (Head::None, Head::Arrow),
                        _ => (l.start, l.end),
                    };
                    if (l.start, l.end) != heads {
                        (l.start, l.end) = heads;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed { Change::Scene } else { Change::Selection }
    }

    /// Takes the Shape tool up with `figure` in hand.
    pub fn choose_figure(&mut self, figure: Figure, doc: &mut Document) {
        self.set_tool(Tool::Shape, doc);
        self.take_figure(figure);
    }

    /// The next figure along, and round to the first after the last:
    /// what the tool's key does with the tool already in hand.
    pub(super) fn next_figure(&mut self) {
        let at = Figure::ALL.iter().position(|f| *f == self.figure).unwrap_or(0);
        self.take_figure(Figure::ALL[(at + 1) % Figure::ALL.len()]);
    }

    /// `figure` is what the tool draws from now on. An arrow is never
    /// drawn bare: one taken up with no head left in the style grows the
    /// open one at its end again.
    fn take_figure(&mut self, figure: Figure) {
        self.figure = figure;
        let style = &mut self.shape_style;
        if figure == Figure::Arrow && style.start == Head::None && style.end == Head::None {
            style.end = Head::Arrow;
        }
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

    /// A board with one line from `from` to `to`, drawn and left selected.
    fn with_line(from: (f64, f64), to: (f64, f64)) -> (Editor, Document) {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Line, &mut doc);
        let _ = drag(&mut e, &mut doc, from, to);
        (e, doc)
    }

    #[test]
    fn dragging_an_end_of_a_lone_line_moves_that_end_alone() {
        let (mut e, mut doc) = with_line((0.0, 0.0), (100.0, 0.0));
        assert!(e.lone_line(&doc).is_some());
        // With the Shape tool still in hand, an end answers as a handle.
        assert_eq!(e.pointer_tool(&doc, &view(), at(100.0, 0.0)), Tool::Select);
        let _ = e.press(Button::Left, &view(), at(100.0, 0.0), &mut doc, &Tip::PENCIL);
        let _ = e.moved(&view(), at(120.0, 70.0), &mut doc);
        let _ = e.release(Button::Left, &view(), at(120.0, 70.0), &mut doc, INK);
        let l = only_line(&doc);
        assert_eq!((l.from, l.to), ([0.0, 0.0], [120.0, 70.0]));
        assert_eq!(e.selection(), std::slice::from_ref(&l.id), "still the one selected");
    }

    #[test]
    fn shift_turns_a_dragged_end_in_fifteen_degree_steps_about_the_other() {
        let (mut e, mut doc) = with_line((0.0, 0.0), (100.0, 0.0));
        e.hold_shift(true);
        let _ = e.press(Button::Left, &view(), at(0.0, 0.0), &mut doc, &Tip::PENCIL);
        let _ = e.moved(&view(), at(0.0, 95.0), &mut doc);
        let l = only_line(&doc);
        // From (100, 0) the pointer is at 136.5 degrees: 135 it is.
        let a = 135f64.to_radians();
        let length = 100f64.hypot(95.0);
        assert!(close(l.from, [100.0 + length * a.cos(), length * a.sin()]), "{:?}", l.from);
        assert_eq!(l.to, [100.0, 0.0]);
    }

    #[test]
    fn esc_puts_a_dragged_end_back() {
        let (mut e, mut doc) = with_line((0.0, 0.0), (100.0, 0.0));
        let _ = e.press(Button::Left, &view(), at(100.0, 0.0), &mut doc, &Tip::PENCIL);
        let _ = e.moved(&view(), at(50.0, 90.0), &mut doc);
        assert_ne!(only_line(&doc).to, [100.0, 0.0]);
        assert!(e.escape(&mut doc));
        assert_eq!(only_line(&doc).to, [100.0, 0.0]);
    }

    #[test]
    fn lines_selected_with_something_else_are_framed_as_a_box() {
        let (mut e, mut doc) = with_line((0.0, 0.0), (100.0, 0.0));
        let first = only_line(&doc).id.clone();
        let _ = drag(&mut e, &mut doc, (0.0, 50.0), (100.0, 80.0));
        let second = doc.elements[1].id().to_owned();
        e.set_tool(Tool::Select, &mut doc);
        e.go(super::super::Spot {
            selection: vec![first, second],
            ..Default::default()
        });
        assert!(e.lone_line(&doc).is_none());
        let f = e.selection_frame(&doc).unwrap();
        let corner = f.corner(crate::geom::Corner::BottomRight);
        let (x, y) = view().world_to_screen(corner[0], corner[1]);
        assert!(matches!(e.hover(&doc, &view(), (x, y)), Some(crate::select::Handle::Resize(_))));
    }

    /// A rectangle and an arrow, both drawn and both selected.
    fn two_selected() -> (Editor, Document) {
        let (mut e, mut doc) = shaping();
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 60.0));
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 100.0), (100.0, 100.0));
        let ids: Vec<String> = doc.elements.iter().map(|el| el.id().to_owned()).collect();
        e.set_tool(Tool::Select, &mut doc);
        e.go(super::super::Spot {
            selection: ids,
            ..Default::default()
        });
        (e, doc)
    }

    #[test]
    fn the_bar_looks_at_the_first_selected_else_at_the_next_figure() {
        let (mut e, mut doc) = shaping();
        assert!(!e.shape_targeted(&doc));
        let next = e.shape_look(&doc, INK);
        assert_eq!(next.figure, Figure::Rectangle);
        assert_eq!((next.fill.as_deref(), next.stroke.as_deref()), (None, Some(INK)));
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 60.0));
        assert!(e.shape_targeted(&doc), "the shape just drawn is selected");
        let (e, doc) = two_selected();
        let look = e.shape_look(&doc, "#999999");
        assert_eq!(look.figure, Figure::Rectangle, "the first of them");
        assert_eq!(look.stroke.as_deref(), Some(INK), "its own ink, not the hand's");
    }

    #[test]
    fn a_change_goes_to_every_one_selected_that_can_take_it() {
        let (mut e, mut doc) = two_selected();
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Fill(Some("#e5484d".into()))), Change::Scene);
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Width(5.0)), Change::Scene);
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Start(Head::Triangle)), Change::Scene);
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Stroke(None)), Change::Scene);
        let s = only(&doc);
        assert_eq!((s.fill.as_deref(), s.stroke.as_deref(), s.width), (Some("#e5484d"), None, 5.0));
        let l = only_line(&doc);
        assert_eq!((l.stroke.as_str(), l.width, l.start), (INK, 5.0, Head::Triangle), "a line keeps its ink");
        assert_eq!(
            e.restyle_shapes(&mut doc, Restyle::Width(5.0)),
            Change::Selection,
            "what is as asked already is no change"
        );
    }

    #[test]
    fn with_nothing_selected_the_bar_sets_how_the_next_is_drawn() {
        let (mut e, mut doc) = shaping();
        let _ = e.restyle_shapes(&mut doc, Restyle::Fill(Some("#30a46c".into())));
        let _ = e.restyle_shapes(&mut doc, Restyle::Stroke(None));
        let _ = e.restyle_shapes(&mut doc, Restyle::Radius(9.0));
        let _ = e.restyle_shapes(&mut doc, Restyle::End(Head::Triangle));
        assert!(doc.elements.is_empty());
        let look = e.shape_look(&doc, INK);
        assert_eq!((look.fill.as_deref(), look.stroke), (Some("#30a46c"), None));
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (50.0, 50.0));
        let s = only(&doc);
        assert_eq!((s.fill.as_deref(), s.stroke.as_deref(), s.radius), (Some("#30a46c"), None, 9.0));
        assert!(e.escape(&mut doc), "nothing selected again");
        let _ = e.restyle_shapes(&mut doc, Restyle::Stroke(Some("#ffffff".into())));
        assert_eq!(e.shape_look(&doc, INK).stroke.as_deref(), Some(INK), "a stroke is back, in the hand's ink");
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 100.0), (50.0, 100.0));
        assert_eq!(only_line(&doc).end, Head::Triangle);
    }

    #[test]
    fn the_bar_switches_the_model_of_what_is_selected_within_its_kind() {
        let (mut e, mut doc) = two_selected();
        assert_eq!(e.set_figure(Figure::Star, &mut doc), Change::Scene);
        assert_eq!(only(&doc).model, Model::Star);
        assert_eq!(only_line(&doc).end, Head::Arrow, "a line is not a star");
        let _ = e.set_figure(Figure::Line, &mut doc);
        assert_eq!((only_line(&doc).start, only_line(&doc).end), (Head::None, Head::None));
        assert_eq!(only(&doc).model, Model::Star, "nor a star a line");
        let _ = e.set_figure(Figure::Arrow, &mut doc);
        assert_eq!(only_line(&doc).end, Head::Arrow, "a bare line made an arrow grows its head");
        assert_eq!(e.figure, Figure::Arrow, "and the tool draws it next");
        let (mut e, mut doc) = shaping();
        assert_eq!(e.set_figure(Figure::Diamond, &mut doc), Change::Selection);
        assert_eq!(e.figure, Figure::Diamond);
    }

    #[test]
    fn a_locked_shape_is_neither_looked_at_nor_restyled() {
        let (mut e, mut doc) = shaping();
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 60.0));
        let layer = only(&doc).layer.clone();
        if let Some(l) = doc.layer_mut(&layer) {
            l.locked = true;
        }
        assert!(!e.shape_targeted(&doc));
        let _ = e.restyle_shapes(&mut doc, Restyle::Fill(Some("#000000".into())));
        assert_eq!(only(&doc).fill, None);
    }

    #[test]
    fn what_the_board_cannot_hold_is_held_to_what_it_can() {
        let (mut e, mut doc) = shaping();
        let _ = e.shape_look(&doc, INK);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 60.0));
        let _ = e.restyle_shapes(&mut doc, Restyle::Sides(2));
        assert_eq!(only(&doc).sides, crate::doc::MIN_SIDES);
        let _ = e.restyle_shapes(&mut doc, Restyle::Sides(500));
        assert_eq!(only(&doc).sides, crate::doc::MAX_SIDES);
        let _ = e.restyle_shapes(&mut doc, Restyle::Inner(1.5));
        assert!(only(&doc).inner < 1.0);
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Width(0.0)), Change::None, "no width is no stroke");
        assert_eq!(e.restyle_shapes(&mut doc, Restyle::Radius(f64::NAN)), Change::None);
    }

    #[test]
    fn what_each_one_wears_is_kept_to_be_put_back() {
        let (mut e, mut doc) = two_selected();
        let _ = e.restyle_shapes(&mut doc, Restyle::Fill(Some("#e5484d".into())));
        let fills = e.paints(&doc, false);
        let strokes = e.paints(&doc, true);
        assert_eq!(fills.len(), 1, "a line has no fill to keep");
        assert_eq!(fills[0].1.as_deref(), Some("#e5484d"));
        assert_eq!(strokes.len(), 2);
        // Tried as the pointer passes a menu's lines, then put back.
        let _ = e.restyle_shapes(&mut doc, Restyle::Fill(None));
        let _ = e.restyle_shapes(&mut doc, Restyle::Stroke(Some("#3b82f6".into())));
        e.repaint(&mut doc, &fills, false);
        e.repaint(&mut doc, &strokes, true);
        assert_eq!(only(&doc).fill.as_deref(), Some("#e5484d"));
        assert_eq!(only(&doc).stroke.as_deref(), Some(INK));
        assert_eq!(only_line(&doc).stroke, INK);
    }

    #[test]
    fn a_head_asked_of_the_next_line_makes_it_an_arrow_and_back() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Line, &mut doc);
        let _ = e.restyle_shapes(&mut doc, Restyle::Start(Head::Triangle));
        assert_eq!(e.figure, Figure::Arrow);
        assert_eq!(e.shape_look(&doc, INK).start, Head::Triangle);
        let _ = e.restyle_shapes(&mut doc, Restyle::Start(Head::None));
        let _ = e.restyle_shapes(&mut doc, Restyle::End(Head::None));
        assert_eq!(e.figure, Figure::Line, "no head left: a line again");
    }

    #[test]
    fn an_arrow_picked_with_no_head_left_in_the_style_grows_one_at_its_end() {
        let (mut e, mut doc) = shaping();
        e.choose_figure(Figure::Arrow, &mut doc);
        let _ = e.restyle_shapes(&mut doc, Restyle::End(Head::None));
        assert_eq!(e.figure, Figure::Line);
        let _ = e.set_figure(Figure::Arrow, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (100.0, 0.0));
        assert_eq!(only_line(&doc).end, Head::Arrow, "an arrow is never drawn bare");
        // Nor when the key picks it.
        assert!(e.escape(&mut doc), "the arrow just drawn let go of");
        let _ = e.restyle_shapes(&mut doc, Restyle::End(Head::None));
        e.choose_figure(Figure::Arrow, &mut doc);
        assert_eq!(e.shape_look(&doc, INK).end, Head::Arrow);
    }
}
