# Styling part of a text

A text has one style through the whole of it (#15). This gives a stretch
of it a style of its own — a bold word, a red name, a bigger title on
the first line — as Figma, Affinity and every rich-text box do. Stacked
on `text-tool` (#15). Test first, every time. Atomic, semantic commits.

## The model

The text's own `TextStyle` stays the **base**. On top of it, `runs`: a
list of stretches, each `start..end` in characters, carrying only what
it sets differently — family, size, bold, italic, underline, strike,
colour, tracking. What a paragraph is — alignment, a frame's vertical
alignment, leading — stays the text's alone. On disk a run is
`{ "start": 0, "end": 5, "bold": true }`; `runs` is absent when there is
none, so every board written before opens meaning what it meant. Runs
are checked on the way in (inside the text, in order, not overlapping,
not empty, their values what a text may be) and kept tidy: adjacent
runs saying the same are one, and a run saying nothing is dropped.

## Tasks

- [x] `spans` (pure): runs — resolving the style at a place, applying a
      change to a range, following an edit (a stretch replaced by new
      characters that take the style of the one before), tidying.
- [x] `doc`: `Text.runs`, checked; `TextStyle` split into what a run
      may set and what only the text sets.
- [x] `typeset`: every character in its own face and size — advances,
      kerning only within one face and size, a line as tall as its
      tallest letters, baselines aligned, the caret as tall as the
      letter before it.
- [x] `scene`: each glyph in its own face, size and ink; underline and
      strike in stretches.
- [x] `select`: scaling artistic text scales every run's size too.
- [x] `editor::typing`: runs follow every edit, the typing's undo keeps
      them, the bar styles the selection while typing (or the next
      letters when nothing is selected), the whole text otherwise; what
      the bar shows is the style where the caret is.
- [x] `ipc` / `cli`: `set_text` takes a `range`; `sinopia text set
      --range A:B`.
- [x] Skill, AGENTS.md, DEVELOPMENT.md.
- [x] Zero warnings, `cargo test`, seen working in a window.
- [ ] PR stacked on #15; CI green.
