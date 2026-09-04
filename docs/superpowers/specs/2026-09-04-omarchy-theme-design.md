# The Omarchy theme

The board is an Omarchy app and has never looked like one. It takes
three colours over `op: theme` (§5) and guesses the rest by luminance;
its corners, its border widths and its type are constants compiled in.
Omarchy changes far more than three colours when a theme is set, and
every other app on the desktop follows all of it.

This is the board following all of it: the palette by its own names,
the border the desktop draws its controls with, the corner Hyprland
rounds its windows to, and the face fontconfig resolves `monospace` to.

## 1. What a theme actually is

`omarchy-theme-set` stages a directory and swaps it in at
`~/.local/state/omarchy/current/theme/`. Four things there matter to a
window that draws its own chrome:

| file | what it carries |
|---|---|
| `colors.toml` | `mode`, `accent`, `selection`, `muted`, four backgrounds, four foregrounds, the named palette (red … magenta) |
| `shell.toml` | `[controls]` border and fill tokens per state, `[spacing] scale`, `[font] base-size`, `[bar]`, `[menu]`, `[popups]`, `[tooltip]` |
| `~/.config/omarchy/shell.toml` | the person's overrides, laid over the theme's |
| fontconfig `monospace` | the family — `omarchy-font-set` writes it, and says the shell and every Qt app resolve through it |

Two more come from the session rather than the theme, and the look is
not Omarchy's without them: Hyprland's `decoration:rounding`, which is
the corner every window on the desktop is cut to, and its
`general:border_size`.

A theme installed from a repo may ship neither `colors.toml` nor
`shell.toml`: the first is generated from `alacritty.toml` when
missing, and the second is rendered from a template at theme-set time.
Both are therefore always on disk — but a third-party theme may still
define only the legacy ANSI names, which is why the reader carries the
same alias cascade `omarchy-theme-color` documents: `bg`/`fg` for the
long names, `colorN` → semantic, shades derived by mixing, and `mode`
from `mode`, then `theme_type`, then background luminance, then dark.

## 2. The surface question

Omarchy paints **every surface in `background`** and separates it from
what is behind with a **border** — the bar, menus, popups and tooltips
all name the same `background`, and `shell.toml` carries fill *alphas*
rather than surface shades.

That does not close on its own here, because a board's canvas *is* the
ground: a dock painted in `background` over a canvas painted in
`background` is invisible but for its outline. Omarchy already names
the way out:

- the canvas is `dark_background`
- every panel is `background`

It holds in both modes — `dark_background` is a step *away* from the
panel whichever way the theme runs — and the confirmation is the
`white` theme, where it lands on canvas `#f5f5f5` under panels of
`#ffffff`. The reference look this board was drawn to is an off-white
canvas of `#f5f5f4` under a near-white dock. One unit apart.

## 3. The rest of the palette, by the role each surface plays

| the board's chrome | the section that describes it | tokens taken |
|---|---|---|
| tab strip | `[bar]` | `background`, `text` |
| dock, brush palette, properties bar, layers panel | `[menu]` | `background`, `text`, `border` |
| buttons, tabs, cards, brush cells, sliders | `[controls]` | `normal-border-*`, `selected-fill-alpha` |
| selection outline, handles | `accent` | |

Which resolves to:

```
bg          dark_background      panel       background
ink, icon   foreground           icon_active accent
muted       muted (named)        selection   accent
dot         mix(bg, ink, 0.25)   handle      panel
border      normal-border at normal-border-alpha
active_bg   foreground at selected-fill-alpha, over the panel
shadow      black, by mode
```

Three of those are worth saying out loud. `muted` stops being a mix
this build invented and becomes the token the theme names. `active_bg`
is composited on the CPU rather than left as an alpha, because a fill
alpha *over a surface* is what Omarchy means by it and an opaque result
is what the rest of the palette can be compared against. And the inks
the dock offers do not move: they are what a board is marked up in, not
what the interface is painted with, and a fixed red goes on matching
itself across themes.

`lifted` stays the fixed blue for the reason it was fixed: it says "in
flight", and it has to say that against whatever palette is up.

## 4. Borders and corners

Every border in the chrome today is a bare `inset(-s)` — one logical
pixel — under a colour this build mixed. Both become the desktop's:
the width is `[controls] normal-border-width`, the colour
`normal-border` at `normal-border-alpha`, which on the theme in front
of me is the foreground at 0.4.

A width may be written as a CSS-style list (`"Y X"`, `"T R B L"`). A
rounded rectangle here has one width, so the first number is taken and
the rest ignored; per-side chrome is not a thing this renderer draws.

Corners are not the theme's at all — no theme ships a radius. They are
Hyprland's `decoration:rounding`, which Omarchy ships as `0`. The
board's radii are a family (12 for a panel, 8 for a button, 7 for a
tab, 6 for a cell) and they scale together by `rounding / 8`: at 8 the
chrome is exactly what it is today, at 0 it is square like every window
on the desktop, at 12 it follows. With no Hyprland to ask, the factor
is 1.

**A capsule is not a corner.** A slider's track, a scrollbar's thumb,
an ink dot and a round knob are shapes whose radius is half their own
height; the factor does not reach them. Squaring those would not make
the board look like Omarchy, it would make it look broken.

The factor and the width ride on `Theme`, which is not a widening of
what a theme is — `shell.toml` carries border widths and a type scale
beside its colours — and it costs nothing: a radius and a border are
only ever read inside a `prims()`, and every `prims()` already takes a
`&Theme`.

## 5. Type

`fc-match monospace` names the file; `fontdue` parses it; the bundled
Liberation Sans stays as the fallback, for the reason it was bundled —
a board that cannot letter its own tabs because a machine is missing a
font is not local-first. A face that will not parse falls back the same
way.

The atlas has one size because the chrome draws one size, and that size
becomes `[font] base-size` rather than the 13 compiled in.

`[spacing] scale`, times `base-size / 12` when `scale-with-font` is on,
is the chrome's own scale. It is folded in exactly where the window's
scale factor is already folded in — at the five places a chrome widget
is laid out — so no signature changes and nothing else moves. The
canvas, the grid and the selection handles keep the window's factor:
panel density is not zoom.

## 6. How a change arrives

Two ways, and the first asks nothing of anybody:

- **On focus.** The theme switcher takes the keyboard and gives it
  back. `focus_gained` compares the mtime of `theme.name` against the
  one the style was read with, and re-reads when it moved. One `stat`
  per focus, no configuration, and it covers the way a person actually
  changes a theme.
- **`omawhite --theme`**, which never opens a window — it joins
  `--export` and `--shutdown` there. Over the socket it is `op: theme`
  with `colors` made optional: with colours it is the plugin sending
  its own, without them it is *read the desktop's theme again*. The
  schema stays closed; an optional field is a declared field.

A script for `hooks/theme-set.d/` ships with the repo and is installed
by hand. The board does not write into `~/.config/omarchy` on its own,
for the same reason a draft's autosave never writes into a file the
person named.

Nothing on disk changes: no board, no `index.json`, no `brushes.json`.
A theme is what the window is painted with, and the window is painted
fresh every frame.

## 7. What is deliberately left out

So that nobody derives it twice:

- **The wallpaper.** A board's canvas is a drawing surface, not a
  desktop.
- **The nine other font tokens.** The chrome has one text size; a scale
  of ten is a vocabulary for a UI that draws captions, titles and
  displays, and this one does not.
- **Hover and focus states.** `[controls]` describes five states and
  the chrome tracks two. Hover is behaviour, not colour, and adding it
  is a different change.
- **The twenty per-token spacing overrides.** `scale` is what moves
  every dimension together; the individual pins address a Qt component
  set this window does not have.
- **The marquee's alpha.** `selection-fill-alpha` is 0.35 and describes
  a text selection inside an input. The marquee is canvas overlay at
  0.1 and stays there.
- **`[controls] normal-fill-alpha`, and `colors.toml`'s `selection`.**
  Both want a surface this window does not draw: nothing in the chrome
  has an idle fill distinct from the panel it sits on, and the one
  editable field draws a caret and no selection. Consuming them would
  mean *adding* those surfaces, which is a design change and not a
  theme mapping.
- **`general:border_size`.** It is the width Hyprland draws *around the
  window*, which the compositor draws already. The chrome's own borders
  are `[controls]`'.

One line falls out of all this and is worth stating plainly, because it
decides what a machine without Omarchy sees: **the palette and the text
size come from the theme, so they need one; the face and the corner come
from the session, so they apply wherever the session answers.** It is
exactly the split `Theme::from_style` makes when it lays `wearing` over a
palette or over the board's own.

## 8. Tests

`omarchy` is pure but for three reads, a `fc-match` and a `hyprctl`, so
the parsing carries the suite: the real `colors.toml` bodies as
fixtures, each step of the alias cascade, mode from each of its four
sources, a `shell.toml` section read and the person's file laid over
the theme's, a border width given as a list, and `hyprctl`'s `int: 0`.

`theme` carries the derivation: canvas below panel in both modes, the
`white` theme reproducing the reference look, the border being the
foreground at the theme's own alpha, `muted` being the theme's own, the
inks unchanged, a rounding of 0 squaring the panels and leaving the
capsules alone.
