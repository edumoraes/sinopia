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
const WIDTH: f32 = 640.0;
const PADDING: f32 = 12.0;
const ROW_H: f32 = 34.0;
const FIELD_H: f32 = 28.0;
const GAP: f32 = 6.0;
const RADIUS: f32 = 12.0;
const ROW_RADIUS: f32 = 7.0;
const TITLE_H: f32 = 24.0;
/// The least room left between the dialog and the window's edge.
const MARGIN: f32 = 24.0;
/// Between an agent's name and the folder it is working in.
const NAME_GAP: f32 = 10.0;
/// A maker's mark on a row, square, and the room between it and the
/// name. Half the sheet's 40 px cell, so scale 1 samples it at an exact
/// halving and scale 2 one to one.
const LOGO: f32 = 20.0;
const LOGO_GAP: f32 = 10.0;
/// A mark's tile, when an agent has no mark of its own to put on one.
const TILE_RADIUS: f32 = 5.0;

/// Cell `i` of the logo sheet, which holds one square per agent in
/// [`crate::agents::KNOWN`]'s order.
fn logo_uv(i: usize) -> [f32; 4] {
    let n = crate::agents::KNOWN.len() as f32;
    [i as f32 / n, 0.0, (i + 1) as f32 / n, 1.0]
}

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
    /// What the logical px above were laid out at, so what is drawn
    /// inside the rects is measured the same way the rects were.
    pub scale: f32,
}

impl Panel {
    /// Lays the panel out centred in the window. `rows` is how many
    /// agents are running — the caller does not open a panel for none.
    pub fn layout(viewport: Viewport, scale: f64, rows: usize, folder: bool) -> Panel {
        let s = scale as f32;
        // As wide as it was drawn, unless the window is narrower than
        // that and its margins: then the window's.
        let w = (WIDTH * s).min(viewport.w as f32 - MARGIN * 2.0 * s);
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
            scale: s,
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
    /// neither — `app` owns them, as it owns every other field. `logos`
    /// is where the sheet of makers' marks was uploaded, once it has
    /// been: until then a row keeps the mark's place empty, so nothing
    /// moves when it lands.
    #[allow(clippy::too_many_arguments)]
    pub fn prims(
        &self,
        agents: &[Agent],
        target: usize,
        folder: Option<&Field>,
        line: &Field,
        atlas: &Atlas,
        slot: u32,
        logos: Option<u32>,
        theme: &Theme,
    ) -> Vec<Prim> {
        let corner = theme.corner(RADIUS, 1.0);
        let mut out = vec![
            Prim::soft(self.rect, corner, 18.0, theme.shadow),
            Prim::rounded(self.rect, corner, theme.panel),
        ];
        let baseline = atlas.baseline_in(self.title);
        for g in atlas.layout("Send to the agent", self.title.x, baseline) {
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink));
        }
        let s = self.scale;
        let folders = crate::agents::folders(agents);
        for (i, ((r, a), folder)) in self.rows.iter().zip(agents).zip(&folders).enumerate() {
            let picked = i == target;
            out.push(Prim::rounded(
                *r,
                theme.corner(ROW_RADIUS, 1.0),
                if picked { theme.active_bg } else { theme.panel },
            ));
            // An agent nothing can reach still takes the files; the row
            // says so by being muted rather than by being missing.
            let ink = if a.reachable() {
                theme.ink
            } else {
                theme.muted
            };
            let baseline = atlas.baseline_in(*r);
            // Its maker's mark first, which is what the eye finds before
            // it reads anything. Dimmed with the row it stands on.
            let mark = ScreenRect {
                x: r.x + PADDING * s,
                y: r.y + (r.h - LOGO * s) / 2.0,
                w: LOGO * s,
                h: LOGO * s,
            };
            let tint = if a.reachable() { 1.0 } else { 0.5 };
            match (a.mark(), logos) {
                (Some(cell), Some(sheet)) => {
                    out.push(Prim::glyph(
                        mark,
                        logo_uv(cell),
                        sheet,
                        [1.0, 1.0, 1.0, tint],
                    ));
                }
                (Some(_), None) => {}
                // An agent herdr knows and nobody here drew a mark for
                // still gets one: its initial, on a tile of its own.
                (None, _) => {
                    out.push(Prim::rounded(
                        mark,
                        theme.corner(TILE_RADIUS, s),
                        theme.border,
                    ));
                    let initial: String = a
                        .name()
                        .chars()
                        .take(1)
                        .flat_map(char::to_uppercase)
                        .collect();
                    let x = mark.x + (mark.w - atlas.measure(&initial)) / 2.0;
                    for g in atlas.layout(&initial, x, atlas.baseline_in(mark)) {
                        out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(*r));
                    }
                }
            }
            // Then what it is, and where: the name the person knows it
            // by, and the project it is in, which is what tells two rows
            // of one agent apart.
            let at = mark.x + mark.w + LOGO_GAP * s;
            for g in atlas.layout(a.name(), at, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(*r));
            }
            let at = at + atlas.measure(a.name()) + NAME_GAP * s;
            for g in atlas.layout(folder, at, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(*r));
            }
            // What herdr says it is doing, written at the row's far end
            // in muted ink: a blocked agent will refuse the prompt, and
            // the row is where that is worth knowing before pressing.
            if let Some(status) = a.status.as_deref() {
                let at = r.x + r.w - PADDING * s - atlas.measure(status);
                for g in atlas.layout(status, at, baseline) {
                    out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(*r));
                }
            }
        }
        if let (Some(rect), Some(field)) = (self.folder, folder) {
            out.push(Prim::rounded(rect, theme.corner(ROW_RADIUS, 1.0), theme.bg));
            out.extend(field.prims(rect, atlas, slot, theme, false));
        }
        out.push(Prim::rounded(
            self.line,
            theme.corner(ROW_RADIUS, 1.0),
            theme.bg,
        ));
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
    fn the_dialog_is_wide_enough_for_a_logo_a_name_a_folder_and_a_status() {
        let p = Panel::layout(Viewport { w: 1600, h: 900 }, 1.0, 2, false);
        assert_eq!(p.rect.w, 640.0);
        let p = Panel::layout(Viewport { w: 3200, h: 1800 }, 2.0, 2, false);
        assert_eq!(p.rect.w, 1280.0, "logical px, like the rest of the chrome");
    }

    #[test]
    fn a_narrow_window_keeps_a_margin_either_side_of_the_dialog() {
        let p = Panel::layout(Viewport { w: 500, h: 800 }, 1.0, 2, false);
        assert!(p.rect.x >= MARGIN - 0.5, "{:?}", p.rect);
        assert!(p.rect.x + p.rect.w <= 500.0 - MARGIN + 0.5, "{:?}", p.rect);
        for r in p.rows.iter().chain([&p.line]) {
            assert!(p.rect.contains_rect(r), "{r:?} outside {:?}", p.rect);
        }
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
        let without = p.prims(&[quiet], 0, None, &Field::new(""), &atlas, 0, None, &theme);
        let with = p.prims(&[busy], 0, None, &Field::new(""), &atlas, 0, None, &theme);
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
        let folders = crate::agents::folders(&agents);
        for ((a, folder), row) in agents.iter().zip(&folders).zip(&p.rows) {
            // The mark and the name start one padding in and the status
            // ends one padding from the far edge, so all of it has to
            // fit between.
            let room = row.w - PADDING * 2.0 - LOGO - LOGO_GAP - atlas.measure("working") - PADDING;
            let said = atlas.measure(a.name()) + NAME_GAP + atlas.measure(folder);
            assert!(said <= room, "{a:?} does not fit its row beside a status");
        }
    }

    /// The glyphs a row wrote: the atlas's prims, cut to that row.
    fn glyphs_in(prims: &[Prim], row: ScreenRect, slot: u32) -> usize {
        prims
            .iter()
            .filter(|p| p.kind == crate::scene::KIND_IMAGE && p.slot == slot)
            .filter(|p| p.clip == [row.x, row.y, row.w, row.h])
            .count()
    }

    #[test]
    fn a_row_says_the_agents_name_and_its_last_directory_and_not_its_path() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = crate::theme::Theme::light();
        let a = crate::agents::Agent::at(
            "claude",
            "/home/e/Work/a/very/deep/project",
            crate::agents::Reach::None,
        );
        let p = Panel::layout(viewport(), 1.0, 1, false);
        let prims = p.prims(&[a], 0, None, &Field::new(""), &atlas, 7, None, &theme);
        let inked = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(
            glyphs_in(&prims, p.rows[0], 7),
            inked("Claude Code") + inked("project")
        );
    }

    #[test]
    fn a_row_wears_its_agents_mark_from_the_logo_sheet() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = crate::theme::Theme::light();
        let codex = crate::agents::Agent::at("codex", "/w/a", crate::agents::Reach::None);
        let p = Panel::layout(viewport(), 1.0, 1, false);
        let prims = p.prims(
            &[codex],
            0,
            None,
            &Field::new(""),
            &atlas,
            0,
            Some(9),
            &theme,
        );
        let marks: Vec<&Prim> = prims.iter().filter(|q| q.slot == 9).collect();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].uv, logo_uv(1), "codex is the second cell");
        let m = marks[0].bounds();
        assert!(p.rows[0].contains_rect(&m), "{m:?}");
        assert_eq!((m.w, m.h), (LOGO, LOGO));
        let (_, cy) = p.rows[0].center();
        assert!((m.y + m.h / 2.0 - cy).abs() < 0.5, "centred on the row");
    }

    #[test]
    fn the_name_stands_in_one_place_whatever_mark_is_beside_it() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = crate::theme::Theme::light();
        let p = Panel::layout(viewport(), 1.0, 1, false);
        let at = |kind, logos| {
            let a = crate::agents::Agent::at(kind, "/w/a", crate::agents::Reach::None);
            let prims = p.prims(&[a], 0, None, &Field::new(""), &atlas, 7, logos, &theme);
            // The letters, not the tile: the tile is the mark's.
            prims
                .iter()
                .filter(|q| {
                    q.slot == 7 && q.clip == [p.rows[0].x, p.rows[0].y, p.rows[0].w, p.rows[0].h]
                })
                .filter(|q| q.bounds().x > p.rows[0].x + PADDING + LOGO)
                .map(|q| q.bounds().x)
                .fold(f32::INFINITY, f32::min)
        };
        let with = at("claude", Some(9));
        assert!(
            with >= p.rows[0].x + PADDING + LOGO + LOGO_GAP - 1.0,
            "{with}"
        );
        assert_eq!(
            at("claude", None),
            with,
            "a sheet not uploaded yet moves nothing"
        );
        assert_eq!(at("pi", Some(9)), with, "nor does a mark nobody drew");
    }

    #[test]
    fn an_agent_nobody_drew_a_mark_for_gets_its_initial_on_a_tile() {
        let atlas = Atlas::build(&crate::text::Font::bundled(), 13);
        let theme = crate::theme::Theme::light();
        let pi = crate::agents::Agent::at("pi", "/w/a", crate::agents::Reach::Herdr("p".into()));
        let p = Panel::layout(viewport(), 1.0, 1, false);
        let prims = p.prims(&[pi], 0, None, &Field::new(""), &atlas, 7, Some(9), &theme);
        assert!(prims.iter().all(|q| q.slot != 9), "no cell of the sheet");
        let inked = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(
            glyphs_in(&prims, p.rows[0], 7),
            1 + inked("pi") + inked("a"),
            "the initial, the name, the folder"
        );
    }
}
