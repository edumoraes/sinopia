//! Shape Properties: the bar a shape is set from, in the middle over the
//! canvas as the brush's and the text's are.
//!
//! The line carries the eight figures the Shape tool draws, the fill and
//! the stroke — each a well that opens the dock's inks, or none — the
//! stroke's width, and what the figure lit has of its own: a rectangle's
//! radius, a polygon's sides, a star's points and how deep it is cut, a
//! line's two heads. It stands as wide whatever it shows, so nothing on
//! it moves from under the pointer when a figure is picked. What it shows
//! is where the bar is looking — the shapes selected, or how the next one
//! is drawn — which is the editor's to say; the bar only lays out, hits
//! and draws.
//!
//! Pure — `app` asks where a click landed and what to draw.

use crate::doc::{Head, MAX_INNER, MIN_INNER};
use crate::editor::{Restyle, ShapeLook};
use crate::menu::Item;
use crate::props::{self, HEIGHT, MARGIN};
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport};
use crate::select::End;
use crate::shape::Figure;
use crate::text::Atlas;
use crate::theme::{INK_NAMES, INKS, Theme};

// Logical px.
const PADDING: f32 = 10.0;
const GAP: f32 = 8.0;
/// A square button: a figure.
const BUTTON: f32 = 26.0;
/// A well — the fill, the stroke, a line's head — and the chevron that
/// says it opens a menu.
const WELL_W: f32 = 44.0;
const RADIUS: f32 = 12.0;
const BUTTON_RADIUS: f32 = 6.0;
/// A slider: its label, its track and its number, as the text's are.
const LABEL_W: f32 = 46.0;
const TRACK_W: f32 = 60.0;
const VALUE_W: f32 = 38.0;
const FIELD_W: f32 = LABEL_W + GAP + TRACK_W + GAP + VALUE_W;
const TRACK_H: f32 = 4.0;
const KNOB: f32 = 11.0;
/// A colour's swatch in its well.
const SWATCH: f32 = 14.0;
/// How deep the stroke's swatch draws its ring.
const SWATCH_RING: f32 = 3.0;
const ICON_BOX: f32 = 16.0;
const ICON_STROKE: f32 = 1.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
/// The bar's whole width: every figure, both wells, the width, and room
/// for the two controls the figure with the most of its own — a star —
/// brings.
const BAR_W: f32 = PADDING
    + Figure::ALL.len() as f32 * BUTTON
    + GAP
    + WELL_W
    + GAP / 2.0
    + WELL_W
    + GAP
    + FIELD_W
    + GAP
    + FIELD_W
    + GAP
    + FIELD_W
    + PADDING;

/// The widths the Width slider runs over, in world units, along a cube as
/// the brush's size does: most of the track is the widths lines are drawn
/// in. A shape can be stroked past either end, and the knob rests there.
const WIDTH_MIN: f64 = 0.5;
const WIDTH_MAX: f64 = 50.0;
const RADIUS_MAX: f64 = 200.0;
/// The sides the slider reaches; a figure may have up to sixty, and one
/// with more than this rests the knob at the end.
const SIDES_MIN: u32 = 3;
const SIDES_MAX: u32 = 24;

/// A value the bar sets by dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slider {
    Width,
    Radius,
    /// A polygon's sides.
    Sides,
    /// A star's points: its sides by another name.
    Points,
    /// How far in a star is cut, as its inner radius.
    Inner,
}

impl Slider {
    pub fn label(self) -> &'static str {
        match self {
            Slider::Width => "Width",
            Slider::Radius => "Radius",
            Slider::Sides => "Sides",
            Slider::Points => "Points",
            Slider::Inner => "Inner",
        }
    }

    /// Where `look` puts the knob, 0–1.
    pub fn fraction(self, look: &ShapeLook) -> f64 {
        let along = |v: f64, lo: f64, hi: f64| (v - lo) / (hi - lo);
        let f = match self {
            Slider::Width => along(look.width, WIDTH_MIN, WIDTH_MAX).max(0.0).cbrt(),
            Slider::Radius => along(look.radius, 0.0, RADIUS_MAX).max(0.0).cbrt(),
            Slider::Sides | Slider::Points => {
                along(f64::from(look.sides), f64::from(SIDES_MIN), f64::from(SIDES_MAX))
            }
            Slider::Inner => along(look.inner, MIN_INNER, MAX_INNER),
        };
        f.clamp(0.0, 1.0)
    }

    /// The change the knob at `f` stands for: a width to the half unit, a
    /// whole radius, whole sides, an inner radius to the hundredth.
    pub fn at(self, f: f64) -> Restyle {
        let f = f.clamp(0.0, 1.0);
        let between = |lo: f64, hi: f64, t: f64| lo + (hi - lo) * t;
        match self {
            Slider::Width => Restyle::Width((between(WIDTH_MIN, WIDTH_MAX, f.powi(3)) * 2.0).round() / 2.0),
            Slider::Radius => Restyle::Radius(between(0.0, RADIUS_MAX, f.powi(3)).round()),
            Slider::Sides | Slider::Points => {
                Restyle::Sides(between(f64::from(SIDES_MIN), f64::from(SIDES_MAX), f).round() as u32)
            }
            Slider::Inner => Restyle::Inner((between(MIN_INNER, MAX_INNER, f) * 100.0).round() / 100.0),
        }
    }

    /// The number written beside the track.
    pub fn format(self, look: &ShapeLook) -> String {
        match self {
            Slider::Width => format!("{}", (look.width * 10.0).round() / 10.0),
            Slider::Radius => format!("{}", look.radius.round()),
            Slider::Sides | Slider::Points => format!("{}", look.sides),
            Slider::Inner => format!("{}%", (look.inner * 100.0).round()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Figure(Figure),
    /// The fill's well: the inks open under it.
    Fill,
    Stroke,
    /// A line's head at that end: the heads open under it.
    Head(End),
    Slider(Slider),
    /// Bar chrome: swallowed, never reaches the canvas.
    Bar,
}

/// A slider's three parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub slider: Slider,
    pub label: ScreenRect,
    pub track: ScreenRect,
    pub value: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeBar {
    pub rect: ScreenRect,
    pub figures: Vec<(Figure, ScreenRect)>,
    pub fill: ScreenRect,
    pub stroke: ScreenRect,
    /// The width, then what the figure has of its own.
    pub fields: Vec<Field>,
    /// A line's two heads; none for a shape.
    pub heads: Vec<(End, ScreenRect)>,
    /// The figure the bar was laid out for: a line has no fill.
    figure: Figure,
    scale: f32,
}

impl ShapeBar {
    /// The bar for `figure`, the one lit. `top` is where the tab strip
    /// ends, in physical px.
    pub fn layout(viewport: Viewport, scale: f64, top: f32, figure: Figure) -> ShapeBar {
        let s = scale as f32;
        let w = BAR_W * s;
        let bar = ScreenRect {
            x: props::centred(viewport, w, s),
            y: (top + MARGIN * s).round(),
            w,
            h: HEIGHT * s,
        };
        let button = |at: f32, width: f32| ScreenRect {
            x: at,
            y: bar.y + (bar.h - BUTTON * s) / 2.0,
            w: width * s,
            h: BUTTON * s,
        };
        let field = |slider: Slider, fx: f32| Field {
            slider,
            label: ScreenRect {
                x: fx,
                y: bar.y,
                w: LABEL_W * s,
                h: bar.h,
            },
            track: ScreenRect {
                x: fx + (LABEL_W + GAP) * s,
                y: bar.y + (bar.h - TRACK_H * s) / 2.0,
                w: TRACK_W * s,
                h: TRACK_H * s,
            },
            value: ScreenRect {
                x: fx + (LABEL_W + GAP + TRACK_W + GAP) * s,
                y: bar.y,
                w: VALUE_W * s,
                h: bar.h,
            },
        };
        let mut pen = bar.x + PADDING * s;
        let figures = Figure::ALL
            .iter()
            .enumerate()
            .map(|(i, &f)| (f, button(pen + i as f32 * BUTTON * s, BUTTON)))
            .collect();
        pen += (Figure::ALL.len() as f32 * BUTTON + GAP) * s;
        let fill = button(pen, WELL_W);
        pen += (WELL_W + GAP / 2.0) * s;
        let stroke = button(pen, WELL_W);
        pen += (WELL_W + GAP) * s;
        let mut fields = vec![field(Slider::Width, pen)];
        pen += (FIELD_W + GAP) * s;
        let next = pen + (FIELD_W + GAP) * s;
        let mut heads = Vec::new();
        match figure {
            Figure::Rectangle => fields.push(field(Slider::Radius, pen)),
            Figure::Polygon => fields.push(field(Slider::Sides, pen)),
            Figure::Star => {
                fields.push(field(Slider::Points, pen));
                fields.push(field(Slider::Inner, next));
            }
            Figure::Line | Figure::Arrow => {
                heads.push((End::From, button(pen, WELL_W)));
                heads.push((End::To, button(pen + (WELL_W + GAP / 2.0) * s, WELL_W)));
            }
            Figure::Ellipse | Figure::Triangle | Figure::Diamond => {}
        }
        ShapeBar {
            rect: bar,
            figures,
            fill,
            stroke,
            fields,
            heads,
            figure,
            scale: s,
        }
    }

    /// Whether the figure laid out for has a fill to set: a line has none.
    fn fills(&self) -> bool {
        self.figure.model().is_some()
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if let Some((f, _)) = self.figures.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Hit::Figure(*f));
        }
        if self.fill.contains(x, y) {
            return Some(if self.fills() { Hit::Fill } else { Hit::Bar });
        }
        if self.stroke.contains(x, y) {
            return Some(Hit::Stroke);
        }
        if let Some((end, _)) = self.heads.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Hit::Head(*end));
        }
        // A slider is grabbed by its track and its number together, as
        // the text's are.
        let grab = |f: &Field| {
            let r = f.track.union(&f.value);
            ScreenRect {
                y: f.value.y,
                h: f.value.h,
                ..r
            }
        };
        Some(match self.fields.iter().find(|f| grab(f).contains(x, y)) {
            Some(f) => Hit::Slider(f.slider),
            None => Hit::Bar,
        })
    }

    /// Where `x` lands along a slider's track, 0–1.
    pub fn fraction(&self, slider: Slider, x: f64) -> f64 {
        let Some(f) = self.fields.iter().find(|f| f.slider == slider) else {
            return 0.0;
        };
        if f.track.w <= 0.0 {
            return 0.0;
        }
        ((x - f64::from(f.track.x)) / f64::from(f.track.w)).clamp(0.0, 1.0)
    }

    pub fn prims(&self, look: &ShapeLook, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(RADIUS, s);
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                corner,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-b), corner + b, theme.border),
            Prim::rounded(self.rect, corner, theme.panel),
        ];
        let button_corner = theme.corner(BUTTON_RADIUS, s);
        let text = |out: &mut Vec<Prim>, words: &str, r: ScreenRect, color: Rgba| {
            let baseline = atlas.baseline_in(r);
            let cut = atlas.truncate(words, r.w);
            for g in atlas.layout(&cut, r.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, color).clipped(self.rect));
            }
        };

        for (f, r) in &self.figures {
            let on = *f == look.figure;
            if on {
                out.push(Prim::rounded(*r, button_corner, theme.active_bg));
            }
            out.extend(icon_prims(figure_icon(*f), *r, s, if on { theme.icon_active } else { theme.icon }));
        }

        // A well: a bordered button, what it holds on its left and the
        // chevron that says it opens on its right.
        let well = |out: &mut Vec<Prim>, r: ScreenRect, muted: bool| -> ScreenRect {
            out.push(Prim::rounded(r.inset(-b), button_corner + b, theme.border));
            out.push(Prim::rounded(r, button_corner, theme.panel));
            let arrow = ScreenRect {
                x: r.x + r.w - r.h,
                w: r.h,
                ..r
            };
            out.extend(icon_prims(CHEVRON, arrow, s, if muted { theme.muted } else { theme.icon }));
            ScreenRect { w: r.w - arrow.w, ..r }
        };
        let swatch = |r: ScreenRect| {
            let side = SWATCH * s;
            let (cx, cy) = r.center();
            ScreenRect {
                x: cx + (GAP / 2.0) * s - side / 2.0,
                y: cy - side / 2.0,
                w: side,
                h: side,
            }
        };
        let no_colour = |out: &mut Vec<Prim>, r: ScreenRect, color: Rgba| {
            out.push(Prim::rounded(r.inset(-b), 2.0 * s + b, theme.border));
            out.push(Prim::rounded(r, 2.0 * s, theme.panel));
            out.push(Prim::segment(
                (r.x + r.w, r.y),
                (r.x, r.y + r.h),
                ICON_STROKE / 2.0 * s,
                color,
            ));
        };
        let muted = !self.fills();
        let room = well(&mut out, self.fill, muted);
        let at = swatch(room);
        match look.fill.as_deref().filter(|_| !muted) {
            Some(hex) => {
                out.push(Prim::rounded(at.inset(-b), 2.0 * s + b, theme.border));
                out.push(Prim::rounded(at, 2.0 * s, scene::parse_color(hex)));
            }
            None => no_colour(&mut out, at, if muted { theme.muted } else { NONE }),
        }
        let room = well(&mut out, self.stroke, false);
        let at = swatch(room);
        match look.stroke.as_deref() {
            Some(hex) => {
                let ring = SWATCH_RING * s;
                out.push(Prim::rounded(at.inset(-b), 2.0 * s + b, theme.border));
                out.push(Prim::rounded(at, 2.0 * s, scene::parse_color(hex)));
                out.push(Prim::rounded(at.inset(ring), 1.0 * s, theme.border));
                out.push(Prim::rounded(at.inset(ring + b), 1.0 * s, theme.panel));
            }
            None => no_colour(&mut out, at, NONE),
        }
        for (end, r) in &self.heads {
            let room = well(&mut out, *r, false);
            let head = match end {
                End::From => look.start,
                End::To => look.end,
            };
            out.extend(icon_prims(&head_icon(head, *end), room, s, theme.icon));
        }

        for f in &self.fields {
            text(&mut out, f.slider.label(), f.label, theme.ink);
            let t = f.track;
            out.push(Prim::rounded(t, t.h / 2.0, theme.border));
            let frac = f.slider.fraction(look) as f32;
            out.push(Prim::rounded(
                ScreenRect {
                    w: frac * t.w,
                    ..t
                },
                t.h / 2.0,
                theme.icon_active,
            ));
            let side = KNOB * s;
            let knob = ScreenRect {
                x: t.x + frac * t.w - side / 2.0,
                y: t.y + (t.h - side) / 2.0,
                w: side,
                h: side,
            };
            out.push(Prim::rounded(knob.inset(-b), side / 2.0 + b, theme.border));
            out.push(Prim::rounded(knob, side / 2.0, theme.panel));
            text(&mut out, &f.slider.format(look), f.value, theme.ink);
        }
        out
    }
}

/// The lines a well's menu offers, and the paint each one is: none —
/// called `none`, and offered only where `can_none` says the paint may go
/// — then every ink of the dock's, under a rule and each with its dot. The
/// one `current` names is checked.
pub fn paint_menu(current: Option<&str>, none: &str, can_none: bool) -> (Vec<Item>, Vec<Option<String>>) {
    let mut items = vec![Item::new(none).checked(current.is_none()).enabled(can_none)];
    let mut paints = vec![None];
    for (i, (hex, name)) in INKS.iter().zip(INK_NAMES).enumerate() {
        let item = Item::new(name)
            .dot(scene::parse_color(hex))
            .checked(current.is_some_and(|c| c.eq_ignore_ascii_case(hex)));
        items.push(if i == 0 { item.ruled() } else { item });
        paints.push(Some((*hex).to_owned()));
    }
    (items, paints)
}

/// The lines a head's menu offers, and the head each one is.
pub fn head_menu(current: Head) -> (Vec<Item>, Vec<Head>) {
    let heads = vec![Head::None, Head::Arrow, Head::Triangle];
    let items = heads
        .iter()
        .map(|h| {
            let name = match h {
                Head::None => "None",
                Head::Arrow => "Arrow",
                Head::Triangle => "Triangle",
            };
            Item::new(name).checked(*h == current)
        })
        .collect();
    (items, heads)
}

/// The slash across a well holding no colour: the red every design tool
/// strikes "none" through with. Fixed, as the dock's inks are.
const NONE: Rgba = [0.9, 0.1, 0.1, 1.0];

type Icon = &'static [&'static [(f32, f32)]];

const CHEVRON: Icon = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];

/// A figure as the bar draws its button.
fn figure_icon(f: Figure) -> Icon {
    match f {
        Figure::Rectangle => &[&[(4.0, 6.0), (20.0, 6.0), (20.0, 18.0), (4.0, 18.0), (4.0, 6.0)]],
        Figure::Ellipse => &[&[
            (21.0, 12.0),
            (19.8, 15.5),
            (16.5, 18.1),
            (12.0, 19.0),
            (7.5, 18.1),
            (4.2, 15.5),
            (3.0, 12.0),
            (4.2, 8.5),
            (7.5, 5.9),
            (12.0, 5.0),
            (16.5, 5.9),
            (19.8, 8.5),
            (21.0, 12.0),
        ]],
        Figure::Triangle => &[&[(12.0, 4.0), (21.0, 19.5), (3.0, 19.5), (12.0, 4.0)]],
        Figure::Diamond => &[&[(12.0, 3.0), (21.0, 12.0), (12.0, 21.0), (3.0, 12.0), (12.0, 3.0)]],
        Figure::Polygon => &[&[
            (12.0, 3.0),
            (20.0, 7.5),
            (20.0, 16.5),
            (12.0, 21.0),
            (4.0, 16.5),
            (4.0, 7.5),
            (12.0, 3.0),
        ]],
        Figure::Star => &[&[
            (12.0, 2.5),
            (14.4, 8.9),
            (21.0, 9.2),
            (15.8, 13.4),
            (17.6, 20.0),
            (12.0, 16.2),
            (6.4, 20.0),
            (8.2, 13.4),
            (3.0, 9.2),
            (9.6, 8.9),
            (12.0, 2.5),
        ]],
        Figure::Line => &[&[(4.0, 20.0), (20.0, 4.0)]],
        Figure::Arrow => &[&[(4.0, 20.0), (20.0, 4.0)], &[(11.0, 4.0), (20.0, 4.0), (20.0, 13.0)]],
    }
}

/// A line's `head` as the well at its `end` shows it: the line running
/// in from the other side, and what stands on it.
fn head_icon(head: Head, end: End) -> Vec<Vec<(f32, f32)>> {
    let lines: Vec<Vec<(f32, f32)>> = match head {
        Head::None => vec![vec![(4.0, 12.0), (19.0, 12.0)]],
        Head::Arrow => vec![vec![(4.0, 12.0), (19.0, 12.0)], vec![(14.0, 7.0), (19.0, 12.0), (14.0, 17.0)]],
        Head::Triangle => vec![
            vec![(4.0, 12.0), (13.0, 12.0)],
            vec![(13.0, 7.5), (19.0, 12.0), (13.0, 16.5), (13.0, 7.5)],
        ],
    };
    match end {
        End::To => lines,
        // The start faces the other way.
        End::From => lines
            .into_iter()
            .map(|l| l.into_iter().map(|(x, y)| (24.0 - x, y)).collect())
            .collect(),
    }
}

fn icon_prims<L: AsRef<[(f32, f32)]>>(lines: &[L], r: ScreenRect, s: f32, color: Rgba) -> Vec<Prim> {
    let lines: Vec<&[(f32, f32)]> = lines.iter().map(AsRef::as_ref).collect();
    scene::icon_prims(&lines, r, 24.0, ICON_BOX, ICON_STROKE, s, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::Tabs;
    use crate::text::Font;

    const VP: Viewport = Viewport { w: 1400, h: 800 };
    const TOP: f32 = 34.0;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    fn mid(r: ScreenRect) -> (f64, f64) {
        (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0))
    }

    fn look(figure: Figure) -> ShapeLook {
        ShapeLook {
            figure,
            fill: Some("#e5484d".into()),
            stroke: Some("#000000".into()),
            width: 2.0,
            radius: 12.0,
            sides: 5,
            inner: 0.38,
            start: Head::None,
            end: Head::Arrow,
        }
    }

    fn sliders(bar: &ShapeBar) -> Vec<Slider> {
        bar.fields.iter().map(|f| f.slider).collect()
    }

    #[test]
    fn the_bar_stands_in_the_middle_under_the_strip() {
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Rectangle);
        let middle = bar.rect.x + bar.rect.w / 2.0;
        assert!((middle - VP.w as f32 / 2.0).abs() <= 0.5);
        assert_eq!(bar.rect.y, TOP + MARGIN);
        assert_eq!(bar.rect.h, HEIGHT);
    }

    #[test]
    fn it_carries_every_figure_both_wells_and_the_width() {
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Star);
        let figures: Vec<Figure> = bar.figures.iter().map(|(f, _)| *f).collect();
        assert_eq!(figures, Figure::ALL);
        let rects: Vec<ScreenRect> = bar
            .figures
            .iter()
            .map(|(_, r)| *r)
            .chain([bar.fill, bar.stroke])
            .chain(bar.fields.iter().flat_map(|f| [f.label, f.track, f.value]))
            .collect();
        for (i, r) in rects.iter().enumerate() {
            assert!(bar.rect.contains_rect(r), "{r:?} is outside");
            for q in &rects[i + 1..] {
                assert!(r.intersect(q).is_none(), "{r:?} overlaps {q:?}");
            }
        }
    }

    #[test]
    fn each_figure_brings_what_it_has_of_its_own() {
        let at = |f| ShapeBar::layout(VP, 1.0, TOP, f);
        assert_eq!(sliders(&at(Figure::Rectangle)), [Slider::Width, Slider::Radius]);
        for f in [Figure::Ellipse, Figure::Triangle, Figure::Diamond] {
            assert_eq!(sliders(&at(f)), [Slider::Width], "{f:?}");
            assert!(at(f).heads.is_empty());
        }
        assert_eq!(sliders(&at(Figure::Polygon)), [Slider::Width, Slider::Sides]);
        assert_eq!(sliders(&at(Figure::Star)), [Slider::Width, Slider::Points, Slider::Inner]);
        for f in [Figure::Line, Figure::Arrow] {
            let bar = at(f);
            assert_eq!(sliders(&bar), [Slider::Width], "{f:?}");
            let ends: Vec<End> = bar.heads.iter().map(|(e, _)| *e).collect();
            assert_eq!(ends, [End::From, End::To]);
            for (_, r) in &bar.heads {
                assert!(bar.rect.contains_rect(r));
            }
        }
    }

    #[test]
    fn nothing_on_the_line_moves_when_another_figure_is_picked() {
        let rect = ShapeBar::layout(VP, 1.0, TOP, Figure::Rectangle);
        for f in Figure::ALL {
            let other = ShapeBar::layout(VP, 1.0, TOP, f);
            assert_eq!(other.rect, rect.rect, "{f:?}");
            assert_eq!(other.figures, rect.figures);
            assert_eq!((other.fill, other.stroke), (rect.fill, rect.stroke));
            assert_eq!(other.fields[0], rect.fields[0], "the width stands where it stood");
        }
    }

    #[test]
    fn hit_names_every_control() {
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Star);
        let at = |bar: &ShapeBar, r: ScreenRect| {
            let (x, y) = mid(r);
            bar.hit(x, y)
        };
        assert_eq!(at(&bar, bar.figures[5].1), Some(Hit::Figure(Figure::Star)));
        assert_eq!(at(&bar, bar.fill), Some(Hit::Fill));
        assert_eq!(at(&bar, bar.stroke), Some(Hit::Stroke));
        assert_eq!(at(&bar, bar.fields[0].track), Some(Hit::Slider(Slider::Width)));
        assert_eq!(at(&bar, bar.fields[2].value), Some(Hit::Slider(Slider::Inner)));
        assert_eq!(bar.hit(f64::from(bar.rect.x + 2.0), f64::from(bar.rect.y + 2.0)), Some(Hit::Bar));
        assert_eq!(bar.hit(1.0, 700.0), None);
        let line = ShapeBar::layout(VP, 1.0, TOP, Figure::Arrow);
        assert_eq!(at(&line, line.heads[1].1), Some(Hit::Head(End::To)));
        assert_eq!(at(&line, line.fill), Some(Hit::Bar), "a line has no fill to open");
    }

    #[test]
    fn a_slider_sets_what_its_knob_stands_for() {
        let mut l = look(Figure::Star);
        let take = |l: &mut ShapeLook, r: Restyle| match r {
            Restyle::Width(w) => l.width = w,
            Restyle::Radius(v) => l.radius = v,
            Restyle::Sides(n) => l.sides = n,
            Restyle::Inner(i) => l.inner = i,
            other => panic!("a slider asked for {other:?}"),
        };
        for slider in [Slider::Width, Slider::Radius, Slider::Sides, Slider::Points, Slider::Inner] {
            take(&mut l, slider.at(0.0));
            assert_eq!(slider.fraction(&l), 0.0, "{slider:?}");
            take(&mut l, slider.at(1.0));
            assert_eq!(slider.fraction(&l), 1.0, "{slider:?}");
        }
        assert_eq!((l.width, l.radius, l.sides), (WIDTH_MAX, RADIUS_MAX, SIDES_MAX));
        assert!((l.inner - MAX_INNER).abs() < 1e-9);
        assert_eq!(Slider::Width.at(0.5), Restyle::Width(6.5), "most of the track is the thin widths");
        l.width = 2.0;
        let f = Slider::Width.fraction(&l);
        assert_eq!(Slider::Width.at(f), Restyle::Width(2.0), "a width set where it stands stays");
        l.sides = 60;
        assert_eq!(Slider::Sides.fraction(&l), 1.0, "past the end rests at the end");
        assert_eq!(Slider::Inner.format(&look(Figure::Star)), "38%");
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Star);
        let t = bar.fields[0].track;
        assert_eq!(bar.fraction(Slider::Width, f64::from(t.x + t.w)), 1.0);
        assert_eq!(bar.fraction(Slider::Width, f64::from(t.x) - 50.0), 0.0);
    }

    #[test]
    fn the_figure_shown_is_lit() {
        let theme = Theme::light();
        for f in Figure::ALL {
            let bar = ShapeBar::layout(VP, 1.0, TOP, f);
            let lit: Vec<Prim> = bar
                .prims(&look(f), &atlas(), 3, &theme)
                .into_iter()
                .filter(|p| p.color == theme.active_bg)
                .collect();
            assert_eq!(lit.len(), 1, "{f:?}");
            let r = bar.figures.iter().find(|(g, _)| *g == f).unwrap().1;
            assert!(r.contains_rect(&lit[0].bounds()));
        }
    }

    #[test]
    fn a_well_shows_its_colour_or_is_struck_through() {
        let theme = Theme::light();
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Rectangle);
        let red = scene::parse_color("#e5484d");
        let in_fill = |prims: &[Prim]| -> Vec<Prim> {
            prims.iter().filter(|p| bar.fill.contains_rect(&p.bounds())).copied().collect()
        };
        let filled = in_fill(&bar.prims(&look(Figure::Rectangle), &atlas(), 3, &theme));
        assert!(filled.iter().any(|p| p.color == red), "the fill's own colour");
        let none = ShapeLook { fill: None, ..look(Figure::Rectangle) };
        let struck = in_fill(&bar.prims(&none, &atlas(), 3, &theme));
        assert!(!struck.iter().any(|p| p.color == red));
        assert!(struck.iter().any(|p| p.kind == scene::KIND_SEGMENT && p.color == NONE), "struck through");
    }

    #[test]
    fn a_lines_fill_is_muted() {
        let theme = Theme::light();
        let bar = ShapeBar::layout(VP, 1.0, TOP, Figure::Line);
        let prims = bar.prims(&look(Figure::Line), &atlas(), 3, &theme);
        let marks: Vec<&Prim> = prims
            .iter()
            .filter(|p| p.kind == scene::KIND_SEGMENT && bar.fill.contains_rect(&p.bounds()))
            .collect();
        assert!(!marks.is_empty());
        assert!(marks.iter().all(|p| p.color == theme.muted), "{marks:?}");
    }

    #[test]
    fn nothing_the_bar_draws_escapes_it() {
        let theme = Theme::light();
        for f in Figure::ALL {
            let bar = ShapeBar::layout(VP, 1.0, TOP, f);
            for p in bar.prims(&look(f), &atlas(), 3, &theme).iter().skip(3) {
                let r = p.bounds();
                assert!(bar.rect.inset(-1.0).contains_rect(&r), "{f:?}: {r:?} past {:?}", bar.rect);
            }
        }
    }

    #[test]
    fn every_label_and_number_fits_the_room_it_gets() {
        let a = atlas();
        let all = [Slider::Width, Slider::Radius, Slider::Sides, Slider::Points, Slider::Inner];
        for slider in all {
            assert!(a.measure(slider.label()) <= LABEL_W, "{slider:?}");
        }
        let widest = ShapeLook {
            width: 49.5,
            radius: 200.0,
            sides: 60,
            inner: 0.95,
            ..look(Figure::Star)
        };
        for slider in all {
            let v = slider.format(&widest);
            assert!(a.measure(&v) <= VALUE_W, "{v}");
        }
    }

    #[test]
    fn layout_scales_with_the_display() {
        let one = ShapeBar::layout(VP, 1.0, TOP, Figure::Star);
        let two = ShapeBar::layout(Viewport { w: 2800, h: 1600 }, 2.0, TOP, Figure::Star);
        assert_eq!(two.rect.w, one.rect.w * 2.0);
        assert_eq!(two.fill.w, one.fill.w * 2.0);
    }

    #[test]
    fn the_paint_menu_is_none_then_every_ink_with_its_dot() {
        use crate::theme::{INK_NAMES, INKS};
        let (items, paints) = paint_menu(Some(INKS[2]), "No Fill", true);
        assert_eq!(items.len(), INKS.len() + 1);
        assert_eq!(paints[0], None);
        assert_eq!(items[0].label, "No Fill");
        for (i, hex) in INKS.iter().enumerate() {
            assert_eq!(paints[i + 1].as_deref(), Some(*hex));
            assert_eq!(items[i + 1].label, INK_NAMES[i]);
            assert_eq!(items[i + 1].dot, Some(scene::parse_color(hex)));
        }
        let checked: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.checked).map(|(k, _)| k).collect();
        assert_eq!(checked, [3], "the ink it wears");
        assert!(items[1].rule, "a rule between none and the inks");
        let (items, _) = paint_menu(None, "No Stroke", false);
        assert!(!items[0].enabled, "a line cannot go without its ink");
    }

    #[test]
    fn the_head_menu_offers_every_head() {
        let (items, heads) = head_menu(Head::Triangle);
        assert_eq!(heads, [Head::None, Head::Arrow, Head::Triangle]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["None", "Arrow", "Triangle"]);
        assert!(items[2].checked && !items[0].checked);
    }
}
