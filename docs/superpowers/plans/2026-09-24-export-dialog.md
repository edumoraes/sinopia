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
- [x] 3. Copy and paste in the input
  - [x] 3a. `Field` selection: anchor, extend, select all, replace
  - [x] 3b. pasted text cleaned for the field it lands in
  - [x] 3c. clipboard reads text (`Ctrl+V`), never types a `v`
  - [x] 3d. clipboard writes text (`Ctrl+C`, `Ctrl+X`)
- [x] 4. Multi-line input that grows, wraps at a maximum width, and
      scrolls past 20 lines
  - [x] 4a. word wrap in `text`
  - [x] 4b. newlines admitted by the sanitizer (still no ESC, no C0)
  - [x] 4c. `Shift+Enter` breaks the line, `Enter` sends
  - [x] 4d. the box grows a line at a time up to 20, then scrolls
  - [x] 4e. caret kept in sight; wheel scrolls; a thumb shows where
  - [x] 4f. up/down walk the wrapped lines; a press places the caret,
        a drag selects
  - [x] 4g. held keys repeat inside a field
- [x] 5. Skills of the selected harness
  - [x] 5a. where each harness keeps its skills, and how each is called
  - [x] 5b. discovery: frontmatter parsed, names checked, reads capped
  - [x] 5c. the token under the caret opens a list, filtered as typed
  - [x] 5d. arrows pick, Tab/Enter accept, Esc dismisses the list only
  - [x] 5e. the invocation reaches the harness as a call, verified live
- [x] 6. Thumbnail of what is being exported, at the dialog's foot
  - [x] 6a. `export::fit_view`: the scope fitted into a box
  - [x] 6b. the layout keeps the foot for it
  - [x] 6c. rendered through the same path as `board.png`, uploaded to a
        slot of its own, redrawn when its size changes
- [x] 7. Docs: README, AGENTS.md, the design note
- [x] 8. Live check of every item in a running window; CI green

## After the review

An independent read of the branch found eight faults; each is fixed
under a test of its own, and one older fault with them.

- [x] held Enter, Tab and Esc no longer repeat through the dialog
- [x] no skill call is typed at an agent herdr reports blocked
- [x] a Claude plugin's skill is called by its folder
- [x] End on a word broken mid-word goes after its last letter
- [x] a cut scrolls, settles the menu, and needs a clipboard
- [x] the skills menu stands only while its line is in sight
- [x] a name holds no more than a directory name may
- [x] the picture's ceiling is a logical density
- [x] coming back to the window keeps the agent aimed at (older)

## After a first look

- [x] the picture at the foot is half the size: at most half the
      dialog's width and 80 px tall
- [x] no folder field for a selection that is not a frame: a page is
      named after the layer that owns it, or else after the tab
