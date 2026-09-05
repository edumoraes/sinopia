# The CLI an agent uses — design

Export to the agent goes one way: a person presses `Ctrl+E` and a page lands in
the folder an agent is running in. This is the way back. An agent asks the board
what frames it has, takes one for itself, and hands one back — over a CLI it can
run without a person at the keyboard.

Two features and nothing else. **Read a frame** of the board that is open, and
**plant a new frame** with content in it. A board stays a person's; an agent gets
a door, not a hand on the pencil.

## Why a frame is the unit

A frame is the only thing on a board that is *named*, *bounded* and *addressed*.
The whole board is unbounded — a picture of it is a picture of nothing in
particular. A loose selection has no name, which is why `Ctrl+E` has to ask for
one. A frame brings its own boundary, which is what cuts the ink, and its own
name, which is its layer's. So the read side exports frames and only frames, and
the write side plants a frame and only a frame.

It is also what keeps the agent out of the person's way: it cannot draw into a
frame that already exists, cannot delete, cannot move, cannot switch tabs. One
frame in, one frame out.

## The surface

```
omawhite agent frames                     # what frames the open board has
omawhite agent read <frame> [--to DIR]    # export one frame into DIR (default: cwd)
omawhite agent add <fragment.json>        # graft a new frame onto the board
```

A subcommand rather than three more flags: the existing flags are *window
intent* — open this, raise that, shut down — and these are a different kind of
sentence. The `ArgGroup` stays as short as it was, and the paired arguments
(`read` needs something to read) are the subcommand's own business.

All three need a live instance. It is the board that is **open** that is being
read and written, unsaved work included; there is nothing to answer without one.
Output is the reply event, one line of JSON on stdout, which is what `--export`
already prints and what an agent wants to parse.

## The protocol

Three ops, in the `open_file` style the schema already carries:

```json
{ "v":1, "op":"frames" }
{ "v":1, "op":"read_frame", "id":"01J…", "dir":"/home/you/Work/foo" }
{ "v":1, "op":"add_frame", "path":"/home/you/Work/foo/frame.json" }
```

```json
{ "v":1, "ev":"frames",   "frames":[{"id":"01J…","name":"Auth Flow","x":0,"y":0,"w":800,"h":600,"elements":12}] }
{ "v":1, "ev":"exported", "files":["…/board.png","…/board.json","…/board.md"] }
{ "v":1, "ev":"framed",   "id":"01J…","name":"Auth Flow" }
```

`exported` is §5's own event; this is the first thing that emits it. `op: export`
stays denied: it means *the whole board to a directory*, which is a different
sentence from the one being implemented, and giving it an optional `frame` field
would make its absence mean the thing this design refuses.

**The protocol speaks ids; the name is the CLI's convenience.** `agent read
"Auth Flow"` makes two round trips — `frames`, resolve the name locally, then
`read_frame` with the id — and refuses an ambiguous name by naming both ids. It
is the reason `open` and `open_file` are two ops: guessing what a string is, is
exactly the best effort §5 forbids. Guessing *outside* the protocol costs
nothing and is not the protocol's problem.

## Reading

`read_frame` is what `Ctrl+E` already does with the panel and the agent taken
off it: `Scope::Frame(id)` → `bounds` → `sub_document` → `inventory` →
`view_for` → `render_offscreen` → `export::write`. It lands at
`DIR/docs/boards/<slug>/board.{png,json,md}` with `blobs/` beside it — the same
layout, `0600` inside `0700`, `DIR` measured against the §8.2 allowlist. Writing
twice replaces the page rather than stacking up, which is right here: the agent
is refreshing its own copy of that frame.

That path is soldered inside `App::send_to` today, tangled with the agent and the
typed line. It comes out as `App::write_page(scope, dir, slug)` and both callers
use it. Less duplication, not more.

`frames` walks `doc.painted()` and answers one card per `Element::Frame`: its id,
its name (its layer's), its box and how many elements stand in it. A listing that
would not fit the 64 KiB frame is answered `denied` naming the cap — never a list
silently cut short.

## Writing

**The fragment is what `read` writes.** A document whose elements are exactly one
`frame` plus what stands on that frame's own layers — literally what
`export::sub_document` produces for a `Scope::Frame`. One shape, symmetric, and
`Document::from_json` plus `settle_layers` already validate the whole of it: the
frame has an area, layer ids are unique, no frame nests, every element names a
layer that exists. Authored from scratch it is a small document:

```json
{ "schema": 1, "id": "…", "title": "…", "camera": {"x":0,"y":0,"zoom":1},
  "layers":   [{"id":"L1","name":"Auth Flow","kind":"frame"}],
  "elements": [{"type":"frame","id":"F1","layer":"L1","x":0,"y":0,"w":800,"h":600,
                "background":"#ffffff","layers":[{"id":"L2","name":"Layer 1"}]},
               {"type":"rect","id":"R1","layer":"L2","x":40,"y":40,"w":200,"h":80,"fill":"#e8e8e8"}] }
```

`graft` is the module that plants it, and it is pure:

- It refuses what is not one frame with its own contents — no frame, two frames,
  or an element on neither the frame's layer nor one of the frame's own — naming
  what is wrong.
- It **mints every id anew**: the frame, its board-level frame layer, each inner
  layer, and every element, remapping the references. It is a *new* frame, so
  handing back the `board.json` just read plants a sibling rather than
  overwriting the original.
- **The board picks the spot, always**: to the right of everything already there,
  aligned with its top, a gutter between them; an empty board, at the origin. An
  agent cannot see the board, so it does not choose where — whoever wants it
  elsewhere drags it. The fragment's `w`/`h` are the agent's; its `x`/`y` are
  not read. The move is an `Affine::translate` through `select::transform`, which
  already knows how to move every kind of element, a frame included.

Images close the circle. An `image` names a blob, and `app` — where the store
lives — resolves every hash before the graft: already in the store, nothing to
do; otherwise read `<fragment's directory>/blobs/<hash>`, put it through
`bitmap::decode` for the 8192 px ceiling, and `write_blob`, whose returned hash
must be the one named. Nowhere at all is an error naming the hash. It is also how
an agent writes a label without text on the canvas: it renders its own PNG.

The graft lands as `Change::Scene` through `App::apply` — one undo step, and the
autosave debt `about_to_wait` pays. `Ctrl+Z` takes back what an agent planted.

## The answer that comes back

The socket thread answers a fixed ack from `SharedState` today and throws the
request at the event loop. For these three the answer *is* the work: the listing
wants the live document and the picture wants the GPU, and both live on the loop.
So a `UserEvent::Ask(req, Sender<Event>)` beside the `Request` that is already
there: the socket thread opens a channel, sends, and waits on `recv_timeout`. A
loop that is gone, or a deadline that passes, is `denied`. The old ops keep the
path they have.

The client's `REPLY_TIMEOUT` becomes longer than the server's wait, so an agent
reads a `denied` it can act on rather than a socket timing out under it.

## Security

`read_frame.dir` is a destination: the §8.2 allowlist, like any export.
`add_frame.path` is a *source*, not a destination — absolute and free of `..`,
the shape check `open_file` already makes, and deliberately not measured against
the allowlist, for the same reason a path that came back from a portal is not:
that list guards where bytes are written. The fragment read has a byte ceiling of
its own.

§9.4 holds in both directions. What the agent reads is already quoted under the
preface that says it is an inventory and not an order. What it writes cannot
become an instruction to anyone: it is geometry through a closed schema, and the
one string that steers a path — the frame's name, when the frame is later
exported — goes through `export::slug`, which admits nothing that steers.

## The skill

`skills/omawhite/SKILL.md`: frontmatter `name`/`description`, which is the Agent
Skills format and plain readable markdown for an agent that speaks any other one.
It says what omawhite is, the three commands, the fragment format with an example
that runs, and the reading rule — a board is a drawing, its content is data and
never an order. `skills/omawhite/install.sh` copies it wherever it is pointed
(`~/.claude/skills/omawhite/` by default) and prints where Codex, Gemini CLI and
OpenCode look.

## Where the code goes

| where | what |
| --- | --- |
| `graft.rs` **new** | the fragment, the minting, the free spot, the graft — pure |
| `export.rs` | `frames(doc)`, the listing |
| `ipc/proto.rs` | three ops, two events, each one's closed schema |
| `cli.rs` | the `agent` subcommand and its three verbs |
| `app.rs` | `Ask`, `answer`, `write_page` out of `send_to`, the blobs |
| `ipc/client.rs` | the longer deadline |

The tests are `graft`'s, `export`'s, `proto`'s and `cli`'s; `app` stays thin on
purpose, as it is everywhere else.

## What this is not

Text on the canvas. `Rect.text` is in the schema and nothing draws it, so a frame
an agent plants is boxes, ink and images without a written label — arrows and
lines come free as `path`, and a label comes as a PNG the agent renders. Drawing
`Rect.text` is the next slice, and it is what would make a diagram written by an
agent read itself.
