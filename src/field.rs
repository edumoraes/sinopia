//! A one-line editable value: the string, the caret, and the keys that
//! move it. It does not know what it is naming — the panel's instruction
//! and a layer's name are the same widget.

use crate::scene::{Prim, ScreenRect};
use crate::text::Atlas;
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
}

impl Field {
    /// A field holding `value`, with the caret after it — a field is
    /// opened to be added to, not to be retyped.
    pub fn new(value: &str) -> Field {
        Field {
            value: value.to_owned(),
            caret: value.chars().count(),
            anchor: None,
        }
    }

    pub fn value(&self) -> &str {
        &self.value
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
    /// caret after it — which is what a paste is.
    pub fn insert_str(&mut self, s: &str) {
        self.take_selection();
        let at = self.byte(self.caret);
        self.value.insert_str(at, s);
        self.caret += s.chars().count();
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
        let r = ScreenRect { x: 0.0, y: 0.0, w: 200.0, h: 24.0 };
        let f = Field::new("hi");
        let focused = f.prims(r, &atlas, 0, &theme, true);
        let idle = f.prims(r, &atlas, 0, &theme, false);
        assert_eq!(focused.len(), idle.len() + 1, "the caret is the extra prim");
    }

    #[test]
    fn the_caret_sits_after_the_text_it_follows() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = Theme::light();
        let r = ScreenRect { x: 10.0, y: 0.0, w: 200.0, h: 24.0 };
        let mut f = Field::new("hi");
        let after = f.prims(r, &atlas, 0, &theme, true).pop().unwrap();
        f.home(false);
        let before = f.prims(r, &atlas, 0, &theme, true).pop().unwrap();
        assert!(after.bounds().x > before.bounds().x);
        assert!((before.bounds().x - (r.x + PADDING)).abs() < 1.0);
    }
}
