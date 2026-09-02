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

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
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
