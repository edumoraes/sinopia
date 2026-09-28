# Pages under `.sinopia/`, and thumbnails on the recents

Two changes in one PR, stacked on `rename-sinopia` (#12). Test first,
every time: a test that fails, then the production code that makes it
pass. Atomic, semantic commits.

## 1. An export lands in `.sinopia/`, not `docs/boards/`

What `Ctrl+E` and `sinopia agent read` write into the agent's repository
goes under a hidden directory, `<dir>/.sinopia/<slug>/board.{png,json,md}`,
blobs beside it. `docs/` is the project's, and a board should not claim a
place in it by default.

- [x] `export::write` lands the page under `.sinopia/<slug>/` (tests first:
      the paths in `export.rs`'s own tests move, and still fail until the
      constant moves).
- [x] What the export locks down to `0700` is still its own and nothing
      of the project's: the page's folder and `.sinopia/`, never the
      directory the agent works in.
- [x] `taken_names` reads the same place (it follows the constant).
- [x] The skill, AGENTS.md, DEVELOPMENT.md and the draft say where the
      page is now; the repo ignores `.sinopia/` as it ignored
      `docs/boards/`.

## 2. The recents show a thumbnail of each project

`index.json` has carried a `thumb` field since the first store, always
`null`, and the store has made `thumbs/` since then too. Fill both in,
and let the plugin draw them.

Engine:

- [x] `export::preview`: the camera and size a thumbnail of the whole
      board is taken with — what is painted, fitted into a small box, and
      nothing for a board with nothing painted on it.
- [x] `Store::set_thumb(id, Option<&[u8]>)`: writes `thumbs/<id>.png`
      (0600, atomic, id validated), or removes it.
- [x] The index names `thumbs/<id>.png` in `thumb` whenever that file
      exists, for a draft and for a named file alike.
- [x] `app`: every save — a draft kept, a file written — takes the
      preview first and hands it to the store. A thumbnail that fails is
      logged; the save still counts.

Plugin:

- [x] Pull the index's parsing out of `BarWidget.qml` into
      `plugin/recents.js`, a pure library, under a `qmltestrunner` suite
      in `tests/plugin/` (behaviour unchanged — the suite pins it first).
- [x] `recents.js` answers a thumbnail's source only for the canonical
      `thumbs/<id>.png` of an id it would open (ARCHITECTURE §6: no path
      assembled from anything else), with the save time on the URL so a
      new picture is not the cached old one.
- [x] Each row draws its thumbnail, the name over the kind's glyph and
      the time; a board with nothing on it keeps an empty tile.
- [x] CI runs the plugin's suite.

## Wrap-up

- [x] `cargo build`, `cargo clippy --all-targets` with zero warnings;
      `cargo test`; the plugin suite.
- [x] Live check: a draft drawn on shows up in the recents with its
      picture (scratch `XDG_DATA_HOME`, never the person's data), and an
      export lands in `.sinopia/`.
- [x] PR stacked on `rename-sinopia`.
