# Undo and Redo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Ctrl+Z` puts the board back the way it was before the last change and the hand back where it was standing then; `Ctrl+Y` and `Ctrl+Shift+Z` put it forward again.

**Architecture:** A per-tab history of *resting states*. Each entry is a clone of the `Document` plus the editor's `Spot` — what was selected, the ink layer, the frame being worked in. An entry is taken in `App::apply` when a `Change` lands and nothing is still in progress, and kept only when the board actually differs. The camera is excluded at both ends: it is in the document but it is not part of a state anybody undoes to.

**Tech Stack:** Rust 2024, no new dependencies. `cargo test` (inline `#[cfg(test)] mod tests`), `cargo clippy`.

**Spec:** `docs/superpowers/specs/2026-09-04-undo-and-redo-design.md`

## Global Constraints

- Documentation, code (identifiers, comments, messages) and commits in **English**.
- Commits are **atomic** — one coherent change each — with succinct messages.
- `CLAUDE.md` is a symlink to `AGENTS.md`; edit `AGENTS.md`.
- Pure core, thin shell: `doc`, `editor` and the new `history` carry tests. `app` is the untested shell — keep logic out of it.
- Every task ends with `cargo test` green and `cargo clippy --all-targets` adding **no new warnings**. `main` carries three when this plan starts (`is_draft` never used, two `sort_by_key`), so `-D warnings` is not the gate mid-plan; a `never used` warning on something a later task consumes is expected and transient. The branch ends on zero, those three included — `cargo build`, `cargo run` and `cargo clippy` are all silent.
- The suite is at 668 tests before this plan; it only ever grows.
- No new file may be added to `src/` without a `mod` line in `src/main.rs` (alphabetical).

---

### Task 1: A board without its camera

`Document` carries the camera, and undo must not move the view. This is the one predicate that says whether two documents hold the same *board*.

**Files:**
- Modify: `src/doc.rs` (add a method to `impl Document`, near `painted`/`layer_index` around line 1010–1060; tests into the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `Document::same_board(&self, other: &Document) -> bool`

- [ ] **Step 1: Write the failing tests**

Add to `src/doc.rs`'s `mod tests`:

```rust
#[test]
fn two_documents_are_the_same_board_when_only_the_camera_differs() {
    // Panning is not work — `Change::Camera` does not even dirty a tab
    // — so a board looked at from somewhere else is the same board.
    let a = Document::new("b");
    let mut b = a.clone();
    b.camera = Camera {
        x: 900.0,
        y: -12.0,
        zoom: 3.5,
    };
    assert!(a.same_board(&b));
    assert!(b.same_board(&a));
}

#[test]
fn every_part_of_a_board_but_the_camera_tells_two_apart() {
    let base = Document::new("b");
    let mut cases: Vec<(&str, Document)> = Vec::new();

    let mut d = base.clone();
    d.schema += 1;
    cases.push(("schema", d));

    let mut d = base.clone();
    d.id = "other".into();
    cases.push(("id", d));

    let mut d = base.clone();
    d.title = "other".into();
    cases.push(("title", d));

    let mut d = base.clone();
    d.layers.push(Layer::new("Layer 2"));
    cases.push(("layers", d));

    let mut d = base.clone();
    d.layers[0].name = "renamed".into();
    cases.push(("a layer's name", d));

    let mut d = base.clone();
    d.elements.push(Element::Rect(Rect {
        id: "R1".into(),
        layer: d.layers[0].id.clone(),
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
        fill: "#000000".into(),
        rotation: 0.0,
    }));
    cases.push(("elements", d));

    for (what, other) in cases {
        assert!(
            !base.same_board(&other),
            "{what} is part of the board and must tell two apart"
        );
    }
}
```

If `Rect`'s fields differ from the above, read the struct at `src/doc.rs:164` and build the literal to match — the point of the case is only that an element was added.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test doc::tests::two_documents_are_the_same_board doc::tests::every_part_of_a_board`
Expected: FAIL — `no method named same_board found`.

- [ ] **Step 3: Write the implementation**

In `impl Document` in `src/doc.rs`:

```rust
    /// Whether `other` holds the same board. The camera is left out:
    /// where the view is looking is not the board's business — panning
    /// is not work to undo, which is why `Change::Camera` does not
    /// dirty a tab — and an undo that moved the view would make the two
    /// mean the same thing.
    ///
    /// The fields are destructured rather than listed, so a field added
    /// to a document fails to compile here until somebody has decided
    /// which side of this line it falls on.
    pub fn same_board(&self, other: &Document) -> bool {
        let Document {
            schema,
            id,
            title,
            camera: _,
            layers,
            elements,
        } = self;
        *schema == other.schema
            && *id == other.id
            && *title == other.title
            && *layers == other.layers
            && *elements == other.elements
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test doc::` — Expected: PASS, no regressions.

- [ ] **Step 5: Commit**

```bash
git add src/doc.rs
git commit -m "doc: two documents hold the same board, camera aside"
```

---

### Task 2: Where the hand was standing

The history keeps the editor's session state beside the document it was taken with. `Spot` is that state; it belongs to `editor`, because that is whose it is.

**Files:**
- Modify: `src/editor.rs` (a `Spot` struct near `Stylus` around line 184; three methods in `impl Editor`; tests into the existing `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct editor::Spot { pub selection: Vec<String>, pub layer: Option<String>, pub inside: Option<String> }` — derives `Debug, Clone, Default, PartialEq`
  - `Editor::at(&self) -> Spot`
  - `Editor::go(&mut self, spot: Spot)`
  - `Editor::busy(&self) -> bool`

- [ ] **Step 1: Write the failing tests**

Add to `src/editor.rs`'s `mod tests`:

```rust
#[test]
fn a_spot_is_what_is_selected_the_ink_layer_and_the_frame_worked_in() {
    let mut e = Editor::new();
    let doc = board();
    e.selection = vec!["a".into()];
    e.layer = Some(doc.layers[0].id.clone());
    e.inside = None;
    let spot = e.at();
    assert_eq!(spot.selection, ["a"]);
    assert_eq!(spot.layer.as_deref(), Some(doc.layers[0].id.as_str()));
    assert_eq!(spot.inside, None);
}

#[test]
fn going_to_a_spot_puts_all_three_back() {
    let mut e = Editor::new();
    e.selection = vec!["a".into()];
    e.layer = Some("L1".into());
    e.inside = Some("F1".into());
    let there = e.at();

    let mut other = Editor::new();
    other.selection = vec!["z".into(), "y".into()];
    other.go(there.clone());
    assert_eq!(other.at(), there);
}

#[test]
fn going_to_a_spot_leaves_the_hand_alone() {
    // A spot is where the work is, not what the hand is doing: the tool
    // and the keys held down are physical and belong to the window.
    let mut e = tool(Tool::Brush);
    e.hold_shift(true);
    e.go(Spot::default());
    assert_eq!(e.tool(), Tool::Brush);
    assert!(e.shift);
}

#[test]
fn an_editor_at_rest_is_not_busy() {
    assert!(!Editor::new().busy());
}

#[test]
fn every_gesture_makes_the_editor_busy() {
    let mut doc = board();
    let view = view();
    let tip = Tip::of(&brush());

    // A stroke.
    let mut e = tool(Tool::Pencil);
    let _ = e.press(Button::Left, &view, (10.0, 10.0), &mut doc, &tip);
    assert!(e.busy(), "a stroke in progress");
    e.cancel(&mut doc);
    assert!(!e.busy(), "cancelled");

    // An area being dragged out.
    let mut e = tool(Tool::Frame);
    let _ = e.press(Button::Left, &view, (10.0, 10.0), &mut doc, &tip);
    assert!(e.busy(), "an area being dragged out");

    // A navigation.
    let mut e = tool(Tool::Hand);
    let _ = e.press(Button::Left, &view, (10.0, 10.0), &mut doc, &tip);
    assert!(e.busy(), "a pan");

    // A marquee, which `is_moving` never covered.
    let mut e = tool(Tool::Select);
    let _ = e.press(Button::Left, &view, (500.0, 500.0), &mut doc, &tip);
    assert!(e.busy(), "a marquee is a drag too");
    assert!(!e.is_moving(), "and is not a move");
}
```

The helpers `board()`, `view()`, `tool(..)`, `brush()` already exist in that test module. If `Tip::of` is not the constructor there, copy whatever the neighbouring tests hand to `press`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test editor::tests::a_spot_is editor::tests::going_to_a_spot editor::tests::an_editor_at_rest editor::tests::every_gesture_makes`
Expected: FAIL — `cannot find type Spot`, `no method named at`/`go`/`busy`.

- [ ] **Step 3: Write the implementation**

Add above `pub struct Editor` in `src/editor.rs`:

```rust
/// Where the hand is standing: what is selected, the layer new ink
/// lands on, and the frame being worked in. Session state (§6.2) — it
/// never enters the document — and exactly the part of it that belongs
/// beside a board rather than to the window. The tool, the keys held
/// down and what the pen last said are not here: those are physical,
/// and a step backwards does not move a hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spot {
    /// Ids of the selected elements, in selection order.
    pub selection: Vec<String>,
    pub layer: Option<String>,
    pub inside: Option<String>,
}
```

Add to `impl Editor`:

```rust
    /// Where the hand is standing now.
    pub fn at(&self) -> Spot {
        Spot {
            selection: self.selection.clone(),
            layer: self.layer.clone(),
            inside: self.inside.clone(),
        }
    }

    /// Stands there instead. Nothing physical moves: a spot is where the
    /// work is, and the tool and the held keys belong to the window.
    pub fn go(&mut self, spot: Spot) {
        let Spot {
            selection,
            layer,
            inside,
        } = spot;
        self.selection = selection;
        self.layer = layer;
        self.inside = inside;
    }

    /// Something is in the middle of happening: a stroke, an area being
    /// dragged out, a navigation or a drag. What the document says
    /// mid-gesture is not a state anybody meant to arrive at, which is
    /// why the history does not write it down.
    ///
    /// Wider than [`Editor::is_moving`], which only ever meant a
    /// `Drag::Move` that had passed the click slop.
    pub fn busy(&self) -> bool {
        self.stroke.is_some()
            || self.framing.is_some()
            || self.nav.is_some()
            || self.drag.is_some()
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test editor::` — Expected: PASS, no regressions.

- [ ] **Step 5: Commit**

```bash
git add src/editor.rs
git commit -m "editor: the spot the hand is standing on, and whether it is busy"
```

---

### Task 3: The history

The module itself: a timeline of resting states whose last entry is the present, so undo and redo are the same move in two directions.

**Files:**
- Create: `src/history.rs`
- Modify: `src/main.rs` (add `mod history;` between `mod grid;` and `mod ipc;`)

**Interfaces:**
- Consumes: `Document::same_board` (Task 1), `editor::Spot`, `Editor::go` (Task 2).
- Produces:
  - `pub struct history::Entry` with `pub fn restore(&self, doc: &mut Document, editor: &mut Editor)`
  - `pub struct history::History` with `new(&Document, Spot) -> History`, `keep(&mut self, &Document, Spot)`, `undo(&mut self) -> Option<&Entry>`, `redo(&mut self) -> Option<&Entry>`, `can_undo(&self) -> bool`, `can_redo(&self) -> bool`

- [ ] **Step 1: Write the module with its failing tests**

Create `src/history.rs`:

```rust
//! What a tab can step back through: the states its board came to rest
//! in, and where the hand was standing at each of them.
//!
//! A step is not every change. `Change::Scene` fires on every sample of
//! a stroke and on every step of a drag — which is why the draft save is
//! debounced — so what is written down here is a change that has come to
//! **rest**: applied, with nothing still happening. `app` is what knows
//! when that is; this module only ever hears about the ones that count.
//!
//! An entry is the whole board rather than an edit that knows how to
//! invert itself. A snapshot cannot come to disagree with the model,
//! because it *is* the model: every operation that exists is undoable,
//! and so is every one written later, without a line at its call site.

use crate::doc::Document;
use crate::editor::{Editor, Spot};

/// How many resting states a tab keeps. Deep enough to hold a session's
/// worth of work, shallow enough that the oldest is not paid for
/// forever.
pub const DEPTH: usize = 100;

/// What the whole history of one tab may weigh. A board grows as it is
/// drawn on and every entry carries all of it, so the cost of a step
/// climbs with the session: a count alone is the wrong ceiling. A heavy
/// board gets fewer steps, which is the honest trade — the alternative
/// is a whiteboard that quietly eats a quarter of a gigabyte.
pub const BUDGET: usize = 64 << 20;

/// One resting state: the board, and the spot the hand was standing on
/// when it got there.
///
/// The two are kept together so that neither has to be checked against
/// the other. A spot restored beside the document it was taken with
/// names ids that document has, by construction — there is nothing to
/// reconcile and nothing to guess, which is why undoing a delete brings
/// the objects back *selected*.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    doc: Document,
    spot: Spot,
    /// Roughly what this costs to keep, so the ceiling can be counted
    /// without walking the board again.
    weight: usize,
}

impl Entry {
    fn of(doc: &Document, spot: Spot) -> Entry {
        Entry {
            doc: doc.clone(),
            spot,
            weight: weight(doc),
        }
    }

    /// Puts the board and the hand back to this state.
    ///
    /// The camera is the document's own field and is deliberately not
    /// restored: panning is not work, and a step backwards that moved
    /// the view would make the two mean the same thing. This is the one
    /// place that knows, so nothing can read an entry's camera by
    /// accident.
    pub fn restore(&self, doc: &mut Document, editor: &mut Editor) {
        let camera = doc.camera;
        doc.clone_from(&self.doc);
        doc.camera = camera;
        editor.go(self.spot.clone());
    }
}

/// The states one tab has rested in. `past` is the timeline and its last
/// entry is the **present**, so undo and redo are one move in two
/// directions: pop one side, push the other, and the board is whatever
/// the top of the past says.
#[derive(Debug, Clone)]
pub struct History {
    past: Vec<Entry>,
    future: Vec<Entry>,
    weight: usize,
}

impl History {
    /// A tab opening. The state it opens in is its first resting state,
    /// so there is always a present and every accessor can say so.
    pub fn new(doc: &Document, spot: Spot) -> History {
        let entry = Entry::of(doc, spot);
        History {
            weight: entry.weight,
            past: vec![entry],
            future: Vec::new(),
        }
    }

    /// A change came to rest — or the hand moved without one.
    ///
    /// The top of the past is the present, and the present's spot is
    /// live: a selection made between two changes belongs to the state
    /// on screen, not to the state before it. So when the board is what
    /// it already was, only the spot is written down. Without that,
    /// undoing a move would deselect what it moved — the entry under it
    /// would still hold the empty selection the tab was born with.
    ///
    /// A board that really changed pushes an entry and drops the future,
    /// because there is no longer a way forward from here.
    pub fn keep(&mut self, doc: &Document, spot: Spot) {
        let present = self.present_mut();
        if present.doc.same_board(doc) {
            present.spot = spot;
            return;
        }
        for gone in self.future.drain(..) {
            self.weight -= gone.weight;
        }
        let entry = Entry::of(doc, spot);
        self.weight += entry.weight;
        self.past.push(entry);
        self.trim();
    }

    /// One step back, and the state to put on screen.
    pub fn undo(&mut self) -> Option<&Entry> {
        if !self.can_undo() {
            return None;
        }
        let leaving = self.past.pop()?;
        self.future.push(leaving);
        self.past.last()
    }

    /// One step forward again.
    pub fn redo(&mut self) -> Option<&Entry> {
        let coming = self.future.pop()?;
        self.past.push(coming);
        self.past.last()
    }

    /// There is a state behind the present one.
    pub fn can_undo(&self) -> bool {
        self.past.len() > 1
    }

    /// A step was taken back and not yet taken again.
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    fn present_mut(&mut self) -> &mut Entry {
        self.past
            .last_mut()
            .expect("the past always holds the present")
    }

    /// Drops the oldest until the history is inside both ceilings. The
    /// present is never dropped: a tab always has a state it is in, even
    /// one too heavy to keep a step behind.
    fn trim(&mut self) {
        while self.past.len() > 1
            && (self.past.len() + self.future.len() > DEPTH || self.weight > BUDGET)
        {
            self.weight -= self.past.remove(0).weight;
        }
    }
}

/// Roughly what keeping `doc` costs. It counts the heap the geometry
/// sits on — the curves and what the pen said along them — because that
/// is the part that grows with a session; a few bytes of struct either
/// way decide nothing, and this is a ceiling rather than an audit.
fn weight(doc: &Document) -> usize {
    doc.layers.len() * LAYER + doc.elements.iter().map(element).sum::<usize>()
}

/// What one element costs beyond its geometry: its ids, its colour, the
/// struct itself. A round number, since this is an estimate.
const ELEMENT: usize = 256;

/// The same, for a layer: an id and a name.
const LAYER: usize = 128;

fn element(el: &Element) -> usize {
    ELEMENT
        + match el {
            Element::Path(p) => p.curves.len() * size_of::<Cubic>() + envelope(&p.pen),
            Element::Paint(p) => p
                .strokes
                .iter()
                .map(|s| ELEMENT + s.curves.len() * size_of::<Cubic>() + envelope(&s.pen))
                .sum(),
            Element::Rect(_) | Element::Image(_) | Element::Frame(_) => 0,
        }
}

fn envelope(pen: &Envelope) -> usize {
    (pen.pressure.len() + pen.twist.len()) * size_of::<f32>()
}
```

Add the imports this needs at the top, beside the two already there:

```rust
use crate::curve::Cubic;
use crate::doc::{Document, Element, Envelope};
```

(replacing the bare `use crate::doc::Document;` line).

Then add the test module at the end of `src/history.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Element, Layer, Paint, Stroke};

    fn board() -> Document {
        Document::new("b")
    }

    fn spot(selection: &[&str]) -> Spot {
        Spot {
            selection: selection.iter().map(|s| (*s).to_owned()).collect(),
            layer: None,
            inside: None,
        }
    }

    /// A board with `n` layers on it, which is a change `same_board` sees.
    fn with_layers(n: usize) -> Document {
        let mut doc = board();
        for i in 0..n {
            doc.layers.push(Layer::new(&format!("Layer {}", i + 2)));
        }
        doc
    }

    fn present(h: &History) -> &Entry {
        h.past.last().expect("a present")
    }

    #[test]
    fn a_tab_opens_with_the_state_it_opens_in_and_nowhere_to_go() {
        let h = History::new(&board(), spot(&[]));
        assert!(!h.can_undo(), "nothing behind the state it opened in");
        assert!(!h.can_redo());
    }

    #[test]
    fn a_board_that_changed_is_one_step() {
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&with_layers(1), spot(&[]));
        assert!(h.can_undo());
        assert_eq!(h.past.len(), 2);
    }

    #[test]
    fn a_board_that_did_not_change_is_no_step_at_all() {
        // Esc restoring a drag puts the board back where it was: there
        // is nothing to step behind, and no case in the code that says
        // so — the guard is the whole of it.
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&board(), spot(&[]));
        h.keep(&board(), spot(&[]));
        assert!(!h.can_undo());
        assert_eq!(h.past.len(), 1);
    }

    #[test]
    fn the_present_spot_follows_the_hand_between_two_changes() {
        // Select a, then move it. Undo has to bring back the board from
        // before the move with a still selected — which is only possible
        // if selecting it was written into the state on screen.
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&board(), spot(&["a"]));
        assert_eq!(present(&h).spot, spot(&["a"]));
        h.keep(&with_layers(1), spot(&["a"]));

        let (mut doc, mut editor) = (with_layers(1), Editor::new());
        let entry = h.undo().expect("a step back").clone();
        entry.restore(&mut doc, &mut editor);
        assert_eq!(editor.at(), spot(&["a"]));
    }

    #[test]
    fn a_step_back_restores_the_board_and_the_hand_together() {
        let before = board();
        let mut h = History::new(&before, spot(&["a", "b"]));
        let after = with_layers(2);
        h.keep(&after, spot(&[]));

        let mut doc = after.clone();
        let mut editor = Editor::new();
        let entry = h.undo().expect("a step back").clone();
        entry.restore(&mut doc, &mut editor);
        assert!(doc.same_board(&before), "the board is what it was");
        assert_eq!(editor.at().selection, ["a", "b"], "and so is the hand");
    }

    #[test]
    fn a_step_back_does_not_move_the_view() {
        let mut looking = board();
        looking.camera = Camera {
            x: 900.0,
            y: -12.0,
            zoom: 3.5,
        };
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&with_layers(1), spot(&[]));

        let mut doc = looking.clone();
        let mut editor = Editor::new();
        let entry = h.undo().expect("a step back").clone();
        entry.restore(&mut doc, &mut editor);
        assert_eq!(doc.camera, looking.camera, "panning is not work to undo");
    }

    #[test]
    fn a_pan_between_two_changes_is_not_a_step() {
        let mut h = History::new(&board(), spot(&[]));
        let mut panned = board();
        panned.camera = Camera {
            x: 40.0,
            y: 40.0,
            zoom: 2.0,
        };
        h.keep(&panned, spot(&[]));
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_and_redo_walk_the_same_line_and_land_where_they_started() {
        let states: Vec<Document> = (0..4).map(with_layers).collect();
        let mut h = History::new(&states[0], spot(&[]));
        for s in &states[1..] {
            h.keep(s, spot(&[]));
        }
        let (mut doc, mut editor) = (states[3].clone(), Editor::new());

        for want in states.iter().rev().skip(1) {
            h.undo().expect("a step back").clone().restore(&mut doc, &mut editor);
            assert!(doc.same_board(want));
        }
        assert!(!h.can_undo());

        for want in &states[1..] {
            h.redo().expect("a step on").clone().restore(&mut doc, &mut editor);
            assert!(doc.same_board(want));
        }
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_change_drops_the_way_forward() {
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&with_layers(1), spot(&[]));
        h.undo();
        assert!(h.can_redo());
        h.keep(&with_layers(7), spot(&[]));
        assert!(!h.can_redo(), "there is no forward from a different past");
    }

    #[test]
    fn redo_at_the_end_of_the_line_answers_nothing() {
        let mut h = History::new(&board(), spot(&[]));
        assert!(h.redo().is_none());
        h.keep(&with_layers(1), spot(&[]));
        assert!(h.redo().is_none());
    }
}
```

- [ ] **Step 2: Wire the module in**

In `src/main.rs`, add `mod history;` between `mod grid;` and `mod ipc;`.

- [ ] **Step 3: Run the tests**

Run: `cargo test history::`
Expected: PASS. If `Document::new` does not put a layer on the board, adjust `with_layers` — the only thing the helper owes is a board `same_board` can tell from another.

- [ ] **Step 4: Run the whole suite and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green. `Paint`/`Stroke` may be unused imports in the test module — drop what the compiler says is unused.

- [ ] **Step 5: Commit**

```bash
git add src/history.rs src/main.rs
git commit -m "history: the states a board came to rest in"
```

---

### Task 4: The ceiling

Depth and weight, both trimming from the oldest, and the present surviving both. The code landed in Task 3; this task is the tests that hold it to what it promises.

**Files:**
- Modify: `src/history.rs` (tests only, unless a test finds the trim wrong)

**Interfaces:**
- Consumes: `History::keep`, `DEPTH`, `BUDGET`, `weight` (Task 3).
- Produces: nothing new.

- [ ] **Step 1: Write the failing tests**

Add to `src/history.rs`'s `mod tests`:

```rust
/// A board carrying `curves` cubics on one paint stroke — the shape a
/// session actually grows into, and the one the budget is about.
fn heavy(curves: usize) -> Document {
    let mut doc = board();
    let layer = doc.layers[0].id.clone();
    doc.elements.push(Element::Paint(Paint {
        id: format!("P{curves}"),
        layer,
        strokes: vec![Stroke {
            curves: vec![[[0.0, 0.0]; 4]; curves],
            stroke: "#000000".into(),
            width: 2.0,
            opacity: 1.0,
            hardness: 1.0,
            stamp: None,
            pen: Envelope::default(),
        }],
        rotation: 0.0,
    }));
    doc
}

#[test]
fn a_boards_weight_is_the_geometry_it_carries() {
    let light = weight(&heavy(1));
    let heavier = weight(&heavy(1001));
    assert_eq!(
        heavier - light,
        1000 * size_of::<Cubic>(),
        "a thousand more curves cost a thousand curves"
    );
}

#[test]
fn the_history_is_no_deeper_than_the_depth() {
    let mut h = History::new(&board(), spot(&[]));
    for i in 1..DEPTH * 2 {
        h.keep(&with_layers(i), spot(&[]));
    }
    assert_eq!(h.past.len() + h.future.len(), DEPTH);
}

#[test]
fn the_depth_drops_the_oldest_and_keeps_the_newest() {
    let mut h = History::new(&board(), spot(&[]));
    for i in 1..DEPTH * 2 {
        h.keep(&with_layers(i), spot(&[]));
    }
    let newest = with_layers(DEPTH * 2 - 1);
    assert!(present(&h).doc.same_board(&newest), "the present survives");
    let oldest = h.past.first().expect("a past");
    assert!(
        !oldest.doc.same_board(&board()),
        "the state the tab opened in is long gone"
    );
}

#[test]
fn a_board_too_heavy_for_the_budget_still_keeps_its_present() {
    // One entry over budget on its own: the ceiling cannot answer by
    // leaving the tab with no state to be in.
    let over = BUDGET / size_of::<Cubic>() + 1;
    let mut h = History::new(&board(), spot(&[]));
    h.keep(&heavy(over), spot(&[]));
    assert_eq!(h.past.len(), 1, "trimmed to the present alone");
    assert!(present(&h).doc.same_board(&heavy(over)));
    assert!(!h.can_undo());
}

#[test]
fn the_budget_bites_before_the_depth_does_on_a_heavy_board() {
    // A tenth of the budget per entry: ten fit, a hundred do not.
    let tenth = BUDGET / 10 / size_of::<Cubic>();
    let mut h = History::new(&heavy(tenth), spot(&[]));
    for i in 1..20 {
        let mut doc = heavy(tenth);
        doc.layers.push(Layer::new(&format!("L{i}")));
        h.keep(&doc, spot(&[]));
    }
    assert!(h.past.len() < DEPTH, "the weight stopped it, not the count");
    assert!(h.weight <= BUDGET, "and it stopped it inside the budget");
}

#[test]
fn stepping_back_and_forth_does_not_leak_weight() {
    // Undo moves an entry from one side to the other; nothing is kept
    // twice, so the total is what it was.
    let mut h = History::new(&board(), spot(&[]));
    for i in 1..5 {
        h.keep(&with_layers(i), spot(&[]));
    }
    let full = h.weight;
    h.undo();
    h.undo();
    assert_eq!(h.weight, full, "a step back keeps what it stepped over");
    h.redo();
    assert_eq!(h.weight, full);
    h.keep(&with_layers(40), spot(&[]));
    let counted: usize = h.past.iter().chain(&h.future).map(|e| e.weight).sum();
    assert_eq!(h.weight, counted, "the running total is what is there");
}
```

If `Paint` or `Stroke` has fields beyond the literal above, read `src/doc.rs:717` and `src/doc.rs:731` and fill them in — the only thing `heavy` owes is a document whose weight is dominated by `curves`.

- [ ] **Step 2: Run the tests to verify they fail or pass**

Run: `cargo test history::tests`
Expected: the weight and depth tests PASS against Task 3's code; if any fails, the trim or `weight` is wrong and Task 3's implementation is what changes — not the test.

- [ ] **Step 3: Fix whatever the tests caught**

Most likely candidates, in the order they are worth checking:
- `trim` running before the weight is added, so a single over-budget push is not trimmed.
- `keep` subtracting the dropped future's weight after `drain`, which is a use-after-move.
- The `+ self.future.len()` missing from the depth test in `trim`, so undoing then pushing grows past `DEPTH`.

- [ ] **Step 4: Run the whole suite and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/history.rs
git commit -m "history: a depth and a budget, the oldest dropped first"
```

---

### Task 5: The keys

`app` gives every tab a history, writes to it when a change comes to rest, and steps through it on `Ctrl+Z` / `Ctrl+Y` / `Ctrl+Shift+Z`.

**Files:**
- Modify: `src/app.rs` — `struct Open` (~line 113), the destructure in `tick` (~line 545), `open_project` (~line 719), the `App` literal (~line 2278), `apply` (~line 1444), the control-key branch (~line 1798)

**Interfaces:**
- Consumes: `History::new/keep/undo/redo` and `Entry::restore` (Task 3), `Editor::at`/`busy` (Task 2).
- Produces: nothing other modules read.

- [ ] **Step 1: Give every tab a history**

Add the import beside the others in `src/app.rs`:

```rust
use crate::history::History;
```

`struct Open` gains a field:

```rust
struct Open {
    project: Project,
    editor: Editor,
    /// What this tab can step back through. Per tab, because a board and
    /// the work done on it go together: switching tabs hands nothing on,
    /// exactly as the tool and the selection do not.
    history: History,
}
```

`open_project` seeds it:

```rust
    fn open_project(&mut self, project: Project) {
        let mut editor = Editor::new();
        editor.set_surface(&self.theme.panel_hex);
        let history = History::new(&project.doc, editor.at());
        self.open.push(Open {
            project,
            editor,
            history,
        });
        self.activate(self.open.len() - 1);
    }
```

The `App` literal near line 2278 does the same:

```rust
        open: vec![{
            let editor = Editor::new();
            let history = History::new(&first.doc, editor.at());
            Open {
                project: first,
                editor,
                history,
            }
        }],
```

And the destructure in `tick` takes the rest:

```rust
        let Open {
            project, editor, ..
        } = &self.open[self.active];
```

- [ ] **Step 2: Build to verify the wiring compiles**

Run: `cargo build`
Expected: clean. Any other `Open { .. }` pattern the compiler names gets `..` the same way.

- [ ] **Step 3: Write down the states that come to rest**

Add to `impl App`, next to `touch`:

```rust
    /// Nothing is in the middle of happening. A carried layer card is
    /// the one gesture the editor knows nothing about: it lives here and
    /// reorders the document on every pointer move while the editor sits
    /// perfectly still.
    fn settled(&self) -> bool {
        !self.editor().busy() && !self.carry.as_ref().is_some_and(|c| c.held)
    }

    /// Writes down where a change left things, if it left them at rest.
    /// Mid-gesture there is nothing to write: a stroke changes the scene
    /// on every sample of the hand and a drag on every step, and neither
    /// is a state anybody meant to arrive at.
    fn remember(&mut self) {
        if !self.settled() {
            return;
        }
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        history.keep(&project.doc, editor.at());
    }
```

And `apply` calls it on both the changes that are a document or a selection:

```rust
    fn apply(&mut self, change: Change) {
        match change {
            Change::None => {}
            Change::Selection => {
                self.remember();
                self.redraw();
            }
            Change::Scene => {
                self.remember();
                self.touch();
                self.redraw();
            }
            Change::Camera(camera) => {
                self.active().1.camera = camera;
                self.redraw();
            }
        }
    }
```

- [ ] **Step 4: Add the two steps and their keys**

Add to `impl App`:

```rust
    /// `Ctrl+Z`: the board goes back to the state before the last
    /// change, and the hand back to where it was standing then.
    fn undo(&mut self) {
        self.drop_gesture();
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        let Some(entry) = history.undo() else { return };
        entry.restore(&mut project.doc, editor);
        self.touch();
        self.redraw();
    }

    /// `Ctrl+Y`, or `Ctrl+Shift+Z`: forward again.
    fn redo(&mut self) {
        self.drop_gesture();
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        let Some(entry) = history.redo() else { return };
        entry.restore(&mut project.doc, editor);
        self.touch();
        self.redraw();
    }

    /// Drops whatever is in progress before a step is taken, exactly as
    /// `Esc` would. A gesture that has not finished is not a change to
    /// step behind — it is a change that has not happened — and the card
    /// in the hand would otherwise be carrying a row the new stack does
    /// not have.
    fn drop_gesture(&mut self) {
        let (editor, doc) = self.active();
        editor.cancel(doc);
        if let Some(carry) = &mut self.carry {
            carry.held = false;
        }
    }
```

In the control-key branch, beside `"v"` and `"s"`:

```rust
                    "z" if shift => self.redo(),
                    "z" => self.undo(),
                    "y" => self.redo(),
```

- [ ] **Step 5: Build, test and lint**

Run: `cargo build && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green, 668 + the new tests.

- [ ] **Step 6: Drive the real window**

Run:

```bash
XDG_DATA_HOME=/tmp/omawhite-undo cargo run -- --socket /tmp/omawhite-undo.sock --smoke-frames 3
```

Expected: opens, renders, exits 0. Then run it for real (`cargo run`) and check by hand, since none of this is covered by tests:

- Draw three pencil strokes; `Ctrl+Z` three times takes them off one at a time, `Ctrl+Y` puts them back.
- Draw one brush stroke; one `Ctrl+Z` takes the whole stroke, not a dab of it.
- Select an object, drag it across the board, `Ctrl+Z` — it goes back in one step and stays selected.
- Delete a selection and `Ctrl+Z` — the objects come back **selected**.
- Pan and zoom between two strokes, then `Ctrl+Z` — the view does not move.
- Drag a layer card up the stack and `Ctrl+Z` — one step, and the cards slide back.
- `Ctrl+Z` past the beginning does nothing; `Ctrl+Y` past the end does nothing.
- Two tabs, work in each: `Ctrl+Z` in one leaves the other alone.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs
git commit -m "app: Ctrl+Z steps a tab back, Ctrl+Y and Ctrl+Shift+Z forward"
```

---

### Task 6: What the docs now say

**Files:**
- Modify: `README.md` (a bullet in Status, the `Not yet` line, the Controls table)
- Modify: `AGENTS.md` (the module list in *Architecture*, and a paragraph on what a step is)
- Modify: `ARCHITECTURE.md` (a *Landed* note under §7.2, where the key table promises `Ctrl+Z / Ctrl+Y`)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: README**

Add a Status bullet after the Ink one:

```markdown
- Undo: `Ctrl+Z` puts the board back the way it was before the last
  change and the hand back where it was standing then; `Ctrl+Y` and
  `Ctrl+Shift+Z` put it forward again. A step is a change that came to
  **rest** — a whole stroke, a whole drag, a delete, a paste, a layer
  moved — never a sample of one, which is why a stroke undoes as a
  stroke. Undoing a delete brings the objects back selected, because
  what is kept is the board *and* the spot the hand was on. The view is
  not part of it: panning between two strokes is not work, and a step
  back does not move it. The history is a tab's own, capped by a depth
  and by a memory budget, so a heavy board gets fewer steps rather than
  a quarter of a gigabyte of them.
```

Take `undo` out of the `Not yet:` line at line ~362 (leave `export` alone — it is already stale, and one thing at a time).

Add to the Controls table, beside the other `Ctrl` rows:

```markdown
| `Ctrl+Z` | undo |
| `Ctrl+Y`, `Ctrl+Shift+Z` | redo |
```

- [ ] **Step 2: AGENTS.md**

In the *Pure core, thin shell* bullet, add `history` to the list of pure modules that carry tests — between `project` and `store`:

> ... `project` (a document's origin and dirty flag), `history` (the states a board came to rest in, and the spot the hand was on at each), `store` (XDG persistence, blobs, project files), ...

Then add a bullet after **Documents and tabs**:

```markdown
- **Undo.** A tab's `History` is the states its board came to **rest**
  in: `Change::Scene` fires on every sample of a stroke and on every
  step of a drag, so the step is the rest, not the change. `App::remember`
  is what decides — `apply` calls it, and it does nothing unless
  `App::settled`, which is the editor holding no gesture (`Editor::busy`:
  stroke, area, navigation or drag) *and* no layer card in the pointer's
  hand, since a carried card lives on `App` and reorders the document
  while the editor sits still. An entry is the whole `Document` plus the
  editor's `Spot` — selection, ink layer, frame being worked in — kept
  together so neither has to be checked against the other: a spot
  restored beside the document it was taken with names ids that document
  has, by construction, which is why undoing a delete brings the objects
  back selected. The top of the past *is* the present and its spot is
  live, so `History::keep` writes only the spot when the board is what it
  already was — otherwise a selection made between two changes would be
  lost to the state before it, and undoing a move would deselect what it
  moved. That same guard is why `Esc` restoring a drag pushes nothing.
  The camera is in the document and deliberately out of a step, at both
  ends: `Document::same_board` destructures rather than lists its fields
  so a field added later cannot slip past undeclared, and `Entry::restore`
  keeps the camera the live document has. A snapshot rather than an edge
  that knows its own inverse: it cannot come to disagree with the model
  because it *is* the model, and every operation is undoable the day it
  lands without a line at its call site. The price is memory, so the
  history is capped by a depth *and* a byte budget, oldest dropped first
  and the present never — a heavy board gets fewer steps. `Vec<Arc<Element>>`
  would buy the depth back if a real board ever makes it hurt, and the
  interface does not change if it does.
```

- [ ] **Step 3: ARCHITECTURE.md**

Under §7.2, after the existing *Landed:* paragraphs, add:

```markdown
Landed: `Ctrl+Z` / `Ctrl+Y` from the table above, plus `Ctrl+Shift+Z`.
A step is a resting state rather than a change — the scene changes on
every sample of a stroke — and what is kept is the whole document beside
the editor's spot, so the two cannot come to disagree about what a
restored board holds. The camera is excluded at both ends: §6.2 keeps the
cursor out of the document for the reason it keeps panning out of a step.
The history is a tab's, capped by a depth and a memory budget.
```

- [ ] **Step 4: Check the README's own count**

Run: `cargo test 2>&1 | tail -3`
Take the test count and update the Status line `cargo test with 668 tests` to the real number.

- [ ] **Step 5: Commit**

```bash
git add README.md AGENTS.md ARCHITECTURE.md
git commit -m "README, AGENTS, ARCHITECTURE: a step back is a state, not a change"
```

---

## Self-Review

**Spec coverage.** §1 what one step is → Task 5 (`settled`/`remember`) and Task 2 (`busy`). §2 the entry → Task 3 (`Entry`, `keep`'s two branches). §3 the camera → Task 1 (`same_board`) and Task 3 (`restore`), tested in both. §4 the budget → Tasks 3 and 4. §5 not in this cut → nothing to build; recorded in Task 6's AGENTS paragraph. §6 the pieces → Tasks 1, 2, 3, 5, one per module. §7 undo dirties → Task 5 (`self.touch()` in both steps). §8 testing → Tasks 1, 2, 3, 4 test lists, plus Task 5 step 6 for the shell, which carries no tests by project convention. §9 not in this cut → nothing built.

**Placeholders.** None: every step carries the code it asks for. The two "read the struct and match it" notes (Task 1's `Rect`, Task 4's `Paint`/`Stroke`) name the exact file and line and state what the literal owes.

**Type consistency.** `Spot` fields (`selection`, `layer`, `inside`) are the same in Tasks 2, 3 and 5. `History::keep(&Document, Spot)` and `Entry::restore(&mut Document, &mut Editor)` are called in Task 5 exactly as Task 3 defines them. `Editor::at()`/`busy()` are defined in Task 2 and used in Tasks 3 and 5. `Document::same_board` is defined in Task 1 and used in Task 3's `keep` and in Tasks 3 and 4's tests. `DEPTH`, `BUDGET` and `weight` are defined in Task 3 and tested in Task 4.
