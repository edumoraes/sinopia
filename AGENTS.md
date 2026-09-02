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

Headless-ish smoke test (opens a window, renders 3 frames, exits):

```sh
XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- --socket /tmp/omawhite-smoke.sock --smoke-frames 3
```

# Architecture

Module layout is in the README. The shape that matters:

- **Single instance.** `main.rs` maps CLI flags to an IPC request; if the socket answers, the intent is forwarded and the process exits — only otherwise does it become the main instance and start the winit loop. `--export` and `--shutdown` never spawn a window.
- **Pure core, thin shell.** `doc` (serde data, versioned schema), `scene` (view + document → SDF prims), `geom` (affine maps, oriented frames), `select` (element frames, hit-testing, handles and the transforms they drive), `bitmap` (image bytes → RGBA8, paste size), `grid`, `theme`, `editor` (tool, held keys, stroke, pan/zoom gesture, selection and the drag reshaping it), `dock`, `store` (XDG persistence, blobs), `cli`, and `ipc` are pure and carry the whole test suite. `app` (winit event loop, input routing, socket → event-loop bridge), `gestures` (Wayland `zwp_pointer_gestures_v1` → event-loop bridge), `clipboard` (`wl_data_device` → event-loop bridge) and `gfx` (one instanced SDF pipeline, image textures) are the untested shell — keep logic out of them so it stays testable.
- **Frame data flow:** grid + document + live stroke + selection overlay + dock → `scene`/`select` prims → `gfx` pipeline. An image is one prim too: `scene` needs the renderer's blob → texture-slot map to emit it, and `gfx` cuts the frame into runs wherever that slot changes.
- **Input flow:** `app` maps winit and gesture events to `editor` calls in screen px plus the current `View` and the document; the editor answers with a `Change` (scene, camera or selection) that `app` stores. Pan/zoom math is `View::showing` in `scene`. Selection drags transform the document live from a snapshot taken at the press; `Esc` restores it, so `cancel`/`set_tool` need the document too. What a press does is `Editor::pointer_tool`, not `active_tool`: a resize handle takes the press back from the Ctrl-is-Zoom override, and the cursor asks the same question so it cannot promise something else.
- **Units:** world unit = logical pixel at zoom 1; `View` folds the window scale factor in. Dock, grid and selection handles are sized in logical px.

# Security Invariants

Verified by tests; don't loosen them casually:

- IPC schema is closed (§5): unknown op, field, or version is an error, never best-effort. One JSON per line, frames capped at 64 KiB, socket `0600`, same uid only.
- Disk (§9.3): `~/.local/share/omawhite` and subdirs `0700`, files `0600`, writes are atomic (tmp in same dir + rename).
- Export destinations arriving over the socket or CLI are candidates; the binary validates them against the allowlist (§8.2) before writing.
- An `image` element's `blob` is a bare lowercase-hex sha256 — checked when the board parses and again before it becomes a path. Pasted bytes are refused past 8192 px a side, read from the header before any texel is allocated.

# Conventions

- Documentation, code (identifiers, comments, messages), and commits in English.
- Commits are atomic — one coherent change each — with succinct messages.
- `CLAUDE.md` is a symlink to this file (omarchy convention); edit `AGENTS.md`.
