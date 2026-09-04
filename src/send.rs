//! The export-to-agent panel: the agents that are running, the folder
//! the page lands in when the scope has no name of its own, and the line
//! of instruction that goes with it. Its shape is `dock`'s — layout,
//! hit, prims — and it holds no state: `app` owns the fields and hands
//! them in to be drawn.

use crate::agents::Agent;
use crate::field::Field;
use crate::scene::{Prim, ScreenRect, Viewport};
use crate::text::Atlas;
use crate::theme::Theme;

/// Logical px, all of them.
const WIDTH: f32 = 420.0;
const PADDING: f32 = 12.0;
const ROW_H: f32 = 30.0;
const FIELD_H: f32 = 28.0;
const GAP: f32 = 6.0;
const RADIUS: f32 = 12.0;
const ROW_RADIUS: f32 = 7.0;
const TITLE_H: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// An agent's row, by its place in the list handed to `layout`.
    Target(usize),
    Folder,
    Line,
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub rect: ScreenRect,
    pub title: ScreenRect,
    /// One per running agent, in the order they were given.
    pub rows: Vec<ScreenRect>,
    /// Where the page lands, shown only when the scope has no name.
    pub folder: Option<ScreenRect>,
    pub line: ScreenRect,
}

impl Panel {
    /// Lays the panel out centred in the window. `rows` is how many
    /// agents are running — the caller does not open a panel for none.
    pub fn layout(viewport: Viewport, scale: f64, rows: usize, folder: bool) -> Panel {
        let s = scale as f32;
        let w = WIDTH * s;
        let fields = if folder { 2.0 } else { 1.0 };
        let h = PADDING * 2.0 * s
            + TITLE_H * s
            + rows as f32 * (ROW_H * s + GAP * s)
            + fields * (FIELD_H * s + GAP * s);
        let x = (viewport.w as f32 - w) / 2.0;
        let y = (viewport.h as f32 - h) / 2.0;
        let rect = ScreenRect { x, y, w, h };
        let inner = x + PADDING * s;
        let inner_w = w - PADDING * 2.0 * s;
        let title = ScreenRect {
            x: inner,
            y: y + PADDING * s,
            w: inner_w,
            h: TITLE_H * s,
        };
        let mut pen = title.y + title.h;
        let rows = (0..rows)
            .map(|_| {
                let r = ScreenRect {
                    x: inner,
                    y: pen,
                    w: inner_w,
                    h: ROW_H * s,
                };
                pen += ROW_H * s + GAP * s;
                r
            })
            .collect();
        let folder = folder.then(|| {
            let r = ScreenRect {
                x: inner,
                y: pen,
                w: inner_w,
                h: FIELD_H * s,
            };
            pen += FIELD_H * s + GAP * s;
            r
        });
        let line = ScreenRect {
            x: inner,
            y: pen,
            w: inner_w,
            h: FIELD_H * s,
        };
        Panel {
            rect,
            title,
            rows,
            folder,
            line,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if let Some(i) = self.rows.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::Target(i));
        }
        if self.folder.is_some_and(|f| f.contains(x, y)) {
            return Some(Hit::Folder);
        }
        // Everything else in the panel is the line: a press on the
        // panel's own ground puts the caret where it was going anyway.
        Some(Hit::Line)
    }

    /// `target` is which row wears the ring; `folder` is the field when
    /// there is one, and `line` is the instruction. The panel holds
    /// neither — `app` owns them, as it owns every other field.
    #[allow(clippy::too_many_arguments)]
    pub fn prims(
        &self,
        agents: &[Agent],
        target: usize,
        folder: Option<&Field>,
        line: &Field,
        atlas: &Atlas,
        slot: u32,
        theme: &Theme,
    ) -> Vec<Prim> {
        let mut out = vec![
            Prim::soft(self.rect, RADIUS, 18.0, theme.shadow),
            Prim::rounded(self.rect, RADIUS, theme.panel),
        ];
        let baseline = atlas.baseline_in(self.title);
        for g in atlas.layout("Send to the agent", self.title.x, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        for (i, (r, a)) in self.rows.iter().zip(agents).enumerate() {
            let picked = i == target;
            out.push(Prim::rounded(
                *r,
                ROW_RADIUS,
                if picked { theme.active_bg } else { theme.panel },
            ));
            // An agent nothing can reach still takes the files; the row
            // says so by being muted rather than by being missing.
            let ink = if a.reachable() { theme.ink } else { theme.muted };
            let baseline = atlas.baseline_in(*r);
            for g in atlas.layout(&a.label(), r.x + PADDING, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(*r));
            }
            // What herdr says it is doing, written at the row's far end
            // in muted ink: a blocked agent will refuse the prompt, and
            // the row is where that is worth knowing before pressing.
            if let Some(status) = a.status.as_deref() {
                let at = r.x + r.w - PADDING - atlas.measure(status);
                for g in atlas.layout(status, at, baseline) {
                    out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(*r));
                }
            }
        }
        if let (Some(rect), Some(field)) = (self.folder, folder) {
            out.push(Prim::rounded(rect, ROW_RADIUS, theme.bg));
            out.extend(field.prims(rect, atlas, slot, theme, false));
        }
        out.push(Prim::rounded(self.line, ROW_RADIUS, theme.bg));
        out.extend(line.prims(self.line, atlas, slot, theme, true));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Viewport {
        Viewport { w: 1200, h: 800 }
    }

    #[test]
    fn a_row_a_target_and_the_line_are_all_inside_the_panel() {
        let p = Panel::layout(viewport(), 1.0, 3, false);
        assert_eq!(p.rows.len(), 3);
        for r in &p.rows {
            assert!(p.rect.contains_rect(r), "a row outside the panel: {r:?}");
        }
        assert!(p.rect.contains_rect(&p.line));
        assert!(p.folder.is_none());
    }

    #[test]
    fn the_folder_field_appears_only_when_it_is_asked_for_and_makes_the_panel_taller() {
        let without = Panel::layout(viewport(), 1.0, 2, false);
        let with = Panel::layout(viewport(), 1.0, 2, true);
        assert!(with.folder.is_some());
        assert!(with.rect.h > without.rect.h);
        assert!(with.rect.contains_rect(&with.folder.unwrap()));
    }

    #[test]
    fn the_panel_is_centred_in_the_window() {
        let p = Panel::layout(viewport(), 1.0, 2, false);
        let (cx, _) = p.rect.center();
        assert!((cx - 600.0).abs() < 1.0);
    }

    #[test]
    fn a_press_finds_the_row_it_landed_on() {
        let p = Panel::layout(viewport(), 1.0, 3, false);
        for (i, r) in p.rows.iter().enumerate() {
            let (x, y) = r.center();
            assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Target(i)));
        }
    }

    #[test]
    fn a_press_on_each_field_names_that_field() {
        let p = Panel::layout(viewport(), 1.0, 1, true);
        let (x, y) = p.line.center();
        assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Line));
        let (x, y) = p.folder.unwrap().center();
        assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Folder));
    }

    #[test]
    fn a_press_outside_the_panel_is_not_the_panels() {
        let p = Panel::layout(viewport(), 1.0, 1, false);
        assert_eq!(p.hit(5.0, 5.0), None);
    }

    #[test]
    fn a_row_says_what_the_agent_is_doing_when_the_backend_says() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = crate::theme::Theme::light();
        let quiet = crate::agents::Agent::at("claude", "/w/a", crate::agents::Reach::None);
        let busy = crate::agents::Agent {
            status: Some("working".into()),
            ..quiet.clone()
        };
        let p = Panel::layout(viewport(), 1.0, 1, false);
        let without = p.prims(&[quiet], 0, None, &Field::new(""), &atlas, 0, &theme);
        let with = p.prims(&[busy], 0, None, &Field::new(""), &atlas, 0, &theme);
        assert!(
            with.len() > without.len(),
            "the status is written when there is one"
        );
    }

    #[test]
    fn every_agents_row_is_written_without_running_off_the_panel() {
        // The same promise the properties bar makes: a name in this
        // window is never written with an ellipsis it did not choose.
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let agents = [
            crate::agents::Agent::at("claude", "/home/e/Work/board", crate::agents::Reach::None),
            crate::agents::Agent::at(
                "opencode",
                "/home/e/Work/a/very/deep/project",
                crate::agents::Reach::None,
            ),
        ];
        let p = Panel::layout(viewport(), 1.0, agents.len(), false);
        for (a, row) in agents.iter().zip(&p.rows) {
            // The label starts one padding in and the status ends one
            // padding from the far edge, so both have to fit between.
            let room = row.w - PADDING * 2.0 - atlas.measure("working") - PADDING;
            assert!(
                atlas.measure(&a.label()) <= room,
                "{:?} does not fit its row beside a status",
                a.label()
            );
        }
    }
}
