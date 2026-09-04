# Brush Slots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the brush tool's left-hand chrome into a narrow strip that names the brush in the hand and keeps ten within reach, and the full Brush Library as a second panel that opens beside it.

**Architecture:** A new pure module `src/slots.rs` draws the strip; `src/palette.rs` sheds its preview block and becomes the library alone; the state saying which brush sits in which slot lives on `Library` in `src/brush.rs`, named rather than indexed, and rides to disk in `Edits`. `src/app.rs` is the only shell change: it lays out both panels, routes their hits, and carries a brush's icon between them during a drag.

**Tech Stack:** Rust, winit 0.30, wgpu. Inline `#[cfg(test)] mod tests`, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-09-04-brush-slots-design.md`

## Global Constraints

- Pure modules carry the tests; `app` and `gfx` stay untested shell — keep logic out of them.
- A brush is addressed by `(set name, brush name)`, never by index: a set added to `brushes/` moves every index after it.
- `brushes.json` stays 0600 and atomic; `Edits` fields absent on disk must open meaning what they meant before the field existed.
- Documentation, code and commits in English. Commits atomic, one coherent change each.
- Zero compiler warnings before the branch is finished (`cargo build 2>&1 | grep warning`).
- Every value read from disk goes through its own `Property::set` — already true of `Library::apply`; slots add no numbers, only names.
- Chrome sizes are logical px scaled by `App::chrome`; radii go through `Theme::corner`, borders through `Theme::edge`.

---

### Task 1: The slot model on `Library`

**Files:**
- Modify: `src/brush.rs` (near `Edits`/`Held` ~line 645, and `impl Library` ~line 773)
- Test: `src/brush.rs` inline `mod tests`

**Interfaces:**
- Produces:
  - `pub const SLOTS: usize = 10;`
  - `pub const SLOT_DEFAULTS: [(&str, &str); 9]`
  - `Library::slots(&self) -> &[Option<(usize, usize)>]` — ten seats, `[0]` the overflow, `[1..=9]` the defaults; each `Some((set, index))` into `sets()`.
  - `Library::take_slot(&mut self, n: usize) -> bool` — takes up that slot's brush; false when empty or out of range.
  - `Library::assign_slot(&mut self, n: usize, at: (usize, usize)) -> bool` — writes a brush into 1..=9; false for slot 0 or out of range.
  - `Edits.slots: Vec<Option<Held>>` (serde `default`, `skip_serializing_if = "Vec::is_empty"`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_slots_ship_with_the_first_nine_of_basic() {
    let lib = Library::default();
    let slots = lib.slots();
    assert_eq!(slots.len(), SLOTS);
    assert_eq!(slots[0], None, "the overflow seat starts empty");
    for (n, (set, name)) in SLOT_DEFAULTS.iter().enumerate() {
        let (s, i) = slots[n + 1].expect("a default sits in every seat 1..=9");
        assert_eq!(lib.sets()[s].name, *set);
        assert_eq!(lib.sets()[s].presets[i].name, *name);
    }
}

#[test]
fn a_brush_taken_from_outside_the_slots_lands_in_slot_zero() {
    let mut lib = Library::default();
    let outside = lib
        .sets()
        .iter()
        .enumerate()
        .find_map(|(s, set)| (set.name == "Splatter").then_some((s, 0)))
        .unwrap();
    lib.select(outside.0, outside.1);
    assert_eq!(lib.slots()[0], Some(outside), "the last one used, kept to hand");

    let inside = lib.slots()[1].unwrap();
    lib.select(inside.0, inside.1);
    assert_eq!(lib.slots()[0], Some(outside), "one already in a slot leaves it alone");
}

#[test]
fn a_slot_takes_up_its_brush_and_an_empty_one_takes_nothing() {
    let mut lib = Library::default();
    let seat = lib.slots()[3].unwrap();
    assert!(lib.take_slot(3));
    assert_eq!(lib.selected(), seat);
    assert!(!lib.take_slot(0), "nothing has overflowed yet");
    assert!(!lib.take_slot(SLOTS), "and there is no eleventh seat");
}

#[test]
fn assigning_a_brush_to_a_seat_clears_the_overflow_it_came_from() {
    let mut lib = Library::default();
    let outside = lib
        .sets()
        .iter()
        .enumerate()
        .find_map(|(s, set)| (set.name == "Splatter").then_some((s, 0)))
        .unwrap();
    lib.select(outside.0, outside.1);
    assert_eq!(lib.slots()[0], Some(outside));

    assert!(lib.assign_slot(5, outside));
    assert_eq!(lib.slots()[5], Some(outside));
    assert_eq!(lib.slots()[0], None, "it is no longer outside the slots");
    assert!(!lib.assign_slot(0, outside), "slot 0 is computed, never assigned");
}

#[test]
fn the_slots_go_to_disk_by_name_and_come_back() {
    let mut lib = Library::default();
    let outside = lib
        .sets()
        .iter()
        .enumerate()
        .find_map(|(s, set)| (set.name == "Splatter").then_some((s, 2)))
        .unwrap();
    lib.assign_slot(2, outside);
    let edits = lib.edits();
    assert_eq!(edits.slots.len(), SLOTS);
    let held = edits.slots[2].clone().expect("seat 2 names its brush");
    assert_eq!(held.set, lib.sets()[outside.0].name);
    assert_eq!(held.name, lib.sets()[outside.0].presets[outside.1].name);

    let mut fresh = Library::default();
    fresh.apply(&edits);
    assert_eq!(fresh.slots(), lib.slots());
}

#[test]
fn edits_written_before_slots_existed_open_with_the_shipped_nine() {
    let mut lib = Library::default();
    let before: Edits = serde_json::from_str("{}").unwrap();
    assert!(before.slots.is_empty(), "absent on disk");
    lib.apply(&before);
    assert_eq!(lib.slots(), Library::default().slots());
}

#[test]
fn a_slot_naming_a_brush_this_build_dropped_opens_empty() {
    let mut lib = Library::default();
    let mut slots = vec![None; SLOTS];
    slots[4] = Some(Held { set: "Gone".into(), name: "Vanished".into() });
    lib.apply(&Edits { slots, ..Edits::default() });
    assert_eq!(lib.slots()[4], None, "passed over, not refused");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test brush:: 2>&1 | tail -20`
Expected: compile errors — `SLOTS`, `SLOT_DEFAULTS`, `slots`, `take_slot`, `assign_slot`, `Edits.slots` not found.

- [ ] **Step 3: Implement**

Add beside `Held`:

```rust
/// How many seats the strip keeps. Nine defaults, and slot 0 — the
/// last brush used that is not in any of them.
pub const SLOTS: usize = 10;

/// What seats 1..=9 hold on a machine that has never been drawn on:
/// the shelf Sketchbook puts first, which covers pencil, marker,
/// airbrush, pen, ink, watercolour, blur and eraser.
pub const SLOT_DEFAULTS: [(&str, &str); SLOTS - 1] = [
    ("Basic", "Textured Pencil"),
    ("Basic", "Textured Marker"),
    ("Basic", "Pressure Airbrush"),
    ("Basic", "Technical Pen"),
    ("Basic", "80% Inking Pen"),
    ("Basic", "Textured Watercolor"),
    ("Basic", "Textured Inker"),
    ("Basic", "Natural Blur"),
    ("Basic", "Auto Eraser Soft"),
];
```

Add `slots: Vec<Option<(usize, usize)>>` to `Library`, seeded in the constructor by `seat(set, name)` over `SLOT_DEFAULTS` (index `n + 1`), with `slots[0] = None`. Add to `Edits`:

```rust
    /// The ten seats, by name. Empty when the strip is the shipped
    /// nine, so a file written before slots existed opens meaning what
    /// it meant.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<Option<Held>>,
```

Methods on `Library`:

```rust
    pub fn slots(&self) -> &[Option<(usize, usize)>] {
        &self.slots
    }

    /// Takes up the brush in a seat. False when the seat is empty, so
    /// `app` knows nothing happened and writes nothing back.
    pub fn take_slot(&mut self, n: usize) -> bool {
        let Some(Some(at)) = self.slots.get(n).copied() else {
            return false;
        };
        self.select(at.0, at.1);
        true
    }

    /// Seats a brush in one of the nine. Slot 0 is computed from what
    /// the hand has been reaching for and is never written to.
    pub fn assign_slot(&mut self, n: usize, at: (usize, usize)) -> bool {
        if n == 0 || n >= SLOTS || !self.holds(at) {
            return false;
        }
        self.slots[n] = Some(at);
        if self.slots[0] == Some(at) {
            self.slots[0] = None;
        }
        true
    }

    fn holds(&self, (set, index): (usize, usize)) -> bool {
        self.sets.get(set).is_some_and(|s| index < s.presets.len())
    }

    /// Whether a brush is already within reach of the numbered seats.
    fn seated(&self, at: (usize, usize)) -> bool {
        self.slots[1..].iter().any(|s| *s == Some(at))
    }
```

Extend `select` so the overflow follows the hand:

```rust
    pub fn select(&mut self, set: usize, index: usize) {
        if !self.holds((set, index)) {
            return;
        }
        self.selected = (set, index);
        // Slot 0 is the way back to whatever was last reached for that
        // no numbered seat already holds.
        if !self.seated((set, index)) {
            self.slots[0] = Some((set, index));
        }
    }
```

Extend `edits()` to write the seats and `apply()` to read them:

```rust
            slots: self
                .slots
                .iter()
                .map(|seat| {
                    seat.map(|(s, i)| Held {
                        set: self.sets[s].name.clone(),
                        name: self.sets[s].presets[i].name.clone(),
                    })
                })
                .collect(),
```

```rust
        // An empty list is a file written before seats existed: the
        // shipped nine stand. A name this build no longer carries
        // leaves its seat empty, as a brush that has gone is one the
        // person can no longer be holding either.
        if !edits.slots.is_empty() {
            let mut slots = vec![None; SLOTS];
            for (n, seat) in edits.slots.iter().take(SLOTS).enumerate() {
                slots[n] = seat.as_ref().and_then(|h| self.seat(&h.set, &h.name));
            }
            self.slots = slots;
        }
```

Note ordering inside `apply`: read the seats **before** `held`, since `select` is not what seats them and `held` must win the hand.

- [ ] **Step 4: Run the tests**

Run: `cargo test brush:: 2>&1 | tail -20`
Expected: PASS, and the pre-existing `brush::` tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/brush.rs
git commit -m "brush: ten seats for the brushes a hand keeps reaching for"
```

---

### Task 2: The strip — `src/slots.rs`

**Files:**
- Create: `src/slots.rs`
- Modify: `src/main.rs` (add `mod slots;`)
- Test: `src/slots.rs` inline `mod tests`

**Interfaces:**
- Consumes: `Library::slots()` from Task 1; `Preset.icon`, `Set.name` from `brush`; `palette::icon_uv`, which this task makes `pub(crate)` — a one-word change to `palette.rs` that breaks nothing.
- Produces:
  - `pub const WIDTH: f32 = 132.0;` `pub const MARGIN: f32 = 12.0;`
  - `pub enum Hit { Slot(usize), Properties, Library, Panel }`
  - `pub struct Seat { pub n: usize, pub rect: ScreenRect }`
  - `pub struct Strip { pub rect, pub header, pub properties, pub library, pub seats: Vec<Seat>, .. }`
  - `Strip::layout(viewport: Viewport, scale: f64, top: f32) -> Strip`
  - `Strip::hit(&self, x: f64, y: f64) -> Option<Hit>`
  - `Strip::prims(&self, sets: &[Set], selected: (usize, usize), slots: &[Option<(usize, usize)>], brush: &Brush, over: Option<usize>, atlas: &Atlas, slot: u32, icons: u32, theme: &Theme) -> Vec<Prim>` — `over` is the seat a dragged brush is hovering, which stands out.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_strip_stands_at_the_left_with_ten_seats_under_its_header() {
    let s = strip(TALL, 1.0);
    assert_eq!(s.rect.x, MARGIN);
    assert_eq!(s.rect.y, TOP + MARGIN);
    assert_eq!(s.rect.w, WIDTH);
    assert_eq!(s.seats.len(), SLOTS, "nine defaults and the overflow");
    assert_eq!(s.seats[0].n, 1, "the numbered nine come first");
    assert_eq!(s.seats[SLOTS - 1].n, 0, "and slot 0 stands at the foot");
    for pair in s.seats.windows(2) {
        assert!(pair[1].rect.y > pair[0].rect.y, "one under the next");
        assert_eq!(pair[1].rect.x, pair[0].rect.x);
    }
    assert!(s.seats[0].rect.y >= s.header.y + s.header.h, "clear of the header");
}

#[test]
fn hit_reports_a_seat_the_two_buttons_and_the_panel() {
    let s = strip(TALL, 1.0);
    let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));
    let seat = s.seats[2];
    let (x, y) = mid(seat.rect);
    assert_eq!(s.hit(x, y), Some(Hit::Slot(seat.n)));
    let (x, y) = mid(s.properties);
    assert_eq!(s.hit(x, y), Some(Hit::Properties));
    let (x, y) = mid(s.library);
    assert_eq!(s.hit(x, y), Some(Hit::Library));
    let (x, y) = mid(s.header);
    assert_eq!(s.hit(x, y), Some(Hit::Panel), "the name is no button");
    assert_eq!(s.hit(2.0, 2.0), None, "outside is canvas");
}

#[test]
fn layout_scales_with_the_display() {
    let s = strip(TALL, 2.0);
    assert_eq!(s.rect.x, MARGIN * 2.0);
    assert_eq!(s.rect.w, WIDTH * 2.0);
    assert_eq!(s.seats[0].rect.h, CELL * 2.0);
}

#[test]
fn a_short_window_keeps_the_header_and_cuts_the_column() {
    let s = strip(Viewport { w: 900, h: 260 }, 1.0);
    assert!(s.rect.h <= 260.0 - TOP - MARGIN, "the strip fits the window");
    assert!(s.rect.contains_rect(&s.header), "the header is never cut");
}

#[test]
fn prims_paint_the_panel_the_seats_and_the_brush_in_hand() {
    let theme = Theme::light();
    let atlas = atlas();
    let lib = Library::default();
    let s = strip(TALL, 1.0);
    let held = lib.slots()[1].unwrap();
    let prims = s.prims(
        lib.sets(), held, lib.slots(), lib.brush(), None,
        &atlas, 7, 9, &theme,
    );

    assert!(prims[0].feather > 0.0, "the soft shadow goes first");
    assert!(
        prims.iter().any(|q| q.color == theme.panel && q.bounds() == s.rect),
        "panel body"
    );
    let ringed: Vec<ScreenRect> = prims
        .iter()
        .filter(|q| q.color == theme.selection)
        .map(|q| q.bounds())
        .collect();
    assert_eq!(ringed.len(), 1, "only the brush in hand is ringed");
    let seat = s.seats.iter().find(|q| q.n == 1).unwrap();
    assert!(seat.rect.contains_rect(&ringed[0]));

    let sprites = prims.iter().filter(|q| q.slot == 9).count();
    assert_eq!(sprites, SLOTS - 1 + 1, "a sprite per filled seat, and the header's");
}

#[test]
fn an_empty_seat_draws_its_number_and_no_icon() {
    let theme = Theme::light();
    let atlas = atlas();
    let lib = Library::default();
    let s = strip(TALL, 1.0);
    let prims = s.prims(
        lib.sets(), lib.selected(), lib.slots(), lib.brush(), None,
        &atlas, 7, 9, &theme,
    );
    let zero = s.seats.iter().find(|q| q.n == 0).unwrap();
    assert!(
        !prims.iter().any(|q| q.slot == 9 && zero.rect.contains_rect(&q.bounds())),
        "slot 0 is empty until something overflows into it"
    );
}

#[test]
fn the_seat_a_drag_is_over_stands_out() {
    let theme = Theme::light();
    let atlas = atlas();
    let lib = Library::default();
    let s = strip(TALL, 1.0);
    let plain = s.prims(lib.sets(), lib.selected(), lib.slots(), lib.brush(), None, &atlas, 7, 9, &theme);
    let over = s.prims(lib.sets(), lib.selected(), lib.slots(), lib.brush(), Some(4), &atlas, 7, 9, &theme);
    assert!(over.len() > plain.len(), "the seat under the pointer is marked");
}

#[test]
fn nothing_the_strip_draws_escapes_it() {
    let theme = Theme::light();
    let atlas = atlas();
    let lib = Library::default();
    let s = strip(Viewport { w: 900, h: 300 }, 1.0);
    let prims = s.prims(lib.sets(), lib.selected(), lib.slots(), lib.brush(), None, &atlas, 7, 9, &theme);
    let room = s.rect.inset(-2.0);
    for q in prims.iter().skip(1) {
        assert!(
            room.contains_rect(&q.bounds()) || q.clip != scene::NO_CLIP,
            "{:?} is loose outside the strip",
            q.bounds()
        );
    }
}
```

Helpers, mirroring `palette`'s:

```rust
    const VP: Viewport = Viewport { w: 900, h: 900 };
    const TOP: f32 = 34.0;
    const TALL: Viewport = Viewport { w: 900, h: 1200 };

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    fn strip(vp: Viewport, scale: f64) -> Strip {
        Strip::layout(vp, scale, TOP)
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test slots:: 2>&1 | tail -20`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the module**

Constants (logical px): `WIDTH 132`, `MARGIN 12`, `PADDING 8`, `RADIUS 12`, `HEADER_ICON 34`, `LABEL_GAP 8`, `DAB_ROW 10`, `ROW_GAP 6`, `BUTTON 22`, `CELL 32`, `SLOT_ICON 26`, `CELL_RADIUS 6`, `CELL_INSET 1`, `HELD_RING 1.5`, `NUMBER_INSET 3`, `SHADOW_OFFSET 3`, `SHADOW_FEATHER 14`, `ICON_BOX 15`, `ICON_STROKE 1.5`.

`layout` builds, from `y = top + MARGIN * s`:
- `header` = the icon row (`HEADER_ICON`) plus `ROW_GAP + DAB_ROW + ROW_GAP + BUTTON`, inset by `PADDING`.
- `properties` = `BUTTON` square at the header's left on the button row; `library` = the same square at its right.
- the column starts at `header.y + header.h + ROW_GAP`; ten `Seat`s of `CELL`, numbered `1..=9` then `0`.
- `band` = as much of the column as the window has room for (`viewport.h - (MARGIN + PADDING) * s - column_y`), and `rect.h` stops there, so a short window cuts the column and never the header.

`hit` answers `Properties`, `Library`, then a `Seat` whose rect contains the point **and** lies in the band, else `Panel`; `None` outside `rect`.

`prims` paints shadow, border, body (`Theme::corner(RADIUS, s)`, `Theme::edge(s)`), then:
- the header's sprite for the held brush's icon, its name in `theme.ink` and its shelf in `theme.muted`, both `atlas.truncate`d to the room left beside the icon;
- the dab, `Prim::soft_segment` across the header at the brush's own opacity — the same `dab(brush)` helper `palette` uses, moved here (see Task 3);
- `icon_prims(SLIDERS, ...)` and `icon_prims(CHEVRON, ...)` in `theme.icon`;
- per seat, clipped to the band: the ring when it holds the brush in the hand (`theme.selection` then `theme.active_bg` inside it, as the grid does), a `theme.active_bg` rounded box when `over == Some(n)`, the brush's sprite when the seat is filled, and the seat's digit in `theme.muted` at the bottom-right corner.

`CHEVRON` is `palette`'s pointing right; reuse `props`'s chevron points if it is already right-facing, else write the two-segment `>` explicitly.

- [ ] **Step 4: Run the tests**

Run: `cargo test slots:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/slots.rs src/main.rs
git commit -m "slots: the strip that names the brush and keeps ten to hand"
```

---

### Task 3: `palette` sheds its preview and becomes the library

**Files:**
- Modify: `src/palette.rs`
- Test: `src/palette.rs` inline `mod tests`

**Interfaces:**
- Produces:
  - `Palette::layout(viewport: Viewport, scale: f64, top: f32, left: f32, sets: &[Set], scroll: f32) -> Palette` — `left` is the panel's own x in physical px, since it no longer stands at `MARGIN`.
  - `Palette::prims(&self, sets: &[Set], selected: (usize, usize), atlas: &Atlas, slot: u32, icons: u32, theme: &Theme) -> Vec<Prim>` — no `brush`.
  - `Hit` loses `Properties` and `Reset`.

- [ ] **Step 1: Change the tests first**

Delete `the_preview_names_the_brush_and_shows_what_it_lays`. Fix `hit_reports_the_brush_the_buttons_and_the_panel` (rename to `hit_reports_the_brush_and_the_panel`, drop the two button assertions). Fix `prims_paint_the_panel_the_icons_and_the_brush_in_hand`'s sprite count to `p.cells.len()` — the preview's extra sprite is gone. Add:

```rust
#[test]
fn the_library_is_the_shelves_alone() {
    let (lib, p) = palette(TALL, 1.0, 0.0);
    let theme = Theme::light();
    let prims = p.prims(lib.sets(), (0, 0), &atlas(), 7, 9, &theme);
    assert!(
        !prims.iter().any(|q| q.kind == KIND_SEGMENT),
        "the dab and the buttons went to the strip"
    );
    assert!(p.band.y <= p.rect.y + PADDING + 1.0, "the shelves start at the top");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test palette:: 2>&1 | tail -20`
Expected: FAIL — arity mismatch on `prims`, `Hit::Properties` gone.

- [ ] **Step 3: Implement**

Delete `preview_prims`, `dab` (move to `slots`), `SLIDERS`, `RESET`, `icon_prims`, and the `preview`/`reset`/`properties` fields plus `PREVIEW`, `BUTTON`, `BUTTON_GAP`, `PREVIEW_ICON`, `LABEL_GAP`, `DAB_MAX`, `DAB_MIN` constants. `band_y` becomes `y + PADDING * s`. Drop `Hit::Properties` and `Hit::Reset`. Make `icon_uv` `pub(crate)`. Drop the now-unused `Brush` import.

- [ ] **Step 4: Run the tests**

Run: `cargo test palette:: 2>&1 | tail -20`
Expected: PASS. `cargo build` will still fail on `app.rs` — that is Task 5.

- [ ] **Step 5: No commit yet**

`Palette::prims` and `Palette::layout` change arity, so `app.rs` does not compile until Task 5 rewires it. Tasks 3, 4 and 5 land as **one commit** — the repo's commits are atomic *and* buildable, and there is no smaller cut of this that builds.

---

### Task 4: The reset button moves to the properties bar

**Files:**
- Modify: `src/props.rs`
- Test: `src/props.rs` inline `mod tests`

**Interfaces:**
- Produces: `Hit::Reset`, `Props.reset: ScreenRect`. `Props::prims` unchanged in arity — it already takes `edited: bool` and draws the button only then.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_bar_carries_the_arrow_that_takes_an_edit_back() {
    let bar = Props::layout(VP, 1.0, TOP, false); // VP and TOP are props.rs's own test consts
    let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));
    let (x, y) = mid(bar.reset);
    assert_eq!(bar.hit(x, y), Some(Hit::Reset));
    assert!(bar.reset.x > bar.name.x + bar.name.w, "after the name and its dot");
    assert!(bar.reset.x + bar.reset.w < bar.fields[0].label.x, "and clear of the sliders");
}

#[test]
fn the_arrow_is_drawn_only_while_there_is_an_edit_to_take_back() {
    let theme = Theme::light();
    let atlas = atlas();
    let brush = *Library::default().brush();
    let bar = Props::layout(VP, 1.0, TOP, false); // VP and TOP are props.rs's own test consts
    let plain = bar.prims("Textured Pencil", false, &brush, &atlas, 7, &theme);
    let edited = bar.prims("Textured Pencil", true, &brush, &atlas, 7, &theme);
    let inside = |list: &[Prim]| {
        list.iter().filter(|q| bar.reset.contains_rect(&q.bounds())).count()
    };
    assert_eq!(inside(&plain), 0, "nothing to take back, nothing drawn");
    assert!(inside(&edited) > 0, "the arrow appears beside the dot");
}

#[test]
fn the_bar_is_the_same_width_edited_or_not() {
    let a = Props::layout(VP, 1.0, TOP, false);
    let b = Props::layout(VP, 1.0, TOP, true);
    assert_eq!(a.bar.w, b.bar.w, "the line must not jump when a slider moves");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test props:: 2>&1 | tail -20`
Expected: FAIL — `Hit::Reset` and `Props.reset` not found.

- [ ] **Step 3: Implement**

Add `const RESET: f32 = 20.0;` and a `MARK` run of `DOT + GAP + RESET` after the name. `closed_w` becomes `PADDING + NAME_W + GAP + MARK + GAP + 2.0 * FIELD_W + GAP + TOGGLE + PADDING`; the room is reserved whether or not there is an edit, so the line never jumps. The closed fields' `fx` starts after the mark. `reset` is the `RESET` square at the mark's right, vertically centered in `bar`. `hit` answers `Hit::Reset` before `Hit::Bar`; it answers whether or not the brush is edited, which costs nothing — resetting an untouched brush is already a no-op, and the bar swallows the click either way. `prims` draws the `RESET` icon lines (moved from `palette`) in `theme.icon` only when `edited`.

- [ ] **Step 4: Run the tests**

Run: `cargo test props:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: No commit yet** — lands with Tasks 3 and 5 (see Task 3, Step 5).

---

### Task 5: `app` lays out both panels and routes them

**Files:**
- Modify: `src/app.rs`
- Modify: `src/main.rs` (`mod slots;` if not already added in Task 2)

**Interfaces:**
- Consumes: everything produced by Tasks 1–4.
- Produces: `App.library_shown: bool`, `App::strip(&self, view) -> Option<Strip>`, `App::library(&self, view) -> Option<Palette>` (renamed from `palette`).

- [ ] **Step 1: Rename and add the state**

`palette_shown` stays (it is the strip's own switch, and `Shift+B` keeps its meaning); add `library_shown: bool`, initialised `false`. `App::palette` becomes `App::library`, guarded by `self.palette_shown && self.library_shown && tool == Brush`, and laid out at `x = strip.rect.x + strip.rect.w + palette::MARGIN * chrome`. Add `App::strip`, guarded by `self.palette_shown && tool == Brush`, hanging off the props bar exactly as the palette did.

`Palette::layout` takes `left: f32` (Task 3), which `app` fills with `strip.rect.x + strip.rect.w + palette::MARGIN * chrome`.

- [ ] **Step 2: Route the hits**

`over_chrome` gains `self.strip(view)...`. In `pointer_pressed`, the strip is hit **before** the library (it stands to its left and cannot overlap, so order is only for tidiness) and both before the dock. New:

```rust
    /// A click on the brush strip.
    fn strip_hit(&mut self, hit: slots::Hit) {
        match hit {
            slots::Hit::Slot(n) => {
                if !self.brushes.take_slot(n) {
                    return;
                }
            }
            slots::Hit::Properties => self.props_open = !self.props_open,
            slots::Hit::Library => {
                self.library_shown = !self.library_shown;
                return;
            }
            slots::Hit::Panel => return,
        }
        self.brushes_dirty = true;
    }
```

`palette_hit` loses `Properties` and `Reset`, keeping only `Brush` and `Panel`. `props_hit` gains `Hit::Reset => self.brushes.reset()`.

- [ ] **Step 3: Draw both**

In `frame`, draw the library (unchanged call minus `brush`) and then the strip:

```rust
        if let (Some(strip), Some(atlas)) = (self.strip(view), self.atlas.as_ref()) {
            frame.extend(strip.prims(
                self.brushes.sets(),
                self.brushes.selected(),
                self.brushes.slots(),
                self.brushes.brush(),
                self.drag.as_ref().and_then(|d| d.over),
                atlas,
                self.atlas_slot,
                self.icon_slot,
                &self.theme,
            ));
        }
```

`self.drag` arrives in Task 7; until then pass `None` and add the field there.

- [ ] **Step 4: Keep the wheel and the glide on the library**

`scrolled` and `show_brush` (`palette_scroll`, `scroll_showing`) now go through `self.library(...)`; both become no-ops while the library is shut, which is right.

- [ ] **Step 5: Build and run the suite**

Run: `cargo build 2>&1 | grep -E "^(error|warning)" ; cargo test 2>&1 | tail -5`
Expected: no errors, no warnings, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/palette.rs src/props.rs src/app.rs src/main.rs
git commit -m "palette, props, app: the library moves aside for the strip"
```

---

### Task 6: `Shift`+digit takes a slot

**Files:**
- Modify: `src/app.rs` (the `KeyboardInput` arm ~line 2292, `key`, `plain_key`)

**Interfaces:**
- Consumes: `Library::take_slot` from Task 1.
- Produces: nothing other modules read.

- [ ] **Step 1: Read the key without its modifiers**

`Shift+1` is a different character on every layout, so the digit is read through winit's `key_without_modifiers()` rather than the character that arrives. Import:

```rust
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
```

Bind the whole event and filter repeats with a guard rather than a pattern:

```rust
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                self.key(&event.logical_key, event.key_without_modifiers(), event.state);
            }
```

`key` takes `bare: Key` and hands it to `plain_key`; `plain_key` gains a `bare` argument and, before anything else:

```rust
        // A seat is numbered, and the number is the key. `Shift+1` is a
        // different character on every layout, so the digit is read off
        // the key without its modifiers rather than off what arrived.
        if shift
            && let Key::Character(text) = &bare
            && let Ok(n) = text.parse::<usize>()
            && self.editor().tool() == Tool::Brush
            && self.brushes.take_slot(n)
        {
            self.brushes_dirty = true;
            self.keep_brushes();
            self.redraw();
            return;
        }
```

`text.parse::<usize>()` accepts exactly one digit's worth here because `Key::Character` for a digit key is a single char; `n >= SLOTS` is refused by `take_slot`.

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | grep -E "^(error|warning)"`
Expected: silent.

- [ ] **Step 3: Check it by hand**

Run the smoke build, press `Shift+3`, confirm the strip's ring moves to seat 3 and the properties bar renames.

```sh
XDG_DATA_HOME=/tmp/omawhite-slots cargo run -- --socket /tmp/omawhite-slots.sock
```

- [ ] **Step 4: Commit**

```bash
git add src/app.rs
git commit -m "app: Shift and a digit reach the seat it numbers"
```

---

### Task 7: Dragging a brush from the library into a seat

**Files:**
- Modify: `src/app.rs`

**Interfaces:**
- Consumes: `Library::assign_slot` (Task 1), `slots::Strip::hit` (Task 2), `palette::Hit::Brush` (Task 3).
- Produces: `App.drag: Option<Dragging>`.

- [ ] **Step 1: The state**

```rust
/// A brush on its way from the library to a seat. The press may still
/// turn out to be a click, so nothing is carried until the pointer has
/// moved past the slop — and the click is what happens if it never does.
struct Dragging {
    at: (usize, usize),
    icon: u16,
    /// Where the press landed, in physical px.
    from: (f32, f32),
    /// Where the pointer is now.
    to: (f32, f32),
    /// Past the slop: the icon is in the hand.
    carried: bool,
    /// The seat under the pointer, if it is one that can be written.
    over: Option<usize>,
}

/// How far a press has to travel before it is a drag and not a click.
const DRAG_SLOP_PX: f64 = 4.0;
```

- [ ] **Step 2: The press starts it**

In the library's branch of `pointer_pressed`, after `palette_hit`:

```rust
                if let palette::Hit::Brush(set, index) = hit
                    && let Some(cell) = pal.cells.iter().find(|c| (c.set, c.index) == (set, index))
                {
                    self.drag = Some(Dragging {
                        at: (set, index),
                        icon: cell.icon,
                        from: (x as f32, y as f32),
                        to: (x as f32, y as f32),
                        carried: false,
                        over: None,
                    });
                }
```

The brush is taken up at the press, as it is today — a drag that ends nowhere still leaves the brush in the hand, which is what a click would have done.

- [ ] **Step 3: The move carries it**

At the head of `pointer_moved`, before the canvas sees anything:

```rust
        if let Some(drag) = &mut self.drag {
            drag.to = (x as f32, y as f32);
            let far = f64::from(drag.to.0 - drag.from.0).hypot(f64::from(drag.to.1 - drag.from.1));
            drag.carried |= far > DRAG_SLOP_PX * self.chrome(&view);
            drag.over = drag.carried.then(|| {
                self.strip(&view).and_then(|s| match s.hit(x, y) {
                    // Slot 0 is computed from what the hand reaches
                    // for; it is not a place to put a brush.
                    Some(slots::Hit::Slot(n)) if n != 0 => Some(n),
                    _ => None,
                })
            }).flatten();
            self.redraw();
            return self.update_cursor_icon();
        }
```

- [ ] **Step 4: The release writes it**

At the head of `pointer_released`:

```rust
        if let Some(drag) = self.drag.take() {
            if let Some(n) = drag.over
                && self.brushes.assign_slot(n, drag.at)
            {
                self.brushes_dirty = true;
                self.keep_brushes();
            }
            self.redraw();
            return self.update_cursor_icon();
        }
```

`focus_lost` drops `self.drag` too, the way it lets go of a carried card.

- [ ] **Step 5: Draw the carried icon**

Last in `frame`, over every panel:

```rust
        if let Some(drag) = self.drag.as_ref().filter(|d| d.carried) {
            let side = palette::ICON * self.chrome(view);
            frame.push(Prim::sprite(
                ScreenRect {
                    x: drag.to.0 - side / 2.0,
                    y: drag.to.1 - side / 2.0,
                    w: side,
                    h: side,
                },
                palette::icon_uv(drag.icon),
                self.icon_slot,
            ));
        }
```

- [ ] **Step 6: Build, test, and check by hand**

Run: `cargo build 2>&1 | grep -E "^(error|warning)" ; cargo test 2>&1 | tail -5`
Then run the app, open the library with the chevron, drag a brush from a far shelf onto seat 6, and confirm: the seat's icon changes, the change survives a restart, and dropping on seat 0 changes nothing.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs
git commit -m "app: a brush carried out of the library into a seat"
```

---

### Task 8: The documentation catches up

**Files:**
- Modify: `README.md` (the brush palette bullet ~line 146, the keys table ~line 446, the module list ~line 527)
- Modify: `AGENTS.md` (the brushes paragraph — `CLAUDE.md` is a symlink to it, edit `AGENTS.md`)

- [ ] **Step 1: README**

Replace the "Brush palette" bullet with two: the strip (header, the two buttons, the ten seats, what slot 0 means, `Shift`+digit, dragging one in) and the library (the shelves, the scroll, that the chevron opens it beside the strip). Add `Shift` + `1`–`9`/`0` and "Drag a brush onto a seat" to the keys table. Add `src/slots.rs` to the module list and reword `src/palette.rs`. Note the reset button's new home in the Brush properties bullet.

- [ ] **Step 2: AGENTS.md**

In the brushes paragraph, record: the seats are named not numbered, for the reason `Edits` already gives; slot 0 is computed by `Library::select` and never assigned; `Edits.slots` absent means the shipped nine; the strip is the brush tool's own chrome and the library opens from inside it, so hiding the strip hides both.

- [ ] **Step 3: Verify and commit**

Run: `cargo test 2>&1 | tail -3 ; cargo build 2>&1 | grep -c warning`
Expected: all pass, zero warnings.

```bash
git add README.md AGENTS.md
git commit -m "docs: the strip, the seats, and the library beside them"
```
