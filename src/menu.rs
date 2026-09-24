//! A menu: a column of items standing over whatever is under it, opened
//! beside what asked for it — the layers' blend modes, a row's commands.
//! Sized in logical px, positioned in physical px, kept inside the
//! window, and a scroll area when the window is too short for it. Pure —
//! `app` asks what is under the pointer and what to draw.

use crate::scene::{Prim, Rgba, ScreenRect, Viewport, icon_prims};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
pub const ITEM: f32 = 24.0;
/// A rule between two runs of items: this tall, its line in the middle.
const RULE: f32 = 7.0;
const PADDING: f32 = 6.0;
/// The room for a check before a label, and for a colour's dot.
const MARK: f32 = 18.0;
const DOT: f32 = 5.0;
const RADIUS: f32 = 8.0;
const GAP: f32 = 4.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;
const ICON_STROKE: f32 = 1.5;
const MIN_W: f32 = 120.0;

/// One line of a menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub label: String,
    /// Offered: a line that cannot be taken is written muted and does
    /// not answer the pointer.
    pub enabled: bool,
    /// What is in force now: a check before the label.
    pub checked: bool,
    /// A rule stands above it, starting a run of its own.
    pub rule: bool,
    /// A colour it is about, as a dot before the label.
    pub dot: Option<Rgba>,
}

impl Item {
    pub fn new(label: &str) -> Item {
        Item {
            label: label.to_owned(),
            enabled: true,
            checked: false,
            rule: false,
            dot: None,
        }
    }

    pub fn checked(self, checked: bool) -> Item {
        Item { checked, ..self }
    }

    pub fn ruled(self) -> Item {
        Item { rule: true, ..self }
    }
}

/// A menu laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub rect: ScreenRect,
    /// Every item's row that the menu reaches, and which item it is.
    pub rows: Vec<(ScreenRect, usize)>,
    rules: Vec<f32>,
    scroll: f32,
    content: f32,
    scale: f32,
}

impl Menu {
    /// Laid out under `at` — what opened it — and inside the window: over
    /// it when there is no room under it, and cut to a scroll area when
    /// there is not room for it anywhere. `scroll` is how far down its
    /// items it is looking, in physical px.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        at: ScreenRect,
        atlas: &Atlas,
        items: &[Item],
        scroll: f32,
    ) -> Menu {
        let s = scale as f32;
        let widest = items.iter().map(|i| atlas.measure(&i.label)).fold(0.0, f32::max);
        let w = (widest + (2.0 * PADDING + MARK + PADDING) * s).max(MIN_W * s);
        let content: f32 = items
            .iter()
            .map(|i| if i.rule { (ITEM + RULE) * s } else { ITEM * s })
            .sum();
        let margin = PADDING * s;
        let room = (viewport.h as f32 - 2.0 * margin).max(0.0);
        let h = (content + 2.0 * PADDING * s).min(room);
        let below = at.y + at.h + GAP * s;
        let y = if below + h <= viewport.h as f32 - margin {
            below
        } else if at.y - GAP * s - h >= margin {
            at.y - GAP * s - h
        } else {
            (viewport.h as f32 - margin - h).max(margin)
        };
        let x = at.x.min(viewport.w as f32 - margin - w).max(margin);
        let rect = ScreenRect { x, y, w, h };
        let band = h - 2.0 * PADDING * s;
        let scroll = scroll.clamp(0.0, (content - band).max(0.0));
        let mut rows = Vec::new();
        let mut rules = Vec::new();
        let mut cy = y + PADDING * s - scroll;
        for (i, item) in items.iter().enumerate() {
            if item.rule {
                rules.push(cy + RULE / 2.0 * s);
                cy += RULE * s;
            }
            let row = ScreenRect {
                x: x + PADDING * s / 2.0,
                y: cy,
                w: w - PADDING * s,
                h: ITEM * s,
            };
            cy += ITEM * s;
            if row.y + row.h > y && row.y < y + h {
                rows.push((row, i));
            }
        }
        Menu {
            rect,
            rows,
            rules,
            scroll,
            content,
            scale: s,
        }
    }

    /// The scroll in use, and how far it can go.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content - (self.rect.h - 2.0 * PADDING * self.scale)).max(0.0)
    }

    /// Whether `(x, y)` is on the menu at all.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.rect.contains(x, y)
    }

    /// Where the rows show: inside the border's padding, which cuts a
    /// row scrolled part way out of sight.
    fn shown(&self) -> ScreenRect {
        self.rect.inset(PADDING * self.scale / 2.0)
    }

    /// The item under `(x, y)`, when it is one that can be taken — and
    /// only where its row shows, so the pointer cannot take a line from
    /// under the edge that hides it.
    pub fn hit(&self, x: f64, y: f64, items: &[Item]) -> Option<usize> {
        if !self.shown().contains(x, y) {
            return None;
        }
        self.rows
            .iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, i)| *i)
            .filter(|i| items.get(*i).is_some_and(|item| item.enabled))
    }

    /// Paint order: shadow, border, body, the rules, the row under the
    /// pointer, then every row's mark and label.
    pub fn prims(&self, items: &[Item], hover: Option<usize>, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
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
        let inner = self.shown();
        for y in &self.rules {
            let line = ScreenRect {
                x: self.rect.x + PADDING * s,
                y: y - b / 2.0,
                w: self.rect.w - 2.0 * PADDING * s,
                h: b,
            };
            out.push(Prim::rect(line, theme.border).clipped(inner));
        }
        for (row, i) in &self.rows {
            let Some(item) = items.get(*i) else { continue };
            if hover == Some(*i) && item.enabled {
                out.push(Prim::rounded(*row, theme.corner(RADIUS / 2.0, s), theme.active_bg).clipped(inner));
            }
            let ink = if item.enabled { theme.ink } else { theme.muted };
            let mark = ScreenRect {
                x: row.x + PADDING * s / 2.0,
                y: row.y + (row.h - MARK * s) / 2.0,
                w: MARK * s,
                h: MARK * s,
            };
            if item.checked {
                out.extend(
                    icon_prims(CHECK, mark, 24.0, MARK - 4.0, ICON_STROKE, s, ink)
                        .into_iter()
                        .map(|p| p.clipped(inner)),
                );
            } else if let Some(color) = item.dot {
                let (cx, cy) = mark.center();
                out.push(Prim::circle(cx, cy, DOT * s, color).clipped(inner));
            }
            let x = mark.x + mark.w + PADDING * s / 2.0;
            let baseline = atlas.baseline_in(*row);
            for g in atlas.layout(&item.label, x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(inner));
            }
        }
        out
    }
}

/// A tick: what is in force.
const CHECK: &[&[(f32, f32)]] = &[&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{KIND_IMAGE, KIND_SEGMENT};
    use crate::tabs::Tabs;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    const VP: Viewport = Viewport { w: 1200, h: 800 };

    fn button(x: f32, y: f32) -> ScreenRect {
        ScreenRect { x, y, w: 90.0, h: 24.0 }
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n).map(|i| Item::new(&format!("Item {i}"))).collect()
    }

    #[test]
    fn a_menu_opens_under_what_opened_it_a_row_an_item() {
        let a = atlas();
        let at = button(100.0, 100.0);
        let m = Menu::layout(VP, 1.0, at, &a, &items(3), 0.0);
        assert_eq!(m.rect.x, at.x);
        assert!(m.rect.y >= at.y + at.h, "under it");
        assert_eq!(m.rows.len(), 3);
        assert_eq!(m.rows[1].0.y - m.rows[0].0.y, ITEM);
        assert!(m.rect.w >= MIN_W);
        for (r, _) in &m.rows {
            assert!(m.rect.contains_rect(r));
        }
    }

    #[test]
    fn a_rule_starts_a_run_of_its_own() {
        let a = atlas();
        let list = vec![Item::new("A"), Item::new("B").ruled(), Item::new("C")];
        let m = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &list, 0.0);
        assert_eq!(m.rows[1].0.y - m.rows[0].0.y, ITEM + RULE, "the rule's room");
        assert_eq!(m.rows[2].0.y - m.rows[1].0.y, ITEM);
    }

    #[test]
    fn a_menu_stays_inside_the_window() {
        let a = atlas();
        // Opened by a button near the bottom right: it goes over the
        // button, and in from the edge.
        let at = button(1180.0, 760.0);
        let m = Menu::layout(VP, 1.0, at, &a, &items(5), 0.0);
        assert!(m.rect.y + m.rect.h <= at.y, "over what opened it");
        assert!(m.rect.x + m.rect.w <= 1200.0);
        // Too tall for any room: cut to the window, and a scroll area.
        let m = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &items(60), 0.0);
        assert!(m.rect.y >= 0.0 && m.rect.y + m.rect.h <= 800.0);
        assert!(m.max_scroll() > 0.0);
        let far = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &items(60), 1e6);
        assert_eq!(far.scroll(), far.max_scroll());
        assert_eq!(far.rows.last().unwrap().1, 59, "the last item comes into sight");
    }

    #[test]
    fn only_an_item_that_can_be_taken_answers_the_pointer() {
        let a = atlas();
        let off = Item {
            enabled: false,
            ..Item::new("Off")
        };
        let list = vec![Item::new("On"), off];
        let m = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &list, 0.0);
        let mid = |r: ScreenRect| {
            let (x, y) = r.center();
            (f64::from(x), f64::from(y))
        };
        let (x, y) = mid(m.rows[0].0);
        assert_eq!(m.hit(x, y, &list), Some(0));
        let (x, y) = mid(m.rows[1].0);
        assert_eq!(m.hit(x, y, &list), None);
        assert!(m.contains(x, y), "though the menu is still there");
        assert_eq!(m.hit(10.0, 10.0, &list), None);
        assert!(!m.contains(10.0, 10.0));
    }

    #[test]
    fn a_row_cut_by_the_menus_edge_answers_only_where_it_shows() {
        let a = atlas();
        let list = items(60);
        let m = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &list, 10.0);
        let shown = m.rect.inset(PADDING / 2.0);
        let (cut, i) = m.rows[0];
        assert!(cut.y < shown.y, "the first row runs under the edge");
        let x = f64::from(cut.x + cut.w / 2.0);
        assert_eq!(m.hit(x, f64::from(shown.y) - 1.0, &list), None);
        assert_eq!(m.hit(x, f64::from(shown.y) + 1.0, &list), Some(i));
    }

    #[test]
    fn a_menu_draws_its_labels_its_check_its_dots_and_the_row_under_the_pointer() {
        let theme = Theme::light();
        let a = atlas();
        let red = [1.0, 0.0, 0.0, 1.0];
        let list = vec![
            Item::new("Normal").checked(true),
            Item {
                dot: Some(red),
                ..Item::new("Red")
            },
            Item {
                enabled: false,
                ..Item::new("Off")
            },
        ];
        let m = Menu::layout(VP, 1.0, button(100.0, 100.0), &a, &list, 0.0);
        let prims = m.prims(&list, Some(1), &a, 7, &theme);
        let glyphs = prims.iter().filter(|q| q.kind == KIND_IMAGE && q.slot == 7).count();
        assert_eq!(glyphs, "Normal".len() + "Red".len() + "Off".len());
        let (first, _) = m.rows[0];
        assert!(prims.iter().any(|q| q.kind == KIND_SEGMENT && first.contains_rect(&q.bounds())), "the check");
        assert!(prims.iter().any(|q| q.color == red), "the dot");
        let hovered: Vec<&Prim> = prims.iter().filter(|q| q.color == theme.active_bg).collect();
        assert_eq!(hovered.len(), 1);
        assert_eq!(hovered[0].bounds(), m.rows[1].0);
        assert!(
            prims.iter().any(|q| q.kind == KIND_IMAGE && q.color == theme.muted),
            "what cannot be taken is muted"
        );
    }
}
