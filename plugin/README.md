# Omawhite bar widget

The Omarchy shell plugin for [Omawhite](https://github.com/edumoraes/omawhite),
the local-first whiteboard: a bar icon whose popout holds the two ways into
the board — `n` for a new one, `o` for the recent projects.

It is a shell, not a second engine. It reads the engine's `index.json` and
nothing else, speaks intent through the engine's own command line, and
launches the board detached, so the board owns its own window and killing it
cannot take the shell down.

## Install

The widget is a plugin for Omarchy's shell, from Omarchy Quattro on, and it
drives the Omawhite engine, a program of its own. Install the engine first —
from the AUR, on Arch and Omarchy:

```sh
yay -S omawhite-bin
```

Then add the widget to the bar:

```sh
omarchy plugin add https://github.com/edumoraes/omawhite-plugin.git --enable
```

`omarchy plugin update edu.omawhite` brings it up to date. omawhite-plugin
moves only when an engine release does, so the widget it hands out is never
ahead of the engine the AUR hands out.

The AUR packages carry the widget too, at `/usr/share/omawhite/plugin`.
Linking that one instead keeps it in step with the engine through pacman; a
change there loads on the shell's next start (`omarchy restart shell`), since
the shell does not watch through a link:

```sh
mkdir -p ~/.config/omarchy/plugins
ln -s /usr/share/omawhite/plugin ~/.config/omarchy/plugins/edu.omawhite
omarchy plugin enable edu.omawhite right
```

One or the other: both are the same plugin id, and `omarchy plugin add`
refuses an id that is already installed.

## Remove

```sh
omarchy plugin remove edu.omawhite
```

That takes the widget off the bar and out of the plugins folder — a clone is
deleted, a link is unlinked. The engine stays until its own package is
removed, and the boards stay in `~/.local/share/omawhite` after that.

## The recents

Both kinds of project stand in one list, newest first, each marked with the
glyph of the door it came through: a **draft**, which the store keeps and
which opens by id (`--open`), or a **file** the person named, which opens by
path (`--open-file`). Two flags rather than one, because the engine's schema
is closed and will not guess which a string is.

Twenty rows show at a time. Typing filters the whole index — the rows are
drawn from the matches, so the fortieth project is a word away rather than a
scroll; `Backspace` and `Ctrl+U` edit what has been typed, and `Esc` clears
it before it steps back anywhere. One pass over the remembered paths, in one
process, dims a file that is not there right now and says so: an unmounted
drive is not a deletion, and only the engine failing to open it drops the
entry — which the watcher then sees.

## Finding the engine

Looked for in this order, when the popout opens:

1. `omawhite` on the `PATH`
2. `~/.local/bin/omawhite`
3. `enginePath`, set on this widget's entry in `~/.config/omarchy/shell.json`
4. `target/release/omawhite`, then `target/debug/omawhite`, beside a checkout
   of the Omawhite repository — the plugin folder is resolved through
   `readlink -f` first, so the search climbs out of the symlink into the
   checkout rather than into `~/.config/omarchy/plugins/`

With none of them, the popout says so and the rows go quiet.

## Keys and gestures

| | |
|---|---|
| `n` | new board |
| `o` | the recent projects |
| `↑` `↓` | move the cursor |
| `→` `←` | into the list, and back out — `←` only while nothing is typed |
| any letter, on the list | filters the whole index |
| `Backspace`, `Ctrl+U` | unwrite it |
| `Enter` | open what the cursor is on |
| `Esc` | the filter, then the menu, then closed |
| right click on the icon | a new board, without the popout |

`Enter` waits for a cursor that can be seen: on the menu the arrows raise it,
and until they do the letters are the way in, since a selection nobody is
shown is one nobody meant. The list is a chooser and so arrives with its
first row picked.

`omarchy-shell edu.omawhite toggle` opens it from a keybinding — the
`ipcTarget` comes with `Ui/Panel`.

## Developing

The widget lives in the [Omawhite repository](https://github.com/edumoraes/omawhite),
in `plugin/`, beside the engine it drives; each engine release mirrors that
folder to [omawhite-plugin](https://github.com/edumoraes/omawhite-plugin).
Changes go to the first, never to the mirror.

From the root of a checkout, link the folder itself — the shell refuses a
symlink *inside* a plugin folder, but the folder may be one:

```sh
ln -s "$PWD/plugin" ~/.config/omarchy/plugins/edu.omawhite
omarchy plugin validate plugin
omarchy plugin enable edu.omawhite right
```

Through the link an edit does not reload on its own: the shell watches
`~/.config/omarchy/plugins/` without following links, and
`omarchy-shell shell rescanPlugins` only refreshes the list of plugins.
`omarchy restart shell` is what loads the edit; QML errors land in
`journalctl --user -t omarchy-shell`.
