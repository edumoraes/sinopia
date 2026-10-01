//! A presentation: the stops of a deck, shown one at a time. A stop is a
//! layer — a frame, a group, or the layer one object stands on — and a
//! layer's `next` names the stop after it, wherever the two stand. A show
//! walks those links from where it starts, fits each stop to the window
//! and holds the camera inside it: zoom out stops at the whole of it, and
//! a pan never looks past its edges. A frame is shown as a slide, the
//! window round it covered; anything else is shown on the board, with
//! room round it and the board in sight. Between two stops the camera
//! flies along the path van Wijk and Nuij worked out for zooming and
//! panning together: straight in to what is already in sight, out only
//! as far as it takes to reach what is not.
//!
//! Pure, like the rest of the core: `app` keeps a [`Show`] while
//! presenting, ages it on its clock and asks it for the camera and for
//! what to cover. The arrows between linked stops, and the handle a link
//! is pulled out of, are drawn here too, so the canvas and the press
//! cannot disagree about where they are.

use std::collections::HashSet;
use std::f64::consts::PI;

use crate::doc::{Camera, Document, Frame, Kind, Layer};
use crate::scene::{Prim, Rgba, ScreenRect, View, Viewport, with_alpha};
use crate::select;
use crate::text::Atlas;

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

    /// The box round both.
    fn union(&self, other: &Area) -> Area {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Area {
            x,
            y,
            w: (self.x + self.w).max(other.x + other.w) - x,
            h: (self.y + self.h).max(other.y + other.h) - y,
        }
    }

    /// Moved as little as it takes to lie inside `bounds` on every axis it
    /// fits on, and centred on `bounds` on one it does not.
    fn inside(self, bounds: &Area) -> Area {
        let fit = |at: f64, size: f64, from: f64, room: f64| {
            if size <= room {
                at.clamp(from, from + room - size)
            } else {
                from + (room - size) / 2.0
            }
        };
        Area {
            x: fit(self.x, self.w, bounds.x, bounds.w),
            y: fit(self.y, self.h, bounds.y, bounds.h),
            ..self
        }
    }

    /// Whether `other` is wholly inside it.
    fn holds(&self, other: &Area) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.w <= self.x + self.w
            && other.y + other.h <= self.y + self.h
    }

    /// Where it falls on screen, in physical px.
    fn on_screen(&self, view: &View) -> (f64, f64, f64, f64) {
        let (x0, y0) = view.world_to_screen(self.x, self.y);
        let (x1, y1) = view.world_to_screen(self.x + self.w, self.y + self.h);
        (x0, y0, x1, y1)
    }
}

/// How much room a stop that is not a frame is shown with, on every
/// side, as a share of its longer side: what it holds does not touch the
/// window's edges.
const ROOM: f64 = 0.08;

/// What a stop shows: the box of what it paints, whether it is a slide —
/// a frame, shown with the window round it covered — and the frame a stop
/// that is no frame stands in, if it stands in one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// A frame's own box; else the box round the ink of everything the
    /// layer holds that is painted, as far as the frame it stands in
    /// lets it show.
    pub ink: Area,
    pub slide: bool,
    /// The box of the frame it stands in: it is part of that slide, so it
    /// is shown inside it and the window past it is covered as the
    /// slide's is.
    pub within: Option<Area>,
}

impl Stop {
    /// What a show fits to a window of `viewport`'s shape: a slide's own
    /// box; anything else its ink with room round it, made as wide or as
    /// tall as it takes to have the window's shape — so nothing round it
    /// is covered, and a pan inside it can reach every part of the window
    /// — and moved, inside a frame, as little as it takes to keep inside
    /// that frame, so a zoom into part of a slide shows the slide round
    /// it and not the board past its edge.
    pub fn area(&self, viewport: Viewport) -> Area {
        if self.slide {
            return self.ink;
        }
        let room = ROOM * self.ink.w.max(self.ink.h);
        let (w, h) = ((self.ink.w + 2.0 * room).max(1.0), (self.ink.h + 2.0 * room).max(1.0));
        let shape = aspect(viewport);
        let (w, h) = if w / h < shape { (h * shape, h) } else { (w, w / shape) };
        let (cx, cy) = self.ink.center();
        let area = Area {
            x: cx - w / 2.0,
            y: cy - h / 2.0,
            w,
            h,
        };
        match self.within {
            Some(frame) => area.inside(&frame),
            None => area,
        }
    }

    /// What the window is not covered past: a slide's own box, or the
    /// frame a stop stands in — none for a stop out on the board.
    fn bounds(&self) -> Option<Area> {
        if self.slide { Some(self.ink) } else { self.within }
    }

    /// How strongly the window round its area is covered: all of it for
    /// a slide and for what stands in one, none for a stop on the board.
    pub fn veil(&self) -> f64 {
        if self.bounds().is_some() { 1.0 } else { 0.0 }
    }
}

/// A window's shape: its width over its height.
fn aspect(viewport: Viewport) -> f64 {
    f64::from(viewport.w.max(1)) / f64::from(viewport.h.max(1))
}

/// The stop layer `id` is, when it is one: a frame layer is its frame;
/// any other layer is the box round what it paints — its own object, or
/// everything under a group. A layer hidden, or holding nothing painted,
/// is no stop: what is not painted cannot be shown.
pub fn stop(doc: &Document, id: &str) -> Option<Stop> {
    if !doc.shown(id) {
        return None;
    }
    let layer = doc.layer(id)?;
    if layer.kind == Kind::Frame {
        return doc.frame_on(id).map(|f| Stop {
            ink: Area::of(f),
            slide: true,
            within: None,
        });
    }
    let within = doc.frame_holding(id).map(Area::of);
    // The layers under it on show — a hidden one takes what it holds with
    // it, as painting does — and then one walk of the elements: this is
    // asked for every stop on every frame of the screen.
    fn shown_under<'a>(doc: &'a Document, layer: &'a Layer, out: &mut HashSet<&'a str>) {
        if layer.visible {
            out.insert(&layer.id);
            for inner in doc.inner(layer) {
                shown_under(doc, inner, out);
            }
        }
    }
    let mut held = HashSet::new();
    shown_under(doc, layer, &mut held);
    doc.elements
        .iter()
        .filter(|el| held.contains(el.layer()))
        .filter_map(|el| {
            let (lo, hi) = select::frame(el)?.aabb();
            let ink = Area {
                x: lo[0],
                y: lo[1],
                w: hi[0] - lo[0],
                h: hi[1] - lo[1],
            };
            // What the frame's boundary cuts away is not there to show.
            match within {
                Some(f) => ink.meet(&f),
                None => Some(ink),
            }
        })
        .reduce(|a, b| a.union(&b))
        .map(|ink| Stop {
            ink,
            slide: false,
            within,
        })
}

/// The layer `id` leads to, when that is another layer on the board. A
/// link to a layer that has gone leads nowhere, and so does a link to
/// itself.
pub fn next_of<'a>(doc: &'a Document, id: &str) -> Option<&'a str> {
    let next = doc.layer(id)?.next.as_deref()?;
    (next != id).then(|| doc.layer(next).map(|l| l.id.as_str())).flatten()
}

/// Every layer on the board in paint order: the board's stack bottom to
/// top, with what a group or a frame holds right after it.
fn layers(doc: &Document) -> Vec<&Layer> {
    fn walk<'a>(doc: &'a Document, stack: &'a [Layer], out: &mut Vec<&'a Layer>) {
        for layer in stack {
            out.push(layer);
            walk(doc, doc.inner(layer), out);
        }
    }
    let mut out = Vec::new();
    walk(doc, &doc.layers, &mut out);
    out
}

/// Every link on the board — a layer, and another layer of the board it
/// leads to — in paint order, read in one walk of the tree: what every
/// question about the decks asked of each layer, each with a walk of its
/// own, is asked once here.
fn linked(doc: &Document) -> Vec<(&str, &str)> {
    let all = layers(doc);
    let ids: HashSet<&str> = all.iter().map(|l| l.id.as_str()).collect();
    all.iter()
        .filter_map(|l| {
            let to = l.next.as_deref()?;
            (to != l.id && ids.contains(to)).then_some((l.id.as_str(), to))
        })
        .collect()
}

/// Whether a link leads out of `id` or into it.
fn in_deck(doc: &Document, id: &str) -> bool {
    linked(doc).iter().any(|&(from, to)| from == id || to == id)
}

/// The stops from `start` on: it, then every layer its links lead to,
/// each once — a deck that loops back ends where it would repeat. A
/// layer that is no stop — hidden, or with nothing painted — is passed
/// through rather than shown, and the stops after it still are.
pub fn sequence(doc: &Document, start: &str) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    let mut at = doc.layer(start).map(|l| l.id.as_str());
    while let Some(id) = at {
        if seen.contains(&id) {
            break;
        }
        seen.push(id);
        if stop(doc, id).is_some() {
            out.push(id.to_owned());
        }
        at = next_of(doc, id);
    }
    out
}

/// Where a show starts: the stop selected, when it is a frame or in a
/// deck — an object linked to nothing is no reason to show it alone —
/// else the first layer that leads somewhere and that nothing leads to,
/// the head of a deck; else the first that leads anywhere, since a deck
/// that loops has no head; else the first frame on show, a deck of one.
/// A deck with nothing on show is passed over for the next, so a deck
/// put away keeps no other from being presented.
pub fn first(doc: &Document, selected: Option<&str>) -> Option<String> {
    if let Some(id) = selected
        && stop(doc, id).is_some_and(|s| s.slide || in_deck(doc, id))
    {
        return Some(id.to_owned());
    }
    let links = linked(doc);
    let led_to: HashSet<&str> = links.iter().map(|&(_, to)| to).collect();
    let shows = |from: &str| !sequence(doc, from).is_empty();
    links
        .iter()
        .find(|&&(from, _)| !led_to.contains(from) && shows(from))
        .or_else(|| links.iter().find(|&&(from, _)| shows(from)))
        .map(|&(from, _)| from.to_owned())
        .or_else(|| {
            layers(doc)
                .into_iter()
                .find(|l| l.kind == Kind::Frame && stop(doc, &l.id).is_some())
                .map(|l| l.id.clone())
        })
}

/// Whether a show started with `selected` would have a stop to show.
pub fn presentable(doc: &Document, selected: Option<&str>) -> bool {
    first(doc, selected).is_some_and(|start| !sequence(doc, &start).is_empty())
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

/// How willing a flight is to zoom out to get somewhere rather than pan:
/// van Wijk and Nuij's ρ, and √2 is what they found people prefer.
const RHO: f64 = std::f64::consts::SQRT_2;

/// How long a flight takes: a base, and more for every unit of the path's
/// length, within a floor and a ceiling.
const FLIGHT_S: f64 = 0.45;
const FLIGHT_PER_S: f64 = 0.4;
const FLIGHT_MIN_S: f64 = 0.6;
const FLIGHT_MAX_S: f64 = 1.6;

/// The way from one view to another — a middle and a width in world
/// units — along the path of van Wijk and Nuij's "Smooth and efficient
/// zooming and panning" (2003): the mix of zooming and panning that is
/// shortest for the eye. Into what is already in sight it goes straight
/// in; to what is not, it opens out only as far as it takes to have both
/// in sight on the way.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Path {
    from: (f64, f64),
    to: (f64, f64),
    w0: f64,
    /// How far the middle travels.
    u1: f64,
    r0: f64,
    /// The path's length, in their units.
    s: f64,
    /// For a zoom with nowhere to go: which way the width moves.
    zoom: Option<f64>,
}

impl Path {
    fn new(from: (f64, f64), w0: f64, to: (f64, f64), w1: f64) -> Path {
        let (w0, w1) = (w0.max(1e-9), w1.max(1e-9));
        let u1 = (to.0 - from.0).hypot(to.1 - from.1);
        if u1 <= 1e-9 * w0.max(w1) {
            let ratio = (w1 / w0).ln();
            return Path {
                from,
                to,
                w0,
                u1,
                r0: 0.0,
                s: ratio.abs() / RHO,
                zoom: Some(ratio.signum()),
            };
        }
        let rho2 = RHO * RHO;
        let b = |w: f64, sign: f64| (w1 * w1 - w0 * w0 + sign * rho2 * rho2 * u1 * u1) / (2.0 * w * rho2 * u1);
        // ln(−b + √(b² + 1)), written so a large b keeps its precision.
        let (r0, r1) = (-b(w0, 1.0).asinh(), -b(w1, -1.0).asinh());
        Path {
            from,
            to,
            w0,
            u1,
            r0,
            s: (r1 - r0) / RHO,
            zoom: None,
        }
    }

    /// The middle and the width `s` along the path.
    fn at(&self, s: f64) -> ((f64, f64), f64) {
        let along = |u: f64| {
            let t = if self.u1 > 0.0 { u / self.u1 } else { 0.0 };
            (
                self.from.0 + (self.to.0 - self.from.0) * t,
                self.from.1 + (self.to.1 - self.from.1) * t,
            )
        };
        match self.zoom {
            Some(way) => (along(0.0), self.w0 * (way * RHO * s).exp()),
            None => {
                let (ch, sh) = (self.r0.cosh(), self.r0.sinh());
                let r = RHO * s + self.r0;
                let u = self.w0 / (RHO * RHO) * (ch * r.tanh() - sh);
                (along(u), self.w0 * ch / r.cosh())
            }
        }
    }
}

/// The camera on its way from one area to another, and the veil on its
/// way from one end's to the other's.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flight {
    from: Area,
    to: Area,
    /// How strongly the window round the area is covered at either end.
    veil: (f64, f64),
    /// The window's shape when it set off.
    aspect: f64,
    path: Path,
    /// Seconds flown, and how long the whole flight takes.
    t: f64,
    length: f64,
}

impl Flight {
    fn new(from: (Area, f64), to: (Area, f64), aspect: f64) -> Flight {
        let width = |a: Area| a.w.max(a.h * aspect);
        let path = Path::new(from.0.center(), width(from.0), to.0.center(), width(to.0));
        Flight {
            from: from.0,
            to: to.0,
            veil: (from.1, to.1),
            aspect,
            path,
            t: 0.0,
            length: (FLIGHT_S + FLIGHT_PER_S * path.s).clamp(FLIGHT_MIN_S, FLIGHT_MAX_S),
        }
    }

    fn done(&self) -> bool {
        self.t >= self.length
    }

    /// How far along it is, eased.
    fn progress(&self) -> f64 {
        ease((self.t / self.length).clamp(0.0, 1.0))
    }

    /// The area shown so far along: the middle and the width the path
    /// says, and a shape going from one end's to the other's, filling the
    /// window across or down all the way — the width is the path's.
    fn area(&self) -> Area {
        let e = self.progress();
        if e <= 0.0 {
            return self.from;
        }
        if e >= 1.0 {
            return self.to;
        }
        let ((x, y), width) = self.path.at(e * self.path.s);
        let fill = |a: Area| {
            let w = a.w.max(a.h * self.aspect);
            (a.w / w, a.h * self.aspect / w)
        };
        let (a, b) = (fill(self.from), fill(self.to));
        let (fw, fh) = (a.0 + (b.0 - a.0) * e, a.1 + (b.1 - a.1) * e);
        let most = fw.max(fh);
        let (w, h) = (width * fw / most, width / self.aspect * fh / most);
        Area {
            x: x - w / 2.0,
            y: y - h / 2.0,
            w,
            h,
        }
    }

    /// How strongly the window round the area is covered so far along.
    fn veil(&self) -> f64 {
        let e = self.progress();
        self.veil.0 + (self.veil.1 - self.veil.0) * e
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

/// A presentation in progress: the stops it walks, by their layers' ids,
/// the one it is on, the flight to it while there is one, and the camera
/// to put back at the end.
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
    /// the board has no stop on show to present.
    pub fn start(doc: &Document, selected: Option<&str>, view: &View) -> Option<Show> {
        Show::from(doc, &first(doc, selected)?, view)
    }

    /// A show from the stop on layer `start`, its deck from there on —
    /// alone, when it leads nowhere. None when it is no stop on show.
    pub fn from(doc: &Document, start: &str, view: &View) -> Option<Show> {
        let slides = sequence(doc, start);
        let to = stop(doc, slides.first()?)?;
        Some(Show {
            slides,
            at: 0,
            flight: Some(Flight::new(
                (Area::seen(view), 0.0),
                (to.area(view.viewport), to.veil()),
                aspect(view.viewport),
            )),
            before: view.camera,
        })
    }

    /// The stop on show, counted from 0, and how many there are.
    pub fn place(&self) -> (usize, usize) {
        (self.at, self.slides.len())
    }

    /// The layer of the stop on show.
    pub fn slide(&self) -> &str {
        &self.slides[self.at]
    }

    /// What the stop on show shows — none once it has gone from the
    /// board, or been hidden, which ends the show.
    pub fn stop(&self, doc: &Document) -> Option<Stop> {
        stop(doc, self.slides.get(self.at)?)
    }

    /// Goes to stop `to`, flying there from whatever is on screen. False
    /// when that is where it already is, past either end, or no longer
    /// a stop.
    pub fn go(&mut self, doc: &Document, view: &View, to: usize) -> bool {
        if to == self.at || to >= self.slides.len() {
            return false;
        }
        let Some(target) = stop(doc, &self.slides[to]) else {
            return false;
        };
        let Some(from) = self.shown(doc, view) else {
            return false;
        };
        self.at = to;
        self.flight = Some(Flight::new(
            from,
            (target.area(view.viewport), target.veil()),
            aspect(view.viewport),
        ));
        true
    }

    /// One stop on or back — past any that has stopped being one since
    /// the show began.
    pub fn step(&mut self, doc: &Document, view: &View, forward: bool) -> bool {
        let next = if forward {
            (self.at + 1..self.slides.len()).find(|&i| stop(doc, &self.slides[i]).is_some())
        } else {
            (0..self.at).rev().find(|&i| stop(doc, &self.slides[i]).is_some())
        };
        next.is_some_and(|to| self.go(doc, view, to))
    }

    /// The last stop's place.
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

    /// What is shown, and how strongly the window round it is covered:
    /// the area flown through, and the veil on its way, during a flight;
    /// else a slide — or the slide a stop stands in — as far as `view`
    /// shows it, covered round, or the whole window for a stop out on
    /// the board.
    pub fn shown(&self, doc: &Document, view: &View) -> Option<(Area, f64)> {
        if let Some(f) = &self.flight {
            return Some((f.area(), f.veil()));
        }
        let stop = self.stop(doc)?;
        let seen = Area::seen(view);
        Some(match stop.bounds() {
            Some(bounds) => (seen.meet(&bounds).unwrap_or(bounds), 1.0),
            None => (seen, 0.0),
        })
    }

    /// The camera the show asks for: the one that fits the area flown
    /// through, during a flight; else `camera` held inside the stop.
    pub fn camera(&self, doc: &Document, view: &View, camera: Camera) -> Option<Camera> {
        if let Some(f) = &self.flight {
            return Some(fit(f.area(), view.viewport, view.scale));
        }
        let area = self.stop(doc)?.area(view.viewport);
        Some(hold(camera, area, view.viewport, view.scale))
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

/// How far out of a stop's right edge its link handle stands — clear of
/// the ring that turns a selection, which a turned object brings beside
/// that edge — and how large it is drawn and hit, in logical px.
const HANDLE_OUT_PX: f64 = 28.0;
const HANDLE_PX: f64 = 7.0;
const HANDLE_HIT_PX: f64 = 11.0;
/// An arrow's line, its head and its bend, in logical px and as a share
/// of its length.
const ARROW_HALF_PX: f32 = 1.25;
const HEAD_PX: f64 = 10.0;
const HEAD_ANGLE: f64 = 0.5;
const BEND: f64 = 0.12;
const ARROW_STEPS: usize = 16;

/// Where the link handle of stop `id` stands on screen: out of the
/// middle of its box's right edge, the way a sequence reads.
pub fn handle_at(doc: &Document, view: &View, id: &str) -> Option<(f64, f64)> {
    let ink = stop(doc, id)?.ink;
    let (x, y) = view.world_to_screen(ink.x + ink.w, ink.y + ink.h / 2.0);
    Some((x + HANDLE_OUT_PX * view.scale, y))
}

/// Whether `screen` is on stop `id`'s link handle.
pub fn on_handle(doc: &Document, view: &View, id: &str, screen: (f64, f64)) -> bool {
    handle_at(doc, view, id).is_some_and(|(x, y)| (screen.0 - x).hypot(screen.1 - y) <= HANDLE_HIT_PX * view.scale)
}

/// Stop `id`'s link handle: a ring with a dot in it, in `ink` on
/// `ground`.
pub fn handle_prims(doc: &Document, view: &View, id: &str, ink: Rgba, ground: Rgba) -> Vec<Prim> {
    let Some((x, y)) = handle_at(doc, view, id) else {
        return Vec::new();
    };
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
    /// the same stops, one each way, do not lie on each other. None when
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

/// The arrow of the link out of layer `from` into layer `to`, when both
/// are stops on show: from the one's box to the other's, each where the
/// line between their middles crosses it. A stop and what it holds — a
/// frame and a group in it — have no room between them for an arrow, and
/// get none.
fn arrow_between(doc: &Document, view: &View, from: &str, to: &str) -> Option<Curve> {
    let from = stop(doc, from)?.ink;
    let to = stop(doc, to)?.ink;
    if from.holds(&to) || to.holds(&from) {
        return None;
    }
    let (a, b) = (screen_box(from, view), screen_box(to, view));
    let centre = |b: (f64, f64, f64, f64)| ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
    Curve::between(exit(a, centre(b)), exit(b, centre(a)), view)
}

/// How near an arrow a press has to land to take hold of it, in logical
/// px.
const ARROW_HIT_PX: f64 = 6.0;

/// Every deck on the board, its stops in the order it walks them: from
/// each head, in paint order, then from the first layer of each loop,
/// which has none. A layer passed over — hidden, or with nothing painted
/// — is no stop of it, as it is no slide; and a stop is in one deck.
pub fn decks(doc: &Document) -> Vec<Vec<String>> {
    let links = linked(doc);
    let led_to: HashSet<&str> = links.iter().map(|&(_, to)| to).collect();
    let leading: Vec<&str> = links.iter().map(|&(from, _)| from).collect();
    let heads = leading.iter().filter(|from| !led_to.contains(*from));
    let mut out: Vec<Vec<String>> = Vec::new();
    for start in heads.chain(leading.iter()) {
        let had = |id: &str| out.iter().flatten().any(|o| o == id);
        if had(start) {
            continue;
        }
        let deck: Vec<String> = sequence(doc, start).into_iter().filter(|id| !had(id)).collect();
        if !deck.is_empty() {
            out.push(deck);
        }
    }
    out
}

/// The place every stop of every deck goes by: from 1, in the order its
/// deck walks it.
pub fn numbers(doc: &Document) -> Vec<(String, usize)> {
    decks(doc)
        .into_iter()
        .flat_map(|deck| deck.into_iter().enumerate().map(|(i, id)| (id, i + 1)))
        .collect()
}

/// The stops of the deck layer `id` is in, walked from its head — or
/// round a loop from the stop the walk back up it came round to. None
/// for a layer in no deck.
pub fn deck_of(doc: &Document, id: &str) -> Vec<String> {
    if !in_deck(doc, id) {
        return Vec::new();
    }
    let all = layers(doc);
    let mut head = id.to_owned();
    let mut seen = vec![head.clone()];
    while let Some(before) = all.iter().find(|l| next_of(doc, &l.id) == Some(head.as_str())) {
        if seen.contains(&before.id) {
            break;
        }
        seen.push(before.id.clone());
        head = before.id.clone();
    }
    sequence(doc, &head)
}

/// Lays a link out of layer `from` to layer `to`, the stop after it, on
/// the terms the canvas lays one: two layers of the board, never one, and
/// a stop no other layer leads to already. A refusal says why, and
/// changes nothing.
pub fn link(doc: &mut Document, from: &str, to: &str) -> Result<(), String> {
    for id in [from, to] {
        if doc.layer(id).is_none() {
            return Err(format!("no layer {id:?} on the board that is open"));
        }
    }
    if from == to {
        return Err(format!("layer {from:?} cannot lead to itself"));
    }
    let before = layers(doc)
        .into_iter()
        .find(|l| l.id != from && next_of(doc, &l.id) == Some(to))
        .map(|l| l.id.clone());
    if let Some(before) = before {
        return Err(format!(
            "layer {to:?} already comes after {before:?}: a stop comes after one stop at most — unlink that one first"
        ));
    }
    if let Some(l) = doc.layer_mut(from) {
        l.next = Some(to.to_owned());
    }
    Ok(())
}

/// Takes away the links out of `ids`. Refused whole, and nothing changed,
/// when one of them names no layer.
pub fn unlink(doc: &mut Document, ids: &[String]) -> Result<(), String> {
    if let Some(missing) = ids.iter().find(|id| doc.layer(id).is_none()) {
        return Err(format!("no layer {missing:?} on the board that is open"));
    }
    for id in ids {
        if let Some(l) = doc.layer_mut(id) {
            l.next = None;
        }
    }
    Ok(())
}

/// Takes away every link on the board, and answers how many there were.
pub fn unlink_all(doc: &mut Document) -> usize {
    let linked: Vec<String> = layers(doc)
        .into_iter()
        .filter(|l| l.next.is_some())
        .map(|l| l.id.clone())
        .collect();
    for id in &linked {
        if let Some(l) = doc.layer_mut(id) {
            l.next = None;
        }
    }
    linked.len()
}

/// Lays `ids` as one deck, exactly as named: each leads to the one after
/// it and the last to nothing, and whatever led into any of them from
/// outside lets go — a stop taken out of the deck it was in leaves the
/// rest of that deck where it stands. Refused whole, and nothing changed,
/// for fewer than two, a layer named twice, or one the board does not
/// have.
pub fn lay_path(doc: &mut Document, ids: &[String]) -> Result<(), String> {
    if ids.len() < 2 {
        return Err("a path is two stops or more".into());
    }
    for (i, id) in ids.iter().enumerate() {
        if doc.layer(id).is_none() {
            return Err(format!("no layer {id:?} on the board that is open"));
        }
        if ids[..i].contains(id) {
            return Err(format!("layer {id:?} is named twice: a deck stops at each stop once"));
        }
    }
    let into: Vec<String> = layers(doc)
        .into_iter()
        .filter(|l| !ids.contains(&l.id) && next_of(doc, &l.id).is_some_and(|n| ids.iter().any(|i| i == n)))
        .map(|l| l.id.clone())
        .collect();
    for id in &into {
        if let Some(l) = doc.layer_mut(id) {
            l.next = None;
        }
    }
    for (i, id) in ids.iter().enumerate() {
        if let Some(l) = doc.layer_mut(id) {
            l.next = ids.get(i + 1).cloned();
        }
    }
    Ok(())
}

/// A badge's least radius, the room round its number, and how far clear
/// of its stop's box it stands, in logical px.
const BADGE_PX: f64 = 9.0;
const BADGE_PAD_PX: f64 = 4.0;
const BADGE_CLEAR_PX: f64 = 3.0;

/// A numbered disc over the top left corner of every stop of a deck, so
/// the order it walks reads at a glance — between a stop and what it
/// holds too, where no arrow is drawn: in `ink`, the number in `ground`,
/// its glyphs out of the chrome's `atlas` in texture `slot`. It stands
/// just above the box and in from its corner, clear of the handles a
/// selected stop wears there.
pub fn badges(doc: &Document, view: &View, atlas: &Atlas, slot: u32, ink: Rgba, ground: Rgba) -> Vec<Prim> {
    let s = view.scale as f32;
    let mut out = Vec::new();
    for (id, n) in numbers(doc) {
        let Some(at) = stop(doc, &id) else {
            continue;
        };
        let (x, y) = view.world_to_screen(at.ink.x, at.ink.y);
        let (x, y) = (x as f32, y as f32);
        let label = n.to_string();
        let w = atlas.measure(&label);
        let r = (BADGE_PX as f32 * s).max(w / 2.0 + BADGE_PAD_PX as f32 * s);
        let clear = BADGE_CLEAR_PX as f32 * s;
        let (x, y) = (x + r + clear, y - r - clear);
        out.push(Prim::circle(x, y, r, ink));
        let disc = ScreenRect {
            x: x - r,
            y: y - r,
            w: 2.0 * r,
            h: 2.0 * r,
        };
        let baseline = atlas.baseline_in(disc);
        for g in atlas.layout(&label, x - w / 2.0, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, ground));
        }
    }
    out
}

/// The links between stops on show, each an arrow from its stop's box to
/// the next one's, in `ink` — all but the one out of `held`, which is in
/// the pointer's hand.
pub fn links(doc: &Document, view: &View, ink: Rgba, held: Option<&str>) -> Vec<Prim> {
    linked(doc)
        .into_iter()
        .filter(|&(from, _)| Some(from) != held)
        .filter_map(|(from, to)| arrow_between(doc, view, from, to))
        .flat_map(|curve| curve.prims(view, with_alpha(ink, 0.85)))
        .collect()
}

/// The layer whose link's arrow passes within reach of `screen`: pressed
/// there, the link is taken hold of and can be pulled onto another stop
/// or off into nothing. The nearest, where two pass close.
pub fn link_at(doc: &Document, view: &View, screen: (f64, f64)) -> Option<String> {
    let reach = ARROW_HIT_PX * view.scale;
    linked(doc)
        .into_iter()
        .filter_map(|(from, to)| Some((arrow_between(doc, view, from, to)?.distance(screen), from)))
        .filter(|(d, _)| *d <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, from)| from.to_owned())
}

/// Whether layer `target` takes a link from layer `from`: it is another
/// layer, and none but `from` leads to it already. A stop comes after one
/// stop at most, so a deck stays a chain — two decks never merge into
/// one stop. A board that already holds two links into one layer still
/// opens; it is only that no new one is laid.
pub fn takes(doc: &Document, from: &str, target: &str) -> bool {
    target != from && !linked(doc).iter().any(|&(by, to)| by != from && to == target)
}

/// A link being pulled out of stop `from` to `to` (world): the arrow to
/// the pointer, and `target` — what it would land on — ringed when it
/// takes it.
pub fn pulling(doc: &Document, view: &View, from: &str, to: [f64; 2], target: Option<&str>, ink: Rgba) -> Vec<Prim> {
    let Some(source) = stop(doc, from) else {
        return Vec::new();
    };
    let point = view.world_to_screen(to[0], to[1]);
    let mut out = Vec::new();
    let ringed = target
        .filter(|t| takes(doc, from, t))
        .and_then(|t| stop(doc, t));
    if let Some(t) = ringed {
        let (x0, y0, x1, y1) = screen_box(t.ink, view);
        let s = view.scale as f32;
        let r = ScreenRect {
            x: x0 as f32 - 2.0 * s,
            y: y0 as f32 - 2.0 * s,
            w: (x1 - x0) as f32 + 4.0 * s,
            h: (y1 - y0) as f32 + 4.0 * s,
        };
        out.push(Prim::rect(r, ink).ring(2.0 * s));
    }
    let a = exit(screen_box(source.ink, view), point);
    if let Some(curve) = Curve::between(a, point, view) {
        out.extend(curve.prims(view, ink));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Element, Rect};

    const VP: Viewport = Viewport { w: 1000, h: 500 };

    fn view(camera: Camera) -> View {
        View {
            camera,
            viewport: VP,
            scale: 1.0,
        }
    }

    /// A board of frames on layers `ids`, side by side, each 200 by 100,
    /// none linked yet: frame `x` stands on layer `x`, as `x-f`, and holds
    /// a stack whose one layer is `x-in`.
    fn deck(ids: &[&str]) -> Document {
        let mut doc = Document::new("t");
        for (i, id) in ids.iter().enumerate() {
            doc.layers.push(Layer {
                id: (*id).into(),
                ..Layer::of(id, Kind::Frame)
            });
            doc.elements.push(Element::Frame(Frame {
                id: format!("{id}-f"),
                layer: (*id).into(),
                x: 300.0 * i as f64,
                y: 0.0,
                w: 200.0,
                h: 100.0,
                background: None,
                layers: vec![Layer {
                    id: format!("{id}-in"),
                    ..Layer::new("Layer 1")
                }],
            }));
        }
        doc
    }

    /// Ties `from` to `to` as a board on disk could, past every check
    /// [`link`] makes.
    fn tie(doc: &mut Document, from: &str, to: &str) {
        doc.layer_mut(from).unwrap().next = Some(to.into());
    }

    fn hide(doc: &mut Document, id: &str) {
        doc.layer_mut(id).unwrap().visible = false;
    }

    /// A rect `id` on a vector layer `layer` of its own, pushed onto the
    /// stack `owner` holds — the board's for none.
    fn rect(doc: &mut Document, owner: Option<&str>, layer: &str, id: &str, at: (f64, f64, f64, f64)) {
        doc.stack_mut(owner).unwrap().push(Layer {
            id: layer.into(),
            ..Layer::of(layer, Kind::Vector)
        });
        doc.elements.push(Element::Rect(Rect {
            id: id.into(),
            layer: layer.into(),
            x: at.0,
            y: at.1,
            w: at.2,
            h: at.3,
            rotation: 0.0,
            stroke: None,
            fill: Some("#123456".into()),
            text: None,
        }));
    }

    /// A group `id` on the stack `owner` holds, holding layers `inner`.
    fn group(doc: &mut Document, owner: Option<&str>, id: &str, inner: &[&str]) {
        let stack = doc.stack_mut(owner).unwrap();
        let held: Vec<Layer> = inner
            .iter()
            .map(|i| {
                let at = stack.iter().position(|l| l.id == *i).unwrap();
                stack.remove(at)
            })
            .collect();
        stack.push(Layer {
            id: id.into(),
            layers: held,
            ..Layer::of(id, Kind::Group)
        });
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    fn same(a: Area, b: Area) -> bool {
        close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h)
    }

    #[test]
    fn a_sequence_follows_the_links_wherever_the_stops_stand() {
        let mut doc = deck(&["a", "b", "c", "d"]);
        tie(&mut doc, "c", "a");
        tie(&mut doc, "a", "d");
        assert_eq!(sequence(&doc, "c"), ["c", "a", "d"]);
        assert_eq!(sequence(&doc, "b"), ["b"], "a frame linked to nothing is a deck of one");
    }

    #[test]
    fn a_loop_ends_where_it_would_repeat_and_a_link_to_itself_goes_nowhere() {
        let mut doc = deck(&["a", "b", "c"]);
        tie(&mut doc, "a", "b");
        tie(&mut doc, "b", "c");
        tie(&mut doc, "c", "a");
        assert_eq!(sequence(&doc, "b"), ["b", "c", "a"]);
        let mut doc = deck(&["a"]);
        tie(&mut doc, "a", "a");
        assert_eq!(sequence(&doc, "a"), ["a"]);
        assert!(next_of(&doc, "a").is_none());
    }

    #[test]
    fn a_link_to_a_layer_that_has_gone_goes_nowhere() {
        let mut doc = deck(&["a", "b"]);
        tie(&mut doc, "a", "gone");
        assert_eq!(sequence(&doc, "a"), ["a"]);
        assert!(next_of(&doc, "a").is_none());
    }

    #[test]
    fn a_hidden_stop_is_passed_through_and_not_shown() {
        let mut doc = deck(&["a", "b", "c"]);
        tie(&mut doc, "a", "b");
        tie(&mut doc, "b", "c");
        hide(&mut doc, "b");
        assert_eq!(sequence(&doc, "a"), ["a", "c"]);
    }

    #[test]
    fn any_layer_is_a_stop_a_frame_a_group_or_the_layer_of_one_object() {
        // Frame a, then a group in it, then one object in the group, then
        // a rect out on the board, then frame b: zooming in and out.
        let mut doc = deck(&["a", "b"]);
        rect(&mut doc, Some("a"), "r1", "r1-el", (20.0, 20.0, 40.0, 20.0));
        rect(&mut doc, Some("a"), "r2", "r2-el", (100.0, 50.0, 40.0, 30.0));
        group(&mut doc, Some("a"), "g", &["r1", "r2"]);
        rect(&mut doc, None, "loose", "loose-el", (0.0, 500.0, 50.0, 50.0));
        tie(&mut doc, "a", "g");
        tie(&mut doc, "g", "r2");
        tie(&mut doc, "r2", "loose");
        tie(&mut doc, "loose", "b");
        assert_eq!(sequence(&doc, "a"), ["a", "g", "r2", "loose", "b"]);
        let g = stop(&doc, "g").unwrap();
        assert!(!g.slide);
        assert!(same(g.ink, Area { x: 20.0, y: 20.0, w: 120.0, h: 60.0 }), "{:?}", g.ink);
        assert!(same(stop(&doc, "r2").unwrap().ink, Area { x: 100.0, y: 50.0, w: 40.0, h: 30.0 }));
    }

    #[test]
    fn a_layer_with_nothing_painted_is_no_stop_and_is_passed_through() {
        let mut doc = deck(&["a", "b"]);
        // `a-in`, the frame's own first layer, holds nothing.
        tie(&mut doc, "a", "a-in");
        tie(&mut doc, "a-in", "b");
        assert!(stop(&doc, "a-in").is_none());
        assert_eq!(sequence(&doc, "a"), ["a", "b"]);
        rect(&mut doc, None, "r", "r-el", (0.0, 300.0, 10.0, 10.0));
        hide(&mut doc, "r");
        assert!(stop(&doc, "r").is_none(), "a hidden one");
    }

    #[test]
    fn a_frame_is_a_slide_its_own_box_whatever_the_window() {
        let doc = deck(&["a"]);
        let s = stop(&doc, "a").unwrap();
        let frame = Area { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        assert!(s.slide && same(s.ink, frame));
        assert!(same(s.area(VP), frame));
        assert!(same(s.area(Viewport { w: 300, h: 900 }), frame));
        assert_eq!(s.veil(), 1.0);
    }

    #[test]
    fn an_object_is_shown_on_the_board_with_room_round_it_in_the_windows_shape() {
        let mut doc = deck(&[]);
        rect(&mut doc, None, "r", "r-el", (10.0, 10.0, 100.0, 50.0));
        let s = stop(&doc, "r").unwrap();
        assert_eq!(s.veil(), 0.0, "nothing round it is covered");
        // Room of 8 on every side makes it 116 by 66; a window twice as
        // wide as it is tall makes that 132 by 66, about the same middle.
        let area = s.area(VP);
        assert!(same(area, Area { x: -6.0, y: 2.0, w: 132.0, h: 66.0 }), "{area:?}");
        let tall = s.area(Viewport { w: 500, h: 1000 });
        assert!(same(tall, Area { x: 2.0, y: -81.0, w: 116.0, h: 232.0 }), "{tall:?}");
    }

    #[test]
    fn a_stop_in_a_frame_is_shown_inside_it_and_covered_round_as_its_slide_is() {
        // A 16:9 frame, and a rect low in it: with its room, in the
        // window's shape, the rect's area would run past the frame's foot.
        let mut doc = Document::new("t");
        doc.layers.push(Layer {
            id: "a".into(),
            ..Layer::of("a", Kind::Frame)
        });
        doc.elements.push(Element::Frame(Frame {
            id: "a-f".into(),
            layer: "a".into(),
            x: 0.0,
            y: 0.0,
            w: 1600.0,
            h: 900.0,
            background: None,
            layers: vec![Layer {
                id: "a-in".into(),
                ..Layer::new("Layer 1")
            }],
        }));
        rect(&mut doc, Some("a"), "r", "r-el", (600.0, 760.0, 400.0, 120.0));
        let s = stop(&doc, "r").unwrap();
        let frame = Area {
            x: 0.0,
            y: 0.0,
            w: 1600.0,
            h: 900.0,
        };
        assert_eq!(s.within, Some(frame));
        assert_eq!(s.veil(), 1.0, "it is part of its slide");
        let wide = Viewport { w: 1600, h: 900 };
        let area = s.area(wide);
        assert!(frame.holds(&area), "inside its frame: {area:?}");
        assert!(area.holds(&s.ink), "with all of it in sight: {area:?}");
        assert!(close(area.y + area.h, 900.0), "moved up only as far as it had to: {area:?}");
        // A rect on the board stands in no frame.
        rect(&mut doc, None, "out", "out-el", (0.0, 2000.0, 10.0, 10.0));
        assert_eq!(stop(&doc, "out").unwrap().within, None);
        assert_eq!(stop(&doc, "out").unwrap().veil(), 0.0);
    }

    #[test]
    fn what_is_shown_of_a_stop_in_a_frame_is_covered_past_the_frame() {
        let mut doc = deck(&["a"]);
        rect(&mut doc, Some("a"), "r", "r-el", (10.0, 10.0, 40.0, 20.0));
        tie(&mut doc, "a", "r");
        let v = view(fit(stop(&doc, "a").unwrap().ink, VP, 1.0));
        let mut show = Show::start(&doc, None, &v).unwrap();
        show.tick(10.0);
        assert!(show.step(&doc, &v, true));
        show.tick(10.0);
        let on = view(show.camera(&doc, &v, v.camera).unwrap());
        let (area, veil) = show.shown(&doc, &on).unwrap();
        assert_eq!(veil, 1.0);
        let frame = stop(&doc, "a").unwrap().ink;
        assert!(frame.holds(&area), "nothing past the frame is left uncovered: {area:?}");
    }

    #[test]
    fn what_a_frame_cuts_away_is_no_part_of_a_stop_and_a_turned_object_is_boxed_round_its_corners() {
        let mut doc = deck(&["a"]);
        // Half of it out past the frame's right edge, at 200.
        rect(&mut doc, Some("a"), "r", "r-el", (150.0, 10.0, 100.0, 20.0));
        assert!(same(stop(&doc, "r").unwrap().ink, Area { x: 150.0, y: 10.0, w: 50.0, h: 20.0 }));
        let mut doc = deck(&[]);
        rect(&mut doc, None, "t", "t-el", (0.0, 0.0, 100.0, 100.0));
        if let Element::Rect(r) = &mut doc.elements[0] {
            r.rotation = 45.0;
        }
        let ink = stop(&doc, "t").unwrap().ink;
        let reach = 50.0 * 2.0_f64.sqrt();
        assert!(same(ink, Area { x: 50.0 - reach, y: 50.0 - reach, w: 2.0 * reach, h: 2.0 * reach }), "{ink:?}");
    }

    #[test]
    fn a_show_starts_at_the_stop_selected_then_at_the_head_of_a_deck() {
        let mut doc = deck(&["a", "b", "c"]);
        tie(&mut doc, "c", "b");
        tie(&mut doc, "b", "a");
        assert_eq!(first(&doc, Some("b")).as_deref(), Some("b"));
        assert_eq!(first(&doc, Some("not-a-layer")).as_deref(), Some("c"), "the head");
        assert_eq!(first(&doc, None).as_deref(), Some("c"));
    }

    #[test]
    fn a_deck_with_nothing_on_show_keeps_no_other_from_being_presented() {
        let mut doc = deck(&["a", "b", "c", "d"]);
        tie(&mut doc, "a", "b");
        tie(&mut doc, "c", "d");
        hide(&mut doc, "a");
        hide(&mut doc, "b");
        assert_eq!(first(&doc, None).as_deref(), Some("c"), "the first deck with a stop on show");
        assert!(presentable(&doc, None));
        // A loop with nothing on show, and a frame alone that is.
        let mut doc = deck(&["a", "b", "x"]);
        tie(&mut doc, "a", "b");
        tie(&mut doc, "b", "a");
        hide(&mut doc, "a");
        hide(&mut doc, "b");
        assert_eq!(first(&doc, None).as_deref(), Some("x"));
    }

    #[test]
    fn an_object_selected_in_no_deck_starts_no_show_of_its_own() {
        let mut doc = deck(&["a", "b"]);
        tie(&mut doc, "a", "b");
        rect(&mut doc, None, "r", "r-el", (0.0, 300.0, 10.0, 10.0));
        assert_eq!(first(&doc, Some("r")).as_deref(), Some("a"), "the head of the deck");
        tie(&mut doc, "b", "r");
        assert_eq!(first(&doc, Some("r")).as_deref(), Some("r"), "once it is in one");
    }

    #[test]
    fn a_deck_that_loops_starts_at_its_first_layer_and_a_board_without_links_at_its_first_frame() {
        let mut doc = deck(&["a", "b", "c"]);
        tie(&mut doc, "b", "c");
        tie(&mut doc, "c", "b");
        assert_eq!(first(&doc, None).as_deref(), Some("b"), "the first that leads anywhere");
        assert_eq!(first(&deck(&["x", "y"]), None).as_deref(), Some("x"));
        assert_eq!(first(&Document::new("t"), None), None, "nothing to present");
        assert!(!presentable(&Document::new("t"), None));
        let mut hidden = deck(&["x"]);
        hide(&mut hidden, "x");
        assert!(!presentable(&hidden, None), "a hidden frame is no slide");
        assert!(presentable(&deck(&["x"]), None));
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

    /// A flight from `from` to `to`, both uncovered, in a window of `VP`'s
    /// shape, and the areas it shows at `n + 1` evenly spaced moments.
    fn flown(from: Area, to: Area, n: usize) -> (Flight, Vec<Area>) {
        let mut f = Flight::new((from, 0.0), (to, 0.0), aspect(VP));
        let length = f.length;
        let areas = (0..=n)
            .map(|i| {
                f.t = length * i as f64 / n as f64;
                f.area()
            })
            .collect();
        f.t = 0.0;
        (f, areas)
    }

    /// The width of the world a window of `VP`'s shape shows to have all
    /// of `a` in it.
    fn width(a: Area) -> f64 {
        a.w.max(a.h * aspect(VP))
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
            h: 300.0,
        };
        let (f, areas) = flown(from, to, 100);
        assert!(same(areas[0], from) && same(areas[100], to));
        // The path's own ends are the two views, not only the clamps.
        let ((x, y), w) = f.path.at(f.path.s);
        assert!(close(x, 3200.0) && close(y, 550.0) && close(w, width(to)), "({x}, {y}) {w}");
        // Every area on the way fills the window across or down.
        for a in &areas {
            let filled = (a.w / width(*a)).max(a.h * aspect(VP) / width(*a));
            assert!(close(filled, 1.0), "{a:?}");
        }
    }

    #[test]
    fn a_flight_into_what_is_in_sight_goes_straight_in_and_keeps_it_in_sight() {
        // From a wide view to a small box near its bottom right corner.
        let from = Area {
            x: 0.0,
            y: 0.0,
            w: 2000.0,
            h: 1000.0,
        };
        let to = Area {
            x: 1800.0,
            y: 880.0,
            w: 100.0,
            h: 50.0,
        };
        let (_, areas) = flown(from, to, 200);
        for pair in areas.windows(2) {
            assert!(width(pair[1]) <= width(pair[0]) * (1.0 + 1e-9), "never out: {pair:?}");
        }
        for a in &areas {
            let seen = Area {
                x: a.center().0 - width(*a) / 2.0,
                y: a.center().1 - width(*a) / aspect(VP) / 2.0,
                w: width(*a),
                h: width(*a) / aspect(VP),
            };
            assert!(seen.holds(&to) || seen.meet(&to).is_some(), "lost sight of it at {a:?}");
        }
    }

    #[test]
    fn a_flight_far_opens_out_on_the_way_further_the_further_it_goes() {
        let a = Area {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let near = Area { x: 300.0, ..a };
        let far = Area { x: 3000.0, ..a };
        let widest = |to| {
            let (_, areas) = flown(a, to, 100);
            areas.into_iter().map(width).fold(0.0, f64::max)
        };
        assert!(widest(near) > width(a), "even next door: {}", widest(near));
        assert!(widest(far) > widest(near));
        let (here, _) = flown(a, a, 1);
        assert_eq!(here.path.s, 0.0, "nowhere to go");
        let (n, _) = flown(a, near, 1);
        let (f, _) = flown(a, far, 1);
        assert!(f.length > n.length);
        assert!(f.length <= FLIGHT_MAX_S && n.length >= FLIGHT_MIN_S);
    }

    #[test]
    fn a_zoom_with_nowhere_to_go_only_zooms() {
        let a = Area {
            x: -500.0,
            y: -250.0,
            w: 1000.0,
            h: 500.0,
        };
        let small = Area {
            x: -5.0,
            y: -2.5,
            w: 10.0,
            h: 5.0,
        };
        let (_, areas) = flown(a, small, 50);
        for area in &areas {
            assert!(close(area.center().0, 0.0) && close(area.center().1, 0.0), "{area:?}");
        }
        assert!(same(areas[50], small));
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
        tie(&mut doc, "a", "b");
        tie(&mut doc, "b", "c");
        doc
    }

    #[test]
    fn a_show_flies_in_and_lands_on_its_first_stop() {
        let doc = deck_of_three();
        let v = view(Camera {
            x: 5000.0,
            y: 5000.0,
            zoom: 1.0,
        });
        let mut show = Show::start(&doc, None, &v).unwrap();
        assert_eq!(show.place(), (0, 3));
        assert_eq!(show.before, v.camera);
        assert!(show.flying());
        let leaving = show.camera(&doc, &v, v.camera).unwrap();
        assert!(close(leaving.x, 5000.0) && close(leaving.zoom, 1.0), "it leaves from the view");
        show.tick(10.0);
        assert!(!show.flying());
        let slide = stop(&doc, "a").unwrap().ink;
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
        let mut show = Show::start(&doc, None, &v).unwrap();
        show.tick(10.0);
        assert!(!show.step(&doc, &v, false), "nothing before the first");
        assert!(show.step(&doc, &v, true));
        assert_eq!(show.slide(), "b");
        assert!(show.flying());
        assert!(show.step(&doc, &v, true));
        assert!(!show.step(&doc, &v, true), "nothing after the last");
        assert_eq!(show.place(), (2, 3));
        assert!(show.go(&doc, &v, 0));
        assert_eq!(show.slide(), "a");
        assert!(!show.go(&doc, &v, 7));
    }

    #[test]
    fn a_stop_hidden_mid_show_is_stepped_over_and_the_one_on_show_ends_it() {
        let mut doc = deck_of_three();
        let v = view(Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        });
        let mut show = Show::start(&doc, None, &v).unwrap();
        show.tick(10.0);
        hide(&mut doc, "b");
        assert!(show.step(&doc, &v, true));
        assert_eq!(show.slide(), "c", "b is passed over");
        show.tick(10.0);
        hide(&mut doc, "c");
        assert!(show.camera(&doc, &v, v.camera).is_none(), "nothing left to show");
    }

    #[test]
    fn a_step_mid_flight_leaves_from_where_the_camera_is() {
        let doc = deck_of_three();
        let v = view(fit(stop(&doc, "a").unwrap().ink, VP, 1.0));
        let mut show = Show::start(&doc, None, &v).unwrap();
        show.tick(10.0);
        show.step(&doc, &v, true);
        show.tick(0.3);
        let midway = show.shown(&doc, &v).unwrap();
        show.step(&doc, &v, true);
        let leaving = show.shown(&doc, &v).unwrap();
        assert!(same(leaving.0, midway.0) && close(leaving.1, midway.1), "no jump");
    }

    #[test]
    fn what_is_shown_of_a_slide_is_the_part_of_it_in_the_window_covered_round() {
        let doc = deck_of_three();
        let slide = stop(&doc, "a").unwrap().ink;
        let mut show = Show::start(&doc, None, &view(fit(slide, VP, 1.0))).unwrap();
        show.tick(10.0);
        let at_fit = view(fit(slide, VP, 1.0));
        let (area, veil) = show.shown(&doc, &at_fit).unwrap();
        assert!(same(area, slide) && veil == 1.0, "the whole slide");
        let zoomed = view(Camera {
            x: 50.0,
            y: 50.0,
            zoom: 20.0,
        });
        let (part, _) = show.shown(&doc, &zoomed).unwrap();
        assert!(close(part.w, 50.0) && close(part.h, 25.0), "{part:?}");
    }

    #[test]
    fn the_veil_comes_and_goes_with_a_flight_between_a_slide_and_a_stop_on_the_board() {
        let mut doc = deck(&["a"]);
        rect(&mut doc, None, "r", "r-el", (0.0, 400.0, 100.0, 50.0));
        tie(&mut doc, "a", "r");
        let v = view(fit(stop(&doc, "a").unwrap().ink, VP, 1.0));
        let mut show = Show::start(&doc, None, &v).unwrap();
        assert!(close(show.shown(&doc, &v).unwrap().1, 0.0), "nothing covered before the show");
        show.tick(10.0);
        assert_eq!(show.shown(&doc, &v).unwrap().1, 1.0, "a slide is covered round");
        assert!(show.step(&doc, &v, true));
        show.tick(show.flight.unwrap().length / 2.0);
        let midway = show.shown(&doc, &v).unwrap().1;
        assert!(midway > 0.0 && midway < 1.0, "{midway}");
        show.tick(10.0);
        let on_board = view(show.camera(&doc, &v, v.camera).unwrap());
        let (area, veil) = show.shown(&doc, &on_board).unwrap();
        assert_eq!(veil, 0.0);
        assert!(same(area, Area::seen(&on_board)), "the board in sight round it");
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
        let mut doc = deck(&["a"]);
        let v = view(Camera {
            x: 100.0,
            y: 50.0,
            zoom: 1.0,
        });
        let (x, y) = handle_at(&doc, &v, "a").unwrap();
        assert_eq!((x, y), (600.0 + HANDLE_OUT_PX, 250.0));
        assert!(on_handle(&doc, &v, "a", (x + 5.0, y - 5.0)));
        assert!(!on_handle(&doc, &v, "a", (590.0, 250.0)), "inside the frame is the frame's");
        // An object's stands off its own box.
        rect(&mut doc, Some("a"), "r", "r-el", (20.0, 20.0, 60.0, 40.0));
        assert_eq!(handle_at(&doc, &v, "r"), Some((480.0 + HANDLE_OUT_PX, 240.0)));
        assert_eq!(handle_at(&doc, &v, "a-in"), None, "nothing painted, no handle");
    }

    #[test]
    fn an_arrow_runs_from_one_stops_edge_to_the_others() {
        let mut doc = deck(&["a", "b"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let ink = [1.0, 0.0, 0.0, 1.0];
        assert!(links(&doc, &v, ink, None).is_empty(), "nothing linked, nothing drawn");
        tie(&mut doc, "a", "b");
        let prims = links(&doc, &v, ink, None);
        assert!(links(&doc, &v, ink, Some("a")).is_empty(), "the link in the hand is not drawn twice");
        assert!(!prims.is_empty());
        // a ends at x = 200 and b begins at 300: in this view 450 and 550.
        let first = prims[0].geom;
        let last_shaft = prims[ARROW_STEPS - 1].geom;
        assert!((first[0] - 450.0).abs() < 0.01, "from a's edge: {first:?}");
        assert!((last_shaft[2] - 550.0).abs() < 0.01, "to b's edge: {last_shaft:?}");
        hide(&mut doc, "b");
        assert!(links(&doc, &v, ink, None).is_empty(), "no arrow into a hidden stop");
    }

    #[test]
    fn no_arrow_runs_between_a_stop_and_what_it_holds() {
        let mut doc = deck(&["a", "b"]);
        rect(&mut doc, Some("a"), "r", "r-el", (20.0, 20.0, 60.0, 40.0));
        tie(&mut doc, "a", "r");
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let ink = [1.0, 0.0, 0.0, 1.0];
        assert!(links(&doc, &v, ink, None).is_empty(), "the frame and a rect in it");
        tie(&mut doc, "r", "b");
        assert!(!links(&doc, &v, ink, None).is_empty(), "the rect and the next frame");
    }

    #[test]
    fn every_stop_of_a_deck_is_numbered_by_its_place_in_it() {
        let mut doc = deck(&["a", "b", "c", "d", "e"]);
        assert!(numbers(&doc).is_empty(), "no deck, no numbers");
        tie(&mut doc, "c", "a");
        tie(&mut doc, "a", "d");
        // A second deck, a loop with no head.
        tie(&mut doc, "b", "e");
        tie(&mut doc, "e", "b");
        let mut got = numbers(&doc);
        got.sort();
        let want: Vec<(String, usize)> = [("a", 2), ("b", 1), ("c", 1), ("d", 3), ("e", 2)]
            .into_iter()
            .map(|(id, n)| (id.to_owned(), n))
            .collect();
        assert_eq!(got, want);
        hide(&mut doc, "a");
        assert!(
            numbers(&doc).iter().all(|(id, n)| id != "a" && (id != "d" || *n == 2)),
            "a stop passed over takes no place: {:?}",
            numbers(&doc)
        );
    }

    #[test]
    fn a_link_is_laid_where_a_stop_takes_it_and_refused_with_the_reason_where_not() {
        let mut doc = deck(&["a", "b", "c"]);
        assert_eq!(link(&mut doc, "a", "b"), Ok(()));
        assert_eq!(next_of(&doc, "a"), Some("b"));
        let refused = link(&mut doc, "c", "b").unwrap_err();
        assert!(refused.contains("\"a\""), "it names the layer that leads there: {refused}");
        assert!(link(&mut doc, "a", "a").is_err(), "never itself");
        assert!(link(&mut doc, "a", "nowhere").unwrap_err().contains("nowhere"));
        assert!(link(&mut doc, "nobody", "a").is_err());
        assert_eq!(link(&mut doc, "a", "c"), Ok(()), "a link laid again elsewhere moves");
        assert_eq!(next_of(&doc, "a"), Some("c"));
    }

    #[test]
    fn unlinking_takes_away_the_links_out_of_the_layers_named_or_every_one() {
        let mut doc = deck(&["a", "b", "c"]);
        link(&mut doc, "a", "b").unwrap();
        link(&mut doc, "b", "c").unwrap();
        assert_eq!(unlink(&mut doc, &["b".to_owned()]), Ok(()));
        assert_eq!((next_of(&doc, "a"), next_of(&doc, "b")), (Some("b"), None));
        assert!(unlink(&mut doc, &["ghost".to_owned()]).is_err());
        link(&mut doc, "b", "c").unwrap();
        assert_eq!(unlink_all(&mut doc), 2);
        assert!(numbers(&doc).is_empty());
    }

    #[test]
    fn a_path_is_laid_exactly_and_takes_its_stops_out_of_any_deck_they_were_in() {
        let mut doc = deck(&["a", "b", "c", "d", "e"]);
        // A deck that runs e -> b -> d, and a link out of c.
        link(&mut doc, "e", "b").unwrap();
        link(&mut doc, "b", "d").unwrap();
        link(&mut doc, "c", "a").unwrap();
        let ids: Vec<String> = ["c", "b", "a"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(lay_path(&mut doc, &ids), Ok(()));
        assert_eq!(sequence(&doc, "c"), ["c", "b", "a"], "exactly that");
        assert_eq!(next_of(&doc, "e"), None, "b is taken out of the deck it was in");
        assert_eq!(next_of(&doc, "a"), None, "and the path ends where it says");
        assert_eq!(first(&doc, None).as_deref(), Some("c"));
        let twice: Vec<String> = ["a", "b", "a"].iter().map(|s| (*s).to_owned()).collect();
        assert!(lay_path(&mut doc, &twice).is_err(), "a stop once");
        assert!(lay_path(&mut doc, &["a".to_owned()]).is_err(), "a path is two stops or more");
        let stray: Vec<String> = ["a", "zz"].iter().map(|s| (*s).to_owned()).collect();
        assert!(lay_path(&mut doc, &stray).is_err());
        assert_eq!(sequence(&doc, "c"), ["c", "b", "a"], "a refusal changes nothing");
    }

    #[test]
    fn the_deck_a_layer_is_in_is_walked_from_its_head() {
        let mut doc = deck(&["a", "b", "c", "d"]);
        link(&mut doc, "c", "a").unwrap();
        link(&mut doc, "a", "d").unwrap();
        assert_eq!(deck_of(&doc, "d"), ["c", "a", "d"]);
        assert_eq!(deck_of(&doc, "c"), ["c", "a", "d"]);
        assert_eq!(deck_of(&doc, "b"), Vec::<String>::new(), "in no deck");
        link(&mut doc, "d", "c").unwrap();
        assert_eq!(deck_of(&doc, "a").len(), 3, "a loop is walked once round");
        assert_eq!(decks(&doc), [vec!["a".to_owned(), "d".to_owned(), "c".to_owned()]]);
        let mut two = deck(&["a", "b", "c", "d"]);
        link(&mut two, "a", "b").unwrap();
        link(&mut two, "d", "c").unwrap();
        assert_eq!(decks(&two), [vec!["a".to_owned(), "b".to_owned()], vec!["d".to_owned(), "c".to_owned()]]);
    }

    #[test]
    fn a_badge_stands_on_the_top_left_corner_of_every_stop_numbered() {
        let atlas = crate::text::Atlas::build(&crate::text::Font::bundled(), 13);
        let mut doc = deck(&["a", "b"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let (ink, ground) = ([1.0, 0.0, 0.0, 1.0], [1.0; 4]);
        assert!(badges(&doc, &v, &atlas, 7, ink, ground).is_empty());
        tie(&mut doc, "a", "b");
        let prims = badges(&doc, &v, &atlas, 7, ink, ground);
        // a's corner is at (0, 0) of the world: (250, 200) on screen; b's
        // at (300, 0): (550, 200). Each badge stands just above it and in
        // from it, by its radius and a little more.
        let discs: Vec<&Prim> = prims.iter().filter(|p| p.color == ink).collect();
        assert_eq!(discs.len(), 2);
        let middles: Vec<(f32, f32)> = discs.iter().map(|p| (p.geom[0] + p.geom[2] / 2.0, p.geom[1] + p.geom[3] / 2.0)).collect();
        let off = BADGE_PX as f32 + BADGE_CLEAR_PX as f32;
        assert_eq!(middles, [(250.0 + off, 200.0 - off), (550.0 + off, 200.0 - off)]);
        let digits = prims.iter().filter(|p| p.slot == 7).count();
        assert_eq!(digits, 2, "a 1 and a 2");
    }

    /// What every frame of the screen and every move of the pointer pays:
    /// the arrows of a board of many layers, only two of them linked. A
    /// walk of the whole board for every layer was a tenth of a second a
    /// frame on a real board of 265 layers; this one is a few walks.
    #[test]
    fn the_arrows_of_a_board_of_many_layers_cost_a_few_walks_of_it_not_one_a_layer() {
        let mut doc = deck(&[]);
        for i in 0..200 {
            rect(&mut doc, None, &format!("l{i}"), &format!("e{i}"), (20.0 * f64::from(i), 0.0, 10.0, 10.0));
        }
        tie(&mut doc, "l0", "l199");
        let v = view(Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        });
        let began = std::time::Instant::now();
        for _ in 0..5 {
            assert!(!links(&doc, &v, [1.0; 4], None).is_empty());
            let _ = link_at(&doc, &v, (500.0, 250.0));
            assert_eq!(numbers(&doc).len(), 2);
        }
        let took = began.elapsed();
        assert!(took.as_millis() < 400, "five frames' arrows and numbers took {took:?}");
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
        tie(&mut doc, "a", "b");
        let curve = arrow_between(&doc, &v, "a", "b").unwrap();
        let middle = curve.at(0.5);
        assert!(middle.1 < 250.0, "bent: {middle:?}");
        assert_eq!(link_at(&doc, &v, middle).as_deref(), Some("a"));
        assert_eq!(link_at(&doc, &v, (middle.0, middle.1 + 5.0)).as_deref(), Some("a"));
        assert!(link_at(&doc, &v, (middle.0, middle.1 + 30.0)).is_none(), "too far off it");
    }

    #[test]
    fn a_layer_takes_one_link_in_and_never_its_own() {
        let mut doc = deck(&["a", "b", "c"]);
        tie(&mut doc, "a", "b");
        assert!(!takes(&doc, "c", "b"), "a already leads to b");
        assert!(takes(&doc, "a", "b"), "a's own link, laid again");
        assert!(takes(&doc, "c", "a"), "nothing leads to a");
        assert!(takes(&doc, "b", "a"), "a loop back to the head is one link in");
        assert!(!takes(&doc, "a", "a"), "never itself");
        let mut gone = deck(&["a", "b"]);
        tie(&mut gone, "a", "a");
        assert!(takes(&gone, "b", "a"), "a link to itself leads nowhere");
    }

    #[test]
    fn pulling_rings_the_stop_it_would_land_on_when_it_takes_it() {
        let mut doc = deck(&["a", "b", "c"]);
        let v = view(Camera {
            x: 250.0,
            y: 50.0,
            zoom: 1.0,
        });
        let ink = [1.0, 0.0, 0.0, 1.0];
        let ringed = |doc: &Document, target| pulling(doc, &v, "a", [350.0, 50.0], target, ink).iter().any(|p| p.line > 0.0);
        assert!(ringed(&doc, Some("b")), "over b");
        assert!(!ringed(&doc, Some("a")), "over itself");
        assert!(!ringed(&doc, None), "over nothing");
        tie(&mut doc, "c", "b");
        assert!(!ringed(&doc, Some("b")), "b has its one link in");
    }
}
