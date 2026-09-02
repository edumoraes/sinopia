# Projects and tabs

Save, open and keep several boards open at once, each in its own tab.

## Why this is not a small change

Today `App` holds one `Document` and writes it to the store on every
scene change. Nothing is ever unsaved, so "save", "save as" and "confirm
before closing" have nothing to act on. The feature replaces that model:
N documents, explicit saves, and a dirty flag per tab.

It also lands the first text on screen. The dock draws hand-made SDF
icons; `Rect.text` exists in the schema and nothing paints it. A tab
strip without labels is not a tab strip, so a minimal glyph atlas comes
first — sized for chrome, reused later by the text tool (§15.4).

## Decisions

- **A project is a file the user names and places.** `~/notes.omawhite`,
  holding exactly the `Document` JSON the store already writes. The XDG
  store keeps the blobs, the index and the boards reached over CLI/IPC.
- **Dialogs are the system's**, over `xdg-desktop-portal`. That buys the
  file browser, the name field and the three-button message box without
  a line of in-app text input.
- **Autosave goes away.** A dirty flag per tab drives the dot on the
  label and the confirmation on close.
- **Tab labels get a real glyph atlas**, not numbered chips.

## Three origins

A tab's origin decides where `Ctrl+S` writes. Keeping `Board` as its own
case is what preserves `--open <id>` and `op: open`, whose schema is
closed and tested (§5).

| Origin | Reached by | `Ctrl+S` writes to |
|---|---|---|
| `File(PathBuf)` | `Ctrl+O`, `Ctrl+Shift+S` | that path |
| `Board(String)` | `--open <id>`, `op: open`, `--new`, the most recent board | `boards/<id>.json` |
| `Untitled` | the `+` button | nothing — opens Save As |

`Ctrl+Shift+S` turns any origin into `File`.

`--new` and `op: new` stay on the `Board` side on purpose: both already
write the document before the window sees it, and `index.json` is the
only file the plugin reads (§5), so a board created for the gallery has
to appear in it. The `+` button answers to nobody, so it starts
`Untitled` and asks for a name on the first `Ctrl+S`, which is what the
shortcut was asked to do.

## Modules

Pure modules carry the tests; `dialogs` joins `clipboard` and `gestures`
as an untested bridge into the event loop.

### `text.rs` — new, pure

Glyph atlas and layout. `fontdue` rasterizes a fixed charset (printable
ASCII, the Latin-1 supplement so Portuguese file names keep their
accents, and `…`) into one RGBA8 bitmap: white, alpha = coverage. Any
character outside the set renders as `·`.

```rust
pub struct Font(fontdue::Font);           // bundled bytes
pub struct Atlas { pub bitmap: Bitmap, cells: HashMap<char, Cell>, px: u32 }
pub struct Glyph { pub rect: ScreenRect, pub uv: [f32; 4] }

impl Atlas {
    pub fn build(font: &Font, px: u32) -> Atlas;
    pub fn measure(&self, s: &str) -> f32;
    pub fn layout(&self, s: &str, x: f32, baseline: f32) -> Vec<Glyph>;
    pub fn truncate(&self, s: &str, max_w: f32) -> String;   // appends `…`
}
```

The atlas is keyed by integer px size and rebuilt when the window's
scale factor changes that integer — one upload, not one per frame. It
gets its own slot in `gfx`, not an entry in the blob map:

```rust
pub fn upload_atlas(&mut self, bmp: &Bitmap) -> anyhow::Result<u32>;
```

`slots` stays what §9.3 says it is — sha256 to texture — so a key that
is not a bare hash never enters it, and a rebuild replaces the atlas
texture in place instead of leaking one per scale change.

Bundled font: `assets/fonts/LiberationSans-Regular.ttf` (SIL OFL 1.1,
402 KB), with its license beside it. `include_bytes!`, so the binary
never depends on what the machine has installed.

Tests: measure grows with the string and is zero for `""`; layout
advances match the sum of advances; truncation fits inside the budget
and never returns more than the input plus the ellipsis; an unknown
character falls back rather than panicking.

### `scene.rs` — `Prim` gains a UV sub-rect

```rust
pub struct Prim {
    // ...
    pub uv: [f32; 4],   // u0, v0, u1, v1 — KIND_IMAGE only
    pub slot: u32,      // stays on the CPU
}

impl Prim {
    pub fn glyph(r: ScreenRect, uv: [f32; 4], slot: u32, color: Rgba) -> Prim;
}
```

Images set `[0.0, 0.0, 1.0, 1.0]`, so their pixels are unchanged. The
shader's one new line maps the box's local coordinate into the sub-rect:

```wgsl
let t = (local + half) / max(in.geom.zw, vec2<f32>(1e-6));
let uv = mix(in.uv.xy, in.uv.zw, t);
```

The atlas is white, so the existing `textureSampleLevel(...) * in.color`
already tints a glyph. The vertex layout gains `6 => Float32x4`, ahead
of `slot`.

Tests: an image prim's UV is the full rect; a glyph prim samples its
cell; existing `image_runs` tests still cut runs on the slot alone.

### `project.rs` — new, pure

```rust
pub enum Origin { File(PathBuf), Board(String), Untitled }

pub struct Project {
    pub doc: Document,
    pub origin: Origin,
    pub dirty: bool,
}

impl Project {
    pub fn label(&self) -> String;      // file stem, else doc title, else "Untitled"
    pub fn touch(&mut self);            // dirty = true
    pub fn saved(&mut self, origin: Origin);
    pub fn key(&self) -> &str;          // doc.id — stable across reorders
}
```

`key()` matters because the confirmation dialog answers asynchronously:
by the time it returns, the tab's index may have moved.

Tests: label for each origin and for a path with no stem; dirty
transitions; `saved` re-homes an `Untitled` to a `File`.

### `store.rs` — arbitrary paths

```rust
pub fn save_document_to(path: &Path, doc: &Document) -> anyhow::Result<()>;
pub fn load_document_from(path: &Path) -> anyhow::Result<Document>;
```

Free functions, not `Store` methods: these paths live outside the root.
Atomic as everywhere else — tmp in the same directory, then rename — but
without forcing `0600`, which belongs to the store (§9.3) and would
surprise anyone saving into a shared directory.

**Security.** §8.2's allowlist covers export destinations arriving over
the socket or the CLI. A path chosen in a portal dialog is neither: it
is direct user intent through a trusted system dialog, so it is not
validated against the allowlist, and this is the one place that
distinction is load-bearing. What still holds on the way in: the parse
stays closed (serde, `SCHEMA_VERSION`), and every `blob` in a loaded
document is re-checked by `is_blob_hash` before it becomes a path. A
`.omawhite` from elsewhere cannot name a file outside `blobs/`.

Tests: round-trip through a `tempfile` path; a malformed file is an
error, not a panic; a document naming `../../etc/passwd` as a blob is
refused on load; the tmp file is gone after a successful save.

### `tabs.rs` — new, pure

The dock's shape, at the top of the window and spanning its width. Sized
in logical px, positioned in physical px, floating over the canvas and
swallowing what it catches.

```rust
pub enum TabHit { Select(usize), Close(usize), New, Strip }

pub struct Tabs { pub strip: ScreenRect, pub tabs: Vec<TabBox>, pub new: ScreenRect }

impl Tabs {
    pub fn layout(viewport: Viewport, scale: f64, labels: &[(String, bool)]) -> Tabs;
    pub fn hit(&self, x: f64, y: f64) -> Option<TabHit>;
    pub fn prims(&self, active: usize, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim>;
}
```

Each tab: a dot when dirty, the label truncated to the width left, and a
close cross. Tabs share the strip evenly, capped at 200 logical px and
floored at 72 — below that the label is `…` and only the dot and the
cross survive. Strip height 34, label 13 px.

Tests: widths sum to the strip minus the `+` button; hit-test returns
`Close` inside the cross and `Select` beside it; `Strip` for the gaps;
labels truncate when the tabs are narrow; one tab and twenty tabs both
lay out.

### `dialogs.rs` — new, untested shell

`rfd` on a worker thread, answering through `EventLoopProxy` — the same
shape as `clipboard.rs`.

```rust
pub fn open(proxy: EventLoopProxy<UserEvent>, parent: &Window);
pub fn save_as(proxy: EventLoopProxy<UserEvent>, parent: &Window, suggested: &str, key: String);
pub fn confirm_close(proxy: EventLoopProxy<UserEvent>, parent: &Window, label: &str, key: String);
```

```toml
rfd = { version = "0.17", default-features = false, features = ["xdg-portal", "wayland"] }
```

`xdg-portal` avoids GTK and pulls in `pollster`, already a dependency;
`wayland` lets the dialog be parented to the window so it is modal to
Omawhite instead of floating loose.

The `key` threaded through each call is the project's `doc.id`: the
answer is matched back by identity, never by index.

### `app.rs` — several projects

```rust
struct Open { project: Project, editor: Editor }

struct App {
    projects: Vec<Open>,
    active: usize,
    // ...
}
```

One `Editor` per tab: tool, selection and any drag belong to a document,
and the editor is pure, so a tab is a document plus its editor. Held
modifiers are physical and global — re-applied to the incoming editor on
every switch, so `Ctrl` held across a tab change does not go stale.

`switch_to` becomes tab activation: escape the outgoing drag, upload the
incoming document's textures, retitle the window. Blob slots in `gfx`
are keyed by hash and shared across tabs, so a screenshot pasted in two
projects uploads once.

The frame gains the tab strip after the dock. Hit-testing order is
tabs, dock, then canvas.

Window title: `Omawhite — <label>`, with a leading `•` while dirty.

## Keys

| Key | Effect |
|---|---|
| `Ctrl` + `S` | Save the active tab; `Untitled` opens Save As |
| `Ctrl` + `Shift` + `S` | Save As, always |
| `Ctrl` + `O` | Open — multi-select opens one tab each |
| `Ctrl` + `W` | Close the active tab; confirms when dirty |

winit reports `Key::Character("S")` when Shift is down, so the match
reads the modifier state and compares case-insensitively, as `Ctrl+V`
already does.

## Closing, and the dialog that answers later

`Ctrl+W` on a clean tab closes it. On a dirty one it spawns the
confirmation and returns to the loop; the answer arrives as a
`UserEvent`:

- **Save** — write, then close. An `Untitled` chains into Save As first,
  and a cancelled Save As cancels the close.
- **Discard** — close.
- **Cancel** — nothing.

Closing the last tab exits, as a tabbed editor does. `+` opens a fresh
untitled board.

`WindowEvent::CloseRequested` with any dirty tab runs the same
confirmation on the first dirty tab. Cancel aborts the quit; Save or
Discard closes that tab and re-issues the request, which walks the rest
one dialog at a time.

## Slices

Each is one commit, and each leaves the tree building and green.

1. `text` — font, atlas, layout, truncation. `Prim.uv`, the shader's
   sub-rect, `Prim::glyph`.
2. `project` + `store` — origins, the dirty flag, save/load at a path.
3. `tabs` — layout, hit-test, prims.
4. `app` — several projects, one editor each, the strip in the frame,
   clicks routed to it.
5. `dialogs` — the portal bridge.
6. Keys and the close flow.
7. Docs — README controls and layout, AGENTS.md architecture.

## Out of scope

- **Images in a project file that travels.** Blobs stay in the XDG
  store, so a `.omawhite` copied to another machine shows placeholders.
  Embedding or a sidecar directory is its own decision.
- Undo (§15.4), which explicit saving makes more tempting, not less.
- Dragging tabs to reorder, and scrolling the strip when it overflows —
  labels shrink instead.
- The text tool. The atlas exists for chrome; the tool needs editing,
  selection and a document element, and is §15.4's business.
- Thumbnails.
