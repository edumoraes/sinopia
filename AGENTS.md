# Omawhite

Local-first whiteboard for Omarchy with export for the agent. Native Rust engine (winit + wgpu), single binary `omawhite`, later fronted by a thin Omarchy shell plugin. User data lives in `~/.local/share/omawhite/`; the control socket is `$XDG_RUNTIME_DIR/omawhite.sock`.

# ARCHITECTURE.md Is a Draft

[ARCHITECTURE.md](ARCHITECTURE.md) is a working draft under construction, not a contract: the real architecture emerges from development. [README.md](README.md) tracks what actually exists — read it first. `§N` references in code and docs point at ARCHITECTURE.md sections. When code and draft diverge, the code is the truth; evolve the draft as decisions actually land.

# Working Style

Scaffolding means minimum: build in small slices, don't implement the whole draft ahead of need. §15 of the draft orders the implementation cuts to de-risk the stack — stay inside the slice at hand.

# Commands

```sh
cargo build
cargo test                    # full suite
cargo test store::            # one module's tests (inline #[cfg(test)] modules)
cargo run                     # most recent board, or a new one
cargo run -- --new            # new board
```

Save, open and the unsaved-work question go through `xdg-desktop-portal`
(any backend with a file chooser; `-gtk` here). The question itself has
no portal and falls back to `zenity`; without it the answer comes back
as a cancel, so nothing is lost.

Headless-ish smoke test (opens a window, renders 3 frames, exits):

```sh
XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- --socket /tmp/omawhite-smoke.sock --smoke-frames 3
```

# Architecture

Module layout is in the README. The shape that matters:

- **Single instance.** `main.rs` maps CLI flags to an IPC request; if the socket answers, the intent is forwarded and the process exits — only otherwise does it become the main instance and start the winit loop. `--export` and `--shutdown` never spawn a window.
- **Pure core, thin shell.** `doc` (serde data, versioned schema, layers and paint order), `scene` (view + document → SDF prims, frames, groups and the passes that composite them), `geom` (affine maps, oriented frames), `select` (element frames, hit-testing, handles and the transforms they drive), `bitmap` (image bytes → RGBA8, paste size), `grid`, `theme`, `brush` (settings, the tip a stroke carries, the pointer's ring), `editor` (tool, held keys, stroke and its tip, pan/zoom gesture, selection and the drag reshaping it, the active layer), `dock`, `layers` (the panel on the right and the handle that opens it), `tabs` (the strip, and what a tab gives up when it narrows), `text` (glyph atlas, measure, layout, truncation), `project` (a document's origin and dirty flag), `store` (XDG persistence, blobs, project files), `cli`, and `ipc` are pure and carry the whole test suite. `app` (winit event loop, input routing, socket → event-loop bridge), `gestures` (Wayland `zwp_pointer_gestures_v1` → event-loop bridge), `clipboard` (`wl_data_device` → event-loop bridge), `dialogs` (portal file chooser and message box → event-loop bridge) and `gfx` (the instanced SDF pipelines, image textures, the scratch texture) are the untested shell — keep logic out of them so it stays testable.
- **Frame data flow:** grid + document + live stroke + selection overlay + brush ring + dock + layers panel + tab strip → `scene`/`select`/`brush`/`layers`/`tabs` prims, gathered in a `scene::Frame`. An image is one prim too: `scene` needs the renderer's blob → texture-slot map to emit it, and `gfx` cuts the frame into runs wherever that slot changes. A glyph is that same textured box with `Prim.uv` naming its cell of the atlas; the atlas is white with alpha for coverage, so `texel * color` is the ink. It has its own slot in `gfx`, never an entry in the blob map. A soft or translucent stroke is a **group** in the frame: `scene::passes` plans, on the CPU where it is tested, a wipe and a union draw of its prims into a window-sized scratch texture (every channel a max, premultiplied — the union of their coverage) and one composite box back onto the window at the stroke's opacity, dropping groups off the viewport; `gfx` only executes the passes. The scratch has a slot of its own, like the atlas. A crisp opaque stroke — every pencil stroke — never becomes a group.
- **Documents and tabs.** One tab is a `Project` (document + `Origin` + dirty flag) and the `Editor` driving it: tool, selection and any drag belong to a document, so a tab switch hands nothing on and only re-applies the held modifiers, which are physical. `Origin` decides where `Ctrl+S` writes — `Board(id)` for anything the CLI or the plugin made (they write before the window sees it, and `index.json` is the only file the plugin reads), `File(path)` for a project the user named, `Untitled` until the first save asks. Nothing autosaves: a scene change dirties the tab, a camera change dirties nothing. `open` is never empty — the last tab closing sets `closing` instead, because every accessor assumes a tab in front.
- **Layers.** `Document.layers` is bottom to top and never empty in memory; every element names its layer by id, and `from_json` settles both — a board without layers gets `Layer 1` and its elements join it, an unknown or duplicate id is an error. Paint order is `Document::painted()`: the layers' order, then document order within a layer, hidden layers skipped — `scene` paints through it and `select` hits through it, so the renderer and the pointer never disagree on what is on top. The active layer is session state on the `Editor`, kept as an id (it survives reorders) and resolved to the top when unset or gone; new ink and pastes land on it, and picking an element makes its layer active. Layer operations live on `Document` (add above, remove but never the last, move one step, reorder to any index); the editor wraps them and keeps the selection honest (what is hidden or removed leaves it). Dragging a row is `reorder_layer` on the active layer, applied live on every move: the press selects the layer, `Panel::drop_index` names the row under the pointer — a purely positional answer, so the reorder it causes cannot make it oscillate — and `app` holds nothing but the flag saying the pointer is still down. A row paints as a card inside it (the rect is the hit target, the card less the gap between two of them is what is drawn), and `Panel::prims` takes that same flag: the carried card is thrown further off the panel, outlined in `theme.lifted` — a fixed blue, the one color not derived from the plugin's three — and painted last, over the cards it is passing. The brush's settings are the window's, not a tab's, as in Photoshop; `Shift+L`, the handle beside the panel and the panel's visibility are the window's too.
- **Input flow:** `app` maps winit and gesture events to `editor` calls in screen px plus the current `View` and the document; the editor answers with a `Change` (scene, camera or selection) that `app` stores. Pan/zoom math is `View::showing` in `scene`. A stroke takes its `Tip` at the press — the pencil's constant, or the brush handed in — and release writes it into the path, so a setting changed mid-stroke does not change the ink. Selection drags transform the document live from a snapshot taken at the press; `Esc` restores it, so `cancel`/`set_tool` need the document too. What a press does is `Editor::pointer_tool`, not `active_tool`: a resize handle takes the press back from the Ctrl-is-Zoom override, and the cursor asks the same question so it cannot promise something else. Hit order over the window is strip, layers handle, layers panel, dock, canvas.
- **Units:** world unit = logical pixel at zoom 1; `View` folds the window scale factor in. Dock, panel, grid and selection handles are sized in logical px; brush size is world units, so it scales with zoom like the ink it makes.

# Security Invariants

Verified by tests; don't loosen them casually:

- IPC schema is closed (§5): unknown op, field, or version is an error, never best-effort. One JSON per line, frames capped at 64 KiB, socket `0600`, same uid only.
- Disk (§9.3): `~/.local/share/omawhite` and subdirs `0700`, files `0600`, writes are atomic (tmp in same dir + rename).
- Export destinations arriving over the socket or CLI are candidates; the binary validates them against the allowlist (§8.2) before writing. A path that came back from a portal dialog is not one of those — it is the user's own choice, made in a system dialog — so save/open paths are not measured against that list. Loading one changes nothing about the parse: schema closed, every blob still checked.
- An `image` element's `blob` is a bare lowercase-hex sha256 — checked when the board parses and again before it becomes a path. Pasted bytes are refused past 8192 px a side, read from the header before any texel is allocated.

# Conventions

- Documentation, code (identifiers, comments, messages), and commits in English.
- Commits are atomic — one coherent change each — with succinct messages.
- `CLAUDE.md` is a symlink to this file (omarchy convention); edit `AGENTS.md`.
