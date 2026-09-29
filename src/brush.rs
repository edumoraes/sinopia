//! The brushes: a library of named presets, the body each one carries,
//! the tip a stroke takes into the document, and the ring the pointer
//! shows. Pure.
//!
//! A brush is Sketchbook's, not Photoshop's: it is a thing in a library
//! with a name, and an edit belongs to it — `[` and the bar's slider both
//! write into the brush in the hand, and `reset` takes it back to what it
//! shipped as. The library is the window's, not any board's: a brush
//! belongs to the person drawing. What they changed about it outlives
//! the process as [`Edits`] — a list of exceptions, named rather than
//! numbered, so the shelf this build ships and the ones imported stay
//! the source of truth for every brush nobody touched.
//!
//! Its body is the whole of Brush Properties. Three of it reach the
//! canvas — [`Property::honored`] is the list — and the rest are the
//! stamp engine's, kept on the brush because that is where they belong.

use serde::{Deserialize, Serialize};

use crate::doc::{Mark, Path, Pressure, Profile, Scatter, Stamp, Stroke};
use crate::editor::PEN_WIDTH;
use crate::scene::{Blend, NIB_MIN_PX, Prim, Rgba, Shapes, polyline_prims};

/// Brush size in world units (logical px at zoom 1): the diameter. The
/// top is the widest brush Sketchbook's own sets carry — a 350-unit
/// radius — so none of them is clamped on the way in when imported.
pub const SIZE_MIN: f64 = 1.0;
pub const SIZE_MAX: f64 = 700.0;
/// What `{` and `}` change the hardness by.
pub const HARDNESS_STEP: f64 = 0.25;
/// Sketchbook's own band for the gap between two stamps, in its own
/// spacing units — the numbers its help names, 0.1 to 10.
pub const SPACING_MIN: f64 = 0.1;
pub const SPACING_MAX: f64 = 10.0;

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
    /// The gap between two stamps, in Sketchbook's spacing units.
    pub spacing: f64,
    /// 1 is a round tip; less flattens it, and only then can it be turned.
    pub roundness: f64,
    /// The tip's own angle, in degrees.
    pub rotation: f64,
    pub dynamics: Dynamics,
    pub profile: Profile,
    /// What one dab does to the ink already down.
    pub mark: Mark,
    /// How much of the radius is crisp, 0–1: 1 is a pencil's edge, 0
    /// fades from the center out. Sketchbook's Edge.
    pub hardness: f64,
    /// How far the texture bites into the nib, 0–1.
    pub texture_depth: f64,
    /// How hard a dab pulls at the paint under it, 0–1. Sketchbook's
    /// Strength: what a smudge brush drags with, and what a colorless
    /// one is made of.
    pub strength: f64,
    /// How much of what it picks up it mixes into its own ink, 0–1.
    pub blending: f64,
    /// How much it thins the paint it drags, 0–1.
    pub dilution: f64,
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
            mark: Mark::Ink,
            hardness: 0.5,
            texture_depth: 0.0,
            strength: 0.0,
            blending: 0.0,
            dilution: 0.0,
            jitter: Jitter::default(),
            // What Sketchbook's own sets do almost to a brush: the pen
            // drives the width and leaves the ink alone.
            pressure: Pressure {
                size: 1.0,
                opacity: 0.0,
                flow: 0.0,
            },
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
    /// What a dab does with the paint already under it — Sketchbook's
    /// Strength, Blending and Dilution, which the Smudge and Colorless
    /// shelves are made of.
    Paint,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Pressure,
        Section::Stamp,
        Section::Nib,
        Section::Randomness,
        Section::Paint,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Pressure => "Pressure",
            Section::Stamp => "Stamp",
            Section::Nib => "Nib",
            Section::Randomness => "Randomness",
            Section::Paint => "Paint",
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
    Strength,
    Blending,
    Dilution,
}

/// How a value is written out under its slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// World units, whole numbers: the size.
    Px,
    Percent,
    /// A quantity in the property's own unit, to one decimal: the gap
    /// between two stamps in spacing units, or how far a randomness throws
    /// the thing it varies.
    Amount,
    Degrees,
}

impl Property {
    pub const ALL: [Property; 16] = [
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
        Property::Strength,
        Property::Blending,
        Property::Dilution,
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
            Property::Strength => "Strength",
            Property::Blending => "Blending",
            Property::Dilution => "Dilution",
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
            Property::Strength | Property::Blending | Property::Dilution => Section::Paint,
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
    /// its [`Stamp`] and that nib's `Scatter` included. The three
    /// randomness amounts left out are the ones whose scale the assets
    /// contradict. Two are not in their property's own unit at all — a
    /// fraction thrown by five, or by twenty. The third is the gap:
    /// seventeen of the thirty brushes that throw it name an amount
    /// larger than the gap itself, so a throw either side of it would
    /// land negative more often than not, and nobody ships that. The
    /// canvas does not guess at any of the three.
    ///
    /// Depth is honored the way Rotation is: the canvas answers to it,
    /// and whether a given brush shows a difference is the brush's own
    /// business — a nib that names no paper is no more moved by Depth
    /// than a round one is by Rotation.
    ///
    /// Nor does it guess at Strength, Blending and Dilution: those
    /// describe what a dab does with the paint *under* it, which asks
    /// the canvas to read back what it has already drawn. Every stroke
    /// here is redrawn from its curves each frame, so there is nothing
    /// to read — the layer would have to be kept as pixels, which is a
    /// different engine and not a missing line.
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
                | Property::JitterSize
                | Property::JitterRotation
                | Property::TextureDepth
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
            Property::Strength => b.strength,
            Property::Blending => b.blending,
            Property::Dilution => b.dilution,
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
            Property::Strength => b.strength = v,
            Property::Blending => b.blending = v,
            Property::Dilution => b.dilution = v,
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

    /// What this body lays, wearing `face` — the nib image the preset
    /// carries, which is not part of the body a slider can reach. Nor
    /// is the paper: [`Preset::tip`] is the one that knows all three.
    pub fn tip(&self, face: Face) -> Tip {
        Tip {
            width: self.size,
            opacity: self.opacity,
            hardness: self.hardness,
            dynamics: self.dynamics,
            stamp: Some(Stamp {
                shape: face.shape().map(str::to_owned),
                grain: face.grain().map(str::to_owned),
                spacing: self.spacing,
                roundness: self.roundness,
                rotation: self.rotation,
                profile: self.profile,
                mark: self.mark,
                // One of Sketchbook's four rotation dynamics is the
                // stroke's own doing and belongs on the nib; the other
                // two are the stylus's, and what they turn the nib by
                // is written into the stroke reading by reading.
                follow: self.dynamics == Dynamics::ToStroke,
                flow: self.flow,
                scatter: Scatter {
                    size: self.jitter.size,
                    rotation: self.jitter.rotation,
                },
                pressure: self.pressure,
                // The preset's, and dressed on after: the body knows
                // how deep a paper bites but not which paper it is.
                paper: None,
            }),
        }
    }
}

/// The nib image a brush carries, which is not part of the body a
/// slider can reach: Sketchbook's two kinds, which do quite different
/// things. A shape is stamped in place of the round dab; a grain is
/// worn over one, so the dab keeps its own edge and the grain eats
/// into it. Most nibs are neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Face<'a> {
    #[default]
    Round,
    Shape(&'a str),
    Grain(&'a str),
}

impl<'a> Face<'a> {
    fn shape(self) -> Option<&'a str> {
        match self {
            Face::Shape(name) => Some(name),
            _ => None,
        }
    }

    fn grain(self) -> Option<&'a str> {
        match self {
            Face::Grain(name) => Some(name),
            _ => None,
        }
    }
}

/// The paper a preset drags its nib over: the image, and how wide one
/// tile of it is in world units. Not part of the body either — no
/// slider addresses either half — though how deep it bites is
/// `Brush::texture_depth`, which one does.
#[derive(Debug, Clone, PartialEq)]
pub struct Paper {
    pub name: String,
    pub period: f64,
}

/// What a stroke is drawn with, taken at the press and written into the
/// path on release: the pencil's constant or the brush's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// World units.
    pub width: f64,
    pub opacity: f64,
    pub hardness: f64,
    /// What turns the nib as the stroke goes. Only the stylus's half of
    /// it is read here — the stroke's own is the nib's `follow`, and
    /// the tilt and the roll have to be asked of the pen sample by
    /// sample, which is why the tip still carries the whole answer.
    pub dynamics: Dynamics,
    /// The nib, when the stroke is stamped. A pencil sweeps and has
    /// none.
    pub stamp: Option<Stamp>,
}

/// How much of the pencil's width the pen takes away at no pressure.
/// A brush says this on its own nib, where a slider reaches it; the
/// pencil has no sliders at all, so its answer is the build's — and the
/// document is none the poorer for it, since it records what the person
/// can vary and there is nothing here to vary.
///
/// The number is off the shipped sets rather than out of the air: not
/// one of Sketchbook's 211 brushes drives its size fully, and its own
/// Fine Art pencils sit between a quarter and two thirds. Half is the
/// middle of that, and a pencil at half is still a pencil at the
/// lightest touch instead of a stroke that vanishes.
pub const PENCIL_DRIVE: Pressure = Pressure {
    size: 0.5,
    opacity: 0.0,
    flow: 0.0,
};

impl Tip {
    pub const PENCIL: Tip = Tip {
        width: PEN_WIDTH,
        opacity: 1.0,
        hardness: 1.0,
        dynamics: Dynamics::None,
        stamp: None,
    };

    /// The tip a committed stroke was laid with. Its dynamics are spent:
    /// what the stylus turned the nib by is already written down, dab
    /// by dab, in the stroke's own envelope.
    pub fn of(path: &Path) -> Tip {
        Tip {
            width: path.width,
            opacity: path.opacity,
            hardness: path.hardness,
            dynamics: Dynamics::None,
            stamp: path.stamp.clone(),
        }
    }

    /// The tip one stroke of a `paint` was laid with. A raster layer keeps
    /// them stroke by stroke, so each one is composited as it was made.
    pub fn of_stroke(s: &Stroke) -> Tip {
        Tip {
            width: s.width,
            opacity: s.opacity,
            hardness: s.hardness,
            dynamics: Dynamics::None,
            stamp: s.stamp.clone(),
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
            && self.stamp.as_ref().is_none_or(|s| {
                // An eraser never covers anything: it has to be taken
                // back out of the sheet, which means going through the
                // scratch like any other composited stroke.
                !s.mark.erases()
                // A nib the pen thins the ink of lays dabs that do not
                // cover either, however full the brush's own flow is.
                    && s.flow >= 1.0
                    && s.pressure.opacity == 0.0
                    && s.pressure.flow == 0.0
            })
    }

    /// How the finished stroke meets what it is laid on: ink over it,
    /// or ink taken out of it.
    pub fn lands(&self) -> Blend {
        match self.stamp.as_ref().is_some_and(|s| s.mark.erases()) {
            true => Blend::Erase,
            false => Blend::Over,
        }
    }

    /// How much of the nib the pen's pressure drives. A stamped tip
    /// says so on its own nib; a swept one is the pencil, and thins
    /// with the hand by [`PENCIL_DRIVE`].
    pub fn drive(&self) -> Pressure {
        match &self.stamp {
            Some(stamp) => stamp.pressure,
            None => PENCIL_DRIVE,
        }
    }

    /// Whether it rubs out instead of painting. A paint holding one of
    /// these has to be built on a sheet of its own.
    pub fn erases(&self) -> bool {
        self.stamp.as_ref().is_some_and(|s| s.mark.erases())
    }
}

/// What the person has changed about their brushes, and nothing else:
/// the brushes moved off what they shipped as, and the one in the hand.
/// A file of exceptions, so the sets stay the source of truth for every
/// brush nobody touched — an asset rebuilt with a new brush in it opens
/// with that brush, not without it.
///
/// A brush is named, never numbered: a set added to `brushes/` moves
/// every index after it, and a file written last week would then dress
/// the wrong brush.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edits {
    /// The set and the brush in the hand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub brushes: Vec<Edit>,
    /// The ten seats, in their own order. Empty when nothing has been
    /// seated by hand, so a file written before the strip existed opens
    /// with the shipped nine rather than with none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<Option<Held>>,
}

/// How many seats the strip keeps within reach. Nine that ship filled,
/// and slot 0 — the last brush used that none of the nine already hold.
pub const SLOTS: usize = 10;

/// What seats 1..=9 hold on a machine that has never been drawn on: the
/// shelf this build ships, whole — pencil, pens, marker, airbrush, two
/// rounds and two erasers, a hand without anybody picking one out.
pub const SLOT_DEFAULTS: [(&str, &str); SLOTS - 1] = [
    (OWN_SET, "Pencil"),
    (OWN_SET, "Fine Liner"),
    (OWN_SET, "Ink Pen"),
    (OWN_SET, "Marker"),
    (OWN_SET, "Airbrush"),
    (OWN_SET, "Hard Round"),
    (OWN_SET, "Soft Round"),
    (OWN_SET, "Eraser"),
    (OWN_SET, "Soft Eraser"),
];

/// Which brush was in the hand, by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub set: String,
    pub name: String,
}

/// One brush the person moved off its factory settings, and where it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub set: String,
    pub name: String,
    pub brush: Brush,
}

/// One named brush in the library, and what it shipped as — so an edit
/// can always be taken back without knowing which brush it was.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub brush: Brush,
    /// What pictures it in the library and the strip.
    pub icon: Icon,
    /// The nib image it carries, by the name the sheet gives it: a
    /// shape it stamps in place of a round dab, or a grain it wears
    /// over one. Not part of the body: no slider addresses it, and
    /// `reset` has nothing to take back.
    pub shape: Option<String>,
    pub grain: Option<String>,
    /// The paper it is dragged over, on the same terms. Forty-nine of
    /// the shipped brushes name one and thirty of those wear a nib as
    /// well: the two are not alternatives.
    pub paper: Option<Paper>,
    factory: Brush,
}

impl Preset {
    /// What this brush lays: its body, wearing the nib it carries and
    /// dragged over the paper it names. The depth is the body's, so a
    /// Depth of nothing is a paper that is not there — which is what
    /// the slider means at the bottom of its own track, and what one
    /// shipped brush already says.
    pub fn tip(&self) -> Tip {
        let mut tip = self.brush.tip(self.face());
        if let Some(paper) = &self.paper
            && self.brush.texture_depth > 0.0
            && let Some(stamp) = tip.stamp.as_mut()
        {
            stamp.paper = Some(crate::doc::Paper {
                name: paper.name.clone(),
                period: paper.period,
                depth: self.brush.texture_depth,
            });
        }
        tip
    }

    /// The nib image it carries, if it carries one. A brush names a
    /// shape or a grain, never both — the asset is built that way, and
    /// the document refuses a stroke that says otherwise.
    pub fn face(&self) -> Face<'_> {
        match (self.shape.as_deref(), self.grain.as_deref()) {
            (Some(name), _) => Face::Shape(name),
            (None, Some(name)) => Face::Grain(name),
            (None, None) => Face::Round,
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

/// What pictures a brush in the library and the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Drawn from the brush's own body, in the chrome's ink: what the
    /// brushes this build ships wear, since they carry no art.
    Drawn,
    /// A cell of the imported sheet, which carries its own colours.
    Sheet(u16),
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
/// not to the board. What was changed about it reaches the disk as
/// [`Edits`].
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    sets: Vec<Set>,
    /// How the imported icon sheet is cut up: how many cells stand
    /// across it, and how many there are. Nothing when nothing was
    /// imported.
    icon_cols: u16,
    icons: u16,
    /// The nib shapes the sheet carries, in its own order: a name's
    /// place here is the cell it is in.
    shapes: Vec<String>,
    /// How many cells the sheet stands across, as the asset was built,
    /// and how big one is.
    shape_cols: u16,
    shape_px: u16,
    /// The papers, in the band under them, on the same terms.
    papers: Vec<String>,
    paper_px: u16,
    /// The brush in the hand: which set, and which brush of it.
    selected: (usize, usize),
    /// The ten seats the strip keeps, `[0]` the overflow and `[1..=9]`
    /// the numbered ones. A seat is empty when it names a brush this
    /// build no longer carries.
    slots: Vec<Option<(usize, usize)>>,
}

impl Library {
    pub fn sets(&self) -> &[Set] {
        &self.sets
    }

    pub fn selected(&self) -> (usize, usize) {
        self.selected
    }

    /// Takes up a brush, if there is one in that seat. Slot 0 follows
    /// the hand: it is the way back to whatever was last reached for
    /// that none of the numbered seats already holds.
    pub fn select(&mut self, set: usize, index: usize) {
        if !self.holds((set, index)) {
            return;
        }
        self.selected = (set, index);
        if !self.seated((set, index)) {
            self.slots[0] = Some((set, index));
        }
    }

    /// The ten seats, `[0]` the overflow and `[1..=9]` the numbered ones.
    pub fn slots(&self) -> &[Option<(usize, usize)>] {
        &self.slots
    }

    /// Takes up the brush in a seat. False when the seat is empty or
    /// there is no such seat, so the caller knows nothing moved and
    /// writes nothing back.
    pub fn take_slot(&mut self, n: usize) -> bool {
        let Some(Some(at)) = self.slots.get(n).copied() else {
            return false;
        };
        self.select(at.0, at.1);
        true
    }

    /// Seats a brush in one of the nine. Slot 0 is computed from what
    /// the hand has been reaching for, so it is never written to.
    pub fn assign_slot(&mut self, n: usize, at: (usize, usize)) -> bool {
        if n == 0 || n >= SLOTS || !self.holds(at) {
            return false;
        }
        self.slots[n] = Some(at);
        // It is within reach by its number now, so the overflow has
        // nothing left to keep.
        if self.slots[0] == Some(at) {
            self.slots[0] = None;
        }
        true
    }

    /// Whether the library has a brush in that seat at all.
    fn holds(&self, (set, index): (usize, usize)) -> bool {
        self.sets.get(set).is_some_and(|s| index < s.presets.len())
    }

    /// Whether a brush is already within reach of the numbered seats.
    fn seated(&self, at: (usize, usize)) -> bool {
        self.slots[1..].contains(&Some(at))
    }

    /// What the seats hold on a build nobody has rearranged: the
    /// overflow empty, and the shipped nine wherever the sets put them —
    /// empty for any name this build no longer carries.
    fn shipped_slots(&self) -> Vec<Option<(usize, usize)>> {
        std::iter::once(None)
            .chain(SLOT_DEFAULTS.iter().map(|(set, name)| self.seat(set, name)))
            .collect()
    }

    /// Fills the seats as the library is built. Called once.
    fn seed_slots(mut self) -> Library {
        let seats = self.shipped_slots();
        self.slots = seats;
        self
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

    /// What the brush in the hand lays.
    pub fn tip(&self) -> Tip {
        self.preset().tip()
    }

    /// Where each nib shape sits on the sheet, once the renderer has
    /// said which slot the sheet was uploaded to. The one place that
    /// says how the sheet is cut up: a name's place in the shipped list
    /// is its cell, and the asset says how many cells stand across.
    pub fn sheet(&self, slot: u32) -> Shapes {
        let named = |names: &[String]| {
            names
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i as u16))
                .collect()
        };
        Shapes {
            slot,
            cols: self.shape_cols,
            rows: match self.shape_cols {
                0 => 0,
                cols => (self.shapes.len() as u16).div_ceil(cols),
            },
            px: self.shape_px,
            cells: named(&self.shapes),
            paper_px: self.paper_px,
            papers: named(&self.papers),
        }
    }

    /// How the imported icon sheet is cut up: cells across, and cells
    /// in all. `(0, 0)` when nothing was imported.
    pub fn icon_grid(&self) -> (u16, u16) {
        (self.icon_cols, self.icons)
    }

    pub fn edited(&self) -> bool {
        self.preset().edited()
    }

    pub fn reset(&mut self) {
        self.preset_mut().reset();
    }

    /// What the person has changed, and nothing else: the brushes off
    /// their factory settings, and the one in the hand. What goes on
    /// disk — the brushes belong to the person, not to any board.
    pub fn edits(&self) -> Edits {
        Edits {
            held: self.sets.get(self.selected.0).map(|set| Held {
                set: set.name.clone(),
                name: self.name().to_owned(),
            }),
            brushes: self
                .sets
                .iter()
                .flat_map(|set| {
                    set.presets.iter().filter(|p| p.edited()).map(|p| Edit {
                        set: set.name.clone(),
                        name: p.name.clone(),
                        brush: p.brush,
                    })
                })
                .collect(),
            // Seats nobody rearranged are not a change, so they are not
            // written down — the same reason an untouched brush is not.
            slots: if self.slots == self.shipped_slots() {
                Vec::new()
            } else {
                self.slots
                    .iter()
                    .map(|seat| {
                        seat.map(|(s, i)| Held {
                            set: self.sets[s].name.clone(),
                            name: self.sets[s].presets[i].name.clone(),
                        })
                    })
                    .collect()
            },
        }
    }

    /// Dresses the brushes in what was kept of them. A name this library
    /// no longer carries is passed over rather than refused: the shipped
    /// shelf changes with the build and the imported ones with each
    /// import, and a brush that has gone is one the person can no longer
    /// be holding either. Every value goes through
    /// its own `Property::set`, as `library.json` does — a file edited
    /// by hand must not seat a number the sliders could never reach.
    pub fn apply(&mut self, edits: &Edits) {
        for edit in &edits.brushes {
            if let Some(p) = self.find_mut(&edit.set, &edit.name) {
                p.brush = settled(edit.brush);
            }
        }
        // An empty list is a file written before the seats existed: the
        // shipped nine stand. A name this build no longer carries leaves
        // its seat empty, since a brush that has gone is one the person
        // can no longer be reaching for either.
        if !edits.slots.is_empty() {
            let mut slots = vec![None; SLOTS];
            for (n, seat) in edits.slots.iter().take(SLOTS).enumerate() {
                slots[n] = seat.as_ref().and_then(|h| self.seat(&h.set, &h.name));
            }
            self.slots = slots;
        }
        if let Some(held) = &edits.held
            && let Some(at) = self.seat(&held.set, &held.name)
        {
            self.selected = at;
        }
    }

    /// Where a brush sits, by the names it and its set carry. The first
    /// of a name, should a set ever ship two.
    fn seat(&self, set: &str, name: &str) -> Option<(usize, usize)> {
        let s = self.sets.iter().position(|x| x.name == set)?;
        let i = self.sets[s].presets.iter().position(|p| p.name == name)?;
        Some((s, i))
    }

    fn find_mut(&mut self, set: &str, name: &str) -> Option<&mut Preset> {
        let (s, i) = self.seat(set, name)?;
        Some(&mut self.sets[s].presets[i])
    }
}

/// The shelf this build ships, and the name it goes by in `brushes.json`.
/// None of Sketchbook's sets is called this, so an imported shelf never
/// shadows it.
pub const OWN_SET: &str = "Sinopia";

/// The brushes this build ships. Every one is a round nib — no shape,
/// no grain, no paper — and every number is this project's own: the art
/// Sketchbook draws its brushes with is Sketchbook's, and comes in only
/// through `sinopia brushes import`, from a copy the person downloaded.
/// Between them they reach every profile, both marks the canvas lays,
/// a flattened nib and what the pen can drive.
fn own_set() -> Set {
    let brush = |name: &str, brush: Brush| Preset {
        name: name.to_owned(),
        brush: settled(brush),
        factory: settled(brush),
        icon: Icon::Drawn,
        shape: None,
        grain: None,
        paper: None,
    };
    let driven = |size: f64, opacity: f64, flow: f64| Pressure {
        size,
        opacity,
        flow,
    };
    let round = Brush {
        spacing: 0.4,
        ..Brush::default()
    };
    Set {
        name: OWN_SET.to_owned(),
        presets: vec![
            brush("Pencil", Brush {
                size: 4.0,
                opacity: 0.9,
                flow: 0.7,
                hardness: 0.8,
                profile: Profile::Sharp,
                pressure: driven(0.5, 0.5, 0.0),
                ..round
            }),
            brush("Fine Liner", Brush {
                size: 3.0,
                spacing: 0.5,
                hardness: 1.0,
                profile: Profile::HardSolid,
                pressure: driven(0.25, 0.0, 0.0),
                ..round
            }),
            brush("Ink Pen", Brush {
                size: 10.0,
                hardness: 0.95,
                profile: Profile::HardSolid,
                pressure: driven(0.9, 0.0, 0.0),
                ..round
            }),
            brush("Marker", Brush {
                size: 24.0,
                opacity: 0.7,
                flow: 0.4,
                spacing: 0.3,
                roundness: 0.3,
                rotation: 45.0,
                hardness: 0.85,
                pressure: Pressure::NONE,
                ..round
            }),
            brush("Airbrush", Brush {
                size: 90.0,
                opacity: 0.9,
                flow: 0.06,
                spacing: 0.3,
                hardness: 0.0,
                profile: Profile::Airbrush,
                pressure: driven(0.0, 0.0, 0.8),
                ..round
            }),
            brush("Hard Round", Brush {
                size: 20.0,
                hardness: 0.9,
                pressure: driven(1.0, 0.0, 0.0),
                ..round
            }),
            brush("Soft Round", Brush {
                size: 50.0,
                flow: 0.25,
                spacing: 0.3,
                hardness: 0.1,
                pressure: driven(0.3, 0.0, 0.6),
                ..round
            }),
            brush("Eraser", Brush {
                size: 24.0,
                hardness: 0.95,
                profile: Profile::HardSolid,
                mark: Mark::Erase,
                pressure: Pressure::NONE,
                ..round
            }),
            brush("Soft Eraser", Brush {
                size: 70.0,
                flow: 0.4,
                spacing: 0.3,
                hardness: 0.1,
                profile: Profile::Airbrush,
                mark: Mark::Erase,
                pressure: driven(0.0, 0.0, 0.6),
                ..round
            }),
        ],
    }
}

/// What an imported `library.json` looks like — what `sinopia brushes
/// import` writes. It is built, not typed, but it is still read through
/// a door: every number is put through its own `Property`, so a file
/// built wrong or edited by hand cannot seat a value the sliders could
/// never reach.
#[derive(Deserialize)]
struct LibraryOnDisk {
    #[serde(default)]
    icons: u16,
    #[serde(default)]
    icon_cols: u16,
    /// The nib shapes the sheet carries, in its own order.
    #[serde(default)]
    shapes: Vec<String>,
    #[serde(default)]
    shape_cols: u16,
    #[serde(default)]
    shape_px: u16,
    /// And the papers, in the band under them.
    #[serde(default)]
    papers: Vec<String>,
    #[serde(default)]
    paper_px: u16,
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
    /// Its cell of the icon sheet; absent when the set shipped no icon
    /// for it, and then it is drawn like the brushes this build ships.
    #[serde(default)]
    icon: Option<u16>,
    brush: Brush,
    /// Absent when the brush ships at its factory settings, which is
    /// every brush in the sets that came with it.
    #[serde(default)]
    factory: Option<Brush>,
    /// The nib image, by name, when the brush carries one of its own:
    /// a shape it stamps, or a grain it wears.
    #[serde(default)]
    shape: Option<String>,
    #[serde(default)]
    grain: Option<String>,
    /// The paper it is dragged over, when it names one.
    #[serde(default)]
    paper: Option<PaperOnDisk>,
}

#[derive(Deserialize)]
struct PaperOnDisk {
    name: String,
    period: f64,
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
    /// The brushes this build ships, and nothing imported.
    fn default() -> Library {
        Library {
            sets: vec![own_set()],
            icon_cols: 0,
            icons: 0,
            shapes: Vec::new(),
            shape_cols: 0,
            shape_px: 0,
            papers: Vec::new(),
            paper_px: 0,
            selected: (0, 0),
            slots: Vec::new(),
        }
        .seed_slots()
    }
}

impl Library {
    /// The brushes this build ships, then the shelves `sinopia brushes
    /// import` wrote into `json`. A library that will not parse is
    /// logged and left out: the brushes are a convenience, never a
    /// reason for the window not to open.
    pub fn with_imported(json: Option<&str>) -> Library {
        let mut lib = Library::default();
        let Some(json) = json else { return lib };
        let disk: LibraryOnDisk = match serde_json::from_str(json) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("the imported brushes did not parse: {e}");
                return lib;
            }
        };
        let imported = disk.sets.into_iter().map(|s| Set {
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
                        // An icon off the sheet is no icon: the brush is
                        // drawn instead of showing a stranger's cell.
                        icon: p
                            .icon
                            .filter(|&i| i < disk.icons && disk.icon_cols > 0)
                            .map_or(Icon::Drawn, Icon::Sheet),
                        // A nib the sheet does not carry is no nib: a
                        // library built wrong must not seat one the
                        // renderer cannot find.
                        shape: p.shape.filter(|n| disk.shapes.contains(n)),
                        grain: p.grain.filter(|n| disk.shapes.contains(n)),
                        paper: p
                            .paper
                            .filter(|q| {
                                disk.papers.contains(&q.name)
                                    && q.period.is_finite()
                                    && q.period > 0.0
                            })
                            .map(|q| Paper {
                                name: q.name,
                                period: q.period,
                            }),
                    }
                })
                .collect(),
        });
        // A shelf going by a name already taken would be one nobody can
        // seat a brush from: a seat names its shelf, and the first of a
        // name answers.
        for set in imported {
            if set.presets.is_empty() || lib.sets.iter().any(|s| s.name == set.name) {
                continue;
            }
            lib.sets.push(set);
        }
        lib.icon_cols = disk.icon_cols;
        lib.icons = disk.icons;
        lib.shapes = disk.shapes;
        lib.shape_cols = disk.shape_cols;
        lib.shape_px = disk.shape_px;
        lib.papers = disk.papers;
        lib.paper_px = disk.paper_px;
        lib
    }
}

#[cfg(test)]
impl Library {
    /// A library as an import leaves it, for the tests: the shelf this
    /// build ships, then four made-up shelves wearing every kind of art
    /// an import carries — stamped shapes, worn grains, papers, one
    /// brush with no icon of its own — and enough brushes that the
    /// palette has to scroll. Nothing in it is anybody's but this
    /// file's.
    pub fn acquired() -> Library {
        use serde_json::json;
        let mut icon = 0u16;
        let mut next = || {
            icon += 1;
            icon - 1
        };
        let shelf = |name: &str, count: usize, dress: &dyn Fn(usize, &mut serde_json::Value), next: &mut dyn FnMut() -> u16| {
            json!({
                "name": name,
                "brushes": (0..count).map(|i| {
                    let mut b = json!({
                        "name": format!("{name} {}", i + 1),
                        "icon": next(),
                        "brush": { "size": 4.0 + 3.0 * i as f64, "spacing": 0.5 },
                    });
                    dress(i, &mut b);
                    b
                }).collect::<Vec<_>>(),
            })
        };
        let sets = vec![
            shelf("Splatter", 150, &|i, b| {
                b["brush"]["jitter"] = json!({ "size": 2.0 + i as f64, "rotation": 30.0 });
                b["brush"]["mark"] = json!(if i == 0 { "smudge" } else { "normal" });
                b["brush"]["strength"] = json!(if i == 0 { 0.6 } else { 0.0 });
                if i == 149 {
                    b.as_object_mut().expect("a brush").remove("icon");
                }
            }, &mut next),
            shelf("Stamps", 14, &|i, b| {
                b["shape"] = json!(if i % 2 == 0 { "star" } else { "leaf" });
                b["brush"]["dynamics"] = json!("ToStroke");
            }, &mut next),
            shelf("Grains", 10, &|i, b| {
                b["grain"] = json!("sand");
                b["brush"]["hardness"] = json!(0.1 * i as f64);
            }, &mut next),
            shelf("Papers", 12, &|i, b| {
                b["paper"] = json!({
                    "name": if i % 3 == 0 { "canvas~i" } else { "canvas" },
                    "period": 250.0 + 50.0 * i as f64,
                });
                b["brush"]["texture_depth"] = json!(0.8);
                if i % 4 == 0 {
                    b["shape"] = json!("leaf");
                }
            }, &mut next),
        ];
        let library = json!({
            "icon_px": 80,
            "icon_cols": 16,
            "icons": icon,
            "shape_px": 128,
            "shape_cols": 12,
            "shapes": ["star", "leaf", "sand"],
            "paper_px": 192,
            "papers": ["canvas", "canvas~i"],
            "sets": sets,
        });
        Library::with_imported(Some(&library.to_string()))
    }
}

/// Points around the ring, and so a multiple of the four corners it is
/// walked by.
const RING_POINTS: usize = 48;

/// The pointer's ring: the outline of the nib the next press would lay,
/// one logical pixel wide, around `center`. `half` is how far it
/// reaches each way in px and `angle` how far the nib is turned — so a
/// round nib is a circle, a squished one the same flattened capsule
/// `Prim::dab` makes, and a turned one leans the way the ink will.
///
/// It traces the nib's shape and not its art. A brush that stamps a
/// silhouette shows the box the silhouette is stamped into, which is
/// where the ink lands even though it is not the outline of it: the
/// alternative is tracing an alpha mask every frame, and the ring is
/// there to say how big and which way round, not to draw the stamp.
pub fn ring_prims(
    center: (f32, f32),
    half: (f32, f32),
    angle: f32,
    scale: f32,
    color: Rgba,
) -> Vec<Prim> {
    let (hx, hy) = (half.0.max(0.0), half.1.max(0.0));
    // The same corner the dab is cut with: the smaller of the two, so
    // the ends stay round however flat the nib is squished.
    let r = hx.min(hy);
    let (ix, iy) = (hx - r, hy - r);
    let quarter = RING_POINTS / 4;
    let (sin, cos) = angle.sin_cos();
    // Four arcs about the corners' own centres, joined by the straights
    // between them — which is the whole outline when the nib is flat,
    // and nothing at all when it is round and the centres collapse.
    let mut points: Vec<(f32, f32)> = (0..RING_POINTS)
        .map(|i| {
            let (sx, sy) = match i / quarter {
                0 => (1.0, 1.0),
                1 => (-1.0, 1.0),
                2 => (-1.0, -1.0),
                _ => (1.0, -1.0),
            };
            let a = i as f32 / RING_POINTS as f32 * std::f32::consts::TAU;
            let (x, y) = (sx * ix + r * a.cos(), sy * iy + r * a.sin());
            (
                center.0 + cos * x - sin * y,
                center.1 + sin * x + cos * y,
            )
        })
        .collect();
    points.push(points[0]);
    polyline_prims(&points, 0.5 * scale, color)
}

/// The ring a brush's body asks for: how far the nib reaches each way
/// on the screen, and how far it is turned. `px_per_world` is what the
/// view makes of a world unit — the nib is world-sized, like the ink.
pub fn ring_of(brush: &Brush, px_per_world: f64) -> ((f32, f32), f32) {
    let half = (brush.size / 2.0 * px_per_world) as f32;
    let squish = (brush.roundness.clamp(0.0, 1.0) as f32).max(NIB_MIN_PX / half.max(NIB_MIN_PX));
    (
        (half.max(NIB_MIN_PX), (half * squish).max(NIB_MIN_PX)),
        (brush.rotation as f32).to_radians(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Envelope, Path};
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
                dynamics: Dynamics::None,
                stamp: None,
            }
        );
        assert!(Tip::PENCIL.is_direct(), "the pencil needs no compositing");
        let soft = Brush::default().tip(Face::Round);
        assert_eq!((soft.width, soft.opacity, soft.hardness), (16.0, 1.0, 0.5));
        assert!(!soft.is_direct(), "a soft edge has to be composited");
        let hard = Brush {
            hardness: 1.0,
            ..Brush::default()
        };
        assert!(hard.tip(Face::Round).is_direct());
        let faint = Brush {
            hardness: 1.0,
            opacity: 0.5,
            ..Brush::default()
        };
        assert!(!faint.tip(Face::Round).is_direct(), "so does translucency");
        let dry = Brush {
            hardness: 1.0,
            flow: 0.5,
            ..Brush::default()
        };
        assert!(
            !dry.tip(Face::Round).is_direct(),
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
            b.tip(Face::Round).stamp,
            Some(Stamp {
                shape: None,
                grain: None,
                follow: false,
                spacing: 0.4,
                roundness: 0.5,
                rotation: 30.0,
                profile: Profile::RegularSolid,
                mark: Mark::Ink,
                flow: 1.0,
                scatter: Scatter::default(),
                pressure: Brush::default().pressure,
                paper: None,
            }),
            "every brush is a nib stamped at a spacing"
        );
    }

    #[test]
    fn a_brush_with_a_nib_of_its_own_names_one_the_sheet_has() {
        let lib = Library::acquired();
        let sheet = lib.sheet(3);
        let (mut shapes, mut grains) = (0, 0);
        for set in lib.sets() {
            for p in &set.presets {
                let name = match p.face() {
                    Face::Shape(name) => {
                        shapes += 1;
                        name
                    }
                    Face::Grain(name) => {
                        grains += 1;
                        name
                    }
                    Face::Round => continue,
                };
                assert!(
                    sheet.cell(name).is_some(),
                    "{} names {name}, which is not on the sheet",
                    p.name
                );
                assert!(
                    !(p.shape.is_some() && p.grain.is_some()),
                    "{} would be a shape and a grain at once",
                    p.name
                );
            }
        }
        assert_eq!(shapes, 17, "the brushes that stamp a nib of their own");
        assert_eq!(grains, 10, "and those that wear a grain over a round one");
        assert!(
            lib.sets()
                .iter()
                .flat_map(|s| &s.presets)
                .any(|p| p.face() == Face::Round),
            "and the rest lay a plain round nib"
        );
        assert!(sheet.cell("no such nib").is_none());
        assert_eq!(sheet.rows, 1, "three nibs, twelve across");
    }

    #[test]
    fn the_brush_in_the_hand_hands_its_shape_to_the_tip() {
        let mut lib = Library::acquired();
        let (set, index) = lib
            .sets()
            .iter()
            .enumerate()
            .find_map(|(s, st)| {
                st.presets
                    .iter()
                    .position(|p| p.shape.is_some())
                    .map(|i| (s, i))
            })
            .expect("an imported brush stamps a shape");
        lib.select(set, index);
        let want = lib.sets()[set].presets[index].shape.clone();
        let nib = lib.tip().stamp.expect("a brush stamps");
        assert_eq!(nib.shape, want, "the tip carries the nib the preset names");
        assert_eq!(nib.spacing, lib.brush().spacing, "and the body with it");
    }

    #[test]
    fn a_brush_throws_its_nib_by_the_randomness_the_canvas_can_honor() {
        let b = Brush {
            jitter: Jitter {
                size: 3.0,
                opacity: 2.0,
                flow: 7.0,
                rotation: 45.0,
                spacing: 1.0,
            },
            ..Brush::default()
        };
        let nib = b.tip(Face::Round).stamp.expect("a brush stamps");
        assert_eq!(
            nib.scatter,
            Scatter {
                size: 3.0,
                rotation: 45.0,
            },
            "an amount in the unit of what it throws goes straight through"
        );
        // The three left out are the ones the assets contradict: two
        // are not in their property's own unit at all, and the gap is
        // thrown by more than the gap itself in most of the brushes
        // that throw it.
        assert!(!Property::JitterOpacity.honored());
        assert!(!Property::JitterFlow.honored());
        assert!(!Property::JitterSpacing.honored());
    }

    #[test]
    fn only_what_the_person_changed_is_kept() {
        let mut lib = Library::acquired();
        assert_eq!(lib.edits().brushes, vec![], "a library nobody touched");
        let held = lib.edits().held.expect("a brush is always in the hand");

        lib.select(2, 3);
        lib.brush_mut().size = 42.0;
        let edits = lib.edits();
        assert_eq!(edits.brushes.len(), 1, "one brush moved, one brush kept");
        assert_eq!(edits.brushes[0].brush.size, 42.0);
        assert_eq!(edits.brushes[0].name, lib.name());
        assert_ne!(edits.held, Some(held), "and the hand moved too");

        // The same library, dressed in what was kept: same brush in the
        // hand, same size on it, and nothing else touched.
        let mut opened = Library::acquired();
        opened.apply(&edits);
        assert_eq!(opened.selected(), (2, 3));
        assert_eq!(opened.brush().size, 42.0);
        assert_eq!(opened.edits(), edits, "and it keeps the same thing again");
    }

    #[test]
    fn a_brush_the_assets_no_longer_carry_is_passed_over() {
        let mut lib = Library::default();
        let held = lib.edits().held.expect("a brush in the hand");
        lib.apply(&Edits {
            held: Some(Held {
                set: "Gone".into(),
                name: "Gone".into(),
            }),
            brushes: vec![Edit {
                set: "Gone".into(),
                name: "Gone".into(),
                brush: Brush {
                    size: 300.0,
                    ..Brush::default()
                },
            }],
            slots: Vec::new(),
        });
        assert_eq!(lib.edits().held, Some(held), "the hand does not move");
        assert!(lib.edits().brushes.is_empty(), "and nothing is dressed");
    }

    #[test]
    fn a_kept_brush_is_held_inside_the_bands_its_sliders_run_over() {
        let mut lib = Library::default();
        let (set, name) = {
            let s = &lib.sets()[0];
            (s.name.clone(), s.presets[0].name.clone())
        };
        // A file edited by hand, saying something no slider could.
        lib.apply(&Edits {
            held: None,
            brushes: vec![Edit {
                set,
                name,
                brush: Brush {
                    size: 5_000.0,
                    opacity: -3.0,
                    ..Brush::default()
                },
            }],
            slots: Vec::new(),
        });
        let b = lib.sets()[0].presets[0].brush;
        assert_eq!(b.size, SIZE_MAX);
        assert_eq!(b.opacity, 0.0);
    }

    #[test]
    fn what_works_on_the_paint_underneath_is_carried_and_not_painted() {
        // None of the three is painted: a dab that works on the paint
        // under it needs the layer kept as pixels, and every stroke here
        // is redrawn from its curves each frame.
        for p in [Property::Strength, Property::Blending, Property::Dilution] {
            assert!(!p.honored(), "{:?} promises a read the canvas cannot do", p);
            assert_eq!(p.section(), Section::Paint);
        }
        // An imported brush that asks for it keeps asking, so the
        // library can say what it is not painting.
        let lib = Library::acquired();
        let smudge = lib
            .sets()
            .iter()
            .flat_map(|s| &s.presets)
            .find(|p| p.brush.mark == Mark::Smudge)
            .expect("the fixture carries a smudge");
        assert!(smudge.brush.strength > 0.0 && !smudge.brush.mark.painted());
        // And nothing this build ships promises it.
        for p in &Library::default().sets()[0].presets {
            assert!(p.brush.mark.painted(), "{} is ink or an eraser", p.name);
            assert_eq!(p.brush.strength, 0.0, "{}", p.name);
        }
    }
    #[test]
    fn an_eraser_is_never_laid_straight_onto_the_board() {
        let rubber = Brush {
            opacity: 1.0,
            hardness: 1.0,
            flow: 1.0,
            pressure: Pressure::NONE,
            mark: Mark::Erase,
            ..Brush::default()
        };
        let tip = rubber.tip(Face::Round);
        assert!(
            !tip.is_direct(),
            "it covers nothing: it has to come out of a sheet"
        );
        assert!(tip.erases());
        assert_eq!(tip.lands(), Blend::Erase);
        // And a brush that paints is laid over what is there.
        let ink = Brush {
            mark: Mark::Ink,
            ..rubber
        };
        assert!(!ink.tip(Face::Round).erases());
        assert_eq!(ink.tip(Face::Round).lands(), Blend::Over);
        assert!(!Tip::PENCIL.erases(), "a pencil has no nib to rub with");
    }

    #[test]
    fn the_shipped_shelf_has_two_erasers_and_the_rest_lay_ink() {
        let lib = Library::default();
        let own = &lib.sets()[0].presets;
        let erasers: Vec<&str> = own
            .iter()
            .filter(|p| p.tip().erases())
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(erasers, ["Eraser", "Soft Eraser"]);
        assert_eq!(
            own.iter().filter(|p| p.brush.mark == Mark::Ink).count(),
            own.len() - 2
        );
    }
    #[test]
    fn a_nib_the_pen_thins_the_ink_of_has_to_be_composited() {
        let solid = Brush {
            opacity: 1.0,
            hardness: 1.0,
            flow: 1.0,
            pressure: Pressure::NONE,
            ..Brush::default()
        };
        assert!(
            solid.tip(Face::Round).is_direct(),
            "a dab that covers on its own goes straight onto the board"
        );
        // The pen driving the width changes nothing: every dab still
        // covers, whatever it is wide.
        let thin = Brush {
            pressure: Pressure {
                size: 1.0,
                ..Pressure::NONE
            },
            ..solid
        };
        assert!(thin.tip(Face::Round).is_direct());
        for driven in [
            Pressure {
                flow: 0.5,
                ..Pressure::NONE
            },
            Pressure {
                opacity: 0.5,
                ..Pressure::NONE
            },
        ] {
            let b = Brush {
                pressure: driven,
                ..solid
            };
            assert!(
                !b.tip(Face::Round).is_direct(),
                "a lighter dab must build in the scratch, not on the board"
            );
        }
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
            pen: Envelope::default(),
        };
        assert_eq!(
            Tip::of(&p),
            Tip {
                width: 7.0,
                opacity: 0.25,
                hardness: 0.75,
                dynamics: Dynamics::None,
                stamp: None,
            }
        );
    }

    #[test]
    fn ring_is_a_closed_polyline_around_the_centre() {
        let (cx, cy, r) = (100.0, 50.0, 12.0);
        let prims = ring_prims((cx, cy), (r, r), 0.0, 2.0, [0.0, 0.0, 0.0, 1.0]);
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
    fn the_ring_is_the_shape_the_nib_will_lay_and_leans_with_it() {
        let (cx, cy) = (100.0, 50.0);
        let ends = |prims: &[Prim]| {
            let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
            for p in prims {
                for (x, y) in [(p.geom[0], p.geom[1]), (p.geom[2], p.geom[3])] {
                    lo = (lo.0.min(x), lo.1.min(y));
                    hi = (hi.0.max(x), hi.1.max(y));
                }
            }
            (hi.0 - lo.0, hi.1 - lo.1)
        };
        // Squished, the ring is as flat as the dab: it reaches 40 px
        // across and 10 down, which is the box `Prim::dab` would fill.
        let flat = ring_prims((cx, cy), (20.0, 5.0), 0.0, 1.0, [0.0; 4]);
        let (w, h) = ends(&flat);
        assert!((w - 40.0).abs() < 1e-3 && (h - 10.0).abs() < 1e-3, "{w}x{h}");
        // Turned a quarter, the same nib stands on end.
        let turned = ring_prims((cx, cy), (20.0, 5.0), std::f32::consts::FRAC_PI_2, 1.0, [0.0; 4]);
        let (w, h) = ends(&turned);
        assert!((w - 10.0).abs() < 1e-3 && (h - 40.0).abs() < 1e-3, "{w}x{h}");
        // And the body is what asks for it: a round brush at zoom 1 is
        // its own size across, a squished one flatter.
        let round = Brush {
            size: 30.0,
            roundness: 1.0,
            rotation: 90.0,
            ..Brush::default()
        };
        assert_eq!(ring_of(&round, 1.0), ((15.0, 15.0), std::f32::consts::FRAC_PI_2));
        let squished = Brush {
            roundness: 0.25,
            ..round
        };
        assert_eq!(ring_of(&squished, 1.0).0, (15.0, 3.75));
        // However flat it is squished, the ring is never let vanish —
        // the ink is not, either.
        let hair = Brush {
            roundness: 0.0,
            ..round
        };
        assert_eq!(ring_of(&hair, 1.0).0, (15.0, NIB_MIN_PX));
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
        assert_eq!(
            b.pressure,
            Pressure {
                size: 1.0,
                opacity: 0.0,
                flow: 0.0,
            },
            "the pen's pressure drives the size, as every drawing app does"
        );
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
                size: 0.0,
                opacity: 0.0,
                flow: 0.0,
            },
            "a nib the pen cannot lean on: what a board says by saying nothing"
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
                    | Property::JitterSize
                    | Property::JitterRotation
                    | Property::TextureDepth
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
    fn the_shipped_library_is_one_shelf_of_its_own() {
        let lib = Library::default();
        assert_eq!(lib.sets().len(), 1, "nothing imported, nothing else");
        assert_eq!(lib.sets()[0].name, OWN_SET);
        assert_eq!(lib.sets()[0].presets.len(), SLOTS - 1, "one to a seat");
        assert_eq!(lib.selected(), (0, 0), "the first brush of the shelf");
        assert_eq!(*lib.brush(), lib.sets()[0].presets[0].brush);
        assert_eq!(lib.icon_grid(), (0, 0), "and no sheet to cut");
        for p in &lib.sets()[0].presets {
            assert_eq!(p.icon, Icon::Drawn, "{} carries no art", p.name);
            assert_eq!(p.face(), Face::Round, "{}", p.name);
            assert!(p.paper.is_none(), "{}", p.name);
        }
    }

    #[test]
    fn an_import_comes_after_the_shipped_shelf() {
        let lib = Library::acquired();
        let names: Vec<&str> = lib.sets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [OWN_SET, "Splatter", "Stamps", "Grains", "Papers"]);
        assert_eq!(lib.selected(), (0, 0), "the hand opens on the shipped shelf");
        assert_eq!(lib.slots(), Library::default().slots(), "and so do the seats");
        assert_eq!(lib.icon_grid(), (16, 186), "a cell for every brush the sets drew");
    }

    #[test]
    fn nothing_imported_or_nothing_readable_is_the_shipped_shelf() {
        assert_eq!(Library::with_imported(None), Library::default());
        assert_eq!(Library::with_imported(Some("{ not json")), Library::default());
        assert_eq!(
            Library::with_imported(Some(r#"{"sets": []}"#)),
            Library::default()
        );
    }

    #[test]
    fn an_imported_shelf_cannot_take_a_name_already_on_the_library() {
        let json = serde_json::json!({
            "sets": [
                { "name": OWN_SET, "brushes": [{ "name": "Pencil", "brush": { "size": 99.0 } }] },
                { "name": "Twice", "brushes": [{ "name": "A", "brush": {} }] },
                { "name": "Twice", "brushes": [{ "name": "B", "brush": {} }] },
            ],
        });
        let lib = Library::with_imported(Some(&json.to_string()));
        let names: Vec<&str> = lib.sets().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [OWN_SET, "Twice"], "the first of a name stands");
        assert_ne!(lib.sets()[0].presets[0].brush.size, 99.0);
    }

    #[test]
    fn an_imported_brush_is_held_inside_the_bands_its_sliders_run_over() {
        let json = serde_json::json!({
            "sets": [{ "name": "Wide", "brushes": [{
                "name": "Huge",
                "brush": { "size": 5000.0, "opacity": -3.0 },
                "shape": "not on the sheet",
                "paper": { "name": "nor this", "period": 100.0 },
            }] }],
        });
        let lib = Library::with_imported(Some(&json.to_string()));
        let huge = &lib.sets()[1].presets[0];
        assert_eq!((huge.brush.size, huge.brush.opacity), (SIZE_MAX, 0.0));
        assert_eq!(huge.face(), Face::Round, "a nib the sheet lacks is no nib");
        assert!(huge.paper.is_none(), "nor a paper");
        assert_eq!(huge.icon, Icon::Drawn, "and no icon is drawn");
    }
    #[test]
    fn every_imported_brush_wears_a_cell_of_its_own_or_is_drawn() {
        let lib = Library::acquired();
        let (_, count) = lib.icon_grid();
        let mut cells: Vec<u16> = lib
            .sets()
            .iter()
            .flat_map(|s| &s.presets)
            .filter_map(|p| match p.icon {
                Icon::Sheet(cell) => Some(cell),
                Icon::Drawn => None,
            })
            .collect();
        let all = cells.len();
        cells.sort_unstable();
        cells.dedup();
        assert_eq!(cells.len(), all, "two brushes share an icon");
        assert!(cells.iter().all(|&i| i < count), "an icon off the sheet");
        let drawn = lib.sets()[1].presets.last().expect("a brush");
        assert_eq!(drawn.icon, Icon::Drawn, "one the set gave no icon is drawn");
    }
    #[test]
    fn a_brush_dragged_over_a_paper_names_one_the_sheet_has() {
        let lib = Library::acquired();
        let sheet = lib.sheet(3);
        let mut widest: f64 = 0.0;
        for set in lib.sets() {
            for p in &set.presets {
                let Some(paper) = &p.paper else { continue };
                assert!(
                    sheet.paper(&paper.name).is_some(),
                    "{} names {}, which is not on the sheet",
                    p.name,
                    paper.name
                );
                assert!(
                    paper.period > 0.0 && paper.period.is_finite(),
                    "{} has a tile of {}",
                    p.name,
                    paper.period
                );
                widest = widest.max(paper.period);
                // The papers are on the same sheet as the nibs, and a
                // name belongs to one band or the other, never both.
                assert!(sheet.cell(&paper.name).is_none(), "{}", paper.name);
            }
        }
        // A tile is a stretch of the board, not a stretch of the nib:
        // the widest of them covers hundreds of world units.
        assert!(widest > 500.0, "the widest tile is {widest}");
    }

    #[test]
    fn a_preset_hands_the_tip_its_paper_and_the_body_says_how_deep() {
        let mut lib = Library::acquired();
        let papered = lib
            .sets()
            .iter()
            .enumerate()
            .find_map(|(s, set)| {
                let i = set.presets.iter().position(|p| p.paper.is_some())?;
                Some((s, i))
            })
            .expect("some imported brush is dragged over a paper");
        lib.select(papered.0, papered.1);
        let paper = lib.sets()[papered.0].presets[papered.1]
            .paper
            .clone()
            .expect("it kept its paper");
        let nib = lib.tip().stamp.expect("a brush stamps");
        assert_eq!(
            nib.paper,
            Some(crate::doc::Paper {
                name: paper.name.clone(),
                period: paper.period,
                depth: lib.brush().texture_depth,
            })
        );
        // Depth is the one half of it a slider reaches, and at the
        // bottom of its track the paper is simply not there.
        Property::TextureDepth.set(lib.brush_mut(), 0.0);
        assert_eq!(lib.tip().stamp.and_then(|s| s.paper), None);
        Property::TextureDepth.set(lib.brush_mut(), 0.5);
        assert_eq!(
            lib.tip().stamp.and_then(|s| s.paper).map(|p| p.depth),
            Some(0.5)
        );
    }

    #[test]
    fn a_paper_and_a_nib_are_not_alternatives() {
        let lib = Library::acquired();
        let all: Vec<&Preset> = lib.sets().iter().flat_map(|s| s.presets.iter()).collect();
        let both = all
            .iter()
            .filter(|p| p.paper.is_some() && p.face() != Face::Round)
            .count();
        assert_eq!(both, 3, "a nib and a paper at once, one dab wearing both");
    }
    #[test]
    fn the_shipped_shelf_carries_every_one_of_the_four_profiles() {
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
    fn every_brush_is_named_and_inside_its_own_ranges() {
        let lib = Library::acquired();
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
        // Every brush counts now: a tip names the nib it stamps, the
        // grain it wears and the paper it is dragged over, and the
        // canvas lays all three.
        let lib = Library::acquired();
        for s in lib.sets() {
            let mut tips: Vec<String> = s
                .presets
                .iter()
                .map(|p| format!("{:?}", p.tip()))
                .collect();
            let all = tips.len();
            tips.sort();
            tips.dedup();
            assert_eq!(tips.len(), all, "{} has two brushes painting alike", s.name);
        }
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
        let mut lib = Library::acquired();
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

    /// A seat holding a brush from a named shelf, for the tests that need
    /// one the shipped nine do not already hold.
    fn outside(lib: &Library, index: usize) -> (usize, usize) {
        lib.sets()
            .iter()
            .position(|s| s.name == "Splatter")
            .map(|s| (s, index))
            .expect("the acquired library has a Splatter shelf")
    }

    #[test]
    fn the_slots_ship_with_the_shelf_this_build_ships() {
        let lib = Library::default();
        let slots = lib.slots();
        assert_eq!(slots.len(), SLOTS);
        assert_eq!(slots[0], None, "the overflow seat starts empty");
        for (n, (set, name)) in SLOT_DEFAULTS.iter().enumerate() {
            let (s, i) = slots[n + 1].expect("a default sits in every seat 1..=9");
            assert_eq!(lib.sets()[s].name, *set);
            assert_eq!(lib.sets()[s].presets[i].name, *name);
        }
    }

    #[test]
    fn a_brush_taken_from_outside_the_slots_lands_in_slot_zero() {
        let mut lib = Library::acquired();
        let far = outside(&lib, 0);
        lib.select(far.0, far.1);
        assert_eq!(lib.slots()[0], Some(far), "the last one used, kept to hand");

        let inside = lib.slots()[1].unwrap();
        lib.select(inside.0, inside.1);
        assert_eq!(
            lib.slots()[0],
            Some(far),
            "one already in a seat leaves the overflow alone"
        );
    }

    #[test]
    fn a_slot_takes_up_its_brush_and_an_empty_one_takes_nothing() {
        let mut lib = Library::default();
        let seat = lib.slots()[3].unwrap();
        assert!(lib.take_slot(3));
        assert_eq!(lib.selected(), seat);
        assert!(!lib.take_slot(0), "nothing has overflowed yet");
        assert!(!lib.take_slot(SLOTS), "and there is no eleventh seat");
    }

    #[test]
    fn assigning_a_brush_to_a_seat_clears_the_overflow_it_came_from() {
        let mut lib = Library::acquired();
        let far = outside(&lib, 0);
        lib.select(far.0, far.1);
        assert_eq!(lib.slots()[0], Some(far));

        assert!(lib.assign_slot(5, far));
        assert_eq!(lib.slots()[5], Some(far));
        assert_eq!(lib.slots()[0], None, "it is no longer outside the seats");
        assert!(
            !lib.assign_slot(0, far),
            "slot 0 is computed, never assigned"
        );
    }

    #[test]
    fn the_slots_go_to_disk_by_name_and_come_back() {
        let mut lib = Library::acquired();
        let far = outside(&lib, 2);
        assert!(lib.assign_slot(2, far));
        let edits = lib.edits();
        assert_eq!(edits.slots.len(), SLOTS);
        let held = edits.slots[2].clone().expect("seat 2 names its brush");
        assert_eq!(held.set, lib.sets()[far.0].name);
        assert_eq!(held.name, lib.sets()[far.0].presets[far.1].name);

        let mut fresh = Library::acquired();
        fresh.apply(&edits);
        assert_eq!(fresh.slots(), lib.slots());
    }

    #[test]
    fn edits_written_before_slots_existed_open_with_the_shipped_nine() {
        let mut lib = Library::default();
        let before: Edits = serde_json::from_str("{}").unwrap();
        assert!(before.slots.is_empty(), "absent on disk");
        lib.apply(&before);
        assert_eq!(lib.slots(), Library::default().slots());
    }

    #[test]
    fn seats_nobody_rearranged_are_not_written_down() {
        let mut lib = Library::acquired();
        assert!(
            lib.edits().slots.is_empty(),
            "the shipped nine are not a change"
        );
        let far = outside(&lib, 0);
        lib.select(far.0, far.1);
        assert_eq!(
            lib.edits().slots.len(),
            SLOTS,
            "but reaching outside them is"
        );
    }

    #[test]
    fn a_slot_naming_a_brush_this_build_dropped_opens_empty() {
        let mut lib = Library::default();
        let mut slots = vec![None; SLOTS];
        slots[4] = Some(Held {
            set: "Gone".to_owned(),
            name: "Vanished".to_owned(),
        });
        lib.apply(&Edits {
            slots,
            ..Edits::default()
        });
        assert_eq!(lib.slots()[4], None, "passed over, not refused");
    }
}
