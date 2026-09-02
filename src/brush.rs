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

use serde::{Deserialize, Serialize};

use crate::doc::{Path, Stamp, Stroke};
use crate::editor::PEN_WIDTH;
use crate::scene::{Prim, Rgba, polyline_prims};

/// Brush size in world units (logical px at zoom 1): the diameter. The
/// top is the widest brush Sketchbook's own sets carry — a 350-unit
/// radius — so nothing that ships is clamped on the way in.
pub const SIZE_MIN: f64 = 1.0;
pub const SIZE_MAX: f64 = 700.0;
/// What `{` and `}` change the hardness by.
pub const HARDNESS_STEP: f64 = 0.25;
/// Sketchbook's own band for the gap between two stamps, in tip widths.
pub const SPACING_MIN: f64 = 0.1;
pub const SPACING_MAX: f64 = 10.0;

/// The shape of the tip's own falloff, from its middle to its edge.
/// Sketchbook picks one of four for every brush, and it is not the same
/// question as hardness: hardness says how much of the radius the ramp
/// takes, the profile says what the ramp does over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Profile {
    /// The default: a plain ramp.
    #[default]
    #[serde(rename = "regularSolid")]
    RegularSolid,
    /// Falls away from the middle the whole way: the airbrush's cloud.
    #[serde(rename = "airbrush")]
    Airbrush,
    /// Comes to a point.
    #[serde(rename = "sharp")]
    Sharp,
    /// Flat to the edge, then over.
    #[serde(rename = "hardSolid")]
    HardSolid,
}

impl Profile {
    #[allow(dead_code)] // the tests hold every shipped brush to it
    pub const ALL: [Profile; 4] = [
        Profile::RegularSolid,
        Profile::Airbrush,
        Profile::Sharp,
        Profile::HardSolid,
    ];
}

/// What turns a stamp as the stroke goes. Sketchbook's Rotation Dynamics:
/// a pattern either keeps its angle, follows the stroke, or is turned by
/// the way the stylus is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    pub profile: Profile,
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
            profile: Profile::RegularSolid,
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
    /// A quantity in the property's own unit, to one decimal: the gap
    /// between two stamps in tip widths, or how far a randomness throws
    /// the thing it varies.
    Amount,
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

    /// Randomness is not a fraction: Sketchbook varies a property by an
    /// amount in that property's own units, and the sets that came with
    /// it use the whole of these bands — a rotation thrown half a turn
    /// either way, a radius thrown twenty units.
    pub fn range(self) -> (f64, f64) {
        match self {
            Property::Size => (SIZE_MIN, SIZE_MAX),
            Property::Spacing => (SPACING_MIN, SPACING_MAX),
            Property::Rotation => (0.0, 360.0),
            Property::JitterSize | Property::JitterFlow => (0.0, 20.0),
            Property::JitterOpacity | Property::JitterSpacing => (0.0, 5.0),
            Property::JitterRotation => (0.0, 180.0),
            _ => (0.0, 1.0),
        }
    }

    /// Whether the canvas paints with it yet: the ones [`Tip`] carries,
    /// its [`Stamp`] included. The rest wait on the stamp engine.
    pub fn honored(self) -> bool {
        matches!(
            self,
            Property::Size
                | Property::Opacity
                | Property::Hardness
                | Property::Flow
                | Property::Spacing
                | Property::Roundness
                | Property::Rotation
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
            Unit::Amount => format!("{v:.1}"),
            Unit::Degrees => format!("{}°", v.round() as i64),
        }
    }

    fn unit(self) -> Unit {
        match self {
            Property::Size => Unit::Px,
            Property::Spacing
            | Property::JitterSize
            | Property::JitterFlow
            | Property::JitterOpacity
            | Property::JitterSpacing => Unit::Amount,
            Property::Rotation | Property::JitterRotation => Unit::Degrees,
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
            stamp: Some(Stamp {
                spacing: self.spacing,
                roundness: self.roundness,
                rotation: self.rotation,
                flow: self.flow,
            }),
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
    /// The nib, when the stroke is stamped. A pencil sweeps and has
    /// none.
    pub stamp: Option<Stamp>,
}

impl Tip {
    pub const PENCIL: Tip = Tip {
        width: PEN_WIDTH,
        opacity: 1.0,
        hardness: 1.0,
        stamp: None,
    };

    pub fn of(path: &Path) -> Tip {
        Tip {
            width: path.width,
            opacity: path.opacity,
            hardness: path.hardness,
            stamp: path.stamp,
        }
    }

    /// The tip one stroke of a `paint` was laid with. A raster layer keeps
    /// them stroke by stroke, so each one is composited as it was made.
    pub fn of_stroke(s: &Stroke) -> Tip {
        Tip {
            width: s.width,
            opacity: s.opacity,
            hardness: s.hardness,
            stamp: s.stamp,
        }
    }

    /// A crisp, opaque stroke that covers with one dab draws straight
    /// onto the frame; anything else has to be composited as one shape.
    /// A swept stroke would otherwise darken wherever its spans
    /// overlap, and a stamped one would build onto the board itself
    /// instead of into its own pile.
    pub fn is_direct(&self) -> bool {
        self.opacity >= 1.0
            && self.hardness >= 1.0
            && self.stamp.is_none_or(|s| s.flow >= 1.0)
    }
}

/// One named brush in the library, and what it shipped as — so an edit
/// can always be taken back without knowing which brush it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub brush: Brush,
    /// Its cell of the icon sheet: the art Sketchbook draws it with.
    pub icon: u16,
    /// Whether the brush is told apart by a shape or a texture of its
    /// own. Two thirds of the shipped brushes are, and the canvas does
    /// not stamp yet — so the icon promises a mark the ink cannot make.
    /// Nothing reads this but the tests; the stamp engine will.
    pub stamp: bool,
    factory: Brush,
}

impl Preset {

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
    /// The brush in the hand: which set, and which brush of it.
    selected: (usize, usize),
}

impl Library {
    pub fn sets(&self) -> &[Set] {
        &self.sets
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

/// Sketchbook's own sets, converted by `tools/import-skbrushes.py` and
/// built into the binary. The originals are 34 MB of zip and 438 MB of
/// shape and texture TIFFs once opened; this is the 73 KB of it the
/// canvas can act on, plus a sheet of the icons.
const SHIPPED: &str = include_str!("../assets/brushes/library.json");

/// How many cells the icon sheet has. The palette needs it to cut the
/// sheet up, and a brush pointing past it is an asset built wrong.
#[allow(dead_code)] // the palette's grid cuts the sheet by it
pub const ICONS: u16 = 211;

/// What `library.json` looks like. It is built, not typed, but it is
/// still read through a door: every number is put through its own
/// `Property`, so an asset built wrong cannot seat a value the sliders
/// could never reach.
#[derive(Deserialize)]
struct LibraryOnDisk {
    icons: u16,
    sets: Vec<SetOnDisk>,
}

#[derive(Deserialize)]
struct SetOnDisk {
    name: String,
    brushes: Vec<PresetOnDisk>,
}

#[derive(Deserialize)]
struct PresetOnDisk {
    name: String,
    icon: u16,
    brush: Brush,
    /// Absent when the brush ships at its factory settings, which is
    /// every brush in the sets that came with it.
    #[serde(default)]
    factory: Option<Brush>,
    #[serde(default)]
    stamp: bool,
}

/// Holds every property inside the band its slider runs over, so what
/// the file says and what the panels can reach are the same thing.
fn settled(mut brush: Brush) -> Brush {
    for p in Property::ALL {
        let v = p.get(&brush);
        p.set(&mut brush, v);
    }
    brush
}

impl Default for Library {
    /// The brushes the binary ships with: Sketchbook's seventeen sets.
    ///
    /// The asset is built from the sets in `brushes/` and checked by the
    /// tests, so a failure here is a broken build rather than bad input
    /// — but it still costs nothing to be strict, and an asset that does
    /// not parse leaves one plain brush to draw with instead of no
    /// window at all.
    fn default() -> Library {
        let disk: LibraryOnDisk = match serde_json::from_str(SHIPPED) {
            Ok(d) => d,
            Err(e) => {
                log::error!("brush library did not parse: {e}");
                return Library::bare();
            }
        };
        let sets: Vec<Set> = disk
            .sets
            .into_iter()
            .map(|s| Set {
                name: s.name,
                presets: s
                    .brushes
                    .into_iter()
                    .map(|p| {
                        let brush = settled(p.brush);
                        Preset {
                            name: p.name,
                            factory: p.factory.map_or(brush, settled),
                            brush,
                            icon: p.icon.min(disk.icons.saturating_sub(1)),
                            stamp: p.stamp,
                        }
                    })
                    .collect(),
            })
            .filter(|s: &Set| !s.presets.is_empty())
            .collect();
        if sets.is_empty() {
            return Library::bare();
        }
        Library {
            sets,
            selected: (0, 0),
        }
    }
}

impl Library {
    /// The one brush there is when the shipped sets cannot be read. Never
    /// empty: every accessor assumes a brush in the hand.
    fn bare() -> Library {
        Library {
            sets: vec![Set {
                name: "Basic".to_owned(),
                presets: vec![Preset {
                    name: "Round".to_owned(),
                    brush: Brush::default(),
                    factory: Brush::default(),
                    icon: 0,
                    stamp: false,
                }],
            }],
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
        for _ in 0..5 {
            b.grow();
        }
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
                hardness: 1.0,
                stamp: None,
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
        let dry = Brush {
            hardness: 1.0,
            flow: 0.5,
            ..Brush::default()
        };
        assert!(
            !dry.tip().is_direct(),
            "a nib that does not cover on its own has to build up offscreen"
        );
    }

    #[test]
    fn a_brush_hands_its_nib_to_the_tip_and_a_pencil_has_none() {
        assert_eq!(Tip::PENCIL.stamp, None, "a pencil sweeps, it does not stamp");
        let b = Brush {
            spacing: 0.4,
            roundness: 0.5,
            rotation: 30.0,
            ..Brush::default()
        };
        assert_eq!(
            b.tip().stamp,
            Some(Stamp {
                spacing: 0.4,
                roundness: 0.5,
                rotation: 30.0,
                flow: 1.0,
            }),
            "every brush is a nib stamped at a spacing"
        );
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
            stamp: None,
        };
        assert_eq!(
            Tip::of(&p),
            Tip {
                width: 7.0,
                opacity: 0.25,
                hardness: 0.75,
                stamp: None,
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
            let honored = matches!(
                p,
                Property::Size
                    | Property::Opacity
                    | Property::Hardness
                    | Property::Flow
                    | Property::Spacing
                    | Property::Roundness
                    | Property::Rotation
            );
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
        // Randomness throws a property by an amount in its own unit, so
        // it is never written as a percentage of anything.
        assert_eq!(Property::JitterSize.format(19.2), "19.2");
        assert_eq!(Property::JitterRotation.format(180.0), "180°");
        assert_eq!(Property::JitterSpacing.format(4.5), "4.5");
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
    fn the_shipped_library_is_sketchbooks_own() {
        let lib = Library::default();
        assert_eq!(lib.sets().len(), 17, "every set that came with it");
        let total: usize = lib.sets().iter().map(|s| s.presets.len()).sum();
        assert_eq!(total, 211);
        assert_eq!(lib.sets()[0].name, "Basic", "the shelf it opens on");
        assert_eq!(lib.selected(), (0, 0), "the first brush of the first set");
        assert_eq!(*lib.brush(), lib.sets()[0].presets[0].brush);
        assert!(!lib.name().is_empty());
        for name in ["Legacy", "Markers", "Fine Art", "Half Tone", "Smudge"] {
            assert!(
                lib.sets().iter().any(|s| s.name == name),
                "{name} did not come through"
            );
        }
    }

    #[test]
    fn every_shipped_brush_names_an_icon_of_its_own() {
        let lib = Library::default();
        let icons: Vec<u16> = lib
            .sets()
            .iter()
            .flat_map(|s| s.presets.iter().map(|p| p.icon))
            .collect();
        let mut seen = icons.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), icons.len(), "two brushes share an icon");
        assert_eq!(seen.len(), ICONS as usize, "the sheet has cells to spare");
        assert!(icons.iter().all(|&i| i < ICONS), "an icon off the sheet");
    }

    #[test]
    fn most_of_the_acquired_brushes_want_a_stamp_the_engine_owes_them() {
        let lib = Library::default();
        let all: Vec<&Preset> = lib.sets().iter().flat_map(|s| s.presets.iter()).collect();
        let stamped = all.iter().filter(|p| p.stamp).count();
        assert!(stamped > all.len() / 2, "{stamped} of {}", all.len());
        assert!(
            all.iter().any(|p| !p.stamp),
            "and some paint with what there is"
        );
    }

    #[test]
    fn every_shipped_brush_carries_one_of_sketchbooks_four_profiles() {
        let lib = Library::default();
        for s in lib.sets() {
            for p in &s.presets {
                assert!(Profile::ALL.contains(&p.brush.profile), "{}", p.name);
            }
        }
        // All four are in use, or the field is not worth carrying.
        for profile in Profile::ALL {
            assert!(
                lib.sets()
                    .iter()
                    .flat_map(|s| s.presets.iter())
                    .any(|p| p.brush.profile == profile),
                "{profile:?} is on no brush"
            );
        }
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
    fn no_two_brushes_on_a_shelf_lay_the_same_ink_today() {
        // A brush that carries a shape is told apart by the shape, which
        // the canvas does not stamp yet — two of those may well paint
        // alike for now. The ones that do not are the shelf's own, and
        // picking one over its neighbour has to change something.
        let lib = Library::default();
        for s in lib.sets() {
            let mut tips: Vec<Tip> = s
                .presets
                .iter()
                .filter(|p| !p.stamp)
                .map(|p| p.brush.tip())
                .collect();
            let all = tips.len();
            tips.sort_by(|a, b| {
                a.width
                    .total_cmp(&b.width)
                    .then(a.opacity.total_cmp(&b.opacity))
                    .then(a.hardness.total_cmp(&b.hardness))
            });
            tips.dedup();
            assert_eq!(tips.len(), all, "{} has two brushes painting alike", s.name);
        }
    }

    #[test]
    fn the_size_range_reaches_the_widest_brush_that_came() {
        let lib = Library::default();
        let widest = lib
            .sets()
            .iter()
            .flat_map(|s| s.presets.iter())
            .map(|p| p.brush.size)
            .fold(0.0, f64::max);
        assert!(widest > 500.0, "the acquired sets go wider than that");
        assert!(
            widest <= SIZE_MAX,
            "{widest} would be clamped on the way in"
        );
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
    fn a_brush_can_be_taken_off_any_shelf() {
        let mut lib = Library::default();
        let last = lib.sets().len() - 1;
        lib.select(last, 0);
        assert_eq!(lib.selected(), (last, 0));
        assert_eq!(*lib.brush(), lib.sets()[last].presets[0].brush);
        assert_eq!(lib.name(), lib.sets()[last].presets[0].name);
    }

    #[test]
    fn a_seat_that_is_not_there_is_refused() {
        let mut lib = Library::default();
        let held = lib.selected();
        lib.select(999, 0);
        lib.select(0, 999);
        assert_eq!(lib.selected(), held);
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
