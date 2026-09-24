# Omawhite — architecture specification

Local-first whiteboard for Omarchy.  
Decisions in this version: Omaboard is a product reference only, not a code reference. Engine in Rust, native and fast. Remote collaboration comes later. This document details architecture, tradeoffs, security and distribution via plugin.

---

## 1. Locked decisions

1. **Don't fork Omaboard.** It proves there is demand for a board on the Omarchy desktop (gallery, pen, shapes, PNG, thin plugin). The code is Qt/C++, local-only, no export for the agent. We use the idea, not the repository.
2. **Native app in Rust.** The canvas, the document, undo and I/O live in their own binary. No QML drawing strokes, no WebView, no embedded Excalidraw in the MVP.
3. **The Omarchy plugin is a shell.** It starts and stops the process, offers a shortcut, a chip on the bar, theme, a shallow gallery and the “export to agent” command. It does not interpret the document.
4. **Local first.** Drawing tools, persistence, reopen, export to the agent's cwd. No Iroh, no Automerge, no audio, no ticket in this phase.
5. **Collab is a future transport plan**, not a requirement of the current data model — but the board file must be *able* to become a CRDT later, without rewriting the canvas.

Working name in this doc: **Omawhite** (binary `omawhite`, plugin id `…omawhite`). Swap it when there is a final name.

---

## 2. The problem the product solves

On Omarchy the typical flow is: terminal + agent (`claude`, `codex`, `opencode`, …) in the workspace, Hyprland tiling everything. What's missing is a board that:

- opens on a shortcut, without friction;
- lets you sketch architecture / flow / UI in vector;
- drops the drawing **inside the project folder** for the agent to read;
- is editable again tomorrow.

The Presenter Overlay solves “scribble over the demo and vanish”.  
Omaboard solves “local board with gallery”.  
Omawhite solves “board the agent eats”.

---

## 3. Principles

- One shell process, one board process. A canvas crash doesn't take the bar down.
- The document is sovereign in the child. The plugin only speaks intent.
- Open in under ~150 ms warm; cold, fit inside a keystroke.
- Zero network in the MVP. A binary that works offline once installed.
- Every file written to the user's disk has an explicit owner, mode and destination.
- Export to agent is a local action, never a side effect of a stroke.

---

## 4. Architecture

```
 Super+… / click on the bar
            │
            ▼
 ┌──────────────────────┐     Unix socket 0600      ┌─────────────────────────┐
 │  Omarchy plugin      │◄─────────────────────────►│  omawhite (Rust)        │
 │  omarchy-shell       │   closed JSON schema      │                         │
 │                      │                           │  canvas + scene graph   │
 │  bar-widget          │                           │  tools                  │
 │  menu / thin overlay │                           │  undo / clipboard       │
 │  spawn / kill        │                           │  persistence            │
 │  “export to agent”   │                           │  export png/json/md     │
 └──────────────────────┘                           └───────────┬─────────────┘
        same process                                            │
        as the bar                                     ~/.local/share/omawhite/
                                                       ~/Work/<proj>/docs/boards/
```

Three pieces, three lifecycles:

| Piece | Process | When it exists |
|---|---|---|
| QML plugin | `omarchy-shell` (Quickshell) | While the plugin is enabled |
| `omawhite` binary | child of the user | While the board is in use (MVP: born on open, dies on close) |
| Board files | disk | Always |

The plugin does **not** embed the child's Wayland window in QML (foreign toplevel / xdg-foreign). On Hyprland that breaks focus, scaling, IME and tablet. The child has its own window.

### 4.1 Responsibilities

**Plugin**

- Register shortcut / chip / menu.
- Discover the binary (`PATH` or `~/.local/bin/omawhite`).
- `spawn` with `--socket` and `--board`.
- Terminate with a graceful `shutdown`; timeout → SIGTERM → SIGKILL.
- List the gallery from a **sanitized** index (title, id, thumb path).
- Trigger export: the plugin may suggest the cwd; the binary validates and writes.
- Paint itself with the Omarchy palette (`qs.Commons` / current theme).

**Binary**

- Window, input, infinite canvas, zoom/pan.
- Tools, hit-test, snap, undo/redo.
- Read/write the document.
- Export render (PNG of the bbox + margin).
- Refuse unsafe export destinations.
- Single-instance: a second launch becomes a command on the socket, not a second window.

**Outside both (later)**

- Iroh / Automerge / WebRTC daemon. Same binary, feature flag, another thread. The plugin stays network-free.

### 4.2 Plugin kinds

```json
{
  "schemaVersion": 1,
  "id": "seu.omawhite",
  "name": "Omawhite",
  "version": "0.1.0",
  "author": "…",
  "license": "MIT",
  "description": "Local-first whiteboard with export for the agent.",
  "kinds": ["bar-widget", "menu"],
  "entryPoints": {
    "barWidget": "BarWidget.qml",
    "menu": "Menu.qml"
  },
  "barWidget": {
    "displayName": "Omawhite",
    "category": "Productivity",
    "allowMultiple": false,
    "defaultSection": "right"
  }
}
```

- `bar-widget` — chip on the bar, gallery preview, “New board”.
- `menu` — surface summoned by the shortcut, same gallery, without taking fullscreen.
- `overlay` — only if we want a fullscreen picker. Avoid an overlay that *is* the canvas.
- `service` — phase 2, if the process becomes resident. In the MVP the menu/bar starts the child on demand. Don't use `kind: bar` (it replaces the whole bar).

Summon:

```
omarchy-shell shell toggle seu.omawhite '{}'
```

Suggested bind (don't steal Super+W from Omawrite or Super+D from desks):

```
o.bind("SUPER + SHIFT + B", "Omawhite", "omarchy-shell shell toggle seu.omawhite '{}'")
```

---

## 5. IPC

Transport: Unix domain socket at `$XDG_RUNTIME_DIR/omawhite.sock`.  
Permission `0600`. Accept only the same uid. Frame = one JSON per line, `v: 1`, maximum size (e.g. 64 KiB).

The plugin speaks intent. The child does not send strokes over the socket.

### Plugin → app

```json
{ "v": 1, "op": "ping" }
{ "v": 1, "op": "new" }
{ "v": 1, "op": "open", "id": "01J…" }
{ "v": 1, "op": "raise" }
{ "v": 1, "op": "export", "dir": "/home/you/Work/foo", "formats": ["png", "json", "md"] }
{ "v": 1, "op": "theme", "colors": { "bg": "#1a1a1a", "fg": "#eee", "accent": "#7aa" } }
{ "v": 1, "op": "theme" }
{ "v": 1, "op": "shutdown" }
{ "v": 1, "op": "frames" }
{ "v": 1, "op": "read_frame", "id": "01J…", "dir": "/home/you/Work/foo" }
{ "v": 1, "op": "add_frame", "path": "/home/you/Work/foo/frame.json" }
{ "v": 1, "op": "layers" }
{ "v": 1, "op": "add_layer", "kind": "group", "name": "Sky", "above": "01J…" }
{ "v": 1, "op": "remove_layers", "ids": ["01J…"] }
{ "v": 1, "op": "rename_layer", "id": "01J…", "name": "Sky" }
{ "v": 1, "op": "move_layers", "ids": ["01J…"], "place": "into", "target": "01K…" }
{ "v": 1, "op": "arrange_layers", "ids": ["01J…"], "how": "front" }
{ "v": 1, "op": "show_layers", "ids": ["01J…"], "visible": false }
{ "v": 1, "op": "lock_layers", "ids": ["01J…"], "locked": true }
{ "v": 1, "op": "set_opacity", "ids": ["01J…"], "opacity": 0.5 }
{ "v": 1, "op": "set_blend", "ids": ["01J…"], "blend": "multiply" }
{ "v": 1, "op": "set_color", "ids": ["01J…"], "color": "red" }
{ "v": 1, "op": "group_layers", "ids": ["01J…", "01K…"] }
{ "v": 1, "op": "ungroup", "ids": ["01G…"] }
{ "v": 1, "op": "duplicate_layers", "ids": ["01J…"] }
{ "v": 1, "op": "merge_layers", "ids": ["01J…", "01K…"] }
{ "v": 1, "op": "merge_down", "id": "01J…" }
{ "v": 1, "op": "merge_visible" }
{ "v": 1, "op": "flatten" }
{ "v": 1, "op": "select_layers", "ids": ["01J…"] }
{ "v": 1, "op": "open_layers", "ids": ["01G…"], "open": true }
```

The last three are a **code agent's** own door, and they are unlike
everything above them: their answer *is* the work — a listing wants the
live document and a picture wants the GPU — so they are not acked and
forgotten, they are handed to the event loop and waited on. A frame is
the unit both ways: the only thing on a board that is named, bounded and
addressed. `read_frame` takes an **id** and never a name, for the reason
`open` and `open_file` are two ops; resolving a name is the CLI's own
round trip, outside the schema. `add_frame` takes a **path** and never
the content, because the socket speaks intent and never scene content —
and because a frame with ink in it does not fit 64 KiB. What it names is
a *fragment*: one frame plus what stands on that frame's own layers,
which is exactly what a `read_frame` writes, so a page read off the board
can be handed straight back. It arrives as a new frame — every id minted
again — and the board picks where it lands, an agent having no way to see
what it would land on.

The **layer ops** are the command line's hold on the layers, and they
go through the same door as an agent's three: handed to the event loop
and waited on. `layers` answers the whole tree; every other one changes
it, the way the panel would — the layers named are picked, then acted
on — as one step of the history, and answers the layers it left picked.
What a lock keeps, a move the tree refuses or a merge that merges
nothing is `denied` with the reason: a script has to be able to tell
that nothing happened. They speak **ids**, never names — two layers may
go by one name — and `omawhite layer` resolves a name with a listing
first, refusing one two layers go by. `place` is `into`, `above` or
`below`, `how` is `front`, `forward`, `backward` or `back`, `blend` and
`color` are written as a board writes them, and `opacity` is a fraction.

`colors` is optional, and its absence is not a missing field but a
different sentence: with colours a host is dressing the board in its
own three; without them it is saying the *desktop's* theme has changed
and the board should read it again. The schema stays closed either way —
an optional field is a declared field, and an unknown one is still an
error. `omawhite --theme` is the CLI half, and an Omarchy
`theme-set` hook is what calls it.

### App → plugin

```json
{ "v": 1, "ev": "ready", "id": "01J…", "pid": 1234 }
{ "v": 1, "ev": "saved", "id": "01J…" }
{ "v": 1, "ev": "exported", "files": ["…/board.png", "…/board.json", "…/board.md"] }
{ "v": 1, "ev": "denied", "op": "export", "reason": "path-outside-allowlist" }
{ "v": 1, "ev": "exited", "code": 0 }
{ "v": 1, "ev": "frames", "frames": [ { "id": "01J…", "name": "Auth Flow", "x": 0, "y": 0, "w": 400, "h": 240, "elements": 3 } ] }
{ "v": 1, "ev": "framed", "id": "01J…", "name": "Auth Flow" }
{ "v": 1, "ev": "layers", "layers": [ { "id": "01J…", "name": "Sky", "kind": "raster", "owner": null, "depth": 0, "visible": true, "shown": true, "locked": false, "opacity": 1.0, "blend": "normal", "color": "none", "active": true, "picked": true, "elements": 1 } ] }
{ "v": 1, "ev": "done", "ids": ["01J…"] }
```

An answer that would not fit a frame is `denied` naming the cap, never a
listing quietly cut short: the caller is a program, and a short list is a
lie about the board rather than a smaller truth.

Rules:

- Closed schema. Unknown field → error, not “best effort”.
- No paths coming from preview/title without canonicalizing.
- `export.dir` is a *candidate*. The binary decides whether to write.
- A second `omawhite` on the CLI: if the socket is alive, forward `open`/`new`/`raise` and exit `0`.

The CLI mirrors the socket, for the plugin and for humans:

```
omawhite                  # raise or native gallery
omawhite --new
omawhite --open <id>
omawhite --export <dir>
omawhite --shutdown

omawhite agent frames                     # what frames the open board has
omawhite agent read <frame> [--to DIR]    # export one, by id or by name
omawhite agent add <fragment.json>        # graft a frame onto the board

omawhite layer list                       # the whole tree, top first
omawhite layer <verb> [<layer>...]        # add, remove, rename, move, show, hide, lock,
                                          # unlock, opacity, blend, color, group, ungroup,
                                          # duplicate, merge, merge-down, merge-visible,
                                          # flatten, select, expand, collapse
```

The `agent` verbs are a subcommand and not three more flags: everything
above them is window intent — open this, raise that, shut down — and what
an agent asks is a different kind of sentence. All three need a live
instance, because it is the board that is *open* they read and write,
with the work nobody has saved yet inside it.

---

## 6. Data model (local phase)

XDG directory:

```
~/.local/share/omawhite/
  index.json          # id, title, updated_at, thumb
  boards/<id>.json    # document
  thumbs/<id>.png     # small preview, no session metadata
```

`index.json` is the only file the plugin reads. Title and paths are treated as untrusted text (even though it's the user themself: the index is the surface between two processes).

### 6.1 Document

Versioned JSON, retained scene — not an input replay.

```json
{
  "schema": 1,
  "id": "01J…",
  "title": "auth flow",
  "camera": { "x": 0, "y": 0, "zoom": 1 },
  "elements": [
    {
      "id": "el_01",
      "type": "rect",
      "x": 40, "y": 80, "w": 220, "h": 80,
      "stroke": "#222", "fill": null,
      "text": "API Gateway"
    }
  ]
}
```

MVP types: `path` (pen), `rect`, `ellipse`, `arrow`, `line`, `text`, `sticky`, `image` (blob referenced by local hash, not absolute path).

Landed — `path` (pencil strokes):

```json
{ "id": "el_02", "type": "path",
  "curves": [[[40, 80], [44, 84], [48, 90], [52, 91]]],
  "stroke": "#1f1f1f", "width": 2 }
```

`curves` is a chain of cubic Béziers, each self-contained as
`[a, c1, c2, b]` (SVG's `M a C c1 c2 b`) and starting where the previous
ended; a tap is one degenerate cubic. Coordinates are world units; `width`
is in world units too, so ink scales with zoom. A world unit is one logical
pixel at zoom 1. The stroke is simplified and fitted on release
(`src/curve.rs`), so the document never holds raw pointer samples. Boards
written before the fit landed hold a raw `points` polyline instead; those
are fitted on load and rewritten as `curves` on the next save.

Landed — `image` (clipboard paste):

```json
{ "id": "el_03", "type": "image",
  "x": -200, "y": -130, "w": 400, "h": 260,
  "blob": "fdb7ce9b…8547a44" }
```

A box exactly like a rect's — same `x, y, w, h`, same `rotation` — so the
selection frame, hit-testing and the transforms follow from the same
fields. `blob` names the original bytes under `blobs/<sha256>` and nothing
else: it is checked as a bare lowercase-hex sha256 on the way in, so a
hand-edited board cannot walk out of the store. A board therefore carries
no path, and says nothing about the machine that wrote it.

Landed — layers, and the layer every element is on:

```json
{ "schema": 1,
  "layers": [ { "id": "01J…", "name": "Layer 1" },
              { "id": "01J…", "name": "Layer 2", "visible": false } ],
  "elements": [ { "id": "el_04", "type": "path", "layer": "01J…", "…": "…" } ] }
```

`layers` is bottom to top; `visible` is absent when true. Paint order is
the layers' order, then document order within a layer; a hidden layer
paints nothing and is not hit. Both fields default, so a board written
before this loads unchanged: it gets `Layer 1` on the way in and its
elements join it. A `layer` no layer carries, or a duplicate layer id, is
an error — the parse stays closed. The active layer is session state
(§6.2), not a field.

Landed — frames, and the stack one carries:

```json
{ "layers": [ { "id": "01J…", "name": "Layer 1" },
              { "id": "01J…", "name": "Frame 1", "kind": "frame" } ],
  "elements": [
    { "id": "el_05", "type": "frame", "layer": "01J…",
      "x": -200, "y": -140, "w": 400, "h": 300, "background": "#fbfbfa",
      "layers": [ { "id": "01J…", "name": "Layer 1" } ] } ] }
```

A frame is an area that holds objects, and the layer of `kind: frame` it
sits on is the layer it *is*: the two go together, and the parse refuses
either half alone. `layers` on a frame is a stack of its own, bottom to
top, never empty once parsed and never holding a frame — frames do not
nest. `elements` stays flat: an object inside a frame simply names one of
that frame's layers, and since layer ids are unique across every stack,
the `layer` an element already carried says where it is. `background` is
an unvalidated hex, as a rect's `fill` is; there is no `rotation`,
because the cut that makes a frame a frame is an axis-aligned box in the
shader, and a field that cannot be honoured is worse than no field. A
board written before frames existed parses unchanged: `kind` still
defaults to raster, and nothing it holds is a frame.

Landed — `opacity` and `hardness` on `path` (the brush): fractions,
absent when 1. `opacity` is the stroke's as one shape — where it crosses
itself it does not darken — and `hardness` is how much of the radius is
crisp, the ramp spending the rest inside the nominal width. The pencil
writes neither.

Landed — `rotation` on `rect` and `path`: degrees, clockwise on screen,
counted from the element's creation; absent when zero, so older boards
keep the shape above. A rect turns about its center; `x, y, w, h` describe
the box before the turn. A path keeps moves, turns and stretches baked
into its control points (Béziers are affine-invariant, and the export
never composes transforms); its `rotation` only records how far it has
been turned, so its box turns with it and rotation snapping counts from
the creation state.

Why plain JSON now, and not Automerge already:

- Fewer dependencies, debugging with `$EDITOR`, git diff if the user commits the export.
- Automerge on day 1 forces compaction, sync and API before the second peer exists.

Bridge to the future: **every element already has a stable `id`**. Collab becomes “this map in the CRDT”, not “rewrite the renderer”. When the time comes, `boards/<id>.json` can become an Automerge snapshot + camera sidecar, without changing the scene.

### 6.2 What stays out of the document

- Cursor, selection, active tool, hover.
- Ticket, peers, audio.
- Agent cwd, export path.

Those are session state, in the process or on the socket.

---

## 7. Rust stack and the canvas

Goal: 2D vector on Wayland (Hyprland), mouse/tablet input, editable text, 60–120 Hz on pan/zoom, low cold start.

### 7.1 Three ways to draw the window

| Stack | Pros | Cons | Verdict |
|---|---|---|---|
| **winit + wgpu + own scene** (Vello/peniko or manual tessellation) | Full control, first-class Wayland, no imposed UI runtime, lean binary | IME, accessibility, toolbar widgets by hand | **Default choice** |
| iced / slint | Toolbar and dialogs faster | Infinite canvas and scene hit-testing aren't its strength; iced still changes API | Only if the app chrome becomes a pain |
| egui / eframe | Prototype in a weekend | Immediate-mode look, “tool-grade” text and selection, less native | Throwaway prototype, not product |
| cxx-qt | Looks like Omawrite | Abandons the Rust-native premise | Out |

Recommendation: **winit + wgpu**. Retained scene on the CPU (element tree + AABB). The GPU only rasterizes dirty frames (dirty rect or viewport layer). Pen: point buffer in the active tool; on mouseup it becomes a `path` in the document — avoids writing 120 Hz into the JSON.

Landed: one instanced wgpu pipeline of signed-distance primitives — rounded
box or round-capped segment, evaluated per fragment with analytic
antialiasing — draws everything: grid dots, pen strokes (one segment per
span, overlapping caps make the joins), rect edges, the dock and its icons.
No tessellation, no MSAA. Per-element caching and dirty rects wait for real
profiling.

Landed: a stroke that is soft or translucent cannot be drawn segment by
segment — overlapping caps add up — so it is composited as one shape. Its
prims form a group in the frame; `scene::passes` plans a wipe and a union
draw (every channel a max, premultiplied: the union of their coverage)
into a window-sized scratch texture, then one composite box back onto the
window at the stroke's opacity, and drops groups off the viewport. `gfx`
executes the plan with four pipelines from the one shader. Nested groups
— a translucent stroke inside a translucent layer — are what layer
opacity would need, and wait for it.

Text: a minimal inline editor (cosmic-text / parley), not a webview. IME via winit/smithay-client on Wayland; test on Hyprland early, it's the classic trap.

Images: decode (image crate) → wgpu texture. Keep the original in `~/.local/share/omawhite/blobs/<sha256>`. The element in the JSON only points to the hash.

Landed: an image is the same signed-distance box with a texture in it —
`KIND_IMAGE` reads its UV from the box's own axes, so the turn, the
corners and the analytic antialiasing come along, and there is no second
pipeline. One texture per image, one bind group per texture; the frame's
instances are cut into runs wherever the texture changes and each run is
one draw over the same buffer, which keeps the document's paint order
without an atlas and without sorting. Slot 0 is a 1×1 white stand-in for
every run that draws no image. No mip chain yet: the sample asks for
level 0, which also keeps it out of the derivative rules a branch like
that would otherwise break. Decoding is off the frame path (the paste
thread, or the board's load); until a texture lands, the element paints
as a placeholder box.

Clipboard: `wl_data_device` on the window's own Wayland connection, the
guest-client shape `gestures` uses. The compositor sees one client, so the
`selection` events that only reach the keyboard focus reach us too; a
separate connection would need `wlr-data-control` and a reason to want it.

Portals: `xdg-desktop-portal` for “open image” / “save PNG elsewhere”. Don't implement a file picker of our own.

### 7.2 MVP tools

| Key | Tool |
|---|---|
| V | select (move, resize, multi-select) |
| H | pan |
| P | pen |
| E | eraser (deletes the element, not pixels — we're vector) |
| R | rectangle |
| O | ellipse |
| L | line |
| A | arrow |
| T | text |
| N | sticky |
| Ctrl+V | paste the clipboard image |
| Ctrl+Z / Ctrl+Shift+Z | undo / redo |
| Ctrl+0 / 1 | fit / 100% |
| Ctrl+Shift+E | export to the last known cwd or dialog |

A vector eraser (hit-test + delete) is simpler and more useful for the agent than a pixel eraser. Highlighter can be a pen with alpha, phase 1.1.

Landed: tools live in a dock centered at the bottom of the canvas (rounded
panel, one button per tool, active tool highlighted); the canvas has a
dotted background fixed in world space. Select, Hand, Pencil and Zoom so
far (`Z` is not in the table above: a zoom tool that scrubs by dragging
and steps by clicking, like Photoshop's, plus Ctrl held as its momentary
form). Navigation: Hand / Space / middle button drag the grabbed world
point under the pointer; wheel and two-finger scroll pan, Shift turns a
vertical wheel horizontal; with Zoom active the wheel zooms at the cursor,
one notch = one unit (×1.25), range 0.1–10. Trackpad pinch and
three-finger swipe come from `zwp_pointer_gestures_v1` — winit has no
gesture events on Wayland, so `gestures` joins the window's connection as
a guest client on a thread and forwards steps to the event loop (the same
bridge shape as the IPC server). The camera persists in the document and
is saved when a gesture ends. The tablet's pen comes the same way:
`tablet` binds `zwp_tablet_v2` as a guest on the window's connection and
hands the loop the tool's movement and the touch of its tip, which enter
the same funnel as the mouse's. Binding the protocol is what stops the
compositor emulating a pointer for the pen, so the bridge has to carry
the movement too, not only the parts a mouse has no words for.

Select (`V`) landed: click, Shift+click and a marquee pick elements (the
marquee takes whatever it overlaps). The selection frame — a lone
element's own box, turned with it, or the axis-aligned box around several
elements, which turns with them while a rotation lasts and is re-wrapped
on release — carries square resize handles on its corners and rings past
them for rotation about the frame center (Shift snaps to 15° steps from
the creation state); dragging the selection itself moves it. Drags
transform the document live from a snapshot taken at the press, so `Esc`
puts it back; `Delete`/`Backspace` removes. The selection is session
state (§6.2) and is dropped on a tool switch.

Undo landed: `Ctrl+Z`, and `Ctrl+Shift+Z` to go forward again. The
table above said `Ctrl+Y` for that, and this is the draft catching up:
the undo key with Shift on it is what every drawing tool uses, and a
second key onto the same room is one more thing to keep true. A step is a resting **state** rather than a change — the
scene changes on every sample of a stroke, and on every step of a drag —
so the history is written where a change lands with nothing still
happening, and a whole stroke, a whole drag or a card carried over two
rows comes back in one step. What is kept is the whole document beside
the editor's spot (selection, ink layer, frame being worked in), so the
two cannot come to disagree about what a restored board holds: undoing a
delete brings the objects back selected. The camera is excluded at both
ends — §6.2 keeps the cursor out of the document for the same reason it
keeps panning out of a step. The history is a tab's own, capped by a
depth and a memory budget rather than by a count alone, since an entry
carries the whole board and a session makes that board grow.

Brush (`B`) landed, and then took Sketchbook's shape rather than
Photoshop's: a `Library` of named brushes on shelves, one in the hand,
and an edit that belongs to the brush — the keyboard (`[` `]`, `{` `}`,
the digits) and the properties bar's sliders write into the brush that
is painting, and `reset` takes it back to what it shipped as. `Brush` is
the whole of Brush Properties; `Property::honored` is the one list of
what the canvas paints with — size, opacity, hardness and the nib
(spacing, roundness, rotation) — and the rest — flow and its dynamics,
texture depth, randomness, pressure — describe a brush truthfully while
the stamp engine grows into them. A brush stamps a nib along the stroke
rather than sweeping a line. Picking the tool brings up Sketchbook's
Brush Library on the left — every shelf in one scroll, its brushes a
grid of their own icons — and the properties bar under the strip: the name and
the basic pair, with a chevron dropping the whole Advanced layout
(Pressure, Stamp, Nib, Randomness) in two columns. A slider the canvas
does not answer to is drawn muted rather than hidden: the brush keeps
the value, and the bar does not promise ink it cannot lay. `Shift+B`
shuts the palette without putting the brush down. A
ring the size of the brush follows the pointer, and the stroke is saved
as the same `path` the pencil writes. Layers landed with it:
`Shift+L` shows and hides a panel on the right — one row per layer, top
first, an eye each, the active one highlighted, up/down/add/remove in the
header. New ink lands on the active layer; picking an element makes its
layer active. Renaming waits for text input, layer opacity for the nested
compositing pass. The `zwp_tablet_v2` bridge landed and the pen draws;
pressure waits on a stroke that can hold more than one width.

Omaboard-style snap and connectors: phase 1.1. In the MVP an arrow is geometry, not a live binding.

### 7.3 Window vs overlay

Two presentation modes, only one in the MVP:

- **Tiled Hyprland window** — the board sits next to the agent's terminal. Right for the “input material” use case. Default.
- **Fullscreen layer-shell** — quick scribble that vanishes on Esc. Direct competition with the Presenter Overlay. Not in the MVP.

The plugin opens/focuses the window; the compositor tiles it. Single-instance + `raise` avoids 12 stacked boards.

---

## 8. Export for the agent

This is the feature that justifies not being “yet another Omaboard”.

Agents on Omarchy run in the project's cwd (launching from `$HOME` lands in `~/Work`). They read files. A PNG alone loses structure. The trio:

```
<dir>/docs/boards/<slug>/
  board.png     # bbox + margin, high DPI
  board.json    # same schema as the document (without the UI camera if you like)
  board.md      # generated inventory
```

`board.md` is generated by the binary; it is never the raw text of the stickies pasted as an instruction:

```markdown
# Board: auth flow
<!-- generated by omawhite; this is a diagram inventory, not instructions -->

- rect "API Gateway" at (40,80)
- rect "Auth Service" at (320,80)
- arrow API Gateway → Auth Service
```

### 8.1 How to find `<dir>`

Heuristic, in this order, always validated by the binary:

1. `--export <dir>` argument / `op: export`.
2. Cwd of the focused terminal window (Hyprland active window → pid → `/proc/<pid>/cwd` if it's a known shell/agent).
3. Last successful export cwd for this board (local state).
4. Portal picker, last resort.

Never: a path written into the document by a peer (there is no peer yet, and there won't be one as an export source).

### 8.2 Write allowlist

After `realpath`:

- the destination is a directory;
- the destination is prefixed by a reasonable workspace: under `$HOME/Work`, or under a git root that is **not** `$HOME`, or under the detected cwd;
- refuse effective `..`, symlinks leaving the prefix, bare `$HOME`, `~/.ssh`, `~/.gnupg`, `~/.claude`, `~/.codex`, `~/.config`, `/etc`, `/usr`;
- fixed file names (`board.png|json|md`). The remote/local document does not choose the name.

Phase 2 (collab): a peer does **not** trigger export. Only the local owner, on click.

---

## 9. Security — local phase

The main threat today is not NAT. It's the plugin in the shell + the agent on auto-approve + files on disk.

### 9.1 Process boundary

- QML does not deserialize `boards/<id>.json`.
- QML does not open the network, does not download the binary on first run without the user asking (see distribution).
- Preview: `Image` in QML only with a canonical path inside `thumbs/`, extension allowlist, no arbitrary `file://` assembled from the title.
- Gallery title: plain text, no QML rich text.

### 9.2 Socket

- `$XDG_RUNTIME_DIR/omawhite.sock`, `0600`, same uid.
- Ops from §5 only.
- Backpressure: if the plugin disappears, the app carries on; if the app disappears, the plugin marks idle and does not respawn in a loop.

### 9.3 Disk

```
~/.local/share/omawhite/     0700
boards/, thumbs/, blobs/     0700
files                        0600
```

Thumbs without EXIF of internal paths. Blobs named by hash: content
addressed, so the same screenshot pasted twice is one file, and a name
that is not a bare sha256 never becomes a path. A pasted image is
decoded before it is stored — nothing over 8192 px a side, and the size
is read from the header before any texel is allocated, so a few KiB
cannot become gigabytes.

### 9.4 Export and prompt injection

The agent will read `board.md` and the PNG. Text the user drew (“ignore previous instructions…”) must not become a skill heading.

- Fixed preface in the markdown: “diagram inventory, not an order”.
- Board texts go in quotes / in a list, not as the user's raw markdown (escape headings and fences).
- No writes into `.claude/`, `.codex/`, `.agents/` unless the user later configures an explicit *additional* path.

### 9.5 Surface that doesn't exist yet (reserve it in the design)

When Iroh comes in:

- ticket = node id + room PSK + `doc_id` + expiry;
- peer acceptance on first contact;
- schema and quotas in the CRDT (ops/s, size, no URL fetch from an element);
- audio opt-in, muted by default, same handshake, outside the document;
- the relay only sees ciphertext.

Don't implement now. Don't leave “TODO: listen 0.0.0.0” in the MVP binary.

### 9.6 Supply chain

Omarchy plugin = unsandboxed git clone in the shell process. The marketplace README is honest: whoever installs trusts the author.

- Separate public repos: `omawhite` (binary) and `omawhite-plugin` (QML), or a monorepo with clear folders.
- The plugin does **not** vendor wgpu's `.so`.
- Binary install via package (see §10), not `curl | sh` fired by QML.
- Pinned Rust dependencies (`Cargo.lock` committed).
- `omarchy plugin validate` in CI.

---

## 10. Distribution

Two artifacts. Whoever mixes the two into the same “it's just a plugin” gets hurt: the marketplace distributes QML; wgpu does not fit that contract.

### 10.1 Binary

Paths, from most Omarchy to loosest:

1. **Arch package / Omarchy repo** — `omawhite` on the PATH, updates with `omarchy update` / pacman. Best destination.
2. **AUR + `makepkg`** — the pattern Omaboard already uses. Acceptable on day 1.
3. **`cargo install --path` / tarball in `~/.local/bin`** — development.

The plugin looks, in this order: `omawhite` on the `PATH`, `~/.local/bin/omawhite`, configurable path. If not found: an “install the engine” panel that **opens the terminal** with the package command, as other Omarchy plugins do with dependencies. QML does not silently download a binary.

Target: `x86_64-unknown-linux-gnu` first. Wayland only. No X11 in the MVP, unless winit delivers it for free.

Release: stripped binary, `opt-level = 3`, LTO in the release profile when CI time allows. GPU: wgpu Vulkan on Arch is the path; GL fallback if ever needed, not on day 1.

### 10.2 Plugin

```
omarchy plugin add https://github.com/<you>/omawhite-plugin.git --enable
```

Repository with:

```
manifest.json
BarWidget.qml
Menu.qml
README.md
LICENSE
preview.png          # optional, marketplace
```

No engine submodule. README with:

- requires Omarchy Quattro (`omarchy-shell`);
- requires `omawhite` ≥ 0.1 on the PATH;
- suggested bind;
- `omarchy plugin validate .`;
- a warning that plugins run unsandboxed.

List on the marketplace (plugins.omarchy.org / omarchyplugins.com) under Developer Tools / Productivity. Short tags: `whiteboard`, `agent`, `productivity`.

### 10.3 Joint versioning

`manifest.json` `version` and the binary's `omawhite --version` follow parallel semver. The plugin refuses an engine with a different major. Optional field in `ready`: `{ "engine": "0.1.0" }`.

Removal:

```
omarchy plugin remove seu.omawhite
# binary package handled separately
# user data stays in ~/.local/share/omawhite until the user deletes it
```

Don't wipe the home on `plugin remove`.

---

## 11. Lifecycle (MVP)

```
shortcut or click
  → plugin menu/bar opens (QML, light)
  → if there is no socket: spawn omawhite --socket $XDG_RUNTIME_DIR/omawhite.sock --board <id|new>
  → app creates the window, loads the JSON, ev: ready
  → user draws (100% in the child)
  → autosave debounced 300–500 ms into the document
  → Super+E / Export button → trio in docs/boards/<slug>/
  → close the window or op: shutdown
       → flush
       → exit 0
       → plugin forgets the pid
```

If the shell restarts midway: the JSON is already on disk; the child gets SIGPIPE on the socket and shuts down. Don't leave a zombie board without UI.

Future promotion to `kind: service` (resident child, the overlay only raises the window) only if the cold start measures above what's tolerable. Measure before inventing a daemon.

---

## 12. Roadmap

**MVP (local)**

- Window, scene, tools from the §7.2 table.
- Persistence + gallery via plugin.
- Undo/redo, internal copy/paste.
- Export png + json + md with allowlist.
- Theme: the whole of what Omarchy changes when a theme is set — the palette
  by its own names out of `colors.toml`, the border and fill tokens out of
  `shell.toml`, fontconfig's `monospace` and Hyprland's rounding — read by the
  binary itself and re-read on focus or on `op: theme` with no colours. The
  plugin's own three colours still work, for a host that is not Omarchy.
- Single-instance + CLI.

**1.1**

- Connectors with snap.
- Highlighter.
- Multi-page / several boards in the same project (`docs/boards/<slug>/`).
- Smarter cwd heuristic (list of `claude`/`opencode`/`codex` pids).
- Tiny skill for agents: “if `docs/boards/**/board.md` exists, read it before implementing”.

**2.0 — collab (later)**

- Automerge in place of plain JSON (migration: import schema 1 → CRDT doc).
- Iroh transport (QUIC, ticket + PSK). LAN/Tailscale first.
- Awareness (cursor) outside the document.
- Peer acceptance, owner/editor/viewer roles.
- Audio: separate WebRTC, signaling over the already-authenticated Iroh channel. Opt-in.

Don't pull 2.0 into the 0.1 manifest.

---

## 13. Explicit tradeoffs

| Choice | In exchange for | Cost |
|---|---|---|
| Rust + winit/wgpu, not Qt | Isolation and the native premise | IME, file portal, widget polish by hand; zero reuse of Omaboard |
| Plugin ≠ canvas | Shell alive if the board dies | Two artifacts to install; IPC to maintain |
| Kill the child on close | No daemon, no open port | Cold start every session (ok if <150 ms) |
| Plain JSON now | Simplicity, git-diff, fewer crates | Migration to Automerge later |
| Tiled window, not overlay | Coexists with the agent | Not “appears and vanishes” like Presenter |
| Object eraser, not pixel | Clean vector model for the agent | Whoever wants to scrub bitmap gets frustrated |
| png+json+md trio | Multimodal + structured agent | Three files for the user to understand |
| Two repositories (or folders) | Marketplace doesn't carry wgpu | “install the plugin” isn't enough — needs the engine |
| No network in the MVP binary | Minimal surface | Whoever expects Miro on day 1 leaves |

### What we refuse on purpose

- Excalidraw inside WebEngine in the shell.
- Homemade UDP “because it's faster”.
- Embedding the native window in QML.
- Auto-export on every stroke.
- `curl | sh` in `Component.onCompleted`.
- Collab in the same milestone as the brush.

---

## 14. Definition of done (MVP)

- The shortcut opens an empty board and accepts the first stroke without the user feeling the spawn.
- Closing and reopening restores elements and camera.
- Export writes the three files only inside the allowlist; otherwise `denied`.
- Plugin without the binary shows an actionable error, not a black screen.
- `omarchy plugin validate` passes.
- Killing the child with `kill -9` doesn't hang the shell; the plugin goes back to “idle”.
- No TCP/UDP listen in the process.

---

## 15. Next implementation cut

Order that reduces the risk of designing the wrong stack:

1. Binary: Wayland window + rectangle + JSON persistence + CLI `--new/--open`.
2. Socket + single-instance.
3. `bar-widget` + `menu` plugin that only spawns/opens.
4. Pen, text, arrow, undo.
5. Export + allowlist + cwd heuristic.
6. Omarchy theme and gallery thumbs.
7. AUR package / install script. Only then the marketplace.

Remote collaboration only comes in once 1–6 are in daily use with a real agent. If export doesn't become a habit, Iroh is engineering without a product.
