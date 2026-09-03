# Omawhite bar widget

The Omarchy shell plugin (§10.2): a bar icon whose popout holds the two ways
into the board — `n` for a new one, `o` for the recent projects.

It is a shell, not a second engine. It reads `index.json` and nothing else
(§6), speaks intent through the CLI that mirrors the socket (§5), and
launches the binary detached, so the board owns its own window (§4.1) and
killing it cannot take the shell down (§11).

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

## Install

The shell refuses a symlink *inside* a plugin folder, but the folder itself
may be one — which is what keeps the plugin versioned with the engine it
drives:

```sh
ln -s ~/Work/board/plugin ~/.config/omarchy/plugins/edu.omawhite
omarchy plugin validate ~/Work/board/plugin
omarchy plugin enable edu.omawhite right
```

Saving any file under `~/.config/omarchy/plugins/` reloads plugin code on its
own; `omarchy-shell shell rescanPlugins` forces it.

## Finding the engine

Looked for in this order, when the popout opens (§10.1):

1. `omawhite` on the `PATH`
2. `~/.local/bin/omawhite`
3. `enginePath`, set on this widget's entry in `~/.config/omarchy/shell.json`
4. `target/release/omawhite`, then `target/debug/omawhite`, beside a checkout
   of this repo — the plugin folder is resolved through `readlink -f` first,
   so the search climbs out of the symlink into the repo rather than into
   `~/.config/omarchy/plugins/`

With none of them, the popout says so and the rows go quiet. It offers no
install command, because there is no package to name yet.

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
