//! Layers panel: the dock's chrome in a column on the right, one row per
//! layer, top layer first. Sized in logical px, positioned in physical
//! px, floating over the canvas and swallowing whatever it catches.
//! Pure — `app` asks where a click landed and what to draw.

use crate::doc::Layer;
use crate::scene::{Prim, ScreenRect, Viewport, icon_prims};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
pub const WIDTH: f32 = 200.0;
/// From the strip above and the window's right edge.
pub const MARGIN: f32 = 12.0;
pub const HEADER: f32 = 34.0;
pub const ROW: f32 = 30.0;
pub const PADDING: f32 = 6.0;
/// The header buttons and the eye are this square.
pub const BUTTON: f32 = 24.0;
pub const RADIUS: f32 = 12.0;
const ROW_RADIUS: f32 = 6.0;
const BUTTON_GAP: f32 = 2.0;
/// Between the eye and the name.
const EYE_GAP: f32 = 6.0;
/// The clickable area around the eye, past the box itself.
const EYE_SLOP: f32 = 2.0;
/// The 24-unit icon grid maps onto a box this big, centered in its button.
const ICON_BOX: f32 = 16.0;
const ICON_STROKE: f32 = 1.5;
const PUPIL: f32 = 2.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
const TITLE: &str = "Layers";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    /// Make this layer the active one. The index is into the document's
    /// layers, bottom to top.
    Select(usize),
    /// Show or hide this layer.
    Toggle(usize),
    Add,
    Remove,
    Up,
    Down,
    /// Panel chrome between controls: swallowed, never reaches the canvas.
    Panel,
}

/// One layer's row, with everything already measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Into the document's layers.
    pub index: usize,
    pub rect: ScreenRect,
    pub eye: ScreenRect,
    /// The name, cut down to what fits.
    pub label: String,
    /// Where the label's pen starts.
    pub label_x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub rect: ScreenRect,
    pub header: ScreenRect,
    /// Top layer first.
    pub rows: Vec<Row>,
    pub up: ScreenRect,
    pub down: ScreenRect,
    pub add: ScreenRect,
    pub remove: ScreenRect,
    scale: f32,
}

impl Panel {
    /// `top` is where the strip ends, in physical px. Rows are laid out
    /// top layer first, as many as fit above the bottom margin; the rest
    /// are not shown.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        top: f32,
        atlas: &Atlas,
        layers: &[Layer],
    ) -> Panel {
        let s = scale as f32;
        let x = (viewport.w as f32 - (MARGIN + WIDTH) * s).round();
        let y = (top + MARGIN * s).round();
        let inner_x = x + PADDING * s;
        let inner_w = (WIDTH - 2.0 * PADDING) * s;
        let header = ScreenRect {
            x: inner_x,
            y: y + PADDING * s,
            w: inner_w,
            h: HEADER * s,
        };

        // Buttons, right-aligned in the header: up, down, add, remove.
        let side = BUTTON * s;
        let by = header.y + (header.h - side) / 2.0;
        let mut bx = header.x + header.w - side;
        let mut button = || {
            let r = ScreenRect {
                x: bx,
                y: by,
                w: side,
                h: side,
            };
            bx -= side + BUTTON_GAP * s;
            r
        };
        let remove = button();
        let add = button();
        let down = button();
        let up = button();

        let limit = viewport.h as f32 - (MARGIN + PADDING) * s;
        let mut rows = Vec::new();
        let mut ry = header.y + header.h;
        for (index, layer) in layers.iter().enumerate().rev() {
            if ry + ROW * s > limit {
                break;
            }
            let rect = ScreenRect {
                x: inner_x,
                y: ry,
                w: inner_w,
                h: ROW * s,
            };
            let eye = ScreenRect {
                x: rect.x + PADDING * s,
                y: rect.y + (rect.h - side) / 2.0,
                w: side,
                h: side,
            };
            let label_x = (eye.x + eye.w + EYE_GAP * s).round();
            let room = rect.x + rect.w - PADDING * s - label_x;
            let label = if room > 0.0 {
                atlas.truncate(&layer.name, room)
            } else {
                String::new()
            };
            rows.push(Row {
                index,
                rect,
                eye,
                label,
                label_x,
            });
            ry += ROW * s;
        }

        let rect = ScreenRect {
            x,
            y,
            w: WIDTH * s,
            h: (2.0 * PADDING + HEADER) * s + rows.len() as f32 * ROW * s,
        };
        Panel {
            rect,
            header,
            rows,
            up,
            down,
            add,
            remove,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<PanelHit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let buttons = [
            (self.up, PanelHit::Up),
            (self.down, PanelHit::Down),
            (self.add, PanelHit::Add),
            (self.remove, PanelHit::Remove),
        ];
        if let Some((_, hit)) = buttons.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(*hit);
        }
        for row in &self.rows {
            if !row.rect.contains(x, y) {
                continue;
            }
            let on_eye = row.eye.inset(-EYE_SLOP * self.scale).contains(x, y);
            return Some(if on_eye {
                PanelHit::Toggle(row.index)
            } else {
                PanelHit::Select(row.index)
            });
        }
        Some(PanelHit::Panel)
    }

    /// Paint order: shadow, border, panel, the title and the buttons,
    /// then per row the active highlight, the eye and the name.
    pub fn prims(
        &self,
        layers: &[Layer],
        active: usize,
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
        let baseline = atlas.baseline_in(self.header);
        for g in atlas.layout(TITLE, self.header.x + PADDING * s, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        for (rect, icon) in [
            (self.up, UP),
            (self.down, DOWN),
            (self.add, PLUS),
            (self.remove, TRASH),
        ] {
            out.extend(icon_prims(icon, rect, 24.0, ICON_BOX, ICON_STROKE, s, theme.icon));
        }
        for row in &self.rows {
            let is_active = row.index == active;
            if is_active {
                out.push(Prim::rounded(row.rect, ROW_RADIUS * s, theme.active_bg));
            }
            let Some(layer) = layers.get(row.index) else {
                continue;
            };
            let (eye, color) = if layer.visible {
                (EYE, theme.icon)
            } else {
                (EYE_HIDDEN, theme.border)
            };
            out.extend(icon_prims(eye, row.eye, 24.0, ICON_BOX, ICON_STROKE, s, color));
            if layer.visible {
                let (cx, cy) = row.eye.center();
                out.push(Prim::circle(cx, cy, PUPIL / 24.0 * ICON_BOX * s, color));
            }
            if !row.label.is_empty() {
                let ink = if is_active { theme.ink } else { theme.icon };
                let baseline = atlas.baseline_in(row.rect);
                for g in atlas.layout(&row.label, row.label_x, baseline) {
                    out.push(Prim::glyph(g.rect, g.uv, slot, ink));
                }
            }
        }
        out
    }
}

// Icons as polylines on a 24×24 grid, like the dock's.
const UP: &[&[(f32, f32)]] = &[&[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)]];
const DOWN: &[&[(f32, f32)]] = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];
const PLUS: &[&[(f32, f32)]] = &[&[(12.0, 5.0), (12.0, 19.0)], &[(5.0, 12.0), (19.0, 12.0)]];
/// A bin: lid, handle, tapered body.
const TRASH: &[&[(f32, f32)]] = &[
    &[(4.0, 7.0), (20.0, 7.0)],
    &[(9.0, 7.0), (9.0, 4.0), (15.0, 4.0), (15.0, 7.0)],
    &[(6.0, 7.0), (7.0, 20.0), (17.0, 20.0), (18.0, 7.0)],
];
/// The almond of an open eye; the pupil is a circle drawn with it.
const ALMOND: &[(f32, f32)] = &[
    (2.0, 12.0),
    (5.0, 8.0),
    (9.0, 5.5),
    (12.0, 5.0),
    (15.0, 5.5),
    (19.0, 8.0),
    (22.0, 12.0),
    (19.0, 16.0),
    (15.0, 18.5),
    (12.0, 19.0),
    (9.0, 18.5),
    (5.0, 16.0),
    (2.0, 12.0),
];
const EYE: &[&[(f32, f32)]] = &[ALMOND];
/// The same almond, struck through.
const EYE_HIDDEN: &[&[(f32, f32)]] = &[ALMOND, &[(4.0, 20.0), (20.0, 4.0)]];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{KIND_BOX, KIND_IMAGE, KIND_SEGMENT};
    use crate::tabs::Tabs;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(1.0))
    }

    fn layers(n: usize) -> Vec<Layer> {
        (1..=n)
            .map(|i| Layer {
                id: format!("L{i}"),
                name: format!("Layer {i}"),
                visible: true,
            })
            .collect()
    }

    fn panel(viewport: Viewport, scale: f64, n: usize) -> Panel {
        Panel::layout(viewport, scale, 34.0 * scale as f32, &atlas(), &layers(n))
    }

    const VP: Viewport = Viewport { w: 1200, h: 800 };

    fn sr(x: f32, y: f32, w: f32, h: f32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    #[test]
    fn panel_sits_below_the_strip_at_the_right_edge() {
        let p = panel(VP, 1.0, 2);
        // MARGIN from the right edge and from the strip's bottom; padding,
        // the header, one row per layer, padding.
        assert_eq!(
            p.rect,
            sr(
                1200.0 - MARGIN - WIDTH,
                34.0 + MARGIN,
                WIDTH,
                2.0 * PADDING + HEADER + 2.0 * ROW
            )
        );
    }

    #[test]
    fn rows_list_the_top_layer_first() {
        let p = panel(VP, 1.0, 3);
        let indices: Vec<usize> = p.rows.iter().map(|r| r.index).collect();
        assert_eq!(indices, vec![2, 1, 0]);
        assert_eq!(p.rows[0].label, "Layer 3");
        assert_eq!(p.rows[0].rect.y, p.header.y + p.header.h);
        assert_eq!(p.rows[1].rect.y, p.rows[0].rect.y + ROW);
        assert_eq!(p.rows[0].rect.h, ROW);
        for r in &p.rows {
            assert!(r.rect.contains_rect(&r.eye), "the eye sits in its row");
            assert!(r.label_x > r.eye.x + r.eye.w, "the label follows the eye");
        }
    }

    #[test]
    fn rows_that_do_not_fit_are_dropped() {
        // Room for the header and one row above the bottom margin.
        let h = (34.0 + MARGIN + PADDING + HEADER + ROW + PADDING + MARGIN) as u32;
        let p = panel(Viewport { w: 1200, h }, 1.0, 3);
        assert_eq!(p.rows.len(), 1, "{:?}", p.rows);
        assert_eq!(p.rows[0].index, 2, "the top layer is what survives");
        assert_eq!(p.rect.h, 2.0 * PADDING + HEADER + ROW);
        // One pixel short, and the row goes too; the header stays.
        let p = panel(Viewport { w: 1200, h: h - 1 }, 1.0, 3);
        assert!(p.rows.is_empty());
        assert_eq!(p.rect.h, 2.0 * PADDING + HEADER);
    }

    #[test]
    fn layout_scales_with_the_display() {
        let p = panel(VP, 2.0, 2);
        assert_eq!(p.rect.w, 2.0 * WIDTH);
        assert_eq!(p.rect.x, 1200.0 - 2.0 * (MARGIN + WIDTH));
        assert_eq!(p.rows[0].rect.h, 2.0 * ROW);
        assert_eq!(p.add.w, 2.0 * BUTTON);
    }

    #[test]
    fn hit_names_the_row_the_eye_and_the_buttons() {
        let p = panel(VP, 1.0, 2);
        let mid = |r: ScreenRect| {
            let (x, y) = r.center();
            (f64::from(x), f64::from(y))
        };
        let (x, y) = mid(p.up);
        assert_eq!(p.hit(x, y), Some(PanelHit::Up));
        let (x, y) = mid(p.down);
        assert_eq!(p.hit(x, y), Some(PanelHit::Down));
        let (x, y) = mid(p.add);
        assert_eq!(p.hit(x, y), Some(PanelHit::Add));
        let (x, y) = mid(p.remove);
        assert_eq!(p.hit(x, y), Some(PanelHit::Remove));
        let (x, y) = mid(p.rows[0].eye);
        assert_eq!(p.hit(x, y), Some(PanelHit::Toggle(1)));
        let (x, y) = mid(p.rows[1].eye);
        assert_eq!(p.hit(x, y), Some(PanelHit::Toggle(0)));
        let (x, y) = mid(p.rows[1].rect);
        assert_eq!(p.hit(x, y), Some(PanelHit::Select(0)));
        let (_, y) = mid(p.header);
        let x = f64::from(p.header.x + 10.0);
        assert_eq!(p.hit(x, y), Some(PanelHit::Panel), "the title swallows the click");
        assert_eq!(p.hit(f64::from(p.rect.x) + 1.0, f64::from(p.rect.y) + 1.0), Some(PanelHit::Panel));
        assert_eq!(p.hit(100.0, 100.0), None);
        assert_eq!(p.hit(f64::from(p.rect.x) - 1.0, f64::from(p.rect.y) + 50.0), None);
    }

    #[test]
    fn prims_highlight_the_active_row_and_dim_a_hidden_layer() {
        let theme = Theme::light();
        let a = atlas();
        let mut ls = layers(3);
        ls[0].visible = false;
        let p = Panel::layout(VP, 1.0, 34.0, &a, &ls);
        let prims = p.prims(&ls, 1, &a, 7, &theme);

        assert!(prims[0].feather > 0.0, "soft shadow goes first");
        assert!(
            prims
                .iter()
                .any(|q| q.color == theme.panel && q.bounds() == p.rect),
            "panel body"
        );
        let highlights: Vec<ScreenRect> = prims
            .iter()
            .filter(|q| q.kind == KIND_BOX && q.color == theme.active_bg)
            .map(|q| q.bounds())
            .collect();
        assert_eq!(highlights.len(), 1, "one active row");
        assert!(p.rows[1].rect.contains_rect(&highlights[0]), "on layer 1's row");

        // Labels are glyphs from the atlas slot; the title and every row
        // that fits have some.
        let glyphs = prims.iter().filter(|q| q.kind == KIND_IMAGE && q.slot == 7);
        assert!(glyphs.count() >= 4 + 3 * 5, "'Layers' and three 'Layer N'");

        // The hidden layer's eye is drawn dimmed, the others in the icon
        // color; every eye stays inside its box.
        for row in &p.rows {
            let expected = if ls[row.index].visible {
                theme.icon
            } else {
                theme.border
            };
            let eye: Vec<&Prim> = prims
                .iter()
                .filter(|q| q.kind == KIND_SEGMENT && row.eye.contains_rect(&q.bounds()))
                .collect();
            assert!(!eye.is_empty(), "row {} has an eye", row.index);
            assert!(
                eye.iter().all(|q| q.color == expected),
                "row {}'s eye color",
                row.index
            );
        }
        // Buttons draw as strokes inside their boxes.
        for b in [p.up, p.down, p.add, p.remove] {
            assert!(
                prims
                    .iter()
                    .any(|q| q.kind == KIND_SEGMENT && b.contains_rect(&q.bounds())),
                "{b:?} has an icon"
            );
        }
    }
}
