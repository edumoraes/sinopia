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
}

impl Element {
    pub fn id(&self) -> &str {
        match self {
            Element::Rect(r) => &r.id,
            Element::Path(p) => &p.id,
            Element::Paint(p) => &p.id,
            Element::Image(i) => &i.id,
        }
    }

    /// The id of the layer this element is on.
    pub fn layer(&self) -> &str {
        match self {
            Element::Rect(r) => &r.layer,
            Element::Path(p) => &p.layer,
            Element::Paint(p) => &p.layer,
            Element::Image(i) => &i.layer,
        }
    }

    pub fn set_layer(&mut self, id: &str) {
        let layer = match self {
            Element::Rect(r) => &mut r.layer,
            Element::Path(p) => &mut p.layer,
            Element::Paint(p) => &mut p.layer,
            Element::Image(i) => &mut i.layer,
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

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// How far each dab is thrown off the nib, in the unit of the thing it
/// throws: `size` a radius in world units, `rotation` degrees,
/// `spacing` tip widths. Sketchbook's Randomness, less the two whose
/// amount is not in their property's own unit — see `brush::Jitter`.
/// All zero is a nib laid true, which is what a board without one says.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Scatter {
    pub size: f64,
    pub rotation: f64,
    pub spacing: f64,
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
        for (what, amount) in [
            ("size", self.size),
            ("rotation", self.rotation),
            ("spacing", self.spacing),
        ] {
            if !(amount.is_finite() && amount >= 0.0) {
                return Err(format!("{what} scatter {amount} is not an amount"));
            }
        }
        Ok(self)
    }
}

/// The nib a stroke was stamped with: what one dab is, beyond the
/// width, opacity and hardness the stroke already names. A stroke
/// carrying none was swept, not stamped — the pencil's.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    /// The gap between two dabs, in tip widths.
    pub spacing: f64,
    /// 1 is a round nib; less flattens it across its own y.
    pub roundness: f64,
    /// The nib's own angle, in degrees, clockwise.
    pub rotation: f64,
    /// What one dab lays, 0–1. The stroke builds toward its opacity
    /// where the dabs cross; 1 is a dab that covers on its own.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub flow: f64,
    /// How far each dab is thrown off it.
    #[serde(default, skip_serializing_if = "Scatter::is_true")]
    pub scatter: Scatter,
}

impl Stamp {
    /// The same nib if the canvas could stamp it, or why it could not.
    /// A board is not trusted to hold a turn of four hundred degrees or
    /// a gap of nothing, whatever wrote it.
    fn checked(self) -> Result<Stamp, String> {
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
        Ok(self)
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

    fn settle_layers(&mut self) -> anyhow::Result<()> {
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Layer 1"));
        }
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.id.is_empty() {
                anyhow::bail!("layer {i} has no id");
            }
            if self.layers[..i].iter().any(|l| l.id == layer.id) {
                anyhow::bail!("layer id {:?} is used twice", layer.id);
            }
        }
        let layers = &self.layers;
        for el in &mut self.elements {
            if el.layer().is_empty() {
                el.set_layer(&layers[0].id);
            } else if !layers.iter().any(|l| l.id == el.layer()) {
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
                spacing: 0.4,
                roundness: 0.5,
                rotation: 30.0,
                flow: 1.0,
                scatter: Scatter::default(),
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
}
