# Omawhite bar widget

The Omarchy shell plugin (§10.2): a bar icon whose popout holds the two ways
into the board — `n` for a new one, `o` for the list of those already saved.

It is a shell, not a second engine. It reads `index.json` and nothing else
(§6), speaks intent through the CLI that mirrors the socket (§5), and
launches the binary detached, so the board owns its own window (§4.1) and
killing it cannot take the shell down (§11).

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
| `o` | the board list |
| `j` `k`, `↑` `↓` | move the cursor |
| `→` `←` | into the list, and back out |
| `Enter` | open what the cursor is on |
| `Esc` | back to the menu, and from there closed |
| right click on the icon | a new board, without the popout |

`omarchy-shell edu.omawhite toggle` opens it from a keybinding — the
`ipcTarget` comes with `Ui/Panel`.
