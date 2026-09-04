//! The brush strip: the narrow panel the brush tool stands up on the
//! left. It says which brush is in the hand — its own icon, its name,
//! the shelf it came off and a dab of what it actually lays — and under
//! that keeps ten within reach.
//!
//! Nine of the seats ship filled and are numbered `1`–`9`; the tenth is
//! slot `0`, which nobody fills. It follows the hand instead, holding
//! whatever was last reached for that none of the nine already keeps —
//! so a brush taken off a far shelf is one key away for as long as it is
//! wanted, and costs nothing when it is not.
//!
//! Two buttons sit between the name and the seats: the sliders open
//! Brush Properties, the chevron opens the library beside the strip.
//! Pure — `app` asks where a click landed and what to draw.

use crate::brush::{Brush, SLOTS, Set};
use crate::palette::icon_uv;
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport, with_alpha};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
/// Wide enough for the icon and a brush's name beside it.
pub const WIDTH: f32 = 132.0;
/// From the window's left edge, and from whatever is above the strip.
pub const MARGIN: f32 = 12.0;
pub const PADDING: f32 = 8.0;
pub const RADIUS: f32 = 12.0;
/// The held brush's own icon, at the head of the strip.
const HEADER_ICON: f32 = 34.0;
const LABEL_GAP: f32 = 8.0;
/// The band the dab of what the brush lays runs across.
const DAB_ROW: f32 = 10.0;
const ROW_GAP: f32 = 6.0;
/// The two buttons are this square.
const BUTTON: f32 = 22.0;
/// One seat, and the icon centered in it.
const CELL: f32 = 32.0;
const SLOT_ICON: f32 = 26.0;
const CELL_RADIUS: f32 = 6.0;
const CELL_INSET: f32 = 1.0;
/// How thick the ring around the brush in the hand is.
const HELD_RING: f32 = 1.5;
/// The seat's number, in from its bottom-right corner.
const NUMBER_INSET: f32 = 3.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
const ICON_BOX: f32 = 15.0;
const ICON_STROKE: f32 = 1.5;
/// The dab in the header: a stroke of what the brush lays, this wide at
/// the very most.
const DAB_MAX: f32 = 8.0;
const DAB_MIN: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A seat, by the number written on it — `0` for the overflow.
    Slot(usize),
    /// The sliders: Brush Properties opens or folds away.
    Properties,
    /// The chevron: the library opens or shuts beside the strip.
    Library,
    /// Strip chrome: swallowed, never reaches the canvas.
    Panel,
}

/// One seat, and the number written on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seat {
    pub n: usize,
    pub rect: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub rect: ScreenRect,
    /// The block at the top: the name, the dab and the two buttons.
    pub header: ScreenRect,
    pub properties: ScreenRect,
    pub library: ScreenRect,
    /// As much of the column of seats as the window has room for.
    pub band: ScreenRect,
    pub seats: Vec<Seat>,
    scale: f32,
}

impl Strip {
    /// `top` is where whatever stands above the strip ends, in physical
    /// px: the properties bar, or the tab strip.
    pub fn layout(viewport: Viewport, scale: f64, top: f32) -> Strip {
        let s = scale as f32;
        let x = (MARGIN * s).round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;

        let header = ScreenRect {
            x: inner_x,
            y: y + PADDING * s,
            w: inner_w,
            h: (HEADER_ICON + ROW_GAP + DAB_ROW + ROW_GAP + BUTTON) * s,
        };
        let side = BUTTON * s;
        let buttons_y = header.y + header.h - side;
        let properties = ScreenRect {
            x: header.x,
            y: buttons_y,
            w: side,
            h: side,
        };
        let library = ScreenRect {
            x: header.x + header.w - side,
            ..properties
        };

        // The seats fall under the header; a window too short for all
        // ten cuts the column and never the header, which is what says
        // what is in the hand.
        let column_y = header.y + header.h + ROW_GAP * s;
        let content = SLOTS as f32 * CELL * s;
        let room = (viewport.h as f32 - (MARGIN + PADDING) * s - column_y).max(0.0);
        let band = ScreenRect {
            x: inner_x,
            y: column_y,
            w: inner_w,
            h: content.min(room),
        };

        // Numbered 1..=9 down the column, and then slot 0 at its foot:
        // the overflow is the one nobody put there, so it stands apart.
        let seats = (0..SLOTS)
            .map(|i| Seat {
                n: (i + 1) % SLOTS,
                rect: ScreenRect {
                    x: inner_x + (inner_w - CELL * s) / 2.0,
                    y: column_y + i as f32 * CELL * s,
                    w: CELL * s,
                    h: CELL * s,
                },
            })
            .collect();

        Strip {
            rect: ScreenRect {
                x,
                y,
                w: WIDTH * s,
                h: header.h + ROW_GAP * s + band.h + 2.0 * PADDING * s,
            },
            header,
            properties,
            library,
            band,
            seats,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.properties.contains(x, y) {
            return Some(Hit::Properties);
        }
        if self.library.contains(x, y) {
            return Some(Hit::Library);
        }
        let seat = self
            .seats
            .iter()
            .find(|q| q.rect.contains(x, y) && self.band.contains(x, y));
        Some(match seat {
            Some(q) => Hit::Slot(q.n),
            None => Hit::Panel,
        })
    }

    /// `slots` is what each seat holds, `selected` the brush in the hand
    /// and `brush` its body, which the dab is drawn with. `over` is the
    /// seat a brush is being dragged onto, which stands out. `icons` is
    /// the slot the icon sheet was uploaded to.
    #[allow(clippy::too_many_arguments)]
    pub fn prims(
        &self,
        sets: &[Set],
        selected: (usize, usize),
        slots: &[Option<(usize, usize)>],
        brush: &Brush,
        over: Option<usize>,
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
        self.header_prims(sets, selected, brush, atlas, slot, icons, theme, &mut out);
        self.seat_prims(sets, selected, slots, over, atlas, slot, icons, theme, &mut out);
        out
    }

    /// The block at the top: the brush's own icon, its name, the shelf it
    /// came off, a dab of what it lays, and the two buttons.
    #[allow(clippy::too_many_arguments)]
    fn header_prims(
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
            .and_then(|q| q.presets.get(index).map(|p| (q, p)))
        else {
            return;
        };

        let side = HEADER_ICON * s;
        let icon_box = ScreenRect {
            x: self.header.x,
            y: self.header.y,
            w: side,
            h: side,
        };
        out.push(Prim::sprite(icon_box, icon_uv(preset.icon), icons));

        // The name on one line, the shelf under it in the muted color.
        let tx = icon_box.x + side + LABEL_GAP * s;
        let room = (self.header.x + self.header.w - tx).max(0.0);
        let line = side / 2.0;
        for (text, y, color) in [
            (preset.name.as_str(), self.header.y, theme.ink),
            (shelf.name.as_str(), self.header.y + line, theme.muted),
        ] {
            let row = ScreenRect {
                x: tx,
                y,
                w: room,
                h: line,
            };
            let baseline = atlas.baseline_in(row);
            for g in atlas.layout(&atlas.truncate(text, room), tx, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, color).clipped(self.header));
            }
        }

        // And what it actually lays. The icon above is Sketchbook's art
        // and may promise a mark the ink cannot make; this is the part
        // that cannot — the three the canvas answers to, drawn.
        let (radius, feather) = dab(brush);
        let nominal = (radius + feather / 2.0) * s;
        let cy = icon_box.y + side + ROW_GAP * s + DAB_ROW * s / 2.0;
        let (x0, x1) = (
            self.header.x + nominal,
            self.header.x + self.header.w - nominal,
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
                .clipped(self.header),
            );
        }

        out.extend(icon_prims(SLIDERS, self.properties, s, theme.icon));
        out.extend(icon_prims(CHEVRON, self.library, s, theme.icon));
    }

    /// The column: a seat's icon, its number, the ring on the one in the
    /// hand and the mark on the one a drag is over.
    #[allow(clippy::too_many_arguments)]
    fn seat_prims(
        &self,
        sets: &[Set],
        selected: (usize, usize),
        slots: &[Option<(usize, usize)>],
        over: Option<usize>,
        atlas: &Atlas,
        slot: u32,
        icons: u32,
        theme: &Theme,
        out: &mut Vec<Prim>,
    ) {
        let s = self.scale;
        let cell = theme.corner(CELL_RADIUS, s);
        for seat in &self.seats {
            let at = slots.get(seat.n).copied().flatten();
            let box_ = seat.rect.inset(CELL_INSET * s);

            // The brush in the hand wears a ring, which is a filled
            // rounded box with the panel's own color laid back inside it.
            if at.is_some() && at == Some(selected) {
                out.push(Prim::rounded(box_, cell, theme.selection).clipped(self.band));
                out.push(
                    Prim::rounded(box_.inset(HELD_RING * s), cell, theme.active_bg)
                        .clipped(self.band),
                );
            } else if over == Some(seat.n) {
                out.push(Prim::rounded(box_, cell, theme.active_bg).clipped(self.band));
            }

            if let Some((set, index)) = at
                && let Some(preset) = sets.get(set).and_then(|q| q.presets.get(index))
            {
                let side = SLOT_ICON * s;
                let art = ScreenRect {
                    x: seat.rect.x + (seat.rect.w - side) / 2.0,
                    y: seat.rect.y + (seat.rect.h - side) / 2.0,
                    w: side,
                    h: side,
                };
                out.push(Prim::sprite(art, icon_uv(preset.icon), icons).clipped(self.band));
            }

            // The number is read, never clicked, so it is drawn muted and
            // tucked into the corner the icon leaves.
            let digit = seat.n.to_string();
            let w = atlas.measure(&digit);
            let row = ScreenRect {
                x: seat.rect.x + seat.rect.w - NUMBER_INSET * s - w,
                y: seat.rect.y + seat.rect.h / 2.0,
                w,
                h: seat.rect.h / 2.0,
            };
            let baseline = atlas.baseline_in(row);
            for g in atlas.layout(&digit, row.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(self.band));
            }
        }
    }
}

/// The half-width and edge ramp the header's dab is drawn with, in
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
/// A chevron pointing right: the library is out that way.
const CHEVRON: &[&[(f32, f32)]] = &[&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]];

fn icon_prims(lines: &[&[(f32, f32)]], r: ScreenRect, s: f32, color: Rgba) -> Vec<Prim> {
    scene::icon_prims(lines, r, 24.0, ICON_BOX, ICON_STROKE, s, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::Library;
    use crate::tabs::Tabs;
    use crate::text::Font;

    /// Where the tab strip ends, in physical px.
    const TOP: f32 = 34.0;
    /// Tall enough that all ten seats are laid out at once.
    const TALL: Viewport = Viewport { w: 900, h: 900 };

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    fn strip(vp: Viewport, scale: f64) -> Strip {
        Strip::layout(vp, scale, TOP)
    }

    fn paint(s: &Strip, lib: &Library, held: (usize, usize), over: Option<usize>) -> Vec<Prim> {
        s.prims(
            lib.sets(),
            held,
            lib.slots(),
            lib.brush(),
            over,
            &atlas(),
            7,
            9,
            &Theme::light(),
        )
    }

    #[test]
    fn the_strip_stands_at_the_left_with_ten_seats_under_its_header() {
        let s = strip(TALL, 1.0);
        assert_eq!(s.rect.x, MARGIN, "off the window's left edge");
        assert_eq!(s.rect.y, TOP + MARGIN, "under whatever is above it");
        assert_eq!(s.rect.w, WIDTH);
        assert_eq!(s.seats.len(), SLOTS, "nine numbered and the overflow");
        assert_eq!(s.seats[0].n, 1, "the numbered nine come first");
        assert_eq!(s.seats[SLOTS - 1].n, 0, "and slot 0 stands at the foot");
        for pair in s.seats.windows(2) {
            assert!(pair[1].rect.y > pair[0].rect.y, "one under the next");
            assert_eq!(pair[1].rect.x, pair[0].rect.x, "in one column");
        }
        assert!(
            s.seats[0].rect.y >= s.header.y + s.header.h,
            "clear of the header"
        );
    }

    #[test]
    fn hit_reports_a_seat_the_two_buttons_and_the_panel() {
        let s = strip(TALL, 1.0);
        let mid = |r: ScreenRect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));

        let seat = s.seats[2];
        let (x, y) = mid(seat.rect);
        assert_eq!(s.hit(x, y), Some(Hit::Slot(seat.n)));
        let (x, y) = mid(s.properties);
        assert_eq!(s.hit(x, y), Some(Hit::Properties));
        let (x, y) = mid(s.library);
        assert_eq!(s.hit(x, y), Some(Hit::Library));
        let (x, y) = mid(s.header);
        assert_eq!(s.hit(x, y), Some(Hit::Panel), "the name is no button");
        assert_eq!(s.hit(2.0, 2.0), None, "outside is canvas");
    }

    #[test]
    fn layout_scales_with_the_display() {
        let s = strip(TALL, 2.0);
        assert_eq!(s.rect.x, MARGIN * 2.0);
        assert_eq!(s.rect.w, WIDTH * 2.0);
        assert_eq!(s.seats[0].rect.h, CELL * 2.0);
    }

    #[test]
    fn a_short_window_keeps_the_header_and_cuts_the_column() {
        let s = strip(Viewport { w: 900, h: 260 }, 1.0);
        assert!(s.rect.h <= 260.0 - TOP - MARGIN, "the strip fits the window");
        assert!(s.rect.contains_rect(&s.header), "the header is never cut");
        assert!(s.band.h < SLOTS as f32 * CELL, "and the column is the part cut");
    }

    #[test]
    fn prims_paint_the_panel_the_seats_and_the_brush_in_hand() {
        let theme = Theme::light();
        let lib = Library::default();
        let s = strip(TALL, 1.0);
        let held = lib.slots()[1].expect("the first seat ships filled");
        let prims = paint(&s, &lib, held, None);

        assert!(prims[0].feather > 0.0, "the soft shadow goes first");
        assert!(
            prims
                .iter()
                .any(|q| q.color == theme.panel && q.bounds() == s.rect),
            "panel body"
        );
        let ringed: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.color == theme.selection)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(ringed.len(), 1, "only the brush in hand is ringed");
        let seat = s.seats.iter().find(|q| q.n == 1).unwrap();
        assert!(seat.rect.contains_rect(&ringed[0]));

        let sprites = prims.iter().filter(|q| q.slot == 9).count();
        assert_eq!(
            sprites,
            SLOTS - 1 + 1,
            "a sprite per filled seat, and one more for the header"
        );
    }

    #[test]
    fn an_empty_seat_draws_its_number_and_no_icon() {
        let lib = Library::default();
        let s = strip(TALL, 1.0);
        let prims = paint(&s, &lib, lib.selected(), None);
        let zero = s.seats.iter().find(|q| q.n == 0).unwrap();
        assert!(
            !prims
                .iter()
                .any(|q| q.slot == 9 && zero.rect.contains_rect(&q.bounds())),
            "slot 0 is empty until something overflows into it"
        );
        assert!(
            prims
                .iter()
                .any(|q| q.slot == 7 && zero.rect.contains_rect(&q.bounds())),
            "but it is still numbered"
        );
    }

    #[test]
    fn the_seat_a_drag_is_over_stands_out() {
        let lib = Library::default();
        let s = strip(TALL, 1.0);
        let plain = paint(&s, &lib, lib.selected(), None);
        let over = paint(&s, &lib, lib.selected(), Some(4));
        assert_eq!(
            over.len(),
            plain.len() + 1,
            "the seat under the pointer is marked, and nothing else moves"
        );
    }

    #[test]
    fn the_dab_says_what_the_brush_actually_lays() {
        let lib = Library::default();
        let s = strip(TALL, 1.0);
        let prims = paint(&s, &lib, lib.selected(), None);
        let dab = prims
            .iter()
            .find(|q| {
                q.kind == scene::KIND_SEGMENT
                    && q.bounds().intersect(&s.properties).is_none()
                    && q.bounds().intersect(&s.library).is_none()
            })
            .expect("the header draws a dab");
        assert!(dab.radius > 0.0);
        assert!(
            (f64::from(dab.color[3]) - lib.brush().opacity).abs() < 1e-3,
            "drawn at the brush's own opacity, not the theme's ink"
        );
    }

    #[test]
    fn nothing_the_strip_draws_escapes_it() {
        let lib = Library::default();
        let s = strip(Viewport { w: 900, h: 300 }, 1.0);
        let prims = paint(&s, &lib, lib.selected(), None);
        let room = s.rect.inset(-2.0);
        for q in prims.iter().skip(1) {
            assert!(
                room.contains_rect(&q.bounds()) || q.clip != scene::NO_CLIP,
                "{:?} is loose outside the strip",
                q.bounds()
            );
        }
    }
}
