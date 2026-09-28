//! What a tab can step back through: the states its board came to rest
//! in, and the spot the hand was standing on at each of them.
//!
//! A step is not every change. `Change::Scene` fires on every sample of
//! a stroke and on every step of a drag — which is why the draft save is
//! debounced — so what is written down here is a change that has come to
//! **rest**: applied, with nothing still happening. `app` is what knows
//! when that is; this module only ever hears about the ones that count.
//!
//! An entry is the whole board rather than an edit that knows its own
//! inverse. A snapshot cannot come to disagree with the model, because
//! it *is* the model: every operation is undoable the day it lands, and
//! none of them owes a line at its call site. The price is memory, and
//! §4 of the design is the ceiling that pays it.

use crate::curve::Cubic;
use crate::doc::{Document, Element, Envelope};
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
    /// place that knows, so nothing else can read an entry's camera by
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
    /// What `past` and `future` weigh together, kept as they change so
    /// the ceiling never walks the whole history to ask.
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
    /// That same guard is why `Esc` restoring a drag pushes nothing:
    /// the board is back where it was, so there is no step to take.
    ///
    /// A board that really changed pushes an entry and drops the future,
    /// because there is no longer a way forward from here.
    pub fn keep(&mut self, doc: &Document, spot: Spot) {
        if self.present_mut().doc.same_board(doc) {
            self.mark(spot);
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

    /// The hand moved, whether or not the board is at rest.
    ///
    /// The top of the past is the present, and the present's spot is
    /// live: what is selected belongs to the state on screen, not to
    /// the one before it. Undoing a move has to leave what it moved in
    /// hand, and that is only possible if the entry under the move
    /// knows it was picked up.
    ///
    /// It cannot wait for a resting point, because a press on an object
    /// selects it *and* starts dragging it in one motion — there is no
    /// moment between the two when anything is settled, so a spot
    /// written only at rest would still be the empty one the tab was
    /// born with. Cheap enough to do on every sample: a handful of ids,
    /// and never the board.
    pub fn mark(&mut self, spot: Spot) {
        self.present_mut().spot = spot;
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

    /// There is a state behind the present one: what the Edit menu asks
    /// before it offers Undo.
    pub fn can_undo(&self) -> bool {
        self.past.len() > 1
    }

    /// A step was taken back and nothing has changed since.
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
    /// one too heavy to keep a single step behind.
    ///
    /// Only the past is counted, and that is enough for the whole
    /// history: this runs at the end of [`History::keep`], which has
    /// just dropped the future, and nothing after that puts entries
    /// back — undo moves one from the past to the future and leaves the
    /// total where it was. Counting the future here as well would read
    /// like a second guard and could never fire.
    fn trim(&mut self) {
        while self.past.len() > 1 && (self.past.len() > DEPTH || self.weight > BUDGET) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Layer, Paint, Stroke};

    fn board() -> Document {
        let mut doc = Document::new("b");
        // A fixed id and layer, so two boards built the same way are the
        // same board — a fresh ULID either side would tell them apart
        // for a reason that has nothing to do with what is drawn on them.
        doc.id = "01JTESTTESTTESTTESTTESTTES".into();
        doc.layers[0].id = "L1".into();
        doc
    }

    pub(super) fn spot(selection: &[&str]) -> Spot {
        Spot {
            selection: selection.iter().map(|s| (*s).to_owned()).collect(),
            layer: None,
            picked: Vec::new(),
        }
    }

    /// [`board`] with `n` more layers on it — a change `same_board` sees.
    fn with_layers(n: usize) -> Document {
        let mut doc = board();
        for i in 0..n {
            let mut layer = Layer::new(&format!("Layer {}", i + 2));
            layer.id = format!("L{}", i + 2);
            doc.layers.push(layer);
        }
        doc
    }

    /// A board carrying `curves` cubics on one paint stroke — the shape a
    /// session actually grows into, and the one the budget is about.
    fn heavy(curves: usize) -> Document {
        let mut doc = board();
        let layer = doc.layers[0].id.clone();
        doc.elements.push(Element::Paint(Paint {
            id: "P1".into(),
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

    pub(super) fn present(h: &History) -> &Entry {
        h.past.last().expect("a present")
    }

    /// Takes the step `back` says and puts it on a fresh board and hand,
    /// the way `app` does.
    fn step(h: &mut History, back: bool) -> (Document, Editor) {
        let (mut doc, mut editor) = (Document::new("scratch"), Editor::new());
        let entry = if back { h.undo() } else { h.redo() };
        entry.expect("a step to take").restore(&mut doc, &mut editor);
        (doc, editor)
    }

    #[test]
    fn a_tab_opens_with_the_state_it_opens_in_and_nowhere_to_go() {
        let h = History::new(&board(), spot(&[]));
        assert!(!h.can_undo(), "nothing behind the state it opened in");
        assert!(h.future.is_empty());
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
        // Esc restoring a drag puts the board back where it was: there is
        // nothing to step behind, and no case in the code that says so —
        // the guard is the whole of it.
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

        let (_, editor) = step(&mut h, true);
        assert_eq!(editor.at(), spot(&["a"]));
    }

    #[test]
    fn a_step_back_restores_the_board_and_the_hand_together() {
        let before = board();
        let mut h = History::new(&before, spot(&["a", "b"]));
        h.keep(&with_layers(2), spot(&[]));

        let (doc, editor) = step(&mut h, true);
        assert!(doc.same_board(&before), "the board is what it was");
        assert_eq!(editor.at().selection, ["a", "b"], "and so is the hand");
    }

    #[test]
    fn a_step_back_does_not_move_the_view() {
        let looking = Camera {
            x: 900.0,
            y: -12.0,
            zoom: 3.5,
        };
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&with_layers(1), spot(&[]));

        let (mut doc, mut editor) = (with_layers(1), Editor::new());
        doc.camera = looking;
        h.undo().expect("a step back").restore(&mut doc, &mut editor);
        assert_eq!(doc.camera, looking, "panning is not work to undo");
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

        for want in states.iter().rev().skip(1) {
            let (doc, _) = step(&mut h, true);
            assert!(doc.same_board(want), "back to {:?}", want.layers.len());
        }
        assert!(!h.can_undo(), "the line ends where the tab opened");

        for want in &states[1..] {
            let (doc, _) = step(&mut h, false);
            assert!(doc.same_board(want), "on to {:?}", want.layers.len());
        }
        assert!(h.future.is_empty());
    }

    #[test]
    fn a_new_change_drops_the_way_forward() {
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&with_layers(1), spot(&[]));
        h.undo();
        assert!(!h.future.is_empty());
        h.keep(&with_layers(7), spot(&[]));
        assert!(h.future.is_empty(), "there is no forward from a different past");
    }

    #[test]
    fn redo_at_the_end_of_the_line_answers_nothing() {
        let mut h = History::new(&board(), spot(&[]));
        assert!(h.redo().is_none());
        h.keep(&with_layers(1), spot(&[]));
        assert!(h.redo().is_none(), "a step nobody took back");
    }

    #[test]
    fn the_way_forward_is_there_only_after_a_step_back() {
        let mut h = History::new(&board(), spot(&[]));
        assert!(!h.can_redo());
        h.keep(&with_layers(1), spot(&[]));
        assert!(!h.can_redo(), "a step nobody took back");
        h.undo();
        assert!(h.can_redo());
        h.redo();
        assert!(!h.can_redo(), "back where the line ends");
    }

    #[test]
    fn undo_at_the_start_of_the_line_answers_nothing() {
        let mut h = History::new(&board(), spot(&[]));
        assert!(h.undo().is_none());
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
    fn no_run_of_changes_and_steps_back_ever_passes_the_depth() {
        // Undo moves an entry from one side to the other and a change
        // drops the far side, so the two ceilings hold across any mix of
        // them — never more states than the depth, and never none.
        let mut h = History::new(&board(), spot(&[]));
        let mut kept = 0usize;
        for round in 1..DEPTH * 3 {
            kept += 1;
            h.keep(&with_layers(kept), spot(&[]));
            if round % 3 == 0 {
                for _ in 0..round % 7 {
                    h.undo();
                }
            }
            assert!(
                h.past.len() + h.future.len() <= DEPTH,
                "round {round}: {} past, {} future",
                h.past.len(),
                h.future.len()
            );
            assert!(!h.past.is_empty(), "round {round}: a tab is always in a state");
        }
    }

    #[test]
    fn a_board_too_heavy_for_the_budget_still_keeps_its_present() {
        // One entry over budget on its own: the ceiling cannot answer by
        // leaving the tab with no state to be in.
        let over = BUDGET / size_of::<Cubic>() + 1;
        let mut h = History::new(&board(), spot(&[]));
        h.keep(&heavy(over), spot(&[]));
        assert_eq!(h.past.len(), 1, "trimmed back to the present alone");
        assert!(present(&h).doc.same_board(&heavy(over)));
        assert!(!h.can_undo());
    }

    #[test]
    fn the_budget_bites_before_the_depth_does_on_a_heavy_board() {
        // A tenth of the budget per entry: ten fit where a hundred would
        // have, so it is the weight that stopped it and not the count.
        let tenth = BUDGET / 10 / size_of::<Cubic>();
        let mut h = History::new(&heavy(tenth), spot(&[]));
        for i in 1..20 {
            let mut doc = heavy(tenth);
            doc.layers.push(Layer::new(&format!("L{i}")));
            h.keep(&doc, spot(&[]));
        }
        assert!(h.past.len() < DEPTH, "the weight stopped it, not the count");
        assert!(h.weight <= BUDGET, "and stopped it inside the budget");
    }

    #[test]
    fn the_running_weight_is_what_is_actually_there() {
        // A step back moves an entry from one side to the other; nothing
        // is kept twice, and nothing is paid for after it is dropped.
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
        h.keep(&heavy(40), spot(&[]));
        let counted: usize = h.past.iter().chain(&h.future).map(|e| e.weight).sum();
        assert_eq!(h.weight, counted, "the future it dropped is not still paid for");
    }
}

/// Whole gestures, driven the way the window drives them: a stroke, a
/// drag, an `Esc`. `app` carries no tests of its own, so the rule it
/// encodes — a change is written down only once nothing is still
/// happening — is held here, where the editor and the history can be
/// put through it without a window.
#[cfg(test)]
mod gestures {
    use super::tests::present;
    use super::*;
    use crate::brush::Tip;
    use crate::doc::{Camera, Element, Rect};
    use crate::editor::{Button, Change, Tool};
    use crate::scene::{View, Viewport};

    /// What `app::App::remember` does, less the carried layer card —
    /// the one gesture the editor cannot see, because it lives on the
    /// window and reorders the document while the editor sits still.
    fn drive(h: &mut History, e: &Editor, doc: &Document, change: Change) {
        if !matches!(change, Change::Scene | Change::Selection) {
            return;
        }
        match e.busy() {
            true => h.mark(e.at()),
            false => h.keep(doc, e.at()),
        }
    }

    fn view() -> View {
        View {
            camera: Camera {
                x: 50.0,
                y: 50.0,
                zoom: 1.0,
            },
            viewport: Viewport { w: 100, h: 100 },
            scale: 1.0,
        }
    }

    /// One rect at (10, 10), 20 by 10, on the board's only layer.
    fn board() -> Document {
        let mut doc = Document::new("t");
        doc.id = "01JTESTTESTTESTTESTTESTTES".into();
        doc.layers[0].id = "L1".into();
        doc.elements = vec![Element::Rect(Rect {
            id: "a".into(),
            layer: "L1".into(),
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 10.0,
            rotation: 0.0,
            stroke: Some("#222".into()),
            fill: None,
            text: None,
        })];
        doc
    }

    fn with(tool: Tool, doc: &mut Document) -> Editor {
        let mut e = Editor::new();
        e.set_tool(tool, doc);
        e
    }

    #[test]
    fn a_whole_stroke_is_one_step_however_many_samples_it_took() {
        let (v, mut doc) = (view(), board());
        let mut e = with(Tool::Pencil, &mut doc);
        let mut h = History::new(&doc, e.at());

        let c = e.press(Button::Left, &v, (40.0, 40.0), &mut doc, &Tip::PENCIL);
        drive(&mut h, &e, &doc, c);
        for i in 1..30 {
            let at = (40.0 + f64::from(i) * 1.5, 40.0 + f64::from(i));
            let c = e.moved(&v, at, &mut doc);
            drive(&mut h, &e, &doc, c);
        }
        assert_eq!(h.past.len(), 1, "nothing is written down mid-stroke");

        let c = e.release(Button::Left, &v, (85.0, 70.0), &mut doc, "#111111");
        drive(&mut h, &e, &doc, c);
        assert_eq!(h.past.len(), 2, "and the whole stroke is one step");

        let (mut back, mut hand) = (doc.clone(), Editor::new());
        h.undo().expect("a step back").restore(&mut back, &mut hand);
        assert!(back.elements.iter().all(|el| el.id() == "a"), "the ink is gone");
    }

    #[test]
    fn a_whole_drag_is_one_step_however_far_it_went() {
        let (v, mut doc) = (view(), board());
        let mut e = with(Tool::Select, &mut doc);
        let mut h = History::new(&doc, e.at());

        // Pick the rect up: a press on it selects it, which is a resting
        // state of its own — the hand moved, the board did not.
        let c = e.press(Button::Left, &v, (15.0, 15.0), &mut doc, &Tip::PENCIL);
        drive(&mut h, &e, &doc, c);
        assert_eq!(h.past.len(), 1, "selecting is not a change to the board");
        assert_eq!(present(&h).spot.selection, ["a"], "but the spot followed it");

        for i in 1..20 {
            let at = (15.0 + f64::from(i) * 2.0, 15.0);
            let c = e.moved(&v, at, &mut doc);
            drive(&mut h, &e, &doc, c);
        }
        assert_eq!(h.past.len(), 1, "nothing is written down mid-drag");

        let c = e.release(Button::Left, &v, (53.0, 15.0), &mut doc, "#111111");
        drive(&mut h, &e, &doc, c);
        assert_eq!(h.past.len(), 2, "and the whole drag is one step");

        // Undo puts it back where it was, still selected: the spot on the
        // entry under the move is the one the press wrote.
        let (mut back, mut hand) = (doc.clone(), Editor::new());
        h.undo().expect("a step back").restore(&mut back, &mut hand);
        let Element::Rect(r) = &back.elements[0] else {
            panic!("a rect");
        };
        assert_eq!((r.x, r.y), (10.0, 10.0), "back where it was");
        assert_eq!(hand.at().selection, ["a"], "and still in hand");
    }

    #[test]
    fn a_drag_taken_back_with_escape_is_no_step_at_all() {
        let (v, mut doc) = (view(), board());
        let mut e = with(Tool::Select, &mut doc);
        let mut h = History::new(&doc, e.at());

        let c = e.press(Button::Left, &v, (15.0, 15.0), &mut doc, &Tip::PENCIL);
        drive(&mut h, &e, &doc, c);
        for i in 1..10 {
            let c = e.moved(&v, (15.0 + f64::from(i) * 3.0, 15.0), &mut doc);
            drive(&mut h, &e, &doc, c);
        }
        assert!(e.escape(&mut doc), "there was a drag to cancel");
        drive(&mut h, &e, &doc, Change::Scene);

        assert_eq!(h.past.len(), 1, "the board is what it already was");
        assert!(!h.can_undo());
    }

    #[test]
    fn two_strokes_are_two_steps() {
        let (v, mut doc) = (view(), board());
        let mut e = with(Tool::Pencil, &mut doc);
        let mut h = History::new(&doc, e.at());
        for round in 0..2 {
            let y = 30.0 + f64::from(round) * 20.0;
            let c = e.press(Button::Left, &v, (20.0, y), &mut doc, &Tip::PENCIL);
            drive(&mut h, &e, &doc, c);
            for i in 1..15 {
                let c = e.moved(&v, (20.0 + f64::from(i) * 2.0, y), &mut doc);
                drive(&mut h, &e, &doc, c);
            }
            let c = e.release(Button::Left, &v, (48.0, y), &mut doc, "#111111");
            drive(&mut h, &e, &doc, c);
        }
        assert_eq!(h.past.len(), 3, "the state it opened in and one per stroke");
    }
}
