//! Tool dock: a floating rounded panel centered at the bottom of the
//! canvas, one button per tool and, after a divider, the inks a stroke
//! can be laid in. Sized in logical px so it reads the same on any
//! display; positioned in physical px. Pure — `app` asks where a click
//! landed and what to draw.
//!
//! The ink lives here and not on the brush's own bar because the pencil
//! lays it too: it is the window's, like the tool in the hand.

use crate::editor::Tool;
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport};
use crate::theme::Theme;

// Logical px.
pub const BUTTON: f32 = 36.0;
pub const GAP: f32 = 2.0;
pub const PADDING: f32 = 6.0;
pub const BOTTOM_MARGIN: f32 = 20.0;
pub const PANEL_RADIUS: f32 = 12.0;
pub const BUTTON_RADIUS: f32 = 8.0;
/// The 24-unit icon grid maps onto a box this big, centered in the button.
pub const ICON_BOX: f32 = 20.0;
pub const ICON_STROKE: f32 = 1.75;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
/// An ink's cell: narrower than a tool's button, because a dot needs
/// less room than an icon and there are more of them.
pub const INK_CELL: f32 = 24.0;
/// The dot drawn in it.
pub const INK_DOT: f32 = 15.0;
/// How much of the cell the ring around the chosen ink takes.
const INK_RING: f32 = 2.0;
/// The room the divider between the tools and the inks stands in.
const DIVIDER: f32 = 11.0;
const DIVIDER_PX: f32 = 1.0;
/// How much of the panel's height the divider is tall.
const DIVIDER_HEIGHT: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tool(Tool),
    /// One of the inks, by its place in the theme's own list.
    Ink(usize),
    /// Panel chrome between buttons: swallowed, never reaches the canvas.
    Panel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dock {
    pub panel: ScreenRect,
    pub buttons: Vec<(Tool, ScreenRect)>,
    /// One cell per ink, in the theme's own order.
    pub inks: Vec<ScreenRect>,
    scale: f32,
}

impl Dock {
    /// The dock for `tools` and `inks` many colours. No inks is a dock
    /// with no divider and nothing after it, exactly as it was before
    /// there were any.
    pub fn layout(viewport: Viewport, scale: f64, tools: &[Tool], inks: usize) -> Dock {
        let s = scale as f32;
        let n = tools.len() as f32;
        let k = inks as f32;
        let strip = if inks == 0 {
            0.0
        } else {
            DIVIDER + k * INK_CELL + (k - 1.0) * GAP
        };
        let w = (n * BUTTON + (n - 1.0).max(0.0) * GAP + strip + 2.0 * PADDING) * s;
        let h = (BUTTON + 2.0 * PADDING) * s;
        let panel = ScreenRect {
            x: ((viewport.w as f32 - w) / 2.0).round(),
            y: (viewport.h as f32 - BOTTOM_MARGIN * s - h).round(),
            w,
            h,
        };
        let buttons: Vec<(Tool, ScreenRect)> = tools
            .iter()
            .enumerate()
            .map(|(i, &tool)| {
                let r = ScreenRect {
                    x: panel.x + (PADDING + i as f32 * (BUTTON + GAP)) * s,
                    y: panel.y + PADDING * s,
                    w: BUTTON * s,
                    h: BUTTON * s,
                };
                (tool, r)
            })
            .collect();
        // The inks pick up where the tools left off, a divider's width
        // along, and stand centred on the taller buttons' line.
        let after = buttons.last().map_or(panel.x + PADDING * s, |(_, r)| r.x + r.w);
        let inks = (0..inks)
            .map(|i| ScreenRect {
                x: after + (DIVIDER + i as f32 * (INK_CELL + GAP)) * s,
                y: panel.y + (PADDING + (BUTTON - INK_CELL) / 2.0) * s,
                w: INK_CELL * s,
                h: INK_CELL * s,
            })
            .collect();
        Dock {
            panel,
            buttons,
            inks,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.panel.contains(x, y) {
            return None;
        }
        if let Some((tool, _)) = self.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Hit::Tool(*tool));
        }
        // An ink's dot is smaller than the room it stands in, and the
        // room is what is aimed at: the cell takes the click, and the
        // full height of the panel over it, so a swatch is as easy to
        // hit as a tool.
        let reach = |r: &ScreenRect| ScreenRect {
            y: self.panel.y,
            h: self.panel.h,
            ..*r
        };
        if let Some(i) = self.inks.iter().position(|r| reach(r).contains(x, y)) {
            return Some(Hit::Ink(i));
        }
        Some(Hit::Panel)
    }

    /// Paint order: shadow, border, panel, then per button the active
    /// highlight and the icon.
    pub fn prims(&self, active: Tool, ink: usize, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let mut out = vec![
            Prim::soft(
                self.panel.offset(0.0, SHADOW_OFFSET * s),
                PANEL_RADIUS * s,
                SHADOW_FEATHER * s,
                theme.shadow,
            ),
            Prim::rounded(self.panel.inset(-s), PANEL_RADIUS * s + s, theme.border),
            Prim::rounded(self.panel, PANEL_RADIUS * s, theme.panel),
        ];
        for (tool, rect) in &self.buttons {
            let is_active = *tool == active;
            if is_active {
                out.push(Prim::rounded(*rect, BUTTON_RADIUS * s, theme.active_bg));
            }
            let color = if is_active {
                theme.icon_active
            } else {
                theme.icon
            };
            out.extend(icon_prims(*tool, *rect, s, color));
        }
        if let Some(first) = self.inks.first() {
            // The divider stands halfway between the last tool and the
            // first ink, a hairline of the panel's own border colour.
            let x = (first.x - DIVIDER * s / 2.0).round();
            let tall = self.panel.h * DIVIDER_HEIGHT;
            out.push(Prim::rect(
                ScreenRect {
                    x,
                    y: self.panel.y + (self.panel.h - tall) / 2.0,
                    w: (DIVIDER_PX * s).max(1.0),
                    h: tall,
                },
                theme.border,
            ));
        }
        for (i, cell) in self.inks.iter().enumerate() {
            let color = theme
                .inks
                .get(i)
                .map_or(theme.ink, |hex| scene::parse_color(hex));
            let (cx, cy) = cell.center();
            // The chosen ink wears a ring, as the brush in the hand does
            // in the palette: a filled circle with the panel laid back
            // inside it, so the dot itself is never made smaller.
            if i == ink {
                out.push(Prim::circle(cx, cy, INK_CELL * s / 2.0, theme.selection));
                out.push(Prim::circle(
                    cx,
                    cy,
                    (INK_CELL / 2.0 - INK_RING) * s,
                    theme.panel,
                ));
            }
            out.push(Prim::circle(cx, cy, INK_DOT * s / 2.0, color));
        }
        out
    }
}

/// Maps a tool's icon from the 24-unit grid into its button.
fn icon_prims(tool: Tool, button: ScreenRect, scale: f32, color: Rgba) -> Vec<Prim> {
    scene::icon_prims(icon(tool), button, 24.0, ICON_BOX, ICON_STROKE, scale, color)
}

/// Icons as polylines on a 24×24 grid (Lucide-style coordinates), drawn
/// with round caps and joins; a closed outline repeats its first point.
fn icon(tool: Tool) -> &'static [&'static [(f32, f32)]] {
    match tool {
        // Arrow pointer outline.
        Tool::Select => &[&[
            (4.5, 4.5),
            (20.0, 11.0),
            (13.8, 12.6),
            (12.6, 13.8),
            (11.0, 20.0),
            (4.5, 4.5),
        ]],
        // Pencil body (a rotated rounded box with a tip) plus the ferrule.
        Tool::Pencil => &[
            &[
                (18.0, 3.5),
                (20.5, 6.0),
                (7.0, 19.5),
                (2.5, 21.5),
                (4.5, 17.0),
                (18.0, 3.5),
            ],
            &[(15.5, 6.0), (18.0, 8.5)],
        ],
        // Open palm: U-shaped palm, three fingers and a thumb.
        Tool::Hand => &[
            &[
                (6.0, 11.0),
                (6.0, 16.0),
                (9.5, 21.0),
                (15.5, 21.0),
                (19.0, 16.0),
                (19.0, 11.0),
            ],
            &[(8.0, 11.0), (8.0, 5.0)],
            &[(12.5, 11.0), (12.5, 3.0)],
            &[(17.0, 11.0), (17.0, 5.0)],
            &[(6.0, 13.0), (3.0, 9.0)],
        ],
        // Paintbrush: a slanted handle and the head it runs into.
        Tool::Brush => &[
            &[
                (18.5, 2.5),
                (21.5, 5.5),
                (13.0, 14.0),
                (10.0, 11.0),
                (18.5, 2.5),
            ],
            &[
                (10.0, 11.0),
                (13.0, 14.0),
                (11.5, 18.5),
                (8.0, 21.0),
                (4.0, 21.0),
                (3.0, 18.0),
                (5.5, 15.0),
                (10.0, 11.0),
            ],
        ],
        // Frame: a square area, and the two marks that say it crops
        // rather than draws.
        Tool::Frame => &[
            &[
                (5.0, 5.0),
                (19.0, 5.0),
                (19.0, 19.0),
                (5.0, 19.0),
                (5.0, 5.0),
            ],
            &[(9.5, 2.0), (9.5, 22.0)],
            &[(2.0, 9.5), (22.0, 9.5)],
        ],
        // Magnifier: 12-gon lens, handle, plus sign.
        Tool::Zoom => &[
            &[
                (17.0, 10.5),
                (16.13, 13.75),
                (13.75, 16.13),
                (10.5, 17.0),
                (7.25, 16.13),
                (4.87, 13.75),
                (4.0, 10.5),
                (4.87, 7.25),
                (7.25, 4.87),
                (10.5, 4.0),
                (13.75, 4.87),
                (16.13, 7.25),
                (17.0, 10.5),
            ],
            &[(15.2, 15.2), (21.0, 21.0)],
            &[(8.0, 10.5), (13.0, 10.5)],
            &[(10.5, 8.0), (10.5, 13.0)],
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::KIND_SEGMENT;

    const VP: Viewport = Viewport { w: 800, h: 600 };
    const TOOLS: [Tool; 2] = [Tool::Select, Tool::Pencil];

    fn sr(x: f32, y: f32, w: f32, h: f32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    #[test]
    fn panel_is_centered_at_the_bottom_with_one_button_per_tool() {
        let d = Dock::layout(VP, 1.0, &TOOLS, 0);
        // 2 buttons of 36 + 1 gap of 2 + padding 6 on each side = 86 wide.
        assert_eq!(d.panel, sr(357.0, 532.0, 86.0, 48.0));
        assert_eq!(
            d.buttons,
            vec![
                (Tool::Select, sr(363.0, 538.0, 36.0, 36.0)),
                (Tool::Pencil, sr(401.0, 538.0, 36.0, 36.0)),
            ]
        );
    }

    #[test]
    fn layout_scales_with_the_display() {
        let d = Dock::layout(VP, 2.0, &TOOLS, 0);
        assert_eq!(d.panel, sr(314.0, 464.0, 172.0, 96.0));
        assert_eq!(d.buttons[1].1, sr(402.0, 476.0, 72.0, 72.0));
    }

    #[test]
    fn hit_reports_the_button_or_the_bare_panel() {
        let d = Dock::layout(VP, 1.0, &TOOLS, 0);
        assert_eq!(d.hit(381.0, 556.0), Some(Hit::Tool(Tool::Select)));
        assert_eq!(d.hit(419.0, 556.0), Some(Hit::Tool(Tool::Pencil)));
        assert_eq!(
            d.hit(359.0, 534.0),
            Some(Hit::Panel),
            "padding swallows the click"
        );
        assert_eq!(d.hit(100.0, 100.0), None);
        assert_eq!(d.hit(400.0, 531.0), None, "just above the panel is canvas");
    }

    #[test]
    fn the_inks_stand_after_the_tools_with_a_divider_between() {
        let bare = Dock::layout(VP, 1.0, &TOOLS, 0);
        let d = Dock::layout(VP, 1.0, &TOOLS, 3);
        assert!(bare.inks.is_empty(), "a dock with no inks has no strip");
        assert_eq!(d.inks.len(), 3);
        // 3 cells of 24 and 2 gaps of 2, a divider's 11 before them.
        assert_eq!(d.panel.w, bare.panel.w + 11.0 + 3.0 * 24.0 + 2.0 * 2.0);
        assert_eq!(d.panel.h, bare.panel.h, "and stands no taller");
        let last = d.buttons.last().unwrap().1;
        assert_eq!(d.inks[0].x, last.x + last.w + 11.0);
        assert_eq!(d.inks[1].x, d.inks[0].x + 24.0 + 2.0);
        // A cell is centred on the taller buttons' line.
        assert_eq!(d.inks[0].h, 24.0);
        assert_eq!(d.inks[0].y + d.inks[0].h / 2.0, last.y + last.h / 2.0);
    }

    #[test]
    fn a_click_anywhere_over_an_ink_is_that_ink() {
        let d = Dock::layout(VP, 1.0, &TOOLS, 3);
        let cell = d.inks[1];
        let (cx, cy) = cell.center();
        assert_eq!(d.hit(f64::from(cx), f64::from(cy)), Some(Hit::Ink(1)));
        // The dot is smaller than its cell, and the cell smaller than
        // the panel: a swatch is as easy to hit as a tool.
        assert_eq!(
            d.hit(f64::from(cx), f64::from(d.panel.y) + 1.0),
            Some(Hit::Ink(1)),
            "the panel's full height over a cell aims at it"
        );
        assert_eq!(
            d.hit(f64::from(cell.x) - 1.0, f64::from(cy)),
            Some(Hit::Panel),
            "the gap between two of them swallows the click"
        );
        assert_eq!(
            d.hit(f64::from(cx), f64::from(d.panel.y) - 1.0),
            None,
            "and above the panel is still canvas"
        );
    }

    #[test]
    fn the_chosen_ink_wears_a_ring_and_every_dot_is_its_own_colour() {
        let theme = Theme::light();
        let d = Dock::layout(VP, 1.0, &TOOLS, theme.inks.len());
        let prims = d.prims(Tool::Pencil, 2, &theme);
        for (i, cell) in d.inks.iter().enumerate() {
            let (cx, cy) = cell.center();
            let want = scene::parse_color(&theme.inks[i]);
            assert!(
                prims
                    .iter()
                    .any(|p| p.color == want && p.bounds().center() == (cx, cy)),
                "ink {i} is not drawn in its own colour"
            );
        }
        let (cx, cy) = d.inks[2].center();
        assert!(
            prims
                .iter()
                .any(|p| p.color == theme.selection && p.bounds().center() == (cx, cy)),
            "the chosen one is ringed"
        );
        let (cx, cy) = d.inks[0].center();
        assert!(
            !prims
                .iter()
                .any(|p| p.color == theme.selection && p.bounds().center() == (cx, cy)),
            "and only it"
        );
        // One hairline between the last tool and the first ink.
        let last = d.buttons.last().unwrap().1;
        assert!(
            prims.iter().any(|p| p.color == theme.border
                && p.geom[2] <= 1.0
                && p.geom[0] > last.x + last.w
                && p.geom[0] < d.inks[0].x),
            "the divider"
        );
    }

    #[test]
    fn prims_paint_shadow_panel_highlight_and_icons_in_place() {
        let theme = Theme::light();
        let d = Dock::layout(VP, 1.0, &TOOLS, 0);
        let prims = d.prims(Tool::Pencil, 0, &theme);

        assert!(prims[0].feather > 0.0, "soft shadow goes first");
        assert!(
            prims
                .iter()
                .any(|p| p.color == theme.panel && p.bounds() == d.panel),
            "panel body"
        );
        let highlights: Vec<ScreenRect> = prims
            .iter()
            .filter(|p| p.color == theme.active_bg)
            .map(|p| p.bounds())
            .collect();
        assert_eq!(
            highlights,
            vec![d.buttons[1].1],
            "only the active tool is highlighted"
        );

        let icons: Vec<&Prim> = prims.iter().filter(|p| p.kind == KIND_SEGMENT).collect();
        assert!(!icons.is_empty());
        for p in icons {
            let b = p.bounds();
            let owner = d
                .buttons
                .iter()
                .find(|(_, r)| r.contains_rect(&b))
                .unwrap_or_else(|| panic!("icon segment {b:?} spills out of its button"));
            let expected = if owner.0 == Tool::Pencil {
                theme.icon_active
            } else {
                theme.icon
            };
            assert_eq!(p.color, expected, "icon color follows the active state");
        }
    }

    #[test]
    fn every_tool_has_an_icon_on_the_24_grid() {
        for t in Tool::ALL {
            let strokes = icon(t);
            assert!(!strokes.is_empty(), "{t:?}");
            for line in strokes {
                assert!(line.len() >= 2, "{t:?}");
                for &(x, y) in *line {
                    assert!(
                        (0.0..=24.0).contains(&x) && (0.0..=24.0).contains(&y),
                        "{t:?}"
                    );
                }
            }
        }
    }
}
