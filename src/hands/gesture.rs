//! Hand gestures (pure): what the hands the camera follows do to the
//! board. Every number here was tuned in front of a camera, in the
//! Python experiment this was ported from; only the clocks moved — the
//! springs, the coast and the laser step at the screen's rate in
//! [`Gestures::tick`], and the camera's readings only move their targets
//! in [`Gestures::read`].
//!
//! - one hand pinching (thumb and index touching), moved: drag the board
//! - two hands pinching: zoom by pulling them apart or together, and pan
//! - one hand pointing (index out, the other three folded): a laser
//! - an open hand swept sideways: the next slide (to the left) or the one
//!   before (to the right)
//!
//! A hand is the same hand from frame to frame by where its wrist is,
//! never by a left or right label: that label flips, and two hands up at
//! once are often both called the same.

use std::collections::VecDeque;
use std::f64::consts::PI;

use super::model::POINTS;

/// A point in the camera's frame, mirrored: 0..1 across and down.
pub type Pt = [f64; 2];

/// Logical px the board moves for a hand travelling the camera's width.
const PAN_GAIN: f64 = 2200.0;
/// The pinch: thumb and index tips apart, in 3D, over the hand's size
/// (wrist to middle knuckle), with hysteresis.
const PINCH_ON: f64 = 0.30;
const PINCH_OFF: f64 = 0.45;
/// Frames a pinch, and a point, has to hold before it counts.
const PINCH_FRAMES: u32 = 2;
const POINT_FRAMES: u32 = 3;
/// Seconds after pointing ends before a pinch moves the board.
const QUIET_AFTER_LASER: f64 = 0.35;
/// A finger is out when its tip is this much farther from the wrist than
/// its middle joint.
const EXTENDED: f64 = 1.1;
const ZOOM_GAIN: f64 = 1.0;

/// A sweep: far and fast enough, more across than up or down, the hand
/// open most of the way.
const SWIPE_WINDOW: f64 = 0.45;
const SWIPE_MIN: f64 = 0.2;
const SWIPE_STRAIGHT: f64 = 1.3;
const SWIPE_OPEN: f64 = 0.5;
const SWIPE_SETTLE: f64 = 0.2;
const SWIPE_GRACE: f64 = 0.3;
const SWIPE_MATCH: f64 = 0.5;
const SWIPE_COOLDOWN: f64 = 0.8;
/// Seconds after a pinch or the laser before a sweep counts.
const QUIET_AFTER_GESTURE: f64 = 0.5;
/// Fingers out, of the four, for a hand to count as open.
const FINGERS_OPEN: u32 = 3;

/// How far a wrist may move between frames and be the same hand, and
/// how long a hand the camera lost is kept to be found again as itself.
const HAND_MATCH: f64 = 0.3;
const HAND_GRACE: f64 = 0.25;

/// The One Euro filter: a still hand is cut hard, a fast one barely.
const MIN_CUTOFF: f64 = 0.8;
const BETA: f64 = 4.0;
const D_CUTOFF: f64 = 1.0;

/// How stiff the springs the board chases the hands on are: higher is
/// snappier.
const PAN_STIFFNESS: f64 = 11.0;
const ZOOM_STIFFNESS: f64 = 6.0;

/// A drag let go of at speed coasts on, losing speed to friction: the
/// coast covers its speed over `FRICTION` px. The speed is read off the
/// drag while the fingers were still closed — opening them moves the
/// point between them, and a hand held still while it lets go is not a
/// throw — over `FLING_WINDOW` seconds ending `FLING_SKIP` before the
/// last closed reading, which is the filter's lag on the gap. Slower than
/// `FLING_MIN` px/s there is no coast, and none is faster than
/// `FLING_MAX`; fingers last closed `FLING_STALE` seconds before the
/// release were opened slowly, and throw nothing either.
const FRICTION: f64 = 4.0;
const FLING_MIN: f64 = 600.0;
const FLING_MAX: f64 = 5000.0;
const FLING_WINDOW: f64 = 0.12;
const FLING_SKIP: f64 = 0.035;
const FLING_STALE: f64 = 0.3;

/// The laser: how long its trail takes to fade, how fast the drawn tip
/// closes on the camera's, how fast it goes out — and how long the
/// chevron of a turned slide stays.
pub const TRAIL_S: f64 = 0.6;
const FOLLOW_RATE: f64 = 28.0;
const FADE_S: f64 = 0.15;
pub const FLASH_S: f64 = 0.6;

/// A slide turned by a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    Next,
    Prev,
}

fn alpha(cutoff: f64, dt: f64) -> f64 {
    let tau = 1.0 / (2.0 * PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

#[derive(Debug, Clone, Default)]
struct OneEuro {
    x: Option<f64>,
    dx: f64,
}

impl OneEuro {
    fn filter(&mut self, x: f64, dt: f64) -> f64 {
        let Some(prev) = self.x else {
            self.x = Some(x);
            return x;
        };
        let a = alpha(D_CUTOFF, dt);
        self.dx = a * (x - prev) / dt + (1.0 - a) * self.dx;
        let a = alpha(MIN_CUTOFF + BETA * self.dx.abs(), dt);
        let next = a * x + (1.0 - a) * prev;
        self.x = Some(next);
        next
    }
}

/// Critically damped, integrated implicitly so it never blows up.
#[derive(Debug, Clone)]
struct Spring {
    omega: f64,
    x: f64,
    v: f64,
    target: f64,
}

impl Spring {
    fn new(omega: f64) -> Spring {
        Spring {
            omega,
            x: 0.0,
            v: 0.0,
            target: 0.0,
        }
    }

    /// One step of `dt` seconds: how far it moved.
    fn step(&mut self, dt: f64) -> f64 {
        let w = self.omega;
        let before = self.x;
        let f = 1.0 + 2.0 * dt * w;
        let hoo = dt * w * w;
        let det = 1.0 / (f + dt * hoo);
        let x = (f * self.x + dt * self.v + dt * hoo * self.target) * det;
        self.v = (self.v + hoo * (self.target - self.x)) * det;
        self.x = x;
        self.x - before
    }

    /// Stops where it is.
    fn hold(&mut self) {
        self.target = self.x;
        self.v = 0.0;
    }

    fn settled(&self) -> bool {
        (self.target - self.x).abs() < 0.05 && self.v.abs() < 0.05
    }
}

/// A hand followed from frame to frame.
#[derive(Debug, Clone)]
struct Hand {
    wrist: Pt,
    /// When the camera last found it.
    seen: f64,
    /// The pinch point (between thumb and index tips), filtered.
    point: Pt,
    pinched: bool,
    gap: f64,
    held: u32,
    pointing: bool,
    pheld: u32,
    /// The index tip, filtered: the laser.
    tip: Pt,
    fingers: u32,
    palm: Pt,
    fx: OneEuro,
    fy: OneEuro,
    fgap: OneEuro,
    ftx: OneEuro,
    fty: OneEuro,
}

impl Hand {
    fn new(seen: f64) -> Hand {
        Hand {
            wrist: [0.5, 0.5],
            seen,
            point: [0.5, 0.5],
            pinched: false,
            gap: 1.0,
            held: 0,
            pointing: false,
            pheld: 0,
            tip: [0.5, 0.5],
            fingers: 0,
            palm: [0.5, 0.5],
            fx: OneEuro::default(),
            fy: OneEuro::default(),
            fgap: OneEuro::default(),
            ftx: OneEuro::default(),
            fty: OneEuro::default(),
        }
    }

    /// Reads the hand off its 21 points (x and depth by the camera's
    /// width, y by its height). In 3D: a finger pointed at the camera is
    /// short on the image and would read as folded, its tip on top of the
    /// thumb's — so y is put back on the scale x and depth share.
    fn update(&mut self, p: &[[f32; 3]; POINTS], dt: f64, aspect: f64) {
        let at = |i: usize| [f64::from(p[i][0]), f64::from(p[i][1]) * aspect, f64::from(p[i][2])];
        let reach = |i: usize| {
            let (a, b) = (at(0), at(i));
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let out = reach(8) > reach(6) * EXTENDED;
        let folded = [(12, 10), (16, 14), (20, 18)].iter().all(|&(tip, pip)| reach(tip) < reach(pip));
        let aimed = out && folded;
        self.fingers = [(8, 6), (12, 10), (16, 14), (20, 18)]
            .iter()
            .filter(|&&(tip, pip)| reach(tip) > reach(pip) * EXTENDED)
            .count() as u32;
        let (t, i) = (at(4), at(8));
        let apart = ((t[0] - i[0]).powi(2) + (t[1] - i[1]).powi(2) + (t[2] - i[2]).powi(2)).sqrt();
        self.gap = self.fgap.filter(apart / reach(9).max(1e-6), dt);

        // The two are exclusive: a hand in the pointing pose, or still
        // pointing, does not pinch, and a pinching one does not point.
        let open = if self.pinched { PINCH_OFF } else { PINCH_ON };
        let reads = self.gap < open && !aimed && !self.pointing;
        self.held = if reads != self.pinched { self.held + 1 } else { 0 };
        if self.held >= PINCH_FRAMES {
            self.pinched = reads;
            self.held = 0;
        }
        let reads = aimed && !self.pinched;
        self.pheld = if reads != self.pointing { self.pheld + 1 } else { 0 };
        if self.pheld >= POINT_FRAMES {
            self.pointing = reads;
            self.pheld = 0;
        }
        let xy = |i: usize| [f64::from(p[i][0]), f64::from(p[i][1])];
        let index = xy(8);
        self.tip = [self.ftx.filter(index[0], dt), self.fty.filter(index[1], dt)];
        let thumb = xy(4);
        let raw = [(thumb[0] + index[0]) / 2.0, (thumb[1] + index[1]) / 2.0];
        self.point = [self.fx.filter(raw[0], dt), self.fy.filter(raw[1], dt)];
        self.wrist = xy(0);
        self.palm = xy(9);
    }
}

fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Puts each hand the camera found on the one followed whose wrist is
/// nearest, starting a new one where none is near; drops the ones lost
/// for longer than the grace. Answers the hands found this frame, by
/// their place in `hands` — the only ones a gesture is read from.
fn follow(hands: &mut Vec<Hand>, found: &[[[f32; 3]; POINTS]], now: f64, dt: f64, aspect: f64) -> Vec<usize> {
    let wrists: Vec<Pt> = found.iter().map(|p| [f64::from(p[0][0]), f64::from(p[0][1])]).collect();
    let mut pairs: Vec<(f64, usize, usize)> = wrists
        .iter()
        .enumerate()
        .flat_map(|(i, w)| hands.iter().enumerate().map(move |(k, h)| (dist(h.wrist, *w), i, k)))
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut placed, mut taken) = (vec![false; found.len()], vec![false; hands.len()]);
    for (d, i, k) in pairs {
        if placed[i] || taken[k] || d > HAND_MATCH {
            continue;
        }
        placed[i] = true;
        taken[k] = true;
        hands[k].update(&found[i], dt, aspect);
        hands[k].seen = now;
    }
    for (i, p) in found.iter().enumerate() {
        if !placed[i] {
            let mut hand = Hand::new(now);
            hand.update(p, dt, aspect);
            hands.push(hand);
        }
    }
    hands.retain(|h| now - h.seen <= HAND_GRACE);
    (0..hands.len()).filter(|&k| hands[k].seen == now).collect()
}

/// One reading of a drag: when, where the board was being taken, and
/// whether the fingers were still closed.
type Dragged = (f64, f64, f64, bool);

/// The drag's speed just before the fingers began to open, or nothing.
fn fling(trail: &VecDeque<Dragged>, released: f64) -> (f64, f64) {
    let Some(closed) = trail.iter().rev().find(|p| p.3) else {
        return (0.0, 0.0);
    };
    if released - closed.0 > FLING_STALE {
        return (0.0, 0.0);
    }
    let end = closed.0 - FLING_SKIP;
    let window: Vec<&Dragged> = trail
        .iter()
        .filter(|p| p.3 && end - FLING_WINDOW <= p.0 && p.0 <= end)
        .collect();
    let (Some(first), Some(last)) = (window.first(), window.last()) else {
        return (0.0, 0.0);
    };
    if last.0 - first.0 < 1e-3 {
        return (0.0, 0.0);
    }
    let (vx, vy) = ((last.1 - first.1) / (last.0 - first.0), (last.2 - first.2) / (last.0 - first.0));
    let speed = vx.hypot(vy);
    if speed < FLING_MIN {
        return (0.0, 0.0);
    }
    let scale = (FLING_MAX / speed).min(1.0);
    (vx * scale, vy * scale)
}

/// One sample of a palm's track: when, where, and whether the hand was
/// open.
type Sample = (f64, f64, f64, bool);

/// The palms that may turn a slide, each followed by where it is and
/// through the frames a fast hand blurs out of. A sweep is read off
/// these tracks.
#[derive(Debug, Clone, Default)]
struct Sweep {
    tracks: Vec<VecDeque<Sample>>,
}

impl Sweep {
    fn reset(&mut self) {
        self.tracks.clear();
    }

    /// The way a palm has just been swept, if one has: `palms` are the
    /// hands not pinching or pointing, with their fingers out.
    fn feed(&mut self, now: f64, palms: &[(Pt, u32)]) -> Option<Turn> {
        self.tracks.retain(|t| t.back().is_some_and(|s| now - s.0 <= SWIPE_GRACE));
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (i, (p, _)) in palms.iter().enumerate() {
            for (k, t) in self.tracks.iter().enumerate() {
                if let Some(s) = t.back() {
                    pairs.push((dist(*p, [s.1, s.2]), i, k));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut placed, mut taken) = (vec![false; palms.len()], vec![false; self.tracks.len()]);
        for (d, i, k) in pairs {
            if placed[i] || taken[k] || d > SWIPE_MATCH {
                continue;
            }
            placed[i] = true;
            taken[k] = true;
            let (p, fingers) = palms[i];
            self.tracks[k].push_back((now, p[0], p[1], fingers >= FINGERS_OPEN));
        }
        for (i, (p, fingers)) in palms.iter().enumerate() {
            if !placed[i] {
                self.tracks.push(VecDeque::from([(now, p[0], p[1], *fingers >= FINGERS_OPEN)]));
            }
        }
        for k in 0..self.tracks.len() {
            if let Some(way) = judge(&self.tracks[k], now) {
                self.tracks.remove(k);
                return Some(way);
            }
        }
        None
    }
}

/// Whether `track` ends in a sweep: across far enough within the window,
/// more across than up or down, the hand open most of the way.
fn judge(track: &VecDeque<Sample>, now: f64) -> Option<Turn> {
    let (first, last) = (track.front()?, track.back()?);
    if last.0 != now || now - first.0 < SWIPE_SETTLE {
        return None;
    }
    let recent: Vec<&Sample> = track.iter().filter(|s| now - s.0 <= SWIPE_WINDOW).collect();
    let (a, b) = (recent.first()?, recent.last()?);
    let (dx, dy) = (b.1 - a.1, b.2 - a.2);
    let opened = recent.iter().filter(|s| s.3).count() as f64 / recent.len() as f64;
    if dx.abs() < SWIPE_MIN || dx.abs() < SWIPE_STRAIGHT * dy.abs() || opened < SWIPE_OPEN {
        return None;
    }
    Some(if dx < 0.0 { Turn::Next } else { Turn::Prev })
}

/// What the hands are doing this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Doing {
    Drag(Pt),
    /// The middle of the two pinches, and how far apart they are.
    Zoom(Pt, f64),
    Laser(Pt),
}

/// The laser as it is drawn: where the camera last saw the tip, where it
/// is drawn — closing on that every frame of the screen — how strongly
/// it glows, and the trail behind it, in strokes that do not join.
#[derive(Debug, Clone, Default)]
pub struct Laser {
    target: Option<Pt>,
    pub tip: Option<Pt>,
    pub glow: f64,
    pub trail: Vec<(f64, Pt, u32)>,
    stroke: u32,
}

/// Everything the hands are doing to the board, and what of it is drawn.
#[derive(Debug, Clone)]
pub struct Gestures {
    hands: Vec<Hand>,
    last: Option<Doing>,
    pan_x: Spring,
    pan_y: Spring,
    zoom: Spring,
    trail: VecDeque<Dragged>,
    coast: (f64, f64),
    quiet_until: f64,
    calm_until: f64,
    turned: f64,
    sweep: Sweep,
    read_at: Option<f64>,
    /// The camera's frame, in px: what the marks are laid over.
    pub size: (u32, u32),
    pub laser: Laser,
    /// A slide just turned, and when.
    pub flash: Option<(Turn, f64)>,
    /// Where the hands pinching are.
    pub pinches: Vec<Pt>,
    /// The middle of the two pinches the last zoom was read off: what
    /// the zoom is about, until the hands zoom again.
    about: Option<Pt>,
}

/// How the hands move the board in one frame of the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Logical px across and down.
    pub dx: f64,
    pub dy: f64,
    /// The zoom, as a factor on the frame before's.
    pub factor: f64,
    /// What the zoom is about, in the camera's frame: the middle of the
    /// two pinches it was read off — none while the hands have zoomed
    /// nothing, for a zoom about the pointer would be about whatever the
    /// mouse was left on.
    pub about: Option<Pt>,
}

impl Default for Gestures {
    fn default() -> Gestures {
        Gestures {
            hands: Vec::new(),
            last: None,
            pan_x: Spring::new(PAN_STIFFNESS),
            pan_y: Spring::new(PAN_STIFFNESS),
            zoom: Spring::new(ZOOM_STIFFNESS),
            trail: VecDeque::new(),
            coast: (0.0, 0.0),
            quiet_until: f64::NEG_INFINITY,
            calm_until: f64::NEG_INFINITY,
            turned: f64::NEG_INFINITY,
            sweep: Sweep::default(),
            read_at: None,
            size: (640, 480),
            laser: Laser::default(),
            flash: None,
            pinches: Vec::new(),
            about: None,
        }
    }
}

impl Gestures {
    /// One reading of the camera at `now` seconds: the hands it found
    /// (each 21 points, x and depth by the frame's width, y by its
    /// height) in a frame `size` px. Moves the springs' targets, aims the
    /// laser, and — while there are `slides` to turn — answers a slide
    /// turned by a sweep. With none, a sweep is nothing: no turn, and no
    /// chevron promising one.
    pub fn read(&mut self, found: &[[[f32; 3]; POINTS]], size: (u32, u32), now: f64, slides: bool) -> Option<Turn> {
        let dt = self.read_at.map_or(1.0 / 30.0, |t| (now - t).clamp(1e-3, 0.1));
        self.read_at = Some(now);
        self.size = size;
        let aspect = f64::from(size.1) / f64::from(size.0.max(1));
        let seen = follow(&mut self.hands, found, now, dt, aspect);
        let hands: Vec<&Hand> = seen.iter().map(|&k| &self.hands[k]).collect();
        let pinching: Vec<Pt> = hands.iter().filter(|h| h.pinched).map(|h| h.point).collect();
        let pointing: Vec<Pt> = hands.iter().filter(|h| h.pointing).map(|h| h.tip).collect();
        let doing = if pinching.is_empty() && pointing.len() == 1 {
            Some(Doing::Laser(pointing[0]))
        } else if now < self.quiet_until || !pointing.is_empty() {
            None
        } else if pinching.len() == 1 {
            Some(Doing::Drag(pinching[0]))
        } else if pinching.len() == 2 {
            let (a, b) = (pinching[0], pinching[1]);
            Some(Doing::Zoom([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], dist(a, b)))
        } else {
            None
        };

        // The hands move the springs' targets; only a step that continues
        // the same drag or zoom does, so a hand joining or leaving never
        // makes the board jump.
        match (doing, self.last) {
            (Some(Doing::Drag(p)), Some(Doing::Drag(q))) | (Some(Doing::Zoom(p, _)), Some(Doing::Zoom(q, _))) => {
                self.pan_x.target += (p[0] - q[0]) * PAN_GAIN;
                self.pan_y.target += (p[1] - q[1]) * PAN_GAIN * aspect;
                if let (Some(Doing::Zoom(_, d)), Some(Doing::Zoom(_, e))) = (doing, self.last)
                    && e > 1e-3
                {
                    self.zoom.target += ZOOM_GAIN * (d / e).ln();
                }
            }
            _ => {}
        }
        if doing.is_some() {
            self.coast = (0.0, 0.0); // a hand on the board stops it
        }
        let dragging = matches!(doing, Some(Doing::Drag(_)));
        if dragging {
            let closed = hands.iter().filter(|h| h.pinched).all(|h| h.gap < PINCH_ON);
            self.trail.push_back((now, self.pan_x.target, self.pan_y.target, closed));
            if self.trail.len() > 30 {
                self.trail.pop_front();
            }
        } else if matches!(self.last, Some(Doing::Drag(_))) {
            self.coast = fling(&self.trail, now);
        }
        if !dragging {
            self.trail.clear();
        }

        // The laser is drawn over the board, which holds still under it.
        let lasing = matches!(doing, Some(Doing::Laser(_)));
        let was_lasing = matches!(self.last, Some(Doing::Laser(_)));
        if lasing && !was_lasing {
            for s in [&mut self.pan_x, &mut self.pan_y, &mut self.zoom] {
                s.hold();
            }
        }
        if was_lasing && !lasing {
            self.quiet_until = now + QUIET_AFTER_LASER;
        }

        // A sweep of an open hand turns a slide — but not while any other
        // gesture holds the board, nor as a hand leaves one: a drag let
        // go of at speed opens into a hand still moving.
        let mut turn = None;
        if doing.is_some() || (self.last.is_some() && doing.is_none()) {
            self.calm_until = now + QUIET_AFTER_GESTURE;
        }
        if !slides || doing.is_some() || now < self.calm_until {
            self.sweep.reset();
        } else {
            let palms: Vec<(Pt, u32)> = hands
                .iter()
                .filter(|h| !h.pinched && !h.pointing)
                .map(|h| (h.palm, h.fingers))
                .collect();
            if let Some(way) = self.sweep.feed(now, &palms)
                && now - self.turned >= SWIPE_COOLDOWN
            {
                self.turned = now;
                self.flash = Some((way, now));
                turn = Some(way);
            }
        }
        self.last = doing;
        self.pinches = pinching;
        if let Some(Doing::Zoom(middle, _)) = doing {
            self.about = Some(middle);
        }
        self.aim(match doing {
            Some(Doing::Laser(p)) => Some(p),
            _ => None,
        });
        turn
    }

    /// Where the camera saw the laser: a new stroke when it comes back
    /// after going out, so its trail does not join the last one's.
    fn aim(&mut self, at: Option<Pt>) {
        let laser = &mut self.laser;
        if let Some(p) = at
            && laser.target.is_none()
            && laser.glow <= 0.0
        {
            laser.tip = Some(p);
            laser.stroke += 1;
        }
        laser.target = at;
    }

    /// One frame of the screen, `dt` seconds after the last: the coast,
    /// the springs, the laser gliding and fading, the chevron going.
    /// Answers how the board moves — logical px and a zoom factor — when
    /// it moves enough to say.
    pub fn tick(&mut self, now: f64, dt: f64) -> Option<Motion> {
        if self.coast != (0.0, 0.0) {
            self.pan_x.target += self.coast.0 * dt;
            self.pan_y.target += self.coast.1 * dt;
            let keep = (-FRICTION * dt).exp();
            self.coast = (self.coast.0 * keep, self.coast.1 * keep);
            if self.coast.0.hypot(self.coast.1) < 15.0 {
                self.coast = (0.0, 0.0);
            }
        }
        let dx = self.pan_x.step(dt);
        let dy = self.pan_y.step(dt);
        let factor = self.zoom.step(dt).clamp(-0.6, 0.6).exp();

        let laser = &mut self.laser;
        match (laser.target, laser.tip) {
            (Some(target), tip) => {
                let k = 1.0 - (-FOLLOW_RATE * dt).exp();
                let tip = tip.unwrap_or(target);
                let tip = [tip[0] + (target[0] - tip[0]) * k, tip[1] + (target[1] - tip[1]) * k];
                laser.tip = Some(tip);
                laser.glow = 1.0;
                laser.trail.push((now, tip, laser.stroke));
            }
            (None, _) => laser.glow = (laser.glow - dt / FADE_S).max(0.0),
        }
        laser.trail.retain(|(t, _, _)| now - t < TRAIL_S);
        if self.flash.is_some_and(|(_, at)| now - at > FLASH_S) {
            self.flash = None;
        }
        (dx.abs() >= 0.05 || dy.abs() >= 0.05 || (factor - 1.0).abs() >= 1e-4).then_some(Motion {
            dx,
            dy,
            factor,
            about: self.about,
        })
    }

    /// Whether anything is still moving, so the next frame of the screen
    /// will not match this one.
    pub fn moving(&self) -> bool {
        self.coast != (0.0, 0.0)
            || !self.pan_x.settled()
            || !self.pan_y.settled()
            || !self.zoom.settled()
            || self.laser.target.is_some()
            || self.laser.glow > 0.0
            || !self.laser.trail.is_empty()
            || self.flash.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (640, 480);

    /// A hand's 21 points with the wrist at `(x, y)` — fingers up, the
    /// knuckles a tenth of the frame above it — out or folded as asked,
    /// thumb and index tips `gap` apart (of the camera's width).
    fn hand(x: f32, y: f32, out: [bool; 4], gap: f32) -> [[f32; 3]; POINTS] {
        let mut p = [[0.0_f32; 3]; POINTS];
        p[0] = [x, y, 0.0];
        // Each finger: the knuckle, two joints and the tip, straight up
        // when out and curled back toward the wrist when folded.
        for (f, base) in [5_usize, 9, 13, 17].iter().enumerate() {
            let fx = x - 0.045 + 0.03 * f as f32;
            p[*base] = [fx, y - 0.13, 0.0];
            if out[f] {
                p[base + 1] = [fx, y - 0.18, 0.0];
                p[base + 2] = [fx, y - 0.21, 0.0];
                p[base + 3] = [fx, y - 0.24, 0.0];
            } else {
                p[base + 1] = [fx, y - 0.16, 0.0];
                p[base + 2] = [fx, y - 0.12, 0.0];
                p[base + 3] = [fx, y - 0.09, 0.0];
            }
        }
        // The thumb, out to the side, its tip `gap` from the index's.
        p[1] = [x - 0.05, y - 0.03, 0.0];
        p[2] = [x - 0.08, y - 0.07, 0.0];
        p[3] = [x - 0.09, y - 0.11, 0.0];
        p[4] = [p[8][0] - gap, p[8][1], 0.0];
        p
    }

    const OPEN: [bool; 4] = [true; 4];
    const POINT: [bool; 4] = [true, false, false, false];

    /// Readings at 30 a second from `t0`, each the hands `at(t)` gives.
    fn feed(g: &mut Gestures, t0: f64, frames: usize, at: impl Fn(f64) -> Vec<[[f32; 3]; POINTS]>) -> Vec<(Turn, f64)> {
        let mut turns = Vec::new();
        for i in 0..frames {
            let t = t0 + i as f64 / 30.0;
            if let Some(turn) = g.read(&at(t), SIZE, t, true) {
                turns.push((turn, t));
            }
            g.tick(t, 1.0 / 30.0);
        }
        turns
    }

    #[test]
    fn a_pinch_holds_two_frames_before_it_counts_and_lets_go_with_room_to_spare() {
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 3, |_| vec![hand(0.5, 0.7, OPEN, 0.1)]);
        assert!(g.pinches.is_empty(), "apart");
        feed(&mut g, 0.1, 1, |_| vec![hand(0.5, 0.7, OPEN, 0.0)]);
        assert!(g.pinches.is_empty(), "one frame is not a pinch");
        feed(&mut g, 0.2, 2, |_| vec![hand(0.5, 0.7, OPEN, 0.0)]);
        assert_eq!(g.pinches.len(), 1);
        // A little apart again is still the pinch: the hysteresis. The
        // hand's size here is 0.099 of the width, so 0.035 apart is 0.35.
        feed(&mut g, 0.3, 4, |_| vec![hand(0.5, 0.7, OPEN, 0.035)]);
        assert_eq!(g.pinches.len(), 1, "between on and off");
        feed(&mut g, 0.5, 4, |_| vec![hand(0.5, 0.7, OPEN, 0.12)]);
        assert!(g.pinches.is_empty(), "well apart");
    }

    #[test]
    fn a_pinch_moved_drags_the_board_and_eases_out_after() {
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 4, |_| vec![hand(0.5, 0.7, OPEN, 0.0)]);
        // The pinch travels 0.09 of the frame to the right, then holds
        // there while the filter catches up with it.
        let mut moved = 0.0;
        for i in 0..35 {
            let t = 0.2 + i as f64 / 30.0;
            g.read(&[hand(0.5 + 0.01 * i.min(9) as f32, 0.7, OPEN, 0.0)], SIZE, t, true);
            if let Some(m) = g.tick(t, 1.0 / 30.0) {
                moved += m.dx;
            }
        }
        for i in 0..60 {
            if let Some(m) = g.tick(1.4 + i as f64 / 60.0, 1.0 / 60.0) {
                moved += m.dx;
            }
        }
        let want = 0.09 * PAN_GAIN;
        assert!((moved - want).abs() < want * 0.05, "the board follows the hand: {moved} of {want}");
    }

    #[test]
    fn pointing_is_the_laser_and_never_moves_the_board() {
        let mut g = Gestures::default();
        let mut moved = 0.0;
        for i in 0..20 {
            let t = i as f64 / 30.0;
            g.read(&[hand(0.3 + 0.02 * i as f32, 0.7, POINT, 0.1)], SIZE, t, true);
            if let Some(m) = g.tick(t, 1.0 / 30.0) {
                moved += m.dx.abs() + m.dy.abs();
            }
        }
        assert!(g.laser.tip.is_some() && g.laser.glow > 0.0, "{:?}", g.laser);
        assert!(!g.laser.trail.is_empty());
        assert_eq!(moved, 0.0, "the laser is drawn over a board that holds still");
        assert!(g.pinches.is_empty());
    }

    #[test]
    fn two_pinches_pulled_apart_zoom_in() {
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 4, |_| vec![hand(0.4, 0.7, OPEN, 0.0), hand(0.6, 0.7, OPEN, 0.0)]);
        assert_eq!(g.pinches.len(), 2);
        let mut zoom = 1.0;
        for i in 0..35 {
            let t = 0.2 + i as f64 / 30.0;
            let d = 0.01 * i.min(9) as f32;
            g.read(&[hand(0.4 - d, 0.7, OPEN, 0.0), hand(0.6 + d, 0.7, OPEN, 0.0)], SIZE, t, true);
            if let Some(m) = g.tick(t, 1.0 / 30.0) {
                zoom *= m.factor;
            }
        }
        for i in 0..90 {
            if let Some(m) = g.tick(1.4 + i as f64 / 60.0, 1.0 / 60.0) {
                zoom *= m.factor;
            }
        }
        // The pinches went from 0.2 apart to 0.38.
        let want = 0.38 / 0.2;
        assert!((zoom - want).abs() < 0.05, "{zoom} for {want}");
    }

    #[test]
    fn two_pinches_zoom_about_where_they_meet_and_not_about_the_mouse() {
        // Two hands off to the left of the frame: the zoom is about the
        // middle of their pinches, wherever the pointer happens to be.
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 4, |_| vec![hand(0.15, 0.7, OPEN, 0.0), hand(0.35, 0.7, OPEN, 0.0)]);
        let mut about = None;
        for i in 0..20 {
            let t = 0.2 + i as f64 / 30.0;
            let d = 0.01 * i.min(9) as f32;
            g.read(&[hand(0.15 - d, 0.7, OPEN, 0.0), hand(0.35 + d, 0.7, OPEN, 0.0)], SIZE, t, true);
            if let Some(m) = g.tick(t, 1.0 / 30.0)
                && m.factor != 1.0
            {
                about = m.about;
            }
        }
        let [a, b] = g.pinches[..] else {
            panic!("two pinches: {:?}", g.pinches);
        };
        let middle = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let about = about.expect("a zoom says what it is about");
        assert!(dist(about, middle) < 0.01, "{about:?} for {middle:?}");
        // Let go of, the zoom settles about the same place.
        let last = (0..30).filter_map(|i| g.tick(1.0 + i as f64 / 60.0, 1.0 / 60.0)).last();
        assert_eq!(last.and_then(|m| m.about), Some(about));
        // A drag alone has never zoomed, and says nothing about where.
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 4, |_| vec![hand(0.5, 0.7, OPEN, 0.0)]);
        for i in 0..20 {
            let t = 0.2 + i as f64 / 30.0;
            g.read(&[hand(0.5 + 0.01 * i as f32, 0.7, OPEN, 0.0)], SIZE, t, true);
            if let Some(m) = g.tick(t, 1.0 / 30.0) {
                assert_eq!((m.factor, m.about), (1.0, None));
            }
        }
    }

    #[test]
    fn an_open_hand_swept_turns_a_slide_and_a_slow_one_does_not() {
        let sweep = |speed: f32| {
            move |t: f64| {
                let x = (0.8 - speed * (t as f32 - 0.5).max(0.0)).max(0.05);
                vec![hand(x, 0.7, OPEN, 0.1)]
            }
        };
        let mut g = Gestures::default();
        let turns = feed(&mut g, 0.0, 40, sweep(1.5));
        assert_eq!(turns.len(), 1, "{turns:?}");
        assert_eq!(turns[0].0, Turn::Next, "to the left is the next");
        assert!(g.flash.is_none(), "the chevron has gone by the end");
        let mut g = Gestures::default();
        assert!(feed(&mut g, 0.0, 40, sweep(0.3)).is_empty(), "too slow");
        // With no show on, a sweep turns nothing and promises nothing.
        let mut g = Gestures::default();
        for i in 0..40 {
            let t = i as f64 / 30.0;
            assert_eq!(g.read(&sweep(1.5)(t), SIZE, t, false), None);
            g.tick(t, 1.0 / 30.0);
            assert!(g.flash.is_none());
        }
        let mut g = Gestures::default();
        let back = feed(&mut g, 0.0, 40, |t| {
            vec![hand((0.2 + 1.5 * (t as f32 - 0.5).max(0.0)).min(0.95), 0.7, OPEN, 0.1)]
        });
        assert_eq!(back.first().map(|t| t.0), Some(Turn::Prev));
    }

    #[test]
    fn a_sweep_survives_the_hand_blurring_out_for_a_moment() {
        let mut g = Gestures::default();
        let turns = feed(&mut g, 0.0, 40, |t| {
            if (0.6..0.8).contains(&t) {
                vec![]
            } else {
                vec![hand((0.8 - 1.5 * (t as f32 - 0.5).max(0.0)).max(0.05), 0.7, OPEN, 0.1)]
            }
        });
        assert_eq!(turns.len(), 1, "{turns:?}");
    }

    #[test]
    fn a_hand_resting_beside_does_not_keep_the_other_from_sweeping() {
        let mut g = Gestures::default();
        let turns = feed(&mut g, 0.0, 40, |t| {
            vec![
                hand(0.15, 0.8, OPEN, 0.1),
                hand((0.9 - 1.5 * (t as f32 - 0.5).max(0.0)).max(0.3), 0.6, OPEN, 0.1),
            ]
        });
        assert_eq!(turns.len(), 1, "{turns:?}");
    }

    #[test]
    fn a_hand_is_itself_by_where_it_is_whatever_order_it_comes_in() {
        let mut hands = Vec::new();
        let a = hand(0.3, 0.7, OPEN, 0.1);
        let b = hand(0.7, 0.7, OPEN, 0.1);
        follow(&mut hands, &[a, b], 0.0, 1.0 / 30.0, 0.75);
        let first: Vec<Pt> = hands.iter().map(|h| h.wrist).collect();
        follow(&mut hands, &[b, a], 1.0 / 30.0, 1.0 / 30.0, 0.75);
        let again: Vec<Pt> = hands.iter().map(|h| h.wrist).collect();
        assert_eq!(first, again, "the same two, in their places");
        assert_eq!(follow(&mut hands, &[], 1.0, 1.0 / 30.0, 0.75), Vec::<usize>::new());
        assert!(hands.is_empty(), "gone after the grace");
    }

    #[test]
    fn a_drag_let_go_of_at_speed_coasts_on() {
        let mut trail = VecDeque::new();
        for i in 0..10 {
            trail.push_back((i as f64 / 30.0, i as f64 * 40.0, 0.0, true));
        }
        let (vx, vy) = fling(&trail, 10.0 / 30.0);
        assert!((vx - 1200.0).abs() < 1e-6 && vy == 0.0);
        let slow: VecDeque<_> = (0..10).map(|i| (i as f64 / 30.0, i as f64 * 15.0, 0.0, true)).collect();
        assert_eq!(fling(&slow, 10.0 / 30.0), (0.0, 0.0), "450 px/s is a drag, not a throw");
    }

    #[test]
    fn the_fingers_opening_are_not_a_throw() {
        // A hand held still, then the fingers opening: the point between
        // them runs 60 px a frame while they do.
        let mut trail: VecDeque<_> = (0..10).map(|i| (i as f64 / 30.0, 300.0, 0.0, true)).collect();
        for i in 10..14 {
            trail.push_back((i as f64 / 30.0, 300.0 + (i - 9) as f64 * 60.0, 0.0, false));
        }
        assert_eq!(fling(&trail, 14.0 / 30.0), (0.0, 0.0));
        // Opened slowly, long after the hand stopped closing them.
        let stale: VecDeque<_> = (0..10).map(|i| (i as f64 / 30.0, i as f64 * 40.0, 0.0, true)).collect();
        assert_eq!(fling(&stale, 1.0), (0.0, 0.0));
    }

    #[test]
    fn a_hand_letting_go_where_it_stands_leaves_the_board_where_it_is_and_a_throw_glides() {
        // Pinched and held still, then the thumb lets go over five frames.
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 10, |_| vec![hand(0.5, 0.7, OPEN, 0.0)]);
        feed(&mut g, 10.0 / 30.0, 12, |t| {
            let opening = ((t - 10.0 / 30.0) * 30.0).round() as f32;
            vec![hand(0.5, 0.7, OPEN, (opening * 0.03).min(0.15))]
        });
        assert!(g.pinches.is_empty(), "let go of");
        assert_eq!(g.coast, (0.0, 0.0), "no throw");
        // Thrown: moving fast, and letting go on the way.
        let mut g = Gestures::default();
        feed(&mut g, 0.0, 4, |_| vec![hand(0.2, 0.7, OPEN, 0.0)]);
        feed(&mut g, 4.0 / 30.0, 16, |t| {
            let i = ((t - 4.0 / 30.0) * 30.0).round() as f32;
            let gap = ((i - 9.0).max(0.0) * 0.03).min(0.15);
            vec![hand(0.2 + 0.04 * i, 0.7, OPEN, gap)]
        });
        let speed = g.coast.0.hypot(g.coast.1);
        assert!((FLING_MIN..=FLING_MAX).contains(&speed), "a throw coasts: {speed}");
    }
}
