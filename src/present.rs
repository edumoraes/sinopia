//! A presentation: the frames linked one to the next, shown one at a
//! time. A frame's `next` names the frame after it, wherever the two
//! stand; a show walks those links from where it starts, fits each slide
//! to the window and holds the camera inside it — zoom out stops at the
//! whole slide, and a pan never looks past its edges. Between two slides
//! the camera flies, from what is on screen to the next slide, opening
//! out on the way as far as it takes to keep both in sight.
//!
//! Pure, like the rest of the core: `app` keeps a [`Show`] while
//! presenting, ages it on its clock and asks it for the camera and for
//! what to cover. The arrows between linked frames, and the handle a
//! link is pulled out of, are drawn here too, so the canvas and the
//! press cannot disagree about where the handle is.

use std::f64::consts::PI;

use crate::doc::{Camera, Document, Element, Frame};
use crate::scene::{Prim, Rgba, ScreenRect, View, Viewport, with_alpha};

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

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// What `view` shows of the world.
    pub fn seen(view: &View) -> Area {
        let k = view.px_per_world();
        let w = f64::from(view.viewport.w) / k;
        let h = f64::from(view.viewport.h) / k;
        Area {
            x: view.camera.x - w / 2.0,
            y: view.camera.y - h / 2.0,
            w,
            h,
        }
    }

    /// The part of both, if they meet.
    pub fn meet(&self, other: &Area) -> Option<Area> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let w = (self.x + self.w).min(other.x + other.w) - x;
        let h = (self.y + self.h).min(other.y + other.h) - y;
        (w > 0.0 && h > 0.0).then_some(Area { x, y, w, h })
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

/// The slides from `start` on: it, then every frame its links lead to,
/// each once — a deck that loops back ends where it would repeat. A
/// hidden frame is passed through rather than shown: what is not painted
/// is not a slide, and the frames after it still are.
pub fn sequence(doc: &Document, start: &str) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    let mut at = doc.frame(start);
    while let Some(f) = at {
        if seen.contains(&f.id.as_str()) {
            break;
        }
        seen.push(&f.id);
        if doc.shown(&f.layer) {
            out.push(f.id.clone());
        }
        at = next_of(doc, f);
    }
    out
}

/// Where a show starts: the frame selected, if one is; else the first
/// frame that leads somewhere and that nothing leads to — the head of a
/// deck; else the first frame that leads anywhere, since a deck that
/// loops has no head; else the first frame there is.
pub fn first(doc: &Document, selection: &[String]) -> Option<String> {
    let selected = selection.iter().find_map(|id| doc.frame(id));
    if let Some(f) = selected {
        return Some(f.id.clone());
    }
    let led_to: Vec<&str> = frames(doc)
        .filter_map(|f| next_of(doc, f))
        .map(|n| n.id.as_str())
        .collect();
    let leads = |f: &&Frame| next_of(doc, f).is_some();
    frames(doc)
        .filter(leads)
        .find(|f| !led_to.contains(&f.id.as_str()))
        .or_else(|| frames(doc).find(leads))
        .or_else(|| frames(doc).next())
        .map(|f| f.id.clone())
}

/// Whether a show started with `selection` would have a slide to show.
pub fn presentable(doc: &Document, selection: &[String]) -> bool {
    first(doc, selection).is_some_and(|start| !sequence(doc, &start).is_empty())
}

/// The camera that shows `area` whole, as large as the window allows and
/// in its middle.
pub fn fit(area: Area, viewport: Viewport, scale: f64) -> Camera {
    let (x, y) = area.center();
    let zoom = (f64::from(viewport.w) / (area.w * scale)).min(f64::from(viewport.h) / (area.h * scale));
    Camera { x, y, zoom }
}

/// `camera`, kept inside `area`: never further out than the whole of it,
/// and never looking past its edges. On an axis the window shows more of
/// than the area has — the bars a slide of another shape leaves — the
/// area stands in the middle.
pub fn hold(camera: Camera, area: Area, viewport: Viewport, scale: f64) -> Camera {
    let least = fit(area, viewport, scale);
    let zoom = if camera.zoom.is_finite() {
        camera.zoom.max(least.zoom)
    } else {
        least.zoom
    };
    let k = zoom * scale;
    let keep = |at: f64, from: f64, size: f64, window: f64| {
        let half = window / k / 2.0;
        // A hair of slack: at the fit, one axis shows exactly the
        // area, and rounding must not turn that into a clamp that
        // moves it.
        if 2.0 * half >= size - size * 1e-9 {
            from + size / 2.0
        } else {
            at.clamp(from + half, from + size - half)
        }
    };
    Camera {
        x: keep(camera.x, area.x, area.w, f64::from(viewport.w)),
        y: keep(camera.y, area.y, area.h, f64::from(viewport.h)),
        zoom,
    }
}

/// How far a flight opens out at its middle, as a power of what it
/// would take to keep both ends in sight: all of it would pull back to
/// an overview between every pair of slides, none would slide flat
/// across whatever is between them.
const LIFT: f64 = 0.6;
/// How long a flight takes at the least, and how much longer for every
/// e-fold it opens out by.
const FLIGHT_S: f64 = 0.7;
const FLIGHT_PER_LIFT_S: f64 = 0.3;
const FLIGHT_MAX_S: f64 = 1.6;

/// The camera on its way from one area to another.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flight {
    from: Area,
    to: Area,
    /// Seconds flown, and how long the whole flight takes.
    t: f64,
    length: f64,
    /// How many times larger than the plain blend of the two the area
    /// shown is at the middle.
    lift: f64,
}

impl Flight {
    fn new(from: Area, to: Area) -> Flight {
        let (a, b) = (from.center(), to.center());
        let wide = (b.0 - a.0).abs() + from.w.max(to.w);
        let tall = (b.1 - a.1).abs() + from.h.max(to.h);
        let need = (wide / (from.w * to.w).sqrt()).max(tall / (from.h * to.h).sqrt());
        let lift = need.max(1.0).powf(LIFT);
        let length = (FLIGHT_S + FLIGHT_PER_LIFT_S * lift.ln()).min(FLIGHT_MAX_S);
        Flight {
            from,
            to,
            t: 0.0,
            length,
            lift,
        }
    }

    fn done(&self) -> bool {
        self.t >= self.length
    }

    /// The area shown so far along: the middle eased across, the size
    /// blended geometrically — a zoom feels even when it multiplies — and
    /// opened out by the lift, most at the middle and not at all at
    /// either end.
    fn area(&self) -> Area {
        let e = ease((self.t / self.length).clamp(0.0, 1.0));
        let (a, b) = (self.from.center(), self.to.center());
        let open = self.lift.powf((PI * e).sin());
        let w = self.from.w.powf(1.0 - e) * self.to.w.powf(e) * open;
        let h = self.from.h.powf(1.0 - e) * self.to.h.powf(e) * open;
        let (x, y) = (a.0 + (b.0 - a.0) * e, a.1 + (b.1 - a.1) * e);
        Area {
            x: x - w / 2.0,
            y: y - h / 2.0,
            w,
            h,
        }
    }
}

/// Slow out, fast through the middle, slow in.
pub fn ease(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// A presentation in progress: the slides it walks, the one it is on,
/// the flight to it while there is one, and the camera to put back at
/// the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Show {
    slides: Vec<String>,
    at: usize,
    flight: Option<Flight>,
    /// Where the board was looking before the show, put back after it.
    pub before: Camera,
}

impl Show {
    /// A show from [`first`], flying in from what `view` shows. None when
    /// the board has no frame on show to present.
    pub fn start(doc: &Document, selection: &[String], view: &View) -> Option<Show> {
        let slides = sequence(doc, &first(doc, selection)?);
        let to = Area::of(doc.frame(slides.first()?)?);
        Some(Show {
            slides,
            at: 0,
            flight: Some(Flight::new(Area::seen(view), to)),
            before: view.camera,
        })
    }

    /// The slide on show, and how many there are.
    #[cfg(test)]
    pub fn place(&self) -> (usize, usize) {
        (self.at, self.slides.len())
    }

    /// The frame on show — none once it has gone from the board, which
    /// ends the show.
    pub fn slide<'a>(&self, doc: &'a Document) -> Option<&'a Frame> {
        doc.frame(self.slides.get(self.at)?)
    }

    /// Goes to slide `to`, flying there from whatever is on screen. False
    /// when that is where it already is, or past either end.
    pub fn go(&mut self, doc: &Document, view: &View, to: usize) -> bool {
        if to == self.at || to >= self.slides.len() {
            return false;
        }
        let Some(target) = doc.frame(&self.slides[to]) else {
            return false;
        };
        let Some(from) = self.shown(doc, view) else {
            return false;
        };
        self.at = to;
        self.flight = Some(Flight::new(from, Area::of(target)));
        true
    }

    /// One slide on or back.
    pub fn step(&mut self, doc: &Document, view: &View, forward: bool) -> bool {
        let to = if forward {
            self.at + 1
        } else {
            match self.at.checked_sub(1) {
                Some(to) => to,
                None => return false,
            }
        };
        self.go(doc, view, to)
    }

    /// The last slide's place.
    pub fn last(&self) -> usize {
        self.slides.len().saturating_sub(1)
    }

    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }

    /// Ages the flight by `dt` seconds, landing it once it is over.
    pub fn tick(&mut self, dt: f64) {
        if let Some(f) = &mut self.flight {
            f.t += dt;
            if f.done() {
                self.flight = None;
            }
        }
    }

    /// What is shown of the slide: the area flown through, during a
    /// flight; else as much of the slide as `view` shows. The window
    /// around it is covered.
    pub fn shown(&self, doc: &Document, view: &View) -> Option<Area> {
        if let Some(f) = &self.flight {
            return Some(f.area());
        }
        let slide = Area::of(self.slide(doc)?);
        Area::seen(view).meet(&slide).or(Some(slide))
    }

    /// The camera the show asks for: the one that fits the area flown
    /// through, during a flight; else `camera` held inside the slide.
    pub fn camera(&self, doc: &Document, view: &View, camera: Camera) -> Option<Camera> {
        if let Some(f) = &self.flight {
            return Some(fit(f.area(), view.viewport, view.scale));
        }
        let slide = Area::of(self.slide(doc)?);
        Some(hold(camera, slide, view.viewport, view.scale))
    }
}

/// The window less `area`: up to four bands, in physical px, that cover
/// what is not the slide.
pub fn veil(area: Area, view: &View) -> Vec<ScreenRect> {
    let (vw, vh) = (f64::from(view.viewport.w), f64::from(view.viewport.h));
    let (x0, y0, x1, y1) = area.on_screen(view);
    // Rounded outwards from the slide by nothing and onto whole pixels,
    // so no sliver of the board shows between a band and the slide.
    let (x0, y0) = (x0.floor().clamp(0.0, vw), y0.floor().clamp(0.0, vh));
    let (x1, y1) = (x1.ceil().clamp(0.0, vw), y1.ceil().clamp(0.0, vh));
    let band = |x: f64, y: f64, w: f64, h: f64| {
        (w > 0.0 && h > 0.0).then_some(ScreenRect {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        })
    };
    [
        band(0.0, 0.0, vw, y0),
        band(0.0, y1, vw, vh - y1),
        band(0.0, y0, x0, y1 - y0),
        band(x1, y0, vw - x1, y1 - y0),
    ]
    .into_iter()
    .flatten()
    .collect()
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
    use crate::doc::{Kind, Layer};

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

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn same(a: Area, b: Area) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h)
    }

    #[test]
    fn a_sequence_follows_the_links_wherever_the_frames_stand() {
        let mut doc = deck(&["a", "b", "c", "d"]);
        link(&mut doc, "c", "a");
        link(&mut doc, "a", "d");
        assert_eq!(sequence(&doc, "c"), ["c", "a", "d"]);
        assert_eq!(sequence(&doc, "b"), ["b"], "a frame linked to nothing is a deck of one");
    }

    #[test]
    fn a_loop_ends_where_it_would_repeat_and_a_link_to_itself_goes_nowhere() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "a", "b");
        link(&mut doc, "b", "c");
        link(&mut doc, "c", "a");
        assert_eq!(sequence(&doc, "b"), ["b", "c", "a"]);
        let mut doc = deck(&["a"]);
        link(&mut doc, "a", "a");
        assert_eq!(sequence(&doc, "a"), ["a"]);
        assert!(next_of(&doc, doc.frame("a").unwrap()).is_none());
    }

    #[test]
    fn a_link_to_a_frame_that_has_gone_goes_nowhere() {
        let mut doc = deck(&["a", "b"]);
        link(&mut doc, "a", "gone");
        assert_eq!(sequence(&doc, "a"), ["a"]);
    }

    #[test]
    fn a_hidden_frame_is_passed_through_and_not_shown() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "a", "b");
        link(&mut doc, "b", "c");
        doc.layers.iter_mut().find(|l| l.id == "b-layer").unwrap().visible = false;
        assert_eq!(sequence(&doc, "a"), ["a", "c"]);
    }

    #[test]
    fn a_show_starts_at_the_frame_selected_then_at_the_head_of_a_deck() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "c", "b");
        link(&mut doc, "b", "a");
        assert_eq!(first(&doc, &["b".into()]).as_deref(), Some("b"));
        assert_eq!(first(&doc, &["not-a-frame".into()]).as_deref(), Some("c"), "the head");
        assert_eq!(first(&doc, &[]).as_deref(), Some("c"));
    }

    #[test]
    fn a_deck_that_loops_starts_at_its_first_frame_and_a_board_without_links_at_the_first() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "b", "c");
        link(&mut doc, "c", "b");
        assert_eq!(first(&doc, &[]).as_deref(), Some("b"), "the first that leads anywhere");
        assert_eq!(first(&deck(&["x", "y"]), &[]).as_deref(), Some("x"));
        assert_eq!(first(&Document::new("t"), &[]), None, "nothing to present");
        assert!(!presentable(&Document::new("t"), &[]));
        let mut hidden = deck(&["x"]);
        hidden.layers.iter_mut().find(|l| l.id == "x-layer").unwrap().visible = false;
        assert!(!presentable(&hidden, &[]), "a hidden frame is no slide");
        assert!(presentable(&deck(&["x"]), &[]));
    }

    #[test]
    fn fit_shows_the_whole_area_in_the_middle_of_the_window() {
        let area = Area {
            x: 100.0,
            y: 50.0,
            w: 400.0,
            h: 100.0,
        };
        let c = fit(area, VP, 1.0);
        assert_eq!((c.x, c.y), (300.0, 100.0));
        assert!(close(c.zoom, 2.5), "the width decides: {}", c.zoom);
        let seen = Area::seen(&view(c));
        assert!(close(seen.w, 400.0) && seen.h >= 100.0);
        // At scale 2 the same area takes the same share of the window.
        assert!(close(fit(area, VP, 2.0).zoom, 1.25));
    }

    #[test]
    fn hold_stops_zooming_out_at_the_whole_slide() {
        let area = Area {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let out = Camera {
            x: 999.0,
            y: -999.0,
            zoom: 0.1,
        };
        assert_eq!(hold(out, area, VP, 1.0), fit(area, VP, 1.0));
    }

    #[test]
    fn hold_keeps_a_zoomed_in_view_inside_the_slide() {
        let area = Area {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        // At zoom 20 the window shows 50 by 25 of the world.
        let held = hold(
            Camera {
                x: -500.0,
                y: 500.0,
                zoom: 20.0,
            },
            area,
            VP,
            1.0,
        );
        assert_eq!(held.zoom, 20.0, "zooming in is the person's");
        assert!(close(held.x, 25.0) && close(held.y, 87.5), "{held:?}");
        let inside = Camera {
            x: 120.0,
            y: 40.0,
            zoom: 20.0,
        };
        assert_eq!(hold(inside, area, VP, 1.0), inside, "a view inside is left alone");
    }

    #[test]
    fn hold_centres_the_slide_on_the_axis_the_window_has_room_to_spare_on() {
        // A tall slide in a wide window: at the fit, the height decides,
        // and across there is room either side.
        let area = Area {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 500.0,
        };
        let c = hold(
            Camera {
                x: 70.0,
                y: 250.0,
                zoom: 1.0,
            },
            area,
            VP,
            1.0,
        );
        assert_eq!(c.x, 50.0, "the middle, wherever it was panned to");
        let nudged = hold(Camera { x: 60.0, ..c }, area, VP, 1.0);
        assert_eq!(nudged, c);
    }

    #[test]
    fn a_flight_leaves_from_one_area_and_lands_on_the_other() {
        let from = Area {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let to = Area {
            x: 3000.0,
            y: 400.0,
            w: 400.0,
            h: 200.0,
        };
        let mut f = Flight::new(from, to);
        assert!(same(f.area(), from));
        f.t = f.length;
        assert!(same(f.area(), to), "{:?}", f.area());
    }

    #[test]
    fn a_flight_opens_out_on_the_way_further_the_further_it_goes() {
        let a = Area {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let near = Area { x: 300.0, ..a };
        let far = Area { x: 3000.0, ..a };
        let middle = |to| {
            let mut f = Flight::new(a, to);
            f.t = f.length / 2.0;
            f.area().w
        };
        assert!(middle(near) > a.w, "even next door: {}", middle(near));
        assert!(middle(far) > middle(near));
        assert_eq!(Flight::new(a, a).lift, 1.0, "nowhere to go, nothing to open");
        assert!(Flight::new(a, far).length > Flight::new(a, near).length);
        assert!(Flight::new(a, far).length <= FLIGHT_MAX_S);
    }

    #[test]
    fn ease_starts_and_ends_still_and_passes_the_middle_at_half() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(0.5), 0.5);
        assert!(ease(0.1) < 0.1 && ease(0.9) > 0.9);
    }

    fn deck_of_three() -> Document {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "a", "b");
        link(&mut doc, "b", "c");
        doc
    }

    #[test]
    fn a_show_flies_in_and_lands_on_its_first_slide() {
        let doc = deck_of_three();
        let v = view(Camera {
            x: 5000.0,
            y: 5000.0,
            zoom: 1.0,
        });
        let mut show = Show::start(&doc, &[], &v).unwrap();
        assert_eq!(show.place(), (0, 3));
        assert_eq!(show.before, v.camera);
        assert!(show.flying());
        let leaving = show.camera(&doc, &v, v.camera).unwrap();
        assert!(close(leaving.x, 5000.0) && close(leaving.zoom, 1.0), "it leaves from the view");
        show.tick(10.0);
        assert!(!show.flying());
        let slide = Area::of(doc.frame("a").unwrap());
        assert_eq!(show.camera(&doc, &v, v.camera), Some(fit(slide, VP, 1.0)));
    }

    #[test]
    fn a_show_steps_on_and_back_and_stops_at_either_end() {
        let doc = deck_of_three();
        let v = view(Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        });
        let mut show = Show::start(&doc, &[], &v).unwrap();
        show.tick(10.0);
        assert!(!show.step(&doc, &v, false), "nothing before the first");
        assert!(show.step(&doc, &v, true));
        assert_eq!(show.slide(&doc).unwrap().id, "b");
        assert!(show.flying());
        assert!(show.step(&doc, &v, true));
        assert!(!show.step(&doc, &v, true), "nothing after the last");
        assert_eq!(show.place(), (2, 3));
        assert!(show.go(&doc, &v, 0));
        assert_eq!(show.slide(&doc).unwrap().id, "a");
        assert!(!show.go(&doc, &v, 7));
    }

    #[test]
    fn a_step_mid_flight_leaves_from_where_the_camera_is() {
        let doc = deck_of_three();
        let v = view(fit(Area::of(doc.frame("a").unwrap()), VP, 1.0));
        let mut show = Show::start(&doc, &[], &v).unwrap();
        show.tick(10.0);
        show.step(&doc, &v, true);
        show.tick(0.3);
        let midway = show.shown(&doc, &v).unwrap();
        show.step(&doc, &v, true);
        assert!(same(show.shown(&doc, &v).unwrap(), midway), "no jump");
    }

    #[test]
    fn what_is_shown_is_the_slide_or_the_part_of_it_in_the_window() {
        let doc = deck_of_three();
        let slide = Area::of(doc.frame("a").unwrap());
        let mut show = Show::start(&doc, &[], &view(fit(slide, VP, 1.0))).unwrap();
        show.tick(10.0);
        let at_fit = view(fit(slide, VP, 1.0));
        assert!(same(show.shown(&doc, &at_fit).unwrap(), slide), "the whole slide");
        let zoomed = view(Camera {
            x: 50.0,
            y: 50.0,
            zoom: 20.0,
        });
        let part = show.shown(&doc, &zoomed).unwrap();
        assert!(close(part.w, 50.0) && close(part.h, 25.0), "{part:?}");
    }

    #[test]
    fn the_veil_covers_the_window_less_the_slide() {
        let v = view(Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        });
        // The area 0..200 by 0..100 lands on 400..600 by 200..300.
        let bands = veil(
            Area {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 100.0,
            },
            &v,
        );
        let covered: f32 = bands.iter().map(|b| b.w * b.h).sum();
        assert_eq!(covered, 1000.0 * 500.0 - 200.0 * 100.0);
        for b in &bands {
            let inside = b.x < 600.0 && b.x + b.w > 400.0 && b.y < 300.0 && b.y + b.h > 200.0;
            assert!(!inside, "{b:?} covers the slide");
        }
        let all = Area {
            x: -1000.0,
            y: -1000.0,
            w: 5000.0,
            h: 5000.0,
        };
        assert!(veil(all, &v).is_empty(), "a slide past every edge leaves nothing to cover");
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
