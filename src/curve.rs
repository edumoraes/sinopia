//! Freehand geometry: polyline simplification, cubic Bézier fitting and
//! flattening. Pure and unit-free — callers pick the tolerance scale.

pub type Point = [f64; 2];

/// Cubic Bézier `[a, c1, c2, b]`: endpoints `a`/`b`, handles `c1`/`c2`.
pub type Cubic = [Point; 4];

/// Ramer–Douglas–Peucker: drops every point closer than `tolerance` to
/// the chord of its neighbours. Endpoints always survive.
pub fn simplify(points: &[Point], tolerance: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0, points.len() - 1)];
    while let Some((first, last)) = spans.pop() {
        let mut worst = (0.0, first);
        for i in first + 1..last {
            let d = point_segment_distance(points[i], points[first], points[last]);
            if d > worst.0 {
                worst = (d, i);
            }
        }
        let (dist, split) = worst;
        if dist > tolerance {
            keep[split] = true;
            spans.push((first, split));
            spans.push((split, last));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Point on `c` at parameter `t` in `[0, 1]`.
pub fn eval(c: &Cubic, t: f64) -> Point {
    let s = 1.0 - t;
    let w = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    let mut p = [0.0, 0.0];
    for (q, wi) in c.iter().zip(w) {
        p = add(p, scale(*q, wi));
    }
    p
}

/// Schneider's least-squares fit (Graphics Gems, 1990): a chain of cubics
/// through `points` whose deviation never exceeds `max_error`, splitting
/// at the worst point when one cubic cannot do it.
pub fn fit(points: &[Point], max_error: f64) -> Vec<Cubic> {
    let mut d: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if d.last() != Some(p) {
            d.push(*p);
        }
    }
    match d.len() {
        0 => return Vec::new(),
        1 => return vec![[d[0]; 4]],
        _ => {}
    }
    let mut out = Vec::new();
    let t_hat1 = normalize(sub(d[1], d[0]));
    let t_hat2 = normalize(sub(d[d.len() - 2], d[d.len() - 1]));
    fit_cubic(&d, t_hat1, t_hat2, max_error * max_error, &mut out);
    out
}

/// Newton–Raphson passes before giving up and splitting.
const MAX_ITERATIONS: usize = 4;

/// Fits `d` (no repeats, at least two points) with unit tangents `t_hat1`
/// leaving the first point and `t_hat2` leaving the last, both pointing
/// into the curve. `max_error_sq` is the squared tolerance.
fn fit_cubic(d: &[Point], t_hat1: Point, t_hat2: Point, max_error_sq: f64, out: &mut Vec<Cubic>) {
    let (first, last) = (d[0], d[d.len() - 1]);
    if d.len() == 2 {
        let third = len(sub(last, first)) / 3.0;
        out.push([
            first,
            add(first, scale(t_hat1, third)),
            add(last, scale(t_hat2, third)),
            last,
        ]);
        return;
    }

    let mut u = chord_length_parameterize(d);
    let mut bez = generate_bezier(d, &u, t_hat1, t_hat2);
    let (mut err, mut split) = max_deviation(d, &bez, &u);
    if err <= max_error_sq {
        out.push(bez);
        return;
    }
    // Reparameterize while it keeps helping, unless the first try is
    // hopeless (more than four times the tolerance off).
    if err <= max_error_sq * 16.0 {
        for _ in 0..MAX_ITERATIONS {
            let u2 = reparameterize(d, &u, &bez);
            if u2.windows(2).any(|w| w[1] < w[0]) {
                break;
            }
            let bez2 = generate_bezier(d, &u2, t_hat1, t_hat2);
            let (err2, split2) = max_deviation(d, &bez2, &u2);
            if err2 >= err {
                break;
            }
            (u, bez, err, split) = (u2, bez2, err2, split2);
            if err <= max_error_sq {
                out.push(bez);
                return;
            }
        }
    }

    // Split at the worst point; the joint tangent is the average of the
    // two chords meeting there (or just the incoming one on a hairpin).
    let mut t_center = normalize(sub(d[split - 1], d[split + 1]));
    if t_center == [0.0, 0.0] {
        t_center = normalize(sub(d[split - 1], d[split]));
    }
    fit_cubic(&d[..=split], t_hat1, t_center, max_error_sq, out);
    fit_cubic(&d[split..], neg(t_center), t_hat2, max_error_sq, out);
}

/// Parameter of each point: cumulative chord length, scaled to `[0, 1]`.
fn chord_length_parameterize(d: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(d.len());
    u.push(0.0);
    for w in d.windows(2) {
        u.push(u[u.len() - 1] + len(sub(w[1], w[0])));
    }
    let total = u[u.len() - 1];
    if total > 0.0 {
        for v in &mut u {
            *v /= total;
        }
    }
    u
}

/// Bernstein weights of a cubic at `t`.
fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// Least-squares handle lengths along the given tangents for the points
/// at parameters `u`; falls back to a third of the chord when the system
/// is degenerate or the solution overshoots.
fn generate_bezier(d: &[Point], u: &[f64], t_hat1: Point, t_hat2: Point) -> Cubic {
    let (first, last) = (d[0], d[d.len() - 1]);
    let mut c = [[0.0; 2]; 2];
    let mut x = [0.0; 2];
    for (p, &ui) in d.iter().zip(u) {
        let [b0, b1, b2, b3] = bernstein(ui);
        let a0 = scale(t_hat1, b1);
        let a1 = scale(t_hat2, b2);
        c[0][0] += dot(a0, a0);
        c[0][1] += dot(a0, a1);
        c[1][1] += dot(a1, a1);
        let tmp = sub(*p, add(scale(first, b0 + b1), scale(last, b2 + b3)));
        x[0] += dot(a0, tmp);
        x[1] += dot(a1, tmp);
    }
    c[1][0] = c[0][1];

    let det_c0_c1 = c[0][0] * c[1][1] - c[1][0] * c[0][1];
    let (mut alpha_l, mut alpha_r) = if det_c0_c1.abs() > 1e-12 {
        let det_c0_x = c[0][0] * x[1] - c[1][0] * x[0];
        let det_x_c1 = x[0] * c[1][1] - x[1] * c[0][1];
        (det_x_c1 / det_c0_c1, det_c0_x / det_c0_c1)
    } else {
        // Under-determined: assume equal handle lengths.
        let c0 = c[0][0] + c[0][1];
        let c1 = c[1][0] + c[1][1];
        let a = if c0.abs() > 1e-12 {
            x[0] / c0
        } else if c1.abs() > 1e-12 {
            x[1] / c1
        } else {
            0.0
        };
        (a, a)
    };

    let seg_len = len(sub(last, first));
    let epsilon = 1e-6 * seg_len;
    let line = sub(last, first);
    let overshoots =
        dot(scale(t_hat1, alpha_l), line) - dot(scale(t_hat2, alpha_r), line) > seg_len * seg_len;
    if alpha_l < epsilon || alpha_r < epsilon || overshoots {
        alpha_l = seg_len / 3.0;
        alpha_r = seg_len / 3.0;
    }
    [
        first,
        add(first, scale(t_hat1, alpha_l)),
        add(last, scale(t_hat2, alpha_r)),
        last,
    ]
}

/// Largest squared distance between an interior point and the curve at
/// its parameter, and where it happens.
fn max_deviation(d: &[Point], bez: &Cubic, u: &[f64]) -> (f64, usize) {
    let mut worst = (0.0, d.len() / 2);
    for i in 1..d.len() - 1 {
        let diff = sub(eval(bez, u[i]), d[i]);
        let dist_sq = dot(diff, diff);
        if dist_sq >= worst.0 {
            worst = (dist_sq, i);
        }
    }
    worst
}

/// One Newton–Raphson step per point towards the parameter of its
/// closest point on `bez`.
fn reparameterize(d: &[Point], u: &[f64], bez: &Cubic) -> Vec<f64> {
    d.iter()
        .zip(u)
        .map(|(p, &ui)| newton_raphson_root_find(bez, *p, ui))
        .collect()
}

fn newton_raphson_root_find(q: &Cubic, p: Point, u: f64) -> f64 {
    let q1 = [
        scale(sub(q[1], q[0]), 3.0),
        scale(sub(q[2], q[1]), 3.0),
        scale(sub(q[3], q[2]), 3.0),
    ];
    let q2 = [scale(sub(q1[1], q1[0]), 2.0), scale(sub(q1[2], q1[1]), 2.0)];
    let s = 1.0 - u;
    let q1_u = add(
        add(scale(q1[0], s * s), scale(q1[1], 2.0 * s * u)),
        scale(q1[2], u * u),
    );
    let q2_u = add(scale(q2[0], s), scale(q2[1], u));
    let diff = sub(eval(q, u), p);
    let numerator = dot(diff, q1_u);
    let denominator = dot(q1_u, q1_u) + dot(diff, q2_u);
    if denominator.abs() < 1e-12 {
        return u;
    }
    (u - numerator / denominator).clamp(0.0, 1.0)
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(v: Point, k: f64) -> Point {
    [v[0] * k, v[1] * k]
}

fn neg(v: Point) -> Point {
    [-v[0], -v[1]]
}

/// Unit vector, or zero when `v` is (nearly) zero.
fn normalize(v: Point) -> Point {
    let l = len(v);
    if l > 1e-12 {
        scale(v, 1.0 / l)
    } else {
        [0.0, 0.0]
    }
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(v: Point) -> f64 {
    dot(v, v).sqrt()
}

/// Distance from `p` to the segment `ab`; the distance to `a` when the
/// segment is degenerate.
fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = sub(b, a);
    let ap = sub(p, a);
    let ab_len_sq = dot(ab, ab);
    let t = if ab_len_sq > 0.0 {
        (dot(ap, ab) / ab_len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    len(sub(ap, [ab[0] * t, ab[1] * t]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_collapses_a_collinear_run_to_its_endpoints() {
        let pts = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]];
        assert_eq!(simplify(&pts, 0.1), vec![[0.0, 0.0], [3.0, 0.0]]);
    }

    #[test]
    fn simplify_keeps_a_corner_beyond_tolerance() {
        let pts = [[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]];
        assert_eq!(simplify(&pts, 0.5), pts.to_vec());
    }

    #[test]
    fn simplify_tolerance_decides_whether_a_bump_survives() {
        let pts = [[0.0, 0.0], [1.0, 0.1], [2.0, 0.0], [3.0, 0.0]];
        assert_eq!(simplify(&pts, 0.5), vec![[0.0, 0.0], [3.0, 0.0]]);
        // Once the bump is kept, [2, 0] sits 0.0499 off the new chord
        // (1, 0.1)→(3, 0) and goes.
        assert_eq!(
            simplify(&pts, 0.05),
            vec![[0.0, 0.0], [1.0, 0.1], [3.0, 0.0]]
        );
    }

    #[test]
    fn simplify_leaves_short_inputs_alone() {
        assert!(simplify(&[], 1.0).is_empty());
        assert_eq!(simplify(&[[1.0, 1.0]], 1.0), vec![[1.0, 1.0]]);
        assert_eq!(
            simplify(&[[1.0, 1.0], [2.0, 2.0]], 1.0),
            vec![[1.0, 1.0], [2.0, 2.0]]
        );
    }

    fn dist(a: Point, b: Point) -> f64 {
        len(sub(a, b))
    }

    /// Distance from `p` to the chain, by dense sampling.
    fn dist_to_chain(p: Point, chain: &[Cubic]) -> f64 {
        chain
            .iter()
            .flat_map(|c| (0..=400).map(move |i| eval(c, i as f64 / 400.0)))
            .map(|q| dist(p, q))
            .fold(f64::INFINITY, f64::min)
    }

    fn arc(radius: f64, n: usize) -> Vec<Point> {
        (0..=n)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * i as f64 / n as f64;
                [radius * a.cos(), radius * a.sin()]
            })
            .collect()
    }

    #[test]
    fn eval_hits_the_endpoints_and_the_midpoint() {
        let c = [[0.0, 0.0], [0.0, 3.0], [3.0, 3.0], [3.0, 0.0]];
        assert_eq!(eval(&c, 0.0), [0.0, 0.0]);
        assert_eq!(eval(&c, 1.0), [3.0, 0.0]);
        assert_eq!(eval(&c, 0.5), [1.5, 2.25]);
    }

    #[test]
    fn fit_of_nothing_is_empty() {
        assert!(fit(&[], 1.0).is_empty());
    }

    #[test]
    fn fit_of_one_point_is_a_degenerate_cubic() {
        assert_eq!(fit(&[[1.0, 2.0]], 1.0), vec![[[1.0, 2.0]; 4]]);
    }

    #[test]
    fn fit_of_two_points_puts_the_handles_at_the_thirds() {
        assert_eq!(
            fit(&[[0.0, 0.0], [3.0, 0.0]], 1.0),
            vec![[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]]
        );
    }

    #[test]
    fn fit_of_a_collinear_run_is_one_cubic_on_the_line() {
        let pts: Vec<Point> = (0..=10).map(|i| [i as f64 * 7.0, 0.0]).collect();
        let got = fit(&pts, 0.5);
        assert_eq!(got.len(), 1, "{got:?}");
        let c = got[0];
        assert_eq!(c[0], [0.0, 0.0]);
        assert_eq!(c[3], [70.0, 0.0]);
        assert!(c[1][1].abs() < 1e-9 && c[2][1].abs() < 1e-9, "{c:?}");
        assert!(
            c[1][0] > 0.0 && c[2][0] < 70.0,
            "handles between the ends: {c:?}"
        );
    }

    #[test]
    fn fit_of_an_arc_stays_within_max_error_with_few_cubics() {
        let pts = arc(100.0, 24);
        let got = fit(&pts, 0.5);
        assert!(got.len() <= 2, "{} cubics for a quarter circle", got.len());
        assert_eq!(got[0][0], pts[0]);
        assert_eq!(got[got.len() - 1][3], pts[24]);
        for p in &pts {
            let d = dist_to_chain(*p, &got);
            assert!(d <= 0.5, "{p:?} is {d} off the fit");
        }
    }

    #[test]
    fn fit_chain_is_continuous() {
        // A wave: too curly for one cubic, so it must split and rejoin.
        let pts: Vec<Point> = (0..=60)
            .map(|i| {
                let x = i as f64 * 5.0;
                [x, 40.0 * (x / 50.0).sin()]
            })
            .collect();
        let got = fit(&pts, 0.5);
        assert!(got.len() >= 2, "{got:?}");
        for w in got.windows(2) {
            assert_eq!(w[0][3], w[1][0]);
        }
        for p in &pts {
            let d = dist_to_chain(*p, &got);
            assert!(d <= 0.5, "{p:?} is {d} off the fit");
        }
    }

    #[test]
    fn fit_splits_at_a_sharp_corner() {
        let mut pts: Vec<Point> = (0..=10).map(|i| [i as f64 * 10.0, 0.0]).collect();
        pts.extend((1..=10).map(|i| [100.0, i as f64 * 10.0]));
        let got = fit(&pts, 0.5);
        assert!(got.len() >= 2, "{got:?}");
        assert!(
            got.iter().any(|c| c[3] == [100.0, 0.0]),
            "the corner must be a joint: {got:?}"
        );
    }

    #[test]
    fn fit_survives_repeated_points() {
        let got = fit(&[[0.0, 0.0], [0.0, 0.0], [3.0, 0.0], [3.0, 0.0]], 0.5);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].iter().flatten().all(|v| v.is_finite()), "{got:?}");
        assert_eq!(got[0][0], [0.0, 0.0]);
        assert_eq!(got[0][3], [3.0, 0.0]);
    }

    #[test]
    fn simplify_measures_from_the_point_when_the_chord_is_degenerate() {
        // A loop that starts and ends at the same spot has no chord to
        // measure against; the far point must still survive.
        let pts = [[0.0, 0.0], [3.0, 0.0], [0.0, 0.0]];
        assert_eq!(simplify(&pts, 1.0), pts.to_vec());
        assert_eq!(
            simplify(&[[0.0, 0.0], [0.5, 0.0], [0.0, 0.0]], 1.0),
            vec![[0.0, 0.0], [0.0, 0.0]]
        );
    }
}
