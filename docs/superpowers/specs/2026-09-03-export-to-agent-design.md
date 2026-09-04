# Export to the agent — design

A sketch leaves the board, lands in the folder an agent is running in, and the
agent is told it is there — on one keystroke, with nobody hunting for a folder.
It is what §2 says the product is for and what §15 item 5 ordered, though it is
not the export §8 drew; the section below says where the two part.

The board becomes what it was drawn for — a notebook of sketches and directions
for the agents on this desktop — rather than a place drawings go to sit.

## One action, and what it depends on

The question that opened this was whether to write files into the agent's folder
*or* to push a message into its running session. It is not a choice: every send
channel available carries **text**, and a diagram is an image. A message can only
say "look at `docs/boards/auth-flow/board.png`", which means the file has to
exist first. The file is the payload and the message is the doorbell, and one
keystroke is both.

This is **not** the generic export of §8, and the difference decides the design.
`--export <dir>` and `op: export` take a directory somebody named. *Export to the
agent* names nothing: the whole point is that the person never goes looking for
a folder — a running agent already knows where it is, and that is the only reason
the shortcut is worth having. So the feature **depends on a live agent**, and
with none running it is off: the window says so and nothing is written. A folder
picker as a fallback would be exactly the friction the feature exists to remove,
which is why there is none.

The two exports share the machinery that writes the three files and nothing else.
§8's keeps its own destination story for the day the plugin asks for it.

## What was verified

Three ways to find a running agent, measured on this machine, where three
`claude` processes were live in three different projects:

| backend | tells us where | can send | how |
| --- | --- | --- | --- |
| `herdr` | `cwd`, `focused`, `agent_status` | yes | `herdr agent prompt <pane> <text>` |
| `tmux` | `pane_current_path`, `pane_active` | yes | `load-buffer` → `paste-buffer -p` → `send-keys Enter` |
| `/proc` scan | `cwd` | **no** | — |

`herdr agent list` answers JSON per agent: `{agent, agent_status, cwd, focused,
pane_id, workspace_id, terminal_title}`. `tmux list-panes -a -F …` answers
`pane_current_command`, `pane_current_path`, `pane_id`, `pane_active`,
`window_active`. The `/proc` walk reads `comm` and `cwd` and finds every agent
either of the others knows about, plus any running under neither.

Two findings that shaped the design:

- **The §8.1 cwd heuristic cannot work here.** It says to take the focused
  window's pid and read `/proc/<pid>/cwd`. On this machine one Hyprland window
  (`foot`, titled "omarchy: board") holds a multiplexer with twelve shells and
  two agents in *different* projects; the window's pid resolves to the
  multiplexer, whose cwd is `$HOME`. A window cannot answer which agent is meant.
  The list can, so the user picks from it: honest, and the same list the send
  needs anyway.
- **A prompt must go in as a bracketed paste.** `tmux paste-buffer -p` wraps the
  text in `ESC [200~ … ESC [201~` when the application has asked for bracketed
  paste, which every agent TUI does. Verified against a probe that enabled
  DECSET 2004: the whole multi-line block arrives as one paste, and the `Enter`
  sent afterwards is what submits it. Plain `send-keys` would submit at the
  first newline and turn one instruction into several turns.

## The shape

Five pieces, each testable on its own, and the boundary between pure and shell
is the one the rest of the codebase already draws.

### 1. Scope

What leaves the board: `Scope::Frame(id)` or `Scope::Selection`, both in this
cut, because a sketch worth sending is often three strokes and a circle and
making the person draw a frame around it first is the friction this feature
exists to remove. `Scope::Board` is the same machine again and follows.

A scope answers three things — the world box it covers, the elements and layers
inside it, and **whether it has a name** — and all three are pure functions over
the document, so they carry tests.

A frame has a name: it is an area with a box and a stack of its own, and §4's
invariant (a layer is the object it holds) means the frame's layer *is* the
frame's identity, which is where the name lives once there is a way to give it
one (§5 below). It owns its folder by that name, so sending the same frame again
updates the same page rather than littering.

A loose selection has no name, and the panel asks for one rather than inventing
it: a second field, shown only in that case, prefilled with the board's own name
and a counter past whatever `docs/boards/` already holds. Picking the free name
is a pure function over the existing names; only the listing is shell.

### 2. The three artifacts

Written to `<agent cwd>/docs/boards/<slug>/`, fixed file names, atomically
(tmp in the same dir, then rename), `0600` on the files and `0700` on the
directories, exactly as the store writes (§9.3).

- **`board.png`** — the scope's box plus a margin of `EXPORT_MARGIN` world units,
  rendered offscreen at `EXPORT_SCALE` px per world unit (2, so a board drawn at
  zoom 1 exports at twice the pixels it was drawn with) and clamped to the
  device's maximum texture dimension. `scene` already builds a `Frame` from a
  `View`, so the export computes the `View` that fits the box at that scale and
  hands `gfx` a frame and a size; `gfx` renders to a texture, copies to a buffer
  and the bytes come back as RGBA8, which `image` encodes. The plan is pure and
  tested; only the readback is shell. The scratch and sheet textures are
  window-sized today and must be sized to the export instead — that is the one
  real piece of engineering here.
- **`board.json`** — the scoped elements and layers in the document's own schema,
  without the camera. Any blob an image element names is copied to
  `docs/boards/<slug>/blobs/<sha256>`, so the JSON is self-contained rather than
  pointing into a store the agent cannot see. A blob's name is its content hash,
  never a name the document chose, so §9.3's rule holds unchanged.
- **`board.md`** — the generated inventory, with §9.4's fixed preface. It is
  deliberately thin for now — dimensions, layers, counts, images — because
  without a text tool or shapes there is nothing else that can be inventoried
  truthfully. It grows when they land.

### 3. Agent discovery

A new `agents` module: an `Agent { kind, cwd, label, send }` where `send` is
`Herdr(pane)`, `Tmux(pane)` or `None`. `None` is an agent the `/proc` scan found
that neither multiplexer claims: the files still land in its folder, which is the
half nobody can do by hand, and the panel says the message could not go rather
than pretending it did.

The shell runs the three commands; **parsing their output is pure**, over
captured fixtures, the way `tablet` keeps its frame accumulation pure while the
protocol stays in the shell. Entries are deduplicated by `cwd` against the ones
that already arrived with a pane, so an agent inside herdr is not also listed by
the scan.

The list is offered and nothing is guessed — but it is not a form to fill in
either. The panel opens with a target already chosen (the last one used on this
board, else the focused one, else the only one there is) and the cursor already
in the instruction — in the instruction even when the folder field is showing,
since its prefill is usually right and the line never is. The fast path is
`Ctrl+E`, type the line, `Enter`. The list is there to change the target, not to
make the person pick one every time.

An empty list is the feature's off switch. Discovery runs when the panel is asked
for and again whenever the window takes focus — which is exactly when the person
has come back from the terminal they started an agent in — so what the window
shows is never more than one alt-tab stale.

### 4. The send

A short fixed template around the user's own line:

```
<the line the user typed>

Diagram exported from the board:
  docs/boards/auth-flow/board.png
  docs/boards/auth-flow/board.json
```

Paths relative to the agent's cwd, which is where it is running and where the
files were just written. The text reaches the process as `argv` (herdr) or
through a buffer on stdin (tmux) — never as a shell string, so a line holding
`$(…)` or `;` has nowhere to execute.

herdr reports `agent_status`, and it refuses a submission to a blocked agent; the
panel shows the status and reports the refusal. tmux has no status to report and
the text lands in the agent's input box either way.

### 5. The field, and the rename it also serves

A one-line text field is new to this codebase: `text` measures and draws already,
but nothing has ever taken keyboard input into a value. Built once, it has two
users, and the second is what makes the first worth having.

Nothing on a board carries a name the user gave it. `Document.title` is born
`untitled`, and `next_layer_name` names every new layer `Layer N` regardless of
kind (`src/doc.rs:1148`) — so even a frame is born "Layer 3". Without a rename,
every export lands in `docs/boards/layer-3/`, which says nothing and collides
across boards.

So this cut also gives a layer its name: double-clicking a card in the panel
opens the field over it, `Enter` commits, `Esc` cancels. And `next_layer_name`
learns the kind, so a frame is born `Frame N` and the rename starts from
something sensible.

The slug — the frame's name, or the one typed for a loose selection — is
lowercased, spaces to hyphens and restricted to `[a-z0-9-]`, and a name that
reduces to nothing falls back to the scope's id. That rule is what keeps the
typed field harmless: `../../etc` holds no character the slug admits, so it can
never be more than one directory under `docs/boards/`. The person chooses the
name; the shape of the path is not theirs to choose.

## Where it hangs

`Ctrl+E` in the `Key::Character` arm at `src/app.rs:1459`, beside `Ctrl+S`. It
opens the panel on whatever is selected — a frame, or a loose selection. `Tab`
walks the fields, `Enter` sends from either, `Esc` cancels. With nothing
selected, or with no agent found, the panel does not open and the window says
which of the two it was — a shortcut that opens onto a dead end teaches nothing.

The IPC protocol does not change. `op: export` already exists for the plugin and
means the other export; this one begins and ends inside the app's own UI, so it
needs no op of its own.

## Security invariants

The existing ones hold unchanged: the destination goes through the §8.2 allowlist
after `realpath` (an agent's cwd is discovered, not typed, but it is still a
candidate and still measured), file names are fixed, writes are atomic and
`0600`, and `board.md` stays a generated, escaped inventory under a fixed preface
— the board's own text never becomes an instruction (§9.4).

One invariant is new, because one power is new: **the send writes bytes into a
live terminal.** So the user's line is printable characters only — no C0, no
ESC, no newline — and is capped in length. Anything else is refused, and refused
visibly rather than stripped in silence: an instruction the user cannot see being
altered is worse than one that does not go.

The separation §9.4 draws survives intact and gets sharper. The document is
inventory; the instruction is a deliberate act by the person at the keyboard,
typed at send time. The board never speaks to the agent in its own voice.

## Testing

Pure, and therefore tested: the scope's box, its sub-document and whether it has
a name; the slug rule, including that a typed name cannot reach past
`docs/boards/`; picking the free name over a listing; `board.md`'s generation and
escaping; the prompt template and the sanitizer (rejecting ESC and C0, capping
length); parsing herdr's JSON and tmux's format output into `Agent`s over
captured fixtures; the dedupe by cwd; the allowlist check on the destination;
`next_layer_name` by kind; the rename's effect on a slug.

Shell, and therefore not: spawning herdr and tmux, the wgpu readback, the field's
keyboard plumbing.

## Out of this cut

`Scope::Board` — the machine is built for all three and the frame and the
selection are wired. §8's own export: `--export <dir>` and `op: export` stay
unimplemented and keep their own destination story. A new IPC op for the send, or
any configurable send command — herdr and tmux are detected, not configured, so
no new config file. Persisting the remembered target across restarts.

Renaming the *board*, which is the field's third user and the one thing that
would make a loose selection's prefill worth reading: `Document.title` is born
`untitled` and stays there, so until a tab can be renamed the prefill is
`untitled`, `untitled-2`, `untitled-3`. It is a placeholder over a field the
person was going to fill anyway, so it costs a name and not a step.

And the text tool that would one day let the direction be written on the board
itself instead of in the field.

## Risks

The offscreen render is the piece that can surprise: scratch and sheet are sized
to the window today, and an export at high DPI needs them sized to the export.
A frame far larger than the window, or a high DPI factor, can ask for a texture
past what the device allows — the export must clamp to the device's maximum
dimension and say what it did rather than fail at the driver.
