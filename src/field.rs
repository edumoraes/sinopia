//! An editable value: the string, the caret, the selection, and the keys
//! that move them. It does not know what it is naming — the dialog's
//! instruction and a layer's name are the same widget, one holding lines
//! and the other a single one.

use crate::scene::{Prim, ScreenRect};
use crate::text::{Atlas, Line};
use crate::theme::Theme;

/// The caret's width, in logical px.
const CARET_W: f32 = 1.5;
/// How far the text sits in from the field's own edge, in logical px.
pub const PADDING: f32 = 6.0;

/// What is selected is painted in the selection's own blue, thinned so
/// the ink over it still reads — the same blue that frames a selected
/// object on the canvas, since both say "this is what you picked".
fn highlight(theme: &Theme) -> crate::scene::Rgba {
    crate::scene::with_alpha(theme.selection, 0.3)
}

/// What a paste leaves once it is in a field. A line breaks where the
/// source broke it — as `\n`, however the source spelt it — in a field
/// that holds lines, and runs on after a space in one that does not,
/// where a break at the very end is only where the copy stopped. A tab
/// is the spaces it stood for. Anything else that is not a printable
/// character is dropped: a field holds only what it can show, and an
/// escape pasted out of a terminal is not text anybody meant.
fn clean(text: &str, lines: bool) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = if lines {
        text.as_str()
    } else {
        text.trim_end_matches('\n')
    };
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' if lines => out.push('\n'),
            '\n' => out.push(' '),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Which of `lines` character `i` is on: the last one starting at or
/// before it. Where a line wrapped without a newline, its end is the next
/// one's start, and a caret there stands at the head of the next line —
/// which is where the text it is in front of is.
fn line_of(lines: &[Line], i: usize) -> usize {
    lines.iter().rposition(|l| l.start <= i).unwrap_or(0)
}

/// Where a box `view_h` px tall has to be scrolled to, in px, for line
/// `k` of lines `line_h` tall to be in sight: as little as it takes, and
/// not at all when it already is.
pub fn follow(scroll: f32, k: usize, line_h: f32, view_h: f32) -> f32 {
    let top = k as f32 * line_h;
    let bottom = top + line_h;
    if top < scroll {
        top
    } else if bottom > scroll + view_h {
        bottom - view_h
    } else {
        scroll
    }
}

/// Where a field that holds lines is drawn: the box, the lines the text
/// wraps to at the box's width, how far down them it is scrolled, how
/// tall one line stands, and how far the text is set in from the box's
/// edge — every length in px.
#[derive(Debug, Clone, Copy)]
pub struct Boxed<'a> {
    pub rect: ScreenRect,
    pub lines: &'a [Line],
    pub scroll: f32,
    pub line_h: f32,
    pub inset: f32,
}

impl Boxed<'_> {
    /// Where line `k`'s top stands on screen.
    pub fn top(&self, k: usize) -> f32 {
        self.rect.y + self.inset + k as f32 * self.line_h - self.scroll
    }

    /// Which line a point on screen is over, and how far along it from
    /// where the text starts. Above the box is the line scrolled out of
    /// sight up there — which is where a drag past the top edge goes.
    pub fn at(&self, x: f64, y: f64) -> (usize, f32) {
        let down = (y as f32 - self.rect.y - self.inset + self.scroll) / self.line_h;
        (down.max(0.0) as usize, x as f32 - self.rect.x - self.inset)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Field {
    value: String,
    /// The caret's place, counted in characters and never in bytes: a
    /// byte index would split a multi-byte character and panic on the
    /// next insert.
    caret: usize,
    /// Where a selection was started from, counted the same way. The
    /// selection is everything between it and the caret, whichever way
    /// round; `None`, or the caret's own place, is no selection at all.
    anchor: Option<usize>,
    /// Whether it holds lines: what a paste's line breaks become.
    lines: bool,
    /// The most it holds, in bytes, when it has a most.
    limit: Option<usize>,
}

impl Field {
    /// A field holding `value`, with the caret after it — a field is
    /// opened to be added to, not to be retyped.
    pub fn new(value: &str) -> Field {
        Field {
            value: value.to_owned(),
            caret: value.chars().count(),
            anchor: None,
            lines: false,
            limit: None,
        }
    }

    /// A field that holds lines: a newline breaks one, and a paste keeps
    /// the breaks it came with.
    pub fn lines(value: &str) -> Field {
        Field {
            lines: true,
            ..Field::new(value)
        }
    }

    /// The same field, holding no more than `bytes`: whatever is typed or
    /// pasted past that is not taken, so a field never holds what the
    /// thing it feeds would refuse.
    pub fn limited(self, bytes: usize) -> Field {
        Field {
            limit: Some(bytes),
            ..self
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// The caret's place, in characters.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Which of `lines` the caret is on.
    pub fn caret_line(&self, lines: &[Line]) -> usize {
        line_of(lines, self.caret)
    }

    /// The value cut into the lines a box `width` px wide shows.
    pub fn wrap(&self, atlas: &Atlas, width: f32) -> Vec<Line> {
        atlas.wrap(&self.value, width)
    }

    /// The furthest a caret goes along line `k`: its end — or, where the
    /// line broke at a space that hangs past the edge, the place before
    /// that space, since past it is the next line's start and End would
    /// take the caret down a line. A word broken mid-word has no space to
    /// hang, and its end is between its two halves.
    fn last_index(&self, lines: &[Line], k: usize) -> usize {
        let line = lines[k];
        let soft = lines.get(k + 1).is_some_and(|next| next.start == line.end);
        let hangs = soft
            && line.end > line.start
            && self
                .value
                .chars()
                .nth(line.end - 1)
                .is_some_and(char::is_whitespace);
        if hangs { line.end - 1 } else { line.end }
    }

    /// How far along `line` character `i` stands, in px.
    fn x_at(&self, atlas: &Atlas, line: Line, i: usize) -> f32 {
        atlas.measure(&self.value[self.byte(line.start)..self.byte(i)])
    }

    /// The place between two characters of line `k` nearest to `x` px
    /// along it. A line past the last is the last: a press below the
    /// text means the end of it.
    pub fn index_at(&self, atlas: &Atlas, lines: &[Line], k: usize, x: f32) -> usize {
        let k = k.min(lines.len().saturating_sub(1));
        let (line, last) = (lines[k], self.last_index(lines, k));
        let mut pen = 0.0;
        let along = self.value.chars().enumerate().skip(line.start);
        for (i, c) in along.take(last - line.start) {
            let advance = atlas.advance(c);
            if x < pen + advance / 2.0 {
                return i;
            }
            pen += advance;
        }
        last
    }

    /// The line above, at the same x — or the start, from the first.
    pub fn up(&mut self, atlas: &Atlas, lines: &[Line], extend: bool) {
        let k = line_of(lines, self.caret);
        if k == 0 {
            return self.go(0, extend);
        }
        let x = self.x_at(atlas, lines[k], self.caret);
        let to = self.index_at(atlas, lines, k - 1, x);
        self.go(to, extend);
    }

    /// The line below, at the same x — or the end, from the last.
    pub fn down(&mut self, atlas: &Atlas, lines: &[Line], extend: bool) {
        let k = line_of(lines, self.caret);
        if k + 1 >= lines.len() {
            return self.go(self.len(), extend);
        }
        let x = self.x_at(atlas, lines[k], self.caret);
        let to = self.index_at(atlas, lines, k + 1, x);
        self.go(to, extend);
    }

    /// The start of the line the caret is on, as the box shows it.
    pub fn line_home(&mut self, lines: &[Line], extend: bool) {
        let k = line_of(lines, self.caret);
        self.go(lines[k].start, extend);
    }

    /// The end of the line the caret is on, as the box shows it.
    pub fn line_end(&mut self, lines: &[Line], extend: bool) {
        let k = line_of(lines, self.caret);
        self.go(self.last_index(lines, k), extend);
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// The byte offset character `i` starts at.
    fn byte(&self, i: usize) -> usize {
        self.value
            .char_indices()
            .nth(i)
            .map_or(self.value.len(), |(b, _)| b)
    }

    /// The selection as characters `start..end`, when there is one.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| (anchor.min(self.caret), anchor.max(self.caret)))
    }

    /// What is selected, or nothing.
    pub fn selected(&self) -> &str {
        match self.selection() {
            Some((a, b)) => &self.value[self.byte(a)..self.byte(b)],
            None => "",
        }
    }

    /// Takes the selection out of the value, and says whether there was
    /// one: whatever is typed, pasted or deleted next spends it.
    fn take_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let (from, to) = (self.byte(a), self.byte(b));
        self.value.replace_range(from..to, "");
        self.caret = a;
        self.anchor = None;
        true
    }

    pub fn insert(&mut self, c: char) {
        self.insert_str(c.encode_utf8(&mut [0; 4]));
    }

    /// `s` at the caret, over the selection if there is one, with the
    /// caret after it — or as much of it as the limit leaves room for,
    /// cut between two characters and never inside one.
    pub fn insert_str(&mut self, s: &str) {
        self.take_selection();
        let room = self
            .limit
            .map_or(usize::MAX, |l| l.saturating_sub(self.value.len()));
        let mut fits = s.len().min(room);
        while !s.is_char_boundary(fits) {
            fits -= 1;
        }
        let s = &s[..fits];
        let at = self.byte(self.caret);
        self.value.insert_str(at, s);
        self.caret += s.chars().count();
    }

    /// What the clipboard held, put in the way this field holds text:
    /// see [`clean`].
    pub fn paste(&mut self, text: &str) {
        self.insert_str(&clean(text, self.lines));
    }

    /// Breaks the line at the caret, in a field that holds lines; a
    /// field of one line has nowhere to put a second.
    pub fn newline(&mut self) {
        if self.lines {
            self.insert('\n');
        }
    }

    pub fn backspace(&mut self) {
        if self.take_selection() || self.caret == 0 {
            return;
        }
        self.caret -= 1;
        let at = self.byte(self.caret);
        self.value.remove(at);
    }

    /// The character after the caret, or the selection.
    pub fn delete(&mut self) {
        if self.take_selection() || self.caret == self.len() {
            return;
        }
        let at = self.byte(self.caret);
        self.value.remove(at);
    }

    /// The selection, handed over and taken out; `None` when nothing is
    /// selected, so a cut of nothing leaves the clipboard alone.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected().to_owned();
        if text.is_empty() {
            return None;
        }
        self.take_selection();
        Some(text)
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.len();
    }

    /// Puts the caret at `to`. With `extend` the selection runs from
    /// where it started — or from where the caret stood, if nothing was
    /// selected — to there; without, nothing is selected.
    pub fn go(&mut self, to: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = to.min(self.len());
    }

    /// One character back — or, with a selection and no `extend`, to
    /// the selection's start, which is where the eye already is.
    pub fn left(&mut self, extend: bool) {
        match self.selection() {
            Some((start, _)) if !extend => self.go(start, false),
            _ => self.go(self.caret.saturating_sub(1), extend),
        }
    }

    /// One character on — or to the selection's end.
    pub fn right(&mut self, extend: bool) {
        match self.selection() {
            Some((_, end)) if !extend => self.go(end, false),
            _ => self.go(self.caret + 1, extend),
        }
    }

    pub fn home(&mut self, extend: bool) {
        self.go(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.go(self.len(), extend);
    }

    /// Back to the start of the word the caret is in, or of the one
    /// before it when it is already at a start.
    pub fn word_left(&mut self, extend: bool) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.caret;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.go(i, extend);
    }

    /// On to the end of the word the caret is in, or of the next one.
    pub fn word_right(&mut self, extend: bool) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.caret;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        self.go(i, extend);
    }

    /// The lines of a field that holds them, drawn into `b`: each one in
    /// sight a line under the one before, whatever is selected on it
    /// painted under its text, and the caret on its own line. Everything
    /// is cut to the box, so a line half scrolled away is half drawn.
    pub fn prims_boxed(
        &self,
        b: &Boxed,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
        focused: bool,
    ) -> Vec<Prim> {
        let mut out = Vec::new();
        let x0 = b.rect.x + b.inset;
        // Every character's byte offset, and the end's: one walk, rather
        // than one per line.
        let bytes: Vec<usize> = self
            .value
            .char_indices()
            .map(|(i, _)| i)
            .chain([self.value.len()])
            .collect();
        let text = |from: usize, to: usize| &self.value[bytes[from]..bytes[to]];
        let mut caret = None;
        for (k, line) in b.lines.iter().enumerate() {
            let top = b.top(k);
            if top + b.line_h < b.rect.y || top > b.rect.y + b.rect.h {
                continue;
            }
            if let Some((a, z)) = self.selection()
                && a <= line.end
                && z >= line.start
            {
                let (from, to) = (a.max(line.start), z.min(line.end));
                // A selection running on past a newline takes it along,
                // and says so with a space's width of band.
                let newline =
                    z > line.end && b.lines.get(k + 1).is_some_and(|n| n.start > line.end);
                let w =
                    atlas.measure(text(from, to)) + if newline { atlas.advance(' ') } else { 0.0 };
                if w > 0.0 {
                    let band = ScreenRect {
                        x: x0 + atlas.measure(text(line.start, from)),
                        y: top,
                        w,
                        h: b.line_h,
                    };
                    out.push(Prim::rect(band, highlight(theme)).clipped(b.rect));
                }
            }
            let row = ScreenRect {
                x: x0,
                y: top,
                w: b.rect.w,
                h: b.line_h,
            };
            let baseline = atlas.baseline_in(row);
            for g in atlas.layout(text(line.start, line.end), x0, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink).clipped(b.rect));
            }
            if focused && k == line_of(b.lines, self.caret) {
                caret = Some(ScreenRect {
                    x: x0 + atlas.measure(text(line.start, self.caret)),
                    y: top + 1.0,
                    w: CARET_W,
                    h: b.line_h - 2.0,
                });
            }
        }
        // Last, so nothing is drawn over it.
        if let Some(c) = caret {
            out.push(Prim::rect(c, theme.ink).clipped(b.rect));
        }
        out
    }

    /// The value written into `r`, with a caret after the character the
    /// caret is at when the field has the keyboard. The rect is the
    /// field's whole box; the text is inset by [`PADDING`], which is
    /// also where an empty field's caret stands.
    pub fn prims(
        &self,
        r: ScreenRect,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
        focused: bool,
    ) -> Vec<Prim> {
        let mut out = Vec::new();
        let baseline = atlas.baseline_in(r);
        let x = r.x + PADDING;
        // The selection goes down first, so the text it covers stays on
        // top of it and reads.
        if let Some((a, b)) = self.selection() {
            let band = ScreenRect {
                x: x + atlas.measure(&self.value[..self.byte(a.min(b))]),
                y: r.y + PADDING * 0.5,
                w: atlas.measure(self.selected()),
                h: r.h - PADDING,
            };
            out.push(Prim::rect(band, highlight(theme)).clipped(r));
        }
        for g in atlas.layout(&self.value, x, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink).clipped(r));
        }
        if focused {
            let ahead: String = self.value.chars().take(self.caret).collect();
            let caret = ScreenRect {
                x: x + atlas.measure(&ahead),
                y: r.y + PADDING * 0.5,
                w: CARET_W,
                h: r.h - PADDING,
            };
            out.push(Prim::rect(caret, theme.ink).clipped(r));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_inserts_at_the_caret_and_carries_it_along() {
        let mut f = Field::new("");
        for c in "auth".chars() {
            f.insert(c);
        }
        assert_eq!(f.value(), "auth");
        f.left(false);
        f.insert('-');
        assert_eq!(f.value(), "aut-h");
    }

    #[test]
    fn a_field_opens_with_the_caret_at_the_end_of_what_it_was_given() {
        let mut f = Field::new("login");
        f.insert('!');
        assert_eq!(f.value(), "login!");
    }

    #[test]
    fn backspace_takes_the_character_before_the_caret_and_nothing_at_the_start() {
        let mut f = Field::new("ab");
        f.backspace();
        assert_eq!(f.value(), "a");
        f.home(false);
        f.backspace();
        assert_eq!(f.value(), "a");
    }

    #[test]
    fn the_caret_walks_by_characters_and_not_by_bytes() {
        // "ç" is two bytes: a caret counting bytes would split it and
        // panic on the next insert.
        let mut f = Field::new("ação");
        f.left(false);
        f.left(false);
        f.insert('-');
        assert_eq!(f.value(), "aç-ão");
    }

    #[test]
    fn the_caret_stops_at_both_ends() {
        let mut f = Field::new("x");
        f.right(false);
        f.right(false);
        f.insert('y');
        assert_eq!(f.value(), "xy");
        f.home(false);
        f.left(false);
        f.insert('w');
        assert_eq!(f.value(), "wxy");
    }

    #[test]
    fn shift_with_an_arrow_selects_and_what_is_typed_replaces_the_selection() {
        let mut f = Field::new("plan");
        f.left(true);
        f.left(true);
        assert_eq!(f.selected(), "an");
        f.insert('X');
        assert_eq!(f.value(), "plX");
        assert_eq!(f.selected(), "", "typing spends the selection");
    }

    #[test]
    fn a_selection_reads_the_same_whichever_way_it_was_made() {
        let mut f = Field::new("board");
        f.home(false);
        f.right(true);
        f.right(true);
        assert_eq!(f.selected(), "bo");
        f.end(false);
        f.left(true);
        f.left(true);
        assert_eq!(f.selected(), "rd");
    }

    #[test]
    fn an_arrow_without_shift_lets_go_of_the_selection_at_its_own_end() {
        let mut f = Field::new("abcdef");
        f.left(true);
        f.left(true);
        f.left(false);
        f.insert('|');
        assert_eq!(f.value(), "abcd|ef", "left lands on the start");
        let mut f = Field::new("abcdef");
        f.home(false);
        f.right(true);
        f.right(true);
        f.right(false);
        f.insert('|');
        assert_eq!(f.value(), "ab|cdef", "right lands on the end");
    }

    #[test]
    fn select_all_then_backspace_empties_the_field() {
        let mut f = Field::new("ação");
        f.select_all();
        assert_eq!(f.selected(), "ação");
        f.backspace();
        assert_eq!(f.value(), "");
    }

    #[test]
    fn delete_takes_the_character_after_the_caret_or_the_selection() {
        let mut f = Field::new("abc");
        f.home(false);
        f.delete();
        assert_eq!(f.value(), "bc");
        f.end(false);
        f.delete();
        assert_eq!(f.value(), "bc", "nothing after the end");
        f.select_all();
        f.delete();
        assert_eq!(f.value(), "");
    }

    #[test]
    fn a_string_goes_in_at_the_caret_over_the_selection() {
        let mut f = Field::new("draw the flow");
        f.home(false);
        for _ in 0.."draw".len() {
            f.right(true);
        }
        f.insert_str("sketch");
        assert_eq!(f.value(), "sketch the flow");
        f.insert_str("!");
        assert_eq!(f.value(), "sketch! the flow", "the caret went after it");
    }

    #[test]
    fn cut_hands_the_selection_over_and_leaves_the_rest() {
        let mut f = Field::new("keep cut");
        f.left(true);
        f.left(true);
        f.left(true);
        assert_eq!(f.cut().as_deref(), Some("cut"));
        assert_eq!(f.value(), "keep ");
        assert_eq!(f.cut(), None, "nothing selected is nothing to cut");
    }

    #[test]
    fn a_word_step_stops_where_a_word_starts_and_where_it_ends() {
        let mut f = Field::new("draw the auth flow");
        f.word_left(false);
        f.insert('|');
        assert_eq!(f.value(), "draw the auth |flow");
        f.home(false);
        f.word_right(false);
        f.insert('|');
        assert_eq!(f.value(), "draw| the auth |flow");
        f.word_right(true);
        assert_eq!(f.selected(), " the");
    }

    #[test]
    fn a_paste_into_a_line_runs_its_lines_on_with_a_space() {
        let mut f = Field::new("");
        f.paste("draw\nthe\r\nflow\tnow\n");
        assert_eq!(f.value(), "draw the flow    now", "and no trailing break");
    }

    #[test]
    fn a_paste_into_a_box_keeps_its_lines_however_they_were_spelt() {
        let mut f = Field::lines("");
        f.paste("one\r\ntwo\rthree\nfour");
        assert_eq!(f.value(), "one\ntwo\nthree\nfour");
    }

    #[test]
    fn a_box_takes_a_newline_and_a_line_does_not() {
        let mut line = Field::new("a");
        line.newline();
        assert_eq!(line.value(), "a", "a name is one line");
        let mut boxed = Field::lines("a");
        boxed.newline();
        boxed.insert('b');
        assert_eq!(boxed.value(), "a\nb");
    }

    #[test]
    fn a_paste_drops_what_a_field_cannot_show() {
        // An escape pasted out of a terminal is not text anybody meant,
        // and a field holds only what it can show.
        let mut f = Field::new("");
        f.paste("red\x1b[31m\x07bell\u{0}");
        assert_eq!(f.value(), "red[31mbell");
    }

    #[test]
    fn a_paste_past_the_limit_is_cut_between_characters() {
        let mut f = Field::new("ab").limited(5);
        f.paste("çççç");
        assert_eq!(f.value(), "abç", "another ç would make six bytes");
    }

    #[test]
    fn typing_at_the_limit_types_nothing_but_a_selection_makes_room() {
        let mut f = Field::new("abcde").limited(5);
        f.insert('f');
        assert_eq!(f.value(), "abcde");
        f.left(true);
        f.insert('X');
        assert_eq!(f.value(), "abcdX", "the selection is spent first");
    }

    fn atlas() -> Atlas {
        Atlas::build(&crate::text::Font::bundled(), 13)
    }

    #[test]
    fn up_and_down_keep_the_caret_over_the_same_place_in_the_line() {
        let a = atlas();
        let mut f = Field::lines("abcde\nabcdefgh\nabc");
        let lines = f.wrap(&a, 1000.0);
        assert_eq!(f.caret(), 18);
        f.up(&a, &lines, false);
        assert_eq!(f.caret(), 9, "after the abc of the middle line");
        f.up(&a, &lines, false);
        assert_eq!(f.caret(), 3);
        f.up(&a, &lines, false);
        assert_eq!(f.caret(), 0, "up from the first line is its start");
        f.down(&a, &lines, false);
        assert_eq!(f.caret(), 6);
        f.down(&a, &lines, true);
        assert_eq!(f.selected(), "abcdefgh\n");
        f.down(&a, &lines, false);
        assert_eq!(f.caret(), 18, "down from the last line is its end");
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_line_the_caret_is_on() {
        let a = atlas();
        let mut f = Field::lines("abcde\nabcdefgh\nabc");
        let lines = f.wrap(&a, 1000.0);
        f.go(9, false);
        f.line_home(&lines, false);
        assert_eq!(f.caret(), 6);
        f.line_end(&lines, true);
        assert_eq!(f.selected(), "abcdefgh");
    }

    #[test]
    fn the_end_of_a_wrapped_line_is_before_the_space_it_broke_at() {
        // Past it would be the next line's start, and End would take the
        // caret down a line instead of along one.
        let a = atlas();
        let mut f = Field::lines("one two three");
        let lines = f.wrap(&a, a.measure("one two th"));
        assert_eq!(lines.len(), 2);
        f.go(2, false);
        f.line_end(&lines, false);
        assert_eq!(f.caret(), 7, "after `two`");
    }

    #[test]
    fn the_end_of_a_word_broken_mid_word_is_after_its_last_character() {
        // With no space to hang past the edge, the line's end is the
        // next line's start: the place between the two halves of the
        // word, which is what End and a press past the line mean.
        let a = atlas();
        let mut f = Field::lines(&"a".repeat(14));
        let lines = f.wrap(&a, a.measure("aaaaaaa") + 0.5);
        assert_eq!(lines[0], Line { start: 0, end: 7 });
        f.go(2, false);
        f.line_end(&lines, false);
        assert_eq!(f.caret(), 7);
        f.go(2, false);
        f.line_end(&lines, true);
        assert_eq!(f.selected(), "aaaaa");
        assert_eq!(f.index_at(&a, &lines, 0, 999.0), 7);
    }

    #[test]
    fn a_press_on_a_line_lands_between_the_two_nearest_characters() {
        let a = atlas();
        let f = Field::lines("abcde\nabcdefgh\nabc");
        let lines = f.wrap(&a, 1000.0);
        let x = a.measure("abc") + a.measure("d") * 0.3;
        assert_eq!(f.index_at(&a, &lines, 1, x), 9);
        let x = a.measure("abc") + a.measure("d") * 0.7;
        assert_eq!(f.index_at(&a, &lines, 1, x), 10);
        assert_eq!(f.index_at(&a, &lines, 1, -5.0), 6, "before it is its start");
        assert_eq!(f.index_at(&a, &lines, 1, 999.0), 14, "past it is its end");
        assert_eq!(
            f.index_at(&a, &lines, 7, 0.0),
            15,
            "below the text is the last line"
        );
    }

    #[test]
    fn the_caret_is_on_the_line_the_box_shows_it_on() {
        let a = atlas();
        let mut f = Field::lines("one two three\nfour");
        let lines = f.wrap(&a, a.measure("one two th"));
        assert_eq!(f.caret_line(&lines), 2);
        f.go(8, false);
        assert_eq!(
            f.caret_line(&lines),
            1,
            "a soft break's end is the next line's start"
        );
        f.go(7, false);
        assert_eq!(f.caret_line(&lines), 0);
    }

    #[test]
    fn the_line_the_caret_is_on_is_kept_in_sight() {
        // Twenty-five-px lines, a box a hundred high: four lines show.
        assert_eq!(follow(0.0, 2, 25.0, 100.0), 0.0, "already in sight");
        assert_eq!(
            follow(0.0, 5, 25.0, 100.0),
            50.0,
            "brought up to the bottom"
        );
        assert_eq!(
            follow(100.0, 1, 25.0, 100.0),
            25.0,
            "brought down to the top"
        );
    }

    /// A box 200 px wide at the origin, `h` tall, lines 20 px apart and
    /// the text inset by 6.
    fn boxed<'a>(lines: &'a [Line], h: f32, scroll: f32) -> Boxed<'a> {
        Boxed {
            rect: ScreenRect {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h,
            },
            lines,
            scroll,
            line_h: 20.0,
            inset: 6.0,
        }
    }

    fn glyphs(prims: &[Prim]) -> Vec<ScreenRect> {
        prims
            .iter()
            .filter(|p| p.kind == crate::scene::KIND_IMAGE)
            .map(Prim::bounds)
            .collect()
    }

    #[test]
    fn a_box_draws_its_lines_one_under_the_other() {
        let (a, theme) = (atlas(), Theme::light());
        let f = Field::lines("aa\naa\naa");
        let lines = f.wrap(&a, 188.0);
        let prims = f.prims_boxed(&boxed(&lines, 100.0, 0.0), &a, 0, &theme, false);
        let g = glyphs(&prims);
        assert_eq!(g.len(), 6);
        assert!((g[2].y - g[0].y - 20.0).abs() < 0.01, "a line apart");
        assert!((g[4].y - g[0].y - 40.0).abs() < 0.01);
    }

    #[test]
    fn a_scrolled_box_draws_what_is_in_sight_and_cuts_the_rest() {
        let (a, theme) = (atlas(), Theme::light());
        let f = Field::lines(&["a"; 10].join("\n"));
        let lines = f.wrap(&a, 188.0);
        let b = boxed(&lines, 72.0, 100.0);
        let prims = f.prims_boxed(&b, &a, 0, &theme, false);
        let g = glyphs(&prims);
        assert!(!g.is_empty() && g.len() < 10, "{} of 10", g.len());
        let cut = [b.rect.x, b.rect.y, b.rect.w, b.rect.h];
        assert!(prims.iter().all(|p| p.clip == cut), "all cut to the box");
        // Scrolled 100 px: the lines wholly above the box are not drawn.
        assert!(g.iter().all(|r| r.y > b.rect.y - 20.0), "{g:?}");
    }

    #[test]
    fn the_caret_stands_on_its_own_line() {
        let (a, theme) = (atlas(), Theme::light());
        let mut f = Field::lines("aa\naa");
        let lines = f.wrap(&a, 188.0);
        let b = boxed(&lines, 100.0, 0.0);
        let low = f.prims_boxed(&b, &a, 0, &theme, true).pop().unwrap();
        f.go(1, false);
        let high = f.prims_boxed(&b, &a, 0, &theme, true).pop().unwrap();
        assert!((low.bounds().y - high.bounds().y - 20.0).abs() < 0.01);
        assert!((high.bounds().x - (6.0 + a.measure("a"))).abs() < 0.01);
    }

    #[test]
    fn a_selection_over_three_lines_is_a_band_on_each() {
        let (a, theme) = (atlas(), Theme::light());
        let mut f = Field::lines("aa\naa\naa");
        let lines = f.wrap(&a, 188.0);
        let b = boxed(&lines, 100.0, 0.0);
        let plain = f.prims_boxed(&b, &a, 0, &theme, false).len();
        f.go(1, false);
        f.go(7, true);
        assert_eq!(f.prims_boxed(&b, &a, 0, &theme, false).len(), plain + 3);
    }

    #[test]
    fn a_press_in_a_box_is_a_line_and_a_place_along_it() {
        let lines = [Line { start: 0, end: 0 }; 10];
        let b = boxed(&lines, 72.0, 100.0);
        // 6 px of inset, then 100 px scrolled away: the first line in
        // sight is the sixth.
        assert_eq!(b.at(40.0, 6.0 + 1.0), (5, 34.0));
        assert_eq!(b.at(40.0, 6.0 + 21.0), (6, 34.0));
        assert_eq!(b.at(40.0, -30.0).0, 3, "above the box scrolls back");
    }

    #[test]
    fn a_selection_is_painted_under_the_text_it_covers() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = Theme::light();
        let r = ScreenRect {
            x: 10.0,
            y: 0.0,
            w: 300.0,
            h: 24.0,
        };
        let mut f = Field::new("keep this");
        let plain = f.prims(r, &atlas, 0, &theme, true);
        for _ in 0.."this".len() {
            f.left(true);
        }
        let lit = f.prims(r, &atlas, 0, &theme, true);
        assert_eq!(lit.len(), plain.len() + 1, "one band more");
        let band = lit[0].bounds();
        assert!((band.x - (r.x + PADDING + atlas.measure("keep "))).abs() < 0.5);
        assert!((band.w - atlas.measure("this")).abs() < 0.5);
    }

    #[test]
    fn a_focused_field_draws_a_caret_and_an_unfocused_one_does_not() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = Theme::light();
        let r = ScreenRect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 24.0,
        };
        let f = Field::new("hi");
        let focused = f.prims(r, &atlas, 0, &theme, true);
        let idle = f.prims(r, &atlas, 0, &theme, false);
        assert_eq!(focused.len(), idle.len() + 1, "the caret is the extra prim");
    }

    #[test]
    fn the_caret_sits_after_the_text_it_follows() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = Theme::light();
        let r = ScreenRect {
            x: 10.0,
            y: 0.0,
            w: 200.0,
            h: 24.0,
        };
        let mut f = Field::new("hi");
        let after = f.prims(r, &atlas, 0, &theme, true).pop().unwrap();
        f.home(false);
        let before = f.prims(r, &atlas, 0, &theme, true).pop().unwrap();
        assert!(after.bounds().x > before.bounds().x);
        assert!((before.bounds().x - (r.x + PADDING)).abs() < 1.0);
    }
}
