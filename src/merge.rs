//! Merging layers: which siblings go into one, whether moving what they
//! hold onto one layer draws the same picture, and doing it. Only
//! siblings merge — layers of one stack — which is what keeps the result
//! where it was, and a frame never does. Pure — `app` draws the merges
//! that are not exact.

use crate::doc::{BlendMode, Camera, Document, Element, Image, Kind, Layer};
use crate::geom::Point;
use crate::scene::{View, Viewport};
use crate::select;

/// Room past the ink's own frames that a merge's picture keeps, in world
/// units: an edge's feather, and a pixel to spare.
const RASTER_PAD: f64 = 1.0;

/// A merge a board is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    /// The layer into the one under it: `Ctrl+Alt+E` with one picked.
    Down(String),
    /// The picked siblings into one — or a group alone into a layer.
    Layers(Vec<String>),
    /// Every visible sibling of every stack into one, frames and locks
    /// standing between: what is under one stays under it.
    Visible,
    /// Merge visible, then what is hidden goes.
    Flatten,
}

/// Siblings going into one layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The stack they stand in, None for the board's root.
    pub owner: Option<String>,
    /// Bottom to top.
    pub members: Vec<String>,
    /// The member whose place and id the result takes, and its name.
    pub keep: String,
    pub name: String,
}

impl Document {
    /// The runs `merge` makes of this board — none when there is nothing
    /// it can merge: a frame, a holder to merge down into, a lock, or
    /// layers that are not siblings.
    pub fn merges(&self, merge: &Merge) -> Vec<Run> {
        match merge {
            Merge::Down(id) => self.merge_down(id).into_iter().collect(),
            Merge::Layers(ids) => self.merge_layers(ids).into_iter().collect(),
            Merge::Visible | Merge::Flatten => self.merge_visible(),
        }
    }

    fn merge_down(&self, id: &str) -> Option<Run> {
        let (owner, index) = self.locate(id)?;
        let upper = self.layer(id)?;
        let lower = self.stack(owner).get(index.checked_sub(1)?)?;
        let fits = matches!(lower.kind, Kind::Raster | Kind::Vector | Kind::Text) && upper.kind != Kind::Frame;
        let free = !self.locked(id) && !self.holds_lock(upper) && !self.locked(&lower.id);
        (fits && free).then(|| Run {
            owner: owner.map(str::to_owned),
            members: vec![lower.id.clone(), upper.id.clone()],
            keep: lower.id.clone(),
            name: lower.name.clone(),
        })
    }

    fn merge_layers(&self, ids: &[String]) -> Option<Run> {
        let first = ids.first()?;
        let (owner, _) = self.locate(first)?;
        let stack = self.stack(owner);
        // Bottom to top, as the stack has them.
        let members: Vec<&Layer> = stack.iter().filter(|l| ids.contains(&l.id)).collect();
        let siblings = members.len() == ids.len();
        let lone_group = matches!(members.as_slice(), [one] if one.kind == Kind::Group);
        let merges = siblings
            && (members.len() > 1 || lone_group)
            && members
                .iter()
                .all(|l| l.kind != Kind::Frame && !self.locked(&l.id) && !self.holds_lock(l));
        let top = members.last()?;
        merges.then(|| Run {
            owner: owner.map(str::to_owned),
            members: members.iter().map(|l| l.id.clone()).collect(),
            keep: top.id.clone(),
            name: top.name.clone(),
        })
    }

    /// Every stack's visible siblings, cut into runs wherever a frame or
    /// something locked stands: content never moves past either. A run of
    /// one merges only when it is a group, and a locked frame's stack
    /// does not merge at all.
    fn merge_visible(&self) -> Vec<Run> {
        let mut out = Vec::new();
        let mut owners: Vec<Option<&str>> = vec![None];
        owners.extend(self.layers.iter().filter(|l| l.kind == Kind::Frame).map(|l| Some(l.id.as_str())));
        for owner in owners.into_iter().filter(|o| !self.fixed(*o)) {
            let mut run: Vec<&Layer> = Vec::new();
            for layer in self.stack(owner).iter().filter(|l| l.visible) {
                if layer.kind == Kind::Frame || self.holds_lock(layer) {
                    out.extend(visible_run(owner, &run));
                    run.clear();
                } else {
                    run.push(layer);
                }
            }
            out.extend(visible_run(owner, &run));
        }
        out.sort_by_key(|r| self.painted_rank(&r.keep));
        out
    }

    /// Whether `layer`, or anything under it, is locked.
    fn holds_lock(&self, layer: &Layer) -> bool {
        self.subtree(layer)
            .iter()
            .any(|id| self.layer(id).is_some_and(|l| l.locked))
    }

    /// Where `id` stands in paint order.
    fn painted_rank(&self, id: &str) -> usize {
        self.rows(|_| true)
            .iter()
            .rev()
            .position(|r| r.layer.id == id)
            .unwrap_or(usize::MAX)
    }

    /// Whether moving what `run` shows onto one layer draws the picture
    /// it drew: every layer that shows in it normal — or passing through,
    /// for a group — and at full strength, and none of it text. A text
    /// stands only on a text layer and the kept one becomes a raster
    /// layer, so a merge that takes text in draws it, as Photoshop
    /// rasterizes a type layer that is merged.
    pub fn exact(&self, run: &Run) -> bool {
        run.members.iter().all(|id| {
            self.layer(id).is_some_and(|l| {
                self.shown_subtree(l).into_iter().all(|l| {
                    let mode = l.blend == BlendMode::Normal
                        || (l.blend == BlendMode::PassThrough && l.kind == Kind::Group);
                    mode && l.opacity >= 1.0 && l.kind != Kind::Text
                })
            })
        })
    }

    /// `layer` and everything under it that shows.
    fn shown_subtree<'a>(&'a self, layer: &'a Layer) -> Vec<&'a Layer> {
        if !layer.visible {
            return Vec::new();
        }
        let mut out = vec![layer];
        for inner in self.inner(layer) {
            out.extend(self.shown_subtree(inner));
        }
        out
    }

    /// Merges `run` by moving the objects it shows onto its kept layer in
    /// paint order — curves stay curves — and letting the rest of it go:
    /// the other members, what they held, and whatever was hidden. The
    /// kept layer becomes a raster layer, since it holds more than the one
    /// object a vector layer is made for; two paints that meet merge into
    /// one, unless the upper one erases, which would then rub out the
    /// lower one too.
    pub fn merge_structural(&mut self, run: &Run) {
        let members = self.under(run);
        let order: Vec<usize> = self
            .painted()
            .filter(|p| members.iter().any(|m| m == p.element.layer()))
            .map(|p| p.index)
            .collect();
        let mut moved: Vec<Element> = Vec::new();
        for i in order {
            let mut el = self.elements[i].clone();
            el.set_layer(&run.keep);
            match (moved.last_mut(), &el) {
                (Some(Element::Paint(below)), Element::Paint(above))
                    if below.rotation == above.rotation && !above.strokes.iter().any(|s| s.erases()) =>
                {
                    below.strokes.extend(above.strokes.iter().cloned());
                }
                _ => moved.push(el),
            }
        }
        self.elements.retain(|el| !members.iter().any(|m| m == el.layer()));
        self.elements.extend(moved);
        self.settle_kept(run);
    }

    /// What a run's picture is drawn from: its members as they stand —
    /// every property they composite with — and what stands on them, and
    /// nothing else of the board.
    pub fn run_document(&self, run: &Run) -> Document {
        let layers: Vec<Layer> = run
            .members
            .iter()
            .filter_map(|id| self.layer(id))
            .cloned()
            .collect();
        let under = self.under(run);
        let elements = self
            .elements
            .iter()
            .filter(|el| under.iter().any(|u| u == el.layer()))
            .cloned()
            .collect();
        Document {
            layers,
            elements,
            ..Document::new(&self.title)
        }
    }

    /// Merges `run` into the picture taken of it: the kept layer holds
    /// `image` and nothing else, normal and at full strength, since the
    /// picture already is what the members drew; the other members go,
    /// with everything they held.
    pub fn merge_raster(&mut self, run: &Run, image: Image) {
        let members = self.under(run);
        self.elements.retain(|el| !members.iter().any(|m| m == el.layer()));
        self.elements.push(Element::Image(Image {
            layer: run.keep.clone(),
            ..image
        }));
        self.settle_kept(run);
    }

    /// Every layer a run's members are, and hold.
    fn under(&self, run: &Run) -> Vec<String> {
        run.members
            .iter()
            .filter_map(|id| self.layer(id))
            .flat_map(|l| self.subtree(l))
            .collect()
    }

    /// The other members out of the stack, and the kept one a raster layer
    /// under the run's name — normal and whole, since what it holds now
    /// is what they drew — shown if any of them was.
    fn settle_kept(&mut self, run: &Run) {
        let shown = run.members.iter().any(|id| self.layer(id).is_some_and(|l| l.visible));
        if let Some(stack) = self.stack_mut(run.owner.as_deref()) {
            stack.retain(|l| l.id == run.keep || !run.members.contains(&l.id));
        }
        if let Some(keep) = self.layer_mut(&run.keep) {
            *keep = Layer {
                id: keep.id.clone(),
                visible: shown,
                color: keep.color,
                ..Layer::of(&run.name, Kind::Raster)
            };
        }
    }

    /// Takes away every hidden layer, with everything on and under it —
    /// flatten's second half. True when anything went.
    pub fn discard_hidden(&mut self) -> bool {
        let hidden: Vec<String> = self
            .rows(|_| true)
            .iter()
            .filter(|r| !r.layer.visible)
            .flat_map(|r| self.subtree(r.layer))
            .collect();
        if hidden.is_empty() {
            return false;
        }
        self.elements.retain(|el| !hidden.iter().any(|h| h == el.layer()));
        shown_only(&mut self.layers);
        for el in &mut self.elements {
            if let Element::Frame(f) = el {
                shown_only(&mut f.layers);
            }
        }
        self.fill_empty_stacks();
        true
    }
}

/// `layers` and every group's under them, with the hidden taken out.
fn shown_only(layers: &mut Vec<Layer>) {
    layers.retain(|l| l.visible);
    for l in layers {
        shown_only(&mut l.layers);
    }
}

/// The world box a picture of `sub` has to cover: every element's frame —
/// the ink's, width and all — with room for what a nib throws past it.
pub fn raster_box(sub: &Document) -> Option<(Point, Point)> {
    let mut span: Option<(Point, Point)> = None;
    for el in &sub.elements {
        let Some(frame) = select::frame(el) else { continue };
        let thrown = match el {
            Element::Path(p) => p.stamp.as_ref().map_or(0.0, |s| s.scatter.size),
            Element::Paint(p) => p
                .strokes
                .iter()
                .filter_map(|s| s.stamp.as_ref())
                .map(|s| s.scatter.size)
                .fold(0.0, f64::max),
            // A letter may reach past the advance it is set on — an
            // italic's lean, a swash — and past the box with it.
            Element::Text(t) => t.style.size / 4.0,
            _ => 0.0,
        };
        let pad = RASTER_PAD + thrown;
        let (lo, hi) = frame.aabb();
        let (lo, hi) = ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad]);
        span = Some(match span {
            None => (lo, hi),
            Some((l, h)) => ([l[0].min(lo[0]), l[1].min(lo[1])], [h[0].max(hi[0]), h[1].max(hi[1])]),
        });
    }
    span
}

/// The camera and the size in px a picture of `lo`..`hi` is taken with:
/// `ceiling` px to a world unit, fewer where that would pass `max_side` —
/// the zoom gives way, never the box — and the world box its pixels cover
/// exactly, a pixel's rounding past `hi`, which is where it is laid.
pub fn raster_view((lo, hi): (Point, Point), ceiling: f64, max_side: u32) -> (View, u32, u32, (Point, Point)) {
    let (world_w, world_h) = ((hi[0] - lo[0]).max(f64::MIN_POSITIVE), (hi[1] - lo[1]).max(f64::MIN_POSITIVE));
    let most = f64::from(max_side.max(1));
    let scale = ceiling.min(most / world_w).min(most / world_h).max(f64::MIN_POSITIVE);
    let w = ((world_w * scale).ceil() as u32).clamp(1, max_side.max(1));
    let h = ((world_h * scale).ceil() as u32).clamp(1, max_side.max(1));
    let covered = [lo[0] + f64::from(w) / scale, lo[1] + f64::from(h) / scale];
    let view = View {
        camera: Camera {
            x: (lo[0] + covered[0]) / 2.0,
            y: (lo[1] + covered[1]) / 2.0,
            zoom: scale,
        },
        viewport: Viewport { w, h },
        scale: 1.0,
    };
    (view, w, h, (lo, covered))
}

/// A run of visible siblings as a merge: two or more, or a group alone.
fn visible_run(owner: Option<&str>, run: &[&Layer]) -> Option<Run> {
    let top = run.last()?;
    (run.len() > 1 || top.kind == Kind::Group).then(|| Run {
        owner: owner.map(str::to_owned),
        members: run.iter().map(|l| l.id.clone()).collect(),
        keep: top.id.clone(),
        name: top.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{BlendMode, Document, Element, Image, Kind};

    /// A board of `layers` (JSON, bottom to top) and a rect on each layer
    /// named in `on`, its id the layer's lowercased.
    fn board(layers: &str, on: &[&str]) -> Document {
        let rects: Vec<String> = on
            .iter()
            .map(|l| {
                format!(
                    r##"{{ "id": "{}", "type": "rect", "layer": "{l}", "x": 0, "y": 0, "w": 1, "h": 1,
                          "stroke": null, "fill": "#ff0000", "text": null }}"##,
                    l.to_lowercase()
                )
            })
            .collect();
        Document::from_json(&format!(
            r#"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                  "layers": [{layers}], "elements": [{}] }}"#,
            rects.join(",")
        ))
        .expect("the board parses")
    }

    fn stack(doc: &Document, owner: Option<&str>) -> Vec<String> {
        doc.stack(owner).iter().map(|l| l.id.clone()).collect()
    }

    /// The elements on `layer`, in document order.
    fn on(doc: &Document, layer: &str) -> Vec<String> {
        doc.elements
            .iter()
            .filter(|e| e.layer() == layer)
            .map(|e| e.id().to_owned())
            .collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    const TWO: &str = r#"{ "id": "L1", "name": "Layer 1" }, { "id": "L2", "name": "Layer 2", "kind": "vector" }"#;

    #[test]
    fn merge_down_takes_the_active_layer_into_the_one_under_it() {
        let mut doc = board(TWO, &["L1", "L2"]);
        let runs = doc.merges(&Merge::Down("L2".into()));
        assert_eq!(
            runs,
            [Run {
                owner: None,
                members: ids(&["L1", "L2"]),
                keep: "L1".into(),
                name: "Layer 1".into(),
            }]
        );
        assert!(doc.exact(&runs[0]));
        doc.merge_structural(&runs[0]);
        assert_eq!(stack(&doc, None), ["L1"]);
        assert_eq!(on(&doc, "L1"), ["l1", "l2"], "in paint order");
        assert_eq!(doc.layer("L1").unwrap().kind, Kind::Raster, "it holds more than one object now");
        assert!(Document::from_json(&doc.to_json().unwrap()).is_ok());
    }

    #[test]
    fn nothing_merges_down_into_a_holder_nor_out_of_the_bottom_nor_under_a_lock() {
        let group = r#"{ "id": "G", "name": "Group", "kind": "group", "layers": [ { "id": "B", "name": "B" } ] },
                       { "id": "L2", "name": "Layer 2" }"#;
        let doc = board(group, &["B", "L2"]);
        assert!(doc.merges(&Merge::Down("L2".into())).is_empty(), "a group is no layer to merge into");
        let doc = board(TWO, &["L1", "L2"]);
        assert!(doc.merges(&Merge::Down("L1".into())).is_empty(), "nothing under it");
        let mut doc = board(TWO, &["L1", "L2"]);
        doc.layer_mut("L1").unwrap().locked = true;
        assert!(doc.merges(&Merge::Down("L2".into())).is_empty(), "a lock keeps what it holds");
        let frame = r#"{ "id": "L1", "name": "Layer 1" }, { "id": "F", "name": "Frame", "kind": "frame" }"#;
        let doc = Document::from_json(&format!(
            r#"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": {{ "x": 0, "y": 0, "zoom": 1 }}, "layers": [{frame}],
                  "elements": [ {{ "id": "fr", "type": "frame", "layer": "F", "x": 0, "y": 0, "w": 9, "h": 9 }} ] }}"#
        ))
        .unwrap();
        assert!(doc.merges(&Merge::Down("F".into())).is_empty(), "a frame never merges");
    }

    #[test]
    fn merging_a_group_keeps_what_it_shows_and_drops_what_it_hides() {
        let group = r#"{ "id": "G", "name": "Group", "kind": "group", "layers": [
                           { "id": "B", "name": "B" }, { "id": "C", "name": "C", "visible": false } ] }"#;
        let mut doc = board(group, &["B", "C"]);
        let runs = doc.merges(&Merge::Layers(ids(&["G"])));
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].keep.as_str(), runs[0].name.as_str()), ("G", "Group"));
        doc.merge_structural(&runs[0]);
        let g = doc.layer("G").unwrap();
        assert_eq!((g.kind, g.layers.len()), (Kind::Raster, 0));
        assert_eq!(on(&doc, "G"), ["b"]);
        assert!(doc.elements.iter().all(|e| e.id() != "c"), "what was hidden is gone");
    }

    #[test]
    fn merging_the_picked_siblings_names_them_after_the_topmost() {
        let three = r#"{ "id": "L1", "name": "Bottom" }, { "id": "L2", "name": "Middle" }, { "id": "L3", "name": "Top" }"#;
        let mut doc = board(three, &["L1", "L2", "L3"]);
        let runs = doc.merges(&Merge::Layers(ids(&["L3", "L1"])));
        assert_eq!(runs[0].members, ids(&["L1", "L3"]));
        assert_eq!((runs[0].keep.as_str(), runs[0].name.as_str()), ("L3", "Top"));
        doc.merge_structural(&runs[0]);
        assert_eq!(stack(&doc, None), ["L2", "L3"]);
        assert_eq!(on(&doc, "L3"), ["l1", "l3"]);
        // Siblings only, and more than one.
        let nested = r#"{ "id": "L1", "name": "L1" }, { "id": "G", "name": "G", "kind": "group", "layers": [ { "id": "B", "name": "B" } ] }"#;
        let doc = board(nested, &["L1", "B"]);
        assert!(doc.merges(&Merge::Layers(ids(&["L1", "B"]))).is_empty());
        assert!(doc.merges(&Merge::Layers(ids(&["L1"]))).is_empty());
    }

    #[test]
    fn a_merge_is_exact_only_where_everything_is_normal_and_whole() {
        let doc = board(TWO, &["L1", "L2"]);
        let run = doc.merges(&Merge::Down("L2".into())).remove(0);
        let mut faded = doc.clone();
        faded.layer_mut("L2").unwrap().opacity = 0.5;
        assert!(!faded.exact(&run));
        let mut screened = doc.clone();
        screened.layer_mut("L1").unwrap().blend = BlendMode::Screen;
        assert!(!screened.exact(&run));
        let group = r#"{ "id": "G", "name": "G", "kind": "group", "blend": "passThrough", "layers": [
                           { "id": "B", "name": "B" }, { "id": "C", "name": "C" } ] }"#;
        let mut doc = board(group, &["B", "C"]);
        let run = doc.merges(&Merge::Layers(ids(&["G"]))).remove(0);
        assert!(doc.exact(&run), "a group passing through at full strength is its layers");
        doc.layer_mut("C").unwrap().blend = BlendMode::Multiply;
        assert!(!doc.exact(&run), "a mode inside it is not");
        doc.layer_mut("C").unwrap().blend = BlendMode::Normal;
        doc.layer_mut("C").unwrap().visible = false;
        doc.layer_mut("C").unwrap().opacity = 0.2;
        assert!(doc.exact(&run), "what is hidden does not draw, and goes");
    }

    #[test]
    fn a_text_is_rasterized_when_it_merges_as_every_editor_does() {
        let layers = r#"{ "id": "L1", "name": "Layer 1" }, { "id": "T", "name": "Words", "kind": "text" }"#;
        let doc = Document::from_json(&format!(
            r##"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": {{ "x": 0, "y": 0, "zoom": 1 }}, "layers": [{layers}],
                  "elements": [ {{ "id": "t", "type": "text", "layer": "T", "x": 0, "y": 0,
                      "w": 40, "h": 20, "text": "hi", "size": 16, "color": "#000" }} ] }}"##
        ))
        .unwrap();
        let run = doc.merges(&Merge::Down("T".into())).remove(0);
        assert!(!doc.exact(&run), "a text's words cannot stand on a raster layer");
        // And one layer merged down into a text layer is drawn too.
        let under = r#"{ "id": "T", "name": "Words", "kind": "text" }, { "id": "L2", "name": "Layer 2" }"#;
        let doc = Document::from_json(&format!(
            r##"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": {{ "x": 0, "y": 0, "zoom": 1 }}, "layers": [{under}],
                  "elements": [ {{ "id": "t", "type": "text", "layer": "T", "x": 0, "y": 0,
                      "w": 40, "h": 20, "text": "hi", "size": 16, "color": "#000" }} ] }}"##
        ))
        .unwrap();
        let runs = doc.merges(&Merge::Down("L2".into()));
        assert_eq!(runs.len(), 1, "a text layer takes what is merged down into it");
        assert!(!doc.exact(&runs[0]));
    }

    #[test]
    fn two_paints_become_one_unless_the_upper_one_erases() {
        let paint = |id: &str, layer: &str, mark: &str| {
            format!(
                r##"{{ "id": "{id}", "type": "paint", "layer": "{layer}", "strokes": [
                      {{ "curves": [[[0,0],[1,0],[2,0],[3,0]]], "stroke": "#000000", "width": 4,
                         "stamp": {{ "spacing": 1.2, "roundness": 1, "rotation": 0, "mark": "{mark}" }} }} ] }}"##
            )
        };
        let doc_with = |upper: &str| {
            Document::from_json(&format!(
                r#"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                      "camera": {{ "x": 0, "y": 0, "zoom": 1 }}, "layers": [{TWO}],
                      "elements": [{}, {}] }}"#,
                paint("p1", "L1", "normal"),
                paint("p2", "L2", upper)
            ))
            .unwrap()
        };
        let mut doc = doc_with("normal");
        let run = doc.merges(&Merge::Down("L2".into())).remove(0);
        doc.merge_structural(&run);
        assert_eq!(on(&doc, "L1"), ["p1"], "one paint");
        let Some(Element::Paint(p)) = doc.elements.first() else { panic!("a paint") };
        assert_eq!(p.strokes.len(), 2);
        let mut doc = doc_with("eraser");
        let run = doc.merges(&Merge::Down("L2".into())).remove(0);
        doc.merge_structural(&run);
        assert_eq!(on(&doc, "L1"), ["p1", "p2"], "an eraser rubs out its own paint and nothing under it");
    }

    const RUNS: &str = r#"
        { "id": "A", "name": "A" }, { "id": "B", "name": "B", "visible": false }, { "id": "C", "name": "C" },
        { "id": "F", "name": "F", "kind": "frame" },
        { "id": "H", "name": "H" }, { "id": "K", "name": "K" }"#;

    fn runs_board() -> Document {
        let rect = |id: &str, layer: &str| {
            format!(
                r##"{{ "id": "{id}", "type": "rect", "layer": "{layer}", "x": 0, "y": 0, "w": 1, "h": 1,
                      "stroke": null, "fill": "#ff0000", "text": null }}"##
            )
        };
        let elements = ["a", "b", "c", "h", "k", "d", "e"]
            .iter()
            .map(|id| rect(id, &id.to_uppercase()))
            .collect::<Vec<_>>()
            .join(",");
        Document::from_json(&format!(
            r#"{{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": {{ "x": 0, "y": 0, "zoom": 1 }}, "layers": [{RUNS}],
                  "elements": [ {{ "id": "fr", "type": "frame", "layer": "F", "x": 0, "y": 0, "w": 9, "h": 9,
                                   "layers": [ {{ "id": "D", "name": "D" }}, {{ "id": "E", "name": "E" }} ] }},
                                {elements} ] }}"#
        ))
        .unwrap()
    }

    #[test]
    fn merge_visible_merges_each_run_between_frames_and_leaves_the_hidden_where_it_was() {
        let mut doc = runs_board();
        let runs = doc.merges(&Merge::Visible);
        let kept: Vec<(&str, Vec<String>)> = runs.iter().map(|r| (r.keep.as_str(), r.members.clone())).collect();
        assert_eq!(
            kept,
            [("C", ids(&["A", "C"])), ("E", ids(&["D", "E"])), ("K", ids(&["H", "K"]))]
        );
        for run in &runs {
            doc.merge_structural(run);
        }
        assert_eq!(stack(&doc, None), ["B", "C", "F", "K"]);
        assert_eq!(stack(&doc, Some("F")), ["E"]);
        assert_eq!(on(&doc, "C"), ["a", "c"]);
        assert_eq!(on(&doc, "B"), ["b"], "hidden, and kept");
    }

    #[test]
    fn a_locked_layer_stands_between_runs_as_a_frame_does() {
        let mut doc = runs_board();
        doc.layer_mut("H").unwrap().locked = true;
        let runs = doc.merges(&Merge::Visible);
        assert!(runs.iter().all(|r| !r.members.iter().any(|m| m == "H")));
        assert!(!runs.iter().any(|r| r.keep == "K"), "K stands alone above the lock");
    }

    #[test]
    fn flatten_is_merge_visible_with_the_hidden_gone() {
        let mut doc = runs_board();
        let runs = doc.merges(&Merge::Flatten);
        for run in &runs {
            doc.merge_structural(run);
        }
        assert!(doc.discard_hidden());
        assert_eq!(stack(&doc, None), ["C", "F", "K"]);
        assert!(doc.elements.iter().all(|e| e.id() != "b"));
        assert!(!doc.discard_hidden(), "nothing left to discard");
    }

    const THREE: &str = r#"{ "id": "L1", "name": "Layer 1" }, { "id": "L2", "name": "Layer 2" }, { "id": "L3", "name": "Layer 3" }"#;

    #[test]
    fn a_run_is_drawn_from_its_members_as_they_stand_and_nothing_else() {
        let mut doc = board(THREE, &["L1", "L2", "L3"]);
        doc.layer_mut("L2").unwrap().opacity = 0.5;
        let run = doc.merges(&Merge::Down("L2".into())).remove(0);
        assert!(!doc.exact(&run));
        let sub = doc.run_document(&run);
        assert_eq!(stack(&sub, None), ["L1", "L2"]);
        assert_eq!(sub.layer("L2").unwrap().opacity, 0.5, "drawn as it composites");
        assert_eq!(
            sub.elements.iter().map(|e| e.id()).collect::<Vec<_>>(),
            ["l1", "l2"]
        );
    }

    #[test]
    fn a_picture_covers_the_ink_and_what_a_nib_throws_past_it() {
        let doc = board(TWO, &["L1"]);
        assert_eq!(raster_box(&doc), Some(([-1.0, -1.0], [2.0, 2.0])));
        let path = Document::from_json(
            r##"{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": { "x": 0, "y": 0, "zoom": 1 }, "layers": [ { "id": "L1", "name": "L1" } ],
                  "elements": [ { "id": "p", "type": "path", "layer": "L1", "width": 10, "stroke": "#000000",
                                  "curves": [[[0,0],[10,0],[20,0],[30,0]]],
                                  "stamp": { "spacing": 1.2, "roundness": 1, "rotation": 0,
                                             "scatter": { "size": 3, "rotation": 0 } } } ] }"##,
        )
        .unwrap();
        assert_eq!(raster_box(&path), Some(([-9.0, -9.0], [39.0, 9.0])));
        assert_eq!(raster_box(&Document::new("t")), None, "nothing to draw");
    }

    #[test]
    fn a_picture_is_taken_pixel_for_pixel_over_the_box_it_lands_in() {
        let (view, w, h, (lo, hi)) = raster_view(([0.0, 0.0], [48.0, 18.0]), 2.0, 8192);
        assert_eq!((w, h), (96, 36));
        assert_eq!((lo, hi), ([0.0, 0.0], [48.0, 18.0]));
        assert_eq!(view.world_to_screen(0.0, 0.0), (0.0, 0.0));
        assert_eq!(view.world_to_screen(48.0, 18.0), (96.0, 36.0));
        // A box too big for the device gives up zoom, never the box.
        let (_, w, h, (lo, hi)) = raster_view(([0.0, 0.0], [20000.0, 10.3]), 2.0, 8192);
        assert!(w <= 8192 && h >= 1);
        assert!(hi[0] - lo[0] >= 20000.0 && hi[1] - lo[1] >= 10.3, "all of it, and a pixel's rounding");
    }

    #[test]
    fn a_raster_merge_leaves_the_picture_alone_on_the_kept_layer() {
        let mut doc = board(THREE, &["L1", "L2", "L3"]);
        doc.layer_mut("L2").unwrap().opacity = 0.5;
        doc.layer_mut("L1").unwrap().blend = BlendMode::Screen;
        let run = doc.merges(&Merge::Down("L2".into())).remove(0);
        let image = Image {
            id: "pic".into(),
            layer: String::new(),
            x: -1.0,
            y: -1.0,
            w: 3.0,
            h: 3.0,
            rotation: 0.0,
            blob: "a".repeat(64),
        };
        doc.merge_raster(&run, image);
        assert_eq!(stack(&doc, None), ["L1", "L3"]);
        let l1 = doc.layer("L1").unwrap();
        assert_eq!((l1.kind, l1.opacity, l1.blend), (Kind::Raster, 1.0, BlendMode::Normal));
        assert_eq!(on(&doc, "L1"), ["pic"]);
        assert_eq!(on(&doc, "L3"), ["l3"], "the rest of the board is left as it was");
        assert!(Document::from_json(&doc.to_json().unwrap()).is_ok());
    }
}
