//! The brush palette: Sketchbook's panel on the left, showing one set of
//! the library at a time. One row per brush — a dab drawn with that
//! brush's own settings, and its name — over a rail on the panel's right
//! edge carrying the two sliders Sketchbook puts there: size on top,
//! opacity under it. Pure — `app` asks where a click landed and what to
//! draw.
//!
//! It is the brush tool's own chrome: it comes up with the tool and goes
//! away with it, and `Shift+B` shuts it without putting the brush down.

use crate::brush::{Brush, Property, Set};
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport, with_alpha};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
pub const WIDTH: f32 = 208.0;
/// From the window's left edge, and from whatever is above the panel.
pub const MARGIN: f32 = 12.0;
pub const HEADER: f32 = 34.0;
pub const ROW: f32 = 34.0;
pub const PADDING: f32 = 6.0;
pub const RADIUS: f32 = 12.0;
/// The header's buttons are this square.
pub const BUTTON: f32 = 24.0;
const BUTTON_GAP: f32 = 2.0;
/// The rail down the panel's right edge, and the room it keeps from the
/// rows it stands beside.
const RAIL: f32 = 26.0;
const RAIL_GAP: f32 = 4.0;
/// Between the two sliders on the rail. Wide enough that a knob at the
/// bottom of the top track and one at the top of the bottom track do not
/// meet and read as a single control.
const RAIL_SPLIT: f32 = 32.0;
const TRACK: f32 = 4.0;
const KNOB: f32 = 11.0;
/// A row's dab preview: this wide, and this far from the name beside it.
const SWATCH: f32 = 46.0;
const LABEL_GAP: f32 = 8.0;
/// The half-width a dab is drawn at, from the smallest brush to the
/// largest. It is not the brush's own size: a 500-unit brush would fill
/// the panel, and a 1-unit one would vanish.
const DAB_MIN: f32 = 1.0;
const DAB_MAX: f32 = 11.0;
const ROW_RADIUS: f32 = 6.0;
/// A row's card sits this far inside it, so the gap between two cards is
/// twice this and a click in the gap still lands on a row.
const CARD_INSET: f32 = 2.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
/// The 24-unit icon grid maps onto a box this big, centered in its button.
const ICON_BOX: f32 = 16.0;
const ICON_STROKE: f32 = 1.5;

/// What the palette's two sliders carry, top to bottom — Sketchbook's
/// pair: the size, and then what a brush is judged translucent by.
pub const RAILS: [Property; 2] = [Property::Size, Property::Opacity];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A brush of the set on show.
    Brush(usize),
    /// The header's set button: the next shelf of the library.
    Set,
    /// The header's undo arrow: the brush goes back to what it shipped as.
    Reset,
    /// One of the rail's sliders, grabbed anywhere along it.
    Rail(Property),
    /// Panel chrome: swallowed, never reaches the canvas.
    Panel,
}

/// One brush's row, with everything already measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub index: usize,
    pub rect: ScreenRect,
    /// Where the dab is drawn.
    pub swatch: ScreenRect,
    /// Where the name starts.
    pub name: ScreenRect,
}

/// One slider on the rail: the property it carries and the track it runs
/// down. Full at the top, as a slider standing on end reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub property: Property,
    pub track: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub rect: ScreenRect,
    pub header: ScreenRect,
    pub set: ScreenRect,
    pub reset: ScreenRect,
    /// As much of the set as the window has room for.
    pub band: ScreenRect,
    pub rows: Vec<Row>,
    pub sliders: [Slider; 2],
    scroll: f32,
    content: f32,
    scale: f32,
}

impl Palette {
    /// `top` is where whatever stands above the panel ends, in physical
    /// px: the tab strip, or the properties bar when it is up.
    pub fn layout(viewport: Viewport, scale: f64, top: f32, set: &Set, scroll: f32) -> Palette {
        let s = scale as f32;
        let x = (MARGIN * s).round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;
        let header = ScreenRect {
            x: inner_x,
            y: y + PADDING * s,
            w: inner_w,
            h: HEADER * s,
        };

        // Buttons, right-aligned in the header: the library's shelves,
        // then Brush Properties.
        let side = BUTTON * s;
        let by = header.y + (header.h - side) / 2.0;
        let reset = ScreenRect {
            x: header.x + header.w - side,
            y: by,
            w: side,
            h: side,
        };
        let set_button = ScreenRect {
            x: reset.x - side - BUTTON_GAP * s,
            ..reset
        };

        let row_h = ROW * s;
        let band_y = header.y + header.h;
        let room = (viewport.h as f32 - (MARGIN + PADDING) * s - band_y).max(0.0);
        let content = set.presets.len() as f32 * row_h;
        let band = ScreenRect {
            x: inner_x,
            y: band_y,
            w: inner_w,
            h: content.min(room),
        };
        let scroll = scroll.clamp(0.0, (content - band.h).max(0.0));

        // The rail stands down the band's right edge; the rows have what
        // is left of the width.
        let rail_w = RAIL * s;
        let rows_w = (band.w - rail_w - RAIL_GAP * s).max(0.0);
        let first = if row_h > 0.0 {
            (scroll / row_h).floor() as usize
        } else {
            0
        };
        let rows = (first..set.presets.len())
            .map(|index| {
                let rect = ScreenRect {
                    x: inner_x,
                    y: band.y + index as f32 * row_h - scroll,
                    w: rows_w,
                    h: row_h,
                };
                let swatch = ScreenRect {
                    x: rect.x + (CARD_INSET + PADDING) * s,
                    y: rect.y + CARD_INSET * s,
                    w: SWATCH * s,
                    h: rect.h - 2.0 * CARD_INSET * s,
                };
                let name_x = swatch.x + swatch.w + LABEL_GAP * s;
                Row {
                    index,
                    rect,
                    swatch,
                    name: ScreenRect {
                        x: name_x,
                        y: rect.y,
                        w: (rect.x + rect.w - PADDING * s - name_x).max(0.0),
                        h: rect.h,
                    },
                }
            })
            .take_while(|r| r.rect.y < band.y + band.h)
            .collect();

        let rail_x = inner_x + band.w - rail_w;
        let track_x = rail_x + (rail_w - TRACK * s) / 2.0;
        let inset = KNOB * s / 2.0;
        let half = ((band.h - RAIL_SPLIT * s) / 2.0).max(0.0);
        let track = |i: usize| ScreenRect {
            x: track_x,
            y: band.y + i as f32 * (half + RAIL_SPLIT * s) + inset,
            w: TRACK * s,
            h: (half - 2.0 * inset).max(0.0),
        };
        let sliders = [
            Slider {
                property: RAILS[0],
                track: track(0),
            },
            Slider {
                property: RAILS[1],
                track: track(1),
            },
        ];

        Palette {
            rect: ScreenRect {
                x,
                y,
                w: WIDTH * s,
                h: header.h + band.h + 2.0 * PADDING * s,
            },
            header,
            set: set_button,
            reset,
            band,
            rows,
            sliders,
            scroll,
            content,
            scale: s,
        }
    }

    /// How wide the rail's sliders are grabbed: the whole rail, not the
    /// hairline track drawn down the middle of it.
    fn rail(&self) -> (f32, f32) {
        let rail_w = RAIL * self.scale;
        (self.band.x + self.band.w - rail_w, rail_w)
    }

    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content - self.band.h).max(0.0)
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.set.contains(x, y) {
            return Some(Hit::Set);
        }
        if self.reset.contains(x, y) {
            return Some(Hit::Reset);
        }
        let (rail_x, rail_w) = self.rail();
        if (f64::from(rail_x)..=f64::from(rail_x + rail_w)).contains(&x) {
            let grab = KNOB * self.scale / 2.0;
            if let Some(s) = self.sliders.iter().find(|s| {
                (f64::from(s.track.y - grab)..=f64::from(s.track.y + s.track.h + grab)).contains(&y)
            }) {
                return Some(Hit::Rail(s.property));
            }
        }
        let row = self
            .rows
            .iter()
            .find(|r| r.rect.contains(x, y) && self.band.contains(x, y));
        Some(match row {
            Some(r) => Hit::Brush(r.index),
            None => Hit::Panel,
        })
    }

    /// Where `y` lands along `property`'s track, 0–1. The top of a track
    /// standing on end is its full value.
    pub fn fraction(&self, property: Property, y: f64) -> f64 {
        let Some(s) = self.sliders.iter().find(|s| s.property == property) else {
            return 0.0;
        };
        if s.track.h <= 0.0 {
            return 0.0;
        }
        (1.0 - (y - f64::from(s.track.y)) / f64::from(s.track.h)).clamp(0.0, 1.0)
    }

    /// `selected` is the brush of this set that is in the hand, if the
    /// hand holds one of them at all — the palette shows one shelf, and
    /// the brush painting may be off another. `brush` is what the rails
    /// read, whichever shelf it came from.
    pub fn prims(
        &self,
        set: &Set,
        selected: Option<usize>,
        brush: &Brush,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
    ) -> Vec<Prim> {
        let s = self.scale;
        let mut out = vec![
            Prim::soft(
                self.rect.offset(0.0, SHADOW_OFFSET * s),
                RADIUS * s,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.rect.inset(-s), RADIUS * s + s, theme.border),
            Prim::rounded(self.rect, RADIUS * s, theme.panel),
        ];

        // The header says which shelf is on show, and offers the others.
        let title_w = (self.set.x - BUTTON_GAP * s - self.header.x).max(0.0);
        let baseline = atlas.baseline_in(self.header);
        for g in atlas.layout(&atlas.truncate(&set.name, title_w), self.header.x, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink).clipped(self.header));
        }
        out.extend(icon_prims(SETS, self.set, s, theme.icon));
        out.extend(icon_prims(RESET, self.reset, s, theme.icon));

        for row in &self.rows {
            let Some(preset) = set.presets.get(row.index) else {
                continue;
            };
            if selected == Some(row.index) {
                out.push(
                    Prim::rounded(
                        row.rect.inset(CARD_INSET * s),
                        ROW_RADIUS * s,
                        theme.active_bg,
                    )
                    .clipped(self.band),
                );
            }
            let (radius, feather) = dab(&preset.brush);
            let nominal = radius + feather / 2.0;
            let (cx, cy) = row.swatch.center();
            let reach = (row.swatch.w / 2.0 - nominal * s).max(0.0);
            out.push(
                Prim::soft_segment(
                    (cx - reach, cy),
                    (cx + reach, cy),
                    radius * s,
                    feather * s,
                    with_alpha(theme.ink, preset.brush.opacity as f32),
                )
                .clipped(self.band),
            );
            let baseline = atlas.baseline_in(row.rect);
            let name = atlas.truncate(&preset.name, row.name.w);
            for g in atlas.layout(&name, row.name.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink).clipped(self.band));
            }
        }

        // The rail: a track each, filled to where the brush sits, with a
        // knob on the mark.
        for slider in &self.sliders {
            let t = slider.track;
            out.push(Prim::rounded(t, t.w / 2.0, theme.border));
            let f = slider.property.fraction(brush) as f32;
            let filled = ScreenRect {
                x: t.x,
                y: t.y + (1.0 - f) * t.h,
                w: t.w,
                h: f * t.h,
            };
            out.push(Prim::rounded(filled, t.w / 2.0, theme.icon_active));
            let side = KNOB * s;
            let knob = ScreenRect {
                x: t.x + (t.w - side) / 2.0,
                y: t.y + (1.0 - f) * t.h - side / 2.0,
                w: side,
                h: side,
            };
            out.push(Prim::rounded(knob.inset(-s), side / 2.0 + s, theme.border));
            out.push(Prim::rounded(knob, side / 2.0, theme.panel));
        }
        out
    }
}

/// The half-width and edge ramp a brush's dab is previewed with, in
/// logical px. Size is taken by its square root so a fine brush is still
/// visible beside a fat one; the ramp is the stroke's own, so a soft
/// brush previews soft.
fn dab(brush: &Brush) -> (f32, f32) {
    let t = ((brush.size / crate::brush::SIZE_MAX) as f32).sqrt();
    let r = DAB_MIN + t * (DAB_MAX - DAB_MIN);
    let feather = (1.0 - brush.hardness.clamp(0.0, 1.0)) as f32 * r;
    (r - feather / 2.0, feather)
}

/// Icons on a 24×24 grid, drawn as polylines like the dock's.
/// A stack of shelves: the library's other sets.
const SETS: &[&[(f32, f32)]] = &[
    &[(3.0, 7.0), (12.0, 3.0), (21.0, 7.0), (12.0, 11.0), (3.0, 7.0)],
    &[(3.0, 12.0), (12.0, 16.0), (21.0, 12.0)],
    &[(3.0, 17.0), (12.0, 21.0), (21.0, 17.0)],
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
    use crate::scene::{KIND_SEGMENT, Viewport};
    use crate::text::{Atlas, Font};

    const VP: Viewport = Viewport { w: 900, h: 700 };
    /// Where the tab strip ends, in physical px.
    const TOP: f32 = 34.0;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), 13)
    }

    fn set() -> Set {
        Library::default().sets()[0].clone()
    }

    fn palette(vp: Viewport, scale: f64, scroll: f32) -> Palette {
        Palette::layout(vp, scale, TOP, &set(), scroll)
    }

    #[test]
    fn the_panel_stands_at_the_left_with_a_row_per_brush() {
        let p = palette(VP, 1.0, 0.0);
        assert_eq!(p.rect.x, MARGIN, "off the window's left edge");
        assert_eq!(p.rect.y, TOP + MARGIN, "under whatever is above it");
        assert_eq!(p.rect.w, WIDTH);
        assert_eq!(p.rows.len(), set().presets.len(), "one row per brush");
        for (i, row) in p.rows.iter().enumerate() {
            assert_eq!(row.index, i, "top of the set first");
            assert!(
                p.rect.contains_rect(&row.rect),
                "row {i} spills out of the panel"
            );
            assert!(row.rect.contains_rect(&row.swatch));
            assert_eq!(row.rect.h, ROW);
        }
        let (a, b) = (p.rows[0].rect, p.rows[1].rect);
        assert_eq!(b.y - a.y, ROW, "rows sit one under the next");
    }

    #[test]
    fn the_rail_stands_beside_the_rows_carrying_size_over_opacity() {
        let p = palette(VP, 1.0, 0.0);
        let [size, opacity] = p.sliders;
        assert_eq!(size.property, Property::Size);
        assert_eq!(opacity.property, Property::Opacity);
        assert!(size.track.y < opacity.track.y, "size is the top handle");
        for s in p.sliders {
            assert!(p.rect.contains_rect(&s.track), "{:?} is outside", s.property);
            assert!(s.track.h > s.track.w, "a slider standing on end");
        }
        let gap = opacity.track.y - (size.track.y + size.track.h);
        assert!(
            gap >= KNOB * 2.0,
            "two knobs that meet at the join read as one: {gap}"
        );
        for row in &p.rows {
            assert!(
                row.rect.x + row.rect.w <= p.sliders[0].track.x,
                "the rows stop before the rail"
            );
        }
    }

    #[test]
    fn layout_scales_with_the_display() {
        let p = palette(VP, 2.0, 0.0);
        assert_eq!(p.rect.x, MARGIN * 2.0);
        assert_eq!(p.rect.w, WIDTH * 2.0);
        assert_eq!(p.rows[0].rect.h, ROW * 2.0);
    }

    #[test]
    fn hit_reports_the_row_the_buttons_and_the_rail() {
        let p = palette(VP, 1.0, 0.0);
        let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));

        let (x, y) = mid(p.rows[2].rect);
        assert_eq!(p.hit(x, y), Some(Hit::Brush(2)));
        let (x, y) = mid(p.set);
        assert_eq!(p.hit(x, y), Some(Hit::Set));
        let (x, y) = mid(p.reset);
        assert_eq!(p.hit(x, y), Some(Hit::Reset));
        let (x, y) = mid(p.sliders[0].track);
        assert_eq!(p.hit(x, y), Some(Hit::Rail(Property::Size)));
        let (x, y) = mid(p.sliders[1].track);
        assert_eq!(p.hit(x, y), Some(Hit::Rail(Property::Opacity)));
        let (x, y) = mid(p.header);
        assert_eq!(p.hit(x, y), Some(Hit::Panel), "the title swallows a click");
        assert_eq!(p.hit(2.0, 2.0), None, "outside is canvas");
    }

    #[test]
    fn a_rail_is_grabbed_from_anywhere_across_it() {
        let p = palette(VP, 1.0, 0.0);
        let t = p.sliders[0].track;
        // The track is a hairline; the whole width of the rail takes it.
        let x = f64::from(t.x + t.w / 2.0 + RAIL / 2.0 - t.w);
        let y = f64::from(t.y + t.h / 2.0);
        assert_eq!(p.hit(x, y), Some(Hit::Rail(Property::Size)));
    }

    #[test]
    fn a_track_standing_on_end_is_full_at_the_top() {
        let p = palette(VP, 1.0, 0.0);
        let t = p.sliders[0].track;
        assert_eq!(p.fraction(Property::Size, f64::from(t.y)), 1.0);
        assert_eq!(p.fraction(Property::Size, f64::from(t.y + t.h)), 0.0);
        let half = p.fraction(Property::Size, f64::from(t.y + t.h / 2.0));
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        assert_eq!(p.fraction(Property::Size, -500.0), 1.0, "past the top");
        assert_eq!(p.fraction(Property::Size, 5000.0), 0.0, "past the bottom");
    }

    #[test]
    fn a_short_window_cuts_the_set_and_the_scroll_walks_it() {
        let tall = palette(VP, 1.0, 0.0);
        assert_eq!(tall.max_scroll(), 0.0, "everything fits");

        let short = palette(Viewport { w: 900, h: 200 }, 1.0, 0.0);
        assert!(short.max_scroll() > 0.0, "the set is longer than the band");
        assert!(short.rows.len() < set().presets.len(), "laid out short");
        let top = short.rows[0].rect.y;

        let scrolled = palette(Viewport { w: 900, h: 200 }, 1.0, ROW);
        assert_eq!(scrolled.scroll(), ROW);
        assert_eq!(scrolled.rows[0].index, 1, "the first row walked off");
        assert_eq!(scrolled.rows[0].rect.y, top, "and the next took its place");

        let past = palette(Viewport { w: 900, h: 200 }, 1.0, 9999.0);
        assert_eq!(past.scroll(), past.max_scroll(), "and it stops at the end");
    }

    #[test]
    fn prims_paint_the_panel_the_rows_and_the_brush_in_hand() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = palette(VP, 1.0, 0.0);
        let s = set();
        let prims = p.prims(&s, Some(2), &s.presets[2].brush, &atlas, 7, &theme);

        assert!(prims[0].feather > 0.0, "the soft shadow goes first");
        assert!(
            prims.iter().any(|q| q.color == theme.panel && q.bounds() == p.rect),
            "panel body"
        );
        let filled: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.color == theme.active_bg)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(filled.len(), 1, "only the brush in hand is filled");
        assert!(p.rows[2].rect.contains_rect(&filled[0]));
    }

    #[test]
    fn a_palette_showing_another_shelf_fills_no_row() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = palette(VP, 1.0, 0.0);
        let s = set();
        let prims = p.prims(&s, None, &Brush::default(), &atlas, 7, &theme);
        assert!(
            !prims.iter().any(|q| q.color == theme.active_bg),
            "the brush in hand is off this shelf"
        );
    }

    #[test]
    fn a_row_previews_the_brush_it_names_with_that_brush() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = palette(VP, 1.0, 0.0);
        let s = set();
        let prims = p.prims(&s, Some(0), &s.presets[0].brush, &atlas, 7, &theme);

        let dab_of = |i: usize| {
            let sw = p.rows[i].swatch;
            prims
                .iter()
                .find(|q| q.kind == KIND_SEGMENT && sw.contains_rect(&q.bounds()))
                .unwrap_or_else(|| panic!("row {i} has no dab"))
        };
        let names: Vec<&str> = s.presets.iter().map(|q| q.name.as_str()).collect();
        let pencil = names.iter().position(|n| *n == "Pencil").unwrap();
        let airbrush = names.iter().position(|n| *n == "Airbrush").unwrap();
        let hard = names.iter().position(|n| *n == "Hard Round").unwrap();

        assert!(
            dab_of(pencil).radius < dab_of(airbrush).radius,
            "a fatter brush previews fatter"
        );
        assert_eq!(dab_of(hard).feather, 0.0, "a crisp brush previews crisp");
        assert!(dab_of(airbrush).feather > 0.0, "a soft one previews soft");
        assert!(
            dab_of(airbrush).color[3] < dab_of(hard).color[3],
            "a translucent brush previews translucent"
        );
    }

    #[test]
    fn nothing_the_panel_draws_escapes_it() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = palette(Viewport { w: 900, h: 220 }, 1.0, 12.0);
        let s = set();
        let prims = p.prims(&s, Some(1), &s.presets[1].brush, &atlas, 7, &theme);
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
