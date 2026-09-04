//! The brush palette: Sketchbook's Brush Library on the left of the
//! canvas. Every set stands one under the next in a single scroll — a
//! heading, then its brushes as a grid of icons, six across — and above
//! them a preview of the brush in the hand: its own icon, its name, the
//! set it came off, and a dab of what it actually paints. Pure — `app`
//! asks where a click landed and what to draw.
//!
//! It is the brush tool's own chrome: it comes up with the tool and goes
//! away with it, and `Shift+B` shuts it without putting the brush down.
//!
//! The icons are Sketchbook's own art, one cell each on a sheet built by
//! `tools/import-skbrushes.py`. Two thirds of them draw a mark the canvas
//! cannot stamp yet, so the icon runs ahead of the ink; the preview's dab
//! is the part that never does.

use crate::brush::{Brush, ICONS, Preset, Set};
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport, with_alpha};
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
/// The block at the top: the brush in the hand, named.
pub const PREVIEW: f32 = 56.0;
/// A set's name over its grid.
pub const HEAD: f32 = 22.0;
pub const RADIUS: f32 = 12.0;
/// The preview's buttons are this square.
pub const BUTTON: f32 = 22.0;
const BUTTON_GAP: f32 = 2.0;
const BAR_W: f32 = 4.0;
const BAR_MIN: f32 = 24.0;
/// The preview's own icon, and the gap before the words beside it.
const PREVIEW_ICON: f32 = 34.0;
const LABEL_GAP: f32 = 8.0;
/// The dab in the preview: a stroke of what the brush lays, this wide at
/// the very most.
const DAB_MAX: f32 = 8.0;
const DAB_MIN: f32 = 1.0;
const CELL_RADIUS: f32 = 6.0;
const CELL_INSET: f32 = 1.0;
/// How thick the ring around the brush in the hand is.
const HELD_RING: f32 = 1.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
const ICON_BOX: f32 = 15.0;
const ICON_STROKE: f32 = 1.5;
/// How the sheet `tools/import-skbrushes.py` writes is cut up.
const ICON_COLS: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A brush, by the shelf it stands on and its place there.
    Brush(usize, usize),
    /// The preview's sliders: Brush Properties opens or folds away.
    Properties,
    /// The preview's undo arrow: the brush goes back to what it shipped as.
    Reset,
    /// Panel chrome: swallowed, never reaches the canvas.
    Panel,
}

/// One brush's place in the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub set: usize,
    pub index: usize,
    /// Its cell of the icon sheet.
    pub icon: u16,
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
    /// The block at the top, which does not scroll.
    pub preview: ScreenRect,
    pub reset: ScreenRect,
    pub properties: ScreenRect,
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
    /// px: the properties bar, or the tab strip.
    pub fn layout(viewport: Viewport, scale: f64, top: f32, sets: &[Set], scroll: f32) -> Palette {
        let s = scale as f32;
        let x = (MARGIN * s).round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;

        let preview = ScreenRect {
            x: inner_x,
            y: y + PADDING * s,
            w: inner_w,
            h: PREVIEW * s,
        };
        let side = BUTTON * s;
        let reset = ScreenRect {
            x: preview.x + preview.w - side,
            y: preview.y + preview.h - side,
            w: side,
            h: side,
        };
        let properties = ScreenRect {
            x: reset.x - side - BUTTON_GAP * s,
            ..reset
        };

        let band_y = preview.y + preview.h;
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
                    icon: p.icon.min(ICONS - 1),
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
                h: preview.h + band.h + 2.0 * PADDING * s,
            },
            preview,
            reset,
            properties,
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
        if self.properties.contains(x, y) {
            return Some(Hit::Properties);
        }
        if self.reset.contains(x, y) {
            return Some(Hit::Reset);
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

    /// `sets` is the whole library and `selected` the brush in the hand;
    /// `brush` is its body, which the preview's dab is drawn with.
    /// `icons` is the slot the icon sheet was uploaded to.
    #[allow(clippy::too_many_arguments)]
    pub fn prims(
        &self,
        sets: &[Set],
        selected: (usize, usize),
        brush: &Brush,
        atlas: &Atlas,
        slot: u32,
        icons: u32,
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
        self.preview_prims(sets, selected, brush, atlas, slot, icons, theme, &mut out);

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
            out.push(Prim::sprite(box_, icon_uv(cell.icon), icons).clipped(self.band));
        }
        if let Some(bar) = self.bar {
            out.push(Prim::rounded(bar, bar.w / 2.0, theme.muted));
        }
        out
    }

    /// The block at the top: the brush's own icon, its name, the shelf it
    /// came off, the two buttons, and a dab of what it paints.
    #[allow(clippy::too_many_arguments)]
    fn preview_prims(
        &self,
        sets: &[Set],
        selected: (usize, usize),
        brush: &Brush,
        atlas: &Atlas,
        slot: u32,
        icons: u32,
        theme: &Theme,
        out: &mut Vec<Prim>,
    ) {
        let s = self.scale;
        let (set, index) = selected;
        let Some((shelf, preset)) = sets
            .get(set)
            .and_then(|q| q.presets.get(index).map(|p: &Preset| (q, p)))
        else {
            return;
        };

        let side = PREVIEW_ICON * s;
        let icon_box = ScreenRect {
            x: self.preview.x,
            y: self.preview.y,
            w: side,
            h: side,
        };
        out.push(Prim::sprite(icon_box, icon_uv(preset.icon), icons));

        // The name on one line, the shelf under it in the muted color.
        let tx = icon_box.x + side + LABEL_GAP * s;
        let room = (self.properties.x - BUTTON_GAP * s - tx).max(0.0);
        let line = side / 2.0;
        for (text, y, color) in [
            (preset.name.as_str(), self.preview.y, theme.ink),
            (shelf.name.as_str(), self.preview.y + line, theme.muted),
        ] {
            let row = ScreenRect {
                x: tx,
                y,
                w: room,
                h: line,
            };
            let baseline = atlas.baseline_in(row);
            for g in atlas.layout(&atlas.truncate(text, room), tx, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, color).clipped(self.preview));
            }
        }

        out.extend(icon_prims(SLIDERS, self.properties, s, theme.icon));
        out.extend(icon_prims(RESET, self.reset, s, theme.icon));

        // And what it actually lays. The icon above is Sketchbook's art
        // and may promise a mark the ink cannot make; this is the part
        // that cannot — the three the canvas answers to, drawn.
        let (radius, feather) = dab(brush);
        let nominal = (radius + feather / 2.0) * s;
        let cy = icon_box.y + side + (self.preview.h - side) / 2.0;
        let (x0, x1) = (
            self.preview.x + nominal,
            self.properties.x - LABEL_GAP * s - nominal,
        );
        if x1 > x0 {
            out.push(
                Prim::soft_segment(
                    (x0, cy),
                    (x1, cy),
                    radius * s,
                    feather * s,
                    with_alpha(theme.ink, brush.opacity as f32),
                )
                .clipped(self.preview),
            );
        }
    }
}

/// The slice of the icon sheet one cell takes.
pub(crate) fn icon_uv(icon: u16) -> [f32; 4] {
    let cols = f32::from(ICON_COLS);
    let rows = (f32::from(ICONS) / cols).ceil();
    let (col, row) = (
        f32::from(icon % ICON_COLS),
        f32::from(icon / ICON_COLS),
    );
    [col / cols, row / rows, (col + 1.0) / cols, (row + 1.0) / rows]
}

/// The half-width and edge ramp the preview's dab is drawn with, in
/// logical px. Size is taken by its square root so a fine brush is still
/// visible beside a fat one; the ramp is the stroke's own, so a soft
/// brush previews soft.
fn dab(brush: &Brush) -> (f32, f32) {
    let t = ((brush.size / crate::brush::SIZE_MAX) as f32).sqrt();
    let r = DAB_MIN + t * (DAB_MAX - DAB_MIN);
    let feather = (1.0 - brush.hardness.clamp(0.0, 1.0)) as f32 * r;
    (r - feather / 2.0, feather)
}

/// Sliders: what Brush Properties is.
const SLIDERS: &[&[(f32, f32)]] = &[
    &[(4.0, 8.0), (20.0, 8.0)],
    &[(4.0, 16.0), (20.0, 16.0)],
    &[(9.0, 5.5), (9.0, 10.5)],
    &[(15.0, 13.5), (15.0, 18.5)],
];
/// An arrow curling back on itself: the brush as it shipped.
const RESET: &[&[(f32, f32)]] = &[
    &[
        (4.0, 12.0),
        (4.6, 8.4),
        (6.6, 5.6),
        (9.7, 4.2),
        (13.1, 4.4),
        (16.6, 6.4),
        (18.9, 9.6),
        (19.4, 13.4),
        (18.0, 16.9),
        (15.2, 19.3),
        (11.6, 20.0),
    ],
    &[(4.0, 7.0), (4.0, 12.0), (9.0, 12.0)],
];

fn icon_prims(lines: &[&[(f32, f32)]], r: ScreenRect, s: f32, color: Rgba) -> Vec<Prim> {
    scene::icon_prims(lines, r, 24.0, ICON_BOX, ICON_STROKE, s, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::Library;
    use crate::scene::KIND_SEGMENT;
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

    fn palette(vp: Viewport, scale: f64, scroll: f32) -> (Library, Palette) {
        let lib = Library::default();
        let p = Palette::layout(vp, scale, TOP, lib.sets(), scroll);
        (lib, p)
    }

    #[test]
    fn the_panel_stands_at_the_left_with_a_grid_six_across() {
        let (lib, p) = palette(TALL, 1.0, 0.0);
        assert_eq!(p.rect.x, MARGIN, "off the window's left edge");
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
            assert!(c.icon < ICONS);
        }
        let uv = icon_uv(0);
        assert_eq!((uv[0], uv[1]), (0.0, 0.0), "the first cell is the corner");
        let last = icon_uv(ICONS - 1);
        assert!(last[2] <= 1.0 && last[3] <= 1.0, "and the last one fits");
        assert!(
            (icon_uv(1)[0] - uv[2]).abs() < 1e-6,
            "cells sit edge to edge across a row"
        );
    }

    #[test]
    fn layout_scales_with_the_display() {
        let (_, p) = palette(TALL, 2.0, 0.0);
        assert_eq!(p.rect.x, MARGIN * 2.0);
        assert_eq!(p.rect.w, WIDTH * 2.0);
        assert_eq!(p.cells[0].rect.w, CELL * 2.0);
    }

    #[test]
    fn hit_reports_the_brush_the_buttons_and_the_panel() {
        let (_, p) = palette(VP, 1.0, 0.0);
        let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));

        let c = p.cells[3];
        let (x, y) = mid(c.rect);
        assert_eq!(p.hit(x, y), Some(Hit::Brush(c.set, c.index)));
        let (x, y) = mid(p.reset);
        assert_eq!(p.hit(x, y), Some(Hit::Reset));
        let (x, y) = mid(p.properties);
        assert_eq!(p.hit(x, y), Some(Hit::Properties));
        let (x, y) = mid(p.heads[0].rect);
        assert_eq!(p.hit(x, y), Some(Hit::Panel), "a shelf's name is no brush");
        assert_eq!(p.hit(2.0, 2.0), None, "outside is canvas");
    }

    #[test]
    fn the_library_outruns_the_band_and_the_scroll_walks_it() {
        let (lib, p) = palette(VP, 1.0, 0.0);
        assert!(p.max_scroll() > 0.0, "211 brushes fit no window");
        assert!(p.bar.is_some(), "so the thumb says where the list is");
        assert!(
            p.cells.iter().all(|c| c.set < lib.sets().len() - 1),
            "only what the band reaches is measured"
        );

        let (_, deep) = palette(VP, 1.0, 900.0);
        assert_eq!(deep.scroll(), 900.0);
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
        let prims = p.prims(lib.sets(), (0, 2), lib.brush(), &atlas, 7, 9, &theme);

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
        assert_eq!(
            sprites,
            p.cells.len() + 1,
            "a sprite per cell, and one more for the preview"
        );
    }

    #[test]
    fn the_preview_names_the_brush_and_shows_what_it_lays() {
        let theme = Theme::light();
        let atlas = atlas();
        let (lib, p) = palette(VP, 1.0, 0.0);
        let prims = p.prims(lib.sets(), (0, 0), lib.brush(), &atlas, 7, 9, &theme);

        // The buttons' icons are segments in the preview too; the dab is
        // the one that runs across it, clear of both.
        let dab_in = |list: &[Prim]| {
            list.iter()
                .find(|q| {
                    q.kind == KIND_SEGMENT
                        && p.preview.contains_rect(&q.bounds())
                        && q.bounds().intersect(&p.properties).is_none()
                        && q.bounds().intersect(&p.reset).is_none()
                })
                .copied()
                .expect("the preview draws a dab")
        };
        let dab = dab_in(&prims);
        assert!(dab.radius > 0.0);
        assert!(
            (f64::from(dab.color[3]) - lib.brush().opacity).abs() < 1e-3,
            "drawn at the brush's own opacity, not the theme's ink"
        );

        let soft = Brush {
            hardness: 0.0,
            ..*lib.brush()
        };
        let softened = p.prims(lib.sets(), (0, 0), &soft, &atlas, 7, 9, &theme);
        assert!(
            dab_in(&softened).feather > dab.feather,
            "a softer brush previews softer"
        );
    }

    #[test]
    fn nothing_the_panel_draws_escapes_it() {
        let theme = Theme::light();
        let atlas = atlas();
        let (lib, p) = palette(Viewport { w: 900, h: 420 }, 1.0, 60.0);
        let prims = p.prims(lib.sets(), (1, 1), lib.brush(), &atlas, 7, 9, &theme);
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
