//! The brush library: the shelf this build ships, then whatever was
//! imported, every set one under the next in a single scroll — a heading, then its brushes as a grid of icons, six
//! across. Pure — `app` asks where a click landed and what to draw.
//!
//! It opens beside the brush strip, from the chevron there, and is the
//! only place every brush can be reached: what the strip keeps is
//! the ten a hand goes back to. Which brush is in the hand, and the two
//! buttons that were once above this grid, belong to the strip — see
//! [`crate::slots`].
//!
//! A brush this build ships is pictured by what it lays — a stroke of
//! its own width, edge, strength and taper, drawn in the chrome's ink
//! ([`brush_icon`]). An imported one wears its set's own art, a cell of
//! the sheet `sinopia brushes import` writes. Some of that art draws a
//! mark the canvas cannot stamp yet, so the icon runs ahead of the ink;
//! the preview's dab is the part that never does.

use crate::brush::{Brush, Icon, Preset, Set};
use crate::doc::Mark;
use crate::scene::{self, Prim, ScreenRect, Viewport};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
/// Six icons across, the way Sketchbook lays its library out.
pub const COLS: usize = 6;
/// One brush's cell in the grid, and the icon centered in it.
pub const CELL: f32 = 34.0;
pub const ICON: f32 = 30.0;
pub const PADDING: f32 = 8.0;
/// The room the scrollbar keeps down the panel's right edge.
const RAIL: f32 = 10.0;
pub const WIDTH: f32 = 2.0 * PADDING + COLS as f32 * CELL + RAIL;
/// From the window's left edge, and from whatever is above the panel.
pub const MARGIN: f32 = 12.0;
/// A set's name over its grid.
pub const HEAD: f32 = 22.0;
pub const RADIUS: f32 = 12.0;
const BAR_W: f32 = 4.0;
const BAR_MIN: f32 = 24.0;
const CELL_RADIUS: f32 = 6.0;
const CELL_INSET: f32 = 1.0;
/// How thick the ring around the brush in the hand is.
const HELD_RING: f32 = 1.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A brush, by the shelf it stands on and its place there.
    Brush(usize, usize),
    /// Panel chrome: swallowed, never reaches the canvas.
    Panel,
}

/// One brush's place in the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub set: usize,
    pub index: usize,
    /// What pictures it.
    pub icon: Icon,
    pub rect: ScreenRect,
}

/// A set's name, over the grid of its brushes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Head {
    pub set: usize,
    pub rect: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub rect: ScreenRect,
    /// As much of the library as the window has room for.
    pub band: ScreenRect,
    pub cells: Vec<Cell>,
    pub heads: Vec<Head>,
    /// The thumb, when there is more library than band.
    pub bar: Option<ScreenRect>,
    scroll: f32,
    content: f32,
    scale: f32,
}

/// How tall one set stands: its heading, then a row per six brushes.
fn set_height(brushes: usize, s: f32) -> f32 {
    (HEAD + brushes.div_ceil(COLS) as f32 * CELL) * s
}

impl Palette {
    /// `top` is where whatever stands above the panel ends, in physical
    /// px: the properties bar, or the tab strip. `left` is the panel's
    /// own x, also physical: it stands beside the strip rather than at
    /// the window's edge, so it is told where rather than working it out.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        top: f32,
        left: f32,
        sets: &[Set],
        scroll: f32,
    ) -> Palette {
        let s = scale as f32;
        let x = left.round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;

        let band_y = y + PADDING * s;
        let room = (viewport.h as f32 - (MARGIN + PADDING) * s - band_y).max(0.0);
        let content: f32 = sets.iter().map(|q| set_height(q.presets.len(), s)).sum();
        let band = ScreenRect {
            x: inner_x,
            y: band_y,
            w: inner_w,
            h: content.min(room),
        };
        let scroll = scroll.clamp(0.0, (content - band.h).max(0.0));

        // Every set falls where the ones above it leave it; only the ones
        // the band reaches are measured, and what it reaches part of is
        // cut when it is drawn.
        let (mut heads, mut cells) = (Vec::new(), Vec::new());
        let (mut shelf_y, bottom) = (band.y - scroll, band.y + band.h);
        for (set, q) in sets.iter().enumerate() {
            let tall = set_height(q.presets.len(), s);
            if shelf_y + tall > band.y && shelf_y < bottom {
                heads.push(Head {
                    set,
                    rect: ScreenRect {
                        x: inner_x,
                        y: shelf_y,
                        w: inner_w - RAIL * s,
                        h: HEAD * s,
                    },
                });
                let grid = shelf_y + HEAD * s;
                cells.extend(q.presets.iter().enumerate().map(|(index, p)| Cell {
                    set,
                    index,
                    icon: p.icon,
                    rect: ScreenRect {
                        x: inner_x + (index % COLS) as f32 * CELL * s,
                        y: grid + (index / COLS) as f32 * CELL * s,
                        w: CELL * s,
                        h: CELL * s,
                    },
                }));
            }
            shelf_y += tall;
        }

        let bar = (content > band.h).then(|| {
            let h = (band.h * band.h / content).max(BAR_MIN * s).min(band.h);
            let at = scroll / (content - band.h);
            ScreenRect {
                x: inner_x + inner_w - BAR_W * s,
                y: band.y + at * (band.h - h),
                w: BAR_W * s,
                h,
            }
        });

        Palette {
            rect: ScreenRect {
                x,
                y,
                w: WIDTH * s,
                h: band.h + 2.0 * PADDING * s,
            },
            band,
            cells,
            heads,
            bar,
            scroll,
            content,
            scale: s,
        }
    }

    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content - self.band.h).max(0.0)
    }

    /// Where the list has to stand for a brush's cell to be in the band —
    /// the nearer edge, so a cell already on show does not move.
    pub fn scroll_showing(&self, sets: &[Set], set: usize, index: usize) -> f32 {
        let s = self.scale;
        let above: f32 = sets
            .iter()
            .take(set)
            .map(|q| set_height(q.presets.len(), s))
            .sum();
        let top = above + (HEAD + (index / COLS) as f32 * CELL) * s;
        self.scroll
            .min(top)
            .max(top + CELL * s - self.band.h)
            .clamp(0.0, self.max_scroll())
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let cell = self
            .cells
            .iter()
            .find(|c| c.rect.contains(x, y) && self.band.contains(x, y));
        Some(match cell {
            Some(c) => Hit::Brush(c.set, c.index),
            None => Hit::Panel,
        })
    }

    /// `sets` is the whole library and `selected` the brush in the hand,
    /// which wears a ring. `icons` is the imported icon sheet, when there
    /// is one on the GPU.
    pub fn prims(
        &self,
        sets: &[Set],
        selected: (usize, usize),
        atlas: &Atlas,
        slot: u32,
        icons: Option<IconSheet>,
        theme: &Theme,
    ) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(RADIUS, s);
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                corner,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-b), corner + b, theme.border),
            Prim::rounded(self.rect, corner, theme.panel),
        ];
        for head in &self.heads {
            let Some(set) = sets.get(head.set) else { continue };
            let baseline = atlas.baseline_in(head.rect);
            let name = atlas.truncate(&set.name, head.rect.w);
            for g in atlas.layout(&name, head.rect.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(self.band));
            }
        }
        for cell in &self.cells {
            // The brush in the hand wears a ring, which is a filled
            // rounded box with the panel's own color laid back inside it.
            if (cell.set, cell.index) == selected {
                let box_ = cell.rect.inset(CELL_INSET * s);
                let cell = theme.corner(CELL_RADIUS, s);
                out.push(Prim::rounded(box_, cell, theme.selection).clipped(self.band));
                out.push(
                    Prim::rounded(box_.inset(HELD_RING * s), cell, theme.active_bg)
                        .clipped(self.band),
                );
            }
            let side = ICON * s;
            let box_ = ScreenRect {
                x: cell.rect.x + (cell.rect.w - side) / 2.0,
                y: cell.rect.y + (cell.rect.h - side) / 2.0,
                w: side,
                h: side,
            };
            if let Some(preset) = sets.get(cell.set).and_then(|q| q.presets.get(cell.index)) {
                out.extend(
                    brush_icon(preset, box_, icons, theme)
                        .into_iter()
                        .map(|p| p.clipped(self.band)),
                );
            }
        }
        if let Some(bar) = self.bar {
            out.push(Prim::rounded(bar, bar.w / 2.0, theme.muted));
        }
        out
    }

}

/// The imported icon sheet as the renderer holds it: the slot it was
/// uploaded to, and how it is cut up — cells across, and cells in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconSheet {
    pub slot: u32,
    pub cols: u16,
    pub count: u16,
}

impl IconSheet {
    /// The slice of the sheet one cell takes.
    pub(crate) fn uv(&self, icon: u16) -> [f32; 4] {
        let cols = f32::from(self.cols.max(1));
        let rows = (f32::from(self.count) / cols).ceil().max(1.0);
        let (col, row) = (
            f32::from(icon % self.cols.max(1)),
            f32::from(icon / self.cols.max(1)),
        );
        [col / cols, row / rows, (col + 1.0) / cols, (row + 1.0) / rows]
    }
}

/// A brush's icon, in the box `r`: its cell of the imported sheet, or —
/// for a brush this build ships, or an imported one whose sheet is not
/// on the GPU — a stroke drawn from its own body.
pub fn brush_icon(preset: &Preset, r: ScreenRect, sheet: Option<IconSheet>, theme: &Theme) -> Vec<Prim> {
    match (preset.icon, sheet) {
        (Icon::Sheet(cell), Some(sheet)) if cell < sheet.count => {
            vec![Prim::sprite(r, sheet.uv(cell), sheet.slot)]
        }
        _ => drawn(&preset.brush, r, theme),
    }
}

/// Spans the drawn stroke is cut into.
const DRAWN_SPANS: usize = 16;

/// What a brush lays, as a picture of it: one wave across the box, as
/// wide as the brush is — a hairline for a liner, a band for an airbrush,
/// on a scale that keeps both inside the box — its edge as soft as the
/// brush's, its ends thinned as far as the pen thins it, laid at its own
/// strength. An eraser is the same wave in the muted ink with a block
/// over its end. The ink is mixed with the panel rather than laid
/// translucent, so the spans that meet at every joint do not bead.
fn drawn(brush: &Brush, r: ScreenRect, theme: &Theme) -> Vec<Prim> {
    let erases = brush.mark == Mark::Erase;
    let ink = if erases { theme.muted } else { theme.icon };
    let color = scene::mix(theme.panel, ink, brush.opacity.clamp(0.35, 1.0) as f32);
    // Sizes run over two decades; the width follows their logarithm.
    let reach = (brush.size.max(1.0).ln() / 100f64.ln()).clamp(0.0, 1.0) as f32;
    let half = r.w * (0.03 + 0.09 * reach);
    let feather = (1.0 - brush.hardness as f32) * half * 2.0;
    let drive = brush.pressure.size.clamp(0.0, 1.0) as f32;
    let (cy, amp) = (r.y + r.h / 2.0, r.h * 0.16);
    let at = |t: f32| {
        (
            r.x + r.w * (0.18 + 0.64 * t),
            cy - amp * (std::f32::consts::TAU * t).sin(),
        )
    };
    let mut out: Vec<Prim> = (0..DRAWN_SPANS)
        .map(|i| {
            let (t0, t1) = (
                i as f32 / DRAWN_SPANS as f32,
                (i + 1) as f32 / DRAWN_SPANS as f32,
            );
            // Full in the middle of the stroke, thinned toward both ends
            // by as much of the width as the pen takes away.
            let swell = (std::f32::consts::PI * (t0 + t1) / 2.0).sin();
            let w = (half * (1.0 - drive * (1.0 - swell))).max(0.6);
            Prim::soft_segment(at(t0), at(t1), w, feather, color)
        })
        .collect();
    if erases {
        const BLOCK: &[&[(f32, f32)]] = &[&[(14.0, 8.0), (20.0, 14.0), (14.5, 19.5), (8.5, 13.5), (14.0, 8.0)]];
        out.extend(scene::icon_prims(BLOCK, r, 24.0, r.w, r.w / 16.0, 1.0, theme.icon));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::Library;
    use crate::scene::{self, KIND_SEGMENT};
    use crate::tabs::Tabs;
    use crate::text::Font;

    const VP: Viewport = Viewport { w: 900, h: 900 };
    /// Where the tab strip ends, in physical px.
    const TOP: f32 = 34.0;
    /// Tall enough that the whole library is laid out at once.
    const TALL: Viewport = Viewport { w: 900, h: 4000 };

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    /// Where the strip leaves the library, in physical px.
    const LEFT: f32 = 156.0;

    /// The imported sheet as the tests upload it.
    fn sheet(lib: &Library) -> Option<IconSheet> {
        let (cols, count) = lib.icon_grid();
        Some(IconSheet { slot: 9, cols, count })
    }

    fn palette(vp: Viewport, scale: f64, scroll: f32) -> (Library, Palette) {
        let lib = Library::acquired();
        let p = Palette::layout(vp, scale, TOP, LEFT * scale as f32, lib.sets(), scroll);
        (lib, p)
    }

    #[test]
    fn the_panel_stands_at_the_left_with_a_grid_six_across() {
        let (lib, p) = palette(TALL, 1.0, 0.0);
        assert_eq!(p.rect.x, LEFT, "where the strip leaves it");
        assert_eq!(p.rect.y, TOP + MARGIN, "under whatever is above it");
        assert_eq!(p.rect.w, WIDTH);

        let first: Vec<&Cell> = p.cells.iter().filter(|c| c.set == 0).collect();
        assert_eq!(first.len(), lib.sets()[0].presets.len());
        for (i, c) in first.iter().enumerate().take(COLS + 1) {
            assert_eq!(c.index, i);
            if i < COLS {
                assert_eq!(c.rect.y, first[0].rect.y, "the first row stays level");
                assert_eq!(c.rect.x, first[0].rect.x + i as f32 * CELL);
            } else {
                assert_eq!(c.rect.x, first[0].rect.x, "and the seventh wraps");
                assert_eq!(c.rect.y, first[0].rect.y + CELL);
            }
        }
    }

    #[test]
    fn every_set_stands_under_the_one_before_it_with_its_name_over_it() {
        let (lib, p) = palette(TALL, 1.0, 0.0);
        assert_eq!(p.heads.len(), lib.sets().len(), "every shelf is named");
        for (i, h) in p.heads.iter().enumerate() {
            assert_eq!(h.set, i, "in the library's own order");
            let cells: Vec<&Cell> = p.cells.iter().filter(|c| c.set == i).collect();
            assert_eq!(cells.len(), lib.sets()[i].presets.len());
            assert!(cells[0].rect.y >= h.rect.y + h.rect.h, "grid under its name");
            if i > 0 {
                assert!(h.rect.y > p.heads[i - 1].rect.y, "shelves stack downward");
            }
        }
    }

    #[test]
    fn a_cell_carries_the_icon_its_brush_names() {
        let (lib, p) = palette(TALL, 1.0, 0.0);
        for c in &p.cells {
            assert_eq!(c.icon, lib.sets()[c.set].presets[c.index].icon);
        }
        let sheet = IconSheet {
            slot: 7,
            cols: 16,
            count: 211,
        };
        let uv = sheet.uv(0);
        assert_eq!((uv[0], uv[1]), (0.0, 0.0), "the first cell is the corner");
        let last = sheet.uv(210);
        assert!(last[2] <= 1.0 && last[3] <= 1.0, "and the last one fits");
        assert!(
            (sheet.uv(1)[0] - uv[2]).abs() < 1e-6,
            "cells sit edge to edge across a row"
        );
    }

    #[test]
    fn a_shipped_brush_is_drawn_and_an_imported_one_wears_its_cell() {
        let theme = Theme::light();
        let r = ScreenRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        };
        let lib = Library::default();
        let pencil = &lib.sets()[0].presets[0];
        let sheet = IconSheet {
            slot: 7,
            cols: 16,
            count: 4,
        };
        let drawn = brush_icon(pencil, r, Some(sheet), &theme);
        assert!(drawn.len() >= DRAWN_SPANS, "a stroke, span by span");
        assert!(drawn.iter().all(|p| p.slot != 7), "and no cell of the sheet");
        for p in &drawn {
            let b = p.painted_bounds();
            assert!(b.x >= r.x - 1.0 && b.x + b.w <= r.x + r.w + 1.0, "inside its box");
        }

        let mut imported = pencil.clone();
        imported.icon = Icon::Sheet(2);
        let cell = brush_icon(&imported, r, Some(sheet), &theme);
        assert_eq!(cell.len(), 1);
        assert_eq!(cell[0].slot, 7, "the sheet's own art");
        assert!(
            brush_icon(&imported, r, None, &theme).len() > 1,
            "a sheet that is not on the GPU leaves it drawn"
        );
        let mut off = pencil.clone();
        off.icon = Icon::Sheet(9);
        assert!(brush_icon(&off, r, Some(sheet), &theme).len() > 1, "a cell off the sheet too");
    }

    #[test]
    fn a_wider_softer_brush_is_drawn_wider_and_softer() {
        let theme = Theme::light();
        let r = ScreenRect {
            x: 0.0,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        };
        let lib = Library::default();
        let find = |name: &str| {
            lib.sets()[0]
                .presets
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let widest = |p: &Preset| {
            brush_icon(p, r, None, &theme)
                .iter()
                .map(|q| q.radius)
                .fold(0.0, f32::max)
        };
        assert!(widest(find("Soft Round")) > widest(find("Fine Liner")));
        let liner = brush_icon(find("Fine Liner"), r, None, &theme);
        let soft = brush_icon(find("Soft Round"), r, None, &theme);
        assert!(soft[0].feather > liner[0].feather, "the soft one ramps wider");
        assert!(
            brush_icon(find("Eraser"), r, None, &theme).len() > liner.len(),
            "an eraser wears its block"
        );
    }

    #[test]
    fn layout_scales_with_the_display() {
        let (_, p) = palette(TALL, 2.0, 0.0);
        assert_eq!(p.rect.x, LEFT * 2.0);
        assert_eq!(p.rect.w, WIDTH * 2.0);
        assert_eq!(p.cells[0].rect.w, CELL * 2.0);
    }

    #[test]
    fn hit_reports_the_brush_and_the_panel() {
        let (_, p) = palette(VP, 1.0, 0.0);
        let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));

        let c = p.cells[3];
        let (x, y) = mid(c.rect);
        assert_eq!(p.hit(x, y), Some(Hit::Brush(c.set, c.index)));
        let (x, y) = mid(p.heads[0].rect);
        assert_eq!(p.hit(x, y), Some(Hit::Panel), "a shelf's name is no brush");
        assert_eq!(p.hit(2.0, 2.0), None, "outside is canvas");
    }

    #[test]
    fn the_library_outruns_the_band_and_the_scroll_walks_it() {
        let (lib, p) = palette(VP, 1.0, 0.0);
        assert!(p.max_scroll() > 0.0, "a library this long fits no window");
        assert!(p.bar.is_some(), "so the thumb says where the list is");
        assert!(
            p.cells.iter().all(|c| c.set < lib.sets().len() - 1),
            "only what the band reaches is measured"
        );

        let (_, deep) = palette(VP, 1.0, 300.0);
        assert_eq!(deep.scroll(), 300.0);
        assert!(
            deep.cells.iter().all(|c| c.set > 0),
            "the first shelf walked off the top"
        );
        let (_, past) = palette(VP, 1.0, 99999.0);
        assert_eq!(past.scroll(), past.max_scroll(), "and it stops at the end");
        assert!(
            past.cells.iter().any(|c| c.set == lib.sets().len() - 1),
            "the bottom of the library is reachable"
        );
    }

    #[test]
    fn the_list_glides_only_far_enough_to_show_a_brush() {
        let (lib, p) = palette(VP, 1.0, 0.0);
        let sets = lib.sets();
        assert_eq!(
            p.scroll_showing(sets, 0, 0),
            0.0,
            "a cell already on show does not move the list"
        );
        let deep = p.scroll_showing(sets, sets.len() - 1, 0);
        assert!(deep > 0.0, "and one far down brings the list to it");
        assert!(deep <= p.max_scroll());
    }

    #[test]
    fn prims_paint_the_panel_the_icons_and_the_brush_in_hand() {
        let theme = Theme::light();
        let atlas = atlas();
        let (lib, p) = palette(VP, 1.0, 0.0);
        let prims = p.prims(lib.sets(), (0, 2), &atlas, 7, sheet(&lib), &theme);

        assert!(prims[0].feather > 0.0, "the soft shadow goes first");
        assert!(
            prims.iter().any(|q| q.color == theme.panel && q.bounds() == p.rect),
            "panel body"
        );
        let framed: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.color == theme.selection)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(framed.len(), 1, "only the brush in hand is ringed");
        let held = p.cells.iter().find(|c| (c.set, c.index) == (0, 2)).unwrap();
        assert!(held.rect.contains_rect(&framed[0]));

        let sprites = prims.iter().filter(|q| q.slot == 9).count();
        let imported = p
            .cells
            .iter()
            .filter(|c| matches!(c.icon, Icon::Sheet(_)))
            .count();
        assert!(imported > 0, "some imported cells are on show");
        assert_eq!(sprites, imported, "a sprite per imported cell, and nothing else");
    }

    #[test]
    fn the_library_is_the_shelves_alone() {
        let theme = Theme::light();
        let (lib, p) = palette(TALL, 1.0, 0.0);
        let prims = p.prims(lib.sets(), (0, 0), &atlas(), 7, sheet(&lib), &theme);
        // What is drawn in segments is the shipped brushes' icons, each
        // inside its own cell: the dab and the two buttons went to the
        // strip.
        assert!(
            prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT)
                .all(|q| p.cells.iter().any(|c| c.rect.inset(-1.0).contains_rect(&q.bounds()))),
            "a segment outside every cell"
        );
        assert_eq!(
            p.band.y,
            p.rect.y + PADDING,
            "and the shelves start at the top"
        );
    }

    #[test]
    fn nothing_the_panel_draws_escapes_it() {
        let theme = Theme::light();
        let atlas = atlas();
        let (lib, p) = palette(Viewport { w: 900, h: 420 }, 1.0, 60.0);
        let prims = p.prims(lib.sets(), (1, 1), &atlas, 7, sheet(&lib), &theme);
        let room = p.rect.inset(-2.0);
        for q in prims.iter().skip(1) {
            assert!(
                room.contains_rect(&q.bounds()) || q.clip != scene::NO_CLIP,
                "{:?} is loose outside the panel",
                q.bounds()
            );
        }
    }
}
