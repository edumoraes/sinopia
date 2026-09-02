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
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

/// Scene elements (§6.1). Types enter as their tools exist: `rect` from the
/// scaffold, `path` with the pencil.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Element {
    Rect(Rect),
    Path(Path),
}

impl Element {
    pub fn id(&self) -> &str {
        match self {
            Element::Rect(r) => &r.id,
            Element::Path(p) => &p.id,
        }
    }
}

/// `x, y, w, h` is the box before rotation; `rotation` turns it about its
/// center, in degrees, clockwise on screen (y down, as in SVG). Absent on
/// disk when zero, so unrotated boards keep the §6.1 shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub id: String,
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

/// Freehand pen stroke: a chain of cubic Béziers in world units, each one
/// self-contained as `[a, c1, c2, b]` and starting where the previous
/// ended. `width` is in world units too (ink scales with zoom).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PathOnDisk")]
pub struct Path {
    pub id: String,
    pub curves: Vec<Cubic>,
    pub stroke: String,
    pub width: f64,
}

/// What a `path` may look like on disk: `curves` today, or the raw
/// `points` polyline boards held before the Bézier fit landed. Legacy
/// polylines are fitted on load and written back as `curves` on the next
/// save.
#[derive(Deserialize)]
struct PathOnDisk {
    id: String,
    curves: Option<Vec<Cubic>>,
    points: Option<Vec<[f64; 2]>>,
    stroke: String,
    width: f64,
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
        Ok(Path {
            id: p.id,
            curves,
            stroke: p.stroke,
            width: p.width,
        })
    }
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
    /// New, empty document with a ULID id and the camera at the origin.
    pub fn new(title: &str) -> Self {
        Document {
            schema: SCHEMA_VERSION,
            id: new_id(),
            title: title.to_owned(),
            camera: Camera::default(),
            elements: Vec::new(),
        }
    }

    /// Deserializes and validates the schema version.
    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        let doc: Document = serde_json::from_str(s)?;
        if doc.schema != SCHEMA_VERSION {
            anyhow::bail!(
                "schema {} not supported (this binary speaks schema {})",
                doc.schema,
                SCHEMA_VERSION
            );
        }
        Ok(doc)
    }

    /// Serializes to the canonical on-disk JSON (pretty: debugging with
    /// $EDITOR is a stated goal of the local phase).
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// New id in ULID format (stable, time-sortable — the bridge to a CRDT).
pub fn new_id() -> String {
    ulid::Ulid::from_datetime(std::time::SystemTime::now()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            elements: vec![
                Element::Rect(Rect {
                    id: "el_01".into(),
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
                    curves: vec![[[1.0, 2.0], [2.0, 3.0], [3.5, 4.0], [6.0, 4.0]]],
                    stroke: "#1f1f1f".into(),
                    width: 2.0,
                }),
            ],
        }
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
}
