//! Tool dock: a floating rounded panel centered at the bottom of the
//! canvas, one button per tool. Sized in logical px so it reads the same on
//! any display; positioned in physical px. Pure — `app` asks where a click
//! landed and what to draw.

use crate::editor::Tool;
use crate::scene::{Prim, Rgba, ScreenRect, Viewport, polyline_prims};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tool(Tool),
    /// Panel chrome between buttons: swallowed, never reaches the canvas.
    Panel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dock {
    pub panel: ScreenRect,
    pub buttons: Vec<(Tool, ScreenRect)>,
    scale: f32,
}

impl Dock {
    pub fn layout(viewport: Viewport, scale: f64, tools: &[Tool]) -> Dock {
        let s = scale as f32;
        let n = tools.len() as f32;
        let w = (n * BUTTON + (n - 1.0).max(0.0) * GAP + 2.0 * PADDING) * s;
        let h = (BUTTON + 2.0 * PADDING) * s;
        let panel = ScreenRect {
            x: ((viewport.w as f32 - w) / 2.0).round(),
            y: (viewport.h as f32 - BOTTOM_MARGIN * s - h).round(),
            w,
            h,
        };
        let buttons = tools
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
        Dock {
            panel,
            buttons,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.panel.contains(x, y) {
            return None;
        }
        let button = self.buttons.iter().find(|(_, r)| r.contains(x, y));
        Some(match button {
            Some((tool, _)) => Hit::Tool(*tool),
            None => Hit::Panel,
        })
    }

    /// Paint order: shadow, border, panel, then per button the active
    /// highlight and the icon.
    pub fn prims(&self, active: Tool, theme: &Theme) -> Vec<Prim> {
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
        out
    }
}

/// Maps a tool's icon from the 24-unit grid into its button.
fn icon_prims(tool: Tool, button: ScreenRect, scale: f32, color: Rgba) -> Vec<Prim> {
    let (cx, cy) = button.center();
    let unit = ICON_BOX / 24.0 * scale;
    let half_width = ICON_STROKE / 2.0 * scale;
    icon(tool)
        .iter()
        .flat_map(|line| {
            let points: Vec<(f32, f32)> = line
                .iter()
                .map(|&(x, y)| (cx + (x - 12.0) * unit, cy + (y - 12.0) * unit))
                .collect();
            polyline_prims(&points, half_width, color)
        })
        .collect()
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
        let d = Dock::layout(VP, 1.0, &TOOLS);
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
        let d = Dock::layout(VP, 2.0, &TOOLS);
        assert_eq!(d.panel, sr(314.0, 464.0, 172.0, 96.0));
        assert_eq!(d.buttons[1].1, sr(402.0, 476.0, 72.0, 72.0));
    }

    #[test]
    fn hit_reports_the_button_or_the_bare_panel() {
        let d = Dock::layout(VP, 1.0, &TOOLS);
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
    fn prims_paint_shadow_panel_highlight_and_icons_in_place() {
        let theme = Theme::light();
        let d = Dock::layout(VP, 1.0, &TOOLS);
        let prims = d.prims(Tool::Pencil, &theme);

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
