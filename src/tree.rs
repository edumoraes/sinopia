//! The layer tree: where a layer stands, what holds it, and what that
//! makes of it — whether it shows, whether it is locked, which frame cuts
//! it, and the order the panel lists it in.
//!
//! A stack is named by the layer that holds it: `None` is the board's
//! root, a group's id is its children, and a frame layer's id is the
//! stack of the frame on it. One address for all three, so the panel,
//! the editor and the CLI speak the same ids. Pure.

use crate::doc::{BlendMode, Document, Element, Frame, Kind, Layer, Tag, new_id};
use crate::geom::Affine;
use crate::graft;
use crate::select;

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
                open: holds && open(&layer.id),
                ..Row::under(layer, parent)
            };
            out.push(row);
            if row.open {
                self.rows_in(self.inner(layer), Some(&row), open, out);
            }
        }
    }

    /// The rows the panel shows under `filter`: every layer it matches,
    /// and every layer holding one, so a match is never shown out of its
    /// place. A holder opens for what matches under it and only for that:
    /// one that matches with nothing under it that does is shown shut,
    /// whatever `open` says. The empty filter is [`Document::rows`].
    pub fn rows_matching(&self, open: impl Fn(&str) -> bool, filter: &Filter) -> Vec<Row<'_>> {
        if filter.is_empty() {
            return self.rows(open);
        }
        let mut out = Vec::new();
        self.matching_in(&self.layers, None, filter, &mut out);
        out
    }

    /// `layers`, top first, onto `out` as far as `filter` keeps them;
    /// whether it kept any.
    fn matching_in<'a>(
        &'a self,
        layers: &'a [Layer],
        parent: Option<&Row<'a>>,
        filter: &Filter,
        out: &mut Vec<Row<'a>>,
    ) -> bool {
        let mut kept = false;
        for layer in layers.iter().rev() {
            let row = Row::under(layer, parent);
            let at = out.len();
            out.push(row);
            if self.matching_in(self.inner(layer), Some(&row), filter, out) {
                out[at].open = true;
            } else if !filter.matches(layer) {
                out.truncate(at);
                continue;
            }
            kept = true;
        }
        kept
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
            Kind::Raster | Kind::Vector | Kind::Text => None,
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
            Kind::Text => "Text",
            Kind::Raster | Kind::Vector => "Layer",
        };
        self.next_name(owner, word)
    }

    /// The name a new layer called by `word` takes in `owner`'s stack: one
    /// past the highest number already carried under that word anywhere
    /// in the same tree — what a shape's layer is named by its model.
    pub(crate) fn next_name(&self, owner: Option<&str>, word: &str) -> String {
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

    /// The layers of `ids` that are on the board, each once, in paint
    /// order — less any held by another of them, which goes where that
    /// one goes.
    fn movable(&self, ids: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in ids {
            let held = self.ancestors(id).iter().any(|a| ids.contains(&a.id));
            if self.layer(id).is_some() && !held && !out.contains(id) {
                out.push(id.clone());
            }
        }
        let order = self.paint_order();
        out.sort_by_key(|m| order.iter().position(|o| o == m));
        out
    }

    /// Every layer's id in paint order: the panel's, the other way up.
    fn paint_order(&self) -> Vec<String> {
        self.rows(|_| true)
            .iter()
            .rev()
            .map(|r| r.layer.id.clone())
            .collect()
    }

    /// Wraps `ids` in a new group standing where the topmost of them did,
    /// and answers its id. They keep their order inside it. None — and
    /// nothing changes — when there is nothing to group, or a frame among
    /// them, which never goes in a group.
    pub fn group_layers(&mut self, ids: &[String]) -> Option<String> {
        let moving = self.movable(ids);
        let frame = moving
            .iter()
            .any(|m| self.layer(m).is_some_and(|l| l.kind == Kind::Frame));
        let top = moving.last().filter(|_| !frame)?.clone();
        let (owner, index) = self.locate(&top).map(|(o, i)| (o.map(str::to_owned), i))?;
        if !self.can_move(&moving, owner.as_deref()) {
            return None;
        }
        let name = self.next_layer_name(owner.as_deref(), Kind::Group);
        let group = Layer {
            blend: BlendMode::PassThrough,
            ..Layer::of(&name, Kind::Group)
        };
        let id = group.id.clone();
        self.stack_mut(owner.as_deref())?.insert(index + 1, group);
        if !self.move_layers(&moving, Some(&id), 0) {
            let stack = self.stack_mut(owner.as_deref())?;
            stack.retain(|l| l.id != id);
            return None;
        }
        Some(id)
    }

    /// Lets group `id`'s layers out into the stack it stood in, where it
    /// stood and in their order, and answers their ids. The group goes;
    /// what it said of itself goes with it. None for anything but a group.
    pub fn ungroup(&mut self, id: &str) -> Option<Vec<String>> {
        if self.layer(id)?.kind != Kind::Group || self.locked(id) || self.fixed_around(id) {
            return None;
        }
        let (owner, index) = self.locate(id).map(|(o, i)| (o.map(str::to_owned), i))?;
        let stack = self.stack_mut(owner.as_deref())?;
        let group = stack.remove(index);
        let out: Vec<String> = group.layers.iter().map(|l| l.id.clone()).collect();
        for (k, layer) in group.layers.into_iter().enumerate() {
            stack.insert(index + k, layer);
        }
        self.fill_empty_stacks();
        Some(out)
    }

    /// A copy of every layer of `ids` — with everything under it and on
    /// it — standing right above its original, and their ids. Every id is
    /// minted anew; a copy is named for what it copies. None goes into a
    /// stack that is fixed.
    pub fn duplicate_layers(&mut self, ids: &[String]) -> Vec<String> {
        let mut made = Vec::new();
        for id in self.movable(ids) {
            if self.fixed_around(&id) {
                continue;
            }
            let Some(layer) = self.layer(&id).cloned() else {
                continue;
            };
            let (copy, elements) = self.copied(&layer);
            let Some((owner, index)) = self.locate(&id).map(|(o, i)| (o.map(str::to_owned), i))
            else {
                continue;
            };
            made.push(copy.id.clone());
            if let Some(stack) = self.stack_mut(owner.as_deref()) {
                stack.insert(index + 1, copy);
            }
            self.elements.extend(elements);
        }
        made
    }

    /// `layer`, everything under it and every element standing on any of
    /// it — a frame's own stack included — with fresh ids throughout,
    /// ready to stand beside the original.
    pub(crate) fn copied(&self, layer: &Layer) -> (Layer, Vec<Element>) {
        let pairs: Vec<(String, String)> = self
            .subtree(layer)
            .into_iter()
            .map(|id| (id, new_id()))
            .collect();
        let minted = |id: &str| pairs.iter().find(|(a, _)| a == id).map(|(_, b)| b.clone());
        let mut copy = layer.clone();
        copy.name = format!("{} copy", layer.name);
        renamed(std::slice::from_mut(&mut copy), &minted);
        let mut elements = Vec::new();
        for el in &self.elements {
            let Some(to) = minted(el.layer()) else {
                continue;
            };
            let mut el = el.clone();
            el.set_layer(&to);
            el.set_id(&new_id());
            if let Element::Frame(f) = &mut el {
                renamed(&mut f.layers, &minted);
                // A copy is not a slide of the deck it was copied from.
                f.next = None;
            }
            elements.push(el);
        }
        (copy, elements)
    }

    /// The clip of `ids`: a board holding them — each one no other of
    /// them holds — on its root in paint order, with everything under
    /// them and every element standing on any of it. Its ids are this
    /// board's own, and [`Document::paste`] mints them anew on the way
    /// back in. None when there is nothing there to take.
    pub fn clip(&self, ids: &[String]) -> Option<Document> {
        let taken = self.movable(ids);
        if taken.is_empty() {
            return None;
        }
        let mut layers = Vec::new();
        let mut under = Vec::new();
        for id in &taken {
            let layer = self.layer(id)?;
            under.extend(self.subtree(layer));
            layers.push(layer.clone());
        }
        let elements = self
            .elements
            .iter()
            .filter(|el| under.iter().any(|id| id == el.layer()))
            .cloned()
            .collect();
        Some(Document {
            layers,
            elements,
            ..Document::new(&self.title)
        })
    }

    /// Plants `clip` above layer `above`, every element moved by `by` and
    /// every id minted anew, so one clip pasted twice is two things. It
    /// lands in `above`'s stack, or the nearest one up that can take it:
    /// a locked holder's takes nothing, and a frame goes on the board's
    /// root or nowhere. Answers the layers planted there, bottom to top —
    /// none, and nothing changed, when an element would land past the
    /// numbers a board can hold.
    pub fn paste(&mut self, clip: &Document, above: &str, by: &Affine) -> Vec<String> {
        let pairs: Vec<(String, String)> = clip
            .layers
            .iter()
            .flat_map(|l| clip.subtree(l))
            .map(|id| (id, new_id()))
            .collect();
        let minted = |id: &str| pairs.iter().find(|(a, _)| a == id).map(|(_, b)| b.clone());
        let mut elements = Vec::new();
        for el in &clip.elements {
            let Some(to) = minted(el.layer()) else {
                continue;
            };
            let mut el = el.clone();
            el.set_layer(&to);
            el.set_id(&new_id());
            if let Element::Frame(f) = &mut el {
                renamed(&mut f.layers, &minted);
                // What is pasted is new: it joins no sequence until it
                // is linked into one.
                f.next = None;
            }
            select::transform(&mut el, by);
            if !graft::placed(&el) {
                return Vec::new();
            }
            elements.push(el);
        }
        let mut layers = clip.layers.clone();
        renamed(&mut layers, &minted);
        let framed = layers.iter().any(|l| l.kind == Kind::Frame);
        let (owner, index) = self.landing(above, framed);
        let planted: Vec<String> = layers.iter().map(|l| l.id.clone()).collect();
        let Some(stack) = self.stack_mut(owner.as_deref()) else {
            return Vec::new();
        };
        let index = index.min(stack.len());
        stack.splice(index..index, layers);
        self.elements.extend(elements);
        planted
    }

    /// Where a paste above `above` lands: just over it in its stack, or
    /// over the holder of a stack that cannot take it — a locked one, or
    /// any but the root for a clip that holds a frame.
    fn landing(&self, above: &str, framed: bool) -> (Option<String>, usize) {
        let mut at = above.to_owned();
        loop {
            let Some((owner, index)) = self.locate(&at) else {
                return (None, self.layers.len());
            };
            match owner {
                Some(o) if framed || self.fixed(Some(o)) => at = o.to_owned(),
                _ => return (owner.map(str::to_owned), index + 1),
            }
        }
    }

    /// Moves `ids` within their own stacks, each stack on its own:
    /// Photoshop's Arrange. True when anything moved.
    pub fn arrange(&mut self, ids: &[String], how: Arrange) -> bool {
        let moving = self.movable(ids);
        let mut owners: Vec<Option<String>> = Vec::new();
        for m in &moving {
            let owner = self.locate(m).map(|(o, _)| o.map(str::to_owned));
            if let Some(owner) = owner
                && !owners.contains(&owner)
            {
                owners.push(owner);
            }
        }
        let mut changed = false;
        for owner in owners {
            if self.fixed(owner.as_deref()) {
                continue;
            }
            let Some(stack) = self.stack_mut(owner.as_deref()) else {
                continue;
            };
            let before: Vec<String> = stack.iter().map(|l| l.id.clone()).collect();
            let picked = |l: &Layer| moving.contains(&l.id);
            match how {
                Arrange::Front | Arrange::Back => {
                    let (mut up, mut rest): (Vec<Layer>, Vec<Layer>) =
                        stack.drain(..).partition(|l| picked(l));
                    if how == Arrange::Front {
                        rest.append(&mut up);
                        *stack = rest;
                    } else {
                        up.append(&mut rest);
                        *stack = up;
                    }
                }
                // Top down, so a run of picked layers climbs as one.
                Arrange::Forward => {
                    for i in (0..stack.len().saturating_sub(1)).rev() {
                        if picked(&stack[i]) && !picked(&stack[i + 1]) {
                            stack.swap(i, i + 1);
                        }
                    }
                }
                Arrange::Backward => {
                    for i in 1..stack.len() {
                        if picked(&stack[i]) && !picked(&stack[i - 1]) {
                            stack.swap(i - 1, i);
                        }
                    }
                }
            }
            changed |= stack.iter().map(|l| l.id.as_str()).ne(before.iter().map(String::as_str));
        }
        changed
    }

    /// The stack and the index a [`Place`] names, as the stacks stand:
    /// on top of what a group or a frame holds, or right above or under a
    /// layer in its own stack. None for a place that is not there.
    pub fn place(&self, place: &Place) -> Option<(Option<&str>, usize)> {
        match place {
            Place::Into(id) => {
                let holder = self.layer(id)?;
                matches!(holder.kind, Kind::Group | Kind::Frame)
                    .then(|| (Some(holder.id.as_str()), self.inner(holder).len()))
            }
            Place::Above(id) => self.locate(id).map(|(o, i)| (o, i + 1)),
            Place::Below(id) => self.locate(id),
        }
    }

    /// Whether the stack `owner` holds may not change: a locked group's or
    /// frame's — or one held by a locked one. Nothing goes into it, comes
    /// out of it or is reordered in it. The board's root is never fixed.
    pub fn fixed(&self, owner: Option<&str>) -> bool {
        owner.is_some_and(|o| self.locked(o))
    }

    /// The stack `id` stands in may not change.
    fn fixed_around(&self, id: &str) -> bool {
        self.locate(id).is_some_and(|(owner, _)| self.fixed(owner))
    }

    /// Whether `ids` may go into the stack `owner` holds: it is a stack,
    /// neither it nor any stack they would leave is fixed, none of them
    /// would go inside itself, and a frame would not leave the board's
    /// root.
    pub fn can_move(&self, ids: &[String], owner: Option<&str>) -> bool {
        if self.fixed(owner) || ids.iter().any(|m| self.fixed_around(m)) {
            return false;
        }
        let Some(o) = owner else {
            return true;
        };
        let Some(holder) = self.layer(o) else {
            return false;
        };
        if !matches!(holder.kind, Kind::Group | Kind::Frame) {
            return false;
        }
        let around: Vec<&str> = self.ancestors(o).iter().map(|a| a.id.as_str()).collect();
        !ids.iter().any(|m| {
            m == o
                || around.contains(&m.as_str())
                || self.layer(m).is_some_and(|l| l.kind == Kind::Frame)
        })
    }

    /// Moves `ids` — each with everything under it — into the stack
    /// `owner` holds, at `index` as that stack stands before the move:
    /// the block lands where the layer now at `index` is, under it, or on
    /// top past the end. They keep their paint order, and a layer that
    /// goes with something holding it moves once, with it. Refused, with
    /// nothing moved, when a layer would go inside itself, when a frame
    /// would leave the board's root, or when nothing would change. What
    /// the move leaves empty the board or a frame gets a fresh layer in;
    /// a group may stand empty.
    pub fn move_layers(&mut self, ids: &[String], owner: Option<&str>, index: usize) -> bool {
        let moving = self.movable(ids);
        if moving.is_empty() || !self.can_move(&moving, owner) {
            return false;
        }
        // What the block lands under, found before anything moves: the
        // first layer at or past `index` that is not itself moving.
        let anchor: Option<String> = self
            .stack(owner)
            .iter()
            .skip(index)
            .map(|l| l.id.clone())
            .find(|id| !moving.contains(id));
        let before = self.stacks();
        let mut block: Vec<Layer> = Vec::new();
        for m in &moving {
            let Some((from, i)) = self.locate(m).map(|(o, i)| (o.map(str::to_owned), i)) else {
                continue;
            };
            if let Some(stack) = self.stack_mut(from.as_deref()) {
                block.push(stack.remove(i));
            }
        }
        let Some(stack) = self.stack_mut(owner) else {
            return false;
        };
        let at = anchor
            .and_then(|a| stack.iter().position(|l| l.id == a))
            .unwrap_or(stack.len());
        for (k, layer) in block.into_iter().enumerate() {
            stack.insert(at + k, layer);
        }
        self.fill_empty_stacks();
        self.stacks() != before
    }

    /// The board's stack and every frame's, as they stand.
    fn stacks(&self) -> Vec<Vec<Layer>> {
        let mut out = vec![self.layers.clone()];
        out.extend(self.elements.iter().filter_map(|el| match el {
            Element::Frame(f) => Some(f.layers.clone()),
            _ => None,
        }));
        out
    }

    /// Gives the board and every frame a fresh layer where they stand
    /// empty, as the parse would: neither is ever without one.
    pub(crate) fn fill_empty_stacks(&mut self) {
        if self.layers.is_empty() {
            self.layers.push(Layer::new("Layer 1"));
        }
        for el in &mut self.elements {
            if let Element::Frame(f) = el
                && f.layers.is_empty()
            {
                f.layers.push(Layer::new("Layer 1"));
            }
        }
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

/// How [`Document::arrange`] moves layers within their own stacks: to
/// the top, a step up, a step down, to the bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrange {
    Front,
    Forward,
    Backward,
    Back,
}

/// A place in the tree, told by a row: where a layer let go of over the
/// panel lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Inside this group or frame, on top of what it holds.
    Into(String),
    /// Right above this layer, in its own stack.
    Above(String),
    /// Right under it.
    Below(String),
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

impl<'a> Row<'a> {
    /// `layer`'s row under `parent`'s — none on the board's root — shut.
    fn under(layer: &'a Layer, parent: Option<&Row<'a>>) -> Row<'a> {
        Row {
            layer,
            owner: parent.map(|p| p.layer.id.as_str()),
            depth: parent.map_or(0, |p| p.depth + 1),
            shown: parent.is_none_or(|p| p.shown) && layer.visible,
            locked: parent.is_some_and(|p| p.locked) || layer.locked,
            open: false,
        }
    }
}

/// What the panel narrows the tree to: layers whose name holds `name`,
/// whatever its case and the space round it, of one of `kinds`, wearing
/// one of `tags`. A part left empty narrows nothing, so the empty filter
/// is the whole tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    pub name: String,
    pub kinds: Vec<Kind>,
    pub tags: Vec<Tag>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty() && self.kinds.is_empty() && self.tags.is_empty()
    }

    pub fn matches(&self, layer: &Layer) -> bool {
        let name = self.name.trim().to_lowercase();
        (name.is_empty() || layer.name.to_lowercase().contains(&name))
            && (self.kinds.is_empty() || self.kinds.contains(&layer.kind))
            && (self.tags.is_empty() || self.tags.contains(&layer.color))
    }

    pub fn toggle_kind(&mut self, kind: Kind) {
        toggle(&mut self.kinds, kind);
    }

    pub fn toggle_tag(&mut self, tag: Tag) {
        toggle(&mut self.tags, tag);
    }
}

/// Takes `v` out of `list` when it is there, and puts it in when not.
fn toggle<T: PartialEq>(list: &mut Vec<T>, v: T) {
    match list.iter().position(|x| *x == v) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(v),
    }
}

/// `layers` and every group's under them, given the ids `minted` says —
/// and where their links lead.
fn renamed(layers: &mut [Layer], minted: &impl Fn(&str) -> Option<String>) {
    for l in layers {
        if let Some(to) = minted(&l.id) {
            l.id = to;
        }
        // A link inside what is copied comes with it, to the copy of
        // where it led; one out of it would be a second way into a stop
        // that already has one, and a copy joins no deck it did not
        // bring.
        l.next = l.next.as_deref().and_then(minted);
        renamed(&mut l.layers, minted);
    }
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

    fn stack_ids(doc: &Document, owner: Option<&str>) -> Vec<String> {
        doc.stack(owner).iter().map(|l| l.id.clone()).collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn layers_move_together_keeping_their_order_and_land_where_asked() {
        let mut doc = nested();
        // A from the root and C from deep in G, into G at the bottom:
        // they arrive in paint order, A under C, and take index 0.
        assert!(doc.move_layers(&ids(&["C", "A"]), Some("G"), 0));
        assert_eq!(stack_ids(&doc, Some("G")), ["A", "C", "B", "H"]);
        assert!(doc.stack(Some("H")).is_empty(), "a group may stand empty");
        assert_eq!(stack_ids(&doc, None), ["G", "F"]);
        // The objects went with their layers, untouched.
        assert!(doc.elements.iter().any(|el| el.id() == "a" && el.layer() == "A"));
        Document::from_json(&doc.to_json().unwrap()).expect("still a board");
    }

    #[test]
    fn an_index_counts_the_stack_as_it_stands_before_the_move() {
        let mut doc = nested();
        // G to the top of the root: index 3 is past F, the end.
        assert!(doc.move_layers(&ids(&["G"]), None, 3));
        assert_eq!(stack_ids(&doc, None), ["A", "F", "G"]);
        // And back down under F: index 1 in the stack as it stands, so
        // between A and F.
        assert!(doc.move_layers(&ids(&["G"]), None, 1));
        assert_eq!(stack_ids(&doc, None), ["A", "G", "F"]);
        // Nowhere to go is no move.
        assert!(!doc.move_layers(&ids(&["G"]), None, 1));
        assert!(!doc.move_layers(&ids(&["G"]), None, 2), "right above itself is where it is");
    }

    #[test]
    fn a_group_does_not_go_inside_itself() {
        let mut doc = nested();
        assert!(!doc.move_layers(&ids(&["G"]), Some("H"), 0));
        assert!(!doc.move_layers(&ids(&["G"]), Some("G"), 0));
        assert_eq!(stack_ids(&doc, None), ["A", "G", "F"], "nothing moved");
    }

    #[test]
    fn a_frame_moves_only_on_the_boards_root() {
        let mut doc = nested();
        assert!(!doc.move_layers(&ids(&["F"]), Some("G"), 0));
        assert!(!doc.move_layers(&ids(&["F", "A"]), Some("G"), 0), "not even in company");
        assert!(doc.move_layers(&ids(&["F"]), None, 0));
        assert_eq!(stack_ids(&doc, None), ["F", "A", "G"]);
    }

    #[test]
    fn what_leaves_a_frame_empty_leaves_it_a_fresh_layer() {
        let mut doc = nested();
        assert!(doc.move_layers(&ids(&["D", "K"]), None, 0));
        let stack = doc.stack(Some("F"));
        assert_eq!(stack.len(), 1, "a frame's stack is never empty");
        assert_eq!(stack[0].kind, Kind::Raster);
        assert_eq!(stack_ids(&doc, None)[..2], ["D", "K"], "and they left for the root");
        assert_eq!(doc.context("E"), None, "E came out of the frame with its group");
    }

    #[test]
    fn a_layer_moved_with_what_holds_it_moves_once() {
        let mut doc = nested();
        // G and C together: C is in G, and goes where G goes.
        assert!(doc.move_layers(&ids(&["G", "C"]), None, 0));
        assert_eq!(stack_ids(&doc, None), ["G", "A", "F"]);
        assert_eq!(doc.locate("C"), Some((Some("H"), 0)), "still in its own group");
    }

    #[test]
    fn a_place_is_a_stack_and_an_index_in_it() {
        let doc = nested();
        let at = |p: Place| doc.place(&p).map(|(o, i)| (o.map(str::to_owned), i));
        assert_eq!(at(Place::Into("G".into())), Some((Some("G".into()), 2)), "on top of it");
        assert_eq!(at(Place::Above("B".into())), Some((Some("G".into()), 1)));
        assert_eq!(at(Place::Below("B".into())), Some((Some("G".into()), 0)));
        assert_eq!(at(Place::Above("F".into())), Some((None, 3)));
        assert_eq!(at(Place::Into("A".into())), None, "a layer holds none");
        assert_eq!(at(Place::Above("nobody".into())), None);
    }

    #[test]
    fn whether_layers_may_go_somewhere_is_asked_before_they_do() {
        let doc = nested();
        assert!(doc.can_move(&ids(&["A"]), Some("G")));
        assert!(doc.can_move(&ids(&["A"]), Some("F")), "into a frame's stack");
        assert!(!doc.can_move(&ids(&["G"]), Some("H")), "not inside itself");
        assert!(!doc.can_move(&ids(&["F"]), Some("G")), "a frame stays on the root");
        assert!(!doc.can_move(&ids(&["A"]), Some("A")), "a layer holds none");
        assert!(doc.can_move(&ids(&["F"]), None));
    }

    #[test]
    fn grouping_wraps_the_layers_where_the_topmost_stood() {
        let mut doc = nested();
        let g = doc.group_layers(&ids(&["A", "C"])).expect("a group");
        // Where C stood, the topmost of the two: inside H.
        assert_eq!(doc.locate(&g), Some((Some("H"), 0)));
        let group = doc.layer(&g).unwrap();
        assert_eq!(group.kind, Kind::Group);
        assert_eq!(group.blend, BlendMode::PassThrough, "a new group passes through");
        let kids: Vec<&str> = group.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(kids, ["A", "C"], "in paint order");
        assert_eq!(group.name, "Group 3");
        Document::from_json(&doc.to_json().unwrap()).expect("still a board");
    }

    #[test]
    fn a_frame_is_never_grouped() {
        let mut doc = nested();
        assert!(doc.group_layers(&ids(&["F"])).is_none());
        assert!(doc.group_layers(&ids(&["A", "F"])).is_none(), "not even in company");
        assert!(doc.group_layers(&ids(&["nobody"])).is_none());
        assert_eq!(stack_ids(&doc, None), ["A", "G", "F"], "and nothing moved");
    }

    #[test]
    fn ungrouping_lets_the_layers_out_where_the_group_stood() {
        let mut doc = nested();
        let out = doc.ungroup("G").expect("G is a group");
        assert_eq!(out, ["B", "H"]);
        assert_eq!(stack_ids(&doc, None), ["A", "B", "H", "F"]);
        assert!(doc.layer("G").is_none());
        assert!(doc.ungroup("A").is_none(), "a layer is no group");
        assert!(doc.ungroup("F").is_none(), "and neither is a frame");
    }

    #[test]
    fn a_duplicate_stands_right_above_its_original_with_ids_of_its_own() {
        let mut doc = nested();
        let made = doc.duplicate_layers(&ids(&["G", "A"]));
        assert_eq!(made.len(), 2, "in paint order: A's, then G's");
        let root = stack_ids(&doc, None);
        assert_eq!(root.len(), 5);
        assert_eq!(root[0], "A");
        assert_eq!(root[1], made[0], "A's copy right above A");
        assert_eq!(root[2], "G");
        assert_eq!(root[3], made[1], "G's copy right above G");
        let copy = doc.layer(&made[1]).unwrap();
        assert_eq!(copy.name, format!("{} copy", doc.layer("G").unwrap().name));
        // Everything under it is new, and so is what stands on it.
        let under = doc.subtree(copy);
        assert_eq!(under.len(), 4, "the group, B, H and C");
        for id in &under {
            assert!(!["G", "B", "H", "C"].contains(&id.as_str()), "{id} is minted anew");
        }
        let on: Vec<&Element> = doc.elements.iter().filter(|el| under.iter().any(|u| u == el.layer())).collect();
        assert_eq!(on.len(), 2, "b and c, copied");
        assert!(on.iter().all(|el| el.id() != "b" && el.id() != "c"));
        Document::from_json(&doc.to_json().unwrap()).expect("still a board");
    }

    #[test]
    fn a_duplicated_frame_carries_a_frame_and_a_stack_of_its_own() {
        let mut doc = nested();
        let made = doc.duplicate_layers(&ids(&["F"]));
        let copy = doc.frame_on(&made[0]).expect("a frame on the copy");
        assert_ne!(copy.id, "fr");
        assert_eq!(copy.layers.len(), 2);
        assert!(copy.layers.iter().all(|l| !["D", "K"].contains(&l.id.as_str())));
        Document::from_json(&doc.to_json().unwrap()).expect("still a board");
    }

    #[test]
    fn a_copy_keeps_the_links_inside_it_and_lets_go_of_the_ones_out() {
        let mut doc = nested();
        // Inside group G: B leads to C (deeper in it) and C to A, out of it.
        doc.layer_mut("B").unwrap().next = Some("C".into());
        doc.layer_mut("C").unwrap().next = Some("A".into());
        // Inside frame F: the frame leads into its own group K, and K to D.
        doc.layer_mut("F").unwrap().next = Some("K".into());
        doc.layer_mut("K").unwrap().next = Some("D".into());
        let made = doc.duplicate_layers(&ids(&["G", "F"]));
        assert_eq!(made.len(), 2);
        let copy = |doc: &Document, of: &str| {
            let original = doc.layer(of).unwrap();
            let name = format!("{} copy", original.name);
            let made = made.iter().find(|id| doc.layer(id).is_some_and(|l| l.name == name)).unwrap();
            doc.layer(made).unwrap().clone()
        };
        let g = copy(&doc, "G");
        let (b, h) = (&g.layers[0], &g.layers[1]);
        let c = &h.layers[0];
        assert_eq!(b.next.as_ref(), Some(&c.id), "B's copy leads to C's copy");
        assert_eq!(c.next, None, "a link out of the copy is let go of");
        let f = copy(&doc, "F");
        let inner = doc.inner(&f).to_vec();
        let (d, k) = (&inner[0], &inner[1]);
        assert_eq!(f.next.as_ref(), Some(&k.id), "the frame's copy leads into its own group");
        assert_eq!(k.next.as_ref(), Some(&d.id));
        // The originals are as they were.
        assert_eq!(doc.layer("B").unwrap().next.as_deref(), Some("C"));
        assert_eq!(doc.layer("C").unwrap().next.as_deref(), Some("A"));
        assert_eq!(doc.layer("F").unwrap().next.as_deref(), Some("K"));
    }

    #[test]
    fn a_paste_keeps_the_links_inside_the_clip_and_lets_go_of_the_ones_out() {
        let mut doc = nested();
        doc.layer_mut("B").unwrap().next = Some("C".into());
        doc.layer_mut("C").unwrap().next = Some("A".into());
        let clip = doc.clip(&ids(&["G"])).unwrap();
        let planted = doc.paste(&clip, "A", &Affine::IDENTITY);
        let g = doc.layer(&planted[0]).unwrap();
        let (b, c) = (&g.layers[0], &g.layers[1].layers[0]);
        assert_ne!(b.id, "B", "minted anew");
        assert_eq!(b.next.as_ref(), Some(&c.id));
        assert_eq!(c.next, None);
    }

    #[test]
    fn a_copy_or_a_paste_of_a_linked_frame_joins_no_deck() {
        let mut doc = nested();
        doc.frame_mut("fr").unwrap().next = Some("elsewhere".into());
        let made = doc.duplicate_layers(&ids(&["F"]));
        assert_eq!(doc.frame_on(&made[0]).unwrap().next, None, "a duplicate");
        let clip = doc.clip(&ids(&["F"])).unwrap();
        let planted = doc.paste(&clip, "A", &Affine::IDENTITY);
        assert_eq!(doc.frame_on(&planted[0]).unwrap().next, None, "a paste");
        assert_eq!(
            doc.frame("fr").unwrap().next.as_deref(),
            Some("elsewhere"),
            "the original keeps its link"
        );
    }

    #[test]
    fn arranging_moves_the_layers_within_their_own_stacks() {
        let mut doc = nested();
        assert!(doc.arrange(&ids(&["A"]), Arrange::Forward));
        assert_eq!(stack_ids(&doc, None), ["G", "A", "F"]);
        assert!(doc.arrange(&ids(&["A"]), Arrange::Front));
        assert_eq!(stack_ids(&doc, None), ["G", "F", "A"]);
        assert!(!doc.arrange(&ids(&["A"]), Arrange::Forward), "already on top");
        assert!(doc.arrange(&ids(&["A", "F"]), Arrange::Back));
        assert_eq!(stack_ids(&doc, None), ["F", "A", "G"], "in their order");
        // Two stacks at once: each moves in its own.
        assert!(doc.arrange(&ids(&["B", "D"]), Arrange::Front));
        assert_eq!(stack_ids(&doc, Some("G")), ["H", "B"]);
        assert_eq!(stack_ids(&doc, Some("F")), ["K", "D"]);
        assert!(doc.arrange(&ids(&["B"]), Arrange::Backward));
        assert_eq!(stack_ids(&doc, Some("G")), ["B", "H"]);
    }

    #[test]
    fn a_locked_holders_stack_does_not_change() {
        let mut doc = nested();
        doc.layer_mut("G").unwrap().locked = true;
        let before = doc.clone();
        // Nothing in, nothing out, nothing reordered, however deep.
        assert!(!doc.can_move(&ids(&["A"]), Some("G")), "nothing goes in");
        assert!(!doc.can_move(&ids(&["A"]), Some("H")), "nor into what it holds");
        assert!(!doc.can_move(&ids(&["B"]), None), "nothing comes out");
        assert!(!doc.move_layers(&ids(&["B"]), None, 0));
        assert!(doc.group_layers(&ids(&["B", "H"])).is_none());
        assert!(doc.ungroup("H").is_none());
        assert!(doc.duplicate_layers(&ids(&["C"])).is_empty());
        assert!(!doc.arrange(&ids(&["B"]), Arrange::Front));
        assert_eq!(doc, before, "and nothing changed");
        // The locked group itself still moves in a stack that is free.
        assert!(doc.arrange(&ids(&["G"]), Arrange::Front));
        assert!(doc.ungroup("G").is_none(), "but it does not come apart");
    }

    #[test]
    fn the_frame_holding_a_layer_is_found_through_its_groups() {
        let doc = nested();
        assert_eq!(doc.frame_holding("E").map(|f| f.id.as_str()), Some("fr"));
        assert_eq!(doc.frame_holding("D").map(|f| f.id.as_str()), Some("fr"));
        assert!(doc.frame_holding("C").is_none(), "a group on the board");
        assert!(doc.frame_holding("F").is_none(), "a frame's own layer is the board's");
    }

    fn row_ids(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.layer.id.clone()).collect()
    }

    #[test]
    fn an_empty_filter_narrows_nothing() {
        let doc = nested();
        let open = |id: &str| id == "G";
        assert_eq!(
            row_ids(&doc.rows_matching(open, &Filter::default())),
            row_ids(&doc.rows(open))
        );
        assert!(Filter::default().is_empty());
        let spaces = Filter {
            name: "  ".into(),
            ..Filter::default()
        };
        assert!(spaces.is_empty(), "space is no name");
    }

    #[test]
    fn a_name_matches_whatever_its_case_and_brings_its_holders_open() {
        let doc = nested();
        let filter = Filter {
            name: " LAYER 3 ".into(),
            ..Filter::default()
        };
        // Everything shut: the match opens the holders it needs.
        let rows = doc.rows_matching(|_| false, &filter);
        assert_eq!(row_ids(&rows), ["G", "H", "C"]);
        assert!(rows[0].open && rows[1].open, "open down to the match");
        assert_eq!(rows.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn a_holder_that_matches_alone_is_shown_shut() {
        let doc = nested();
        let filter = Filter {
            name: "group 2".into(),
            ..Filter::default()
        };
        // Even open, it shows nothing it holds that does not match.
        let rows = doc.rows_matching(|_| true, &filter);
        assert_eq!(row_ids(&rows), ["G", "H"]);
        assert!(rows[0].open);
        assert!(!rows[1].open, "nothing under it matches");
    }

    #[test]
    fn kinds_narrow_and_a_frame_opens_for_what_it_holds() {
        let doc = nested();
        let filter = Filter {
            kinds: vec![Kind::Group],
            ..Filter::default()
        };
        let rows = doc.rows_matching(|_| false, &filter);
        assert_eq!(row_ids(&rows), ["F", "K", "G", "H"]);
        assert!(rows[0].open, "the frame holds a group");
        assert!(!rows[1].open, "K holds only a layer");
        assert!(rows[2].open, "G matches, and so does what it holds");
        let frames = Filter {
            kinds: vec![Kind::Frame],
            ..Filter::default()
        };
        assert_eq!(row_ids(&doc.rows_matching(|_| true, &frames)), ["F"]);
    }

    #[test]
    fn tags_kinds_and_a_name_narrow_together() {
        let mut doc = nested();
        doc.layer_mut("C").unwrap().color = Tag::Red;
        doc.layer_mut("E").unwrap().color = Tag::Red;
        doc.layer_mut("K").unwrap().color = Tag::Red;
        let red = Filter {
            tags: vec![Tag::Red],
            ..Filter::default()
        };
        assert_eq!(
            row_ids(&doc.rows_matching(|_| false, &red)),
            ["F", "K", "E", "G", "H", "C"]
        );
        let narrow = Filter {
            name: "2".into(),
            kinds: vec![Kind::Raster],
            tags: vec![Tag::Red, Tag::Blue],
        };
        assert_eq!(row_ids(&doc.rows_matching(|_| false, &narrow)), ["F", "K", "E"]);
        let none = Filter {
            name: "nothing like it".into(),
            ..Filter::default()
        };
        assert!(doc.rows_matching(|_| true, &none).is_empty());
    }

    #[test]
    fn a_filters_toggles_go_in_and_out() {
        let mut f = Filter::default();
        f.toggle_kind(Kind::Vector);
        f.toggle_tag(Tag::Green);
        assert_eq!((f.kinds.clone(), f.tags.clone()), (vec![Kind::Vector], vec![Tag::Green]));
        assert!(!f.is_empty());
        f.toggle_kind(Kind::Vector);
        f.toggle_tag(Tag::Green);
        assert!(f.is_empty());
    }

    fn element_ids(doc: &Document) -> Vec<String> {
        let mut out: Vec<String> = doc.elements.iter().map(|e| e.id().to_owned()).collect();
        out.sort();
        out
    }

    #[test]
    fn a_clip_holds_the_picked_layers_with_what_is_under_and_on_them() {
        let doc = nested();
        let clip = doc.clip(&ids(&["H", "D", "B", "C"])).expect("something to take");
        // C goes with H, which holds it; the rest in paint order.
        assert_eq!(stack_ids(&clip, None), ["B", "H", "D"]);
        assert_eq!(stack_ids(&clip, Some("H")), ["C"]);
        assert_eq!(element_ids(&clip), ["b", "c", "d"]);
        // What leaves is a board the parse takes back.
        let back = Document::from_json(&clip.to_json().unwrap()).expect("a clip parses");
        assert_eq!(stack_ids(&back, None), ["B", "H", "D"]);
        assert!(doc.clip(&ids(&["nope"])).is_none());
    }

    #[test]
    fn a_clip_of_a_frame_carries_the_frame_and_its_stack() {
        let doc = nested();
        let clip = doc.clip(&ids(&["F"])).unwrap();
        assert_eq!(stack_ids(&clip, None), ["F"]);
        assert_eq!(stack_ids(&clip, Some("F")), ["D", "K"]);
        assert_eq!(element_ids(&clip), ["d", "e", "fr"]);
    }

    #[test]
    fn a_paste_mints_every_id_anew_and_lands_above() {
        let mut doc = nested();
        let clip = doc.clip(&ids(&["G"])).unwrap();
        let before = doc.clone();
        let planted = doc.paste(&clip, "A", &Affine::IDENTITY);
        assert_eq!(planted.len(), 1);
        let new = &planted[0];
        assert_eq!(stack_ids(&doc, None), ["A", new.as_str(), "G", "F"]);
        let g = doc.layer(new).unwrap();
        assert_eq!(g.name, "Group 1", "a paste keeps the names");
        let inner: Vec<String> = doc.subtree(g);
        assert_eq!(inner.len(), 4);
        assert!(inner.iter().all(|id| before.layer(id).is_none()), "every layer id is new");
        let on: Vec<&Element> = doc.elements.iter().filter(|e| inner.iter().any(|i| i == e.layer())).collect();
        assert_eq!(on.len(), 2, "b and c, copied");
        assert!(on.iter().all(|e| before.elements.iter().all(|b| b.id() != e.id())));
        // The originals are where they were, and a second paste collides
        // with neither.
        assert_eq!(doc.layer("G"), before.layer("G"));
        let again = doc.paste(&clip, "A", &Affine::IDENTITY);
        assert_ne!(again, planted);
        assert!(Document::from_json(&doc.to_json().unwrap()).is_ok(), "the board still parses");
    }

    #[test]
    fn a_paste_is_moved_and_refused_whole_past_the_numbers_a_board_holds() {
        let mut doc = nested();
        let clip = doc.clip(&ids(&["A"])).unwrap();
        let planted = doc.paste(&clip, "A", &Affine::translate(10.0, 5.0));
        let el = doc.elements.iter().find(|e| e.layer() == planted[0]).unwrap();
        let Element::Rect(r) = el else { panic!("a rect") };
        assert_eq!((r.x, r.y), (10.0, 5.0));
        let before = doc.clone();
        let mut far = clip.clone();
        if let Element::Rect(r) = &mut far.elements[0] {
            r.x = f64::MAX;
        }
        let mut doc = before.clone();
        assert!(doc.paste(&far, "A", &Affine::translate(f64::MAX, 0.0)).is_empty());
        assert_eq!(doc, before, "nothing of it lands");
    }

    #[test]
    fn a_paste_climbs_out_of_a_stack_that_cannot_take_it() {
        // A frame never goes into a group or a frame: it lands on the
        // board's root, above the frame the active layer is in.
        let mut doc = nested();
        let frame = doc.clip(&ids(&["F"])).unwrap();
        let planted = doc.paste(&frame, "E", &Affine::IDENTITY);
        assert_eq!(stack_ids(&doc, None), ["A", "G", "F", planted[0].as_str()]);
        // A locked group takes nothing in: the paste lands above it.
        let mut doc = nested();
        doc.layer_mut("H").unwrap().locked = true;
        let clip = doc.clip(&ids(&["A"])).unwrap();
        let planted = doc.paste(&clip, "C", &Affine::IDENTITY);
        assert_eq!(stack_ids(&doc, Some("G")), ["B", "H", planted[0].as_str()]);
        doc.layer_mut("G").unwrap().locked = true;
        let planted = doc.paste(&clip, "C", &Affine::IDENTITY);
        assert_eq!(stack_ids(&doc, None), ["A", "G", planted[0].as_str(), "F"]);
    }
}
