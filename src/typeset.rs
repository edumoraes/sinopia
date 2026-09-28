//! Setting a text: where its lines break, where each one stands, and
//! where every place between two characters is — which is what the
//! caret, the pointer and a selection all read.
//!
//! Everything here is in the text's own box, in world units, before the
//! box turns: `(0, 0)` is its top-left corner. What the renderer draws
//! and what the pointer hits are both read off the same [`Laid`], so a
//! caret cannot stand anywhere but between two letters that are drawn.
//!
//! Places are counted in characters, never bytes, as a field's caret is.

use crate::doc::{Align, Text, TextMode, TextStyle, Valign};
use crate::fonts::Fonts;

/// One line as it is set: characters `start..end` of the text — a line a
/// newline ended does not hold the newline — and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    /// The top of the line's own band, `leading` times the size tall.
    pub top: f64,
    pub baseline: f64,
    /// Where each place of the line stands across the box: one more
    /// than the characters on it, the first before the first character
    /// and the last after the last. Alignment and justification are in
    /// them already.
    pub stops: Vec<f64>,
    /// How wide the line's ink is: to the end of its last character that
    /// is not a space.
    pub width: f64,
    /// Whether it ends a paragraph — a newline, or the end of the text —
    /// rather than wrapping on to the next line.
    pub hard: bool,
}

impl Row {
    /// Where place `i` of the text stands on this line.
    pub fn x(&self, i: usize) -> f64 {
        let k = i.clamp(self.start, self.end) - self.start;
        self.stops[k]
    }
}

/// A text, set.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    pub rows: Vec<Row>,
    /// How many of the rows the box shows, from the first: all of them,
    /// but for a frame too short for what it holds.
    pub shown: usize,
    /// The box's size: a frame's own, and for artistic text what its
    /// lines measure.
    pub w: f64,
    pub h: f64,
    /// Baseline to baseline.
    pub line_h: f64,
    /// How far the face reaches above the baseline and below it.
    pub ascent: f64,
    pub descent: f64,
    /// The text, as characters.
    chars: Vec<char>,
}

impl Laid {
    /// A text as it stands on the board.
    pub fn of(t: &Text, fonts: &Fonts) -> Laid {
        lay(&t.text, &t.style, t.mode, t.w, t.h, fonts)
    }

    /// Whether a frame holds more than it shows: Affinity's red mark.
    pub fn overflows(&self) -> bool {
        self.shown < self.rows.len()
    }

    /// How many places there are to stand on: one past the last
    /// character.
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// Which row place `i` is on. Where a line wrapped, the place at the
    /// break is the next line's start — where the text it is in front of
    /// is.
    pub fn row_of(&self, i: usize) -> usize {
        self.rows.iter().rposition(|r| r.start <= i).unwrap_or(0)
    }

    /// The caret at place `i`: its x, and the top and bottom of the band
    /// it stands in.
    pub fn caret(&self, i: usize) -> (f64, f64, f64) {
        let row = &self.rows[self.row_of(i)];
        (row.x(i), row.top, row.top + self.line_h)
    }

    /// The furthest a caret goes along row `k`: its end — or, where the
    /// line wrapped, the place before its last character, since past it
    /// is the next line's start: before the space that hangs past the
    /// edge, or before the last letter of a word broken across it.
    pub fn last(&self, k: usize) -> usize {
        let row = &self.rows[k];
        let wraps = !row.hard && row.end > row.start;
        if wraps { row.end - 1 } else { row.end }
    }

    /// The place on row `k` nearest `x` across the box.
    pub fn index_on(&self, k: usize, x: f64) -> usize {
        let k = k.min(self.rows.len() - 1);
        let row = &self.rows[k];
        let last = self.last(k);
        (row.start..=last)
            .min_by(|&a, &b| {
                let (da, db) = ((row.x(a) - x).abs(), (row.x(b) - x).abs());
                da.total_cmp(&db)
            })
            .unwrap_or(row.start)
    }

    /// The place nearest the point `(x, y)` in the box. Above the first
    /// line is the first, below the last is the last.
    pub fn index_at(&self, x: f64, y: f64) -> usize {
        let top = self.rows[0].top;
        let k = ((y - top) / self.line_h).floor().max(0.0) as usize;
        self.index_on(k.min(self.rows.len() - 1), x)
    }

    /// The place a line up from `i`, as near `goal` across as that line
    /// has — or the very start, from the first line.
    pub fn up(&self, i: usize, goal: f64) -> usize {
        match self.row_of(i) {
            0 => 0,
            k => self.index_on(k - 1, goal),
        }
    }

    /// The place a line down, or the very end from the last.
    pub fn down(&self, i: usize, goal: f64) -> usize {
        let k = self.row_of(i);
        if k + 1 >= self.rows.len() {
            self.len()
        } else {
            self.index_on(k + 1, goal)
        }
    }

    /// The start of the line place `i` is on.
    pub fn home(&self, i: usize) -> usize {
        self.rows[self.row_of(i)].start
    }

    /// The end of the line place `i` is on.
    pub fn end(&self, i: usize) -> usize {
        self.last(self.row_of(i))
    }

    /// The bands a selection of places `a..b` paints: one per line it
    /// touches, `(x, top, w, h)`. A selection running on past a newline
    /// takes it along, and says so with a space's width of band.
    pub fn bands(&self, a: usize, b: usize, space: f64) -> Vec<(f64, f64, f64, f64)> {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = Vec::new();
        for row in &self.rows {
            if b < row.start || a > row.end || a == b {
                continue;
            }
            let (from, to) = (a.max(row.start), b.min(row.end));
            let newline = row.hard && b > row.end;
            let w = row.x(to) - row.x(from) + if newline { space } else { 0.0 };
            if w > 0.0 {
                out.push((row.x(from), row.top, w, self.line_h));
            }
        }
        out
    }

    /// The word place `i` is in, as `start..end` — or the run of spaces,
    /// when that is what it is in. What a double click selects.
    pub fn word_at(&self, i: usize) -> (usize, usize) {
        let n = self.chars.len();
        if n == 0 {
            return (0, 0);
        }
        // The character under a double click is the one after the place,
        // or the one before it at the very end of a line.
        let at = if i >= n || self.chars[i] == '\n' {
            i.saturating_sub(1)
        } else {
            i
        };
        if self.chars[at] == '\n' {
            return (i, i);
        }
        let class = |c: char| (c.is_whitespace(), c.is_alphanumeric() || c == '_');
        let kind = class(self.chars[at]);
        let same = |c: char| c != '\n' && class(c) == kind;
        let mut start = at;
        while start > 0 && same(self.chars[start - 1]) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < n && same(self.chars[end]) {
            end += 1;
        }
        (start, end)
    }

    /// The paragraph place `i` is in, newline excluded. What a triple
    /// click selects.
    pub fn paragraph_at(&self, i: usize) -> (usize, usize) {
        let i = i.min(self.chars.len());
        let start = self.chars[..i]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |p| p + 1);
        let end = self.chars[i..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.chars.len(), |p| i + p);
        (start, end)
    }

    /// Every character there is ink for on the rows shown, where its pen
    /// starts and the baseline it sits on. A space is not drawn.
    pub fn glyphs(&self) -> impl Iterator<Item = (char, f64, f64)> + '_ {
        self.rows[..self.shown].iter().flat_map(move |row| {
            (row.start..row.end).filter_map(move |i| {
                let c = self.chars[i];
                (!c.is_whitespace()).then(|| (c, row.x(i), row.baseline))
            })
        })
    }
}

/// Sets `text` in `style`. A frame wraps at `w` and shows what fits in
/// `h`; artistic text breaks only at a newline, and its box is what its
/// lines measure.
pub fn lay(text: &str, style: &TextStyle, mode: TextMode, w: f64, h: f64, fonts: &Fonts) -> Laid {
    let face = fonts.face(&style.font, style.bold, style.italic);
    let chars: Vec<char> = text.chars().collect();
    let size = style.size;
    let track = style.tracking / 1000.0 * size;
    let wraps = mode == TextMode::Frame;
    let limit = if wraps { w.max(0.0) } else { f64::INFINITY };

    // Each character's own advance, and how far the pen moves past it on
    // its way to the next: the advance, the kerning to the character
    // after it in the same paragraph, and the tracking.
    let advance: Vec<f64> = chars
        .iter()
        .map(|&c| if c == '\n' { 0.0 } else { f64::from(face.advance(c, size as f32)) })
        .collect();
    let step: Vec<f64> = (0..chars.len())
        .map(|i| {
            let kern = match chars.get(i + 1) {
                Some(&next) if chars[i] != '\n' && next != '\n' => {
                    f64::from(face.kern(chars[i], next, size as f32))
                }
                _ => 0.0,
            };
            advance[i] + kern + track
        })
        .collect();

    // Where the lines break: at every newline, and in a frame wherever
    // the next character would run past the edge — after the last space
    // that fits, or between two characters of a word wider than the box.
    let mut breaks: Vec<(usize, usize, bool)> = Vec::new();
    let mut start = 0;
    for (p_start, p_end) in paragraphs(&chars) {
        start = p_start;
        let (mut pen, mut after_space): (f64, Option<usize>) = (0.0, None);
        let mut i = p_start;
        while i < p_end {
            let c = chars[i];
            if wraps && !c.is_whitespace() && i > start && pen + advance[i] > limit + 1e-9 {
                let at = after_space.filter(|&b| b > start).unwrap_or(i);
                breaks.push((start, at, false));
                pen = step[at..i].iter().sum();
                start = at;
                after_space = None;
                continue;
            }
            pen += step[i];
            if c.is_whitespace() {
                after_space = Some(i + 1);
            }
            i += 1;
        }
        breaks.push((start, p_end, true));
    }
    if breaks.is_empty() {
        breaks.push((start, chars.len(), true));
    }

    // How wide each line's ink is: to the end of its last character that
    // is not a space, with no tracking or kerning trailing after it.
    let pens = |a: usize, b: usize| -> Vec<f64> {
        let mut xs = Vec::with_capacity(b - a + 1);
        let mut x = 0.0;
        xs.push(x);
        for s in &step[a..b] {
            x += s;
            xs.push(x);
        }
        xs
    };
    let ink = |a: usize, b: usize, xs: &[f64]| -> f64 {
        (a..b)
            .rev()
            .find(|&i| !chars[i].is_whitespace())
            .map_or(0.0, |i| xs[i - a] + advance[i])
    };
    let inks: Vec<f64> = breaks
        .iter()
        .map(|&(a, b, _)| ink(a, b, &pens(a, b)))
        .collect();
    let box_w = if wraps {
        w.max(0.0)
    } else {
        inks.iter().copied().fold(0.0, f64::max)
    };

    let (up, down) = face.extents(size as f32);
    let (ascent, descent) = (f64::from(up), f64::from(down));
    let line_h = size * style.leading;
    let content_h = line_h * breaks.len() as f64;
    let box_h = if wraps { h.max(0.0) } else { content_h };
    let shown = if wraps {
        let fit = ((box_h + 1e-9) / line_h).floor() as usize;
        fit.min(breaks.len())
    } else {
        breaks.len()
    };
    let top = match (wraps, style.valign) {
        (true, _) if content_h > box_h => 0.0,
        (true, Valign::Middle) => (box_h - content_h) / 2.0,
        (true, Valign::Bottom) => box_h - content_h,
        _ => 0.0,
    };
    let lift = (line_h - (ascent + descent)) / 2.0 + ascent;

    let rows = breaks
        .iter()
        .zip(&inks)
        .enumerate()
        .map(|(k, (&(a, b, hard), &width))| {
            let mut xs = pens(a, b);
            // A justified line that wrapped runs the full width: what it
            // lacks is shared out among the spaces inside its ink.
            if style.align == Align::Justify && !hard {
                let last_ink = (a..b).rev().find(|&i| !chars[i].is_whitespace());
                let gaps: Vec<usize> = match last_ink {
                    Some(end) => (a..end).filter(|&i| chars[i].is_whitespace()).collect(),
                    None => Vec::new(),
                };
                if !gaps.is_empty() {
                    let extra = (box_w - width).max(0.0) / gaps.len() as f64;
                    let mut added = 0.0;
                    for (k, x) in xs.iter_mut().enumerate().skip(1) {
                        if gaps.contains(&(a + k - 1)) {
                            added += extra;
                        }
                        *x += added;
                    }
                }
            }
            let shift = match style.align {
                // A justified line either runs the full width already or
                // is the last of its paragraph, which stands at the left.
                Align::Left | Align::Justify => 0.0,
                Align::Center => (box_w - width) / 2.0,
                Align::Right => box_w - width,
            };
            let justified = style.align == Align::Justify && !hard;
            let width = if justified { box_w.max(width) } else { width };
            let row_top = top + k as f64 * line_h;
            Row {
                start: a,
                end: b,
                top: row_top,
                baseline: row_top + lift,
                stops: xs.iter().map(|x| x + shift).collect(),
                width,
                hard,
            }
        })
        .collect();

    Laid {
        rows,
        shown,
        w: box_w,
        h: box_h,
        line_h,
        ascent,
        descent,
        chars,
    }
}

/// The paragraphs of `chars`, each as `start..end` with its newline left
/// out: one more than there are newlines, so an empty text is one empty
/// paragraph and a text ending in a newline ends in an empty one.
fn paragraphs(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    out.push((start, chars.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::DEFAULT_FONT;
    use crate::fonts::Face;

    const SIZE: f64 = 20.0;

    fn style() -> TextStyle {
        TextStyle::with_size(SIZE, "#000000")
    }

    fn fonts() -> Fonts {
        Fonts::bundled()
    }

    fn face(f: &Fonts) -> std::rc::Rc<Face> {
        f.face(DEFAULT_FONT, false, false)
    }

    /// How wide `s` sets in the default style, plainly: its advances and
    /// the face's own kerning between them.
    fn measure(f: &Fonts, s: &str) -> f64 {
        let face = face(f);
        let chars: Vec<char> = s.chars().collect();
        let mut w = 0.0;
        for (k, &c) in chars.iter().enumerate() {
            w += f64::from(face.advance(c, SIZE as f32));
            if let Some(&next) = chars.get(k + 1) {
                w += f64::from(face.kern(c, next, SIZE as f32));
            }
        }
        w
    }

    fn artistic(s: &str, style: &TextStyle) -> Laid {
        lay(s, style, TextMode::Artistic, 0.0, 0.0, &fonts())
    }

    fn framed(s: &str, style: &TextStyle, w: f64, h: f64) -> Laid {
        lay(s, style, TextMode::Frame, w, h, &fonts())
    }

    fn texts(s: &str, laid: &Laid) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        laid.rows
            .iter()
            .map(|r| chars[r.start..r.end].iter().collect())
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn artistic_text_is_one_line_as_wide_as_what_is_on_it() {
        let f = fonts();
        let laid = artistic("Hello", &style());
        assert_eq!(laid.rows.len(), 1);
        assert!(close(laid.w, measure(&f, "Hello")), "{} {}", laid.w, measure(&f, "Hello"));
        assert!(close(laid.h, SIZE * 1.2));
        assert!(close(laid.line_h, SIZE * 1.2));
        assert_eq!(laid.rows[0].stops.len(), 6, "one place more than the letters");
        assert!(close(laid.rows[0].x(0), 0.0));
    }

    #[test]
    fn a_newline_breaks_the_line_and_stands_on_neither() {
        let s = "one\ntwo\n";
        let laid = artistic(s, &style());
        assert_eq!(texts(s, &laid), ["one", "two", ""]);
        assert!(laid.rows.iter().all(|r| r.hard));
        assert!(close(laid.h, 3.0 * SIZE * 1.2));
        assert!(close(laid.rows[1].top, SIZE * 1.2));
    }

    #[test]
    fn nothing_sets_to_one_empty_line() {
        let laid = artistic("", &style());
        assert_eq!(laid.rows.len(), 1);
        assert_eq!((laid.rows[0].start, laid.rows[0].end), (0, 0));
        assert!(close(laid.w, 0.0));
        assert!(close(laid.h, SIZE * 1.2), "an empty text is still a line tall");
        assert_eq!(laid.caret(0), (0.0, 0.0, SIZE * 1.2));
    }

    #[test]
    fn artistic_text_never_wraps_whatever_its_box_says() {
        let laid = lay("a long line of words", &style(), TextMode::Artistic, 5.0, 5.0, &fonts());
        assert_eq!(laid.rows.len(), 1);
        assert!(laid.w > 5.0, "its box is what its line measures");
    }

    #[test]
    fn a_frame_wraps_after_the_last_space_that_fits() {
        let f = fonts();
        let s = "one two three";
        let laid = framed(s, &style(), measure(&f, "one two th"), 500.0);
        assert_eq!(texts(s, &laid), ["one two ", "three"], "the space hangs");
        assert!(!laid.rows[0].hard && laid.rows[1].hard);
        assert!(close(laid.rows[0].width, measure(&f, "one two")), "ink stops before the space");
    }

    #[test]
    fn a_word_wider_than_the_frame_breaks_between_characters() {
        let f = fonts();
        let s = "aaaaaaaaaaaaaaaaaaaa";
        let w = measure(&f, "aaaaaaa");
        let laid = framed(s, &style(), w, 500.0);
        assert!(laid.rows.len() >= 3, "{:?}", texts(s, &laid));
        for r in &laid.rows {
            assert!(r.width <= w + 1e-9);
        }
        assert_eq!(texts(s, &laid).concat(), s);
    }

    #[test]
    fn a_frame_too_narrow_for_one_character_still_sets_one_a_line() {
        let s = "abc";
        let laid = framed(s, &style(), 0.0, 500.0);
        assert_eq!(texts(s, &laid), ["a", "b", "c"]);
    }

    #[test]
    fn alignment_moves_each_line_across_the_box() {
        let f = fonts();
        let s = "a\nwider line";
        let mut st = style();
        let wide = measure(&f, "wider line");
        let narrow = measure(&f, "a");
        st.align = Align::Center;
        let c = artistic(s, &st);
        assert!(close(c.w, wide));
        assert!(close(c.rows[0].x(0), (wide - narrow) / 2.0));
        assert!(close(c.rows[1].x(2), 0.0));
        st.align = Align::Right;
        let r = framed(s, &st, 300.0, 500.0);
        assert!(close(r.rows[0].x(0), 300.0 - narrow));
        assert!(close(r.rows[1].x(r.rows[1].end), 300.0));
    }

    #[test]
    fn justified_lines_run_the_full_width_but_the_last_of_a_paragraph() {
        let f = fonts();
        let s = "one two three four five six";
        let mut st = style();
        st.align = Align::Justify;
        let w = measure(&f, "one two three fo");
        let laid = framed(s, &st, w, 500.0);
        assert!(laid.rows.len() >= 2);
        let first = &laid.rows[0];
        // The last character before the hanging space ends at the edge.
        let before_space = laid.last(0);
        assert!(close(first.x(before_space), w), "{} {w}", first.x(before_space));
        let last = laid.rows.last().unwrap();
        assert!(close(last.x(last.start), 0.0));
        assert!(last.x(last.end) < w, "the last line keeps its own width");
    }

    #[test]
    fn leading_sets_the_lines_apart() {
        let mut st = style();
        st.leading = 2.0;
        let laid = artistic("a\nb", &st);
        assert!(close(laid.line_h, 40.0));
        assert!(close(laid.rows[1].baseline - laid.rows[0].baseline, 40.0));
        assert!(close(laid.h, 80.0));
        // The baseline sits in its band as CSS sets it: the leading split
        // evenly above the ascent and below the descent.
        let above = laid.rows[0].baseline - laid.ascent;
        let below = laid.line_h - (laid.rows[0].baseline + laid.descent);
        assert!(close(above, below), "{above} {below}");
    }

    #[test]
    fn tracking_adds_its_share_of_an_em_between_characters() {
        let mut st = style();
        let plain = artistic("abcd", &st);
        st.tracking = 100.0;
        let tracked = artistic("abcd", &st);
        // Three gaps between four characters, a tenth of the size each.
        assert!(close(tracked.w - plain.w, 3.0 * SIZE * 0.1), "{}", tracked.w - plain.w);
    }

    #[test]
    fn a_frame_shows_only_the_lines_that_fit_and_says_it_overflows() {
        let s = "1\n2\n3\n4";
        let laid = framed(s, &style(), 100.0, SIZE * 1.2 * 2.5);
        assert_eq!(laid.rows.len(), 4);
        assert_eq!(laid.shown, 2);
        assert!(laid.overflows());
        assert_eq!(laid.glyphs().count(), 2);
        let roomy = framed(s, &style(), 100.0, 500.0);
        assert!(!roomy.overflows());
    }

    #[test]
    fn a_frame_stands_its_lines_at_the_top_the_middle_or_the_bottom() {
        let mut st = style();
        let h = 100.0;
        let tall = SIZE * 1.2;
        assert!(close(framed("a", &st, 50.0, h).rows[0].top, 0.0));
        st.valign = Valign::Middle;
        assert!(close(framed("a", &st, 50.0, h).rows[0].top, (h - tall) / 2.0));
        st.valign = Valign::Bottom;
        assert!(close(framed("a", &st, 50.0, h).rows[0].top, h - tall));
        // What does not fit stands from the top, as Affinity does.
        let over = framed("1\n2\n3\n4\n5", &st, 50.0, h);
        assert!(close(over.rows[0].top, 0.0));
    }

    #[test]
    fn the_caret_stands_between_characters_and_at_a_break_on_the_next_line() {
        let f = fonts();
        let s = "one two three";
        let laid = framed(s, &style(), measure(&f, "one two th"), 500.0);
        let (x, top, bottom) = laid.caret(3);
        assert!(close(x, measure(&f, "one")));
        assert!(close(top, 0.0) && close(bottom, SIZE * 1.2));
        // Place 8 is where the first line wrapped: the caret is in front
        // of "three", on the second line.
        assert_eq!(laid.row_of(8), 1);
        assert!(close(laid.caret(8).0, 0.0));
        assert!(close(laid.caret(8).1, SIZE * 1.2));
    }

    #[test]
    fn the_place_under_a_point_is_the_nearest_gap() {
        let f = fonts();
        let s = "one\ntwo";
        let laid = artistic(s, &style());
        let one = measure(&f, "one");
        assert_eq!(laid.index_at(-10.0, 1.0), 0);
        assert_eq!(laid.index_at(measure(&f, "o") * 0.4, 1.0), 0);
        assert_eq!(laid.index_at(measure(&f, "o") * 0.6, 1.0), 1);
        assert_eq!(laid.index_at(one + 50.0, 1.0), 3, "past the end of a line is its end");
        assert_eq!(laid.index_at(0.0, SIZE * 1.2 + 1.0), 4, "the second line");
        assert_eq!(laid.index_at(500.0, 900.0), 7, "below everything is the last line");
        assert_eq!(laid.index_at(0.0, -40.0), 0, "above everything is the first");
    }

    #[test]
    fn up_and_down_keep_to_the_same_x() {
        let f = fonts();
        let s = "abcdef\nabcdef\nab";
        let laid = artistic(s, &style());
        let goal = measure(&f, "abc");
        assert_eq!(laid.down(3, goal), 10);
        assert_eq!(laid.up(10, goal), 3);
        assert_eq!(laid.down(10, goal), 16, "a shorter line takes its end");
        assert_eq!(laid.up(3, goal), 0, "up from the first line is the start");
        assert_eq!(laid.down(16, goal), 16, "down from the last is the end");
    }

    #[test]
    fn home_and_end_are_the_lines_as_they_are_set() {
        let f = fonts();
        let s = "one two three";
        let laid = framed(s, &style(), measure(&f, "one two th"), 500.0);
        assert_eq!(laid.home(5), 0);
        assert_eq!(laid.end(5), 7, "before the space that hangs");
        assert_eq!(laid.home(10), 8);
        assert_eq!(laid.end(10), 13);
    }

    #[test]
    fn a_selection_paints_one_band_a_line() {
        let f = fonts();
        let s = "one\ntwo";
        let laid = artistic(s, &style());
        let bands = laid.bands(1, 6, 5.0);
        assert_eq!(bands.len(), 2);
        let (x, top, w, h) = bands[0];
        assert!(close(x, measure(&f, "o")));
        assert!(close(top, 0.0) && close(h, SIZE * 1.2));
        assert!(close(w, measure(&f, "one") - measure(&f, "o") + 5.0), "the newline goes along");
        assert!(close(bands[1].0, 0.0) && close(bands[1].2, laid.rows[1].x(6)));
        assert!(laid.bands(3, 3, 5.0).is_empty(), "nothing selected paints nothing");
    }

    #[test]
    fn a_double_click_takes_a_word_and_a_triple_a_paragraph() {
        let s = "hello, big world\nnext";
        let laid = artistic(s, &style());
        assert_eq!(laid.word_at(1), (0, 5));
        assert_eq!(laid.word_at(5), (5, 6), "punctuation is a run of its own");
        assert_eq!(laid.word_at(8), (7, 10));
        assert_eq!(laid.word_at(16), (11, 16), "the end of a line takes the word before it");
        assert_eq!(laid.paragraph_at(8), (0, 16));
        assert_eq!(laid.paragraph_at(19), (17, 21));
    }

    #[test]
    fn only_ink_is_drawn() {
        let laid = artistic("a b\nc", &style());
        let drawn: String = laid.glyphs().map(|(c, _, _)| c).collect();
        assert_eq!(drawn, "abc");
    }

    #[test]
    fn the_caret_crosses_a_line_broken_mid_word() {
        let f = fonts();
        let s = "mmmmmmmmmmmm";
        let laid = framed(s, &style(), measure(&f, "mmmm"), 500.0);
        assert_eq!(texts(s, &laid), ["mmmm", "mmmm", "mmmm"]);
        let goal = laid.caret(12).0;
        let up = laid.up(12, goal);
        assert_eq!(laid.row_of(up), 1, "up from the last row lands on the one above");
        let up = laid.up(up, goal);
        assert_eq!(laid.row_of(up), 0);
        assert_eq!(laid.row_of(laid.end(1)), 0, "End stays on the row it was asked on");
        let edge = laid.index_at(laid.rows[0].stops[4] + 1.0, laid.rows[0].top + 1.0);
        assert_eq!(laid.row_of(edge), 0, "a click at a row's right edge stays on it");
    }
}
