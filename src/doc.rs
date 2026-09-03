//! Board document: retained scene, versioned JSON (ARCHITECTURE.md §6.1).
//!
//! The document is pure data. Camera and geometry transforms live in
//! `scene`; disk I/O lives in `store`.

use serde::{Deserialize, Serialize};

use crate::curve::{self, Cubic};

/// Schema version this binary writes and accepts.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema: u32,
    pub id: String,
    pub title: String,
    pub camera: Camera,
    /// Bottom to top. Never empty once parsed: a board written before
    /// layers existed gets one on the way in (see [`Document::from_json`]).
    #[serde(default)]
    pub layers: Vec<Layer>,
    pub elements: Vec<Element>,
}

/// What a layer holds. A raster layer accumulates — every brush stroke
/// and every pasted bitmap joins the ones already on it, as pixels would;
/// a vector layer holds the one object it was made for. It is what
/// decides where new ink lands (see `editor::Editor::ink_layer`).
///
/// Raster is the default, and absent on disk: a board written before
/// kinds existed is a stack that accumulates, so it opens meaning what it
/// always meant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Raster,
    Vector,
    /// The layer a frame is the object of. It never appears inside a
    /// frame's own stack: a frame does not nest.
    Frame,
}

impl Kind {
    fn is_raster(&self) -> bool {
        *self == Kind::Raster
    }
}

/// A layer: a name, what it holds, whether it shows, and a place in the
/// order. Elements name it by id. `visible` is absent on disk when true
/// and `kind` when raster, so boards that never hid anything — or never
/// saw a vector layer — keep their shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Kind::is_raster")]
    pub kind: Kind,
}

impl Layer {
    /// A visible raster layer with a fresh ULID.
    pub fn new(name: &str) -> Layer {
        Layer {
            id: new_id(),
            name: name.to_owned(),
            visible: true,
            kind: Kind::Raster,
        }
    }

    /// The same, holding `kind`.
    pub fn of(name: &str, kind: Kind) -> Layer {
        Layer {
            kind,
            ..Layer::new(name)
        }
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

/// Whether `v` is a fraction — what `opacity` and `hardness` have to be.
fn is_unit(v: f64) -> bool {
    (0.0..=1.0).contains(&v)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

/// Scene elements (§6.1). Types enter as their tools exist: `rect` from the
/// scaffold, `path` with the pencil, `image` with the clipboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Element {
    Rect(Rect),
    Path(Path),
    Paint(Paint),
    Image(Image),
    Frame(Frame),
}

impl Element {
    pub fn id(&self) -> &str {
        match self {
            Element::Rect(r) => &r.id,
            Element::Path(p) => &p.id,
            Element::Paint(p) => &p.id,
            Element::Image(i) => &i.id,
            Element::Frame(f) => &f.id,
        }
    }

    /// The id of the layer this element is on.
    pub fn layer(&self) -> &str {
        match self {
            Element::Rect(r) => &r.layer,
            Element::Path(p) => &p.layer,
            Element::Paint(p) => &p.layer,
            Element::Image(i) => &i.layer,
            Element::Frame(f) => &f.layer,
        }
    }

    pub fn set_layer(&mut self, id: &str) {
        let layer = match self {
            Element::Rect(r) => &mut r.layer,
            Element::Path(p) => &mut p.layer,
            Element::Paint(p) => &mut p.layer,
            Element::Image(i) => &mut i.layer,
            Element::Frame(f) => &mut f.layer,
        };
        id.clone_into(layer);
    }
}

/// `x, y, w, h` is the box before rotation; `rotation` turns it about its
/// center, in degrees, clockwise on screen (y down, as in SVG). Absent on
/// disk when zero, so unrotated boards keep the §6.1 shape. `layer` names
/// the layer the rect is on; absent on disk, it is the first one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    pub stroke: Option<String>,
    pub fill: Option<String>,
    pub text: Option<String>,
}

/// An area that holds objects, with a stack of its own. `x, y, w, h` is
/// the boundary in world units; it does not turn, because the cut that
/// makes it a frame is an axis-aligned box in the shader. `background`
/// is the colour under everything inside it — an unvalidated hex, as a
/// rect's `fill` is — absent on disk when the frame is clear. `layers`
/// is its own stack, bottom to top: never empty once parsed, and never
/// holding a frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

impl Frame {
    /// Whether the area holds `p`, its edges included.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        p[0] >= self.x && p[0] <= self.x + self.w && p[1] >= self.y && p[1] <= self.y + self.h
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// How far each dab is thrown off the nib, in the unit of the thing it
/// throws: `size` a radius in world units, `rotation` degrees. What is
/// left of Sketchbook's Randomness once the three amounts whose scale
/// its own assets contradict are set aside — see [`Property::honored`].
/// All zero is a nib laid true, which is what a board without one says.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Scatter {
    pub size: f64,
    pub rotation: f64,
}

impl Scatter {
    /// Nothing thrown at all: absent on disk, and the walk skips the
    /// dice for it.
    pub fn is_true(&self) -> bool {
        *self == Scatter::default()
    }

    /// The same scatter if the canvas could throw by it, or why not. An
    /// amount is a distance, never a direction: negative is refused
    /// rather than folded, so a board cannot say something it does not
    /// mean.
    fn checked(self) -> Result<Scatter, String> {
        for (what, amount) in [("size", self.size), ("rotation", self.rotation)] {
            if !(amount.is_finite() && amount >= 0.0) {
                return Err(format!("{what} scatter {amount} is not an amount"));
            }
        }
        Ok(self)
    }
}

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

    /// How it bends the ramp the Edge spends, as an exponent on the
    /// coverage across it: below one the nib stays full almost to its
    /// own edge, above one it fades over the whole band.
    ///
    /// Sketchbook does not say what its four curves are. Its own sets
    /// do: every brush ships an Edge to go with its profile, and the
    /// two move together — airbrush never above 0.28, regularSolid up
    /// to 0.75, hardSolid at 0.88 to a brush, sharp from 0.89 up. So
    /// the profile is read in the order the sets themselves put it in,
    /// and it shapes the ramp whose width the Edge sets.
    pub fn falloff(self) -> f32 {
        match self {
            Profile::Sharp => 0.5,
            Profile::HardSolid => 0.7,
            Profile::RegularSolid => 1.0,
            Profile::Airbrush => 2.0,
        }
    }

    /// A plain ramp: absent on disk, and what a board that never named
    /// one means.
    fn is_regular(&self) -> bool {
        *self == Profile::RegularSolid
    }
}

/// What a dab does to what is already on the sheet. Sketchbook's stamp
/// blend style, which 91 of its 211 brushes name something other than
/// `normal`: most lay ink over it, an eraser takes ink away, and the
/// rest mix with the paint underneath — reading the sheet as well as
/// writing it, which is another engine. Until it is written they lay
/// ink like the rest, and [`Mark::painted`] is what says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mark {
    /// Ink over what is there: 120 of the brushes, and the pencil.
    #[default]
    #[serde(rename = "normal")]
    Ink,
    /// Ink taken away: the eight erasers.
    #[serde(rename = "eraser")]
    Erase,
    #[serde(rename = "acrylic")]
    Acrylic,
    #[serde(rename = "marker")]
    Marker,
    #[serde(rename = "pastel")]
    Pastel,
    #[serde(rename = "smudge")]
    Smudge,
    #[serde(rename = "makerColorless")]
    Colorless,
    #[serde(rename = "glowBrush")]
    Glow,
}

impl Mark {
    /// Whether the canvas does what it says. Ink and the eraser, so
    /// far: the other six read the paint under the dab, which is a
    /// different engine, and a brush naming one of them lays plain ink
    /// rather than promising a mixture it cannot make.
    #[allow(dead_code)] // the library is held to it, brush by brush
    pub fn painted(self) -> bool {
        matches!(self, Mark::Ink | Mark::Erase)
    }

    /// Whether it takes ink away instead of laying it. An erasing
    /// stroke is never drawn straight onto the board: it is composited
    /// out of the sheet its own layer is built on, so it rubs out that
    /// layer and nothing under it.
    pub fn erases(self) -> bool {
        self == Mark::Erase
    }

    /// Plain ink: absent on disk, and what a board that never named one
    /// means.
    fn is_ink(&self) -> bool {
        *self == Mark::Ink
    }
}

/// How much of each property the pen's pressure drives, 0–1: 0 is one
/// pressure never touches, 1 one it drives from nothing up to the value
/// the brush names. A light touch gives `v * (1 − amount)`, a heavy one
/// all of `v`, which is how Sketchbook states it — a brush names the
/// two ends and the amount is the gap between them.
///
/// All zero is a nib the pen cannot lean on, which is what a board
/// written before the pen was read means, and what a mouse makes of
/// any brush.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pressure {
    pub size: f64,
    pub opacity: f64,
    pub flow: f64,
}

impl Default for Pressure {
    fn default() -> Pressure {
        Pressure::NONE
    }
}

impl Pressure {
    /// A nib the pen cannot lean on: what a board says by saying
    /// nothing, and what a brush with no dynamics of its own lays.
    pub const NONE: Pressure = Pressure {
        size: 0.0,
        opacity: 0.0,
        flow: 0.0,
    };

    /// Nothing driven at all: absent on disk, and the walk lays every
    /// dab the same.
    pub fn is_none(&self) -> bool {
        *self == Pressure::NONE
    }

    /// What `v` is worth at pressure `p`: all of it under a full press,
    /// `1 − amount` of it under none.
    pub fn scale(amount: f64, p: f64) -> f64 {
        1.0 - amount.clamp(0.0, 1.0) * (1.0 - p.clamp(0.0, 1.0))
    }

    /// The same amounts if the canvas could drive by them, or why not.
    fn checked(self) -> Result<Pressure, String> {
        for (what, amount) in [
            ("size", self.size),
            ("opacity", self.opacity),
            ("flow", self.flow),
        ] {
            if !is_unit(amount) {
                return Err(format!("{what} pressure {amount} is not between 0 and 1"));
            }
        }
        Ok(self)
    }
}

/// What the pen did along a stroke: readings taken from its first sample
/// to its last, evenly spaced in the stroke's own arc length. The walk
/// asks for a reading at a fraction of the way along and gets the two
/// nearest mixed, so the same envelope reads the same whether the stroke
/// is still a polyline or has been fitted into curves.
///
/// Empty is a stroke drawn with no pen at all — pressed all the way,
/// the nib standing where the brush put it — which is what a mouse says
/// and what every board written before the pen was read means.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Envelope {
    /// How hard the tip was pressed, 0–1.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pressure: Vec<f32>,
    /// The turn the stylus's own lean gave the nib, in degrees.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub twist: Vec<f32>,
}

impl Envelope {
    /// Nothing the pen said: absent on disk, and the walk skips the
    /// lookup for it.
    pub fn is_empty(&self) -> bool {
        self.pressure.is_empty() && self.twist.is_empty()
    }

    /// How hard the tip was pressed `u` of the way along, 0 to 1. A
    /// stroke with no readings was pressed all the way.
    pub fn pressure_at(&self, u: f32) -> f32 {
        read_at(&self.pressure, u).unwrap_or(1.0)
    }

    /// The turn the stylus gave the nib there, in degrees; none when it
    /// never leaned on it.
    pub fn twist_at(&self, u: f32) -> f32 {
        read_at(&self.twist, u).unwrap_or(0.0)
    }

    /// The same envelope if the canvas could lay it, or why not. A
    /// pressure is a fraction and a twist is an angle; neither may be
    /// a number that is not one.
    fn checked(self) -> Result<Envelope, String> {
        for p in &self.pressure {
            if !(p.is_finite() && (0.0..=1.0).contains(p)) {
                return Err(format!("pressure {p} is not between 0 and 1"));
            }
        }
        for t in &self.twist {
            if !t.is_finite() {
                return Err(format!("twist {t} is not an angle"));
            }
        }
        Ok(self)
    }
}

/// One reading `u` of the way along a row of them, the two nearest
/// mixed. `None` when there are none to read.
fn read_at(readings: &[f32], u: f32) -> Option<f32> {
    match readings.len() {
        0 => None,
        1 => Some(readings[0]),
        n => {
            let at = u.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (at.floor() as usize).min(n - 2);
            let t = at - i as f32;
            Some(readings[i] + (readings[i + 1] - readings[i]) * t)
        }
    }
}

/// The paper a stroke was dragged over. It belongs to the canvas and
/// not to the nib, which is the whole difference between it and a
/// [`Stamp::grain`]: a grain is the nib's own and turns with it, while
/// the paper stands still under a nib that turns over it, so two
/// strokes crossing one place meet the same fibres. One tile of it
/// covers `period` world units, which is hundreds of them and not one
/// dab's worth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    /// Its image, by the name the brush library gives it — a name and
    /// never a cell, for the reason [`Stamp::shape`] is one.
    pub name: String,
    /// How wide one tile of it is, in world units.
    pub period: f64,
    /// How deep it bites into the ink, 0–1: at 0 the paper is not
    /// there, at 1 the ink is the paper's own coverage.
    pub depth: f64,
}

impl Paper {
    /// The same paper if the canvas could drag a nib over it, or why it
    /// could not.
    fn checked(self) -> Result<Paper, String> {
        if self.name.is_empty() {
            return Err("a paper is named or absent, never empty".into());
        }
        if !(self.period.is_finite() && self.period > 0.0) {
            return Err(format!("period {} is not a tile of a paper", self.period));
        }
        if !is_unit(self.depth) {
            return Err(format!("depth {} is not between 0 and 1", self.depth));
        }
        Ok(self)
    }
}

/// The nib a stroke was stamped with: what one dab is, beyond the
/// width, opacity and hardness the stroke already names. A stroke
/// carrying none was swept, not stamped — the pencil's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    /// The nib's own shape, by the name the brush library gives it, or
    /// `None` for a nib with no silhouette of its own. A name and not a
    /// cell of the sheet: a board outlives the sheet it was painted
    /// from, and an index would move the day a set is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// The grain the nib wears, by the same kind of name. Sketchbook's
    /// other kind of nib image, and not the same thing at all: a shape
    /// is stamped in place of the round dab, a grain is worn over one,
    /// so the dab keeps its own edge and the grain eats into it. A nib
    /// carries one or the other, never both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain: Option<String>,
    /// The gap between two dabs, in Sketchbook's own spacing units:
    /// 0.1 to 10, its Pencil's default 1.2, each unit a quarter of the
    /// nib's width (`scene::SPACING_UNIT`).
    pub spacing: f64,
    /// 1 is a round nib; less flattens it across its own y.
    pub roundness: f64,
    /// The nib's own angle, in degrees, clockwise.
    pub rotation: f64,
    /// What its edge does over the ramp the hardness leaves it.
    #[serde(default, skip_serializing_if = "Profile::is_regular")]
    pub profile: Profile,
    /// What one dab does to the ink already on the sheet.
    #[serde(default, skip_serializing_if = "Mark::is_ink")]
    pub mark: Mark,
    /// Whether the nib turns with the stroke, its own angle added to
    /// the heading. Sketchbook's Rotation Dynamics, less the two the
    /// stylus drives: a pattern that has to run along the stroke says
    /// so, and the rest stand still.
    #[serde(default, skip_serializing_if = "is_false")]
    pub follow: bool,
    /// What one dab lays, 0–1. The stroke builds toward its opacity
    /// where the dabs cross; 1 is a dab that covers on its own.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub flow: f64,
    /// How far each dab is thrown off it.
    #[serde(default, skip_serializing_if = "Scatter::is_true")]
    pub scatter: Scatter,
    /// How much of the nib the pen's pressure drives. The readings
    /// themselves are the stroke's, in its [`Envelope`]: this is only
    /// how much they are worth.
    #[serde(default, skip_serializing_if = "Pressure::is_none")]
    pub pressure: Pressure,
    /// The paper the nib was dragged over, when it was dragged over
    /// one. Absent on disk otherwise, which is most strokes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<Paper>,
}

impl Stamp {
    /// The same nib if the canvas could stamp it, or why it could not.
    /// A board is not trusted to hold a turn of four hundred degrees or
    /// a gap of nothing, whatever wrote it.
    fn checked(self) -> Result<Stamp, String> {
        for (what, name) in [("shape", &self.shape), ("grain", &self.grain)] {
            if name.as_ref().is_some_and(|s| s.is_empty()) {
                return Err(format!("a nib's {what} is named or absent, never empty"));
            }
        }
        if self.shape.is_some() && self.grain.is_some() {
            return Err("a nib stamps a shape or wears a grain, never both".into());
        }
        if !is_unit(self.roundness) {
            return Err(format!("roundness {} is not between 0 and 1", self.roundness));
        }
        if !(self.spacing.is_finite() && self.spacing > 0.0) {
            return Err(format!("spacing {} is not a gap between two dabs", self.spacing));
        }
        if !(self.rotation.is_finite() && (0.0..=360.0).contains(&self.rotation)) {
            return Err(format!("rotation {} is not a turn of a nib", self.rotation));
        }
        if !is_unit(self.flow) {
            return Err(format!("flow {} is not between 0 and 1", self.flow));
        }
        self.scatter.checked()?;
        self.pressure.checked()?;
        let paper = self.paper.map(Paper::checked).transpose()?;
        Ok(Stamp { paper, ..self })
    }
}

/// Freehand stroke — the pencil's or the brush's: a chain of cubic
/// Béziers in world units, each one self-contained as `[a, c1, c2, b]`
/// and starting where the previous ended. `width` is in world units too
/// (ink scales with zoom). `opacity` is the stroke's as one shape —
/// where it crosses itself it does not darken — and `hardness` is how
/// much of its radius is crisp: 1 is the pencil's edge, 0 fades from the
/// center out. Both are fractions, both absent on disk when 1. Transforms
/// are baked into the curves; `rotation` (degrees, clockwise on screen)
/// only records how far the stroke has been turned since it was drawn, so
/// its box turns with it and snapping counts from the creation state.
/// Absent on disk when zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PathOnDisk")]
pub struct Path {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub curves: Vec<Cubic>,
    pub stroke: String,
    pub width: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub hardness: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// The nib it was stamped with, if it was stamped at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
    /// What the pen said while it was drawn.
    #[serde(default, skip_serializing_if = "Envelope::is_empty")]
    pub pen: Envelope,
}

/// What a `path` may look like on disk: `curves` today, or the raw
/// `points` polyline boards held before the Bézier fit landed. Legacy
/// polylines are fitted on load and written back as `curves` on the next
/// save.
#[derive(Deserialize)]
struct PathOnDisk {
    id: String,
    #[serde(default)]
    layer: String,
    curves: Option<Vec<Cubic>>,
    points: Option<Vec<[f64; 2]>>,
    stroke: String,
    width: f64,
    #[serde(default = "one")]
    opacity: f64,
    #[serde(default = "one")]
    hardness: f64,
    #[serde(default)]
    rotation: f64,
    #[serde(default)]
    stamp: Option<Stamp>,
    #[serde(default)]
    pen: Envelope,
}

/// Fit tolerance for legacy polylines, in world units (one pixel at
/// zoom 1, the pencil's own default).
const LEGACY_FIT_TOLERANCE: f64 = 1.0;

impl TryFrom<PathOnDisk> for Path {
    type Error = String;

    fn try_from(p: PathOnDisk) -> Result<Path, String> {
        let curves = match (p.curves, p.points) {
            (Some(curves), _) => curves,
            (None, Some(points)) => curve::fit(
                &curve::simplify(&points, LEGACY_FIT_TOLERANCE),
                LEGACY_FIT_TOLERANCE,
            ),
            (None, None) => return Err("path needs `curves`".into()),
        };
        if !is_unit(p.opacity) {
            return Err(format!("opacity {} is not between 0 and 1", p.opacity));
        }
        if !is_unit(p.hardness) {
            return Err(format!("hardness {} is not between 0 and 1", p.hardness));
        }
        Ok(Path {
            id: p.id,
            layer: p.layer,
            curves,
            stroke: p.stroke,
            width: p.width,
            opacity: p.opacity,
            hardness: p.hardness,
            rotation: p.rotation,
            stamp: p.stamp.map(Stamp::checked).transpose()?,
            pen: p.pen.checked()?,
        })
    }
}

/// The painting on a raster layer: one object, however many strokes went
/// into it. A raster layer accumulates — a second brush stroke joins the
/// paint already there instead of becoming an element of its own — so the
/// layer holds one thing, which moves, turns and is deleted as one.
/// Every stroke keeps the ink it was laid with, because that is what a
/// layer of pixels would have kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paint {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub strokes: Vec<Stroke>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
}

/// One stroke of a [`Paint`]: a `path` without an identity of its own.
/// The curves are a chain, as a path's are; two strokes are not, which is
/// why they cannot share one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StrokeOnDisk")]
pub struct Stroke {
    pub curves: Vec<Cubic>,
    pub stroke: String,
    pub width: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub hardness: f64,
    /// The nib it was stamped with, if it was stamped at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
    /// What the pen said while it was drawn.
    #[serde(default, skip_serializing_if = "Envelope::is_empty")]
    pub pen: Envelope,
}

/// What a stroke may look like on disk. Checked on the way in, like a
/// path's: a fraction is a fraction.
#[derive(Deserialize)]
struct StrokeOnDisk {
    curves: Vec<Cubic>,
    stroke: String,
    width: f64,
    #[serde(default = "one")]
    opacity: f64,
    #[serde(default = "one")]
    hardness: f64,
    #[serde(default)]
    stamp: Option<Stamp>,
    #[serde(default)]
    pen: Envelope,
}

impl TryFrom<StrokeOnDisk> for Stroke {
    type Error = String;

    fn try_from(s: StrokeOnDisk) -> Result<Stroke, String> {
        if !is_unit(s.opacity) {
            return Err(format!("opacity {} is not between 0 and 1", s.opacity));
        }
        if !is_unit(s.hardness) {
            return Err(format!("hardness {} is not between 0 and 1", s.hardness));
        }
        Ok(Stroke {
            curves: s.curves,
            stroke: s.stroke,
            width: s.width,
            opacity: s.opacity,
            hardness: s.hardness,
            stamp: s.stamp.map(Stamp::checked).transpose()?,
            pen: s.pen.checked()?,
        })
    }
}

/// A bitmap on the board. `x, y, w, h` is the box before rotation, in
/// world units, and `rotation` turns it about its center exactly as a
/// rect's does. `blob` names the original bytes in the blob store by their
/// sha256 — never a path, so a board says nothing about the machine that
/// wrote it (§7.1, §9.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ImageOnDisk")]
pub struct Image {
    pub id: String,
    #[serde(default)]
    pub layer: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    pub blob: String,
}

/// What an `image` may look like on disk. The blob is checked on the way
/// in: a hand-edited board must not be able to name a file outside the
/// store.
#[derive(Deserialize)]
struct ImageOnDisk {
    id: String,
    #[serde(default)]
    layer: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    #[serde(default)]
    rotation: f64,
    blob: String,
}

impl TryFrom<ImageOnDisk> for Image {
    type Error = String;

    fn try_from(i: ImageOnDisk) -> Result<Image, String> {
        if !is_blob_hash(&i.blob) {
            return Err(format!("blob {:?} is not a sha256 hash", i.blob));
        }
        Ok(Image {
            id: i.id,
            layer: i.layer,
            x: i.x,
            y: i.y,
            w: i.w,
            h: i.h,
            rotation: i.rotation,
            blob: i.blob,
        })
    }
}

/// Whether `s` is a blob name: a bare sha256 in lowercase hex. 64 such
/// characters can only ever name a file directly inside `blobs/`.
pub fn is_blob_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

impl Document {
    /// New, empty document with a ULID id, one layer and the camera at
    /// the origin.
    pub fn new(title: &str) -> Self {
        Document {
            schema: SCHEMA_VERSION,
            id: new_id(),
            title: title.to_owned(),
            camera: Camera::default(),
            layers: vec![Layer::new("Layer 1")],
            elements: Vec::new(),
        }
    }

    /// Deserializes and validates the schema version, then settles the
    /// layers: a board without any gets one, an element without one joins
    /// the first, and a layer that is named has to exist.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let mut doc: Document = serde_json::from_str(s)?;
        if doc.schema != SCHEMA_VERSION {
            anyhow::bail!(
                "schema {} not supported (this binary speaks schema {})",
                doc.schema,
                SCHEMA_VERSION
            );
        }
        doc.settle_layers()?;
        Ok(doc)
    }

    /// Settles both stacks: a board without layers gets one, a frame
    /// without layers gets one too, every layer id is unique across the
    /// whole document, a frame layer and its frame are one thing that
    /// neither half stands without, no frame nests, and every element
    /// names a layer that exists in some stack.
    fn settle_layers(&mut self) -> anyhow::Result<()> {
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Layer 1"));
        }
        // A frame's area has to be one, and its stack is never empty —
        // the same rule the board keeps, so every accessor's assumption
        // that there is a layer to paint on holds in both stacks.
        for el in &mut self.elements {
            if let Element::Frame(f) = el {
                if !(f.w.is_finite() && f.h.is_finite() && f.w > 0.0 && f.h > 0.0) {
                    anyhow::bail!("frame {:?} has no area: {} by {}", f.id, f.w, f.h);
                }
                if f.layers.is_empty() {
                    f.layers.push(Layer::new("Layer 1"));
                }
            }
        }
        // Every layer of every stack, board first, checked for an id and
        // for being used only once in the whole document — which is what
        // lets an element name its layer and say nothing else about
        // where it is.
        let mut seen: Vec<&str> = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.id.is_empty() {
                anyhow::bail!("layer {i} has no id");
            }
            if seen.contains(&layer.id.as_str()) {
                anyhow::bail!("layer id {:?} is used twice", layer.id);
            }
            seen.push(&layer.id);
        }
        for el in &self.elements {
            let Element::Frame(f) = el else { continue };
            for layer in &f.layers {
                if layer.id.is_empty() {
                    anyhow::bail!("a layer of frame {:?} has no id", f.id);
                }
                if layer.kind == Kind::Frame {
                    anyhow::bail!(
                        "layer {:?} is a frame inside frame {:?}; frames do not nest",
                        layer.id,
                        f.id
                    );
                }
                if seen.contains(&layer.id.as_str()) {
                    anyhow::bail!("layer id {:?} is used twice", layer.id);
                }
                seen.push(&layer.id);
            }
        }
        // A frame layer and its frame go together in both directions.
        for layer in &self.layers {
            if layer.kind == Kind::Frame && self.frame_on(&layer.id).is_none() {
                anyhow::bail!("layer {:?} is a frame layer with no frame on it", layer.id);
            }
        }
        for el in &self.elements {
            let Element::Frame(f) = el else { continue };
            if !self
                .layers
                .iter()
                .any(|l| l.id == f.layer && l.kind == Kind::Frame)
            {
                anyhow::bail!(
                    "frame {:?} names layer {:?}, which is not a frame layer",
                    f.id,
                    f.layer
                );
            }
        }
        let first = self.layers[0].id.clone();
        let known: Vec<String> = seen.iter().map(|s| (*s).to_owned()).collect();
        for el in &mut self.elements {
            if el.layer().is_empty() {
                el.set_layer(&first);
            } else if !known.iter().any(|id| id == el.layer()) {
                anyhow::bail!(
                    "element {:?} names layer {:?}, which does not exist",
                    el.id(),
                    el.layer()
                );
            }
        }
        Ok(())
    }

    /// Serializes to the canonical on-disk JSON (pretty: debugging with
    /// $EDITOR is a stated goal of the local phase).
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

impl Document {
    /// The elements in paint order — bottom layer first, document order
    /// within a layer — each with its index in `elements`. Hidden layers
    /// are skipped: what is not painted is not there. Double-ended, so
    /// the pointer can walk it from the top.
    pub fn painted(&self) -> impl DoubleEndedIterator<Item = (usize, &Element)> {
        self.layers
            .iter()
            .filter(|layer| layer.visible)
            .flat_map(move |layer| {
                self.elements
                    .iter()
                    .enumerate()
                    .filter(move |(_, el)| el.layer() == layer.id)
            })
    }

    pub fn layer_index(&self, id: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// The frame element `id` names.
    pub fn frame(&self, id: &str) -> Option<&Frame> {
        self.elements.iter().find_map(|el| match el {
            Element::Frame(f) if f.id == id => Some(f),
            _ => None,
        })
    }

    pub fn frame_mut(&mut self, id: &str) -> Option<&mut Frame> {
        self.elements.iter_mut().find_map(|el| match el {
            Element::Frame(f) if f.id == id => Some(f),
            _ => None,
        })
    }

    /// The frame that `layer` is the layer of, when it is a frame's.
    pub fn frame_on(&self, layer: &str) -> Option<&Frame> {
        self.elements.iter().find_map(|el| match el {
            Element::Frame(f) if f.layer == layer => Some(f),
            _ => None,
        })
    }

    /// The layers of the frame `frame` names, or the board's own when it
    /// names none. An empty slice for a frame that is not there.
    pub fn stack(&self, frame: Option<&str>) -> &[Layer] {
        match frame {
            None => &self.layers,
            Some(id) => self.frame(id).map_or(&[], |f| &f.layers),
        }
    }

    pub fn stack_mut(&mut self, frame: Option<&str>) -> Option<&mut Vec<Layer>> {
        match frame {
            None => Some(&mut self.layers),
            Some(id) => self.frame_mut(id).map(|f| &mut f.layers),
        }
    }

    /// Where a layer lives: the frame holding it — none for the board's
    /// own stack — and its index in that stack. A layer id is unique
    /// across the whole document, so this is the whole answer.
    pub fn locate(&self, layer: &str) -> Option<(Option<&str>, usize)> {
        if let Some(i) = self.layers.iter().position(|l| l.id == layer) {
            return Some((None, i));
        }
        self.elements.iter().find_map(|el| match el {
            Element::Frame(f) => f
                .layers
                .iter()
                .position(|l| l.id == layer)
                .map(|i| (Some(f.id.as_str()), i)),
            _ => None,
        })
    }

    /// The topmost frame whose area holds `p`, or none for the open
    /// board. A hidden frame claims nothing: what is not painted is not
    /// there, for the pointer as for the eye.
    pub fn frame_at(&self, p: [f64; 2]) -> Option<&str> {
        self.layers
            .iter()
            .rev()
            .filter(|l| l.visible && l.kind == Kind::Frame)
            .find_map(|l| {
                self.frame_on(&l.id)
                    .filter(|f| f.contains(p))
                    .map(|f| f.id.as_str())
            })
    }

    /// Adds a layer of `kind` just above `above` (on top when that is
    /// past the end) and answers its index. It is named `Layer N` with N
    /// past every number in use, so a name is never handed out twice.
    pub fn add_layer(&mut self, above: usize, kind: Kind) -> usize {
        let name = self.next_layer_name();
        let at = above.saturating_add(1).min(self.layers.len());
        self.layers.insert(at, Layer::of(&name, kind));
        at
    }

    fn next_layer_name(&self) -> String {
        let highest = self
            .layers
            .iter()
            .filter_map(|l| l.name.strip_prefix("Layer ")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("Layer {}", highest.saturating_add(1))
    }

    /// Removes layer `index` and every element on it. A board keeps its
    /// last layer: false then, and for an index past the end.
    pub fn remove_layer(&mut self, index: usize) -> bool {
        if self.layers.len() < 2 || index >= self.layers.len() {
            return false;
        }
        let gone = self.layers.remove(index);
        self.elements.retain(|el| el.layer() != gone.id);
        true
    }

    /// Swaps layer `index` with the one above it (`up`) or below, and
    /// answers where it went. Nothing moves past the edge.
    pub fn move_layer(&mut self, index: usize, up: bool) -> Option<usize> {
        let to = if up {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        self.reorder_layer(index, to).then_some(to)
    }

    /// Takes layer `from` out of the stack and puts it back at `to`,
    /// shifting whatever lies between and leaving their order alone —
    /// what a row dragged several places down does. False when either
    /// index is past the end, or the layer is already there.
    pub fn reorder_layer(&mut self, from: usize, to: usize) -> bool {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return false;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        true
    }
}

/// New id in ULID format (stable, time-sortable — the bridge to a CRDT).
pub fn new_id() -> String {
    ulid::Ulid::from_datetime(std::time::SystemTime::now()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sha256 of the empty input — a valid hash, and easy to recognize.
    const BLOB: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// The one layer of [`sample_doc`].
    const L1: &str = "01JLAYER1LAYER1LAYER1LAYER";

    fn layer(id: &str, name: &str) -> Layer {
        Layer {
            id: id.into(),
            name: name.into(),
            visible: true,
            kind: Kind::Raster,
        }
    }

    fn sample_doc() -> Document {
        Document {
            schema: SCHEMA_VERSION,
            id: "01JTESTTESTTESTTESTTESTTES".into(),
            title: "auth flow".into(),
            camera: Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            layers: vec![layer(L1, "Layer 1")],
            elements: vec![
                Element::Rect(Rect {
                    id: "el_01".into(),
                    layer: L1.into(),
                    x: 40.0,
                    y: 80.0,
                    w: 220.0,
                    h: 80.0,
                    rotation: 0.0,
                    stroke: Some("#222".into()),
                    fill: None,
                    text: Some("API Gateway".into()),
                }),
                Element::Path(Path {
                    id: "el_02".into(),
                    layer: L1.into(),
                    curves: vec![[[1.0, 2.0], [2.0, 3.0], [3.5, 4.0], [6.0, 4.0]]],
                    stroke: "#1f1f1f".into(),
                    width: 2.0,
                    opacity: 1.0,
                    hardness: 1.0,
                    rotation: 0.0,
                    stamp: None,
                    pen: Envelope::default(),
                }),
                Element::Image(Image {
                    id: "el_03".into(),
                    layer: L1.into(),
                    x: 10.0,
                    y: 20.0,
                    w: 64.0,
                    h: 48.0,
                    rotation: 0.0,
                    blob: BLOB.into(),
                }),
            ],
        }
    }

    /// A board with `layers` and three elements, one per layer, in the
    /// JSON `elements` order `top, bottom, middle` — so paint order and
    /// document order disagree on purpose.
    fn three_layers() -> Document {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [
                { "id": "bottom", "name": "Layer 1" },
                { "id": "middle", "name": "Layer 2" },
                { "id": "top", "name": "Layer 3" }
            ],
            "elements": [
                { "id": "on_top", "type": "rect", "layer": "top",
                  "x": 0, "y": 0, "w": 1, "h": 1, "stroke": null, "fill": null, "text": null },
                { "id": "on_bottom", "type": "rect", "layer": "bottom",
                  "x": 0, "y": 0, "w": 1, "h": 1, "stroke": null, "fill": null, "text": null },
                { "id": "on_middle", "type": "rect", "layer": "middle",
                  "x": 0, "y": 0, "w": 1, "h": 1, "stroke": null, "fill": null, "text": null }
            ]
        }"##;
        Document::from_json(json).unwrap()
    }

    fn painted_ids(doc: &Document) -> Vec<(usize, &str)> {
        doc.painted().map(|(i, el)| (i, el.id())).collect()
    }

    #[test]
    fn new_document_has_one_visible_layer_named_layer_1() {
        let doc = Document::new("t");
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].name, "Layer 1");
        assert!(doc.layers[0].visible);
        assert_eq!(doc.layers[0].id.len(), 26, "layer ids are ULIDs");
    }

    #[test]
    fn layer_kind_defaults_to_raster_and_stays_off_disk() {
        // Every board written before kinds existed is a stack that
        // accumulates, which is what raster means: it opens unchanged.
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": []
        }"##;
        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.layers[0].kind, Kind::Raster);
        assert_eq!(Document::new("t").layers[0].kind, Kind::Raster);
        let v: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        assert!(v["layers"][0].get("kind").is_none(), "{v}");
    }

    #[test]
    fn a_vector_layer_says_so_on_disk_and_comes_back() {
        let mut doc = sample_doc();
        doc.layers[0].kind = Kind::Vector;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["layers"][0]["kind"], "vector");
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn an_unknown_layer_kind_is_an_error() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1", "kind": "bitmap" } ],
            "elements": []
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("bitmap"), "{err}");
    }

    #[test]
    fn add_layer_makes_the_kind_it_is_asked_for() {
        let mut doc = Document::new("t");
        assert_eq!(doc.add_layer(0, Kind::Vector), 1);
        assert_eq!(doc.layers[1].kind, Kind::Vector);
        assert_eq!(doc.add_layer(1, Kind::Raster), 2);
        assert_eq!(doc.layers[2].kind, Kind::Raster);
    }

    #[test]
    fn a_board_without_layers_gets_one_and_its_elements_join_it() {
        // Every board written before layers existed looks like the §6.1
        // example: no `layers`, no `layer` on the elements.
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "auth flow",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [
                { "id": "el_01", "type": "rect", "x": 40, "y": 80, "w": 220, "h": 80,
                  "stroke": "#222", "fill": null, "text": "API Gateway" }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].name, "Layer 1");
        assert!(doc.layers[0].visible);
        assert_eq!(doc.elements[0].layer(), doc.layers[0].id);
        // And it is written back with both, so the next reader need not guess.
        let v: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        assert_eq!(v["layers"][0]["id"], doc.layers[0].id.as_str());
        assert_eq!(v["elements"][0]["layer"], doc.layers[0].id.as_str());
    }

    #[test]
    fn layers_and_element_layers_roundtrip() {
        let mut doc = sample_doc();
        doc.layers.push(Layer {
            id: "L2".into(),
            name: "Layer 2".into(),
            visible: false,
            kind: Kind::Raster,
        });
        doc.elements[1].set_layer("L2");
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["layers"][0].get("visible").is_none(), "visible: absent when true");
        assert_eq!(v["layers"][1]["visible"], false);
        assert_eq!(v["layers"][1]["name"], "Layer 2");
        assert_eq!(v["elements"][1]["layer"], "L2");
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn an_element_naming_an_unknown_layer_is_an_error() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "L1", "name": "Layer 1" } ],
            "elements": [ { "id": "el_01", "type": "rect", "layer": "nope",
                "x": 0, "y": 0, "w": 1, "h": 1, "stroke": null, "fill": null, "text": null } ]
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("layer") && err.contains("nope"), "{err}");
    }

    #[test]
    fn duplicate_layer_ids_are_an_error() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "L1", "name": "a" }, { "id": "L1", "name": "b" } ],
            "elements": []
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("L1"), "{err}");
    }

    #[test]
    fn path_opacity_and_hardness_default_to_one_and_stay_off_disk() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [ { "id": "p1", "type": "path",
                "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 3 } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        assert_eq!((p.opacity, p.hardness), (1.0, 1.0));
        let v: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        assert!(v["elements"][0].get("opacity").is_none(), "{v}");
        assert!(v["elements"][0].get("hardness").is_none(), "{v}");
    }

    #[test]
    fn path_opacity_and_hardness_roundtrip() {
        let mut doc = sample_doc();
        let Element::Path(p) = &mut doc.elements[1] else {
            panic!("expected a path");
        };
        p.opacity = 0.5;
        p.hardness = 0.25;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][1]["opacity"].as_f64(), Some(0.5));
        assert_eq!(v["elements"][1]["hardness"].as_f64(), Some(0.25));
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn the_profiles_bend_the_ramp_in_the_order_the_sets_put_them_in() {
        // Sketchbook ships an Edge to go with every profile, and the two
        // move together: the sharper the profile, the crisper the Edge
        // it comes with. The falloff follows the same order — below one
        // is a fuller nib, above one a fainter.
        let order: Vec<f32> = [
            Profile::Sharp,
            Profile::HardSolid,
            Profile::RegularSolid,
            Profile::Airbrush,
        ]
        .map(Profile::falloff)
        .to_vec();
        for pair in order.windows(2) {
            assert!(pair[0] < pair[1], "{order:?} is not in order");
        }
        assert_eq!(
            Profile::RegularSolid.falloff(),
            1.0,
            "the default is the plain ramp, and bends nothing"
        );
    }

    #[test]
    fn a_nib_stamps_a_shape_or_wears_a_grain_but_never_both() {
        let board = |nib: &str| {
            format!(
                r##"{{
                    "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                    "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                    "layers": [ {{ "id": "l1", "name": "Layer 1" }} ],
                    "elements": [ {{ "id": "p1", "type": "path", "layer": "l1",
                        "curves": [[[0, 0], [1, 0], [2, 0], [3, 0]]],
                        "stroke": "#000", "width": 8,
                        "stamp": {{ "spacing": 1, "roundness": 1, "rotation": 0, {nib} }} }} ]
                }}"##
            )
        };
        let doc = Document::from_json(&board(r#""grain": "fine grain""#)).unwrap();
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        let nib = p.stamp.as_ref().unwrap();
        assert_eq!(nib.grain.as_deref(), Some("fine grain"));
        assert_eq!(nib.shape, None);
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][0]["stamp"]["grain"].as_str(), Some("fine grain"));
        assert!(v["elements"][0]["stamp"].get("shape").is_none());
        assert_eq!(Document::from_json(&json).unwrap(), doc);

        for (nib, want) in [
            (r#""shape": "a", "grain": "b""#, "never both"),
            (r#""grain": """#, "named or absent"),
        ] {
            let err = Document::from_json(&board(nib)).unwrap_err().to_string();
            assert!(err.contains(want), "{err}");
        }
    }

    #[test]
    fn a_stroke_keeps_the_profile_of_the_nib_that_laid_it() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": [ { "id": "pt1", "type": "paint", "layer": "l1", "strokes": [
                { "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 8,
                  "stamp": { "spacing": 1, "roundness": 1, "rotation": 0,
                             "profile": "airbrush" } },
                { "curves": [[[0, 9], [3, 9], [7, 9], [10, 9]]], "stroke": "#000", "width": 3,
                  "stamp": { "spacing": 1, "roundness": 1, "rotation": 0 } }
            ] } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Paint(p) = &doc.elements[0] else {
            panic!("expected a paint");
        };
        let profile = |i: usize| p.strokes[i].stamp.as_ref().unwrap().profile;
        assert_eq!(profile(0), Profile::Airbrush);
        assert_eq!(
            profile(1),
            Profile::RegularSolid,
            "a board that never named one means the plain ramp"
        );
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["elements"][0]["strokes"][0]["stamp"]["profile"].as_str(),
            Some("airbrush")
        );
        assert!(
            v["elements"][0]["strokes"][1]["stamp"].get("profile").is_none(),
            "and keeps saying nothing: {v}"
        );
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn a_reading_is_asked_for_by_how_far_along_the_stroke_it_is() {
        let e = Envelope {
            pressure: vec![0.0, 1.0, 0.0],
            twist: Vec::new(),
        };
        assert_eq!(e.pressure_at(0.0), 0.0);
        assert_eq!(e.pressure_at(0.5), 1.0, "the middle station");
        assert_eq!(e.pressure_at(1.0), 0.0);
        assert_eq!(e.pressure_at(0.25), 0.5, "halfway between two of them");
        // Past either end is the end: a walk that overruns by a rounding
        // error still reads a pressure and not a nonsense.
        assert_eq!(e.pressure_at(-1.0), 0.0);
        assert_eq!(e.pressure_at(2.0), 0.0);
    }

    #[test]
    fn a_stroke_with_no_readings_was_pressed_all_the_way() {
        let none = Envelope::default();
        assert!(none.is_empty());
        assert_eq!(none.pressure_at(0.5), 1.0, "what a mouse says");
        assert_eq!(none.twist_at(0.5), 0.0, "and it turns nothing");
        // One reading is that reading the whole way.
        let flat = Envelope {
            pressure: vec![0.4],
            twist: Vec::new(),
        };
        assert_eq!((flat.pressure_at(0.0), flat.pressure_at(1.0)), (0.4, 0.4));
    }

    #[test]
    fn a_light_touch_is_worth_what_the_amount_says() {
        // Nothing driven: the value stands whatever the hand does.
        assert_eq!(Pressure::scale(0.0, 0.0), 1.0);
        // Driven the whole way: no press is nothing at all.
        assert_eq!(Pressure::scale(1.0, 0.0), 0.0);
        assert_eq!(Pressure::scale(1.0, 1.0), 1.0);
        // Half driven: a light touch keeps the other half.
        assert_eq!(Pressure::scale(0.5, 0.0), 0.5);
        assert_eq!(Pressure::scale(0.5, 0.5), 0.75);
    }

    #[test]
    fn a_stroke_keeps_what_the_pen_said_along_it() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": [ { "id": "pt1", "type": "paint", "layer": "l1", "strokes": [
                { "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 8,
                  "stamp": { "spacing": 0.4, "roundness": 1, "rotation": 0,
                             "pressure": { "size": 0.5, "flow": 0.25 } },
                  "pen": { "pressure": [0.2, 1, 0.2], "twist": [0, 45] } },
                { "curves": [[[0, 9], [3, 9], [7, 9], [10, 9]]], "stroke": "#000", "width": 3 }
            ] } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Paint(p) = &doc.elements[0] else {
            panic!("expected a paint");
        };
        assert_eq!(
            p.strokes[0].stamp.as_ref().unwrap().pressure,
            Pressure {
                size: 0.5,
                opacity: 0.0,
                flow: 0.25,
            },
            "how much of the nib the hand drives"
        );
        assert_eq!(p.strokes[0].pen.pressure, vec![0.2, 1.0, 0.2]);
        assert_eq!(p.strokes[0].pen.twist, vec![0.0, 45.0]);
        assert!(p.strokes[1].pen.is_empty(), "a stroke drawn with no pen");

        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][0]["strokes"][0]["pen"]["pressure"][1].as_f64(), Some(1.0));
        assert!(
            v["elements"][0]["strokes"][1].get("pen").is_none(),
            "a board no pen touched keeps saying nothing: {v}"
        );
        assert!(
            v["elements"][0]["strokes"][1]["stamp"].get("pressure").is_none(),
            "and neither does a nib the pen cannot lean on"
        );
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn a_board_cannot_say_the_pen_pressed_harder_than_it_can() {
        for (pen, want) in [
            (r#""pen": { "pressure": [0.5, 1.5] }"#, "not between 0 and 1"),
            (r#""pen": { "twist": [0, 1e39] }"#, "not an angle"),
            (
                r#""stamp": { "spacing": 1, "roundness": 1, "rotation": 0,
                             "pressure": { "size": 2 } }"#,
                "not between 0 and 1",
            ),
        ] {
            let json = format!(
                r##"{{
                    "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                    "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                    "layers": [ {{ "id": "l1", "name": "Layer 1" }} ],
                    "elements": [ {{ "id": "p1", "type": "path", "layer": "l1",
                        "curves": [[[0, 0], [1, 0], [2, 0], [3, 0]]],
                        "stroke": "#000", "width": 8, {pen} }} ]
                }}"##
            );
            let err = Document::from_json(&json).unwrap_err().to_string();
            assert!(err.contains(want), "{err}");
        }
    }

    #[test]
    fn a_stroke_keeps_the_nib_it_was_stamped_with() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": [ { "id": "pt1", "type": "paint", "layer": "l1", "strokes": [
                { "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 8,
                  "stamp": { "spacing": 0.4, "roundness": 0.5, "rotation": 30 } },
                { "curves": [[[0, 9], [3, 9], [7, 9], [10, 9]]], "stroke": "#000", "width": 3 }
            ] } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Paint(p) = &doc.elements[0] else {
            panic!("expected a paint");
        };
        assert_eq!(
            p.strokes[0].stamp,
            Some(Stamp {
                shape: None,
                grain: None,
                paper: None,
                follow: false,
                spacing: 0.4,
                roundness: 0.5,
                rotation: 30.0,
                profile: Profile::RegularSolid,
                mark: Mark::Ink,
                flow: 1.0,
                scatter: Scatter::default(),
                pressure: Pressure::NONE,
            }),
            "a brush stroke says which nib laid it"
        );
        assert_eq!(p.strokes[1].stamp, None, "a pencil stroke sweeps");

        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][0]["strokes"][0]["stamp"]["spacing"].as_f64(), Some(0.4));
        assert!(
            v["elements"][0]["strokes"][1].get("stamp").is_none(),
            "a board with no nib in it keeps saying nothing: {v}"
        );
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn a_nib_the_canvas_could_not_stamp_is_refused() {
        for (nib, want) in [
            (r#"{ "spacing": 0.4, "roundness": 2, "rotation": 0 }"#, "roundness"),
            (r#"{ "spacing": 0, "roundness": 1, "rotation": 0 }"#, "spacing"),
            (r#"{ "spacing": 0.4, "roundness": 1, "rotation": 400 }"#, "rotation"),
            (r#"{ "spacing": 0.4, "roundness": 1, "rotation": 0, "flow": -1 }"#, "flow"),
            (
                r#"{ "spacing": 0.4, "roundness": 1, "rotation": 0,
                     "scatter": { "rotation": -5 } }"#,
                "rotation scatter",
            ),
            (
                r#"{ "spacing": 0.4, "roundness": 1, "rotation": 0,
                     "paper": { "name": "", "period": 512, "depth": 1 } }"#,
                "paper is named",
            ),
            (
                r#"{ "spacing": 0.4, "roundness": 1, "rotation": 0,
                     "paper": { "name": "canvas", "period": 0, "depth": 1 } }"#,
                "period",
            ),
            (
                r#"{ "spacing": 0.4, "roundness": 1, "rotation": 0,
                     "paper": { "name": "canvas", "period": 512, "depth": 2 } }"#,
                "depth",
            ),
        ] {
            let json = format!(
                r##"{{
                    "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                    "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                    "elements": [ {{ "id": "p1", "type": "path",
                        "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]],
                        "stroke": "#000", "width": 3, "stamp": {nib} }} ]
                }}"##
            );
            let err = Document::from_json(&json).unwrap_err().to_string();
            assert!(err.contains(want), "{nib} → {err}");
        }
    }

    #[test]
    fn a_stroke_keeps_the_paper_it_was_dragged_over() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [ { "id": "p1", "type": "path",
                "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]],
                "stroke": "#000", "width": 8,
                "stamp": { "spacing": 1, "roundness": 1, "rotation": 0, "grain": "tooth", "paper": { "name": "canvas", "period": 512, "depth": 0.8 } } } ]
        }"##;
        let doc = Document::from_json(json).expect("a papered stroke opens");
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path");
        };
        let stamp = p.stamp.as_ref().expect("the stroke was stamped");
        // A nib's own grain and the canvas's paper are not alternatives:
        // the grain turns with the dab, the paper stands still under it.
        assert_eq!(stamp.grain.as_deref(), Some("tooth"));
        assert_eq!(
            stamp.paper,
            Some(Paper {
                name: "canvas".to_owned(),
                period: 512.0,
                depth: 0.8,
            })
        );
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(back.elements, doc.elements, "and it survives the round trip");
        // A stroke that was dragged over nothing says nothing on disk.
        let plain = json.replace(
            r#", "paper": { "name": "canvas", "period": 512, "depth": 0.8 }"#,
            "",
        );
        let bare = Document::from_json(&plain).unwrap().to_json().unwrap();
        assert!(!bare.contains("paper"), "{bare}");
    }

    #[test]
    fn a_paint_is_one_element_holding_every_stroke_laid_on_it() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": [ { "id": "pt1", "type": "paint", "layer": "l1", "strokes": [
                { "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 3 },
                { "curves": [[[0, 9], [3, 9], [7, 9], [10, 9]]], "stroke": "#f00", "width": 8,
                  "opacity": 0.5, "hardness": 0.25 }
            ] } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Paint(p) = &doc.elements[0] else {
            panic!("expected a paint");
        };
        assert_eq!(p.strokes.len(), 2, "one object, two strokes");
        assert_eq!(p.layer, "l1");
        // Every stroke keeps the ink it was laid with.
        assert_eq!((p.strokes[0].width, p.strokes[0].opacity), (3.0, 1.0));
        assert_eq!((p.strokes[1].width, p.strokes[1].opacity), (8.0, 0.5));
        assert_eq!(p.strokes[1].stroke, "#f00");
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][0]["type"], "paint");
        assert!(
            v["elements"][0]["strokes"][0].get("opacity").is_none(),
            "a full stroke stays off disk: {v}"
        );
        assert_eq!(v["elements"][0]["strokes"][1]["hardness"], 0.25);
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn a_paint_stroke_outside_the_unit_range_is_an_error() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [ { "id": "pt1", "type": "paint", "strokes": [
                { "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000",
                  "width": 3, "opacity": 1.5 }
            ] } ]
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("opacity"), "{err}");
    }

    #[test]
    fn path_opacity_or_hardness_outside_the_unit_range_is_an_error() {
        for (field, value) in [
            ("opacity", "1.5"),
            ("opacity", "-0.1"),
            ("hardness", "2"),
            ("hardness", "-1"),
        ] {
            let json = format!(
                r##"{{
                "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                "elements": [ {{ "id": "p1", "type": "path", "{field}": {value},
                    "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]], "stroke": "#000", "width": 3 }} ]
            }}"##
            );
            let err = Document::from_json(&json).unwrap_err().to_string();
            assert!(err.contains(field), "{field} = {value}: {err}");
        }
    }

    #[test]
    fn painted_walks_layers_bottom_up_and_skips_hidden_ones() {
        let mut doc = three_layers();
        assert_eq!(
            painted_ids(&doc),
            vec![(1, "on_bottom"), (2, "on_middle"), (0, "on_top")]
        );
        doc.layers[1].visible = false;
        assert_eq!(painted_ids(&doc), vec![(1, "on_bottom"), (0, "on_top")]);
    }

    #[test]
    fn layer_index_finds_a_layer_by_id() {
        let doc = three_layers();
        assert_eq!(doc.layer_index("middle"), Some(1));
        assert_eq!(doc.layer_index("nope"), None);
    }

    #[test]
    fn add_layer_inserts_above_and_names_past_the_highest_number() {
        let mut doc = Document::new("t");
        assert_eq!(doc.add_layer(0, Kind::Raster), 1);
        assert_eq!(doc.layers[1].name, "Layer 2");
        assert!(doc.remove_layer(1));
        // "Layer 2" is gone, but its number is not reused: numbering only
        // ever counts up, as in Photoshop.
        assert_eq!(doc.add_layer(0, Kind::Raster), 1);
        assert_eq!(doc.layers[1].name, "Layer 2");
        doc.layers[1].name = "Layer 7".into();
        assert_eq!(doc.add_layer(0, Kind::Raster), 1);
        assert_eq!(doc.layers[1].name, "Layer 8");
        assert_eq!(doc.layers[2].name, "Layer 7", "the new layer went in above index 0");
        assert!(doc.layers[1].visible);
        assert_eq!(doc.layers[1].id.len(), 26);
        // Past the end still lands on top.
        assert_eq!(doc.add_layer(99, Kind::Raster), 3);
    }

    #[test]
    fn remove_layer_drops_its_elements_and_refuses_the_last() {
        let mut doc = three_layers();
        assert!(doc.remove_layer(1));
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(painted_ids(&doc), vec![(1, "on_bottom"), (0, "on_top")]);
        assert_eq!(doc.elements.len(), 2, "the middle layer's element went with it");
        assert!(!doc.remove_layer(5), "no such layer");
        assert!(doc.remove_layer(0));
        assert!(!doc.remove_layer(0), "a board keeps its last layer");
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.elements.len(), 1);
    }

    #[test]
    fn move_layer_swaps_with_the_neighbour_and_stops_at_the_edge() {
        let mut doc = three_layers();
        assert_eq!(doc.move_layer(0, true), Some(1));
        assert_eq!(doc.layers[1].id, "bottom");
        assert_eq!(doc.layers[0].id, "middle");
        assert_eq!(doc.move_layer(2, true), None, "already on top");
        assert_eq!(doc.move_layer(0, false), None, "already at the bottom");
        assert_eq!(doc.move_layer(9, true), None, "no such layer");
        assert_eq!(doc.move_layer(1, false), Some(0));
        assert_eq!(doc.layers[0].id, "bottom");
    }

    #[test]
    fn reorder_layer_lifts_one_out_and_drops_it_in() {
        let mut doc = three_layers();
        assert!(doc.reorder_layer(2, 0), "the top one to the bottom");
        let ids: Vec<&str> = doc.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, vec!["top", "bottom", "middle"], "the others keep their order");
        assert!(doc.reorder_layer(0, 2));
        let ids: Vec<&str> = doc.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, vec!["bottom", "middle", "top"]);
        assert!(!doc.reorder_layer(1, 1), "nowhere to go");
        assert!(!doc.reorder_layer(3, 0), "no such layer");
        assert!(!doc.reorder_layer(0, 3), "no such place");
    }

    #[test]
    fn new_document_is_empty_with_ulid_and_default_camera() {
        let doc = Document::new("meu board");
        assert_eq!(doc.schema, SCHEMA_VERSION);
        assert_eq!(doc.title, "meu board");
        assert_eq!(doc.id.len(), 26, "id deve ser ULID (26 chars)");
        assert!(doc.elements.is_empty());
        assert_eq!(
            doc.camera,
            Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0
            }
        );
    }

    #[test]
    fn new_ids_are_unique() {
        assert_ne!(new_id(), new_id());
    }

    #[test]
    fn element_serializes_flat_with_lowercase_type_tag() {
        let doc = sample_doc();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        let el = &v["elements"][0];
        // §6.1 format: flattened fields + "type": "rect", not {"Rect": {...}}.
        assert_eq!(el["type"], "rect");
        assert_eq!(el["id"], "el_01");
        assert_eq!(el["x"].as_f64(), Some(40.0));
        assert_eq!(el["text"], "API Gateway");
        // fill: null shows up explicitly, as in the spec example.
        assert!(el.get("fill").is_some());
        assert!(el["fill"].is_null());
    }

    #[test]
    fn parses_the_spec_example_verbatim() {
        // The §6.1 example from ARCHITECTURE.md, with raw integers in the JSON.
        let json = r##"{
            "schema": 1,
            "id": "01JXXXXXXXXXXXXXXXXXXXXXXX",
            "title": "auth flow",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [
                {
                    "id": "el_01",
                    "type": "rect",
                    "x": 40, "y": 80, "w": 220, "h": 80,
                    "stroke": "#222", "fill": null,
                    "text": "API Gateway"
                }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.title, "auth flow");
        assert_eq!(doc.elements.len(), 1);
        let Element::Rect(r) = &doc.elements[0] else {
            panic!("expected a rect, got {:?}", doc.elements[0]);
        };
        assert_eq!((r.x, r.y, r.w, r.h), (40.0, 80.0, 220.0, 80.0));
        assert_eq!(r.stroke.as_deref(), Some("#222"));
        assert_eq!(r.fill, None);
    }

    #[test]
    fn path_serializes_curves_as_self_contained_cubics() {
        let doc = sample_doc();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        let el = &v["elements"][1];
        // Pen strokes are `type: "path"` with one `[a, c1, c2, b]` per cubic
        // — SVG's `M a C c1 c2 b`, with nothing to cross-check between arrays.
        assert_eq!(el["type"], "path");
        assert_eq!(el["id"], "el_02");
        assert_eq!(
            el["curves"],
            serde_json::json!([[[1.0, 2.0], [2.0, 3.0], [3.5, 4.0], [6.0, 4.0]]])
        );
        assert_eq!(el["stroke"], "#1f1f1f");
        assert_eq!(el["width"].as_f64(), Some(2.0));
    }

    #[test]
    fn parses_path_element_with_integer_coordinates() {
        let json = r##"{
            "schema": 1,
            "id": "01JXXXXXXXXXXXXXXXXXXXXXXX",
            "title": "sketch",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [
                { "id": "p1", "type": "path",
                  "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]],
                  "stroke": "#000", "width": 3 }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path, got {:?}", doc.elements[0]);
        };
        assert_eq!(
            p.curves,
            vec![[[0.0, 0.0], [3.0, 0.0], [7.0, 5.0], [10.0, 5.0]]]
        );
        assert_eq!(p.stroke, "#000");
        assert_eq!(p.width, 3.0);
    }

    #[test]
    fn parses_legacy_path_points_by_fitting_them() {
        // Boards written before the Bézier fit landed hold raw polylines.
        let json = r##"{
            "schema": 1,
            "id": "01JXXXXXXXXXXXXXXXXXXXXXXX",
            "title": "sketch",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [
                { "id": "p1", "type": "path", "points": [[0, 0], [4.5, 0.2], [9, 0]],
                  "stroke": "#000", "width": 2 }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Path(p) = &doc.elements[0] else {
            panic!("expected a path, got {:?}", doc.elements[0]);
        };
        assert_eq!(
            p.curves,
            vec![[[0.0, 0.0], [3.0, 0.0], [6.0, 0.0], [9.0, 0.0]]]
        );
        assert_eq!(p.stroke, "#000");
    }

    #[test]
    fn path_without_curves_or_points_is_an_error() {
        let json = r##"{
            "schema": 1,
            "id": "01JXXXXXXXXXXXXXXXXXXXXXXX",
            "title": "sketch",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [ { "id": "p1", "type": "path", "stroke": "#000", "width": 2 } ]
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("curves"), "{err}");
    }

    #[test]
    fn rect_rotation_defaults_to_zero_and_stays_off_disk() {
        // The §6.1 example has no `rotation`: unrotated, and written back
        // without the field so unrotated boards keep their shape on disk.
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [ { "id": "el_01", "type": "rect",
                "x": 40, "y": 80, "w": 220, "h": 80,
                "stroke": "#222", "fill": null, "text": null } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        let Element::Rect(r) = &doc.elements[0] else {
            panic!("expected a rect");
        };
        assert_eq!(r.rotation, 0.0);
        let v: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        assert!(v["elements"][0].get("rotation").is_none(), "{v}");
    }

    #[test]
    fn rect_rotation_roundtrips_in_degrees() {
        let mut doc = sample_doc();
        let Element::Rect(r) = &mut doc.elements[0] else {
            panic!("expected a rect");
        };
        r.rotation = 30.0;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][0]["rotation"].as_f64(), Some(30.0));
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn path_rotation_defaults_to_zero_and_stays_off_disk() {
        // Paths written before rotation existed, and legacy polylines, are
        // in their creation state: zero, and nothing new on disk.
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "elements": [
                { "id": "p1", "type": "path",
                  "curves": [[[0, 0], [3, 0], [7, 5], [10, 5]]],
                  "stroke": "#000", "width": 3 },
                { "id": "p2", "type": "path", "points": [[0, 0], [4.5, 0.2], [9, 0]],
                  "stroke": "#000", "width": 2 }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        for el in &doc.elements {
            let Element::Path(p) = el else {
                panic!("expected a path")
            };
            assert_eq!(p.rotation, 0.0, "{}", p.id);
        }
        let v: serde_json::Value = serde_json::from_str(&doc.to_json().unwrap()).unwrap();
        assert!(v["elements"][0].get("rotation").is_none(), "{v}");
    }

    #[test]
    fn path_rotation_roundtrips_in_degrees() {
        let mut doc = sample_doc();
        let Element::Path(p) = &mut doc.elements[1] else {
            panic!("expected a path");
        };
        p.rotation = 45.0;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][1]["rotation"].as_f64(), Some(45.0));
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let json = r#"{ "schema": 2, "id": "x", "title": "t", "camera": {"x":0,"y":0,"zoom":1}, "elements": [] }"#;
        let err = Document::from_json(json).unwrap_err();
        assert!(
            err.to_string().contains("schema"),
            "erro deve citar o schema: {err}"
        );
    }

    #[test]
    fn json_roundtrip_preserves_document() {
        let doc = sample_doc();
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn image_serializes_with_its_blob_hash() {
        let doc = sample_doc();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        let el = &v["elements"][2];
        // An image points at the blob store by hash — never at a path, so a
        // board carries nothing about where it was written (§6.1, §9.3).
        assert_eq!(el["type"], "image");
        assert_eq!(el["id"], "el_03");
        assert_eq!(el["blob"], BLOB);
        assert_eq!(el["x"].as_f64(), Some(10.0));
        assert_eq!(el["w"].as_f64(), Some(64.0));
        assert!(el.get("rotation").is_none(), "{el}");
    }

    #[test]
    fn parses_image_element_with_integer_coordinates() {
        let json = format!(
            r##"{{
            "schema": 1,
            "id": "01JXXXXXXXXXXXXXXXXXXXXXXX",
            "title": "shot",
            "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
            "elements": [
                {{ "id": "i1", "type": "image", "x": 0, "y": 0, "w": 32, "h": 16,
                   "blob": "{BLOB}" }}
            ]
        }}"##
        );
        let doc = Document::from_json(&json).unwrap();
        let Element::Image(i) = &doc.elements[0] else {
            panic!("expected an image, got {:?}", doc.elements[0]);
        };
        assert_eq!((i.x, i.y, i.w, i.h), (0.0, 0.0, 32.0, 16.0));
        assert_eq!(i.blob, BLOB);
        assert_eq!(i.rotation, 0.0);
    }

    #[test]
    fn image_rotation_roundtrips_in_degrees() {
        let mut doc = sample_doc();
        let Element::Image(i) = &mut doc.elements[2] else {
            panic!("expected an image");
        };
        i.rotation = 15.0;
        let json = doc.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["elements"][2]["rotation"].as_f64(), Some(15.0));
        assert_eq!(Document::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn image_blob_that_is_not_a_hash_is_an_error() {
        // The blob names a file under blobs/; anything but a bare sha256
        // could walk out of the store (§9.3), so it never parses.
        for blob in [
            "../../../etc/passwd",
            "a/b",
            "",
            "ABCDEF",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85",
        ] {
            let json = format!(
                r##"{{
                "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                "elements": [ {{ "id": "i1", "type": "image",
                    "x": 0, "y": 0, "w": 1, "h": 1, "blob": "{blob}" }} ]
            }}"##
            );
            let err = Document::from_json(&json).unwrap_err().to_string();
            assert!(err.contains("blob"), "{blob:?} should be rejected: {err}");
        }
    }

    /// A frame carries its area, its background and a stack of its own,
    /// and comes back the same.
    #[test]
    fn a_frame_round_trips() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [
                { "id": "fr", "type": "frame", "layer": "fl",
                  "x": -100, "y": -50, "w": 200, "h": 100,
                  "background": "#fbfbfa",
                  "layers": [ { "id": "in", "name": "Layer 1" } ] }
            ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.layers[0].kind, Kind::Frame);
        let Element::Frame(f) = &doc.elements[0] else {
            panic!("not a frame");
        };
        assert_eq!((f.x, f.y, f.w, f.h), (-100.0, -50.0, 200.0, 100.0));
        assert_eq!(f.background.as_deref(), Some("#fbfbfa"));
        assert_eq!(f.layers.len(), 1);
        assert_eq!(f.layers[0].name, "Layer 1");
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(back, doc);
    }

    /// The area is the boundary, edges included.
    #[test]
    fn a_frame_holds_the_points_inside_its_area() {
        let f = Frame {
            id: "fr".into(),
            layer: "fl".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            background: None,
            layers: Vec::new(),
        };
        assert!(f.contains([5.0, 5.0]));
        assert!(f.contains([0.0, 0.0]));
        assert!(f.contains([10.0, 10.0]));
        assert!(!f.contains([10.1, 5.0]));
        assert!(!f.contains([5.0, -0.1]));
    }

    /// A board written before frames existed keeps its shape: no `kind`
    /// is still raster, and nothing it holds is a frame.
    #[test]
    fn a_board_without_frames_is_unchanged() {
        let doc = sample_doc();
        let back = Document::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(back, doc);
        assert!(!doc.to_json().unwrap().contains("\"kind\""));
    }

    /// A board with one frame: board layers `bottom` then the frame
    /// layer `fl`, the frame `fr` spanning (0,0)–(100,100) with one
    /// inner layer `in`, and one rect on each of `bottom` and `in`.
    fn framed() -> Document {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [
                { "id": "bottom", "name": "Layer 1" },
                { "id": "fl", "name": "Frame 1", "kind": "frame" }
            ],
            "elements": [
                { "id": "outside", "type": "rect", "layer": "bottom",
                  "x": 200, "y": 200, "w": 10, "h": 10, "stroke": null, "fill": null, "text": null },
                { "id": "fr", "type": "frame", "layer": "fl",
                  "x": 0, "y": 0, "w": 100, "h": 100, "background": "#fff",
                  "layers": [ { "id": "in", "name": "Layer 1" } ] },
                { "id": "inside", "type": "rect", "layer": "in",
                  "x": 10, "y": 10, "w": 10, "h": 10, "stroke": null, "fill": null, "text": null }
            ]
        }"##;
        Document::from_json(json).unwrap()
    }

    #[test]
    fn stack_answers_the_board_or_one_frame() {
        let doc = framed();
        assert_eq!(doc.stack(None).len(), 2, "the board's own");
        let ids: Vec<&str> = doc.stack(Some("fr")).iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["in"]);
        assert!(doc.stack(Some("nobody")).is_empty());
    }

    #[test]
    fn locate_says_which_stack_a_layer_is_on() {
        let doc = framed();
        assert_eq!(doc.locate("bottom"), Some((None, 0)));
        assert_eq!(doc.locate("fl"), Some((None, 1)));
        assert_eq!(doc.locate("in"), Some((Some("fr"), 0)));
        assert_eq!(doc.locate("nobody"), None);
    }

    #[test]
    fn a_frame_is_reached_by_its_id_and_by_its_layer() {
        let doc = framed();
        assert_eq!(doc.frame("fr").map(|f| f.w), Some(100.0));
        assert_eq!(doc.frame_on("fl").map(|f| f.id.as_str()), Some("fr"));
        assert!(doc.frame_on("bottom").is_none());
    }

    #[test]
    fn frame_at_names_the_topmost_frame_over_a_point_and_skips_hidden_ones() {
        let mut doc = framed();
        assert_eq!(doc.frame_at([50.0, 50.0]), Some("fr"));
        assert_eq!(doc.frame_at([500.0, 500.0]), None, "the open board");

        // A second frame over the same place, higher up, wins.
        doc.layers.push(Layer {
            id: "fl2".into(),
            name: "Frame 2".into(),
            visible: true,
            kind: Kind::Frame,
        });
        doc.elements.push(Element::Frame(Frame {
            id: "fr2".into(),
            layer: "fl2".into(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            background: None,
            layers: vec![layer("in2", "Layer 1")],
        }));
        assert_eq!(doc.frame_at([50.0, 50.0]), Some("fr2"));

        // A hidden frame claims nothing.
        doc.layers[2].visible = false;
        assert_eq!(doc.frame_at([50.0, 50.0]), Some("fr"));
    }

    /// A board holding a frame layer that no frame is on is not a
    /// document.
    #[test]
    fn a_frame_layer_needs_its_frame() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": []
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("fl"), "{err}");
    }

    /// And a frame sitting on a layer that is not one is not a document
    /// either: the two halves are one thing.
    #[test]
    fn a_frame_needs_a_frame_layer() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "l1", "name": "Layer 1" } ],
            "elements": [ { "id": "fr", "type": "frame", "layer": "l1",
                            "x": 0, "y": 0, "w": 10, "h": 10, "layers": [] } ]
        }"##;
        assert!(Document::from_json(json).is_err());
    }

    /// A frame does not nest.
    #[test]
    fn a_frame_layer_inside_a_frame_is_refused() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [ { "id": "fr", "type": "frame", "layer": "fl",
                            "x": 0, "y": 0, "w": 10, "h": 10,
                            "layers": [ { "id": "nested", "name": "F", "kind": "frame" } ] } ]
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("nested"), "{err}");
    }

    /// Layer ids are unique across the whole document, not per stack.
    #[test]
    fn a_layer_id_used_in_a_frame_and_on_the_board_is_refused() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "twice", "name": "Layer 1" },
                        { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [ { "id": "fr", "type": "frame", "layer": "fl",
                            "x": 0, "y": 0, "w": 10, "h": 10,
                            "layers": [ { "id": "twice", "name": "Layer 1" } ] } ]
        }"##;
        let err = Document::from_json(json).unwrap_err().to_string();
        assert!(err.contains("twice"), "{err}");
    }

    /// A frame's stack is never empty, exactly as the board's is not.
    #[test]
    fn an_empty_frame_stack_gets_layer_1() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [ { "id": "fr", "type": "frame", "layer": "fl",
                            "x": 0, "y": 0, "w": 10, "h": 10, "layers": [] } ]
        }"##;
        let doc = Document::from_json(json).unwrap();
        assert_eq!(doc.stack(Some("fr")).len(), 1);
        assert_eq!(doc.stack(Some("fr"))[0].name, "Layer 1");
    }

    /// An area of no size is not an area.
    #[test]
    fn a_frame_with_no_size_is_refused() {
        let json = r##"{
            "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
            "camera": { "x": 0, "y": 0, "zoom": 1 },
            "layers": [ { "id": "fl", "name": "Frame 1", "kind": "frame" } ],
            "elements": [ { "id": "fr", "type": "frame", "layer": "fl",
                            "x": 0, "y": 0, "w": 0, "h": 10, "layers": [] } ]
        }"##;
        assert!(Document::from_json(json).is_err());
    }

    /// An element on an inner layer is not an orphan.
    #[test]
    fn an_element_on_a_frames_layer_parses() {
        assert_eq!(framed().elements.len(), 3);
    }
}
