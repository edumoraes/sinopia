# Brush and Layers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A brush tool (`B`) with size, opacity and hardness, painting on layers the user picks, shows, hides, adds, removes and reorders from a panel on the right that `Shift+L` toggles.

**Architecture:** The brush writes the existing `path` element with two new optional fields; soft or translucent strokes are composited as one shape through an offscreen scratch texture (`scene::passes` plans it, `gfx` executes it). Layers are additive document structure (`Document.layers`, an element's `layer`), read through `Document::painted()` by both the renderer and the pointer. The panel is a pure module in the dock's shape; `app` routes clicks and keys.

**Tech Stack:** Rust 2024, winit 0.30, wgpu 30, serde. Tests are inline `#[cfg(test)]` modules; `cargo test <module>::` runs one.

**Spec:** `docs/superpowers/specs/2026-09-02-brush-and-layers-design.md`

## Global Constraints

- Code, comments, docs and commits in English; commits atomic, one slice each.
- Pure modules carry the tests; `app` and `gfx` stay logic-free shells.
- Schema stays 1: `layers`, `layer`, `opacity`, `hardness` all default; boards written before this load unchanged.
- The parse stays closed: unknown layer id, duplicate layer id, `opacity`/`hardness` outside 0–1 are errors.
- Logical px for chrome sizes, physical px for positions; world unit = logical px at zoom 1.
- `cargo build` warning-free, `cargo test` green after every task.

---

### Task 1: `doc` — layers, the element's layer, opacity and hardness

**Files:**
- Modify: `src/doc.rs`
- Modify (struct literals gain `layer`): `src/editor.rs`, `src/select.rs`, `src/scene.rs`, `src/store.rs` tests

**Interfaces:**
- Produces:
  - `pub struct Layer { pub id: String, pub name: String, pub visible: bool }` (serde: `visible` default true, skipped when true).
  - `Document.layers: Vec<Layer>` (serde default; normalized non-empty by `from_json`; `Document::new` makes `Layer 1`).
  - `Rect/Path/Image.layer: String` (serde default; `from_json` fills the first layer's id when empty); `Element::layer(&self) -> &str`, `Element::set_layer(&mut self, id: &str)`.
  - `Path.opacity: f64`, `Path.hardness: f64` (default 1, skipped when 1; `TryFrom<PathOnDisk>` rejects values outside 0..=1).
  - `Document::painted(&self) -> impl Iterator<Item = (usize, &Element)>` — bottom layer first, document order within, hidden layers skipped.
  - `Document::layer_index(&self, id: &str) -> Option<usize>`.
  - `Document::add_layer(&mut self, above: usize) -> usize` — inserts at `above + 1`, name `Layer N`, N one past the highest `Layer k` in use, returns the new index.
  - `Document::remove_layer(&mut self, index: usize) -> bool` — false for the last layer or a bad index; drops the layer's elements.
  - `Document::move_layer(&mut self, index: usize, up: bool) -> Option<usize>` — swaps with the neighbour, None at the edge.

- [ ] **Step 1: Failing tests in `doc::tests`**
  - `new_document_has_one_visible_layer_named_layer_1` (id is a 26-char ULID).
  - `a_board_without_layers_gets_one_and_its_elements_join_it` — the §6.1 example parses; `layers.len() == 1`, element's `layer` equals `layers[0].id`.
  - `layers_and_element_layers_roundtrip` — two layers, one hidden; JSON has `"visible": false` only on the hidden one; `from_json(to_json)` equals.
  - `an_element_naming_an_unknown_layer_is_an_error` (message contains `layer`).
  - `duplicate_layer_ids_are_an_error`.
  - `path_opacity_and_hardness_default_to_one_and_stay_off_disk`.
  - `path_opacity_and_hardness_roundtrip` (0.5 / 0.25 → present in JSON, equal after reload).
  - `path_opacity_or_hardness_outside_the_unit_range_is_an_error` (1.5, -0.1 for each field).
  - `painted_walks_layers_bottom_up_and_skips_hidden_ones` — three layers, elements interleaved in `elements`; expect indices ordered by layer then position; hiding the middle layer drops its elements.
  - `add_layer_inserts_above_and_names_past_the_highest_number` — `Layer 1`, `Layer 2`; remove `Layer 2`; add above 0 → `Layer 3` at index 1.
  - `remove_layer_drops_its_elements_and_refuses_the_last`.
  - `move_layer_swaps_with_the_neighbour_and_stops_at_the_edge`.
- [ ] **Step 2: `cargo test doc::` fails to compile / fails.**
- [ ] **Step 3: Implement** the struct fields, `PathOnDisk`/`ImageOnDisk` gains `layer` (default) and `opacity`/`hardness` (default 1); `Document::from_json` normalizes (`layers` empty → one layer; element `layer` empty → first id; unknown → `bail!`; duplicates → `bail!`); `painted`, `layer_index`, `add_layer`, `remove_layer`, `move_layer`; a private `fn next_layer_name(&self) -> String`.
- [ ] **Step 4: Fix every `Element::...` struct literal** in the other modules' tests (`layer: String::new()` or a `LAYER` const) and the `Document::new` uses that build elements by hand — `cargo build --tests` guides. Where the editor pushes new elements (Task 5 replaces this), fill `layer` with `doc.layers[0].id.clone()` for now.
- [ ] **Step 5: `cargo test` green (all modules).**
- [ ] **Step 6: Commit** — `doc: layers, the layer an element is on, and a path's opacity and hardness`.

### Task 2: `brush` — settings, tip, ring

**Files:**
- Create: `src/brush.rs`; register in `src/main.rs` (`mod brush;`).

**Interfaces:**
- Produces:
  - `pub struct Brush { pub size: f64, pub opacity: f64, pub hardness: f64 }` + `Default` (16, 1.0, 0.5).
  - `pub const SIZE_MIN: f64 = 1.0; pub const SIZE_MAX: f64 = 500.0; pub const HARDNESS_STEP: f64 = 0.25;`
  - `impl Brush { pub fn grow(&mut self); pub fn shrink(&mut self); pub fn harder(&mut self); pub fn softer(&mut self); pub fn set_opacity_digit(&mut self, digit: u8); pub fn tip(&self) -> Tip; }`
  - `#[derive(Debug, Clone, Copy, PartialEq)] pub struct Tip { pub width: f64, pub opacity: f64, pub hardness: f64 }`; `Tip::PENCIL` (width `editor::PEN_WIDTH`, 1, 1) — define `PEN_WIDTH` here and re-export from editor, or keep it in editor and reference it; `Tip::of(path: &doc::Path) -> Tip`; `Tip::is_direct(&self) -> bool` (opacity ≥ 1 and hardness ≥ 1).
  - `pub fn size_step(size: f64) -> f64` — 1 below 10, 10 below 100, 25 below 200, 50 below 300, else 100.
  - `pub fn ring_prims(center: (f32, f32), radius: f32, scale: f32, color: Rgba) -> Vec<Prim>` — 48-point polyline circle, half-width `0.5 * scale`.

- [ ] **Step 1: Failing tests** — `grow_and_shrink_follow_photoshop_steps_within_bounds` (9→10→20…, 500 stays, 1 stays; shrink from 10 → 9; from 100 → 90); `harder_and_softer_step_a_quarter_and_clamp`; `opacity_digits_map_one_to_nine_and_zero` (1→0.1, 9→0.9, 0→1.0, other digits ignored); `tip_of_a_pencil_and_of_a_brush` (`Tip::PENCIL.is_direct()`, brush default not direct, brush with hardness 1 & opacity 1 direct); `tip_of_a_path_reads_its_fields`; `ring_is_a_closed_polyline_around_the_centre` (every prim is a segment, every endpoint at distance `radius` from centre within 1e-3, first and last meet).
- [ ] **Step 2: run, fail. Step 3: implement. Step 4: `cargo test brush::` green.**
- [ ] **Step 5: Commit** — `brush: settings, the tip a stroke carries, and the ring the pointer shows`.

### Task 3: `scene` — soft strokes, `Frame`, groups, `passes`

**Files:**
- Modify: `src/scene.rs` (make `Prim::bounds` always compiled; add `Prim::painted_bounds`, `Prim::composite`, `Frame`, `Group`, `Pass`, `passes`, tip-aware stroke functions; `document_prims` → `Frame`, iterating `doc.painted()`).
- Modify: callers in `src/app.rs` (`frame()` returns `Frame`; `gfx.render` takes `&frame.prims` until Task 4) and `src/dock.rs` tests if they used `bounds` under cfg(test) (unchanged).

**Interfaces:**
- Produces:
  - `pub fn soft_radius(width: f64, hardness: f64, view: &View) -> (f32, f32)` — `(radius_px, feather_px)`: `r = half_width_px(width)`, `f = (1 - h) * r`, radius `r - f/2`.
  - `pub fn path_prims(curves: &[Cubic], tip: Tip, color: Rgba, view: &View) -> Vec<Prim>`; `pub fn stroke_prims(points: &[[f64; 2]], tip: Tip, color: Rgba, view: &View) -> Vec<Prim>`; `polyline_prims` gains a `feather: f32` parameter (`Prim::segment` → `Prim::soft_segment(a, b, half_width, feather, color)`; the dot fallback uses `Prim::soft`).
  - `pub struct Frame { pub prims: Vec<Prim>, pub groups: Vec<Group> }` with `new`, `extend(&mut self, impl IntoIterator<Item = Prim>)`, `group(&mut self, prims: Vec<Prim>, opacity: f32)`, `stroke(&mut self, prims: Vec<Prim>, tip: Tip)`, `append(&mut self, other: Frame)`.
  - `pub struct Group { pub start: u32, pub end: u32, pub opacity: f32, pub bounds: ScreenRect }` — bounds = union of `painted_bounds` of the prims (empty group → no group pushed).
  - `Prim::painted_bounds(&self) -> ScreenRect` — `bounds` grown by `max(feather, 1) / 2 + 1`.
  - `Prim::composite(r: ScreenRect, viewport: Viewport, slot: u32, opacity: f32) -> Prim` — `KIND_IMAGE`, uv = r over the viewport, color `[opacity; 4]`.
  - `pub enum Pass { Direct { composite: Option<u32>, start: u32, end: u32 }, Offscreen { wipe: u32, start: u32, end: u32 } }`.
  - `pub fn passes(frame: &Frame, viewport: Viewport, scratch: u32) -> (Vec<Prim>, Vec<Pass>)` — appended prims: per kept group a wipe (`Prim::rect(bounds, [0.0; 4])`) and a composite; groups whose bounds (clamped to the viewport) are empty are dropped along with their prims (a `Direct` range never covers them). Always ends with a `Direct` pass (possibly empty range).
  - `pub fn document_prims(doc: &Document, view: &View, images: &ImageSlots) -> Frame`.
  - `ScreenRect::intersect(&self, other: &ScreenRect) -> Option<ScreenRect>`, `ScreenRect::union(&self, other: &ScreenRect) -> ScreenRect`.

- [ ] **Step 1: Failing tests** — `soft_radius_keeps_the_stroke_inside_its_width` (h=1 → (r, 0); h=0 → (r/2, r); h=0.5 → (0.75r, 0.5r)); `a_hard_opaque_path_is_direct_and_a_soft_one_is_a_group` (`document_prims` with a pencil path → no group; with hardness 0.5 → one group covering exactly its prims with opacity 1; with opacity 0.5 → opacity 0.5); `group_bounds_wrap_the_prims_and_their_ramp`; `frame_append_offsets_the_groups`; `passes_split_around_a_group` (prims `[a, b | g1, g2 | c]`, group over 2..4 → `Direct{None,0,2}`, `Offscreen{wipe: 5, 2, 4}`, `Direct{Some(6), 4, 5}`; appended prims: index 5 is the wipe rect with alpha 0, index 6 the composite with slot = scratch and color `[op; 4]`); `a_group_outside_the_viewport_is_dropped_with_its_prims`; `composite_samples_the_scratch_over_its_own_bounds` (uv arithmetic); `painted_order_puts_a_lower_layer_first_and_hides_a_hidden_one` (via `document_prims`); existing tests updated to `.prims`.
- [ ] **Step 2: run, fail. Step 3: implement. Step 4: `cargo test scene::` green; `cargo build` (app compiles with `frame.prims`).**
- [ ] **Step 5: Commit** — `scene: soft and translucent strokes composited as one shape; the frame's groups and passes`.

### Task 4: `gfx` — scratch, pipelines, executing passes

**Files:**
- Modify: `src/gfx.rs`, `src/app.rs` (`gfx.render(bg, &frame)`).

**Interfaces:**
- Consumes: `scene::passes`, `scene::Frame`, `Pass`.
- Produces: `Gfx::render(&mut self, background: Rgba, frame: &Frame) -> anyhow::Result<bool>`.

- [ ] **Step 1: Shader** — add `@fragment fn fs_premul(in: VsOut)` returning `vec4(rgba.rgb * rgba.a * coverage, rgba.a * coverage)`; factor the distance/coverage into `fn shade(in) -> vec4` used by both entry points.
- [ ] **Step 2: Pipelines** — `fn pipeline(device, layout, shader, format, fragment: &str, blend: Option<BlendState>) -> RenderPipeline`; build `direct` (fs_main, ALPHA_BLENDING), `composite` (fs_main, PREMULTIPLIED_ALPHA_BLENDING), `union` (fs_premul, `BlendComponent { One, One, Max }` for color and alpha), `wipe` (fs_premul, `None`).
- [ ] **Step 3: Scratch** — `scratch: Option<(u32, wgpu::TextureView)>`; `fn ensure_scratch(&mut self)` creates a `config.width × height` texture (format `config.format`, usage `RENDER_ATTACHMENT | TEXTURE_BINDING`), its bind group in the slot (replace in place like the atlas); called from `resize` and lazily in `render`. Not in `slots`.
- [ ] **Step 4: Execute** — `let (prims, passes) = scene::passes(frame, viewport, scratch_slot)`; one instance buffer; for each pass a render pass: `Direct` on the surface (first: clear to background; later: load) drawing the composite prim with `composite` + scratch group if present, then `scene::runs(&prims[start..end])` offset by `start` with `direct`; `Offscreen` on the scratch view (load), `wipe` pipeline for the wipe prim, `union` pipeline for the runs.
- [ ] **Step 5: Smoke** — `XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- --socket /tmp/omawhite-smoke.sock --smoke-frames 3` exits 0; `cargo test` green.
- [ ] **Step 6: Commit** — `gfx: a scratch texture and the passes that lay a stroke on the frame as one shape`.

### Task 5: `select`, `editor`, `dock` — layers under the pointer; the brush tool; the active layer

**Files:**
- Modify: `src/select.rs` (`element_at`, `elements_in` through `painted()`), `src/editor.rs`, `src/dock.rs` (icon), `src/app.rs` (compile: `press(..., &self.brush)`, `stroke()` shape, `Tool::Brush` cursor arm).

**Interfaces:**
- Produces:
  - `Tool::Brush`, `Tool::ALL = [Select, Hand, Pencil, Brush, Zoom]`, hotkey `'b'`.
  - `pub struct Stroke { pub points: Vec<[f64; 2]>, pub tip: Tip }`; `Editor::stroke(&self) -> Option<&Stroke>`.
  - `Editor::press(&mut self, button, view, screen, doc, brush: &Brush) -> Change`.
  - `Editor::active_layer(&self, doc: &Document) -> usize` (id resolved; topmost when None or gone).
  - `Editor::select_layer(&mut self, doc, index) -> Change`, `add_layer(&mut self, doc) -> Change`, `remove_layer(&mut self, doc) -> Change`, `toggle_layer(&mut self, doc, index) -> Change`, `move_layer(&mut self, doc, up: bool) -> Change`.
- Behaviour: release writes `Path { width: tip.width, opacity: tip.opacity, hardness: tip.hardness, layer: active id }`; `paste_image` sets `layer`; `select_press` on an element sets the active layer to that element's; `remove_layer` drops its ids from the selection; `toggle_layer` hiding deselects its elements.

- [ ] **Step 1: Failing tests** — select: `the_top_layer_wins_under_the_pointer`, `a_hidden_layer_is_neither_hit_nor_marqueed`; editor: `brush_records_the_tip_at_the_press_and_writes_it_into_the_path`, `pencil_strokes_stay_direct` (opacity/hardness 1), `ink_lands_on_the_active_layer` (add a layer → stroke → `layer` is the new id), `picking_an_element_activates_its_layer`, `add_layer_activates_the_new_layer_above_the_active_one`, `remove_layer_drops_its_elements_from_the_selection_and_activates_the_neighbour`, `remove_layer_keeps_the_last_layer`, `toggle_layer_deselects_what_it_hides`, `move_layer_swaps_and_keeps_the_active_id`, `hotkey_b_is_the_brush`; dock: `every_tool_has_an_icon_on_the_24_grid` covers Brush automatically; update `dock_order...` test.
- [ ] **Step 2: run, fail. Step 3: implement. Step 4: `cargo test` green, `cargo build` warning-free.**
- [ ] **Step 5: Commit** — `select, editor, dock: the brush tool, the active layer, and a pointer that respects layers`.

### Task 6: `layers` — the panel

**Files:**
- Create: `src/layers.rs`; register in `src/main.rs`.

**Interfaces:**
- Produces:
  - consts (logical px): `WIDTH = 200.0`, `MARGIN = 12.0`, `HEADER = 34.0`, `ROW = 30.0`, `PADDING = 6.0`, `BUTTON = 24.0`, `RADIUS = 12.0`, `EYE = 20.0`.
  - `pub enum PanelHit { Select(usize), Toggle(usize), Add, Remove, Up, Down, Panel }`.
  - `pub struct Row { pub index: usize, pub rect: ScreenRect, pub eye: ScreenRect, pub label: String, pub label_x: f32 }`.
  - `pub struct Panel { pub rect: ScreenRect, pub header: ScreenRect, pub rows: Vec<Row>, pub up: ScreenRect, pub down: ScreenRect, pub add: ScreenRect, pub remove: ScreenRect, scale: f32 }`.
  - `Panel::layout(viewport: Viewport, scale: f64, top: f32, atlas: &Atlas, layers: &[Layer]) -> Panel` — `top` is the strip's bottom in physical px; rows top layer first; rows past `viewport.h - MARGIN*s` dropped.
  - `Panel::hit(&self, x: f64, y: f64) -> Option<PanelHit>`.
  - `Panel::prims(&self, layers: &[Layer], active: usize, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim>`.

- [ ] **Step 1: Failing tests** — `panel_sits_below_the_strip_at_the_right_edge` (x = w - MARGIN - WIDTH, y = top + MARGIN, one header + n rows high); `rows_list_the_top_layer_first` (indices `[2, 1, 0]`); `rows_that_do_not_fit_are_dropped` (small viewport); `hit_names_the_row_the_eye_and_the_buttons`; `prims_highlight_the_active_row_and_dim_a_hidden_layer` (active_bg box on the active row; hidden layer's eye in `theme.border`... assert a segment prim in that colour within the eye rect); `layout_scales_with_the_display`.
- [ ] **Step 2: run, fail. Step 3: implement** (icons as polylines on the 24-grid like `dock::icon`; eye: an almond of 12 points + a pupil circle; hidden: same in `theme.border` plus a slash). **Step 4: green.**
- [ ] **Step 5: Commit** — `layers: the panel — rows for the layers, an eye each, and the four buttons`.

### Task 7: `app` — keys, routing, the panel and the ring in the frame

**Files:**
- Modify: `src/app.rs`.

- [ ] **Step 1: State** — `brush: Brush`, `layers_shown: bool`; `fn panel(&self, view) -> Option<Panel>` (None when hidden or no atlas; `top = Tabs::HEIGHT * scale`).
- [ ] **Step 2: Keys** — in `key()`: `Key::Character` with Shift and no Ctrl/Alt/Super: `"l"` → toggle panel + redraw; with the brush tool selected: `[`/`]` → `brush.shrink()/grow()`, `{`/`}` → `softer()/harder()`, digits → `set_opacity_digit`, each followed by `redraw()` (the ring changes). Tool hotkeys unchanged otherwise.
- [ ] **Step 3: Routing** — `pointer_pressed`: strip, then panel (`PanelHit` → editor call → `apply`; `Panel` swallowed), then dock, then canvas with `&self.brush`. `update_cursor_icon`: panel counts as chrome; `Tool::Brush` → `Crosshair`.
- [ ] **Step 4: Frame** — grid → `document_prims` frame → `frame.stroke(stroke_prims(..), tip)` → selection → marquee → ring (`pointer_tool == Brush`, cursor over the canvas, radius `brush.size / 2 * px_per_world`) → dock → panel → strip.
- [ ] **Step 5: Smoke + screenshot** — smoke run exits 0; launch with a temp `XDG_DATA_HOME`, press `b`, drag a stroke, `Shift+L`, capture with `grim` per the memory note; confirm the stroke composites without beads and the panel shows.
- [ ] **Step 6: Commit** — `app: B paints with the brush, Shift+L shows the layers, and the panel's clicks reach the editor`.

### Task 8: Docs

**Files:**
- Modify: `README.md` (status, controls, layout), `AGENTS.md` (module list, documents and layers, frame data flow), `ARCHITECTURE.md` (§6.1 landed notes for layers and the path fields; §7.2 landed notes for the brush and `Shift+L`).

- [ ] **Step 1: Write. Step 2: `cargo test` count updated in README. Step 3: Commit** — `docs: the brush, layers, and how a soft stroke reaches the frame`.

## Self-review

- Spec coverage: document (T1), brush (T2), scene/gfx compositing (T3, T4), select/editor/dock (T5), panel (T6), app keys/routing/frame/ring/cursor (T7), docs (T8). Out-of-scope items untouched.
- Names used across tasks: `Tip`, `Brush`, `Frame`, `Group`, `Pass`, `passes`, `painted`, `PanelHit`, `Panel::layout/hit/prims`, `Editor::active_layer/select_layer/add_layer/remove_layer/toggle_layer/move_layer`, `Editor::press(.., &Brush)`, `Editor::stroke() -> Option<&Stroke>` — consistent with the spec.
