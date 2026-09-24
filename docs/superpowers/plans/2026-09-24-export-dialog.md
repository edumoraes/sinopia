# Export dialog, second cut — task list

The `Ctrl+E` panel grows into a dialog: wider, a real text box, the
agents shown the way a person recognises them, the selected harness's
skills one keystroke away, and a picture of what is about to leave.

Branch `export-dialog`. Every slice is test first: a failing test, the
least code that passes it, repeat. Commits are atomic and semantic
(`type(scope): subject`). Done means CI green with every item below
implemented.

## Ground rules

- Pure core, thin shell: `send`, `field`, `text`, `agents`, `skills`,
  `export` carry the tests; `app`, `gfx`, `clipboard` stay thin.
- The send still writes bytes into a live terminal: whatever the box
  now admits, ESC and every C0 but the newline stay refused (§9.4).
- `cargo build`, `cargo clippy --all-targets` with zero warnings.

## Tasks

- [x] 0. CI: a workflow that builds, lints (`-D warnings`) and tests
- [x] 1. Wider dialog: 640 logical px, never past the window's margin
- [x] 2. Agent rows: official logo + agent name + the cwd's last directory
  - [x] 2a. `Agent::name`, `Agent::folder`
  - [x] 2b. rows draw name and folder (status kept at the far end)
  - [x] 2c. logos sourced from the vendors, sheet built by a tool, embedded
  - [x] 2d. a row draws its logo from the sheet
- [ ] 3. Copy and paste in the input
  - [x] 3a. `Field` selection: anchor, extend, select all, replace
  - [ ] 3b. pasted text cleaned for the field it lands in
  - [ ] 3c. clipboard reads text (`Ctrl+V`), never types a `v`
  - [ ] 3d. clipboard writes text (`Ctrl+C`, `Ctrl+X`)
- [ ] 4. Multi-line input that grows, wraps at a maximum width, and
      scrolls past 20 lines
  - [ ] 4a. word wrap in `text`
  - [ ] 4b. newlines admitted by the sanitizer (still no ESC, no C0)
  - [ ] 4c. `Shift+Enter` breaks the line, `Enter` sends
  - [ ] 4d. the box grows a line at a time up to 20, then scrolls
  - [ ] 4e. caret kept in sight; wheel scrolls; a thumb shows where
  - [ ] 4f. up/down walk the wrapped lines; a press places the caret,
        a drag selects
  - [ ] 4g. held keys repeat inside a field
- [ ] 5. Skills of the selected harness
  - [ ] 5a. where each harness keeps its skills, and how each is called
  - [ ] 5b. discovery: frontmatter parsed, names checked, reads capped
  - [ ] 5c. the token under the caret opens a list, filtered as typed
  - [ ] 5d. arrows pick, Tab/Enter accept, Esc dismisses the list only
  - [ ] 5e. the invocation reaches the harness as a call, verified live
- [ ] 6. Thumbnail of what is being exported, at the dialog's foot
  - [ ] 6a. `export::fit_view`: the scope fitted into a box
  - [ ] 6b. the layout keeps the foot for it
  - [ ] 6c. rendered through the same path as `board.png`, uploaded to a
        slot of its own, redrawn when its size changes
- [ ] 7. Docs: README, AGENTS.md, the design note
- [ ] 8. Live check of every item in a running window; CI green
