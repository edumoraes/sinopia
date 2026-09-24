//! The layer tree: where a layer stands, what holds it, and what that
//! makes of it — whether it shows, whether it is locked, which frame cuts
//! it, and the order the panel lists it in.
//!
//! A stack is named by the layer that holds it: `None` is the board's
//! root, a group's id is its children, and a frame layer's id is the
//! stack of the frame on it. One address for all three, so the panel,
//! the editor and the CLI speak the same ids. Pure.

use crate::doc::{Document, Element, Frame, Kind, Layer};

impl Document {
    /// The layer `id`, however deep it stands, in whichever stack.
    pub fn layer(&self, id: &str) -> Option<&Layer> {
        self.trail(id)?.last().copied()
    }

    /// The same, to change.
    pub fn layer_mut(&mut self, id: &str) -> Option<&mut Layer> {
        // Asked first and taken second: a conditional return of a
        // mutable borrow is one the borrow checker will not follow.
        if find(&self.layers, id).is_some() {
            return find_mut(&mut self.layers, id);
        }
        self.elements.iter_mut().find_map(|el| match el {
            Element::Frame(f) => find_mut(&mut f.layers, id),
            _ => None,
        })
    }

    /// What layer `holder` holds: a group's children, or the stack of the
    /// frame on a frame layer. Empty for a layer that holds nothing.
    pub fn inner<'a>(&'a self, holder: &'a Layer) -> &'a [Layer] {
        match holder.kind {
            Kind::Frame => self.frame_on(&holder.id).map_or(&[], |f| &f.layers),
            _ => &holder.layers,
        }
    }

    /// The layers from the top of `id`'s tree down to `id` itself: a
    /// root layer of the board first, then every group — or the frame
    /// layer whose stack it is in — on the way down.
    fn trail(&self, id: &str) -> Option<Vec<&Layer>> {
        let mut path = Vec::new();
        self.trail_in(&self.layers, id, &mut path).then_some(path)
    }

    fn trail_in<'a>(&'a self, layers: &'a [Layer], id: &str, path: &mut Vec<&'a Layer>) -> bool {
        for layer in layers {
            path.push(layer);
            if layer.id == id || self.trail_in(self.inner(layer), id, path) {
                return true;
            }
            path.pop();
        }
        false
    }

    /// Where `id` stands: the layer holding its stack — none for the
    /// board's root — and its index in that stack.
    pub fn locate(&self, id: &str) -> Option<(Option<&str>, usize)> {
        let trail = self.trail(id)?;
        let owner = trail.len().checked_sub(2).map(|i| trail[i]);
        let stack = match owner {
            None => self.layers.as_slice(),
            Some(holder) => self.inner(holder),
        };
        let index = stack.iter().position(|l| l.id == id)?;
        Some((owner.map(|l| l.id.as_str()), index))
    }


    /// The layers holding `id`, nearest first.
    pub fn ancestors(&self, id: &str) -> Vec<&Layer> {
        let mut trail = self.trail(id).unwrap_or_default();
        trail.pop();
        trail.reverse();
        trail
    }

    /// The frame whose stack `id` stands in, through however many groups
    /// — none for a layer on the board, a frame's own layer included.
    pub fn frame_holding(&self, id: &str) -> Option<&Frame> {
        let trail = self.trail(id)?;
        let top = trail.first()?;
        (trail.len() > 1 && top.kind == Kind::Frame)
            .then(|| self.frame_on(&top.id))
            .flatten()
    }



    /// Whether `id` is on show: it and everything holding it visible.
    pub fn shown(&self, id: &str) -> bool {
        self.trail(id).is_some_and(|t| t.iter().all(|l| l.visible))
    }

    /// Whether `id`'s content is locked: it, or anything holding it.
    pub fn locked(&self, id: &str) -> bool {
        self.trail(id).is_some_and(|t| t.iter().any(|l| l.locked))
    }

    /// Every layer in the order the panel lists it — top first, a
    /// holder's layers under it and one deeper — descending only into
    /// the holders `open` says are expanded.
    pub fn rows(&self, open: impl Fn(&str) -> bool) -> Vec<Row<'_>> {
        let mut out = Vec::new();
        self.rows_in(&self.layers, None, &open, &mut out);
        out
    }

    /// `layers`, top first, onto `out`: what `parent` is — whether it
    /// shows, whether it is locked — reaches every row under it. None is
    /// the board's own root, which nothing holds.
    fn rows_in<'a>(
        &'a self,
        layers: &'a [Layer],
        parent: Option<&Row<'a>>,
        open: &impl Fn(&str) -> bool,
        out: &mut Vec<Row<'a>>,
    ) {
        for layer in layers.iter().rev() {
            let holds = matches!(layer.kind, Kind::Group | Kind::Frame);
            let row = Row {
                layer,
                owner: parent.map(|p| p.layer.id.as_str()),
                depth: parent.map_or(0, |p| p.depth + 1),
                shown: parent.is_none_or(|p| p.shown) && layer.visible,
                locked: parent.is_some_and(|p| p.locked) || layer.locked,
                open: holds && open(&layer.id),
            };
            out.push(row);
            if row.open {
                self.rows_in(self.inner(layer), Some(&row), open, out);
            }
        }
    }

    /// The stack `owner` holds — the board's root for none. Empty for a
    /// layer that holds nothing, or that is not there.
    pub fn stack(&self, owner: Option<&str>) -> &[Layer] {
        match owner {
            None => &self.layers,
            Some(id) => self.layer(id).map_or(&[], |l| self.inner(l)),
        }
    }

    pub fn stack_mut(&mut self, owner: Option<&str>) -> Option<&mut Vec<Layer>> {
        let Some(id) = owner else {
            return Some(&mut self.layers);
        };
        match self.layer(id)?.kind {
            Kind::Frame => self.elements.iter_mut().find_map(|el| match el {
                Element::Frame(f) if f.layer == id => Some(&mut f.layers),
                _ => None,
            }),
            Kind::Group => self.layer_mut(id).map(|l| &mut l.layers),
            Kind::Raster | Kind::Vector => None,
        }
    }

    /// The frame layer whose stack `id` stands in — the frame's area is
    /// what cuts it — or none for a layer on the board.
    pub fn context(&self, id: &str) -> Option<&str> {
        self.frame_holding(id).map(|f| f.layer.as_str())
    }

    /// The topmost frame whose area holds `p`, by its layer — the stack a
    /// press there lands in — or none for the open board. A hidden frame
    /// claims nothing: what is not painted is not there, for the pointer
    /// as for the eye.
    pub fn stack_at(&self, p: [f64; 2]) -> Option<&str> {
        self.layers
            .iter()
            .rev()
            .filter(|l| l.visible && l.kind == Kind::Frame)
            .find_map(|l| {
                self.frame_on(&l.id)
                    .filter(|f| f.contains(p))
                    .map(|f| f.layer.as_str())
            })
    }

    /// Every layer id in `layer`'s subtree, itself first — a group's
    /// children, a frame's whole stack.
    pub fn subtree(&self, layer: &Layer) -> Vec<String> {
        let mut out = vec![layer.id.clone()];
        for inner in self.inner(layer) {
            out.extend(self.subtree(inner));
        }
        out
    }

    /// Adds a layer of `kind` just above `above` (on top when that is
    /// past the end) in the stack `owner` holds, and answers its index.
    /// It is named for its kind, with N past every number in use under
    /// that word **in that stack's tree** — the board's, or one frame's —
    /// so a frame's first layer is `Layer 1` however many the board has.
    /// None when the stack is not there, or the layer is a frame asked
    /// for anywhere but the board's root.
    pub fn add_layer(&mut self, owner: Option<&str>, above: usize, kind: Kind) -> Option<usize> {
        if kind == Kind::Frame && owner.is_some() {
            return None;
        }
        let name = self.next_layer_name(owner, kind);
        let layers = self.stack_mut(owner)?;
        let at = above.saturating_add(1).min(layers.len());
        layers.insert(at, Layer::of(&name, kind));
        Some(at)
    }

    /// The name a new layer of `kind` takes in `owner`'s stack: one past
    /// the highest number already carried under that kind's own word
    /// anywhere in the same tree. A frame is not a gap in the layers'
    /// numbering, a layer is not a gap in the groups'.
    pub(crate) fn next_layer_name(&self, owner: Option<&str>, kind: Kind) -> String {
        let word = match kind {
            Kind::Frame => "Frame",
            Kind::Group => "Group",
            Kind::Raster | Kind::Vector => "Layer",
        };
        // The tree it is counted in: a frame's own stack when the new
        // layer goes inside one, the board's otherwise — which a frame's
        // stack is not part of, since it lives on the frame.
        let frame = owner.and_then(|o| match self.layer(o)?.kind {
            Kind::Frame => Some(o),
            _ => self.context(o),
        });
        let prefix = format!("{word} ");
        let mut names = Vec::new();
        names_in(self.stack(frame), &mut names);
        let highest = names
            .iter()
            .filter_map(|n| n.strip_prefix(&prefix)?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("{word} {}", highest.saturating_add(1))
    }

    /// Removes layer `index` of the stack `owner` holds, everything under
    /// it and every element on any of those — a frame layer takes its
    /// frame and its frame's whole stack, a group its children. The
    /// board and a frame keep their last layer: false then, and for an
    /// index past the end. A group may be left empty.
    pub fn remove_layer(&mut self, owner: Option<&str>, index: usize) -> bool {
        let keeps_last = match owner {
            None => true,
            Some(o) => self.layer(o).is_some_and(|l| l.kind == Kind::Frame),
        };
        let Some(gone) = self.stack(owner).get(index) else {
            return false;
        };
        if keeps_last && self.stack(owner).len() < 2 {
            return false;
        }
        // Read before anything goes: a frame's stack is found through the
        // frame, which is one of the elements about to be dropped.
        let held = self.subtree(gone);
        let Some(layers) = self.stack_mut(owner) else {
            return false;
        };
        layers.remove(index);
        self.elements
            .retain(|el| !held.iter().any(|id| id == el.layer()));
        true
    }

    /// Swaps layer `index` with the one above it (`up`) or below, and
    /// answers where it went. Nothing moves past the edge.
    pub fn move_layer(&mut self, owner: Option<&str>, index: usize, up: bool) -> Option<usize> {
        let to = if up {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        self.reorder_layer(owner, index, to).then_some(to)
    }

    /// Takes layer `from` out of the stack `owner` holds and puts it back
    /// at `to`, shifting whatever lies between and leaving their order
    /// alone — what a row dragged several places down does. False when
    /// either index is past the end, or the layer is already there.
    pub fn reorder_layer(&mut self, owner: Option<&str>, from: usize, to: usize) -> bool {
        let Some(layers) = self.stack_mut(owner) else {
            return false;
        };
        if from >= layers.len() || to >= layers.len() || from == to {
            return false;
        }
        let layer = layers.remove(from);
        layers.insert(to, layer);
        true
    }

    /// Moves the layer `layer` — and so the object on it — to the top of
    /// the stack of frame layer `to`, or of the board's root. A layer
    /// holds one object, so the two are one move: the element goes on
    /// naming its layer, and its layer changes stacks. False when the
    /// layer is a frame's own (a frame does not nest), when it already
    /// stands in that frame's tree — a group there is left as it is — or
    /// when either end is missing.
    pub fn rehome_layer(&mut self, layer: &str, to: Option<&str>) -> bool {
        let Some((from, index)) = self.locate(layer).map(|(o, i)| (o.map(str::to_owned), i)) else {
            return false;
        };
        if self.context(layer) == to {
            return false;
        }
        if self.stack(from.as_deref())[index].kind == Kind::Frame {
            return false;
        }
        if to.is_some_and(|t| self.layer(t).is_none_or(|l| l.kind != Kind::Frame)) {
            return false;
        }
        let keeps_last = match from.as_deref() {
            None => true,
            Some(o) => self.layer(o).is_some_and(|l| l.kind == Kind::Frame),
        };
        let Some(layers) = self.stack_mut(from.as_deref()) else {
            return false;
        };
        let taken = layers.remove(index);
        // The board or a frame may now be empty, and neither ever is: it
        // gets a fresh layer, as the parse would have given it. A group
        // may stand empty.
        if keeps_last && layers.is_empty() {
            let name = self.next_layer_name(from.as_deref(), Kind::Raster);
            if let Some(layers) = self.stack_mut(from.as_deref()) {
                layers.push(Layer::new(&name));
            }
        }
        match self.stack_mut(to) {
            Some(layers) => {
                layers.push(taken);
                true
            }
            None => false,
        }
    }


}


/// One line of the panel's tree: the layer, the layer holding its stack
/// (none on the board's root), how deep it stands, and what the layers
/// holding it make of it — on show only when they all are, locked when
/// any one is — and, for a group or a frame, whether it is open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row<'a> {
    pub layer: &'a Layer,
    pub owner: Option<&'a str>,
    pub depth: usize,
    pub shown: bool,
    pub locked: bool,
    pub open: bool,
}

/// Every name in `layers` and the groups under them.
fn names_in(layers: &[Layer], out: &mut Vec<String>) {
    for l in layers {
        out.push(l.name.clone());
        names_in(&l.layers, out);
    }
}

fn find<'a>(layers: &'a [Layer], id: &str) -> Option<&'a Layer> {
    layers
        .iter()
        .find_map(|l| if l.id == id { Some(l) } else { find(&l.layers, id) })
}

fn find_mut<'a>(layers: &'a mut [Layer], id: &str) -> Option<&'a mut Layer> {
    for l in layers {
        if l.id == id {
            return Some(l);
        }
        if let Some(found) = find_mut(&mut l.layers, id) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A board with every kind of nesting there is:
    ///
    /// ```text
    /// F  frame "Frame 1" (fr) ── K group ── E
    ///                         └─ D
    /// G  group ── H group ── C
    ///         └─ B
    /// A
    /// ```
    ///
    /// bottom to top `A, G, F`, with a rect on each of A to E.
    pub(crate) fn nested() -> Document {
        let rect = |id: &str, layer: &str| {
            format!(
                r#"{{ "id": "{id}", "type": "rect", "layer": "{layer}",
                      "x": 0, "y": 0, "w": 1, "h": 1,
                      "stroke": null, "fill": null, "text": null }}"#
            )
        };
        let elements = [
            rect("a", "A"),
            rect("b", "B"),
            rect("c", "C"),
            rect("d", "D"),
            rect("e", "E"),
        ]
        .join(", ");
        Document::from_json(&format!(
            r##"{{
                "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                "camera": {{ "x": 0, "y": 0, "zoom": 1 }},
                "layers": [
                    {{ "id": "A", "name": "Layer 1" }},
                    {{ "id": "G", "name": "Group 1", "kind": "group", "layers": [
                        {{ "id": "B", "name": "Layer 2" }},
                        {{ "id": "H", "name": "Group 2", "kind": "group", "layers": [
                            {{ "id": "C", "name": "Layer 3" }} ] }} ] }},
                    {{ "id": "F", "name": "Frame 1", "kind": "frame" }}
                ],
                "elements": [
                    {{ "id": "fr", "type": "frame", "layer": "F",
                       "x": 0, "y": 0, "w": 100, "h": 100, "layers": [
                        {{ "id": "D", "name": "Layer 1" }},
                        {{ "id": "K", "name": "Group 1", "kind": "group", "layers": [
                            {{ "id": "E", "name": "Layer 2" }} ] }} ] }},
                    {elements}
                ]
            }}"##
        ))
        .expect("the nested board parses")
    }

    #[test]
    fn a_layer_is_found_however_deep_and_in_whichever_stack() {
        let doc = nested();
        for id in ["A", "G", "B", "H", "C", "F", "D", "K", "E"] {
            assert_eq!(doc.layer(id).map(|l| l.id.as_str()), Some(id));
        }
        assert!(doc.layer("nope").is_none());
    }

    #[test]
    fn a_nested_layer_can_be_changed_where_it_stands() {
        let mut doc = nested();
        doc.layer_mut("C").unwrap().name = "Deep".into();
        doc.layer_mut("E").unwrap().name = "In a frame".into();
        assert_eq!(doc.layer("C").unwrap().name, "Deep");
        assert_eq!(doc.layer("E").unwrap().name, "In a frame");
        assert!(doc.layer_mut("nope").is_none());
    }

    #[test]
    fn locate_names_the_layer_holding_the_stack() {
        let doc = nested();
        assert_eq!(doc.locate("A"), Some((None, 0)));
        assert_eq!(doc.locate("F"), Some((None, 2)));
        assert_eq!(doc.locate("B"), Some((Some("G"), 0)));
        assert_eq!(doc.locate("C"), Some((Some("H"), 0)));
        // A frame's stack is held by the frame's own layer.
        assert_eq!(doc.locate("D"), Some((Some("F"), 0)));
        assert_eq!(doc.locate("E"), Some((Some("K"), 0)));
        assert_eq!(doc.locate("nope"), None);
    }


    #[test]
    fn ancestors_come_nearest_first() {
        let doc = nested();
        let ids = |id: &str| -> Vec<String> {
            doc.ancestors(id).iter().map(|l| l.id.clone()).collect()
        };
        assert_eq!(ids("C"), ["H", "G"]);
        assert_eq!(ids("E"), ["K", "F"]);
        assert!(ids("A").is_empty());
    }

    #[test]
    fn the_panel_lists_top_first_and_only_opens_what_is_open() {
        let doc = nested();
        let listed = |open: &[&str]| -> Vec<(String, usize)> {
            doc.rows(|id| open.contains(&id))
                .iter()
                .map(|r| (r.layer.id.clone(), r.depth))
                .collect()
        };
        let all_shut = listed(&[]);
        assert_eq!(
            all_shut,
            [("F".into(), 0), ("G".into(), 0), ("A".into(), 0)],
            "a shut holder hides what it holds"
        );
        let open = listed(&["F", "G", "H", "K"]);
        let want: Vec<(String, usize)> = [
            ("F", 0),
            ("K", 1),
            ("E", 2),
            ("D", 1),
            ("G", 0),
            ("H", 1),
            ("C", 2),
            ("B", 1),
            ("A", 0),
        ]
        .iter()
        .map(|(id, d)| ((*id).to_owned(), *d))
        .collect();
        assert_eq!(open, want);
        // Every row knows the stack it stands in, and whether it is open.
        let rows = doc.rows(|_| true);
        let c = rows.iter().find(|r| r.layer.id == "C").unwrap();
        assert_eq!(c.owner, Some("H"));
        let d = rows.iter().find(|r| r.layer.id == "D").unwrap();
        assert_eq!(d.owner, Some("F"));
        assert!(rows.iter().find(|r| r.layer.id == "G").unwrap().open);
        assert!(!c.open, "a layer that holds none is never open");
    }

    #[test]
    fn a_row_is_on_show_and_open_as_everything_holding_it_says() {
        let mut doc = nested();
        doc.layer_mut("G").unwrap().visible = false;
        doc.layer_mut("F").unwrap().locked = true;
        let rows = doc.rows(|_| true);
        let row = |id: &str| *rows.iter().find(|r| r.layer.id == id).unwrap();
        assert!(!row("G").shown && !row("C").shown, "hidden with its group");
        assert!(row("C").layer.visible, "though its own eye is open");
        assert!(row("A").shown);
        assert!(row("E").locked && row("F").locked, "locked with its frame");
        assert!(!row("C").locked);
    }

    #[test]
    fn a_layer_shows_only_when_everything_holding_it_does() {
        let mut doc = nested();
        assert!(doc.shown("C"));
        doc.layer_mut("G").unwrap().visible = false;
        assert!(!doc.shown("C"), "hidden by its group's group");
        assert!(doc.shown("A"));
        doc.layer_mut("F").unwrap().visible = false;
        assert!(!doc.shown("E"), "hidden with its frame");
    }

    #[test]
    fn a_layer_is_locked_by_its_own_lock_or_its_holders() {
        let mut doc = nested();
        assert!(!doc.locked("C"));
        doc.layer_mut("H").unwrap().locked = true;
        assert!(doc.locked("C"));
        assert!(doc.locked("H"));
        assert!(!doc.locked("B"), "a sibling of the locked group is not");
        doc.layer_mut("F").unwrap().locked = true;
        assert!(doc.locked("E"), "locked with its frame");
    }

    #[test]
    fn the_frame_holding_a_layer_is_found_through_its_groups() {
        let doc = nested();
        assert_eq!(doc.frame_holding("E").map(|f| f.id.as_str()), Some("fr"));
        assert_eq!(doc.frame_holding("D").map(|f| f.id.as_str()), Some("fr"));
        assert!(doc.frame_holding("C").is_none(), "a group on the board");
        assert!(doc.frame_holding("F").is_none(), "a frame's own layer is the board's");
    }



}
