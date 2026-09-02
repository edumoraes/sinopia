//! The brushes: a library of named presets, the body each one carries,
//! the tip a stroke takes into the document, and the ring the pointer
//! shows. Pure.
//!
//! A brush is Sketchbook's, not Photoshop's: it is a thing in a library
//! with a name, and an edit belongs to it — `[` and the bar's slider both
//! write into the brush in the hand, and `reset` takes it back to what it
//! shipped as. The library is session state (§6.2), global to the window
//! and never on disk yet: a brush belongs to the person drawing, not to
//! the board.
//!
//! Its body is the whole of Brush Properties. Three of it reach the
//! canvas — [`Property::honored`] is the list — and the rest are the
//! stamp engine's, kept on the brush because that is where they belong.

use crate::doc::{Path, Stroke};
use crate::editor::PEN_WIDTH;
use crate::scene::{Prim, Rgba, polyline_prims};

/// Brush size in world units (logical px at zoom 1): the diameter.
pub const SIZE_MIN: f64 = 1.0;
pub const SIZE_MAX: f64 = 500.0;
/// What `{` and `}` change the hardness by.
pub const HARDNESS_STEP: f64 = 0.25;
/// Sketchbook's own band for the gap between two stamps, in tip widths.
#[allow(dead_code)] // named for `Property::range`; the stamp engine reads it
pub const SPACING_MIN: f64 = 0.1;
#[allow(dead_code)] // named for `Property::range`; the stamp engine reads it
pub const SPACING_MAX: f64 = 10.0;

/// What turns a stamp as the stroke goes. Sketchbook's Rotation Dynamics:
/// a pattern either keeps its angle, follows the stroke, or is turned by
/// the way the stylus is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)] // the Stamp section offers them; the stamp engine turns by them
pub enum Dynamics {
    #[default]
    None,
    /// For patterns that have to follow the stroke.
    ToStroke,
    Tilt,
    TiltAndRoll,
}

/// How much of each property the stroke throws away at random, 0–1.
/// Sketchbook's Randomness section: the same five it varies.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Jitter {
    pub size: f64,
    pub opacity: f64,
    pub flow: f64,
    pub rotation: f64,
    pub spacing: f64,
}

/// How much of each property the pen's pressure drives, 0–1: 0 is a
/// property pressure never touches, 1 one it drives from nothing to the
/// value the brush names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pressure {
    pub size: f64,
    pub opacity: f64,
    pub flow: f64,
}

impl Default for Pressure {
    fn default() -> Pressure {
        Pressure {
            size: 1.0,
            opacity: 0.0,
            flow: 0.0,
        }
    }
}

/// A brush's whole body, as Sketchbook's Brush Properties lays it out.
///
/// Three of these reach the canvas today — `size`, `opacity` and
/// `hardness` are what [`Tip`] carries and what `scene` composites with.
/// The rest are the stamp engine's, and are kept because a brush is the
/// place they belong: a preset that says how far apart its stamps sit
/// keeps saying it while the code that honours it is written.
/// [`Property::honored`] is the list, and the only one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    pub size: f64,
    /// The stroke's opacity as one shape, 0–1.
    pub opacity: f64,
    /// How fast the paint comes out, 0–1: what one stamp lays, which
    /// builds up toward `opacity` where a stroke crosses itself.
    pub flow: f64,
    /// The gap between two stamps, in tip widths.
    pub spacing: f64,
    /// 1 is a round tip; less flattens it, and only then can it be turned.
    pub roundness: f64,
    /// The tip's own angle, in degrees.
    pub rotation: f64,
    pub dynamics: Dynamics,
    /// How much of the radius is crisp, 0–1: 1 is a pencil's edge, 0
    /// fades from the center out. Sketchbook's Edge.
    pub hardness: f64,
    /// How far the texture bites into the nib, 0–1.
    pub texture_depth: f64,
    pub jitter: Jitter,
    pub pressure: Pressure,
}

impl Default for Brush {
    fn default() -> Brush {
        Brush {
            size: 16.0,
            opacity: 1.0,
            flow: 1.0,
            spacing: 1.2,
            roundness: 1.0,
            rotation: 0.0,
            dynamics: Dynamics::None,
            hardness: 0.5,
            texture_depth: 0.0,
            jitter: Jitter::default(),
            pressure: Pressure::default(),
        }
    }
}

/// Where a property sits in Brush Properties' Advanced tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Pressure,
    Stamp,
    Nib,
    Randomness,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Pressure,
        Section::Stamp,
        Section::Nib,
        Section::Randomness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Pressure => "Pressure",
            Section::Stamp => "Stamp",
            Section::Nib => "Nib",
            Section::Randomness => "Randomness",
        }
    }
}

/// One slider on the brush's body: everything the panels need to draw it
/// and to move it, so neither of them has to know a field's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    Size,
    Opacity,
    Flow,
    Spacing,
    Roundness,
    Rotation,
    Hardness,
    TextureDepth,
    JitterSize,
    JitterOpacity,
    JitterFlow,
    JitterRotation,
    JitterSpacing,
}

/// How a value is written out under its slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// World units, whole numbers: the size.
    Px,
    Percent,
    /// Tip widths, one decimal: the spacing.
    Widths,
    Degrees,
}

impl Property {
    pub const ALL: [Property; 13] = [
        Property::Size,
        Property::Opacity,
        Property::Flow,
        Property::Spacing,
        Property::Roundness,
        Property::Rotation,
        Property::Hardness,
        Property::TextureDepth,
        Property::JitterSize,
        Property::JitterOpacity,
        Property::JitterFlow,
        Property::JitterRotation,
        Property::JitterSpacing,
    ];

    /// What the bar shows without being opened — the pair a brush is
    /// usually judged by, and the two the canvas answers to at once.
    pub const BASIC: [Property; 2] = [Property::Size, Property::Opacity];

    pub fn label(self) -> &'static str {
        match self {
            Property::Size | Property::JitterSize => "Size",
            Property::Opacity | Property::JitterOpacity => "Opacity",
            Property::Flow | Property::JitterFlow => "Flow",
            Property::Spacing | Property::JitterSpacing => "Spacing",
            Property::Roundness => "Roundness",
            Property::Rotation | Property::JitterRotation => "Rotation",
            Property::Hardness => "Edge",
            Property::TextureDepth => "Depth",
        }
    }

    pub fn section(self) -> Section {
        match self {
            Property::Size | Property::Opacity | Property::Flow => Section::Pressure,
            Property::Spacing | Property::Roundness | Property::Rotation => Section::Stamp,
            Property::Hardness | Property::TextureDepth => Section::Nib,
            Property::JitterSize
            | Property::JitterOpacity
            | Property::JitterFlow
            | Property::JitterRotation
            | Property::JitterSpacing => Section::Randomness,
        }
    }

    pub fn range(self) -> (f64, f64) {
        match self {
            Property::Size => (SIZE_MIN, SIZE_MAX),
            Property::Spacing => (SPACING_MIN, SPACING_MAX),
            Property::Rotation => (0.0, 360.0),
            _ => (0.0, 1.0),
        }
    }

    /// Whether the canvas paints with it yet. The three that do are the
    /// ones [`Tip`] carries; the rest wait on the stamp engine.
    pub fn honored(self) -> bool {
        matches!(
            self,
            Property::Size | Property::Opacity | Property::Hardness
        )
    }

    pub fn get(self, b: &Brush) -> f64 {
        match self {
            Property::Size => b.size,
            Property::Opacity => b.opacity,
            Property::Flow => b.flow,
            Property::Spacing => b.spacing,
            Property::Roundness => b.roundness,
            Property::Rotation => b.rotation,
            Property::Hardness => b.hardness,
            Property::TextureDepth => b.texture_depth,
            Property::JitterSize => b.jitter.size,
            Property::JitterOpacity => b.jitter.opacity,
            Property::JitterFlow => b.jitter.flow,
            Property::JitterRotation => b.jitter.rotation,
            Property::JitterSpacing => b.jitter.spacing,
        }
    }

    /// Writes `v` into the one field the property names, held inside its
    /// own range. A value that is not a number reads as the bottom of it,
    /// so a slider dividing by nothing cannot poison the brush.
    pub fn set(self, b: &mut Brush, v: f64) {
        let (lo, hi) = self.range();
        let v = if v.is_nan() { lo } else { v.clamp(lo, hi) };
        match self {
            Property::Size => b.size = v,
            Property::Opacity => b.opacity = v,
            Property::Flow => b.flow = v,
            Property::Spacing => b.spacing = v,
            Property::Roundness => b.roundness = v,
            Property::Rotation => b.rotation = v,
            Property::Hardness => b.hardness = v,
            Property::TextureDepth => b.texture_depth = v,
            Property::JitterSize => b.jitter.size = v,
            Property::JitterOpacity => b.jitter.opacity = v,
            Property::JitterFlow => b.jitter.flow = v,
            Property::JitterRotation => b.jitter.rotation = v,
            Property::JitterSpacing => b.jitter.spacing = v,
        }
    }

    /// How a slider's travel maps onto the value. Size runs from 1 to
    /// 500, and a handle moving straight through that would spend nine
    /// tenths of its travel on widths nobody draws with — so it moves
    /// over the cube of its travel, and the sizes people reach for get
    /// the first third of the rail.
    fn curve(self) -> f64 {
        match self {
            Property::Size => 3.0,
            _ => 1.0,
        }
    }

    /// Where the value sits in its own range, 0–1: how far along its
    /// slider the handle is.
    pub fn fraction(self, b: &Brush) -> f64 {
        let (lo, hi) = self.range();
        ((self.get(b) - lo) / (hi - lo))
            .clamp(0.0, 1.0)
            .powf(1.0 / self.curve())
    }

    /// The other way round: a handle dropped `f` of the way along.
    pub fn set_fraction(self, b: &mut Brush, f: f64) {
        let (lo, hi) = self.range();
        self.set(b, lo + f.clamp(0.0, 1.0).powf(self.curve()) * (hi - lo));
    }

    /// What is written under the slider.
    pub fn format(self, v: f64) -> String {
        match self.unit() {
            Unit::Px => format!("{}", v.round() as i64),
            Unit::Percent => format!("{}%", (v * 100.0).round() as i64),
            Unit::Widths => format!("{v:.1}"),
            Unit::Degrees => format!("{}°", v.round() as i64),
        }
    }

    fn unit(self) -> Unit {
        match self {
            Property::Size => Unit::Px,
            Property::Spacing => Unit::Widths,
            Property::Rotation => Unit::Degrees,
            _ => Unit::Percent,
        }
    }
}

/// What `]` adds at `size` — Photoshop's steps: one below ten, ten below
/// a hundred, twenty-five below two hundred, fifty below three hundred,
/// a hundred past that.
pub fn size_step(size: f64) -> f64 {
    if size < 10.0 {
        1.0
    } else if size < 100.0 {
        10.0
    } else if size < 200.0 {
        25.0
    } else if size < 300.0 {
        50.0
    } else {
        100.0
    }
}

impl Brush {
    pub fn grow(&mut self) {
        self.size = (self.size + size_step(self.size)).min(SIZE_MAX);
    }

    /// Steps by what a grow from just below would have added, so a grow
    /// and a shrink undo each other at the band edges too.
    pub fn shrink(&mut self) {
        self.size = (self.size - size_step(self.size - 1.0)).max(SIZE_MIN);
    }

    pub fn harder(&mut self) {
        self.hardness = (self.hardness + HARDNESS_STEP).min(1.0);
    }

    pub fn softer(&mut self) {
        self.hardness = (self.hardness - HARDNESS_STEP).max(0.0);
    }

    /// `1`–`9` are 10%–90%, `0` is 100%; anything else is not a digit
    /// and changes nothing.
    pub fn set_opacity_digit(&mut self, digit: u8) {
        self.opacity = match digit {
            0 => 1.0,
            1..=9 => f64::from(digit) / 10.0,
            _ => return,
        };
    }

    pub fn tip(&self) -> Tip {
        Tip {
            width: self.size,
            opacity: self.opacity,
            hardness: self.hardness,
        }
    }
}

/// What a stroke is drawn with, taken at the press and written into the
/// path on release: the pencil's constant or the brush's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tip {
    /// World units.
    pub width: f64,
    pub opacity: f64,
    pub hardness: f64,
}

impl Tip {
    pub const PENCIL: Tip = Tip {
        width: PEN_WIDTH,
        opacity: 1.0,
        hardness: 1.0,
    };

    pub fn of(path: &Path) -> Tip {
        Tip {
            width: path.width,
            opacity: path.opacity,
            hardness: path.hardness,
        }
    }

    /// The tip one stroke of a `paint` was laid with. A raster layer keeps
    /// them stroke by stroke, so each one is composited as it was made.
    pub fn of_stroke(s: &Stroke) -> Tip {
        Tip {
            width: s.width,
            opacity: s.opacity,
            hardness: s.hardness,
        }
    }

    /// A crisp, opaque stroke draws straight onto the frame; anything
    /// else has to be composited as one shape, or its segments would
    /// darken wherever they overlap.
    pub fn is_direct(&self) -> bool {
        self.opacity >= 1.0 && self.hardness >= 1.0
    }
}

/// One named brush in the library, and what it shipped as — so an edit
/// can always be taken back without knowing which brush it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub brush: Brush,
    factory: Brush,
}

impl Preset {
    fn new(name: &str, brush: Brush) -> Preset {
        Preset {
            name: name.to_owned(),
            brush,
            factory: brush,
        }
    }

    /// Whether it has been moved off what it shipped as. The properties
    /// bar says so, and offers to take it back.
    pub fn edited(&self) -> bool {
        self.brush != self.factory
    }

    pub fn reset(&mut self) {
        self.brush = self.factory;
    }
}

/// A shelf of the library. The palette shows one of these at a time —
/// Sketchbook's pinned set.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub name: String,
    pub presets: Vec<Preset>,
}

/// Every brush there is, in sets, plus which one the palette shows and
/// which one is in the hand. Session state (§6.2), global to the window
/// like the brush it replaced — a brush belongs to the person drawing,
/// not to the board. Nothing of it reaches disk yet: an edit lasts as
/// long as the process.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    sets: Vec<Set>,
    pinned: usize,
    /// The brush in the hand: which set, and which brush of it. It is
    /// not the same as the pinned set — pinning changes what is on show,
    /// never what is painting.
    selected: (usize, usize),
}

impl Library {
    pub fn sets(&self) -> &[Set] {
        &self.sets
    }

    pub fn pinned(&self) -> &Set {
        &self.sets[self.pinned]
    }

    pub fn pinned_index(&self) -> usize {
        self.pinned
    }

    pub fn pin(&mut self, set: usize) {
        if set < self.sets.len() {
            self.pinned = set;
        }
    }

    pub fn selected(&self) -> (usize, usize) {
        self.selected
    }

    /// Takes up a brush, if there is one in that seat.
    pub fn select(&mut self, set: usize, index: usize) {
        if self.sets.get(set).is_some_and(|s| index < s.presets.len()) {
            self.selected = (set, index);
        }
    }

    fn preset(&self) -> &Preset {
        let (s, i) = self.selected;
        &self.sets[s].presets[i]
    }

    fn preset_mut(&mut self) -> &mut Preset {
        let (s, i) = self.selected;
        &mut self.sets[s].presets[i]
    }

    pub fn name(&self) -> &str {
        &self.preset().name
    }

    /// What the canvas paints with.
    pub fn brush(&self) -> &Brush {
        &self.preset().brush
    }

    /// An edit goes into the brush itself, as Sketchbook does it — the
    /// keyboard's `[` and the bar's slider both land here.
    pub fn brush_mut(&mut self) -> &mut Brush {
        &mut self.preset_mut().brush
    }

    pub fn edited(&self) -> bool {
        self.preset().edited()
    }

    pub fn reset(&mut self) {
        self.preset_mut().reset();
    }
}

impl Default for Library {
    /// The brushes the binary ships with. They differ in the three the
    /// canvas answers to, so picking one changes the ink; what they say
    /// about flow and spacing is true of them and waits on the engine.
    fn default() -> Library {
        let b = |size, opacity, hardness, flow, spacing| Brush {
            size,
            opacity,
            hardness,
            flow,
            spacing,
            ..Brush::default()
        };
        Library {
            sets: vec![
                Set {
                    name: "Essentials".to_owned(),
                    presets: vec![
                        Preset::new("Pencil", b(2.0, 1.0, 1.0, 1.0, 1.2)),
                        Preset::new("Ink Pen", b(5.0, 1.0, 1.0, 1.0, 0.5)),
                        Preset::new("Marker", b(24.0, 0.65, 0.9, 0.8, 0.4)),
                        Preset::new("Highlighter", b(36.0, 0.3, 1.0, 1.0, 0.3)),
                        Preset::new("Hard Round", b(40.0, 1.0, 1.0, 1.0, 0.25)),
                        Preset::new("Soft Round", b(48.0, 1.0, 0.25, 1.0, 0.25)),
                        Preset::new("Airbrush", b(80.0, 0.4, 0.0, 0.35, 0.1)),
                    ],
                },
                Set {
                    name: "Paint".to_owned(),
                    presets: vec![
                        Preset::new("Dry Edge", b(30.0, 0.85, 0.75, 0.9, 0.6)),
                        Preset::new("Glaze", b(64.0, 0.35, 0.15, 0.4, 0.2)),
                        Preset::new("Blot", b(90.0, 0.9, 0.4, 1.0, 1.5)),
                        Preset::new("Wash", b(120.0, 0.2, 0.0, 0.25, 0.2)),
                    ],
                },
            ],
            pinned: 0,
            selected: (0, 0),
        }
    }
}

/// Points around the ring.
const RING_POINTS: usize = 48;

/// The pointer's ring: a one-logical-pixel circle of `radius` px around
/// `center`, so the brush's size shows before it paints.
pub fn ring_prims(center: (f32, f32), radius: f32, scale: f32, color: Rgba) -> Vec<Prim> {
    let points: Vec<(f32, f32)> = (0..=RING_POINTS)
        .map(|i| {
            let a = i as f32 / RING_POINTS as f32 * std::f32::consts::TAU;
            (center.0 + radius * a.cos(), center.1 + radius * a.sin())
        })
        .collect();
    polyline_prims(&points, 0.5 * scale, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Path;
    use crate::editor::PEN_WIDTH;
    use crate::scene::KIND_SEGMENT;

    #[test]
    fn defaults_are_a_mid_sized_soft_opaque_brush() {
        let b = Brush::default();
        assert_eq!((b.size, b.opacity, b.hardness), (16.0, 1.0, 0.5));
    }

    #[test]
    fn grow_and_shrink_follow_photoshop_steps_within_bounds() {
        let mut b = Brush {
            size: 9.0,
            ..Brush::default()
        };
        b.grow();
        assert_eq!(b.size, 10.0, "by one below ten");
        b.grow();
        assert_eq!(b.size, 20.0, "by ten below a hundred");
        b.size = 175.0;
        b.grow();
        assert_eq!(b.size, 200.0, "by twenty-five below two hundred");
        b.grow();
        assert_eq!(b.size, 250.0, "by fifty below three hundred");
        b.grow();
        b.grow();
        assert_eq!(b.size, 400.0, "by a hundred past it");
        b.grow();
        b.grow();
        assert_eq!(b.size, SIZE_MAX, "and never past the largest");

        b.size = 10.0;
        b.shrink();
        assert_eq!(b.size, 9.0, "back down by one");
        b.size = 100.0;
        b.shrink();
        assert_eq!(b.size, 90.0);
        b.size = 200.0;
        b.shrink();
        assert_eq!(b.size, 175.0, "a grow is undone by a shrink");
        b.size = 1.0;
        b.shrink();
        assert_eq!(b.size, SIZE_MIN, "and never below the smallest");
    }

    #[test]
    fn harder_and_softer_step_a_quarter_and_clamp() {
        let mut b = Brush::default();
        b.harder();
        assert_eq!(b.hardness, 0.75);
        b.harder();
        b.harder();
        assert_eq!(b.hardness, 1.0);
        b.softer();
        b.softer();
        b.softer();
        b.softer();
        assert_eq!(b.hardness, 0.0);
        b.softer();
        assert_eq!(b.hardness, 0.0);
    }

    #[test]
    fn opacity_digits_map_one_to_nine_and_zero() {
        let mut b = Brush::default();
        b.set_opacity_digit(1);
        assert_eq!(b.opacity, 0.1);
        b.set_opacity_digit(5);
        assert_eq!(b.opacity, 0.5);
        b.set_opacity_digit(9);
        assert_eq!(b.opacity, 0.9);
        b.set_opacity_digit(0);
        assert_eq!(b.opacity, 1.0);
        b.set_opacity_digit(5);
        b.set_opacity_digit(12);
        assert_eq!(b.opacity, 0.5, "not a digit: ignored");
    }

    #[test]
    fn tip_of_a_pencil_and_of_a_brush() {
        assert_eq!(
            Tip::PENCIL,
            Tip {
                width: PEN_WIDTH,
                opacity: 1.0,
                hardness: 1.0
            }
        );
        assert!(Tip::PENCIL.is_direct(), "the pencil needs no compositing");
        let soft = Brush::default().tip();
        assert_eq!((soft.width, soft.opacity, soft.hardness), (16.0, 1.0, 0.5));
        assert!(!soft.is_direct(), "a soft edge has to be composited");
        let hard = Brush {
            hardness: 1.0,
            ..Brush::default()
        };
        assert!(hard.tip().is_direct());
        let faint = Brush {
            hardness: 1.0,
            opacity: 0.5,
            ..Brush::default()
        };
        assert!(!faint.tip().is_direct(), "so does translucency");
    }

    #[test]
    fn tip_of_a_path_reads_its_fields() {
        let p = Path {
            id: "p".into(),
            layer: "l".into(),
            curves: vec![],
            stroke: "#000".into(),
            width: 7.0,
            opacity: 0.25,
            hardness: 0.75,
            rotation: 0.0,
        };
        assert_eq!(
            Tip::of(&p),
            Tip {
                width: 7.0,
                opacity: 0.25,
                hardness: 0.75
            }
        );
    }

    #[test]
    fn ring_is_a_closed_polyline_around_the_centre() {
        let (cx, cy, r) = (100.0, 50.0, 12.0);
        let prims = ring_prims((cx, cy), r, 2.0, [0.0, 0.0, 0.0, 1.0]);
        assert!(prims.len() >= 24, "smooth enough to read as a circle");
        for p in &prims {
            assert_eq!(p.kind, KIND_SEGMENT);
            assert_eq!(p.radius, 1.0, "one logical pixel wide on a 2x display");
            for (x, y) in [(p.geom[0], p.geom[1]), (p.geom[2], p.geom[3])] {
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                assert!((d - r).abs() < 1e-3, "{d} off the radius");
            }
        }
        let first = prims[0].geom;
        let last = prims[prims.len() - 1].geom;
        assert!(
            (first[0] - last[2]).abs() < 1e-3 && (first[1] - last[3]).abs() < 1e-3,
            "the ring closes"
        );
    }

    #[test]
    fn a_factory_brush_carries_sketchbooks_defaults() {
        let b = Brush::default();
        assert_eq!((b.size, b.opacity, b.hardness), (16.0, 1.0, 0.5));
        assert_eq!(b.flow, 1.0, "full flow until told otherwise");
        assert_eq!(b.spacing, 1.2, "Sketchbook's default for the pencil");
        assert_eq!(b.roundness, 1.0, "round: rotation and tilt do nothing");
        assert_eq!(b.rotation, 0.0);
        assert_eq!(b.dynamics, Dynamics::None);
        assert_eq!(b.texture_depth, 0.0, "no texture over the nib");
        assert_eq!(b.jitter, Jitter::default(), "nothing random");
        assert_eq!(b.pressure, Pressure::default());
    }

    #[test]
    fn randomness_and_pressure_are_off_by_default() {
        assert_eq!(
            Jitter::default(),
            Jitter {
                size: 0.0,
                opacity: 0.0,
                flow: 0.0,
                rotation: 0.0,
                spacing: 0.0,
            }
        );
        assert_eq!(
            Pressure::default(),
            Pressure {
                size: 1.0,
                opacity: 0.0,
                flow: 0.0,
            },
            "the pen's pressure drives the size, as every drawing app does"
        );
    }

    #[test]
    fn every_property_reads_back_what_it_wrote() {
        for p in Property::ALL {
            let mut b = Brush::default();
            let (lo, hi) = p.range();
            let mid = (lo + hi) / 2.0;
            p.set(&mut b, mid);
            assert_eq!(p.get(&b), mid, "{p:?} did not keep its own value");
        }
    }

    #[test]
    fn each_property_writes_only_its_own_field() {
        for p in Property::ALL {
            let mut b = Brush::default();
            let (lo, hi) = p.range();
            p.set(&mut b, (lo + hi) / 2.0);
            let changed: Vec<Property> = Property::ALL
                .iter()
                .copied()
                .filter(|&q| q.get(&b) != q.get(&Brush::default()))
                .collect();
            assert!(
                changed.iter().all(|&q| q == p),
                "{p:?} also moved {changed:?}"
            );
        }
    }

    #[test]
    fn a_property_clamps_to_its_own_range() {
        for p in Property::ALL {
            let (lo, hi) = p.range();
            assert!(lo < hi, "{p:?} has an empty range");
            let mut b = Brush::default();
            p.set(&mut b, hi + 1000.0);
            assert_eq!(p.get(&b), hi, "{p:?} went past its top");
            p.set(&mut b, lo - 1000.0);
            assert_eq!(p.get(&b), lo, "{p:?} went under its bottom");
            p.set(&mut b, f64::NAN);
            assert_eq!(p.get(&b), lo, "{p:?} took a number that is not one");
        }
    }

    #[test]
    fn only_what_the_engine_paints_with_is_honored() {
        for p in Property::ALL {
            let honored = matches!(p, Property::Size | Property::Opacity | Property::Hardness);
            assert_eq!(p.honored(), honored, "{p:?}");
        }
        assert!(
            Property::ALL.iter().any(|p| !p.honored()),
            "the stamp engine is still ahead"
        );
    }

    #[test]
    fn every_property_has_a_section_and_a_label() {
        for p in Property::ALL {
            assert!(!p.label().is_empty(), "{p:?}");
        }
        assert_eq!(Property::Size.section(), Section::Pressure);
        assert_eq!(Property::Spacing.section(), Section::Stamp);
        assert_eq!(Property::Hardness.section(), Section::Nib);
        assert_eq!(Property::JitterSize.section(), Section::Randomness);
        for s in Section::ALL {
            assert!(
                Property::ALL.iter().any(|p| p.section() == s),
                "{s:?} has no property under it"
            );
        }
    }

    #[test]
    fn the_basic_row_is_the_pair_a_brush_is_usually_judged_by() {
        assert_eq!(Property::BASIC, [Property::Size, Property::Opacity]);
        for p in Property::BASIC {
            assert!(p.honored(), "the bar must not open on a dead control");
        }
    }

    #[test]
    fn a_property_reads_the_way_sketchbook_writes_it() {
        assert_eq!(Property::Size.format(24.0), "24");
        assert_eq!(Property::Size.format(1.0), "1");
        assert_eq!(Property::Opacity.format(0.7), "70%");
        assert_eq!(Property::Opacity.format(1.0), "100%");
        assert_eq!(Property::Hardness.format(0.5), "50%");
        assert_eq!(Property::Roundness.format(1.0), "100%");
        assert_eq!(Property::Spacing.format(1.2), "1.2");
        assert_eq!(Property::Rotation.format(45.0), "45°");
    }

    #[test]
    fn a_fraction_is_where_a_property_sits_in_its_own_range() {
        let mut b = Brush::default();
        Property::Opacity.set(&mut b, 0.25);
        assert_eq!(Property::Opacity.fraction(&b), 0.25);
        Property::Spacing.set(&mut b, 0.1);
        assert_eq!(Property::Spacing.fraction(&b), 0.0, "at the bottom");
        Property::Spacing.set_fraction(&mut b, 1.0);
        assert_eq!(Property::Spacing.get(&b), 10.0, "and back up to the top");
    }

    #[test]
    fn a_fresh_library_opens_on_a_working_set_of_brushes() {
        let lib = Library::default();
        assert!(lib.sets().len() >= 2, "pinning needs somewhere to go");
        assert!(lib.pinned().presets.len() >= 4, "a palette worth showing");
        assert_eq!(lib.selected(), (0, 0), "the first brush of the first set");
        assert_eq!(*lib.brush(), lib.pinned().presets[0].brush);
        assert!(!lib.name().is_empty());
    }

    #[test]
    fn every_built_in_brush_is_named_and_inside_its_own_ranges() {
        let lib = Library::default();
        for set in lib.sets() {
            assert!(!set.name.is_empty());
            let mut seen = Vec::new();
            for p in &set.presets {
                assert!(!p.name.is_empty(), "{} has a nameless brush", set.name);
                assert!(!seen.contains(&&p.name), "{} twice in {}", p.name, set.name);
                seen.push(&p.name);
                for prop in Property::ALL {
                    let (lo, hi) = prop.range();
                    let v = prop.get(&p.brush);
                    assert!((lo..=hi).contains(&v), "{}: {prop:?} is {v}", p.name);
                }
            }
        }
    }

    #[test]
    fn the_built_in_brushes_lay_different_ink() {
        let lib = Library::default();
        let mut tips: Vec<Tip> = lib
            .sets()
            .iter()
            .flat_map(|s| s.presets.iter().map(|p| p.brush.tip()))
            .collect();
        let all = tips.len();
        tips.dedup_by(|a, b| a == b);
        tips.sort_by(|a, b| a.width.total_cmp(&b.width));
        tips.dedup_by(|a, b| a == b);
        assert_eq!(tips.len(), all, "two brushes that paint the same are one");
    }

    #[test]
    fn editing_writes_into_the_brush_in_hand_and_it_stays_there() {
        let mut lib = Library::default();
        lib.select(0, 1);
        lib.brush_mut().size = 123.0;
        assert_eq!(lib.brush().size, 123.0);
        lib.select(0, 0);
        assert_ne!(lib.brush().size, 123.0, "the other brush is untouched");
        lib.select(0, 1);
        assert_eq!(lib.brush().size, 123.0, "and the edit was kept");
    }

    #[test]
    fn reset_puts_a_brush_back_the_way_it_shipped() {
        let mut lib = Library::default();
        let shipped = *lib.brush();
        lib.brush_mut().size = 321.0;
        lib.brush_mut().hardness = 0.0;
        assert!(lib.edited(), "the brush is off its factory settings");
        lib.reset();
        assert_eq!(*lib.brush(), shipped);
        assert!(!lib.edited());
    }

    #[test]
    fn pinning_another_set_leaves_the_brush_in_hand_alone() {
        let mut lib = Library::default();
        let held = *lib.brush();
        lib.pin(1);
        assert_eq!(lib.pinned_index(), 1);
        assert_eq!(lib.pinned().name, lib.sets()[1].name);
        assert_eq!(*lib.brush(), held, "the palette changed, not the hand");
        assert_eq!(lib.selected(), (0, 0));
    }

    #[test]
    fn a_seat_that_is_not_there_is_refused() {
        let mut lib = Library::default();
        let held = lib.selected();
        lib.select(99, 0);
        lib.select(0, 99);
        assert_eq!(lib.selected(), held);
        lib.pin(99);
        assert_eq!(lib.pinned_index(), 0);
    }

    #[test]
    fn the_size_slider_spends_its_travel_on_the_sizes_people_draw_with() {
        let mut b = Brush::default();
        Property::Size.set_fraction(&mut b, 0.5);
        assert!(
            (10.0..100.0).contains(&b.size),
            "half the travel must not be half of 500: {}",
            b.size
        );
        let f = Property::Size.fraction(&b);
        assert!((f - 0.5).abs() < 1e-9, "and it is still its own inverse: {f}");

        let mut c = Brush::default();
        Property::Opacity.set_fraction(&mut c, 0.5);
        assert_eq!(c.opacity, 0.5, "a fraction is a fraction everywhere else");
    }

    #[test]
    fn size_moves_over_photoshop_steps_however_it_is_asked() {
        // The slider is linear over the same band the brackets step through.
        let mut b = Brush::default();
        Property::Size.set_fraction(&mut b, 0.0);
        assert_eq!(b.size, SIZE_MIN);
        Property::Size.set_fraction(&mut b, 1.0);
        assert_eq!(b.size, SIZE_MAX);
    }
}
