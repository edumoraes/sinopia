//! What leaves the board for an agent, and what is written when it does
//! (ARCHITECTURE.md §8). The scope answers three things — the box it
//! covers, what is inside it, and whether it has a name — and all three
//! are answered here, where they can be tested.

use crate::doc::{Document, Element, Kind, Layer};
use crate::geom::Frame;
use crate::select;

/// What is being exported.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// A frame, by the id of the `Element::Frame` itself. It brings a
    /// name and owns its folder.
    Frame(String),
    /// Loose objects, by element id. It has no name, so the panel asks.
    Selection(Vec<String>),
}

impl Scope {
    /// The ids the scope names on the board — a frame is its own id.
    fn ids(&self) -> Vec<String> {
        match self {
            Scope::Frame(id) => vec![id.clone()],
            Scope::Selection(ids) => ids.clone(),
        }
    }
}

/// The world box the scope covers: a frame's own boundary, or the box
/// around everything a selection names. A frame's box is the frame's and
/// not its contents' — the boundary is what cuts the ink, so it is what
/// the picture is of.
pub fn bounds(doc: &Document, scope: &Scope) -> Option<Frame> {
    select::frame_of(doc, &scope.ids())
}

/// The name the scope exports under, when it has one. A frame's name is
/// its layer's, since a frame layer and its frame are one thing. A loose
/// selection has none and the panel asks for one.
pub fn named(doc: &Document, scope: &Scope) -> Option<String> {
    let Scope::Frame(id) = scope else {
        return None;
    };
    let frame = doc.frame(id)?;
    doc.layers
        .iter()
        .find(|l| l.id == frame.layer)
        .map(|l| l.name.clone())
}

/// A document holding only what the scope covers, in paint order, with
/// the layers those elements name and nothing else. It is a document in
/// the schema's own terms, so it parses back (§8).
pub fn sub_document(doc: &Document, scope: &Scope) -> Document {
    let wanted = scope.ids();
    let mut out = Document::new(&doc.title);
    out.id = doc.id.clone();
    let mut elements: Vec<Element> = Vec::new();
    for p in doc.painted() {
        let id = p.element.id();
        let held = match scope {
            // A frame brings what stands inside it, which `within`
            // already answers — the same seam the renderer and the
            // pointer read, so a picture and its json cannot disagree.
            Scope::Frame(f) => id == f || p.within.is_some_and(|w| &w.id == f),
            Scope::Selection(_) => wanted.iter().any(|w| w == id),
        };
        if held {
            elements.push(p.element.clone());
        }
    }
    let mut layers: Vec<Layer> = Vec::new();
    for el in &elements {
        let name = el.layer();
        if layers.iter().any(|l| l.id == name) {
            continue;
        }
        if let Some((None, at)) = doc.locate(name) {
            layers.push(doc.layers[at].clone());
        }
    }
    // A board is never without a layer, in memory or on disk.
    if layers.is_empty() {
        layers.push(Layer::of("Layer 1", Kind::Raster));
        for el in &mut elements {
            let id = layers[0].id.clone();
            el.set_layer(&id);
        }
    }
    out.layers = layers;
    out.elements = elements;
    out
}

/// What a name becomes as a directory: lowercase, words joined by
/// hyphens, and nothing but `[a-z0-9-]` left. It is the whole of the
/// defence around the typed folder field — `../../etc` holds no
/// character the rule admits, so what comes out is one directory under
/// `docs/boards/` whatever went in. The person names the folder; the
/// shape of the path is not theirs to name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// The fallback when a name slugs to nothing.
const UNNAMED: &str = "board";

/// A slug of `base` that nothing in `taken` already holds, counting up
/// from 2 as a file manager does. `taken` is what `docs/boards/` already
/// holds — the listing is the shell's; picking is not.
pub fn free_name(base: &str, taken: &[String]) -> String {
    let stem = match slug(base) {
        s if s.is_empty() => UNNAMED.to_owned(),
        s => s,
    };
    if !taken.iter().any(|t| *t == stem) {
        return stem;
    }
    (2u32..)
        .map(|n| format!("{stem}-{n}"))
        .find(|c| !taken.iter().any(|t| t == c))
        .unwrap_or(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Rect;

    fn rect(id: &str, layer: &str, x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            id: id.into(),
            layer: layer.into(),
            x,
            y,
            w,
            h,
            rotation: 0.0,
            stroke: None,
            fill: None,
            text: None,
        }
    }

    /// A board with one frame holding one rect, and one loose rect
    /// outside it.
    fn board() -> Document {
        let mut doc = Document::new("plan");
        doc.layers[0].id = "l0".into();
        doc.layers.push(Layer {
            id: "fl".into(),
            name: "Auth Flow".into(),
            visible: true,
            kind: Kind::Frame,
        });
        doc.elements.push(Element::Frame(crate::doc::Frame {
            id: "f1".into(),
            layer: "fl".into(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
            background: Some("#ffffff".into()),
            layers: vec![Layer {
                id: "in".into(),
                name: "Layer 1".into(),
                visible: true,
                kind: Kind::Raster,
            }],
        }));
        doc.elements
            .push(Element::Rect(rect("inside", "in", 10.0, 10.0, 20.0, 20.0)));
        doc.elements
            .push(Element::Rect(rect("outside", "l0", 500.0, 500.0, 10.0, 10.0)));
        doc
    }

    #[test]
    fn a_frames_box_is_the_frames_own_and_not_its_contents() {
        let doc = board();
        let b = bounds(&doc, &Scope::Frame("f1".into())).unwrap();
        assert_eq!(b.center, [50.0, 25.0]);
        assert_eq!(b.half, [50.0, 25.0]);
    }

    #[test]
    fn a_selections_box_wraps_what_is_selected() {
        let doc = board();
        let b = bounds(&doc, &Scope::Selection(vec!["outside".into()])).unwrap();
        assert_eq!(b.center, [505.0, 505.0]);
    }

    #[test]
    fn an_empty_selection_has_no_box() {
        let doc = board();
        assert!(bounds(&doc, &Scope::Selection(vec![])).is_none());
    }

    #[test]
    fn a_frame_is_named_by_its_layer_and_a_selection_is_not_named() {
        let doc = board();
        assert_eq!(
            named(&doc, &Scope::Frame("f1".into())).as_deref(),
            Some("Auth Flow")
        );
        assert_eq!(named(&doc, &Scope::Selection(vec!["outside".into()])), None);
    }

    #[test]
    fn a_frames_sub_document_carries_the_frame_its_stack_and_what_is_on_it() {
        let doc = board();
        let sub = sub_document(&doc, &Scope::Frame("f1".into()));
        let ids: Vec<_> = sub.elements.iter().map(Element::id).collect();
        assert_eq!(ids, ["f1", "inside"], "the frame first, then what it holds");
        assert!(sub.layers.iter().any(|l| l.id == "fl"));
        assert!(
            !sub.elements.iter().any(|e| e.id() == "outside"),
            "what the frame does not hold does not travel"
        );
    }

    #[test]
    fn a_selections_sub_document_carries_the_layers_its_elements_name() {
        let doc = board();
        let sub = sub_document(&doc, &Scope::Selection(vec!["outside".into()]));
        assert_eq!(sub.elements.len(), 1);
        assert_eq!(sub.layers.len(), 1);
        assert_eq!(sub.layers[0].id, "l0");
    }

    #[test]
    fn a_sub_document_is_a_document_that_parses() {
        let doc = board();
        let sub = sub_document(&doc, &Scope::Frame("f1".into()));
        let json = sub.to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back.elements.len(), sub.elements.len());
    }

    #[test]
    fn a_slug_is_lowercase_words_joined_by_hyphens() {
        assert_eq!(slug("Auth Flow"), "auth-flow");
        assert_eq!(slug("  Login   screen  "), "login-screen");
        assert_eq!(slug("Frame 12"), "frame-12");
    }

    #[test]
    fn a_slug_cannot_reach_out_of_the_folder_it_names() {
        // The typed field is the only place a person names a path
        // component. Nothing that steers a path survives the rule.
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("a/b"), "a-b");
        assert_eq!(slug("..."), "");
        assert_eq!(slug("~/.ssh"), "ssh");
        assert_eq!(slug(".hidden"), "hidden");
    }

    #[test]
    fn a_name_that_reduces_to_nothing_is_no_name_at_all() {
        assert_eq!(slug("///"), "");
        assert_eq!(slug("   "), "");
    }

    #[test]
    fn a_free_name_is_the_base_when_the_base_is_free() {
        assert_eq!(free_name("plan", &[]), "plan");
        assert_eq!(free_name("plan", &["other".into()]), "plan");
    }

    #[test]
    fn a_taken_name_counts_up_until_it_is_free() {
        let taken = vec!["plan".into(), "plan-2".into()];
        assert_eq!(free_name("plan", &taken), "plan-3");
    }

    #[test]
    fn a_base_that_slugs_to_nothing_falls_back_to_a_word() {
        assert_eq!(free_name("...", &[]), "board");
        assert_eq!(free_name("...", &["board".into()]), "board-2");
    }
}
