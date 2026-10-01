//! The two hand models' arithmetic (pure): MediaPipe's palm detector and
//! hand landmark model, as OpenCV's model zoo converted them to ONNX, and
//! everything that goes into and comes out of them that is not running
//! them. Ported from the zoo's own reference (`mp_palmdet.py`,
//! `mp_handpose.py`, Apache 2.0), with one simplification: the zoo crops,
//! pads, turns and crops again on the way to the landmark model, and that
//! chain is one affine map — so here it is one warp, the region a
//! [`Roi`] and its two directions [`Roi::crop`] and [`Roi::back`].
//!
//! The detector finds palms on a letterboxed square; the landmark model
//! reads 21 points off a square turned upright on the hand. A hand once
//! found is followed from its own points ([`palm_of`]), so the detector —
//! the heavier of the two — only runs when a hand is missing, which is
//! what MediaPipe itself does.

use std::f32::consts::PI;

/// The detector's input side, and the landmark model's.
pub const PALM_SIDE: usize = 192;
pub const HAND_SIDE: usize = 224;

/// How many points the landmark model reads off a hand.
pub const POINTS: usize = 21;

/// What the detector has to be sure of for a palm to count, and how far
/// two palms may overlap and still be two.
const SCORE_MIN: f32 = 0.5;
const NMS_IOU: f32 = 0.3;
/// The palms kept at most: two hands.
const MAX_PALMS: usize = 2;

/// Which of the hand's 21 points stand where the detector's seven palm
/// keypoints do: the wrist, the four knuckles from the index out, and the
/// thumb's two lowest joints. A hand followed from its own points is put
/// back into the detector's terms through these.
pub const PALM_KEYS: [usize; 7] = [0, 5, 9, 13, 17, 1, 2];

pub type Point = [f32; 2];

/// A palm the detector found, in image px: its box, its seven keypoints
/// and how sure it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palm {
    pub lo: Point,
    pub hi: Point,
    pub keys: [Point; 7],
    pub score: f32,
}

impl Palm {
    pub fn middle(&self) -> Point {
        [(self.lo[0] + self.hi[0]) / 2.0, (self.lo[1] + self.hi[1]) / 2.0]
    }

    fn area(&self) -> f32 {
        (self.hi[0] - self.lo[0]).max(0.0) * (self.hi[1] - self.lo[1]).max(0.0)
    }

    /// How much two palms overlap, of what they cover together.
    fn iou(&self, other: &Palm) -> f32 {
        let w = (self.hi[0].min(other.hi[0]) - self.lo[0].max(other.lo[0])).max(0.0);
        let h = (self.hi[1].min(other.hi[1]) - self.lo[1].max(other.lo[1])).max(0.0);
        let both = w * h;
        let either = self.area() + other.area() - both;
        if either > 0.0 { both / either } else { 0.0 }
    }
}

/// The detector's anchors: the middle of every cell of a 24 by 24 grid
/// twice over, then of a 12 by 12 grid six times over — 2016 in all, in
/// the order its outputs come in.
pub fn anchors() -> Vec<Point> {
    let mut out = Vec::with_capacity(2016);
    for (grid, per) in [(24_u16, 2), (12, 6)] {
        let g = f32::from(grid);
        for y in 0..grid {
            for x in 0..grid {
                for _ in 0..per {
                    out.push([(f32::from(x) + 0.5) / g, (f32::from(y) + 0.5) / g]);
                }
            }
        }
    }
    out
}

/// How a `w` by `h` image goes into the detector's square: scaled to fit
/// its long side and padded black about the short one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    /// The image scaled, in input px.
    pub size: [i32; 2],
    /// The padding to its left and above it, in input px.
    pub pad: [i32; 2],
    /// The same padding in image px: what comes off a decoded point.
    pub bias: [f32; 2],
    /// The image's long side: the detector's unit square, in image px.
    pub scale: f32,
}

pub fn letterbox(w: i32, h: i32) -> Letterbox {
    let side = PALM_SIDE as f32;
    let ratio = (side / h as f32).min(side / w as f32);
    let size = [(w as f32 * ratio) as i32, (h as f32 * ratio) as i32];
    let pad = [(PALM_SIDE as i32 - size[0]) / 2, (PALM_SIDE as i32 - size[1]) / 2];
    Letterbox {
        size,
        pad,
        bias: [(pad[0] as f32 / ratio).trunc(), (pad[1] as f32 / ratio).trunc()],
        scale: w.max(h) as f32,
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The palms in the detector's output — `regressors` 18 floats an anchor
/// (a box's middle and size, then seven keypoints, each as an offset from
/// the anchor in input px), `scores` one logit an anchor — in image px,
/// the surest first, two at most, none overlapping another.
pub fn palms(regressors: &[f32], scores: &[f32], lb: &Letterbox, anchors: &[Point]) -> Vec<Palm> {
    let side = PALM_SIDE as f32;
    let to_image = |u: f32, a: f32, bias: f32| (u / side + a) * lb.scale - bias;
    let mut found: Vec<Palm> = anchors
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let score = sigmoid(*scores.get(i)?);
            if score < SCORE_MIN {
                return None;
            }
            let r = regressors.get(i * 18..(i + 1) * 18)?;
            let (cx, cy) = (r[0], r[1]);
            let (w, h) = (r[2], r[3]);
            let mut keys = [[0.0; 2]; 7];
            for (k, key) in keys.iter_mut().enumerate() {
                *key = [
                    to_image(r[4 + 2 * k], a[0], lb.bias[0]),
                    to_image(r[5 + 2 * k], a[1], lb.bias[1]),
                ];
            }
            Some(Palm {
                lo: [to_image(cx - w / 2.0, a[0], lb.bias[0]), to_image(cy - h / 2.0, a[1], lb.bias[1])],
                hi: [to_image(cx + w / 2.0, a[0], lb.bias[0]), to_image(cy + h / 2.0, a[1], lb.bias[1])],
                keys,
                score,
            })
        })
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Palm> = Vec::new();
    for p in found {
        if kept.iter().all(|k| k.iou(&p) <= NMS_IOU) {
            kept.push(p);
            if kept.len() == MAX_PALMS {
                break;
            }
        }
    }
    kept
}

/// Where the landmark model looks: a square of `side` image px about
/// `center`, turned by `angle` radians so the hand stands upright in it —
/// wrist down, fingers up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    pub center: Point,
    pub side: f32,
    pub angle: f32,
}

/// How far the square is moved off the palm toward the fingers, as a
/// share of the palm's height, and how many palms wide it is.
const ROI_SHIFT: f32 = -0.4;
const ROI_ENLARGE: f32 = 3.0;

impl Roi {
    /// The square that holds the hand whose palm has keypoints `keys`
    /// about `pivot`: turned so the wrist (key 0) lies under the middle
    /// knuckle (key 2), and laid over the keypoints' box once turned —
    /// moved toward the fingers, three palms wide.
    pub fn of(keys: &[Point; 7], pivot: Point) -> Roi {
        let (p1, p2) = (keys[0], keys[2]);
        let mut angle = PI / 2.0 - (-(p2[1] - p1[1])).atan2(p2[0] - p1[0]);
        angle -= 2.0 * PI * ((angle + PI) / (2.0 * PI)).floor();
        let (c, s) = (angle.cos(), angle.sin());
        // Turned about the pivot, the way the zoo turns the image.
        let turn = |p: Point| {
            let (dx, dy) = (p[0] - pivot[0], p[1] - pivot[1]);
            [c * dx + s * dy + pivot[0], -s * dx + c * dy + pivot[1]]
        };
        let turned: Vec<Point> = keys.iter().map(|k| turn(*k)).collect();
        let lo = turned.iter().fold([f32::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
        let hi = turned.iter().fold([f32::NEG_INFINITY; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        let middle = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0 + ROI_SHIFT * h];
        // And back: the square's middle in the image as it is.
        let (dx, dy) = (middle[0] - pivot[0], middle[1] - pivot[1]);
        Roi {
            center: [c * dx - s * dy + pivot[0], s * dx + c * dy + pivot[1]],
            side: ROI_ENLARGE * w.max(h),
            angle,
        }
    }

    /// The map from image px to the landmark model's input: a 2 by 3
    /// affine matrix, rows first, for a warp.
    pub fn crop(&self) -> [[f64; 3]; 2] {
        let k = HAND_SIDE as f64 / f64::from(self.side);
        let (c, s) = (f64::from(self.angle.cos()) * k, f64::from(self.angle.sin()) * k);
        let half = HAND_SIDE as f64 / 2.0;
        let (x, y) = (f64::from(self.center[0]), f64::from(self.center[1]));
        [[c, s, half - c * x - s * y], [-s, c, half + s * x - c * y]]
    }

    /// A point the landmark model read, in its input px with its depth,
    /// back in image px — the depth scaled as the plane is.
    pub fn back(&self, p: [f32; 3]) -> [f32; 3] {
        let k = self.side / HAND_SIDE as f32;
        let half = HAND_SIDE as f32 / 2.0;
        let (dx, dy) = ((p[0] - half) * k, (p[1] - half) * k);
        let (c, s) = (self.angle.cos(), self.angle.sin());
        [self.center[0] + c * dx - s * dy, self.center[1] + s * dx + c * dy, p[2] * k]
    }

    /// Whether `other` looks at the same hand: their middles closer than
    /// half the larger square.
    pub fn same_hand(&self, other: &Roi) -> bool {
        let d = (self.center[0] - other.center[0]).hypot(self.center[1] - other.center[1]);
        d < self.side.max(other.side) / 2.0
    }
}

/// A hand's 21 points put back into the detector's terms — the seven
/// keypoints and the middle of their box — so the next frame looks for
/// it where it is instead of searching the whole image.
pub fn palm_of(points: &[[f32; 3]; POINTS]) -> ([Point; 7], Point) {
    let keys = PALM_KEYS.map(|i| [points[i][0], points[i][1]]);
    let lo = keys.iter().fold([f32::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
    let hi = keys.iter().fold([f32::NEG_INFINITY; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
    (keys, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, by: f32) -> bool {
        (a - b).abs() <= by
    }

    #[test]
    fn the_anchors_are_two_grids_in_the_order_the_outputs_come() {
        let a = anchors();
        assert_eq!(a.len(), 2016);
        assert_eq!(a[0], [0.5 / 24.0, 0.5 / 24.0]);
        assert_eq!(a[1], a[0], "two to a cell");
        assert_eq!(a[2], [1.5 / 24.0, 0.5 / 24.0]);
        assert_eq!(a[1152], [0.5 / 12.0, 0.5 / 12.0], "the coarse grid starts");
        assert_eq!(a[2015], [11.5 / 12.0, 11.5 / 12.0]);
    }

    #[test]
    fn a_wide_frame_is_letterboxed_top_and_bottom() {
        let lb = letterbox(640, 480);
        assert_eq!(lb.size, [192, 144]);
        assert_eq!(lb.pad, [0, 24]);
        assert_eq!(lb.bias, [0.0, 80.0]);
        assert_eq!(lb.scale, 640.0);
    }

    /// The raw output of a detector that saw one palm at anchor `at`,
    /// `dx` input px right of it, 40 by 40 input px, and nothing else.
    fn one_palm(at: usize, dx: f32) -> (Vec<f32>, Vec<f32>) {
        let mut reg = vec![0.0; 2016 * 18];
        let mut scores = vec![-10.0; 2016];
        scores[at] = 4.0;
        let r = &mut reg[at * 18..(at + 1) * 18];
        r[0] = dx;
        r[2] = 40.0;
        r[3] = 40.0;
        for k in 0..7 {
            r[4 + 2 * k] = dx;
            r[5 + 2 * k] = -(k as f32) * 2.0;
        }
        (reg, scores)
    }

    #[test]
    fn a_palm_is_decoded_into_image_px() {
        let anchors = anchors();
        let lb = letterbox(640, 480);
        // The coarse grid's cell (6, 6): its middle is 6.5 / 12 of the
        // way in, which is 346.67 px across and 266.67 - 80 down.
        let at = 1152 + (6 * 12 + 6) * 6;
        let (reg, scores) = one_palm(at, 19.2);
        let found = palms(&reg, &scores, &lb, &anchors);
        assert_eq!(found.len(), 1);
        let p = found[0];
        let (cx, cy) = ((6.5 / 12.0 + 0.1) * 640.0, 6.5 / 12.0 * 640.0 - 80.0);
        assert!(close(p.middle()[0], cx, 0.01) && close(p.middle()[1], cy, 0.01), "{p:?}");
        assert!(close(p.hi[0] - p.lo[0], 40.0 / 192.0 * 640.0, 0.01));
        assert!(close(p.keys[0][0], cx, 0.01));
        assert!(p.score > 0.98);
    }

    #[test]
    fn two_reads_of_one_palm_are_one_palm_and_two_hands_are_two() {
        let anchors = anchors();
        let lb = letterbox(640, 480);
        let (mut reg, mut scores) = one_palm(1152 + (6 * 12 + 6) * 6, 0.0);
        // The second anchor of the same cell, a hair to the side.
        let twin = 1152 + (6 * 12 + 6) * 6 + 1;
        scores[twin] = 3.0;
        reg[twin * 18 + 2] = 40.0;
        reg[twin * 18 + 3] = 40.0;
        reg[twin * 18] = 2.0;
        assert_eq!(palms(&reg, &scores, &lb, &anchors).len(), 1, "the same palm");
        // A palm three cells over is another hand.
        let other = 1152 + (6 * 12 + 9) * 6;
        scores[other] = 2.0;
        reg[other * 18 + 2] = 40.0;
        reg[other * 18 + 3] = 40.0;
        let found = palms(&reg, &scores, &lb, &anchors);
        assert_eq!(found.len(), 2);
        assert!(found[0].score > found[1].score, "the surest first");
    }

    /// A palm standing upright at `(x, y)`: the wrist below, the middle
    /// knuckle 60 px above it.
    fn upright(x: f32, y: f32) -> [Point; 7] {
        [
            [x, y],
            [x - 25.0, y - 55.0],
            [x, y - 60.0],
            [x + 20.0, y - 55.0],
            [x + 35.0, y - 45.0],
            [x - 30.0, y - 10.0],
            [x - 40.0, y - 25.0],
        ]
    }

    #[test]
    fn an_upright_palm_is_not_turned_and_its_square_reaches_toward_the_fingers() {
        let keys = upright(300.0, 300.0);
        let roi = Roi::of(&keys, [300.0, 270.0]);
        assert!(close(roi.angle, 0.0, 1e-6), "{roi:?}");
        // The keypoints' box is 75 wide and 60 tall: three times the
        // long side, moved up by 0.4 of the height.
        assert!(close(roi.side, 225.0, 1e-3));
        assert!(close(roi.center[0], 297.5, 1e-3) && close(roi.center[1], 270.0 - 24.0, 1e-3), "{roi:?}");
    }

    #[test]
    fn a_palm_lying_on_its_side_is_turned_upright() {
        // The fingers point right: the middle knuckle is right of the wrist.
        let keys = [
            [100.0, 100.0],
            [155.0, 75.0],
            [160.0, 100.0],
            [155.0, 120.0],
            [145.0, 135.0],
            [110.0, 70.0],
            [125.0, 60.0],
        ];
        let roi = Roi::of(&keys, [130.0, 100.0]);
        assert!(close(roi.angle.abs(), PI / 2.0, 1e-5), "a quarter turn: {roi:?}");
        // Upright in the crop, the middle knuckle stands above the wrist.
        let m = roi.crop();
        let at = |p: Point| {
            let (x, y) = (f64::from(p[0]), f64::from(p[1]));
            [m[0][0] * x + m[0][1] * y + m[0][2], m[1][0] * x + m[1][1] * y + m[1][2]]
        };
        let (wrist, knuckle) = (at(keys[0]), at(keys[2]));
        assert!(close(wrist[0] as f32, knuckle[0] as f32, 1e-3), "{wrist:?} {knuckle:?}");
        assert!(knuckle[1] < wrist[1], "fingers up");
    }

    #[test]
    fn the_crop_and_the_way_back_are_one_map_and_its_inverse() {
        let roi = Roi {
            center: [320.0, 200.0],
            side: 300.0,
            angle: 0.7,
        };
        let m = roi.crop();
        for p in [[320.0_f32, 200.0], [100.0, 50.0], [400.0, 310.0]] {
            let (x, y) = (f64::from(p[0]), f64::from(p[1]));
            let u = (m[0][0] * x + m[0][1] * y + m[0][2]) as f32;
            let v = (m[1][0] * x + m[1][1] * y + m[1][2]) as f32;
            let back = roi.back([u, v, 10.0]);
            assert!(close(back[0], p[0], 1e-2) && close(back[1], p[1], 1e-2), "{p:?} -> {back:?}");
            assert!(close(back[2], 10.0 * 300.0 / 224.0, 1e-4), "depth scales with the plane");
        }
        // The middle of the square is the middle of the input.
        let mid = roi.back([112.0, 112.0, 0.0]);
        assert!(close(mid[0], 320.0, 1e-3) && close(mid[1], 200.0, 1e-3));
    }

    #[test]
    fn a_hand_is_followed_through_its_own_palm_points() {
        let mut points = [[0.0_f32; 3]; POINTS];
        for (i, p) in points.iter_mut().enumerate() {
            *p = [i as f32 * 10.0, 500.0 - i as f32 * 10.0, 0.0];
        }
        let (keys, middle) = palm_of(&points);
        assert_eq!(keys[0], [0.0, 500.0], "the wrist");
        assert_eq!(keys[2], [90.0, 410.0], "the middle knuckle, point 9");
        assert_eq!(keys[6], [20.0, 480.0], "the thumb's second joint, point 2");
        assert_eq!(middle, [85.0, 415.0]);
    }

    #[test]
    fn the_same_hand_is_told_apart_from_another() {
        let a = Roi {
            center: [100.0, 100.0],
            side: 200.0,
            angle: 0.0,
        };
        assert!(a.same_hand(&Roi { center: [150.0, 120.0], ..a }));
        assert!(!a.same_hand(&Roi { center: [400.0, 100.0], ..a }));
    }
}
