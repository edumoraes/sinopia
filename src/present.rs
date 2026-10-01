//! A presentation's deck: the frames linked one to the next. A frame's
//! `next` names the frame after it, wherever the two stand. The arrows
//! between linked frames, and the handle a link is pulled out of, are
//! drawn here, so the canvas and the press cannot disagree about where
//! the handle is. Pure, like the rest of the core.

use std::f64::consts::PI;

use crate::doc::{Document, Element, Frame};
use crate::scene::{Prim, Rgba, ScreenRect, View, with_alpha};

/// A box in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    pub fn of(f: &Frame) -> Area {
        Area {
            x: f.x,
            y: f.y,
            w: f.w,
            h: f.h,
        }
    }

    /// Where it falls on screen, in physical px.
    fn on_screen(&self, view: &View) -> (f64, f64, f64, f64) {
        let (x0, y0) = view.world_to_screen(self.x, self.y);
        let (x1, y1) = view.world_to_screen(self.x + self.w, self.y + self.h);
        (x0, y0, x1, y1)
    }
}

/// The frame `f` links to, when that is a frame on the board and not `f`
/// itself. A link to a frame that has gone leads nowhere.
pub fn next_of<'a>(doc: &'a Document, f: &Frame) -> Option<&'a Frame> {
    let id = f.next.as_deref()?;
    doc.frame(id).filter(|n| n.id != f.id)
}

/// Every frame on the board, in paint order.
fn frames(doc: &Document) -> impl Iterator<Item = &Frame> {
    doc.elements.iter().filter_map(|el| match el {
        Element::Frame(f) => Some(f),
        _ => None,
    })
}

/// How far out of a frame's right edge its link handle stands, and how
/// large it is drawn and hit, in logical px.
const HANDLE_OUT_PX: f64 = 18.0;
const HANDLE_PX: f64 = 7.0;
const HANDLE_HIT_PX: f64 = 11.0;
/// An arrow's line, its head and its bend, in logical px and as a share
/// of its length.
const ARROW_HALF_PX: f32 = 1.25;
const HEAD_PX: f64 = 10.0;
const HEAD_ANGLE: f64 = 0.5;
const BEND: f64 = 0.12;
const ARROW_STEPS: usize = 16;

/// Where `f`'s link handle stands on screen: out of the middle of its
/// right edge, the way a sequence reads.
pub fn handle_at(f: &Frame, view: &View) -> (f64, f64) {
    let (x, y) = view.world_to_screen(f.x + f.w, f.y + f.h / 2.0);
    (x + HANDLE_OUT_PX * view.scale, y)
}

/// Whether `screen` is on `f`'s link handle.
pub fn on_handle(f: &Frame, view: &View, screen: (f64, f64)) -> bool {
    let (x, y) = handle_at(f, view);
    (screen.0 - x).hypot(screen.1 - y) <= HANDLE_HIT_PX * view.scale
}

/// `f`'s link handle: a ring with a dot in it, in `ink` on `ground`.
pub fn handle_prims(f: &Frame, view: &View, ink: Rgba, ground: Rgba) -> Vec<Prim> {
    let (x, y) = handle_at(f, view);
    let (x, y, s) = (x as f32, y as f32, view.scale as f32);
    let r = HANDLE_PX as f32 * s;
    vec![
        Prim::circle(x, y, r, ink),
        Prim::circle(x, y, r - 2.0 * s, ground),
        Prim::circle(x, y, 2.5 * s, ink),
    ]
}

/// A screen box: the corners of `area` on screen.
fn screen_box(area: Area, view: &View) -> (f64, f64, f64, f64) {
    area.on_screen(view)
}

/// Where the line from the middle of `b` toward `toward` leaves it.
fn exit(b: (f64, f64, f64, f64), toward: (f64, f64)) -> (f64, f64) {
    let c = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
    let (hw, hh) = ((b.2 - b.0) / 2.0, (b.3 - b.1) / 2.0);
    let (dx, dy) = (toward.0 - c.0, toward.1 - c.1);
    let tx = if dx.abs() > 1e-9 { hw / dx.abs() } else { f64::INFINITY };
    let ty = if dy.abs() > 1e-9 { hh / dy.abs() } else { f64::INFINITY };
    let t = tx.min(ty).min(1.0);
    (c.0 + dx * t, c.1 + dy * t)
}

/// An arrow on screen: where it leaves, the point it bends toward and
/// where it lands — a quadratic Bézier. The drawing and the pointer read
/// the same one, so a press lands on the arrow that is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Curve {
    a: (f64, f64),
    c: (f64, f64),
    b: (f64, f64),
}

impl Curve {
    /// From `a` to `b`, bent a little to its left so two links between
    /// the same frames, one each way, do not lie on each other. None when
    /// the two ends are too close for an arrow to show.
    fn between(a: (f64, f64), b: (f64, f64), view: &View) -> Option<Curve> {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if len < 4.0 * view.scale {
            return None;
        }
        let (nx, ny) = (dy / len, -dx / len);
        let bend = BEND * len;
        let c = ((a.0 + b.0) / 2.0 + nx * bend, (a.1 + b.1) / 2.0 + ny * bend);
        Some(Curve { a, c, b })
    }

    fn at(&self, t: f64) -> (f64, f64) {
        let (a, c, b) = (self.a, self.c, self.b);
        let u = 1.0 - t;
        (
            u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0,
            u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1,
        )
    }

    /// The curve as the straight spans it is drawn in.
    fn spans(&self) -> impl Iterator<Item = ((f64, f64), (f64, f64))> + '_ {
        (0..ARROW_STEPS).map(|i| {
            let t = |k: usize| k as f64 / ARROW_STEPS as f64;
            (self.at(t(i)), self.at(t(i + 1)))
        })
    }

    /// How far `p` is from the curve, in screen px.
    fn distance(&self, p: (f64, f64)) -> f64 {
        self.spans()
            .map(|(q, r)| {
                let (dx, dy) = (r.0 - q.0, r.1 - q.1);
                let along = ((p.0 - q.0) * dx + (p.1 - q.1) * dy) / (dx * dx + dy * dy).max(1e-12);
                let t = along.clamp(0.0, 1.0);
                (p.0 - (q.0 + dx * t)).hypot(p.1 - (q.1 + dy * t))
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// The arrow: the line, a head where it lands pointing the way the
    /// curve arrives, and a dot where it leaves.
    fn prims(&self, view: &View, ink: Rgba) -> Vec<Prim> {
        let s = view.scale;
        let half = ARROW_HALF_PX * s as f32;
        let f = |p: (f64, f64)| (p.0 as f32, p.1 as f32);
        let mut out: Vec<Prim> = self.spans().map(|(p, q)| Prim::segment(f(p), f(q), half, ink)).collect();
        let (tx, ty) = (self.b.0 - self.c.0, self.b.1 - self.c.1);
        let back = tx.atan2(ty) + PI;
        for side in [-HEAD_ANGLE, HEAD_ANGLE] {
            let angle = back + side;
            let tip = (self.b.0 + HEAD_PX * s * angle.sin(), self.b.1 + HEAD_PX * s * angle.cos());
            out.push(Prim::segment(f(self.b), f(tip), half, ink));
        }
        out.push(Prim::circle(self.a.0 as f32, self.a.1 as f32, 2.5 * s as f32, ink));
        out
    }
}

/// The arrow of `f`'s link, when it leads to a frame and both are on
/// show: from `f`'s edge to the next one's, each where the line between
/// their middles crosses it.
fn arrow_of(doc: &Document, view: &View, f: &Frame) -> Option<Curve> {
    if !doc.shown(&f.layer) {
        return None;
    }
    let n = next_of(doc, f).filter(|n| doc.shown(&n.layer))?;
    let (from, to) = (screen_box(Area::of(f), view), screen_box(Area::of(n), view));
    let centre = |b: (f64, f64, f64, f64)| ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
    Curve::between(exit(from, centre(to)), exit(to, centre(from)), view)
}

/// How near an arrow a press has to land to take hold of it, in logical
/// px.
const ARROW_HIT_PX: f64 = 6.0;

/// The links between frames on show, each an arrow from its frame's edge
/// to the next one's, in `ink` — all but the one out of `held`, which is
/// in the pointer's hand.
pub fn links(doc: &Document, view: &View, ink: Rgba, held: Option<&str>) -> Vec<Prim> {
    frames(doc)
        .filter(|f| Some(f.id.as_str()) != held)
        .filter_map(|f| arrow_of(doc, view, f))
        .flat_map(|curve| curve.prims(view, with_alpha(ink, 0.85)))
        .collect()
}

/// The frame whose link's arrow passes within reach of `screen`: pressed
/// there, the link is taken hold of and can be pulled onto another frame
/// or off into nothing. The nearest, where two pass close.
pub fn link_at<'a>(doc: &'a Document, view: &View, screen: (f64, f64)) -> Option<&'a Frame> {
    let reach = ARROW_HIT_PX * view.scale;
    frames(doc)
        .filter_map(|f| Some((arrow_of(doc, view, f)?.distance(screen), f)))
        .filter(|(d, _)| *d <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
}

/// The frame under `world` a link could be pulled onto: the topmost one
/// on show holding the point.
pub fn frame_under(doc: &Document, world: [f64; 2]) -> Option<&Frame> {
    doc.stack_at(world).and_then(|layer| doc.frame_on(layer))
}

/// Whether `target` takes a link from frame `from`: it is another frame,
/// and no frame but `from` leads to it already. A slide comes after one
/// slide at most, so a deck stays a chain — two decks never merge into
/// one slide. A board that already holds two links into one frame still
/// opens; it is only that no new one is laid.
pub fn takes(doc: &Document, from: &str, target: &Frame) -> bool {
    target.id != from
        && !frames(doc).any(|f| f.id != from && next_of(doc, f).is_some_and(|n| n.id == target.id))
}

/// A link being pulled out of frame `from` to `to` (world): the arrow to
/// the pointer, and the frame it would land on ringed — only one that
/// takes it.
pub fn pulling(doc: &Document, view: &View, from: &str, to: [f64; 2], ink: Rgba) -> Vec<Prim> {
    let Some(f) = doc.frame(from) else {
        return Vec::new();
    };
    let point = view.world_to_screen(to[0], to[1]);
    let mut out = Vec::new();
    let target = frame_under(doc, to).filter(|t| takes(doc, from, t));
    if let Some(t) = target {
        let (x0, y0, x1, y1) = screen_box(Area::of(t), view);
        let s = view.scale as f32;
        let r = ScreenRect {
            x: x0 as f32 - 2.0 * s,
            y: y0 as f32 - 2.0 * s,
            w: (x1 - x0) as f32 + 4.0 * s,
            h: (y1 - y0) as f32 + 4.0 * s,
        };
        out.push(Prim::rect(r, ink).ring(2.0 * s));
    }
    let a = exit(screen_box(Area::of(f), view), point);
    if let Some(curve) = Curve::between(a, point, view) {
        out.extend(curve.prims(view, ink));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Kind, Layer};
    use crate::scene::Viewport;

    const VP: Viewport = Viewport { w: 1000, h: 500 };

    fn view(camera: Camera) -> View {
        View {
            camera,
            viewport: VP,
            scale: 1.0,
        }
    }

    /// A board of frames `ids`, side by side, each 200 by 100, none
    /// linked yet.
    fn deck(ids: &[&str]) -> Document {
        let mut doc = Document::new("t");
        for (i, id) in ids.iter().enumerate() {
            let layer = format!("{id}-layer");
            doc.layers.push(Layer {
                id: layer.clone(),
                ..Layer::of(id, Kind::Frame)
            });
            doc.elements.push(Element::Frame(Frame {
                id: (*id).into(),
                layer,
                x: 300.0 * i as f64,
                y: 0.0,
                w: 200.0,
                h: 100.0,
                background: None,
                next: None,
                layers: vec![Layer::new("Layer 1")],
            }));
        }
        doc
    }

    fn link(doc: &mut Document, from: &str, to: &str) {
        doc.frame_mut(from).unwrap().next = Some(to.into());
    }

    #[test]
    fn the_handle_stands_out_of_the_right_edge_and_is_hit_round_it() {
        let doc = deck(&["a"]);
        let f = doc.frame("a").unwrap();
        let v = view(Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        });
        let (x, y) = handle_at(f, &v);
        assert_eq!((x, y), (600.0 + HANDLE_OUT_PX, 250.0));
        assert!(on_handle(f, &v, (x + 5.0, y - 5.0)));
        assert!(!on_handle(f, &v, (590.0, 250.0)), "inside the frame is the frame's");
    }

    #[test]
    fn an_arrow_runs_from_one_frames_edge_to_the_others() {
        let mut doc = deck(&["a", "b"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let ink = [1.0, 0.0, 0.0, 1.0];
        assert!(links(&doc, &v, ink, None).is_empty(), "nothing linked, nothing drawn");
        link(&mut doc, "a", "b");
        let prims = links(&doc, &v, ink, None);
        assert!(links(&doc, &v, ink, Some("a")).is_empty(), "the link in the hand is not drawn twice");
        assert!(!prims.is_empty());
        // a ends at x = 200 and b begins at 300: in this view 450 and 550.
        let first = prims[0].geom;
        let last_shaft = prims[ARROW_STEPS - 1].geom;
        assert!((first[0] - 450.0).abs() < 0.01, "from a's edge: {first:?}");
        assert!((last_shaft[2] - 550.0).abs() < 0.01, "to b's edge: {last_shaft:?}");
        doc.layers.iter_mut().find(|l| l.id == "b-layer").unwrap().visible = false;
        assert!(links(&doc, &v, ink, None).is_empty(), "no arrow into a hidden frame");
    }

    #[test]
    fn a_press_on_an_arrow_takes_hold_of_its_link() {
        let mut doc = deck(&["a", "b"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        // a's right edge is at 450 on screen and b's left at 550, both
        // at the middle height 250; the arrow bends up between them.
        assert!(link_at(&doc, &v, (500.0, 250.0)).is_none(), "nothing linked");
        link(&mut doc, "a", "b");
        let curve = arrow_of(&doc, &v, doc.frame("a").unwrap()).unwrap();
        let middle = curve.at(0.5);
        assert!(middle.1 < 250.0, "bent: {middle:?}");
        assert_eq!(link_at(&doc, &v, middle).map(|f| f.id.as_str()), Some("a"));
        assert_eq!(link_at(&doc, &v, (middle.0, middle.1 + 5.0)).map(|f| f.id.as_str()), Some("a"));
        assert!(link_at(&doc, &v, (middle.0, middle.1 + 30.0)).is_none(), "too far off it");
    }

    #[test]
    fn a_frame_takes_one_link_in_and_never_its_own() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "a", "b");
        let frame = |id| doc.frame(id).unwrap();
        assert!(!takes(&doc, "c", frame("b")), "a already leads to b");
        assert!(takes(&doc, "a", frame("b")), "a's own link, laid again");
        assert!(takes(&doc, "c", frame("a")), "nothing leads to a");
        assert!(takes(&doc, "b", frame("a")), "a loop back to the head is one link in");
        assert!(!takes(&doc, "a", frame("a")), "never itself");
        let mut gone = deck(&["a", "b"]);
        link(&mut gone, "a", "a");
        assert!(takes(&gone, "b", gone.frame("a").unwrap()), "a link to itself leads nowhere");
    }

    #[test]
    fn pulling_rings_the_frame_it_would_land_on_and_not_its_own() {
        let doc = deck(&["a", "b"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let ink = [1.0, 0.0, 0.0, 1.0];
        let ringed = |to| pulling(&doc, &v, "a", to, ink).iter().any(|p| p.line > 0.0);
        assert!(ringed([350.0, 50.0]), "over b");
        assert!(!ringed([100.0, 50.0]), "over itself");
        assert!(!ringed([250.0, 400.0]), "over nothing");
    }
}
