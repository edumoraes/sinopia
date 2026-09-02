//! Selection (§6.2 session state): what an element occupies, what is under
//! the pointer, the handles around a selection and the transforms those
//! handles drive. Pure — `editor` decides when, this decides what.

use crate::curve;
use crate::doc::{Document, Element, Rect};
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
/// and turn; a path's ink (curve bounds grown by half the width), unturned.
/// `None` for a path with no curves.
pub fn frame(el: &Element) -> Option<Frame> {
    match el {
        Element::Rect(r) => Some(rect_frame(r)),
        Element::Path(p) => {
            let (lo, hi) = curve::bounds(&p.curves)?;
            let pad = p.width / 2.0;
            Some(Frame::spanning(
                [lo[0] - pad, lo[1] - pad],
                [hi[0] + pad, hi[1] + pad],
            ))
        }
    }
}

fn rect_frame(r: &Rect) -> Frame {
    Frame {
        center: [r.x + r.w / 2.0, r.y + r.h / 2.0],
        half: [r.w / 2.0, r.h / 2.0],
        angle: r.rotation.to_radians(),
    }
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
/// edges. Rects hit anywhere inside; paths only on their ink.
pub fn element_at(doc: &Document, p: Point, slop: f64) -> Option<&str> {
    doc.elements
        .iter()
        .rev()
        .find(|el| hits(el, p, slop))
        .map(Element::id)
}

fn hits(el: &Element, p: Point, slop: f64) -> bool {
    let Some(f) = frame(el) else {
        return false;
    };
    if !f.contains(p, slop) {
        return false;
    }
    match el {
        Element::Rect(_) => true,
        Element::Path(path) => {
            let reach = path.width / 2.0 + slop;
            // Flatten far finer than the reach so the polyline's error
            // cannot decide the answer.
            let tolerance = (reach / 20.0).max(1e-6);
            path.curves.iter().any(|c| {
                curve::flatten(c, tolerance)
                    .windows(2)
                    .any(|w| curve::point_segment_distance(p, w[0], w[1]) <= reach)
            })
        }
    }
}

/// Ids of the elements whose box overlaps the unturned box with `a` and
/// `b` as opposite corners, in document order.
pub fn elements_in(doc: &Document, a: Point, b: Point) -> Vec<String> {
    let lo = [a[0].min(b[0]), a[1].min(b[1])];
    let hi = [a[0].max(b[0]), a[1].max(b[1])];
    doc.elements
        .iter()
        .filter(|el| {
            frame(el).is_some_and(|f| {
                let (flo, fhi) = f.aabb();
                flo[0] <= hi[0] && fhi[0] >= lo[0] && flo[1] <= hi[1] && fhi[1] >= lo[1]
            })
        })
        .map(|el| el.id().to_owned())
        .collect()
}

/// Applies `m` to an element. Path control points map exactly; a rect maps
/// its frame (see [`Frame::transformed`]) and lands back on zero rotation
/// when the turn cancels out, so the field stays off disk.
pub fn transform(el: &mut Element, m: &Affine) {
    match el {
        Element::Path(p) => {
            for c in &mut p.curves {
                for point in c {
                    *point = m.apply(*point);
                }
            }
        }
        Element::Rect(r) => {
            let f = rect_frame(r).transformed(m);
            r.x = f.center[0] - f.half[0];
            r.y = f.center[1] - f.half[1];
            r.w = 2.0 * f.half[0];
            r.h = 2.0 * f.half[1];
            let degrees = f.angle.to_degrees();
            r.rotation = if degrees.abs() < 1e-9 { 0.0 } else { degrees };
        }
    }
}

/// The map that drags `corner` of `f` to `pointer` along the frame's own
/// axes, the opposite corner pinned. An axis of zero extent is left alone.
pub fn resize_map(f: &Frame, corner: Corner, pointer: Point) -> Affine {
    let signs = corner.signs();
    let fixed = [-signs[0] * f.half[0], -signs[1] * f.half[1]];
    let local = f.to_local(pointer);
    let factor = |axis: usize| {
        let span = 2.0 * signs[axis] * f.half[axis];
        if span.abs() < 1e-9 {
            1.0
        } else {
            (local[axis] - fixed[axis]) / span
        }
    };
    let to_local = Affine::translate(-f.center[0], -f.center[1]).then(Affine::rotate(-f.angle));
    let to_world = Affine::rotate(f.angle).then(Affine::translate(f.center[0], f.center[1]));
    to_local
        .then(Affine::scale(factor(0), factor(1)).about(fixed))
        .then(to_world)
}

/// The turn about the center of `f` that sweeps the pointer from `from` to
/// `to`.
pub fn rotate_map(f: &Frame, from: Point, to: Point) -> Affine {
    let c = f.center;
    let before = (from[1] - c[1]).atan2(from[0] - c[0]);
    let after = (to[1] - c[1]).atan2(to[0] - c[0]);
    Affine::rotate(after - before).about(c)
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
    use crate::doc::{Camera, Path, Rect};
    use crate::scene::{KIND_BOX, KIND_SEGMENT, Viewport};
    use crate::theme::Theme;

    const QUARTER: f64 = std::f64::consts::FRAC_PI_2;

    fn rect(id: &str, x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Element {
        Element::Rect(Rect {
            id: id.into(),
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
            curves,
            stroke: "#000".into(),
            width,
        })
    }

    fn doc(elements: Vec<Element>) -> Document {
        let mut d = Document::new("t");
        d.elements = elements;
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
    fn resize_map_pins_the_opposite_corner_and_follows_the_pointer() {
        let f = Frame {
            center: [5.0, 5.0],
            half: [5.0, 5.0],
            angle: 0.0,
        };
        let m = resize_map(&f, Corner::BottomRight, [20.0, 15.0]);
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
        let m = resize_map(&f, Corner::TopRight, target);
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
        let m = resize_map(&f, Corner::BottomRight, [12.0, 3.0]);
        assert_close(m.apply([10.0, 0.0]), [12.0, 0.0]);
    }

    #[test]
    fn rotate_map_turns_about_the_center_by_the_pointer_sweep() {
        let f = Frame {
            center: [10.0, 10.0],
            half: [3.0, 3.0],
            angle: 0.0,
        };
        let m = rotate_map(&f, [20.0, 10.0], [10.0, 20.0]);
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
}
