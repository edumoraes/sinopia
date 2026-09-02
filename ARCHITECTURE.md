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
{ "v": 1, "op": "shutdown" }
```

### App → plugin

```json
{ "v": 1, "ev": "ready", "id": "01J…", "pid": 1234 }
{ "v": 1, "ev": "saved", "id": "01J…" }
{ "v": 1, "ev": "exported", "files": ["…/board.png", "…/board.json", "…/board.md"] }
{ "v": 1, "ev": "denied", "op": "export", "reason": "path-outside-allowlist" }
{ "v": 1, "ev": "exited", "code": 0 }
```

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
```

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

Text: a minimal inline editor (cosmic-text / parley), not a webview. IME via winit/smithay-client on Wayland; test on Hyprland early, it's the classic trap.

Images: decode (image crate) → wgpu texture. Keep the original in `~/.local/share/omawhite/blobs/<sha256>`. The element in the JSON only points to the hash.

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
| Ctrl+Z / Ctrl+Y | undo / redo |
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
is saved when a gesture ends.

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

Thumbs without EXIF of internal paths. Blobs named by hash.

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
- Theme: Omarchy colors via `op: theme` (the plugin reads the palette and sends it).
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
