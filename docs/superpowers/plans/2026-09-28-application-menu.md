# The application menu

A desktop application's menu bar — File, Edit, View, Layer — over the
operations the board already has. Nothing new is done by it: every line
is a door onto something a key, a button or a row's menu already does,
and it is written with the keys that do the same. Stacked on
`sinopia-dir-thumbs` (#13). Test first, every time: a test that fails,
then the production code that makes it pass. Atomic, semantic commits.

## Where it stands

The titles stand at the left end of the tab strip, and the tabs start
after them: the strip is mostly empty, and a row of its own would take
height from the canvas for four words. A title opens its menu under it
through `menu` — the one menu the layers panel already draws and hits
through — and while one is open, passing over another title opens that
one instead, as a menu bar does. `Esc` or a press anywhere else puts it
away.

## Tasks

Pure half (`menubar`, tested):

- [ ] `Action`: what a line does — New, Open, Save, Save As, Export,
      Close, Quit; Undo, Redo, Cut, Copy, Paste, Delete; the layers
      panel and the brush library; New Layer, New Group, Rename, and
      every layer `Command`.
- [ ] `menubar::shortcut`: the `Ctrl` keys, one mapping for the window
      and the menu — `layer_key` moves in, so a hint cannot disagree
      with the key it names.
- [ ] `menubar::items(title, &State)`: each menu's lines, enabled only
      where the thing would happen (undo with a past, redo with a
      future, paste with something to paste, a layer command where
      `Editor::can` says so), the checks on what is shown, and every
      hint answering to `shortcut`.
- [ ] `History::can_undo` / `can_redo` public: a line greyed out is the
      question the comment said nobody asked yet.
- [ ] `Tabs::layout` takes where the row starts, so the titles have room.
- [ ] `Bar::layout` / `hit` / `prims`: the titles measured with the
      atlas, the open one lit.

Shell (`app`):

- [ ] The key handler asks `menubar::shortcut` and runs the action.
- [ ] `Ctrl+N` opens a new board and `Ctrl+Q` quits — the two keys every
      desktop app has, for things the board already does.
- [ ] The bar is drawn in the strip; a press on a title opens its menu
      (and closes it when it is the one open); passing to another title
      switches; a line taken runs its action.
- [ ] Hit order: menu bar before the tab strip.

Wrap-up:

- [ ] AGENTS.md (module list, hit order), DEVELOPMENT.md, README keys.
- [ ] `cargo build`, `cargo clippy --all-targets` with zero warnings;
      `cargo test`.
- [ ] Seen working in a window: every menu opens, a line runs.
- [ ] PR stacked on #13; CI green.
