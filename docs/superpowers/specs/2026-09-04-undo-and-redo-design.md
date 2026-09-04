# Undo and redo

`Ctrl+Z` puts the board back the way it was before the last change, and
the hand back where it was standing then. `Ctrl+Shift+Z` puts it
forward again — one key and not two, which is §7.2's table catching up
rather than being obeyed. Keys only: nothing new is drawn in the window.

ARCHITECTURE.md promises this in three places — §7.2's key table, §12's
MVP list, §15's fourth cut — and each of them assumed a history that
does not exist yet. This is that history.

## 1. What one step is

Not every change: `Change::Scene` fires on every sample of a stroke and
on every step of a drag, which is why the draft save is already
debounced. A step is a change that has come to **rest**.

A change is at rest when it has been applied and nothing is still
happening: the editor holds no stroke, no area being dragged out, no
navigation and no drag, and no layer card is in the pointer's hand.
The last of those is not the editor's business — a carried card lives
in `App.carry` and reorders the document on every pointer move while
the editor sits perfectly still — so the question is asked of the
window, not of the editor alone.

Everything else falls out of that one rule. A stroke is one step,
written when the hand comes up. A move is one step, whatever it passed
over on the way. A delete, a paste, a layer added, a layer renamed, an
ink laid on a frame's ground — each is atomic and each is already at
rest when it returns. And an operation that does not exist yet is
undoable the day it lands, because it will go down the same funnel.

## 2. The entry

An entry is the board at rest and the spot the hand was standing on:

```rust
pub struct Entry {
    doc: Document,
    spot: Spot,
}
```

`Spot` is the editor's own session state — what is selected, the layer
new ink lands on, the frame being worked in — and it lives in `editor`,
because that is whose it is. `history` keeps one; it does not define it.

Keeping the two together is what makes reconciliation unnecessary. A
spot restored beside the document it was taken with names ids that
document has, **by construction**: there is nothing to check and nothing
to guess. Undo a delete and the objects come back selected, which is how
you see what came back.

For that to hold, the spot has to be current. The top of the past *is*
the present, and the present's spot is live: a selection made between
two changes belongs to the state on screen, not to the state before it.

Without that, undoing a move would deselect what it moved: the entry
under it would still hold the empty selection the tab was born with.

**Corrected during the build.** This section first said `History::keep`
could do both jobs — push when the board changed, write the spot when it
had not — and that a resting point was the only moment either was
needed. A test of a whole drag proved otherwise: `select_press` selects
an object *and* arms the drag in the same call, so there is no moment
between the two when anything is at rest, and a spot written only at
rest is still the empty one the tab was born with. `History::mark` is
therefore its own method and is called on every change, at rest or not;
`keep` marks and returns when the board has not moved. Marking is cheap
enough for a pointer sample — a handful of ids, and never the board.

## 3. The camera stays out

`Document` carries the camera, and undo must not move the view. Panning
is not work — `Change::Camera` deliberately does not even dirty the tab
— and being teleported by an undo would make the two mean the same
thing.

So the camera is out of a resting state at both ends:

- **Comparing.** `Document::same_board` answers whether two documents
  hold the same board, with the camera excluded. It destructures
  `Document` rather than listing fields, so a field added later fails to
  compile until somebody decides which side of the line it is on.
- **Restoring.** `Entry::restore` keeps the camera the live document
  has. The rule lives in exactly one place, so nothing can read an
  entry's camera by accident.

## 4. The budget

An entry is a whole document, so the cost of a step climbs with the
session — the board grows as it is drawn on, and every entry carries all
of it.

Measured rather than guessed: the heaviest board on this machine holds
143 strokes and 1751 curves — about 250 KB in memory, though its
pretty-printed file is 971 KB. A hundred entries of it are 25 MB, which
is nothing. Ten times that board is 250 MB, which is not.

So a count alone is the wrong ceiling. The history is capped by **both**
a depth (100 entries) and a weight (64 MB), the oldest dropped first,
and the present never dropped. A heavy board gets fewer steps. That is
what Krita and GIMP do, and it is the honest trade: the alternative is a
whiteboard that quietly eats a quarter of a gigabyte.

`history::weight` estimates an entry from the heap its geometry sits on
— curves and pen readings — which is the part that actually grows. A few
bytes of struct either way decide nothing.

## 5. Structural sharing is not in this cut

`Vec<Arc<Element>>` would make a snapshot a handful of pointer bumps and
share every element a step did not touch. It is the right answer if the
budget ever starts biting.

It is not this cut's answer. It changes the shape of the document —
serde's `rc` feature, `Arc::make_mut` at every mutable element access —
to solve a problem the measurement says is not here, and ARCHITECTURE.md
is explicit about that order: *measure before inventing a daemon*. The
interface designed here does not change if it lands later.

The same goes for an edit log with inverses. It is cheaper in memory and
far more expensive in correctness: every mutation site would owe an
edit, and a wrong inverse corrupts a board in silence. The mutations
here are not uniform — a drag rewrites arbitrary elements from a
snapshot, `rehome_layer` moves a layer between two stacks, `settle_layers`
imposes invariants on the way in. A snapshot cannot desync from the
model, because it *is* the model.

## 6. The pieces

**`src/history.rs`** — new, pure, tested.

```rust
pub struct History { past: Vec<Entry>, future: Vec<Entry>, weight: usize }

impl History {
    pub fn new(doc: &Document, spot: Spot) -> History;
    pub fn keep(&mut self, doc: &Document, spot: Spot);
    pub fn undo(&mut self) -> Option<&Entry>;
    pub fn redo(&mut self) -> Option<&Entry>;
    pub fn can_undo(&self) -> bool;
    pub fn can_redo(&self) -> bool;
}
```

`past` is the timeline of resting states and its last entry is the
present, so undo and redo are the same move in two directions: pop one
side, push the other, and the board is whatever the top of the past
says.

**`src/editor.rs`** — three additions and nothing else:

- `Spot` — what is selected, the ink layer, the frame being worked in.
- `Editor::at()` / `Editor::go(spot)` — read it and put it back.
- `Editor::busy()` — a stroke, an area, a navigation or a drag is in
  progress. Wider than the `is_moving` that exists, which only ever
  meant `Drag::Move`.

**`src/doc.rs`** — `Document::same_board`.

**`src/app.rs`** — the wiring:

- `Open` gains a `history`, seeded at every tab's birth.
- `App::settled` — the editor is not busy and no card is being carried.
- `App::remember` — called from `apply` on `Change::Scene` and
  `Change::Selection`, and does nothing unless settled. A marquee never
  pays for a document comparison, because a marquee is a drag.
- `App::undo` / `App::redo` — cancel whatever is in progress first, the
  way `Esc` does (a gesture that has not finished is not a change to
  step behind), then swap and `touch`.
- `Ctrl+Z` and `Ctrl+Shift+Z` in the existing control branch. A
  field being typed into already swallows the whole keyboard, so
  `Ctrl+Z` mid-rename cannot reach the canvas.

The layers panel needs nothing: `Slides::restack` already compares the
stack against the one it last saw, so undoing a reorder animates the
cards back into place on its own.

## 7. Undo dirties the tab

The document changed relative to what is on disk, so it did. Tracking
whether an undo happened to land exactly on the last-saved state would
need a marker the history does not carry, and would buy a clean dot in
one case out of many.

## 8. Testing

`history` carries the suite, as every pure module here does:

- A change at rest is one step; a change that is not, none.
- Restoring a state restores the spot with it, and the spot's ids are in
  the document it came with.
- The present's spot follows the selection between two changes.
- The camera is neither compared nor restored.
- `Esc` restoring a drag pushes nothing, because the board is what it
  already was.
- A whole gesture is one step: a stroke however many samples it took, a
  drag however far it went, two strokes two steps. Driven through the
  editor the way the window drives it, since `app` carries no tests.
- A push drops the future.
- Depth and weight both trim, oldest first, and the present survives
  both.
- Undo and redo walk the same line in both directions and land where
  they started.

`Document::same_board` is tested to disagree on every field that is part
of a board and agree across a camera.

## 8.1 What the build corrected

Three things this design got wrong, kept here because a spec that
quietly disagrees with the code is worse than none:

- **The spot could not wait for a resting point.** See §2. A test of a
  whole drag found it; `History::mark` is the fix.
- **A carried layer card is let go of behind an early return.**
  `App::pointer_released` and `App::focus_lost` both clear the carry and
  return without reaching `apply`, so the stack the card was dropped in
  was never written down: the drag applied and was not undoable, and the
  next change to come to rest would have swallowed it into one step.
  Driving the real window found it — no test could have, since `app`
  carries none. Both now call `remember` themselves.
- **`trim` counting the future was dead code.** It runs only at the end
  of `keep`, which has just dropped the future, so the term could never
  fire. It read like a second guard and was not one. Removed, with the
  reason written where it stood.

## 9. Not in this cut

- Any visible affordance — dock buttons, a history panel.
- Undo across tabs, or a history that outlives the process.
- Undoing a brush edit, a tool change or a panel's scroll. None of them
  is the board.
- Coalescing (two nudges in a row staying two steps). Each rest is a
  step; that is the rule, and merging is a second rule that would have
  to say when.
