//! Plane geometry for selection and transforms, in world units: affine
//! maps and oriented boxes. Pure. Angles are radians, positive clockwise on
//! screen (y down), so a rotation of +90° turns +x into +y.

pub use crate::curve::Point;

/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f` — SVG's `matrix(a b c d e f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(dx: f64, dy: f64) -> Affine {
        Affine {
            e: dx,
            f: dy,
            ..Affine::IDENTITY
        }
    }

    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine {
            a: sx,
            d: sy,
            ..Affine::IDENTITY
        }
    }

    /// Rotation about the origin by `angle` radians.
    pub fn rotate(angle: f64) -> Affine {
        let (s, c) = angle.sin_cos();
        Affine {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    /// The same map pivoted on `center` instead of the origin.
    pub fn about(self, center: Point) -> Affine {
        Affine::translate(-center[0], -center[1])
            .then(self)
            .then(Affine::translate(center[0], center[1]))
    }

    pub fn apply(&self, p: Point) -> Point {
        [
            self.a * p[0] + self.c * p[1] + self.e,
            self.b * p[0] + self.d * p[1] + self.f,
        ]
    }

    /// The linear part only: what the map does to a direction.
    pub fn linear(&self, v: Point) -> Point {
        [self.a * v[0] + self.c * v[1], self.b * v[0] + self.d * v[1]]
    }
}

/// A box corner, in the order [`Frame::corners`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

impl Corner {
    /// Clockwise from the top left.
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
    ];

    /// Where the corner sits in the frame's local axes, as ±1 per axis.
    pub fn signs(self) -> Point {
        match self {
            Corner::TopLeft => [-1.0, -1.0],
            Corner::TopRight => [1.0, -1.0],
            Corner::BottomRight => [1.0, 1.0],
            Corner::BottomLeft => [-1.0, 1.0],
        }
    }

    pub fn opposite(self) -> Corner {
        match self {
            Corner::TopLeft => Corner::BottomRight,
            Corner::TopRight => Corner::BottomLeft,
            Corner::BottomRight => Corner::TopLeft,
            Corner::BottomLeft => Corner::TopRight,
        }
    }
}

/// Oriented box: `half` extents along its own axes, turned by `angle`
/// about `center`. Local coordinates put the center at the origin with the
/// axes unturned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub center: Point,
    pub half: Point,
    pub angle: f64,
}

impl Frame {
    /// The unturned frame around `points`; `None` for no points.
    pub fn around(points: &[Point]) -> Option<Frame> {
        let first = *points.first()?;
        let (lo, hi) = points.iter().fold((first, first), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        });
        Some(Frame::spanning(lo, hi))
    }

    /// The unturned frame with `lo` and `hi` as opposite corners.
    pub fn spanning(lo: Point, hi: Point) -> Frame {
        Frame {
            center: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
            half: [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0],
            angle: 0.0,
        }
    }

    pub fn corner(&self, corner: Corner) -> Point {
        let s = corner.signs();
        self.to_world([s[0] * self.half[0], s[1] * self.half[1]])
    }

    /// Clockwise from the top left (in local terms; the turn may put that
    /// corner anywhere).
    pub fn corners(&self) -> [Point; 4] {
        Corner::ALL.map(|c| self.corner(c))
    }

    pub fn to_local(self, p: Point) -> Point {
        Affine::rotate(-self.angle).apply([p[0] - self.center[0], p[1] - self.center[1]])
    }

    pub fn to_world(self, local: Point) -> Point {
        let p = Affine::rotate(self.angle).apply(local);
        [p[0] + self.center[0], p[1] + self.center[1]]
    }

    /// Inside the box grown by `slop` on every side.
    pub fn contains(&self, p: Point, slop: f64) -> bool {
        let l = self.to_local(p);
        l[0].abs() <= self.half[0] + slop && l[1].abs() <= self.half[1] + slop
    }

    /// Axis-aligned `(min, max)` around the turned box.
    pub fn aabb(&self) -> (Point, Point) {
        let c = self.corners();
        c.iter().fold((c[0], c[0]), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1])],
                [hi[0].max(p[0]), hi[1].max(p[1])],
            )
        })
    }

    /// The frame after `m`: the center is mapped, the axes are mapped as
    /// directions. Exact for translations, rotations, and stretches along
    /// the frame's own axes; a stretch across them is approximated by a box
    /// (the true image would be a parallelogram).
    pub fn transformed(&self, m: &Affine) -> Frame {
        let (s, c) = self.angle.sin_cos();
        let u = m.linear([c, s]);
        let v = m.linear([-s, c]);
        Frame {
            center: m.apply(self.center),
            half: [
                self.half[0] * u[0].hypot(u[1]),
                self.half[1] * v[0].hypot(v[1]),
            ],
            angle: u[1].atan2(u[0]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF_TURN: f64 = std::f64::consts::PI;
    const QUARTER_TURN: f64 = std::f64::consts::FRAC_PI_2;

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[track_caller]
    fn assert_close(a: Point, b: Point) {
        assert!(close(a, b), "{a:?} != {b:?}");
    }

    #[test]
    fn rotation_turns_x_into_y_clockwise_on_screen() {
        assert_close(Affine::rotate(QUARTER_TURN).apply([1.0, 0.0]), [0.0, 1.0]);
        assert_close(Affine::rotate(QUARTER_TURN).apply([0.0, 1.0]), [-1.0, 0.0]);
    }

    #[test]
    fn then_composes_left_to_right() {
        // Move first, then turn: (1, 0) → (2, 0) → (0, 2).
        let m = Affine::translate(1.0, 0.0).then(Affine::rotate(QUARTER_TURN));
        assert_close(m.apply([1.0, 0.0]), [0.0, 2.0]);
        // Turn first, then move: (1, 0) → (0, 1) → (1, 1).
        let m = Affine::rotate(QUARTER_TURN).then(Affine::translate(1.0, 0.0));
        assert_close(m.apply([1.0, 0.0]), [1.0, 1.0]);
    }

    #[test]
    fn about_pivots_a_map_on_a_point() {
        let m = Affine::rotate(HALF_TURN).about([5.0, 5.0]);
        assert_close(m.apply([6.0, 5.0]), [4.0, 5.0]);
        assert_close(m.apply([5.0, 5.0]), [5.0, 5.0]);
        let m = Affine::scale(2.0, 3.0).about([1.0, 1.0]);
        assert_close(m.apply([2.0, 2.0]), [3.0, 4.0]);
    }

    #[test]
    fn linear_ignores_the_translation() {
        let m = Affine::scale(2.0, 1.0).then(Affine::translate(10.0, 10.0));
        assert_close(m.apply([1.0, 1.0]), [12.0, 11.0]);
        assert_close(m.linear([1.0, 1.0]), [2.0, 1.0]);
    }

    #[test]
    fn frame_corners_go_clockwise_from_the_top_left() {
        let f = Frame {
            center: [10.0, 10.0],
            half: [4.0, 2.0],
            angle: 0.0,
        };
        assert_eq!(
            f.corners(),
            [[6.0, 8.0], [14.0, 8.0], [14.0, 12.0], [6.0, 12.0]]
        );
        // Turned a quarter: the top-left corner ends up top-right.
        let turned = Frame {
            angle: QUARTER_TURN,
            ..f
        };
        let c = turned.corners();
        assert_close(c[0], [12.0, 6.0]);
        assert_close(c[1], [12.0, 14.0]);
        assert_close(c[2], [8.0, 14.0]);
        assert_close(c[3], [8.0, 6.0]);
    }

    #[test]
    fn corner_signs_match_corner_order() {
        assert_eq!(
            Corner::ALL.map(|c| c.signs()),
            [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
        );
        let f = Frame {
            center: [10.0, 10.0],
            half: [4.0, 2.0],
            angle: 0.3,
        };
        for (i, corner) in Corner::ALL.into_iter().enumerate() {
            assert_close(f.corner(corner), f.corners()[i]);
            assert_eq!(corner.opposite().signs(), corner.signs().map(|s| -s));
        }
    }

    #[test]
    fn local_coordinates_round_trip_through_the_frame() {
        let f = Frame {
            center: [3.0, -2.0],
            half: [5.0, 1.0],
            angle: 0.7,
        };
        for p in [[0.0, 0.0], [3.0, -2.0], [10.5, 4.0]] {
            assert_close(f.to_world(f.to_local(p)), p);
        }
        assert_close(f.to_local(f.corner(Corner::BottomRight)), [5.0, 1.0]);
        assert_close(f.to_world([-5.0, -1.0]), f.corner(Corner::TopLeft));
    }

    #[test]
    fn contains_tests_in_local_space_with_slop() {
        let f = Frame {
            center: [0.0, 0.0],
            half: [10.0, 1.0],
            angle: QUARTER_TURN,
        };
        // Long axis now runs along y.
        assert!(f.contains([0.0, 9.0], 0.0));
        assert!(!f.contains([9.0, 0.0], 0.0));
        assert!(!f.contains([0.0, 10.5], 0.0));
        assert!(f.contains([0.0, 10.5], 1.0), "slop grows the box");
        assert!(f.contains([1.5, 0.0], 1.0));
    }

    #[test]
    fn aabb_wraps_the_rotated_box() {
        let f = Frame {
            center: [0.0, 0.0],
            half: [1.0, 1.0],
            angle: std::f64::consts::FRAC_PI_4,
        };
        let (lo, hi) = f.aabb();
        let d = std::f64::consts::SQRT_2;
        assert_close(lo, [-d, -d]);
        assert_close(hi, [d, d]);
    }

    #[test]
    fn around_is_the_axis_aligned_frame_of_points() {
        let f = Frame::around(&[[1.0, 5.0], [7.0, 1.0], [3.0, 3.0]]).unwrap();
        assert_eq!(f.center, [4.0, 3.0]);
        assert_eq!(f.half, [3.0, 2.0]);
        assert_eq!(f.angle, 0.0);
        assert_eq!(Frame::around(&[]), None);
    }

    #[test]
    fn transformed_maps_the_frame_through_an_affine() {
        let f = Frame {
            center: [2.0, 2.0],
            half: [1.0, 3.0],
            angle: 0.0,
        };
        let g = f.transformed(&Affine::rotate(QUARTER_TURN).then(Affine::translate(1.0, 0.0)));
        assert_close(g.center, [-1.0, 2.0]);
        assert!((g.angle - QUARTER_TURN).abs() < 1e-9);
        assert_close(g.half, [1.0, 3.0]);
        // A stretch along the frame's own axes changes its size, not its angle.
        let g = f.transformed(&Affine::scale(2.0, 0.5).about(f.center));
        assert_close(g.center, [2.0, 2.0]);
        assert_close(g.half, [2.0, 1.5]);
        assert_eq!(g.angle, 0.0);
        // Mirroring keeps the size positive.
        let g = f.transformed(&Affine::scale(-1.0, 1.0));
        assert_close(g.half, [1.0, 3.0]);
    }
}
