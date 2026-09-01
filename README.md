# Omawhite

Local-first whiteboard for Omarchy with export for the agent. Native Rust
engine (winit + wgpu). See [ARCHITECTURE.md](ARCHITECTURE.md) — a **draft**:
the real architecture emerges from development.

## Status

Scaffold (§15 items 1–2) plus the first tool (item 4, pencil only):

- `cargo build` clean, `cargo test` with 80 tests.
- Wayland window + wgpu, one instanced pipeline of SDF primitives (rounded
  boxes and round-capped segments, analytic antialiasing) for everything
  on screen.
- Dotted background fixed in world space; light theme matching the
  reference look, re-derived from `op: theme`.
- Pencil: freehand strokes become `path` elements, saved on release.
- Tool dock centered at the bottom — Select `V`, Pencil `P`; `Esc` cancels
  the stroke in progress.
- Versioned JSON document (schema 1) + XDG persistence (0700/0600, atomic
  save).
- IPC protocol §5 (closed schema) + single instance via socket.
- CLI: `--new`, `--open <id>`, `--export <dir>`, `--shutdown`,
  `--socket <path>`.

Not yet: pan/zoom input, selection, eraser, text, shapes, undo, export,
Omarchy plugin, thumbnails.

## Run

```sh
cargo run                      # most recent board (or a new one)
cargo run -- --new             # new board
cargo test                     # full suite
```

Smoke test (opens the window, renders 3 frames, exits):

```sh
XDG_DATA_HOME=/tmp/omawhite-smoke cargo run -- \
  --socket /tmp/omawhite-smoke.sock --smoke-frames 3
```

## Layout

```
src/main.rs      CLI dispatch → forward to the live instance, or become it
src/cli.rs       flags (clap), mutually exclusive actions
src/doc.rs       document §6.1 (pure data, serde): rect, path
src/store.rs     ~/.local/share/omawhite: boards/, index.json, perms §9.3
src/ipc/         §5: proto (strict parser), client (forward), server (socket 0600)
src/scene.rs     View (camera + viewport + scale) and document → SDF prims (pure, tested)
src/grid.rs      dotted background (pure, tested)
src/theme.rs     palette: light default, derived from op: theme (pure, tested)
src/editor.rs    active tool + stroke in progress → path on release (pure, tested)
src/dock.rs      bottom tool dock: layout, hit-test, icons (pure, tested)
src/gfx.rs       wgpu 30: the instanced SDF pipeline
src/app.rs       winit: window, input routing, socket → event loop bridge
```

Frame data flow: grid + document + live stroke + dock → `scene` prims →
`gfx`.

User data: `~/.local/share/omawhite/`. Socket:
`$XDG_RUNTIME_DIR/omawhite.sock`.
