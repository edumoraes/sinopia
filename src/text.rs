//! Text for the chrome: one glyph atlas and the layout that places it.
//!
//! The atlas is an ordinary RGBA8 bitmap — white everywhere, alpha the
//! glyph's coverage — so it rides the texture path the images already
//! use, and the shader's `texel * color` tints a glyph for free. What it
//! adds is a sub-rectangle per glyph, which is why [`Prim`] carries `uv`.
//!
//! The face ships inside the binary: a whiteboard that cannot draw its
//! own tab labels because a machine is missing a font is not local-first.
//!
//! [`Prim`]: crate::scene::Prim

use std::collections::HashMap;

use crate::bitmap::Bitmap;
use crate::scene::ScreenRect;

/// Liberation Sans (SIL OFL 1.1) — see `assets/fonts/`.
const FACE: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");

/// Atlas width in texels. Rows fill left to right and wrap.
const ATLAS_W: u32 = 512;
/// Transparent gutter around every cell: the sampler is linear, so a
/// glyph pressed against its neighbour would smear into it.
const PAD: u32 = 1;

/// What the atlas holds: printable ASCII plus the Latin-1 supplement, so
/// an acentuação in a file name survives, and the ellipsis truncation
/// appends.
fn charset() -> impl Iterator<Item = char> {
    (0x20u32..=0x7E)
        .chain(0xA0..=0xFF)
        .filter_map(char::from_u32)
        .chain(std::iter::once('…'))
}

/// Stands in for anything the atlas does not hold.
const MISSING: char = '·';

/// The bundled face, parsed once.
pub struct Font(fontdue::Font);

impl Font {
    /// The bytes are compiled in and known good; a failure here means the
    /// binary itself is broken, not the machine it runs on.
    pub fn bundled() -> Font {
        let settings = fontdue::FontSettings::default();
        Font(fontdue::Font::from_bytes(FACE, settings).expect("the bundled face parses"))
    }
}

/// Where one glyph sits in the atlas, and where it sits relative to the
/// pen that draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    uv: [f32; 4],
    /// Offset from the pen to the quad's top-left corner, in px.
    dx: f32,
    dy: f32,
    w: f32,
    h: f32,
    advance: f32,
}

/// One glyph, placed: the box to draw and the slice of atlas to draw in
/// it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub rect: ScreenRect,
    pub uv: [f32; 4],
}

/// Every glyph of [`charset`] rasterized at one size. Rebuilt when the
/// size changes, which happens when the window moves to a display with a
/// different scale factor — not per frame.
pub struct Atlas {
    pub bitmap: Bitmap,
    cells: HashMap<char, Cell>,
    px: u32,
    ascent: f32,
    descent: f32,
}

impl Atlas {
    pub fn build(font: &Font, px: u32) -> Atlas {
        let px = px.max(1);
        let size = px as f32;

        // Rasterize first, then place: the atlas height is not knowable
        // before every glyph's box is.
        let rendered: Vec<(char, fontdue::Metrics, Vec<u8>)> = charset()
            .map(|ch| {
                let (metrics, coverage) = font.0.rasterize(ch, size);
                (ch, metrics, coverage)
            })
            .collect();

        let (places, height) = pack(&rendered);
        let mut bitmap = Bitmap {
            w: ATLAS_W,
            h: height,
            rgba: vec![0; (ATLAS_W * height * 4) as usize],
        };

        let mut cells = HashMap::with_capacity(rendered.len());
        for ((ch, metrics, coverage), (ox, oy)) in rendered.iter().zip(places) {
            blit(&mut bitmap, coverage, metrics.width as u32, ox, oy);
            let (w, h) = (metrics.width as f32, metrics.height as f32);
            cells.insert(
                *ch,
                Cell {
                    uv: [
                        ox as f32 / ATLAS_W as f32,
                        oy as f32 / height as f32,
                        (ox as f32 + w) / ATLAS_W as f32,
                        (oy as f32 + h) / height as f32,
                    ],
                    dx: metrics.xmin as f32,
                    // fontdue measures y up from the baseline; the screen
                    // measures it down from the top.
                    dy: -(metrics.ymin as f32 + h),
                    w,
                    h,
                    advance: metrics.advance_width,
                },
            );
        }

        // A face with no horizontal line metrics would not be a text face;
        // fall back on the em box rather than refusing to draw.
        let line = font.0.horizontal_line_metrics(size);
        Atlas {
            bitmap,
            cells,
            px,
            ascent: line.map_or(size * 0.8, |m| m.ascent),
            descent: line.map_or(size * -0.2, |m| m.descent),
        }
    }

    /// The size this atlas was built at. `app` compares it against the
    /// size the current scale factor asks for.
    pub fn px(&self) -> u32 {
        self.px
    }

    /// Where the baseline goes for `s` to sit centered in `r`.
    pub fn baseline_in(&self, r: ScreenRect) -> f32 {
        (r.y + (r.h - (self.ascent - self.descent)) / 2.0 + self.ascent).round()
    }

    /// Advance width of `s` in px. Zero for the empty string.
    pub fn measure(&self, s: &str) -> f32 {
        s.chars().map(|ch| self.cell(ch).advance).sum()
    }

    /// Places `s` with its pen starting at `x` on `baseline`. Glyphs with
    /// nothing to paint — the space, above all — take their advance and
    /// leave no quad behind.
    pub fn layout(&self, s: &str, x: f32, baseline: f32) -> Vec<Glyph> {
        let mut pen = x;
        let mut out = Vec::new();
        for ch in s.chars() {
            let cell = self.cell(ch);
            if cell.w > 0.0 && cell.h > 0.0 {
                out.push(Glyph {
                    rect: ScreenRect {
                        x: pen + cell.dx,
                        y: baseline + cell.dy,
                        w: cell.w,
                        h: cell.h,
                    },
                    uv: cell.uv,
                });
            }
            pen += cell.advance;
        }
        out
    }

    /// `s` if it fits in `max_w`, else the longest prefix that fits with
    /// an ellipsis after it. Empty when not even the ellipsis fits.
    pub fn truncate(&self, s: &str, max_w: f32) -> String {
        if self.measure(s) <= max_w {
            return s.to_owned();
        }
        let dots = self.cell('…').advance;
        if dots > max_w {
            return String::new();
        }
        let mut kept = String::new();
        let mut w = dots;
        for ch in s.chars() {
            let advance = self.cell(ch).advance;
            if w + advance > max_w {
                break;
            }
            w += advance;
            kept.push(ch);
        }
        kept.push('…');
        kept
    }

    /// The cell for `ch`, or the one standing in for it. Every atlas
    /// holds [`MISSING`], so this never fails.
    fn cell(&self, ch: char) -> &Cell {
        self.cells
            .get(&ch)
            .or_else(|| self.cells.get(&MISSING))
            .expect("the atlas holds its own replacement glyph")
    }
}

/// Lays the rasterized glyphs out in rows, left to right, wrapping at
/// [`ATLAS_W`]. Answers where each one goes and how tall the sheet ends
/// up.
fn pack(rendered: &[(char, fontdue::Metrics, Vec<u8>)]) -> (Vec<(u32, u32)>, u32) {
    let (mut x, mut y, mut row_h) = (PAD, PAD, 0u32);
    let mut places = Vec::with_capacity(rendered.len());
    for (_, metrics, _) in rendered {
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        if x + w + PAD > ATLAS_W {
            x = PAD;
            y += row_h + PAD;
            row_h = 0;
        }
        places.push((x, y));
        x += w + PAD;
        row_h = row_h.max(h);
    }
    (places, y + row_h + PAD)
}

/// Writes one glyph's coverage into the sheet as white texels whose
/// alpha is that coverage.
fn blit(sheet: &mut Bitmap, coverage: &[u8], w: u32, ox: u32, oy: u32) {
    if w == 0 {
        return;
    }
    for (i, &a) in coverage.iter().enumerate() {
        let (gx, gy) = (i as u32 % w, i as u32 / w);
        let at = (((oy + gy) * sheet.w + ox + gx) * 4) as usize;
        sheet.rgba[at..at + 4].copy_from_slice(&[255, 255, 255, a]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), 16)
    }

    #[test]
    fn the_empty_string_measures_nothing() {
        assert_eq!(atlas().measure(""), 0.0);
    }

    #[test]
    fn measuring_grows_with_the_string() {
        let a = atlas();
        assert!(a.measure("board") > a.measure("boar"));
        assert!(a.measure("boar") > a.measure("boa"));
    }

    #[test]
    fn a_space_advances_without_painting() {
        let a = atlas();
        assert!(a.measure(" ") > 0.0);
        assert!(a.layout(" ", 0.0, 0.0).is_empty());
    }

    #[test]
    fn layout_advances_by_what_it_measures() {
        let a = atlas();
        let glyphs = a.layout("ab", 100.0, 0.0);
        assert_eq!(glyphs.len(), 2);
        // The second glyph starts one 'a' further along.
        let advance = a.measure("a");
        assert!((glyphs[1].rect.x - glyphs[0].rect.x - advance).abs() < 1.5);
    }

    #[test]
    fn glyphs_sit_above_the_baseline() {
        let a = atlas();
        // 'x' has no descender: its box is entirely above the baseline.
        let g = a.layout("x", 0.0, 40.0)[0];
        assert!(g.rect.y < 40.0);
        assert!(g.rect.y + g.rect.h <= 40.5);
    }

    #[test]
    fn every_uv_lands_inside_the_sheet() {
        let a = atlas();
        for g in a.layout("Quadro — ação 123", 0.0, 20.0) {
            let [u0, v0, u1, v1] = g.uv;
            assert!((0.0..=1.0).contains(&u0) && (0.0..=1.0).contains(&u1));
            assert!((0.0..=1.0).contains(&v0) && (0.0..=1.0).contains(&v1));
            assert!(u1 > u0 && v1 > v0);
        }
    }

    #[test]
    fn accented_names_keep_their_accents() {
        let a = atlas();
        // 'ç' is its own glyph, not the replacement.
        assert_ne!(a.measure("ç"), a.measure(MISSING.to_string().as_str()));
    }

    #[test]
    fn a_glyph_the_atlas_lacks_falls_back() {
        let a = atlas();
        assert_eq!(a.measure("漢"), a.measure(MISSING.to_string().as_str()));
        assert_eq!(a.layout("漢", 0.0, 20.0).len(), 1);
    }

    #[test]
    fn truncating_returns_what_already_fits() {
        let a = atlas();
        let wide = a.measure("notes.omawhite") + 10.0;
        assert_eq!(a.truncate("notes.omawhite", wide), "notes.omawhite");
    }

    #[test]
    fn a_truncated_label_fits_its_budget() {
        let a = atlas();
        let budget = a.measure("notes.omawhite") / 2.0;
        let cut = a.truncate("notes.omawhite", budget);
        assert!(cut.ends_with('…'), "{cut:?}");
        assert!(cut.chars().count() < "notes.omawhite".chars().count());
        assert!(a.measure(&cut) <= budget);
    }

    #[test]
    fn a_budget_too_small_for_the_ellipsis_leaves_nothing() {
        let a = atlas();
        assert_eq!(a.truncate("notes", 0.5), "");
    }

    #[test]
    fn truncation_cuts_on_character_boundaries() {
        let a = atlas();
        let cut = a.truncate("ação — ç", a.measure("ação") * 0.6);
        // Reaching this means every byte lands where a char starts.
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_sheet_is_white_where_it_is_opaque() {
        let a = atlas();
        assert_eq!(a.bitmap.rgba.len(), (a.bitmap.w * a.bitmap.h * 4) as usize);
        let painted = a.bitmap.rgba.as_chunks::<4>().0.iter().find(|t| t[3] > 0);
        assert_eq!(painted.map(|t| &t[..3]), Some(&[255u8, 255, 255][..]));
    }

    #[test]
    fn a_bigger_size_makes_wider_text() {
        let font = Font::bundled();
        let small = Atlas::build(&font, 12);
        let big = Atlas::build(&font, 24);
        assert_eq!(small.px(), 12);
        assert!(big.measure("board") > small.measure("board"));
    }

    #[test]
    fn a_baseline_centers_the_line_in_its_box() {
        let a = atlas();
        let r = ScreenRect {
            x: 0.0,
            y: 100.0,
            w: 80.0,
            h: 30.0,
        };
        let baseline = a.baseline_in(r);
        assert!(baseline > r.y && baseline < r.y + r.h);
        // The gap above the ascent matches the one below the descent.
        let above = baseline - a.ascent - r.y;
        let below = r.y + r.h - (baseline - a.descent);
        assert!((above - below).abs() <= 1.0, "{above} vs {below}");
    }
}
