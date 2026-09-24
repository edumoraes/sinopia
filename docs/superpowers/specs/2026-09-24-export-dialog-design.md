# The export dialog, second cut — design

`Ctrl+E` used to open a 420 px panel: a row per agent reading
`claude · /home/…/Work/board`, a one-line instruction that typed a `v`
when asked to paste, and nothing to say what was about to leave. This
cut makes it a dialog a person can work in. Six asks, and what each one
became.

## Wider

640 logical px, and never closer to the window's edge than 24 either
side — a narrow window is still a window the dialog has to fit. The
room goes to the rows and to the instruction's wrap width.

## A row is a mark, a name and a folder

**Mark.** Each vendor's own, from its own source, chosen because it
reads on a light panel and a dark one alike (`assets/agents/README.md`
has the sources): the Claude spark, the Codex app icon, the opencode
favicon, the heart Crush puts on its own notifications, the Gemini CLI
icon. The OpenAI blossom and opencode's square wordmark both ship as a
black file and a white one, and one of the two vanishes on whichever
ground the desktop's theme is not — the app would have had to pick per
theme, and a theme set mid-session would have left the wrong one up.
`tools/agent-logos.sh` sheets them into 40 px cells: twice the 20
logical px they are drawn at, a halving the bilinear sampler averages
cleanly at scale 1, and one to one at scale 2. An agent herdr knows and
this build does not gets its initial on a tile.

**Name.** What its makers call it (`Agent::name`); the process name
stays the kind.

**Folder.** The last directory of the working one — and, only for two
rows that would otherwise read alike, as many directories above it as it
takes (`agents::folders`). Two agents in the same directory are one
place and say it alike.

## Copy and paste

The field gains a selection (an anchor; the caret is the other end):
Shift with the arrows, Home and End, Ctrl a word at a time, Ctrl+A, the
pointer. Typing, deleting or pasting spends it. `edit` is the one
mapping of keys onto a field, for the dialog's and a layer rename's, and
a letter held with Ctrl or Super is a command there, never text.

`Ctrl+V` asks the clipboard for text — `text/plain;charset=utf-8`, then
`UTF8_STRING`, then `text/plain` — and cleans it for the field it lands
in. `Ctrl+C`/`Ctrl+X` make the board the selection's owner through a
`wl_data_source`. The one thing Wayland asks for that winit hides is the
serial of the input event a selection answers, so the clipboard's queue
binds its own `wl_keyboard` and `wl_pointer` on the seat and listens to
them for nothing but serials. Verified live: a copy read back through
`wl-paste`, and pasted back into the same field.

## A box that grows, wraps, and scrolls past twenty lines

`Shift+Enter` breaks a line; Enter still sends. The text wraps at a
fixed width — short of the scrollbar whether or not it shows, so nothing
rewraps when scrolling starts — after the last space that fits, the
space left hanging, or between two characters of a word wider than the
box. The box grows a line at a time to 20, then keeps its height and
scrolls; a window too short for 20 shows fewer. The arrows and Home/End
walk the lines as shown, the scroll follows the caret, the wheel moves
the lines, a thumb says where; a press places the caret and a drag
selects. Held keys repeat — in a field only.

The sanitizer admits the newline now. It was refused because the
instruction was one line; it is harmless where it lands, inside a
bracketed paste, where it is a line of the same message. ESC, CR, tab
and the rest of C0 stay refused. The cap goes to 16 KiB.

## The selected harness's skills

What was verified, from each harness's source at the version installed
here (Claude Code 2.1.280, Codex 0.154.0, OpenCode 1.18.30, Crush
0.96.1; Gemini CLI 0.61.0 from source only):

| harness | typed call | where it is read | pasted, then Enter |
| --- | --- | --- | --- |
| Claude Code | `/name args` | start of the message | only while the paste stays inline (≤ 800 chars, ≤ 3 lines) |
| Codex | `$name` | anywhere | yes — pastes are expanded before the scan |
| OpenCode | `/name args` | first character | yes, with a space after the name |
| Gemini CLI | `/name args` | start | yes, if Enter comes > 40 ms after the paste |
| Crush | none | — | skills reach its model and its palette only |

So the dialog opens a menu of the target's skills on `/` first in the
message, or `$` at any word for Codex; Crush gets `/` too, and taking a
skill there writes "Use the <name> skill." for its model to take up.
Where each harness keeps its skills — home, XDG config, how far up the
project it looks, whether the repository's root is the ceiling — and how
it names one is written down in `skills::harness`. Claude Code names a
skill by its folder and brings the skills of the plugins switched on for
the person, as `plugin:skill`.

**The one that needed measuring.** Every prompt this sends is more than
three lines — the file list alone is three — so in Claude Code it always
collapses into a placeholder, and a `/` inside it is text. Against
Claude Code 2.1.280 in tmux, with a probe skill that answers one fixed
word: the whole prompt pasted drew a generic reply about the diagram and
the skill never ran; the call typed as keys and the rest pasted, it ran
and answered `PROBE-OK`. So `agents::typed_call` splits the call off for
a harness whose `typed` says so, and `agents::send` types it —
`herdr pane send-text`, `tmux send-keys -l`, no Enter — before the paste
it always made. tmux now waits 300 ms before its Enter, as herdr does,
for Gemini's 40 ms.

## A picture of what leaves

At the foot: the scope's sub-document drawn offscreen through the same
path `board.png` is (`App::render_sub`, which both call now), fitted
into the foot at the size it is shown in px (`export::fit_view`, never
past 4 logical px a world unit — logical, so a small scope is as sharp
at scale 2 as at 1), uploaded into a slot of its own replaced in
place, and taken again when that size changes. The layout knows the
shape before the picture exists (`export::shape`): as wide as the dialog
for a wide scope, at most 160 px tall, and in a short window it gives
up height before the box gives up its last line — and goes once too
small to read.

## Security

- A skill's name comes out of a file in somebody's repository and may
  be typed into a live terminal, so it is offered only when
  `skills::callable` passes it: an ASCII letter or digit, then letters,
  digits and `-_.:`, at most 64 bytes. Nothing but that name, after `/`
  or `$`, is ever typed; the rest is pasted as before, and none of it
  becomes a shell word.
- `SKILL.md` is read from its head only, never past 64 KiB, through the
  same `O_NONBLOCK` open and regular-file check a capped read uses: a
  FIFO named `SKILL.md` cannot park the window's thread.
- A description is only ever drawn, and drawn cut to its row.
- herdr's `agent prompt` refuses a blocked agent before any input is
  sent; the keys a call is typed with go ahead of it and would not be.
  So the pane's status is asked of herdr right before typing, and a
  blocked agent — or one herdr cannot say anything about — is sent
  nothing: typed into the question it holds open, a digit in a name
  could pick one of its options.
- A held key repeats in a field only if it writes or moves. Enter,
  Tab and Esc finish something, and a repeat of them would send the
  unfinished prompt, re-aim the dialog or close it.

## What a review found

An independent read of the branch found eight faults, each now fixed
under a test of its own: held Enter/Tab/Esc repeating through the
dialog's commands; the typed call slipping past herdr's blocked guard;
a Claude plugin's skill offered under its frontmatter's name instead of
its folder's; End one letter short on a word broken mid-word; a cut
that neither scrolled nor settled the menu; the menu hanging over the
rows once its line was scrolled away; the name fields taking a
mebibyte of paste; and the picture's ceiling blurring small scopes at
scale 2. One more predated the branch — the target kept by index across
a re-listing that reorders — and is fixed with them.
