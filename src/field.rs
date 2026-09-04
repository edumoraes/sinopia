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

#[derive(Debug, Clone, Default)]
pub struct Field {
    value: String,
    /// The caret's place, counted in characters and never in bytes: a
    /// byte index would split a multi-byte character and panic on the
    /// next insert.
    caret: usize,
}

impl Field {
    /// A field holding `value`, with the caret after it — a field is
    /// opened to be added to, not to be retyped.
    pub fn new(value: &str) -> Field {
        Field {
            value: value.to_owned(),
            caret: value.chars().count(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// The byte offset the caret sits at.
    fn offset(&self) -> usize {
        self.value
            .char_indices()
            .nth(self.caret)
            .map_or(self.value.len(), |(i, _)| i)
    }

    pub fn insert(&mut self, c: char) {
        let at = self.offset();
        self.value.insert(at, c);
        self.caret += 1;
    }

    pub fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        self.caret -= 1;
        let at = self.offset();
        self.value.remove(at);
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.value.chars().count());
    }

    pub fn home(&mut self) {
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.caret = self.value.chars().count();
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
        f.left();
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
        f.home();
        f.backspace();
        assert_eq!(f.value(), "a");
    }

    #[test]
    fn the_caret_walks_by_characters_and_not_by_bytes() {
        // "ç" is two bytes: a caret counting bytes would split it and
        // panic on the next insert.
        let mut f = Field::new("ação");
        f.left();
        f.left();
        f.insert('-');
        assert_eq!(f.value(), "aç-ão");
    }

    #[test]
    fn the_caret_stops_at_both_ends() {
        let mut f = Field::new("x");
        f.right();
        f.right();
        f.insert('y');
        assert_eq!(f.value(), "xy");
        f.home();
        f.left();
        f.insert('w');
        assert_eq!(f.value(), "wxy");
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
        f.home();
        let before = f.prims(r, &atlas, 0, &theme, true).pop().unwrap();
        assert!(after.bounds().x > before.bounds().x);
        assert!((before.bounds().x - (r.x + PADDING)).abs() < 1.0);
    }
}
