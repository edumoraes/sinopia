//! The board's glyph sheet: every letter a text on the board is drawn
//! with, rasterized at the size it is seen at.
//!
//! The chrome's atlas (`text`) is one face at one size, built whole. A
//! text on the board is seen at any zoom, so its letters are rasterized
//! as they are asked for — at a rung of [`ladder`] near the size on
//! screen, so a zoom does not rasterize every glyph again at every
//! fraction of a pixel — and packed into one sheet, which the renderer
//! uploads a region of at a time. When the sheet is full it is started
//! over: the frame that found it full goes without the glyphs that did
//! not fit, and the next one rasterizes what it needs into a clean sheet.
//!
//! The sheet is white with the coverage in its alpha, exactly as the
//! chrome's atlas is, so `texel * ink` is the letter.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;

use crate::bitmap::Bitmap;
use crate::fonts::Face;

/// The sheet's side, in texels.
pub const SIDE: u32 = 2048;

/// The largest a glyph is rasterized at, in px. Past it the raster is
/// drawn larger than it was made: a glyph this big is a handful of
/// letters filling the window, and a sheet of them would hold a dozen.
pub const MAX_PX: u32 = 384;

/// Up to here every whole pixel is a rung of its own: small text is where
/// a size off by a fraction shows.
const EXACT_PX: u32 = 32;

/// Past [`EXACT_PX`] the rungs stand this far apart, as a ratio: close
/// enough that the raster is never drawn more than a few percent away
/// from its own size.
const RUNG: f32 = 1.0905077; // 2^(1/8)

/// Transparent gutter around every cell: the sampler is linear.
const PAD: u32 = 1;

/// The size a glyph wanted at `px` is rasterized at, or none when it is
/// too small to be anything but a smudge — which is not drawn at all.
pub fn ladder(px: f32) -> Option<u32> {
    if !px.is_finite() || px < 1.0 {
        return None;
    }
    let exact = EXACT_PX as f32;
    if px <= exact {
        return Some(px.round().max(1.0) as u32);
    }
    let steps = ((px / exact).ln() / RUNG.ln()).round();
    let rung = (exact * RUNG.powf(steps)).round() as u32;
    Some(rung.min(MAX_PX))
}

/// Where one glyph sits in the sheet, and where it stands from the pen
/// that draws it — at the size it was rasterized at, which the renderer
/// scales to the size it is seen at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub uv: [f32; 4],
    /// From the pen on the baseline to the box's top-left corner, px.
    pub dx: f32,
    pub dy: f32,
    pub w: f32,
    pub h: f32,
    /// The size it was rasterized at.
    pub px: u32,
}

/// A rectangle of the sheet that changed and has not been uploaded:
/// `x, y, w, h` in texels.
pub type Region = (u32, u32, u32, u32);

/// Where the next cell goes: rows filling left to right, each as tall as
/// its tallest cell.
#[derive(Debug, Default, Clone, Copy)]
struct Shelf {
    x: u32,
    y: u32,
    row_h: u32,
}

pub struct Glyphs {
    side: u32,
    bitmap: RefCell<Bitmap>,
    cells: RefCell<HashMap<(u32, char, u32), Option<Placed>>>,
    shelf: Cell<Shelf>,
    dirty: Cell<Option<Region>>,
    full: Cell<bool>,
}

impl Default for Glyphs {
    fn default() -> Glyphs {
        Glyphs::with_side(SIDE)
    }
}

impl Glyphs {
    /// An empty sheet `side` texels a side. Nothing is allocated until
    /// the first glyph asks for room.
    pub fn with_side(side: u32) -> Glyphs {
        Glyphs {
            side,
            bitmap: RefCell::new(Bitmap {
                w: 0,
                h: 0,
                rgba: Vec::new(),
            }),
            cells: RefCell::new(HashMap::new()),
            shelf: Cell::new(Shelf {
                x: PAD,
                y: PAD,
                row_h: 0,
            }),
            dirty: Cell::new(None),
            full: Cell::new(false),
        }
    }

    /// `ch` of `face` rasterized at `px`, placed in the sheet the first
    /// time it is asked for. `None` for a glyph with no ink — a space —
    /// and for one the sheet has no room left for, which marks it full.
    pub fn cell(&self, face: &Face, ch: char, px: u32) -> Option<Placed> {
        let key = (face.id, ch, px);
        if let Some(known) = self.cells.borrow().get(&key) {
            return *known;
        }
        let (metrics, coverage) = face.rasterize(ch, px as f32);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        if w == 0 || h == 0 {
            self.cells.borrow_mut().insert(key, None);
            return None;
        }
        let (ox, oy) = self.place(w, h)?;
        self.blit(&coverage, w, h, ox, oy);
        let side = self.side as f32;
        let cell = Placed {
            uv: [
                ox as f32 / side,
                oy as f32 / side,
                (ox + w) as f32 / side,
                (oy + h) as f32 / side,
            ],
            dx: metrics.xmin as f32,
            // fontdue measures y up from the baseline; the screen
            // measures it down from the top.
            dy: -(metrics.ymin as f32 + h as f32),
            w: w as f32,
            h: h as f32,
            px,
        };
        self.cells.borrow_mut().insert(key, Some(cell));
        Some(cell)
    }

    /// Room for a `w` by `h` cell, or none — and the sheet marked full.
    fn place(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        let mut shelf = self.shelf.get();
        if shelf.x + w + PAD > self.side {
            shelf.x = PAD;
            shelf.y += shelf.row_h + PAD;
            shelf.row_h = 0;
        }
        if shelf.x + w + PAD > self.side || shelf.y + h + PAD > self.side {
            self.full.set(true);
            return None;
        }
        let at = (shelf.x, shelf.y);
        shelf.x += w + PAD;
        shelf.row_h = shelf.row_h.max(h);
        self.shelf.set(shelf);
        Some(at)
    }

    /// Writes one glyph's coverage as white texels whose alpha is that
    /// coverage, and grows the region owed to the renderer over it.
    fn blit(&self, coverage: &[u8], w: u32, h: u32, ox: u32, oy: u32) {
        let mut sheet = self.bitmap.borrow_mut();
        if sheet.rgba.is_empty() {
            *sheet = Bitmap {
                w: self.side,
                h: self.side,
                rgba: vec![0; (self.side * self.side * 4) as usize],
            };
        }
        for (i, &a) in coverage.iter().enumerate() {
            let (gx, gy) = (i as u32 % w, i as u32 / w);
            let at = (((oy + gy) * self.side + ox + gx) * 4) as usize;
            sheet.rgba[at..at + 4].copy_from_slice(&[255, 255, 255, a]);
        }
        let grown = match self.dirty.get() {
            None => (ox, oy, w, h),
            Some((x, y, dw, dh)) => {
                let (x0, y0) = (x.min(ox), y.min(oy));
                let (x1, y1) = ((x + dw).max(ox + w), (y + dh).max(oy + h));
                (x0, y0, x1 - x0, y1 - y0)
            }
        };
        self.dirty.set(Some(grown));
    }

    /// The region written since it was last asked for, and nothing owed
    /// after.
    pub fn take_dirty(&self) -> Option<Region> {
        self.dirty.take()
    }

    /// Whether a glyph found no room: the sheet has to start over.
    pub fn is_full(&self) -> bool {
        self.full.get()
    }

    /// Starts the sheet over, empty. Every cell handed out before is
    /// gone with it.
    pub fn clear(&self) {
        self.cells.borrow_mut().clear();
        self.shelf.set(Shelf {
            x: PAD,
            y: PAD,
            row_h: 0,
        });
        let mut sheet = self.bitmap.borrow_mut();
        sheet.rgba.fill(0);
        self.dirty
            .set((!sheet.rgba.is_empty()).then_some((0, 0, self.side, self.side)));
        self.full.set(false);
    }

    /// The sheet as it stands — empty until the first glyph lands.
    pub fn bitmap(&self) -> Ref<'_, Bitmap> {
        self.bitmap.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::DEFAULT_FONT;
    use crate::fonts::Fonts;

    #[test]
    fn small_sizes_are_their_own_rungs_and_large_ones_share() {
        assert_eq!(ladder(0.4), None, "too small to be a letter");
        assert_eq!(ladder(f32::NAN), None);
        assert_eq!(ladder(1.0), Some(1));
        assert_eq!(ladder(12.4), Some(12));
        assert_eq!(ladder(12.6), Some(13));
        assert_eq!(ladder(32.0), Some(32));
        let mut last = 32;
        for px in 33..2000 {
            let rung = ladder(px as f32).unwrap();
            assert!(rung >= last, "{px}px went down to {rung}");
            let off = (rung as f32 - px as f32).abs() / px as f32;
            assert!(rung == MAX_PX || off < 0.05, "{px}px is drawn from {rung}px");
            last = rung;
        }
        assert_eq!(ladder(5000.0), Some(MAX_PX));
    }

    #[test]
    fn a_glyph_is_rasterized_once_and_lands_inside_the_sheet() {
        let fonts = Fonts::bundled();
        let face = fonts.face(DEFAULT_FONT, false, false);
        let g = Glyphs::default();
        let a = g.cell(&face, 'A', 24).unwrap();
        for v in a.uv {
            assert!((0.0..=1.0).contains(&v));
        }
        assert!(a.w > 0.0 && a.h > 0.0 && a.dy < 0.0, "above the baseline");
        let owed = g.take_dirty().unwrap();
        assert!(owed.2 >= a.w as u32 && owed.3 >= a.h as u32);
        assert_eq!(g.cell(&face, 'A', 24), Some(a));
        assert_eq!(g.take_dirty(), None, "nothing new to upload");
        let bigger = g.cell(&face, 'A', 48).unwrap();
        assert!(bigger.w > a.w, "each size is a cell of its own");
    }

    #[test]
    fn a_space_has_no_cell() {
        let fonts = Fonts::bundled();
        let g = Glyphs::default();
        assert_eq!(g.cell(&fonts.face(DEFAULT_FONT, false, false), ' ', 24), None);
        assert!(!g.is_full());
    }

    #[test]
    fn cells_never_overlap() {
        let fonts = Fonts::bundled();
        let face = fonts.face(DEFAULT_FONT, false, false);
        let g = Glyphs::with_side(256);
        let cells: Vec<Placed> = "abcdefghijklmnop"
            .chars()
            .filter_map(|c| g.cell(&face, c, 20))
            .collect();
        assert_eq!(cells.len(), 16);
        for (i, p) in cells.iter().enumerate() {
            for q in &cells[i + 1..] {
                let apart = p.uv[2] <= q.uv[0] || q.uv[2] <= p.uv[0] || p.uv[3] <= q.uv[1] || q.uv[3] <= p.uv[1];
                assert!(apart, "{p:?} overlaps {q:?}");
            }
        }
    }

    #[test]
    fn a_full_sheet_says_so_and_starts_over_empty() {
        let fonts = Fonts::bundled();
        let face = fonts.face(DEFAULT_FONT, false, false);
        let g = Glyphs::with_side(128);
        let placed = "ABCDEFGH".chars().filter_map(|c| g.cell(&face, c, 60)).count();
        assert!(placed < 8 && g.is_full(), "{placed} of 8 fit");
        g.clear();
        assert!(!g.is_full());
        assert_eq!(g.take_dirty(), Some((0, 0, 128, 128)), "the whole sheet is owed");
        assert!(g.bitmap().rgba.iter().all(|&b| b == 0));
        assert!(g.cell(&face, 'A', 60).is_some());
    }

    #[test]
    fn the_sheet_is_white_where_it_is_inked() {
        let fonts = Fonts::bundled();
        let g = Glyphs::with_side(64);
        g.cell(&fonts.face(DEFAULT_FONT, false, false), 'M', 30);
        let sheet = g.bitmap();
        assert_eq!(sheet.w, 64);
        let inked = sheet.rgba.as_chunks::<4>().0.iter().find(|t| t[3] > 0);
        assert_eq!(inked.map(|t| &t[..3]), Some(&[255u8, 255, 255][..]));
    }
}
