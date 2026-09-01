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
- **Pure core, thin shell.** `doc` (serde data, versioned schema), `scene` (view + document → SDF prims), `grid`, `theme`, `editor` (tool + stroke in progress), `dock`, `store` (XDG persistence), `cli`, and `ipc` are pure and carry the whole test suite. `app` (winit event loop, input routing, socket → event-loop bridge) and `gfx` (one instanced SDF pipeline) are the untested shell — keep logic out of them so it stays testable.
- **Frame data flow:** grid + document + live stroke + dock → `scene` prims → `gfx` pipeline.
- **Units:** world unit = logical pixel at zoom 1; `View` folds the window scale factor in. Dock and grid are sized in logical px.

# Security Invariants

Verified by tests; don't loosen them casually:

- IPC schema is closed (§5): unknown op, field, or version is an error, never best-effort. One JSON per line, frames capped at 64 KiB, socket `0600`, same uid only.
- Disk (§9.3): `~/.local/share/omawhite` and subdirs `0700`, files `0600`, writes are atomic (tmp in same dir + rename).
- Export destinations arriving over the socket or CLI are candidates; the binary validates them against the allowlist (§8.2) before writing.

# Conventions

- Documentation, code (identifiers, comments, messages), and commits in English.
- Commits are atomic — one coherent change each — with succinct messages.
- `CLAUDE.md` is a symlink to this file (omarchy convention); edit `AGENTS.md`.
