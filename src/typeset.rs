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

use crate::doc::{Align, Run, Text, TextMode, TextStyle, Valign};
use crate::fonts::{Face, Fonts};

/// One line as it is set: characters `start..end` of the text — a line a
/// newline ended does not hold the newline — and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    /// The top of the line's own band, and how tall it is: `leading`
    /// times the size of its tallest letters, the letters standing on one
    /// baseline.
    pub top: f64,
    pub height: f64,
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
    /// Baseline to baseline in the text's own style — what a line with
    /// nothing set apart on it is.
    pub line_h: f64,
    /// How far the text's own face reaches above the baseline and below.
    pub ascent: f64,
    pub descent: f64,
    /// How tall all the lines are together.
    pub content: f64,
    /// The text, as characters.
    chars: Vec<char>,
    /// Each character's style, as an index into `styles`, and its own
    /// advance.
    which: Vec<usize>,
    styles: Vec<TextStyle>,
    advance: Vec<f64>,
    /// How far each style reaches above its baseline and below, the
    /// leading shared out on both sides.
    reach: Vec<(f64, f64)>,
}

/// One letter to draw: what it is, where its pen starts, the baseline it
/// sits on, and how it is set.
#[derive(Debug, Clone, Copy)]
pub struct Glyph<'a> {
    pub ch: char,
    pub x: f64,
    pub baseline: f64,
    pub style: &'a TextStyle,
}

/// A rule under a stretch of letters, or through it: from `x0` to `x1`
/// at `y` (the baseline, which the renderer offsets by the size), in the
/// stretch's size and ink.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub x0: f64,
    pub x1: f64,
    pub baseline: f64,
    pub size: f64,
    pub color: String,
    pub strike: bool,
}

impl Laid {
    /// A text as it stands on the board.
    pub fn of(t: &Text, fonts: &Fonts) -> Laid {
        lay_with(&t.text, &t.style, &t.runs, t.mode, t.w, t.h, fonts)
    }

    /// How character `i` is set.
    pub fn style_of(&self, i: usize) -> &TextStyle {
        &self.styles[self.which.get(i).copied().unwrap_or(0)]
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
    ///
    /// The caret is as tall as the letter it follows — the one it comes
    /// before at a line's start, the line's own style on an empty line —
    /// standing on the line's baseline.
    pub fn caret(&self, i: usize) -> (f64, f64, f64) {
        let row = &self.rows[self.row_of(i)];
        let of = if i > row.start {
            Some(i - 1)
        } else if i < row.end {
            Some(i)
        } else {
            None
        };
        let (up, down) = match of {
            Some(c) => self.reach[self.which[c]],
            None => (row.baseline - row.top, row.top + row.height - row.baseline),
        };
        (row.x(i), row.baseline - up, row.baseline + down)
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
        let k = self
            .rows
            .iter()
            .position(|r| y < r.top + r.height)
            .unwrap_or(self.rows.len() - 1);
        self.index_on(k, x)
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
                out.push((row.x(from), row.top, w, row.height));
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
    /// starts, the baseline it sits on and how it is set. A space is not
    /// drawn.
    pub fn glyphs(&self) -> impl Iterator<Item = Glyph<'_>> + '_ {
        self.rows[..self.shown].iter().flat_map(move |row| {
            (row.start..row.end).filter_map(move |i| {
                let c = self.chars[i];
                (!c.is_whitespace()).then(|| Glyph {
                    ch: c,
                    x: row.x(i),
                    baseline: row.baseline,
                    style: self.style_of(i),
                })
            })
        })
    }

    /// The rules the rows shown carry: one under — or through — each
    /// stretch of letters set alike with underline — or strikethrough —
    /// on, as far as the line's ink goes and no further.
    pub fn rules(&self) -> Vec<Rule> {
        let mut out = Vec::new();
        for row in &self.rows[..self.shown] {
            let ink_end = (row.start..row.end)
                .rev()
                .find(|&i| !self.chars[i].is_whitespace())
                .map_or(row.start, |i| i + 1);
            for strike in [false, true] {
                // Stretches of letters set alike with the rule on, as
                // `(from, to, style)`.
                let mut stretches: Vec<(usize, usize, usize)> = Vec::new();
                for i in row.start..ink_end {
                    let st = self.which[i];
                    let s = &self.styles[st];
                    if !(if strike { s.strike } else { s.underline }) {
                        continue;
                    }
                    match stretches.last_mut() {
                        Some((_, to, style)) if *to == i && *style == st => *to = i + 1,
                        _ => stretches.push((i, i + 1, st)),
                    }
                }
                for (from, to, style) in stretches {
                    let s = &self.styles[style];
                    out.push(Rule {
                        x0: row.x(from),
                        x1: row.x(to - 1) + self.advance[to - 1],
                        baseline: row.baseline,
                        size: s.size,
                        color: s.color.clone(),
                        strike,
                    });
                }
            }
        }
        out
    }
}

/// Sets `text` in `style`. A frame wraps at `w` and shows what fits in
/// `h`; artistic text breaks only at a newline, and its box is what its
/// lines measure.
pub fn lay(text: &str, style: &TextStyle, mode: TextMode, w: f64, h: f64, fonts: &Fonts) -> Laid {
    lay_with(text, style, &[], mode, w, h, fonts)
}

/// [`lay`], with stretches of the text set apart by `runs`: every letter
/// in its own face and size, a line as tall as its tallest letters and
/// all of them on one baseline.
pub fn lay_with(
    text: &str,
    style: &TextStyle,
    runs: &[Run],
    mode: TextMode,
    w: f64,
    h: f64,
    fonts: &Fonts,
) -> Laid {
    let chars: Vec<char> = text.chars().collect();
    let wraps = mode == TextMode::Frame;
    let limit = if wraps { w.max(0.0) } else { f64::INFINITY };

    // Every style the text is set in, the text's own first, and which of
    // them each character wears.
    let mut styles: Vec<TextStyle> = vec![style.clone()];
    let which: Vec<usize> = crate::spans::expand(runs, chars.len())
        .iter()
        .map(|r| {
            let s = r.over(style);
            match styles.iter().position(|known| *known == s) {
                Some(k) => k,
                None => {
                    styles.push(s);
                    styles.len() - 1
                }
            }
        })
        .collect();
    let faces: Vec<std::rc::Rc<Face>> = styles
        .iter()
        .map(|s| fonts.face(&s.font, s.bold, s.italic))
        .collect();
    // How far each style reaches above its baseline and below it, the
    // leading split evenly on both sides as CSS splits it.
    let reach: Vec<(f64, f64)> = styles
        .iter()
        .zip(&faces)
        .map(|(s, face)| {
            let (up, down) = face.extents(s.size as f32);
            let (up, down) = (f64::from(up), f64::from(down));
            let half = (s.size * style.leading - (up + down)) / 2.0;
            (up + half, down + half)
        })
        .collect();

    // Each character's own advance, and how far the pen moves past it on
    // its way to the next: the advance, the kerning to the character
    // after it — in the same paragraph, face and size — and the tracking.
    let advance: Vec<f64> = chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if c == '\n' {
                return 0.0;
            }
            let k = which[i];
            f64::from(faces[k].advance(c, styles[k].size as f32))
        })
        .collect();
    let step: Vec<f64> = (0..chars.len())
        .map(|i| {
            let k = which[i];
            let size = styles[k].size;
            let kern = match chars.get(i + 1) {
                Some(&next) if chars[i] != '\n' && next != '\n' && which[i + 1] == k => {
                    f64::from(faces[k].kern(chars[i], next, size as f32))
                }
                _ => 0.0,
            };
            advance[i] + kern + styles[k].tracking / 1000.0 * size
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

    // A line reaches as far above and below its baseline as its furthest
    // reaching letter; an empty one as far as the letter before it — the
    // newline that ended the line above — or the text's own style.
    let reaches: Vec<(f64, f64)> = breaks
        .iter()
        .map(|&(a, b, _)| {
            let own: Vec<(f64, f64)> = (a..b).filter(|&i| chars[i] != '\n').map(|i| reach[which[i]]).collect();
            if own.is_empty() {
                let before = a.checked_sub(1).map_or(0, |i| which[i]);
                return reach[before];
            }
            own.iter().fold((0.0, 0.0), |(u, d), &(uu, dd)| (f64::max(u, uu), f64::max(d, dd)))
        })
        .collect();
    let content_h: f64 = reaches.iter().map(|(u, d)| u + d).sum();
    let box_h = if wraps { h.max(0.0) } else { content_h };
    let shown = if wraps {
        let mut bottom = 0.0;
        reaches
            .iter()
            .take_while(|(u, d)| {
                bottom += u + d;
                bottom <= box_h + 1e-9
            })
            .count()
    } else {
        breaks.len()
    };
    let top = match (wraps, style.valign) {
        (true, _) if content_h > box_h => 0.0,
        (true, Valign::Middle) => (box_h - content_h) / 2.0,
        (true, Valign::Bottom) => box_h - content_h,
        _ => 0.0,
    };

    let mut row_top = top;
    let rows = breaks
        .iter()
        .zip(&inks)
        .zip(&reaches)
        .map(|((&(a, b, hard), &width), &(up, down))| {
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
            let row = Row {
                start: a,
                end: b,
                top: row_top,
                height: up + down,
                baseline: row_top + up,
                stops: xs.iter().map(|x| x + shift).collect(),
                width,
                hard,
            };
            row_top += up + down;
            row
        })
        .collect();

    let (up, down) = faces[0].extents(style.size as f32);
    Laid {
        rows,
        shown,
        w: box_w,
        h: box_h,
        line_h: style.size * style.leading,
        ascent: f64::from(up),
        descent: f64::from(down),
        content: content_h,
        chars,
        which,
        styles,
        advance,
        reach,
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
        let drawn: String = laid.glyphs().map(|g| g.ch).collect();
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

    fn run(start: usize, end: usize, style: crate::doc::RunStyle) -> crate::doc::Run {
        crate::doc::Run { start, end, style }
    }

    fn big() -> crate::doc::RunStyle {
        crate::doc::RunStyle {
            size: Some(SIZE * 2.0),
            ..Default::default()
        }
    }

    fn lay_runs(s: &str, runs: &[crate::doc::Run], mode: TextMode, w: f64) -> Laid {
        lay_with(s, &style(), runs, mode, w, 500.0, &fonts())
    }

    #[test]
    fn a_bigger_stretch_makes_its_line_taller_and_shares_its_baseline() {
        let plain = artistic("ab\ncd", &style());
        let laid = lay_runs("ab\ncd", &[run(1, 2, big())], TextMode::Artistic, 0.0);
        let (first, second) = (&laid.rows[0], &laid.rows[1]);
        assert!(close(first.height, 2.0 * plain.rows[0].height), "{} {}", first.height, plain.rows[0].height);
        assert!(close(second.height, plain.rows[1].height), "the plain line is as tall as it was");
        assert!(close(second.top, first.height));
        assert!(close(laid.h, first.height + second.height));
        assert!(first.baseline - first.top > plain.rows[0].baseline - plain.rows[0].top);
        let glyphs: Vec<(char, f64)> = laid.glyphs().map(|g| (g.ch, g.baseline)).collect();
        assert_eq!(glyphs[0].1, glyphs[1].1, "letters of one line stand on one baseline");
    }

    #[test]
    fn a_letter_advances_by_its_own_face_and_size() {
        let f = fonts();
        let plain = artistic("mm", &style());
        let bold = crate::doc::RunStyle {
            bold: Some(true),
            ..Default::default()
        };
        let laid = lay_runs("mm", &[run(0, 1, bold)], TextMode::Artistic, 0.0);
        let bold_m = f64::from(f.face(crate::doc::DEFAULT_FONT, true, false).advance('m', SIZE as f32));
        assert!(close(laid.rows[0].x(1), bold_m), "{} {bold_m}", laid.rows[0].x(1));
        assert!(laid.w > plain.w);
        let styles: Vec<bool> = laid.glyphs().map(|g| g.style.bold).collect();
        assert_eq!(styles, [true, false]);
    }

    #[test]
    fn the_caret_is_as_tall_as_the_letter_it_follows() {
        let laid = lay_runs("aBc", &[run(1, 2, big())], TextMode::Artistic, 0.0);
        let (_, t1, b1) = laid.caret(1);
        let (_, t2, b2) = laid.caret(2);
        assert!(b2 - t2 > 1.5 * (b1 - t1), "after the big letter the caret is big");
        let baseline = laid.rows[0].baseline;
        assert!(t2 < baseline && b2 > baseline && t1 < baseline && b1 > baseline);
    }

    #[test]
    fn a_point_finds_its_line_whatever_the_lines_heights() {
        let laid = lay_runs("aa\nbb\ncc", &[run(3, 5, big())], TextMode::Artistic, 0.0);
        let mid = |k: usize| laid.rows[k].top + laid.rows[k].height / 2.0;
        assert_eq!(laid.row_of(laid.index_at(0.0, mid(0))), 0);
        assert_eq!(laid.row_of(laid.index_at(0.0, mid(1))), 1);
        assert_eq!(laid.row_of(laid.index_at(0.0, mid(2))), 2);
        assert_eq!(laid.row_of(laid.index_at(0.0, laid.h + 50.0)), 2);
        let bands = laid.bands(0, 8, 1.0);
        assert!(close(bands[1].3, laid.rows[1].height), "a band is as tall as its line");
    }

    #[test]
    fn only_the_stretch_underlined_carries_a_rule() {
        let under = crate::doc::RunStyle {
            underline: Some(true),
            ..Default::default()
        };
        let laid = lay_runs("one two three", &[run(4, 7, under)], TextMode::Artistic, 0.0);
        let rules = laid.rules();
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert!(close(r.x0, laid.rows[0].x(4)));
        assert!(r.x1 > r.x0 && r.x1 <= laid.rows[0].x(7) + 1e-9);
        assert!(!r.strike);
        let everything = artistic("a b", &TextStyle {
            strike: true,
            ..style()
        });
        assert_eq!(everything.rules().len(), 1, "a whole line struck is one rule");
    }

    #[test]
    fn a_frame_shows_the_lines_its_height_holds_whatever_their_heights() {
        let laid = lay_with("a\nb\nc", &style(), &[run(0, 1, big())], TextMode::Frame, 200.0, SIZE * 1.2 * 2.5, &fonts());
        assert_eq!(laid.shown, 1, "the big first line and one more do not fit");
        assert!(laid.overflows());
    }
}
