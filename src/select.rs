//! Selection (§6.2 session state): what an element occupies, what is under
//! the pointer, the handles around a selection and the transforms those
//! handles drive. Pure — `editor` decides when, this decides what.

use crate::curve::{self, Cubic};
use crate::doc::{Document, Element, Line, MAX_TEXT_SIZE, MIN_TEXT_SIZE, Shape, TextMode};
use crate::geom::{Affine, Corner, Frame, Point};
use crate::scene::{Prim, ScreenRect, View, with_alpha};
use crate::theme::Theme;

// Logical px.
/// Side of a resize handle.
pub const HANDLE_PX: f32 = 8.0;
/// Diameter of a rotation handle.
pub const ROTATE_HANDLE_PX: f32 = 8.0;
/// How far past its corner a rotation handle sits, along the corner's
/// diagonal.
pub const ROTATE_OFFSET_PX: f32 = 14.0;
/// Extra reach around a handle when hit-testing.
pub const HANDLE_SLOP_PX: f32 = 4.0;
const OUTLINE_PX: f32 = 1.0;
const HANDLE_BORDER_PX: f32 = 1.5;
const RING_PX: f32 = 1.5;
/// Marquee fill opacity.
const MARQUEE_ALPHA: f32 = 0.1;

/// A control on the selection frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Drags a corner, the opposite one stays put.
    Resize(Corner),
    /// Sits outside a corner; turns the selection about its center.
    Rotate(Corner),
}

/// The oriented box an element occupies, in world units: a rect's own box
/// and turn; for a path, the box around its ink (curve bounds grown by
/// half the width) as it was drawn, turned by the path's rotation. `None`
/// for a path with no curves.
pub fn frame(el: &Element) -> Option<Frame> {
    match el {
        Element::Rect(r) => Some(box_frame(r.x, r.y, r.w, r.h, r.rotation)),
        Element::Shape(s) => Some(box_frame(s.x, s.y, s.w, s.h, s.rotation)),
        Element::Line(l) => Some(line_frame(l)),
        Element::Image(i) => Some(box_frame(i.x, i.y, i.w, i.h, i.rotation)),
        Element::Path(p) => ink_frame([(p.curves.as_slice(), p.width)], p.rotation),
        Element::Paint(p) => ink_frame(
            p.strokes.iter().map(|s| (s.curves.as_slice(), s.width)),
            p.rotation,
        ),
        // An area, and one that does not turn: the cut that makes a
        // frame is an axis-aligned box in the shader.
        Element::Frame(f) => Some(box_frame(f.x, f.y, f.w, f.h, 0.0)),
        Element::Text(t) => Some(box_frame(t.x, t.y, t.w, t.h, t.rotation)),
    }
}

/// The box a line's ink takes: along it from end to end and past each by
/// its round cap, turned as it runs; across it, as wide as it is or as its
/// widest head opens.
fn line_frame(l: &Line) -> Frame {
    let d = [l.to[0] - l.from[0], l.to[1] - l.from[1]];
    Frame {
        center: [(l.from[0] + l.to[0]) / 2.0, (l.from[1] + l.to[1]) / 2.0],
        half: [d[0].hypot(d[1]) / 2.0 + l.width / 2.0, crate::shape::line_reach(l)],
        angle: d[1].atan2(d[0]),
    }
}

/// The frame of an element that is a box on disk — a rect or an image:
/// `x, y, w, h` before the turn, turned about its own center.
fn box_frame(x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Frame {
    Frame {
        center: [x + w / 2.0, y + h / 2.0],
        half: [w / 2.0, h / 2.0],
        angle: rotation.to_radians(),
    }
}

/// The `x, y, w, h, rotation` a frame describes — the way back in.
fn box_fields(f: Frame) -> (f64, f64, f64, f64, f64) {
    (
        f.center[0] - f.half[0],
        f.center[1] - f.half[1],
        2.0 * f.half[0],
        2.0 * f.half[1],
        degrees(f.angle),
    )
}

fn find<'a>(doc: &'a Document, id: &str) -> Option<&'a Element> {
    doc.elements.iter().find(|el| el.id() == id)
}

/// The frame around `ids`: a lone element keeps its own turn; several are
/// wrapped by the unturned box around all their corners. Ids not in the
/// document are ignored.
pub fn frame_of(doc: &Document, ids: &[String]) -> Option<Frame> {
    let frames: Vec<Frame> = ids.iter().filter_map(|id| frame(find(doc, id)?)).collect();
    match frames.as_slice() {
        [] => None,
        [one] => Some(*one),
        many => {
            let corners: Vec<Point> = many.iter().flat_map(Frame::corners).collect();
            Frame::around(&corners)
        }
    }
}

/// The topmost element under `p` (world units), reaching `slop` beyond its
/// edges — topmost as painted, so the layers' order counts before the
/// document's; a hidden layer is never hit, and the pointer passes through
/// a locked one to what is under it. Rects hit anywhere inside; paths only
/// on their ink.
pub fn element_at(doc: &Document, p: Point, slop: f64) -> Option<&str> {
    doc.painted()
        .rev()
        .filter(|painted| !painted.locked)
        .find(|painted| {
            // What the boundary cut away is not there for the pointer
            // either: the renderer and the pointer read one answer.
            painted.within.is_none_or(|f| f.contains(p)) && hits(painted.element, p, slop)
        })
        .map(|painted| painted.element.id())
}

fn hits(el: &Element, p: Point, slop: f64) -> bool {
    let Some(f) = frame(el) else {
        return false;
    };
    if !f.contains(p, slop) {
        return false;
    }
    match el {
        // A bitmap is opaque to the pointer: the box decides, not the pixels.
        Element::Rect(_) | Element::Image(_) | Element::Text(_) => true,
        Element::Shape(s) => shape_hit(s, &f, p, slop),
        Element::Line(l) => crate::shape::line_distance(l, p) <= slop,
        Element::Path(path) => ink_hit(&path.curves, path.width, p, slop),
        // A frame is an area with a surface, not an outline. It is
        // painted before what it holds, so a walk from the top finds
        // the contents first: the frame is picked on its own ground.
        Element::Frame(_) => true,
        // Any one of its strokes is the object: the gaps between them
        // are not.
        Element::Paint(paint) => paint
            .strokes
            .iter()
            .any(|s| ink_hit(&s.curves, s.width, p, slop)),
    }
}

/// Whether `p` is on what shape `s` shows, framed by `f`: anywhere in its
/// figure when it is filled, and on its stroke alone when it is hollow —
/// the stroke laid inside the edge, `width` deep. A hollow shape drawn
/// round other things is not in the way of the pointer reaching them.
fn shape_hit(s: &Shape, f: &Frame, p: Point, slop: f64) -> bool {
    let d = crate::shape::distance(s, f.to_local(p));
    match s.fill {
        Some(_) => d <= slop,
        None => d <= slop && d >= -(s.width + slop),
    }
}

/// The oriented box around strokes drawn together: each one's curve
/// bounds grown by half its width, unturned by `rotation` so the box is
/// the one they were drawn in, then turned back. `None` when there is no
/// ink at all.
fn ink_frame<'a>(strokes: impl IntoIterator<Item = (&'a [Cubic], f64)>, rotation: f64) -> Option<Frame> {
    let angle = rotation.to_radians();
    let back = Affine::rotate(-angle);
    let mut span: Option<([f64; 2], [f64; 2])> = None;
    for (curves, width) in strokes {
        let drawn: Vec<Cubic> = curves
            .iter()
            .map(|c| c.map(|point| back.apply(point)))
            .collect();
        let Some((lo, hi)) = curve::bounds(&drawn) else {
            continue;
        };
        let pad = width / 2.0;
        let (lo, hi) = ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad]);
        span = Some(match span {
            None => (lo, hi),
            Some((l, h)) => (
                [l[0].min(lo[0]), l[1].min(lo[1])],
                [h[0].max(hi[0]), h[1].max(hi[1])],
            ),
        });
    }
    let (lo, hi) = span?;
    let unturned = Frame::spanning(lo, hi);
    Some(Frame {
        center: Affine::rotate(angle).apply(unturned.center),
        half: unturned.half,
        angle,
    })
}

/// Whether `p` is within `slop` of ink `width` wide laid along `curves`.
fn ink_hit(curves: &[Cubic], width: f64, p: Point, slop: f64) -> bool {
    let reach = width / 2.0 + slop;
    // Flatten far finer than the reach so the polyline's error cannot
    // decide the answer.
    let tolerance = (reach / 20.0).max(1e-6);
    curves.iter().any(|c| {
        curve::flatten(c, tolerance)
            .windows(2)
            .any(|w| curve::point_segment_distance(p, w[0], w[1]) <= reach)
    })
}

/// Ids of the painted elements whose box overlaps the unturned box with
/// `a` and `b` as opposite corners, in paint order — none that is locked.
pub fn elements_in(doc: &Document, a: Point, b: Point) -> Vec<String> {
    let lo = [a[0].min(b[0]), a[1].min(b[1])];
    let hi = [a[0].max(b[0]), a[1].max(b[1])];
    let overlaps = |flo: Point, fhi: Point| {
        flo[0] <= hi[0] && fhi[0] >= lo[0] && flo[1] <= hi[1] && fhi[1] >= lo[1]
    };
    doc.painted()
        .filter(|painted| !painted.locked)
        .filter(|painted| {
            // A boundary that cut ink away keeps the marquee off it too.
            painted
                .within
                .is_none_or(|f| overlaps([f.x, f.y], [f.x + f.w, f.y + f.h]))
                && frame(painted.element).is_some_and(|f| {
                    let (flo, fhi) = f.aabb();
                    overlaps(flo, fhi)
                })
        })
        .map(|painted| painted.element.id().to_owned())
        .collect()
}

/// Applies `m` to an element. Path control points map exactly and the
/// path's rotation follows the map's turn; a rect and an image map their
/// frame (see [`Frame::transformed`]).
pub fn transform(el: &mut Element, m: &Affine) {
    match el {
        Element::Path(p) => {
            for c in &mut p.curves {
                for point in c {
                    *point = m.apply(*point);
                }
            }
            p.rotation = turned(p.rotation, m);
        }
        Element::Paint(p) => {
            for s in &mut p.strokes {
                for c in &mut s.curves {
                    for point in c {
                        *point = m.apply(*point);
                    }
                }
            }
            p.rotation = turned(p.rotation, m);
        }
        Element::Rect(r) => {
            (r.x, r.y, r.w, r.h, r.rotation) =
                box_fields(box_frame(r.x, r.y, r.w, r.h, r.rotation).transformed(m));
        }
        Element::Image(i) => {
            (i.x, i.y, i.w, i.h, i.rotation) =
                box_fields(box_frame(i.x, i.y, i.w, i.h, i.rotation).transformed(m));
        }
        // A line's ends are what it is, and map exactly.
        Element::Line(l) => {
            l.from = m.apply(l.from);
            l.to = m.apply(l.to);
        }
        // A box that only turns cannot mirror: a map that does is the
        // box's turn and the model flipped inside it.
        Element::Shape(s) => {
            (s.x, s.y, s.w, s.h, s.rotation) =
                box_fields(box_frame(s.x, s.y, s.w, s.h, s.rotation).transformed(m));
            if m.a * m.d - m.b * m.c < 0.0 {
                s.flip = !s.flip;
            }
        }
        // A frame's box is mapped and its lines reflow in it. Artistic
        // text is its letters: it is scaled evenly, by the middle of
        // what the map does to its two sides, so a stretch grows the
        // letters rather than distorting them — and never past the sizes
        // a text may be set at — and stands centred where the map put
        // it, turned as the map turned it.
        Element::Text(t) => {
            let mapped = box_frame(t.x, t.y, t.w, t.h, t.rotation).transformed(m);
            let (x, y, w, h, rotation) = box_fields(mapped);
            if t.mode == TextMode::Frame {
                (t.x, t.y, t.w, t.h, t.rotation) = (x, y, w, h, rotation);
                return;
            }
            let stretch = |new: f64, old: f64| if old > 0.0 { new / old } else { 1.0 };
            let k = (stretch(w, t.w) * stretch(h, t.h)).sqrt();
            let k = if k.is_finite() { k } else { 1.0 };
            let size = (t.style.size * k).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
            let k = size / t.style.size;
            t.style.size = size;
            // Every stretch set at a size of its own scales with the rest,
            // within what a text may be set at.
            for run in &mut t.runs {
                if let Some(s) = &mut run.style.size {
                    *s = (*s * k).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
                }
            }
            (t.w, t.h) = (t.w * k, t.h * k);
            t.x = mapped.center[0] - t.w / 2.0;
            t.y = mapped.center[1] - t.h / 2.0;
            t.rotation = rotation;
        }
        // A frame does not turn: its box is mapped and whatever
        // rotation the map carried is spent on nothing.
        Element::Frame(f) => {
            let mapped = box_frame(f.x, f.y, f.w, f.h, 0.0).transformed(m);
            let (lo, hi) = mapped.aabb();
            f.x = lo[0];
            f.y = lo[1];
            f.w = hi[0] - lo[0];
            f.h = hi[1] - lo[1];
        }
    }
}

/// A rotation (degrees) after `m`: what the map does to that orientation.
fn turned(rotation: f64, m: &Affine) -> f64 {
    let f = Frame {
        center: [0.0, 0.0],
        half: [1.0, 1.0],
        angle: rotation.to_radians(),
    };
    degrees(f.transformed(m).angle)
}

/// Radians → degrees, rounded to a nanodegree: degrees do not survive a
/// trip through radians otherwise, and a turn that cancels out must land
/// on exactly zero to stay off disk.
fn degrees(angle: f64) -> f64 {
    (angle.to_degrees() * 1e9).round() / 1e9 + 0.0
}

/// What the held modifiers make of a resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resize {
    /// Shift: one factor for both axes, so the box keeps its proportions.
    pub uniform: bool,
    /// Ctrl: the center stays put instead of the opposite corner.
    pub from_center: bool,
}

/// The map that drags `corner` of `f` to `pointer` along the frame's own
/// axes. What stays put is the opposite corner, or the center under
/// [`Resize::from_center`]; [`Resize::uniform`] hands both axes the wider
/// of the two factors, keeping their sign, so the proportions survive and
/// a flip still flips. An axis of zero extent is left alone, and stays out
/// of the shared factor: it has no ratio to keep.
pub fn resize_map(f: &Frame, corner: Corner, pointer: Point, opts: Resize) -> Affine {
    let signs = corner.signs();
    let fixed = if opts.from_center {
        [0.0, 0.0]
    } else {
        let pinned = corner.opposite().signs();
        [pinned[0] * f.half[0], pinned[1] * f.half[1]]
    };
    let local = f.to_local(pointer);
    // How far the dragged corner sits from what stays put: the whole side
    // from the opposite corner, half of it from the center.
    let factor = |axis: usize| {
        let span = signs[axis] * f.half[axis] - fixed[axis];
        (span.abs() >= 1e-9).then(|| (local[axis] - fixed[axis]) / span)
    };
    let mut factors = [factor(0), factor(1)];
    if opts.uniform {
        let wider = factors.iter().flatten().fold(0.0, |w: f64, s| w.max(s.abs()));
        for s in factors.iter_mut().flatten() {
            *s = wider * s.signum();
        }
    }
    let scale = Affine::scale(factors[0].unwrap_or(1.0), factors[1].unwrap_or(1.0));
    let to_local = Affine::translate(-f.center[0], -f.center[1]).then(Affine::rotate(-f.angle));
    let to_world = Affine::rotate(f.angle).then(Affine::translate(f.center[0], f.center[1]));
    to_local.then(scale.about(fixed)).then(to_world)
}

/// The angle (radians) the pointer swept about the center of `f` going
/// from `from` to `to`.
pub fn sweep(f: &Frame, from: Point, to: Point) -> f64 {
    let c = f.center;
    let before = (from[1] - c[1]).atan2(from[0] - c[0]);
    let after = (to[1] - c[1]).atan2(to[0] - c[0]);
    after - before
}

/// The turn by `delta` radians about the center of `f`.
pub fn rotate_map(f: &Frame, delta: f64) -> Affine {
    Affine::rotate(delta).about(f.center)
}

/// The part of a `delta` turn that lands `reference + delta` on a multiple
/// of `step` — snapping counts from the creation state, not from where the
/// drag began.
pub fn snap_turn(reference: f64, delta: f64, step: f64) -> f64 {
    ((reference + delta) / step).round() * step - reference
}

/// Every handle with its screen position: the four resize handles on the
/// corners, then the four rotation handles past them.
pub fn handles(f: &Frame, view: &View) -> [(Handle, Point); 8] {
    let offset = f64::from(ROTATE_OFFSET_PX) * view.scale / std::f64::consts::SQRT_2;
    let (s, c) = f.angle.sin_cos();
    let mut out = [(Handle::Resize(Corner::TopLeft), [0.0; 2]); 8];
    for (i, corner) in Corner::ALL.into_iter().enumerate() {
        let w = f.corner(corner);
        let (sx, sy) = view.world_to_screen(w[0], w[1]);
        out[i] = (Handle::Resize(corner), [sx, sy]);
        let signs = corner.signs();
        let (dx, dy) = (signs[0] * offset, signs[1] * offset);
        let outward = [c * dx - s * dy, s * dx + c * dy];
        out[4 + i] = (Handle::Rotate(corner), [sx + outward[0], sy + outward[1]]);
    }
    out
}

/// The handle under `screen` (physical px), if any. Resize handles win
/// where the two kinds overlap.
pub fn handle_at(f: &Frame, view: &View, screen: (f64, f64)) -> Option<Handle> {
    handles(f, view)
        .into_iter()
        .find(|(h, p)| {
            let size = match h {
                Handle::Resize(_) => HANDLE_PX,
                Handle::Rotate(_) => ROTATE_HANDLE_PX,
            };
            let reach = f64::from(size / 2.0 + HANDLE_SLOP_PX) * view.scale;
            (p[0] - screen.0).abs() <= reach && (p[1] - screen.1).abs() <= reach
        })
        .map(|(h, _)| h)
}

/// Selection overlay: the outline through the corners, a bordered square
/// on each corner, a ring past each corner.
pub fn prims(f: &Frame, view: &View, theme: &Theme) -> Vec<Prim> {
    let s = view.scale as f32;
    let corners = f.corners().map(|p| {
        let (x, y) = view.world_to_screen(p[0], p[1]);
        (x as f32, y as f32)
    });
    let mut out = Vec::with_capacity(20);
    for i in 0..4 {
        out.push(Prim::segment(
            corners[i],
            corners[(i + 1) % 4],
            OUTLINE_PX / 2.0 * s,
            theme.selection,
        ));
    }
    let angle = f.angle as f32;
    for (handle, p) in handles(f, view) {
        let (x, y) = (p[0] as f32, p[1] as f32);
        match handle {
            Handle::Resize(_) => {
                let side = HANDLE_PX * s;
                let r = ScreenRect {
                    x: x - side / 2.0,
                    y: y - side / 2.0,
                    w: side,
                    h: side,
                };
                out.push(Prim::turned(r, (x, y), angle, theme.selection));
                out.push(Prim::turned(
                    r.inset(HANDLE_BORDER_PX * s),
                    (x, y),
                    angle,
                    theme.handle,
                ));
            }
            Handle::Rotate(_) => {
                let radius = ROTATE_HANDLE_PX / 2.0 * s;
                out.push(Prim::circle(x, y, radius, theme.selection));
                out.push(Prim::circle(x, y, radius - RING_PX * s, theme.handle));
            }
        }
    }
    out
}

/// The marquee between two screen corners: translucent fill and a one
/// pixel outline.
pub fn marquee_prims(a: (f64, f64), b: (f64, f64), theme: &Theme) -> Vec<Prim> {
    let (x0, y0) = (a.0.min(b.0) as f32, a.1.min(b.1) as f32);
    let (x1, y1) = (a.0.max(b.0) as f32, a.1.max(b.1) as f32);
    let r = ScreenRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    };
    let mut out = vec![Prim::rect(r, with_alpha(theme.selection, MARQUEE_ALPHA))];
    let c = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    for i in 0..4 {
        out.push(Prim::segment(c[i], c[(i + 1) % 4], 0.5, theme.selection));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Cubic;
    use crate::doc::{Camera, Envelope, Image, Kind, Layer, Paint, Path, Rect, Stroke};
    use crate::scene::{KIND_BOX, KIND_SEGMENT, Viewport};
    use crate::theme::Theme;

    const QUARTER: f64 = std::f64::consts::FRAC_PI_2;

    /// Each axis follows the pointer on its own, the opposite corner pinned.
    const FREE: Resize = Resize {
        uniform: false,
        from_center: false,
    };
    const UNIFORM: Resize = Resize {
        uniform: true,
        from_center: false,
    };
    const FROM_CENTER: Resize = Resize {
        uniform: false,
        from_center: true,
    };
    const BOTH: Resize = Resize {
        uniform: true,
        from_center: true,
    };

    fn rect(id: &str, x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Element {
        Element::Rect(Rect {
            id: id.into(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation,
            stroke: Some("#000".into()),
            fill: None,
            text: None,
        })
    }

    fn path(id: &str, curves: Vec<Cubic>, width: f64) -> Element {
        Element::Path(Path {
            id: id.into(),
            layer: String::new(),
            curves,
            stroke: "#000".into(),
            width,
            opacity: 1.0,
            hardness: 1.0,
            rotation: 0.0,
            stamp: None,
            pen: Envelope::default(),
        })
    }

    fn paint(id: &str, strokes: Vec<(Vec<Cubic>, f64)>) -> Element {
        Element::Paint(Paint {
            id: id.into(),
            layer: String::new(),
            strokes: strokes
                .into_iter()
                .map(|(curves, width)| Stroke {
                    curves,
                    stroke: "#000".into(),
                    width,
                    opacity: 1.0,
                    hardness: 1.0,
                    stamp: None,
                    pen: Envelope::default(),
                })
                .collect(),
            rotation: 0.0,
        })
    }

    fn image(id: &str, x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Element {
        Element::Image(Image {
            id: id.into(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation,
            blob: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        })
    }

    fn path_rotation(el: &Element) -> f64 {
        let Element::Path(p) = el else { panic!() };
        p.rotation
    }

    #[test]
    fn a_paint_is_framed_and_hit_across_every_stroke() {
        let flat = |y: f64| -> Vec<Cubic> { vec![[[0.0, y], [3.0, y], [7.0, y], [10.0, y]]] };
        let el = paint("pt", vec![(flat(0.0), 2.0), (flat(20.0), 2.0)]);
        let f = frame(&el).unwrap();
        assert_eq!(f.center, [5.0, 10.0], "the box spans both strokes");
        assert_eq!(f.half, [6.0, 11.0], "each grown by half its width");
        assert!(hits(&el, [5.0, 20.0], 0.0), "on the second stroke");
        assert!(!hits(&el, [5.0, 10.0], 0.0), "the gap between them is not ink");
        // An empty paint has no frame, as a curveless path has none.
        assert!(frame(&paint("empty", vec![])).is_none());
    }

    #[test]
    fn transforming_a_paint_carries_every_stroke() {
        let flat = |y: f64| -> Vec<Cubic> { vec![[[0.0, y], [3.0, y], [7.0, y], [10.0, y]]] };
        let mut el = paint("pt", vec![(flat(0.0), 2.0), (flat(20.0), 2.0)]);
        transform(&mut el, &Affine::translate(5.0, -1.0));
        let Element::Paint(p) = &el else { panic!() };
        assert_eq!(p.strokes[0].curves[0][0], [5.0, -1.0]);
        assert_eq!(p.strokes[1].curves[0][3], [15.0, 19.0]);
        transform(&mut el, &Affine::rotate(std::f64::consts::FRAC_PI_2));
        let Element::Paint(p) = &el else { panic!() };
        assert_eq!(p.rotation, 90.0, "the object turns as one");
    }

    fn doc(elements: Vec<Element>) -> Document {
        let mut d = Document::new("t");
        d.elements = elements;
        let layer = d.layers[0].id.clone();
        for el in &mut d.elements {
            el.set_layer(&layer);
        }
        d
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

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_top_layer_wins_under_the_pointer() {
        let mut d = doc(vec![
            rect("upper", 0.0, 0.0, 10.0, 10.0, 0.0),
            rect("lower", 0.0, 0.0, 10.0, 10.0, 0.0),
        ]);
        d.layers.push(Layer {
            id: "top".into(),
            ..Layer::of("Layer 2", Kind::Raster)
        });
        d.elements[0].set_layer("top");
        // `lower` comes later in `elements`, but its layer is underneath.
        assert_eq!(element_at(&d, [5.0, 5.0], 0.0), Some("upper"));
        assert_eq!(
            elements_in(&d, [-1.0, -1.0], [11.0, 11.0]),
            ids(&["lower", "upper"]),
            "the marquee lists in paint order"
        );
    }

    #[test]
    fn a_hidden_layer_is_neither_hit_nor_marqueed() {
        let mut d = doc(vec![
            rect("upper", 0.0, 0.0, 10.0, 10.0, 0.0),
            rect("lower", 0.0, 0.0, 10.0, 10.0, 0.0),
        ]);
        d.layers.push(Layer {
            id: "top".into(),
            visible: false,
            ..Layer::of("Layer 2", Kind::Raster)
        });
        d.elements[0].set_layer("top");
        assert_eq!(element_at(&d, [5.0, 5.0], 0.0), Some("lower"));
        assert_eq!(elements_in(&d, [-1.0, -1.0], [11.0, 11.0]), ids(&["lower"]));
        d.layers[0].visible = false;
        assert_eq!(element_at(&d, [5.0, 5.0], 0.0), None);
        assert!(elements_in(&d, [-1.0, -1.0], [11.0, 11.0]).is_empty());
    }

    #[test]
    fn a_locked_layer_is_neither_hit_nor_marqueed_but_is_still_there() {
        let mut d = doc(vec![
            rect("upper", 0.0, 0.0, 10.0, 10.0, 0.0),
            rect("lower", 0.0, 0.0, 10.0, 10.0, 0.0),
        ]);
        d.layers.push(Layer {
            id: "top".into(),
            locked: true,
            ..Layer::of("Layer 2", Kind::Raster)
        });
        d.elements[0].set_layer("top");
        // The pointer goes through what it cannot take hold of, to what
        // it can: a locked layer is still painted, just not handled.
        assert_eq!(element_at(&d, [5.0, 5.0], 0.0), Some("lower"));
        assert_eq!(elements_in(&d, [-1.0, -1.0], [11.0, 11.0]), ids(&["lower"]));
        assert_eq!(d.painted().count(), 2, "and it still paints");
    }

    #[test]
    fn a_locked_group_locks_what_it_holds() {
        let mut d = doc(vec![rect("inside", 0.0, 0.0, 10.0, 10.0, 0.0)]);
        let layer = d.layers[0].id.clone();
        let inner = d.layers.remove(0);
        d.layers.push(Layer {
            id: "g".into(),
            locked: true,
            layers: vec![inner],
            ..Layer::of("Group 1", Kind::Group)
        });
        assert_eq!(d.elements[0].layer(), layer);
        assert_eq!(element_at(&d, [5.0, 5.0], 0.0), None);
        assert!(elements_in(&d, [-1.0, -1.0], [11.0, 11.0]).is_empty());
    }

    #[track_caller]
    fn assert_close(a: Point, b: Point) {
        assert!(
            (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn rect_frame_is_its_box_turned_about_its_center() {
        let f = frame(&rect("r", 10.0, 20.0, 40.0, 10.0, 90.0)).unwrap();
        assert_eq!(f.center, [30.0, 25.0]);
        assert_eq!(f.half, [20.0, 5.0]);
        assert!((f.angle - QUARTER).abs() < 1e-12);
    }

    #[test]
    fn path_frame_hugs_the_ink() {
        let straight = [[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]];
        let f = frame(&path("p", vec![straight], 2.0)).unwrap();
        // Curve bounds grown by half the width on every side.
        assert_eq!(f.center, [4.5, 0.0]);
        assert_eq!(f.half, [5.5, 1.0]);
        assert_eq!(f.angle, 0.0);
        assert_eq!(frame(&path("p", vec![], 2.0)), None);
    }

    #[test]
    fn path_frame_turns_with_its_rotation() {
        // The same straight stroke, drawn along x and then turned a quarter:
        // its ink now runs along y, but its box is the drawn one, turned.
        let along_y = [[0.0, 0.0], [0.0, 3.0], [0.0, 6.0], [0.0, 9.0]];
        let mut el = path("p", vec![along_y], 2.0);
        let Element::Path(p) = &mut el else { panic!() };
        p.rotation = 90.0;
        let f = frame(&el).unwrap();
        assert_close(f.center, [0.0, 4.5]);
        assert_close(f.half, [5.5, 1.0]);
        assert!((f.angle - QUARTER).abs() < 1e-12);
        assert_close(f.corner(Corner::TopLeft), [1.0, -1.0]);
        assert_close(f.corner(Corner::BottomRight), [-1.0, 10.0]);
    }

    #[test]
    fn transform_turns_a_path_rotation_with_the_map() {
        let mut p = path(
            "p",
            vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]],
            2.0,
        );
        transform(&mut p, &Affine::rotate(QUARTER).about([0.0, 0.0]));
        assert_eq!(path_rotation(&p), 90.0);
        let Element::Path(inner) = &p else { panic!() };
        assert_close(inner.curves[0][3], [0.0, 9.0]);
        // A move and a stretch along its own axes leave the turn alone.
        transform(&mut p, &Affine::translate(5.0, -1.0));
        assert_eq!(path_rotation(&p), 90.0);
        let f = frame(&p).unwrap();
        transform(
            &mut p,
            &resize_map(&f, Corner::BottomRight, f.corner(Corner::BottomRight), FREE),
        );
        assert_eq!(path_rotation(&p), 90.0);
        transform(&mut p, &Affine::rotate(-QUARTER).about([0.0, 0.0]));
        assert_eq!(path_rotation(&p), 0.0);
    }

    #[test]
    fn snap_turn_rounds_the_total_angle_to_the_step() {
        let step = 15f64.to_radians();
        let deg = |d: f64| d.to_radians();
        // From the creation state: 7° rounds down, 8° rounds up.
        assert!((snap_turn(0.0, deg(7.0), step) - 0.0).abs() < 1e-12);
        assert!((snap_turn(0.0, deg(8.0), step) - deg(15.0)).abs() < 1e-12);
        // Already at 10°: a 12° sweep lands on 15°, not on 25°.
        assert!((snap_turn(deg(10.0), deg(12.0), step) - deg(5.0)).abs() < 1e-12);
        assert!((snap_turn(deg(10.0), deg(-12.0), step) - deg(-10.0)).abs() < 1e-12);
    }

    #[test]
    fn frame_of_one_element_keeps_its_turn_and_of_several_wraps_them() {
        let d = doc(vec![
            rect("a", 0.0, 0.0, 10.0, 10.0, 45.0),
            rect("b", 100.0, 100.0, 10.0, 10.0, 0.0),
        ]);
        let one = frame_of(&d, &ids(&["a"])).unwrap();
        assert!((one.angle - QUARTER / 2.0).abs() < 1e-12);
        assert_eq!(one.half, [5.0, 5.0]);
        let both = frame_of(&d, &ids(&["a", "b"])).unwrap();
        assert_eq!(both.angle, 0.0);
        let r = 5.0 * std::f64::consts::SQRT_2;
        let (lo, hi) = both.aabb();
        assert_close(lo, [5.0 - r, 5.0 - r]);
        assert_close(hi, [110.0, 110.0]);
        assert_eq!(frame_of(&d, &ids(&["zzz"])), None);
        assert_eq!(frame_of(&d, &[]), None);
    }

    #[test]
    fn element_at_picks_the_topmost_within_slop() {
        let d = doc(vec![
            rect("a", 0.0, 0.0, 10.0, 10.0, 0.0),
            rect("b", 5.0, 5.0, 10.0, 10.0, 0.0),
        ]);
        assert_eq!(element_at(&d, [7.0, 7.0], 0.0), Some("b"));
        assert_eq!(element_at(&d, [2.0, 2.0], 0.0), Some("a"));
        assert_eq!(element_at(&d, [16.0, 7.0], 0.0), None);
        assert_eq!(element_at(&d, [15.5, 7.0], 1.0), Some("b"));
    }

    #[test]
    fn element_at_hits_a_path_by_its_ink_not_its_box() {
        let arch = [[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
        let d = doc(vec![path("p", vec![arch], 2.0)]);
        // The curve peaks at (5, 7.5); ink reaches 1 unit further.
        assert_eq!(element_at(&d, [5.0, 7.5], 0.0), Some("p"));
        assert_eq!(element_at(&d, [5.0, 8.4], 0.0), Some("p"));
        assert_eq!(
            element_at(&d, [5.0, 2.0], 0.0),
            None,
            "inside the box, off the ink"
        );
        assert_eq!(element_at(&d, [5.0, 9.0], 0.0), None);
        assert_eq!(element_at(&d, [5.0, 9.0], 1.0), Some("p"));
    }

    #[test]
    fn elements_in_lists_what_the_box_overlaps_in_document_order() {
        let d = doc(vec![
            rect("a", 0.0, 0.0, 10.0, 10.0, 0.0),
            path("p", vec![[[50.0, 50.0]; 4]], 2.0),
            rect("b", 20.0, 0.0, 10.0, 10.0, 0.0),
        ]);
        assert_eq!(elements_in(&d, [5.0, -1.0], [25.0, 1.0]), ids(&["a", "b"]));
        assert_eq!(
            elements_in(&d, [11.0, 0.0], [19.0, 1.0]),
            Vec::<String>::new()
        );
        assert_eq!(elements_in(&d, [10.0, 0.0], [12.0, 1.0]), ids(&["a"]));
        assert_eq!(
            elements_in(&d, [0.0, 0.0], [60.0, 60.0]),
            ids(&["a", "p", "b"])
        );
    }

    #[test]
    fn transform_moves_path_points_and_rect_frames() {
        let mut p = path(
            "p",
            vec![[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]],
            2.0,
        );
        transform(&mut p, &Affine::translate(1.0, 2.0));
        let Element::Path(p) = &p else { panic!() };
        assert_eq!(
            p.curves,
            vec![[[1.0, 2.0], [2.0, 2.0], [3.0, 2.0], [4.0, 2.0]]]
        );

        let mut r = rect("r", 0.0, 0.0, 20.0, 10.0, 0.0);
        transform(&mut r, &Affine::rotate(QUARTER).about([10.0, 5.0]));
        let Element::Rect(inner) = &r else { panic!() };
        assert_eq!((inner.x, inner.y, inner.w, inner.h), (0.0, 0.0, 20.0, 10.0));
        assert!((inner.rotation - 90.0).abs() < 1e-9, "{}", inner.rotation);
        // Turning back lands on exactly zero, so the field stays off disk.
        transform(&mut r, &Affine::rotate(-QUARTER).about([10.0, 5.0]));
        let Element::Rect(inner) = &r else { panic!() };
        assert_eq!(inner.rotation, 0.0);

        let mut r = rect("r", 1.0, 1.0, 2.0, 2.0, 0.0);
        transform(&mut r, &Affine::scale(2.0, 1.0));
        let Element::Rect(inner) = &r else { panic!() };
        assert_eq!((inner.x, inner.y, inner.w, inner.h), (2.0, 1.0, 4.0, 2.0));
    }

    #[test]
    fn moving_a_turned_rect_keeps_its_rotation_exact() {
        // Degrees → radians → degrees does not round-trip in floats; the
        // document must not pick up 29.999999999999996 from a plain move.
        let mut r = rect("r", 0.0, 0.0, 20.0, 10.0, 30.0);
        transform(&mut r, &Affine::translate(5.0, -1.0));
        let Element::Rect(inner) = &r else { panic!() };
        assert_eq!((inner.x, inner.y), (5.0, -1.0));
        assert_eq!(inner.rotation, 30.0);
        // A quarter turn on top lands on a round number too.
        transform(&mut r, &Affine::rotate(QUARTER).about([15.0, 4.0]));
        let Element::Rect(inner) = &r else { panic!() };
        assert_eq!(inner.rotation, 120.0);
    }

    #[test]
    fn resize_map_pins_the_opposite_corner_and_follows_the_pointer() {
        let f = Frame {
            center: [5.0, 5.0],
            half: [5.0, 5.0],
            angle: 0.0,
        };
        let m = resize_map(&f, Corner::BottomRight, [20.0, 15.0], FREE);
        assert_close(m.apply([0.0, 0.0]), [0.0, 0.0]);
        assert_close(m.apply([10.0, 10.0]), [20.0, 15.0]);
        assert_close(m.apply([5.0, 5.0]), [10.0, 7.5]);

        // Turned frame: still along its own axes.
        let f = Frame {
            center: [0.0, 0.0],
            half: [10.0, 5.0],
            angle: QUARTER,
        };
        let target = [3.0, 25.0];
        let m = resize_map(&f, Corner::TopRight, target, FREE);
        assert_close(
            m.apply(f.corner(Corner::BottomLeft)),
            f.corner(Corner::BottomLeft),
        );
        assert_close(m.apply(f.corner(Corner::TopRight)), target);

        // A flat frame cannot stretch across its zero axis.
        let f = Frame {
            center: [5.0, 0.0],
            half: [5.0, 0.0],
            angle: 0.0,
        };
        let m = resize_map(&f, Corner::BottomRight, [12.0, 3.0], FREE);
        assert_close(m.apply([10.0, 0.0]), [12.0, 0.0]);
    }

    #[test]
    fn shift_scales_both_axes_by_one_factor() {
        // 20 x 10: whatever the pointer asks for, the box stays 2:1.
        let f = Frame {
            center: [10.0, 5.0],
            half: [10.0, 5.0],
            angle: 0.0,
        };
        // 1.3x across, 3x down: the larger one takes both axes.
        let m = resize_map(&f, Corner::BottomRight, [26.0, 30.0], UNIFORM);
        assert_close(m.apply([0.0, 0.0]), [0.0, 0.0]);
        assert_close(m.apply([20.0, 10.0]), [60.0, 30.0]);

        // Past the pinned corner the box mirrors, still 2:1.
        let m = resize_map(&f, Corner::BottomRight, [-16.0, 2.0], UNIFORM);
        assert_close(m.apply([0.0, 0.0]), [0.0, 0.0]);
        assert_close(m.apply([20.0, 10.0]), [-16.0, 8.0]);

        // A flat frame has no ratio to keep: the live axis decides alone,
        // and the axis that cannot stretch does not hold it back.
        let f = Frame {
            center: [5.0, 0.0],
            half: [5.0, 0.0],
            angle: 0.0,
        };
        let m = resize_map(&f, Corner::BottomRight, [8.0, 3.0], UNIFORM);
        assert_close(m.apply([10.0, 0.0]), [8.0, 0.0]);
    }

    #[test]
    fn ctrl_scales_about_the_center() {
        let f = Frame {
            center: [5.0, 5.0],
            half: [5.0, 5.0],
            angle: 0.0,
        };
        let m = resize_map(&f, Corner::BottomRight, [15.0, 7.5], FROM_CENTER);
        assert_close(m.apply([5.0, 5.0]), [5.0, 5.0]);
        assert_close(m.apply([10.0, 10.0]), [15.0, 7.5]);
        // The corner that used to be pinned now moves the other way.
        assert_close(m.apply([0.0, 0.0]), [-5.0, 2.5]);

        // Turned frame: the center holds along its own axes too.
        let f = Frame {
            center: [0.0, 0.0],
            half: [10.0, 5.0],
            angle: QUARTER,
        };
        let m = resize_map(&f, Corner::TopRight, [10.0, 20.0], FROM_CENTER);
        assert_close(m.apply(f.center), f.center);
        assert_close(m.apply(f.corner(Corner::TopRight)), [10.0, 20.0]);
        assert_close(m.apply(f.corner(Corner::BottomLeft)), [-10.0, -20.0]);
    }

    #[test]
    fn shift_and_ctrl_scale_uniformly_about_the_center() {
        let f = Frame {
            center: [5.0, 5.0],
            half: [5.0, 5.0],
            angle: 0.0,
        };
        // 2x across, 0.5x down about the center; uniform takes the 2x.
        let m = resize_map(&f, Corner::BottomRight, [15.0, 7.5], BOTH);
        assert_close(m.apply([5.0, 5.0]), [5.0, 5.0]);
        assert_close(m.apply([10.0, 10.0]), [15.0, 15.0]);
        assert_close(m.apply([0.0, 0.0]), [-5.0, -5.0]);
    }

    #[test]
    fn rotate_map_turns_about_the_center_by_the_pointer_sweep() {
        let f = Frame {
            center: [10.0, 10.0],
            half: [3.0, 3.0],
            angle: 0.0,
        };
        let delta = sweep(&f, [20.0, 10.0], [10.0, 20.0]);
        assert!((delta - QUARTER).abs() < 1e-12);
        let m = rotate_map(&f, delta);
        assert_close(m.apply([20.0, 10.0]), [10.0, 20.0]);
        assert_close(m.apply([10.0, 10.0]), [10.0, 10.0]);
        assert_close(m.apply([10.0, 0.0]), [20.0, 10.0]);
    }

    #[test]
    fn handles_sit_on_the_corners_and_outside_them() {
        let f = Frame {
            center: [50.0, 50.0],
            half: [20.0, 10.0],
            angle: 0.0,
        };
        let at = |v: &View, h: Handle| {
            handles(&f, v)
                .into_iter()
                .find(|(hh, _)| *hh == h)
                .map(|(_, p)| p)
                .unwrap()
        };
        let v = view();
        assert_close(at(&v, Handle::Resize(Corner::TopLeft)), [30.0, 40.0]);
        assert_close(at(&v, Handle::Resize(Corner::BottomRight)), [70.0, 60.0]);
        let d = f64::from(ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        assert_close(
            at(&v, Handle::Rotate(Corner::TopLeft)),
            [30.0 - d, 40.0 - d],
        );
        assert_close(
            at(&v, Handle::Rotate(Corner::BottomLeft)),
            [30.0 - d, 60.0 + d],
        );
        // The offset is in logical px: twice as far on a 2x display.
        let hidpi = View { scale: 2.0, ..v };
        let corner = at(&hidpi, Handle::Resize(Corner::TopLeft));
        let rot = at(&hidpi, Handle::Rotate(Corner::TopLeft));
        assert_close(rot, [corner[0] - 2.0 * d, corner[1] - 2.0 * d]);
    }

    #[test]
    fn handle_at_prefers_resize_over_rotate_and_ignores_the_rest() {
        let f = Frame {
            center: [50.0, 50.0],
            half: [20.0, 10.0],
            angle: 0.0,
        };
        let v = view();
        assert_eq!(
            handle_at(&f, &v, (30.0, 40.0)),
            Some(Handle::Resize(Corner::TopLeft))
        );
        assert_eq!(
            handle_at(&f, &v, (32.0, 43.0)),
            Some(Handle::Resize(Corner::TopLeft))
        );
        assert_eq!(
            handle_at(&f, &v, (70.0, 60.0)),
            Some(Handle::Resize(Corner::BottomRight))
        );
        let d = f64::from(ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2;
        assert_eq!(
            handle_at(&f, &v, (30.0 - d, 40.0 - d)),
            Some(Handle::Rotate(Corner::TopLeft))
        );
        assert_eq!(handle_at(&f, &v, (50.0, 50.0)), None);
        assert_eq!(
            handle_at(&f, &v, (30.0, 50.0)),
            None,
            "edges have no handle"
        );
    }

    #[test]
    fn prims_outline_the_frame_and_mark_every_handle() {
        let f = Frame {
            center: [50.0, 50.0],
            half: [20.0, 10.0],
            angle: 0.0,
        };
        let theme = Theme::light();
        let got = prims(&f, &view(), &theme);
        let outline: Vec<&Prim> = got.iter().filter(|p| p.kind == KIND_SEGMENT).collect();
        assert_eq!(outline.len(), 4);
        for p in &outline {
            assert_eq!(p.color, theme.selection);
        }
        assert_eq!(outline[0].geom, [30.0, 40.0, 70.0, 40.0], "top edge first");
        // Each resize handle: a selection-colored box under a handle-colored
        // one, both centered on the corner.
        let boxes: Vec<&Prim> = got.iter().filter(|p| p.kind == KIND_BOX).collect();
        let centered_on = |x: f32, y: f32| {
            boxes
                .iter()
                .filter(|p| {
                    (p.geom[0] + p.geom[2] / 2.0 - x).abs() < 1e-3
                        && (p.geom[1] + p.geom[3] / 2.0 - y).abs() < 1e-3
                })
                .count()
        };
        assert_eq!(centered_on(30.0, 40.0), 2);
        assert_eq!(centered_on(70.0, 60.0), 2);
        let d = (f64::from(ROTATE_OFFSET_PX) / std::f64::consts::SQRT_2) as f32;
        assert_eq!(
            centered_on(30.0 - d, 40.0 - d),
            2,
            "rotation handle is a ring"
        );
        assert_eq!(boxes.len(), 16);
    }

    #[test]
    fn marquee_prims_fill_and_outline_the_box() {
        let theme = Theme::light();
        let got = marquee_prims((10.0, 20.0), (40.0, 30.0), &theme);
        assert_eq!(got[0].kind, KIND_BOX);
        assert_eq!(got[0].geom, [10.0, 20.0, 30.0, 10.0]);
        assert!(got[0].color[3] < 0.5, "translucent fill");
        assert_eq!(got.len(), 5, "fill plus four edges");
        // Any two corners define it.
        assert_eq!(
            marquee_prims((40.0, 30.0), (10.0, 20.0), &theme)[0].geom,
            got[0].geom
        );
    }

    #[test]
    fn image_frame_is_its_box_turned_about_its_center() {
        let f = frame(&image("i", 10.0, 20.0, 60.0, 40.0, 90.0)).unwrap();
        assert_eq!(f.center, [40.0, 40.0]);
        assert_eq!(f.half, [30.0, 20.0]);
        assert!((f.angle - QUARTER).abs() < 1e-12, "{}", f.angle);
    }

    #[test]
    fn image_is_hit_anywhere_inside_its_box() {
        // A bitmap is opaque to the pointer the way a rect is: the pixels
        // do not decide, the box does.
        let d = doc(vec![image("i", 0.0, 0.0, 40.0, 20.0, 0.0)]);
        assert_eq!(element_at(&d, [20.0, 10.0], 0.0), Some("i"));
        assert_eq!(element_at(&d, [39.0, 1.0], 0.0), Some("i"));
        assert_eq!(element_at(&d, [45.0, 10.0], 0.0), None);
        assert_eq!(element_at(&d, [45.0, 10.0], 6.0), Some("i"));
    }

    #[test]
    fn transforming_an_image_maps_its_box_and_turn() {
        let mut el = image("i", 0.0, 0.0, 40.0, 20.0, 0.0);
        transform(&mut el, &Affine::rotate(QUARTER));
        let Element::Image(i) = &el else { panic!() };
        // The box keeps its size; the turn is recorded in degrees.
        assert!((i.w - 40.0).abs() < 1e-9, "{}", i.w);
        assert!((i.h - 20.0).abs() < 1e-9, "{}", i.h);
        assert_eq!(i.rotation, 90.0);
        let f = frame(&el).unwrap();
        assert!((f.center[0] - -10.0).abs() < 1e-9, "{:?}", f.center);
        assert!((f.center[1] - 20.0).abs() < 1e-9, "{:?}", f.center);
    }

    #[test]
    fn resizing_an_image_stretches_its_box() {
        let mut el = image("i", 0.0, 0.0, 40.0, 20.0, 0.0);
        let f = frame(&el).unwrap();
        transform(&mut el, &resize_map(&f, Corner::BottomRight, [80.0, 20.0], FREE));
        let Element::Image(i) = &el else { panic!() };
        assert_eq!((i.x, i.y, i.w, i.h), (0.0, 0.0, 80.0, 20.0));
    }

    /// A frame at (0,0)–(100,100) holding a rect that runs well past its
    /// right edge.
    fn framed_doc() -> Document {
        Document::from_json(
            r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [
                { "id": "fr", "type": "frame", "layer": "fl",
                  "x": 0, "y": 0, "w": 100, "h": 100, "background": "#fff",
                  "layers": [ { "id": "in", "name": "Layer 1" } ] },
                { "id": "inside", "type": "rect", "layer": "in",
                  "x": 50, "y": 50, "w": 400, "h": 20,
                  "stroke": null, "fill": "#000", "text": null }
            ]
        }"##,
        )
        .unwrap()
    }

    #[test]
    fn a_point_on_a_frames_ground_picks_the_frame() {
        let doc = framed_doc();
        assert_eq!(element_at(&doc, [10.0, 10.0], 0.0), Some("fr"));
    }

    #[test]
    fn a_point_on_what_a_frame_holds_picks_the_content() {
        let doc = framed_doc();
        assert_eq!(element_at(&doc, [60.0, 55.0], 0.0), Some("inside"));
    }

    #[test]
    fn ink_the_boundary_cut_away_is_not_there_for_the_pointer_either() {
        let doc = framed_doc();
        assert_eq!(
            element_at(&doc, [300.0, 55.0], 0.0),
            None,
            "the rect reaches here, but the frame does not"
        );
    }

    #[test]
    fn a_marquee_misses_what_the_boundary_cut_away() {
        let doc = framed_doc();
        let picked = elements_in(&doc, [200.0, 40.0], [400.0, 80.0]);
        assert!(picked.is_empty(), "{picked:?}");
    }

    #[test]
    fn a_marquee_over_a_frame_takes_it_and_what_it_holds() {
        let doc = framed_doc();
        let picked = elements_in(&doc, [-10.0, -10.0], [110.0, 110.0]);
        assert_eq!(picked, ["fr", "inside"]);
    }

    #[test]
    fn a_frame_occupies_its_own_area() {
        let doc = framed_doc();
        let f = frame(&doc.elements[0]).expect("a frame has a box");
        assert_eq!(f.center, [50.0, 50.0]);
        assert_eq!(f.half, [50.0, 50.0]);
        assert_eq!(f.angle, 0.0);
    }

    #[test]
    fn a_frame_does_not_turn() {
        let mut doc = framed_doc();
        let before = frame(&doc.elements[0]).unwrap();
        transform(&mut doc.elements[0], &Affine::rotate(QUARTER));
        let after = frame(&doc.elements[0]).unwrap();
        assert_eq!(after.angle, 0.0, "the turn is dropped");
        assert!(
            (after.half[0] - before.half[0]).abs() < 1e-9
                && (after.half[1] - before.half[1]).abs() < 1e-9,
            "and the box keeps its extents: {:?} was {:?}",
            after.half,
            before.half
        );
    }

    #[test]
    fn a_frame_moves_and_resizes_like_any_box() {
        let mut doc = framed_doc();
        transform(&mut doc.elements[0], &Affine::translate(10.0, 20.0));
        let f = frame(&doc.elements[0]).unwrap();
        assert_eq!(f.center, [60.0, 70.0]);
        let Element::Frame(fr) = &doc.elements[0] else {
            panic!("not a frame");
        };
        assert_eq!((fr.x, fr.y, fr.w, fr.h), (10.0, 20.0, 100.0, 100.0));
    }

    fn text(mode: crate::doc::TextMode) -> Element {
        Element::Text(crate::doc::Text {
            id: "t".into(),
            layer: String::new(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 40.0,
            rotation: 0.0,
            mode,
            text: "words".into(),
            style: crate::doc::TextStyle::with_size(20.0, "#000"),
            runs: Vec::new(),
        })
    }

    fn text_of(el: &Element) -> &crate::doc::Text {
        let Element::Text(t) = el else { panic!() };
        t
    }

    #[test]
    fn a_text_is_its_box_and_is_hit_anywhere_in_it() {
        let el = text(crate::doc::TextMode::Artistic);
        let f = frame(&el).unwrap();
        assert_eq!(f.center, [50.0, 20.0]);
        assert_eq!(f.half, [50.0, 20.0]);
        assert!(hits(&el, [90.0, 35.0], 0.0), "between the letters too");
        assert!(!hits(&el, [120.0, 35.0], 0.0));
    }

    #[test]
    fn resizing_artistic_text_scales_its_letters() {
        let mut el = text(crate::doc::TextMode::Artistic);
        let f = frame(&el).unwrap();
        let m = resize_map(&f, Corner::BottomRight, [200.0, 80.0], UNIFORM);
        transform(&mut el, &m);
        let t = text_of(&el);
        assert!((t.style.size - 40.0).abs() < 1e-9, "{}", t.style.size);
        assert_eq!((t.x, t.y), (0.0, 0.0));
        assert!((t.w - 200.0).abs() < 1e-9 && (t.h - 80.0).abs() < 1e-9);
    }

    #[test]
    fn a_stretch_of_artistic_text_scales_it_evenly_about_where_it_went() {
        let mut el = text(crate::doc::TextMode::Artistic);
        let f = frame(&el).unwrap();
        // Twice as wide, as tall as it was: the letters grow by the
        // middle of the two, and the box stays centred where the map put
        // it rather than stretching.
        let m = resize_map(&f, Corner::BottomRight, [200.0, 40.0], FREE);
        transform(&mut el, &m);
        let t = text_of(&el);
        let k = 2.0f64.sqrt();
        assert!((t.style.size - 20.0 * k).abs() < 1e-9);
        assert!((t.w - 100.0 * k).abs() < 1e-9 && (t.h - 40.0 * k).abs() < 1e-9);
        assert!((t.x + t.w / 2.0 - 100.0).abs() < 1e-9);
        assert!((t.y + t.h / 2.0 - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_text_is_never_scaled_past_what_a_text_may_be() {
        let mut el = text(crate::doc::TextMode::Artistic);
        let f = frame(&el).unwrap();
        let m = resize_map(&f, Corner::BottomRight, [0.001, 0.0004], UNIFORM);
        transform(&mut el, &m);
        let t = text_of(&el);
        assert_eq!(t.style.size, crate::doc::MIN_TEXT_SIZE);
        assert!(t.style.checked().is_ok());
    }

    #[test]
    fn resizing_a_text_frame_reflows_it_and_leaves_the_letters_alone() {
        let mut el = text(crate::doc::TextMode::Frame);
        let f = frame(&el).unwrap();
        let m = resize_map(&f, Corner::BottomRight, [300.0, 60.0], FREE);
        transform(&mut el, &m);
        let t = text_of(&el);
        assert_eq!(t.style.size, 20.0);
        assert_eq!((t.x, t.y, t.w, t.h), (0.0, 0.0, 300.0, 60.0));
    }

    #[test]
    fn a_text_turns_like_a_box() {
        for mode in [crate::doc::TextMode::Artistic, crate::doc::TextMode::Frame] {
            let mut el = text(mode);
            let f = frame(&el).unwrap();
            transform(&mut el, &rotate_map(&f, QUARTER));
            let t = text_of(&el);
            assert!((t.rotation - 90.0).abs() < 1e-9);
            assert_eq!(t.style.size, 20.0, "a turn scales nothing");
            assert!((t.w - 100.0).abs() < 1e-9);
        }
    }

    #[test]
    fn scaling_artistic_text_scales_the_stretches_set_apart_in_it() {
        let mut el = text(crate::doc::TextMode::Artistic);
        if let Element::Text(t) = &mut el {
            t.runs = vec![crate::doc::Run {
                start: 0,
                end: 2,
                style: crate::doc::RunStyle {
                    size: Some(30.0),
                    ..Default::default()
                },
            }];
        }
        let f = frame(&el).unwrap();
        let m = resize_map(&f, Corner::BottomRight, [200.0, 80.0], UNIFORM);
        transform(&mut el, &m);
        let t = text_of(&el);
        assert!((t.style.size - 40.0).abs() < 1e-9);
        assert_eq!(t.runs[0].style.size, Some(60.0), "twice the size, as the rest");
    }

    /// A `model` over (0,0)–(100,60), stroked 4 wide, filled or not.
    fn shape(model: crate::doc::Model, filled: bool) -> Element {
        Element::Shape(crate::doc::Shape {
            id: "s".into(),
            layer: String::new(),
            model,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 60.0,
            rotation: 0.0,
            flip: false,
            fill: filled.then(|| "#e5484d".into()),
            stroke: Some("#000000".into()),
            width: 4.0,
            radius: 0.0,
            sides: 5,
            inner: crate::doc::DEFAULT_INNER,
        })
    }

    fn shape_of(el: &Element) -> &crate::doc::Shape {
        match el {
            Element::Shape(s) => s,
            other => panic!("expected a shape, got {other:?}"),
        }
    }

    use crate::doc::Model;

    #[test]
    fn a_shape_is_its_box_turned_about_its_centre() {
        let mut el = shape(Model::Star, true);
        if let Element::Shape(s) = &mut el {
            s.rotation = 90.0;
        }
        let f = frame(&el).unwrap();
        assert_eq!((f.center, f.half), ([50.0, 30.0], [50.0, 30.0]));
        assert!((f.angle - QUARTER).abs() < 1e-12);
    }

    #[test]
    fn a_filled_shape_is_hit_on_its_figure_and_not_beside_it() {
        let d = doc(vec![shape(Model::Ellipse, true)]);
        assert_eq!(element_at(&d, [50.0, 30.0], 0.0), Some("s"));
        assert_eq!(element_at(&d, [3.0, 3.0], 1.0), None, "the box's corner is past the ellipse");
        assert_eq!(element_at(&d, [101.0, 30.0], 2.0), Some("s"), "within slop of the edge");
        assert_eq!(element_at(&d, [105.0, 30.0], 2.0), None);
    }

    #[test]
    fn a_hollow_shape_is_hit_on_its_stroke_alone() {
        let d = doc(vec![shape(Model::Rectangle, false)]);
        assert_eq!(element_at(&d, [50.0, 30.0], 2.0), None, "the middle of it is empty");
        assert_eq!(element_at(&d, [2.0, 30.0], 0.0), Some("s"), "on the stroke, inside the edge");
        assert_eq!(element_at(&d, [-1.0, 30.0], 2.0), Some("s"), "within slop outside it");
        assert_eq!(element_at(&d, [5.5, 30.0], 2.0), Some("s"), "within slop inside the stroke");
        assert_eq!(element_at(&d, [9.0, 30.0], 2.0), None);
    }

    #[test]
    fn a_turned_shape_is_hit_where_it_was_turned_to() {
        // A triangle in a 100 by 60 box, a quarter turn: its apex points
        // right, out of the box's centre, and its base stands on the left.
        let mut el = shape(Model::Triangle, true);
        let f = frame(&el).unwrap();
        transform(&mut el, &rotate_map(&f, QUARTER));
        let d = doc(vec![el]);
        assert_eq!(element_at(&d, [78.0, 30.0], 0.0), Some("s"), "the apex");
        assert_eq!(element_at(&d, [78.0, 5.0], 0.0), None, "beside it");
        assert_eq!(element_at(&d, [22.0, 76.0], 0.0), Some("s"), "the base's end");
    }

    #[test]
    fn a_shape_moves_turns_and_stretches_as_its_box_does() {
        let mut el = shape(Model::Diamond, true);
        let f = frame(&el).unwrap();
        transform(&mut el, &resize_map(&f, Corner::BottomRight, [200.0, 90.0], FREE));
        let s = shape_of(&el);
        assert_eq!((s.x, s.y, s.w, s.h), (0.0, 0.0, 200.0, 90.0));
        transform(&mut el, &Affine::translate(10.0, -5.0));
        let s = shape_of(&el);
        assert_eq!((s.x, s.y, s.width), (10.0, -5.0, 4.0), "the stroke keeps its width");
    }

    #[test]
    fn a_flip_mirrors_a_shape_rather_than_turning_it() {
        // Near the box's top left a triangle is empty and near its bottom
        // left it is not — until it is flipped top to bottom. Flipped side
        // to side it stands as it stood.
        let (top, bottom) = ([12.0, 8.0], [12.0, 56.0]);
        let under = |el: &Element, p: Point| element_at(&doc(vec![el.clone()]), p, 0.0).is_some();
        let upright = shape(Model::Triangle, true);
        assert!(!under(&upright, top) && under(&upright, bottom));

        let f = frame(&upright).unwrap();
        let mut flipped = upright.clone();
        transform(&mut flipped, &Affine::scale(1.0, -1.0).about(f.center));
        assert!(shape_of(&flipped).flip);
        assert!(under(&flipped, top) && !under(&flipped, bottom), "top to bottom");

        let mut mirrored = upright.clone();
        transform(&mut mirrored, &Affine::scale(-1.0, 1.0).about(f.center));
        assert!(!under(&mirrored, top) && under(&mirrored, bottom), "side to side");

        // Twice over is where it started.
        transform(&mut flipped, &Affine::scale(1.0, -1.0).about(f.center));
        assert!(!shape_of(&flipped).flip);
    }

    fn line(from: Point, to: Point, width: f64) -> Element {
        Element::Line(crate::doc::Line {
            id: "l".into(),
            layer: String::new(),
            from,
            to,
            stroke: "#000000".into(),
            width,
            start: crate::doc::Head::None,
            end: crate::doc::Head::None,
        })
    }

    fn line_of(el: &Element) -> &crate::doc::Line {
        match el {
            Element::Line(l) => l,
            other => panic!("expected a line, got {other:?}"),
        }
    }

    #[test]
    fn a_line_is_framed_along_itself_and_past_its_ends_by_its_caps() {
        let f = frame(&line([0.0, 0.0], [30.0, 40.0], 4.0)).unwrap();
        assert_close(f.center, [15.0, 20.0]);
        assert!((f.half[0] - 27.0).abs() < 1e-9, "half of 50, and a cap of 2: {:?}", f.half);
        assert!((f.half[1] - 2.0).abs() < 1e-9, "{:?}", f.half);
        assert!((f.angle - 40f64.atan2(30.0)).abs() < 1e-12);
    }

    #[test]
    fn a_line_is_hit_on_its_ink_alone() {
        let d = doc(vec![line([0.0, 0.0], [100.0, 100.0], 6.0)]);
        assert_eq!(element_at(&d, [50.0, 52.0], 0.0), Some("l"), "within its width");
        // 4.2 off the line, 3 of ink and 2 of slop.
        assert_eq!(element_at(&d, [50.0, 56.0], 2.0), Some("l"), "and the slop past it");
        assert_eq!(element_at(&d, [50.0, 62.0], 2.0), None);
        assert_eq!(element_at(&d, [-3.0, -1.0], 1.0), Some("l"), "round its end");
    }

    #[test]
    fn a_line_moves_turns_and_flips_by_its_two_ends() {
        let mut el = line([10.0, 0.0], [30.0, 0.0], 2.0);
        transform(&mut el, &Affine::rotate(QUARTER));
        let l = line_of(&el);
        assert_close(l.from, [0.0, 10.0]);
        assert_close(l.to, [0.0, 30.0]);
        transform(&mut el, &Affine::scale(-2.0, 1.0));
        let l = line_of(&el);
        assert_close(l.from, [0.0, 10.0]);
        assert_eq!(l.width, 2.0, "a stretch leaves the ink as wide as it was");
        let mut el = line([0.0, 0.0], [10.0, 10.0], 2.0);
        transform(&mut el, &Affine::scale(3.0, 1.0));
        assert_close(line_of(&el).to, [30.0, 10.0]);
    }

    #[test]
    fn a_line_is_framed_and_hit_as_wide_as_its_heads() {
        let mut el = line([0.0, 0.0], [100.0, 0.0], 2.0);
        if let Element::Line(l) = &mut el {
            l.end = crate::doc::Head::Triangle;
        }
        let f = frame(&el).unwrap();
        let reach = crate::shape::line_reach(line_of(&el));
        assert!(reach > 1.0);
        assert!((f.half[1] - reach).abs() < 1e-9, "{:?}", f.half);
        let d = doc(vec![el]);
        assert_eq!(element_at(&d, [95.0, 2.5], 0.0), Some("l"), "on its head, off its shaft");
        assert_eq!(element_at(&d, [40.0, 2.5], 0.0), None, "the shaft is as wide as it is");
    }
}
