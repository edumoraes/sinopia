//! The Shape tool's figures as geometry, in a shape's own frame: its box's
//! centre at the origin, its axes unturned, y down. Where a model's corners
//! stand, and how far a point is from its edge — negative inside. Pure:
//! `scene` hands the shader the numbers that place a polygon's corners,
//! and `select` asks here what the pointer is over. The shader works the
//! same distances out again, line for line, so the two cannot come to
//! disagree about where an edge is.

use crate::curve::point_segment_distance;
use crate::doc::{Head, Line, MAX_SIDES, Model, Shape};
use crate::geom::Point;

/// The most corners the shader walks: a star of the most points a figure
/// may have, each with a corner cut in beside it.
pub const MAX_CORNERS: u32 = 2 * MAX_SIDES;

/// What the Shape tool draws: one of the six closed models, a line, or an
/// arrow — a line with a head at its end. The bar lists them in this
/// order, and the tool's key steps through them in it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Figure {
    #[default]
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Polygon,
    Star,
    Line,
    Arrow,
}

impl Figure {
    pub const ALL: [Figure; 8] = [
        Figure::Rectangle,
        Figure::Ellipse,
        Figure::Triangle,
        Figure::Diamond,
        Figure::Polygon,
        Figure::Star,
        Figure::Line,
        Figure::Arrow,
    ];

    /// The figure a closed model is drawn by.
    pub fn of(model: Model) -> Figure {
        match model {
            Model::Rectangle => Figure::Rectangle,
            Model::Ellipse => Figure::Ellipse,
            Model::Triangle => Figure::Triangle,
            Model::Diamond => Figure::Diamond,
            Model::Polygon => Figure::Polygon,
            Model::Star => Figure::Star,
        }
    }

    /// The figure a line is: an arrow while it wears a head at either
    /// end, a line otherwise.
    pub fn of_line(l: &Line) -> Figure {
        if l.start == Head::None && l.end == Head::None {
            Figure::Line
        } else {
            Figure::Arrow
        }
    }

    /// The closed model it is, or none for a line.
    pub fn model(self) -> Option<Model> {
        match self {
            Figure::Rectangle => Some(Model::Rectangle),
            Figure::Ellipse => Some(Model::Ellipse),
            Figure::Triangle => Some(Model::Triangle),
            Figure::Diamond => Some(Model::Diamond),
            Figure::Polygon => Some(Model::Polygon),
            Figure::Star => Some(Model::Star),
            Figure::Line | Figure::Arrow => None,
        }
    }

    /// What it is called: the word its layer is named after.
    pub fn name(self) -> &'static str {
        match self {
            Figure::Line => "Line",
            Figure::Arrow => "Arrow",
            _ => self.model().map_or("Shape", Model::name),
        }
    }
}

/// Where a polygon model's corners stand. Every one of them is walked
/// from straight up, clockwise, evenly round a circle of radius 1 — every
/// other one on a circle of `inner` for a star — and then fitted to the
/// box: stretched so the widest reach either way across and the lowest
/// corner touch its sides, the top one being the first, at -1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corners {
    /// A polygon's sides, a star's points.
    pub sides: u32,
    /// How far in every other corner is: 1 for a polygon, whose corners
    /// all stand on the one circle.
    pub inner: f64,
    /// How far the corners reach either side of the centre, on the unit
    /// circle.
    pub reach: f64,
    /// How far below the centre the lowest corner stands, on the unit
    /// circle.
    pub drop: f64,
}

impl Corners {
    /// The corners `model` walks with `sides` and `inner` as a shape says
    /// them — a triangle's three and a diamond's four whatever `sides`
    /// says — or none, for a model that is not a polygon.
    pub fn of(model: Model, sides: u32, inner: f64) -> Option<Corners> {
        let (sides, inner) = match model {
            Model::Rectangle | Model::Ellipse => return None,
            Model::Triangle => (3, 1.0),
            Model::Diamond => (4, 1.0),
            Model::Polygon => (sides, 1.0),
            Model::Star => (sides, inner),
        };
        let mut c = Corners {
            sides,
            inner,
            reach: 0.0,
            drop: 0.0,
        };
        for k in 0..c.count() {
            let [x, y] = c.unit(k);
            c.reach = c.reach.max(x.abs());
            c.drop = c.drop.max(y);
        }
        Some(c)
    }

    /// How many corners there are: a star's cut-in ones between its
    /// points.
    pub fn count(&self) -> u32 {
        if self.inner < 1.0 { 2 * self.sides } else { self.sides }
    }

    /// Corner `k` on the unit circle, before it is fitted to a box.
    pub fn unit(&self, k: u32) -> Point {
        let n = f64::from(self.count());
        let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * f64::from(k) / n;
        let r = if self.inner < 1.0 && k % 2 == 1 { self.inner } else { 1.0 };
        [r * a.cos(), r * a.sin()]
    }

    /// Every corner, fitted to a box of half extents `half`.
    pub fn fitted(&self, half: Point) -> Vec<Point> {
        (0..self.count())
            .map(|k| {
                let [x, y] = self.unit(k);
                [
                    x / self.reach * half[0],
                    ((y + 1.0) / (self.drop + 1.0) * 2.0 - 1.0) * half[1],
                ]
            })
            .collect()
    }
}

/// The signed distance from `p`, in the shape's own frame, to its edge —
/// the model mirrored top to bottom when the shape is flipped.
pub fn distance(s: &Shape, p: Point) -> f64 {
    let half = [s.w / 2.0, s.h / 2.0];
    let p = if s.flip { [p[0], -p[1]] } else { p };
    match Corners::of(s.model, s.sides, s.inner) {
        Some(c) => polygon(p, &c.fitted(half)),
        None if s.model == Model::Ellipse => ellipse(p, half),
        None => rounded_box(p, half, s.radius),
    }
}

/// The signed distance from `p` to a box of half extents `half` whose
/// corners are rounded by `radius` — never past the shorter half, where
/// the box is a capsule already.
pub fn rounded_box(p: Point, half: Point, radius: f64) -> f64 {
    let r = radius.clamp(0.0, half[0].min(half[1]).max(0.0));
    let q = [p[0].abs() - half[0] + r, p[1].abs() - half[1] + r];
    let outside = q[0].max(0.0).hypot(q[1].max(0.0));
    outside + q[0].max(q[1]).min(0.0) - r
}

/// How many steps of Newton's method find the nearest point of an
/// ellipse. Measured against the nearest of 200 000 points along it, over
/// a grid round ellipses from a circle to one 166 times as wide as it is
/// tall: five steps miss by nearly a hundredth of its size, eight by
/// five hundred-thousandths, and ten by a ten-millionth, where it stays.
const ELLIPSE_STEPS: usize = 10;

/// The signed distance from `p` to the ellipse of half extents `half`:
/// the nearest point along it is found by Newton's method on its angle,
/// started on whichever axis is nearer and kept in the quarter `p` is in,
/// which is what lets a circle be no special case at all.
pub fn ellipse(p: Point, half: Point) -> f64 {
    // The ellipse is its own mirror image either way, so its nearest point
    // to `p` is in the quarter `p` is in.
    let p = [p[0].abs(), p[1].abs()];
    let ab = [half[0].max(TINY), half[1].max(TINY)];
    let q = [ab[0] * (p[0] - ab[0]), ab[1] * (p[1] - ab[1])];
    let mut t: f64 = if q[0] < q[1] { std::f64::consts::FRAC_PI_2 } else { 0.0 };
    for _ in 0..ELLIPSE_STEPS {
        let (sin, cos) = t.sin_cos();
        let u = [ab[0] * cos, ab[1] * sin];
        let v = [-ab[0] * sin, ab[1] * cos];
        let to = [p[0] - u[0], p[1] - u[1]];
        let bend = to[0] * u[0] + to[1] * u[1] + v[0] * v[0] + v[1] * v[1];
        if bend.abs() <= TINY {
            break;
        }
        t = (t + (to[0] * v[0] + to[1] * v[1]) / bend).clamp(0.0, std::f64::consts::FRAC_PI_2);
    }
    let (sin, cos) = t.sin_cos();
    let d = (p[0] - ab[0] * cos).hypot(p[1] - ab[1] * sin);
    if (p[0] / ab[0]).powi(2) + (p[1] / ab[1]).powi(2) > 1.0 { d } else { -d }
}

/// Small enough to be nothing, in world units.
const TINY: f64 = 1e-9;

/// How far a blob's outline swells out and sinks in, as a share of its
/// radius: enough to read as alive, never enough to look torn.
pub const BLOB_DEPTH: f64 = 0.07;

/// The waves a blob's outline is made of: how many lobes each has, how
/// much of the depth it takes — the shares add up to the whole — and how
/// fast it turns as the phase moves on, some one way and some the other,
/// so the shape never quite repeats. The shader is written these numbers
/// from here.
pub const BLOB_WAVES: [(f64, f64, f64); 3] = [(2.0, 0.55, 0.9), (3.0, 0.3, -1.3), (5.0, 0.15, 1.7)];

/// How far a blob's outline stands out at angle `a`, as a share of its
/// radius, at `phase`. Only the shader draws a blob, so this and
/// [`blob`] are its mirror, here to be held to it by the tests.
#[cfg(test)]
fn blob_reach(a: f64, phase: f64) -> f64 {
    1.0 + BLOB_DEPTH
        * BLOB_WAVES
            .iter()
            .map(|(lobes, share, turn)| share * (lobes * a + turn * phase).sin())
            .sum::<f64>()
}

/// The signed distance from `p` to the blob in a box of half extents
/// `half` at `phase` — the presenter's camera in its irregular shape. Its
/// radius is the box's smaller half, held in so that the furthest a wave
/// can reach is the box's edge. The distance is the one to the outline
/// along the ray from the middle, over how steeply that changes: exact on
/// a circle, and within a fiftieth of the radius of the true one near
/// the gentle swells a blob has — which is where an edge is antialiased.
#[cfg(test)]
fn blob(p: Point, half: Point, phase: f64) -> f64 {
    let radius = half[0].min(half[1]) / (1.0 + BLOB_DEPTH);
    let len = p[0].hypot(p[1]);
    let a = p[1].atan2(p[0]);
    let steep = radius
        * BLOB_DEPTH
        * BLOB_WAVES
            .iter()
            .map(|(lobes, share, turn)| share * lobes * (lobes * a + turn * phase).cos())
            .sum::<f64>();
    (len - radius * blob_reach(a, phase)) / (1.0 + (steep / len.max(TINY)).powi(2)).sqrt()
}

/// The signed distance from `p` to the polygon `corners`: the nearest of
/// its edges, and inside where the edges crossing the line through `p`
/// wind round it — Inigo Quilez's `sdPolygon`, which the shader walks too.
pub fn polygon(p: Point, corners: &[Point]) -> f64 {
    let Some(&last) = corners.last() else {
        return f64::MAX;
    };
    let mut nearest = f64::MAX;
    let mut sign = 1.0;
    let mut prev = last;
    for &v in corners {
        let e = [prev[0] - v[0], prev[1] - v[1]];
        let w = [p[0] - v[0], p[1] - v[1]];
        let along = ((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]).max(TINY)).clamp(0.0, 1.0);
        let b = [w[0] - e[0] * along, w[1] - e[1] * along];
        nearest = nearest.min(b[0] * b[0] + b[1] * b[1]);
        let c = [p[1] >= v[1], p[1] < prev[1], e[0] * w[1] > e[1] * w[0]];
        if c.iter().all(|&b| b) || c.iter().all(|&b| !b) {
            sign = -sign;
        }
        prev = v;
    }
    sign * nearest.sqrt()
}

/// How long a line's head is, in world units: this much, and this much
/// again for every unit the line is wide, as Figma sizes its caps by the
/// stroke they end.
pub const HEAD_BASE: f64 = 10.0;
pub const HEAD_PER_WIDTH: f64 = 3.0;
/// How far a head opens either side of its line.
pub const HEAD_ANGLE: f64 = std::f64::consts::FRAC_PI_6;

/// How long a head is on a line `width` wide and `length` long: never past
/// half the line, so a short arrow with a head at either end still shows
/// the shaft between them.
pub fn head_length(width: f64, length: f64) -> f64 {
    (HEAD_BASE + HEAD_PER_WIDTH * width).min(length / 2.0)
}

/// A line as it is drawn: the strokes along it — the shaft, then the two
/// arms of every open head — each as wide as the line and capped round,
/// and the triangles of its filled heads, each its point first.
#[derive(Debug, Clone, PartialEq)]
pub struct LineParts {
    pub strokes: Vec<[Point; 2]>,
    pub heads: Vec<[Point; 3]>,
}

/// What `l` is drawn as. A filled head stands where the line ends and the
/// shaft stops at its base, so no round cap shows past the point; a line
/// of no length is a dot, with no way for a head to face.
pub fn line_parts(l: &Line) -> LineParts {
    let d = [l.to[0] - l.from[0], l.to[1] - l.from[1]];
    let length = d[0].hypot(d[1]);
    if length <= TINY {
        return LineParts {
            strokes: vec![[l.from, l.to]],
            heads: Vec::new(),
        };
    }
    let u = [d[0] / length, d[1] / length];
    let len = head_length(l.width, length);
    let mut shaft = [l.from, l.to];
    let mut arms = Vec::new();
    let mut heads = Vec::new();
    // Each end with the way out of the line there.
    for (end, tip, out, head) in [(0, l.from, [-u[0], -u[1]], l.start), (1, l.to, u, l.end)] {
        let base = [tip[0] - out[0] * len, tip[1] - out[1] * len];
        match head {
            Head::None => {}
            Head::Arrow => {
                for side in [1.0, -1.0] {
                    let (sin, cos) = (side * HEAD_ANGLE).sin_cos();
                    let back = [-out[0], -out[1]];
                    let arm = [back[0] * cos - back[1] * sin, back[0] * sin + back[1] * cos];
                    arms.push([tip, [tip[0] + arm[0] * len, tip[1] + arm[1] * len]]);
                }
            }
            Head::Triangle => {
                let half = len * HEAD_ANGLE.tan();
                let across = [-out[1], out[0]];
                heads.push([
                    tip,
                    [base[0] + across[0] * half, base[1] + across[1] * half],
                    [base[0] - across[0] * half, base[1] - across[1] * half],
                ]);
                shaft[end] = base;
            }
        }
    }
    let mut strokes = vec![shaft];
    strokes.extend(arms);
    LineParts { strokes, heads }
}

/// How far `l`'s ink reaches either side of it: half its width, or as far
/// as its widest head opens.
pub fn line_reach(l: &Line) -> f64 {
    let length = (l.to[0] - l.from[0]).hypot(l.to[1] - l.from[1]);
    let len = head_length(l.width, length);
    let half = l.width / 2.0;
    [l.start, l.end]
        .iter()
        .map(|head| match head {
            Head::None => half,
            Head::Arrow => len * HEAD_ANGLE.sin() + half,
            Head::Triangle => len * HEAD_ANGLE.tan(),
        })
        .fold(half, f64::max)
}

/// The signed distance from `p` to `l`'s ink — its strokes as wide as the
/// line, and its filled heads — negative on it.
pub fn line_distance(l: &Line, p: Point) -> f64 {
    let parts = line_parts(l);
    let strokes = parts
        .strokes
        .iter()
        .map(|[a, b]| point_segment_distance(p, *a, *b) - l.width / 2.0);
    let heads = parts.heads.iter().map(|tri| polygon(p, tri));
    strokes.chain(heads).fold(f64::MAX, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{DEFAULT_INNER, Model};

    const EPS: f64 = 1e-9;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn a_triangle_stands_on_its_base_with_its_apex_up() {
        let c = Corners::of(Model::Triangle, 7, 0.5).expect("a triangle has corners");
        assert_eq!((c.sides, c.count()), (3, 3), "a triangle has three, whatever `sides` says");
        let box_ = c.fitted([50.0, 40.0]);
        assert!(close(box_[0][0], 0.0, EPS) && close(box_[0][1], -40.0, EPS), "{box_:?}");
        assert!(close(box_[1][0], 50.0, EPS) && close(box_[1][1], 40.0, EPS), "{box_:?}");
        assert!(close(box_[2][0], -50.0, EPS) && close(box_[2][1], 40.0, EPS), "{box_:?}");
    }

    #[test]
    fn a_diamond_touches_the_middle_of_every_side() {
        let c = Corners::of(Model::Diamond, 7, 0.5).unwrap();
        let at = c.fitted([30.0, 20.0]);
        let want = [[0.0, -20.0], [30.0, 0.0], [0.0, 20.0], [-30.0, 0.0]];
        for (got, want) in at.iter().zip(want) {
            assert!(close(got[0], want[0], EPS) && close(got[1], want[1], EPS), "{at:?}");
        }
    }

    #[test]
    fn every_polygon_and_star_is_fitted_to_its_whole_box() {
        for model in [Model::Triangle, Model::Diamond, Model::Polygon, Model::Star] {
            for sides in [3, 4, 5, 6, 7, 12, 60] {
                let c = Corners::of(model, sides, DEFAULT_INNER).unwrap();
                let at = c.fitted([40.0, 25.0]);
                let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
                for p in &at {
                    lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                    hi = [hi[0].max(p[0]), hi[1].max(p[1])];
                }
                let what = format!("{model:?} of {sides}: {lo:?} {hi:?}");
                assert!(close(lo[0], -40.0, 1e-9) && close(hi[0], 40.0, 1e-9), "{what}");
                assert!(close(lo[1], -25.0, 1e-9) && close(hi[1], 25.0, 1e-9), "{what}");
            }
        }
    }

    #[test]
    fn a_star_has_twice_its_points_every_other_one_cut_in() {
        let c = Corners::of(Model::Star, 5, 0.4).unwrap();
        assert_eq!(c.count(), 10);
        let outer = c.unit(0);
        let inner = c.unit(1);
        assert!(close(outer[0].hypot(outer[1]), 1.0, EPS));
        assert!(close(inner[0].hypot(inner[1]), 0.4, EPS));
        assert!(close(outer[1], -1.0, EPS), "the first point is straight up");
        // A polygon is the same walk with no corner cut in.
        let p = Corners::of(Model::Polygon, 5, 0.4).unwrap();
        assert_eq!((p.count(), p.inner), (5, 1.0));
    }

    #[test]
    fn a_rectangle_and_an_ellipse_have_no_corners_to_walk() {
        assert!(Corners::of(Model::Rectangle, 5, 0.5).is_none());
        assert!(Corners::of(Model::Ellipse, 5, 0.5).is_none());
    }

    #[test]
    fn a_rounded_box_is_as_far_as_its_edge_and_its_corners_are_round() {
        let half = [50.0, 30.0];
        assert!(close(rounded_box([0.0, 0.0], half, 0.0), -30.0, EPS));
        assert!(close(rounded_box([60.0, 0.0], half, 0.0), 10.0, EPS));
        assert!(close(rounded_box([53.0, 34.0], half, 0.0), 5.0, EPS), "past a corner, to it");
        // Rounded by 10: the corner is a quarter circle about (40, 20).
        let d = rounded_box([50.0, 30.0], half, 10.0);
        assert!(close(d, 10.0 * std::f64::consts::SQRT_2 - 10.0, 1e-9), "{d}");
        // A radius past the shorter half is the shorter half: a capsule.
        let capsule = rounded_box([0.0, 30.0], half, 500.0);
        assert!(close(capsule, 0.0, 1e-9), "{capsule}");
        assert!(close(rounded_box([50.0, 0.0], half, 500.0), 0.0, 1e-9));
    }

    /// The distance to an ellipse the slow way: the nearest of a few
    /// thousand points along it, then narrowed down between the points
    /// either side of that one until the angle stops moving.
    fn brute_ellipse(p: Point, half: Point) -> f64 {
        let at = |t: f64| (p[0] - half[0] * t.cos()).hypot(p[1] - half[1] * t.sin());
        let n = 4000;
        let step = std::f64::consts::TAU / f64::from(n);
        let best = (0..n)
            .map(|k| step * f64::from(k))
            .min_by(|a, b| at(*a).total_cmp(&at(*b)))
            .unwrap();
        let (mut lo, mut hi) = (best - step, best + step);
        for _ in 0..200 {
            let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if at(a) < at(b) { hi = b } else { lo = a }
        }
        let d = at((lo + hi) / 2.0);
        let inside = (p[0] / half[0]).powi(2) + (p[1] / half[1]).powi(2) < 1.0;
        if inside { -d } else { d }
    }

    #[test]
    fn the_distance_to_an_ellipse_is_the_distance_to_its_nearest_point() {
        for half in [[50.0_f64, 50.0], [80.0, 40.0], [30.0, 90.0], [200.0, 10.0], [7.0, 140.0]] {
            let span = half[0].max(half[1]);
            for i in -8..=8 {
                for j in -8..=8 {
                    let p = [half[0] * f64::from(i) / 4.0, half[1] * f64::from(j) / 4.0];
                    let want = brute_ellipse(p, half);
                    let got = ellipse(p, half);
                    assert!(
                        close(got, want, span * 1e-4),
                        "{half:?} at {p:?}: {got} against {want}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_centre_of_a_circle_is_its_radius_in() {
        let d = ellipse([0.0, 0.0], [25.0, 25.0]);
        assert!(close(d, -25.0, 1e-6), "{d}");
        let d = ellipse([0.0, 0.0], [40.0, 10.0]);
        assert!(close(d, -10.0, 1e-6), "the nearest edge is the near side: {d}");
    }

    /// The outline of the blob in a box of half extents `half` at `phase`,
    /// as a polygon of `n` corners: what the distance is held to.
    fn blob_outline(half: Point, phase: f64, n: usize) -> Vec<Point> {
        let radius = half[0].min(half[1]) / (1.0 + BLOB_DEPTH);
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                let r = radius * blob_reach(a, phase);
                [r * a.cos(), r * a.sin()]
            })
            .collect()
    }

    #[test]
    fn a_blob_stays_inside_its_box_and_moves_with_its_phase() {
        let half = [100.0, 80.0];
        for phase in [0.0, 0.7, 3.1, 10.0] {
            for [x, y] in blob_outline(half, phase, 720) {
                assert!(x.hypot(y) <= 80.0 + 1e-9, "past the box at phase {phase}: ({x}, {y})");
            }
        }
        assert!(blob([0.0, 0.0], half, 0.0) < 0.0, "the middle is inside");
        assert!(blob([100.0, 80.0], half, 0.0) > 0.0, "the box's corner is outside");
        let at = |phase| blob_reach(0.3, phase);
        assert!((at(0.0) - at(1.0)).abs() > 1e-3, "the outline moves as the phase does");
        assert!((at(0.0) - at(1e-6)).abs() < 1e-5, "and moves smoothly");
    }

    #[test]
    fn a_blobs_distance_is_close_to_the_true_one_near_its_outline() {
        let half = [120.0, 120.0];
        let radius = 120.0 / (1.0 + BLOB_DEPTH);
        for phase in [0.0, 2.0] {
            let outline = blob_outline(half, phase, 2000);
            for i in 0..72 {
                let a = std::f64::consts::TAU * i as f64 / 72.0;
                for by in [-0.08, -0.02, 0.02, 0.08] {
                    let r = radius * (blob_reach(a, phase) + by);
                    let p = [r * a.cos(), r * a.sin()];
                    let truth = polygon(p, &outline);
                    let got = blob(p, half, phase);
                    assert!(
                        (got - truth).abs() <= 0.02 * radius,
                        "at {a:.2} rad, {by} out: {got} for {truth}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_ellipse_with_no_height_is_no_trouble() {
        let d = ellipse([3.0, 4.0], [10.0, 0.0]);
        assert!(d.is_finite() && d > 0.0, "{d}");
    }

    #[test]
    fn the_distance_to_a_polygon_is_to_its_nearest_edge_and_signed() {
        let square = [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]];
        assert!(close(polygon([0.0, 0.0], &square), -10.0, EPS));
        assert!(close(polygon([0.0, 13.0], &square), 3.0, EPS));
        assert!(close(polygon([13.0, 14.0], &square), 5.0, EPS));
        assert!(close(polygon([9.0, 0.0], &square), -1.0, EPS));
        // A star is not convex: between two points is outside.
        let c = Corners::of(Model::Star, 5, 0.3).unwrap();
        let star = c.fitted([50.0, 50.0]);
        let between = [0.0, 30.0];
        assert!(polygon(between, &star) > 0.0, "the notch at the bottom is outside");
        assert!(polygon([0.0, -40.0], &star) < 0.0, "the top point is inside");
    }

    #[test]
    fn a_polygon_with_no_area_is_no_trouble() {
        let flat = [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]];
        let d = polygon([3.0, 4.0], &flat);
        assert!(close(d, 5.0, EPS), "{d}");
    }

    #[test]
    fn a_shapes_distance_is_its_models() {
        let mut s = crate::doc::Shape {
            id: "s".into(),
            layer: "l".into(),
            model: Model::Rectangle,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 60.0,
            rotation: 0.0,
            flip: false,
            fill: None,
            stroke: Some("#000".into()),
            width: 2.0,
            radius: 0.0,
            sides: 5,
            inner: DEFAULT_INNER,
        };
        let corner = [49.0, 29.0];
        assert!(distance(&s, corner) < 0.0, "a rectangle's corner is inside it");
        s.model = Model::Ellipse;
        assert!(distance(&s, corner) > 0.0, "and past an ellipse's edge");
        s.model = Model::Diamond;
        assert!(close(distance(&s, [0.0, -30.0]), 0.0, 1e-9), "a diamond's top is on its edge");
        s.model = Model::Triangle;
        assert!(distance(&s, [-45.0, -25.0]) > 0.0, "beside a triangle's apex is outside");
        assert!(distance(&s, [0.0, 20.0]) < 0.0);
    }

    fn a_line(from: Point, to: Point, start: Head, end: Head) -> Line {
        Line {
            id: "l".into(),
            layer: "v".into(),
            from,
            to,
            stroke: "#000000".into(),
            width: 2.0,
            start,
            end,
        }
    }

    #[test]
    fn a_bare_line_is_one_stroke_from_end_to_end() {
        let parts = line_parts(&a_line([0.0, 0.0], [100.0, 0.0], Head::None, Head::None));
        assert_eq!(parts.strokes, vec![[[0.0, 0.0], [100.0, 0.0]]]);
        assert!(parts.heads.is_empty());
    }

    #[test]
    fn an_open_head_is_two_strokes_back_from_its_end() {
        let l = a_line([0.0, 0.0], [100.0, 0.0], Head::None, Head::Arrow);
        let parts = line_parts(&l);
        let len = head_length(l.width, 100.0);
        assert_eq!(len, HEAD_BASE + HEAD_PER_WIDTH * 2.0);
        assert_eq!(parts.strokes.len(), 3, "the shaft to the tip, and two arms");
        assert_eq!(parts.strokes[0], [[0.0, 0.0], [100.0, 0.0]]);
        for arm in &parts.strokes[1..] {
            assert_eq!(arm[0], [100.0, 0.0], "from the tip");
            let back = [arm[1][0] - 100.0, arm[1][1]];
            assert!(close(back[0].hypot(back[1]), len, 1e-9), "{arm:?}");
            let opens = back[1].abs().atan2(-back[0]);
            assert!(close(opens, HEAD_ANGLE, 1e-9), "{opens}");
        }
        assert!(parts.strokes[1][1][1] * parts.strokes[2][1][1] < 0.0, "one each side");
    }

    #[test]
    fn a_filled_head_is_a_triangle_and_the_shaft_stops_at_its_base() {
        let l = a_line([0.0, 0.0], [0.0, 100.0], Head::Triangle, Head::None);
        let parts = line_parts(&l);
        let len = head_length(l.width, 100.0);
        assert_eq!(parts.heads.len(), 1);
        let [tip, a, b] = parts.heads[0];
        assert_eq!(tip, [0.0, 0.0], "its point at the end");
        assert!(close(a[1], len, 1e-9) && close(b[1], len, 1e-9), "its base across the line");
        assert!(close(a[0], -b[0], 1e-9));
        assert_eq!(parts.strokes, vec![[[0.0, len], [0.0, 100.0]]], "no cap past the point");
    }

    #[test]
    fn a_head_is_never_longer_than_half_its_line() {
        assert_eq!(head_length(2.0, 20.0), 10.0);
        assert_eq!(head_length(2.0, 1000.0), HEAD_BASE + HEAD_PER_WIDTH * 2.0);
    }

    #[test]
    fn a_line_of_no_length_is_a_dot_with_no_heads() {
        let parts = line_parts(&a_line([5.0, 5.0], [5.0, 5.0], Head::Arrow, Head::Triangle));
        assert_eq!(parts.strokes, vec![[[5.0, 5.0], [5.0, 5.0]]]);
        assert!(parts.heads.is_empty());
    }

    #[test]
    fn a_lines_ink_reaches_as_wide_as_its_widest_head() {
        let bare = a_line([0.0, 0.0], [100.0, 0.0], Head::None, Head::None);
        assert_eq!(line_reach(&bare), 1.0);
        let len = head_length(2.0, 100.0);
        let open = Line { end: Head::Arrow, ..bare.clone() };
        assert!(close(line_reach(&open), len * HEAD_ANGLE.sin() + 1.0, 1e-9));
        let filled = Line { start: Head::Triangle, ..bare };
        assert!(close(line_reach(&filled), len * HEAD_ANGLE.tan(), 1e-9));
    }

    #[test]
    fn the_distance_to_a_line_is_to_its_ink_heads_and_all() {
        let l = a_line([0.0, 0.0], [100.0, 0.0], Head::None, Head::Triangle);
        assert!(close(line_distance(&l, [50.0, 5.0]), 4.0, 1e-9), "5 off, 1 of ink");
        assert!(line_distance(&l, [95.0, 2.0]) < 0.0, "inside the head");
        assert!(line_distance(&l, [95.0, 30.0]) > 0.0);
    }
}
