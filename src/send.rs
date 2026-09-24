//! The export dialog: the agents that are running, the folder the page
//! lands in when the scope has no name of its own, and the instruction
//! that goes with it — a box that grows a line at a time and scrolls past
//! [`MAX_LINES`]. Its shape is `dock`'s — layout, hit, prims — and it
//! holds no state: `app` owns the fields and lends them for a frame.

use crate::agents::Agent;
use crate::field::{self, Boxed, Field};
use crate::scene::{Prim, ScreenRect, Viewport};
use crate::text::{Atlas, Line};
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
/// The instruction's scrollbar thumb, and the room kept between it and
/// the text: the text wraps short of it whether or not it is showing,
/// so a line does not rewrap the moment the box starts to scroll.
const BAR_W: f32 = 4.0;
const BAR_GAP: f32 = 4.0;
const BAR_MIN: f32 = 16.0;

/// The most lines the instruction box grows to. Past it the box keeps
/// its height and scrolls.
pub const MAX_LINES: usize = 20;

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

/// What the dialog is showing, which is what decides its shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// How many agents are running — the caller opens no dialog for none.
    pub rows: usize,
    /// Whether the scope needs a folder named for it.
    pub folder: bool,
    /// How many lines the instruction wraps to at [`Panel::text_width`].
    pub lines: usize,
    /// How far apart two lines stand, in px: the atlas's own.
    pub line_h: f32,
}

/// What the dialog draws: the state `app` keeps, lent for one frame.
#[derive(Debug, Clone, Copy)]
pub struct Look<'a> {
    pub agents: &'a [Agent],
    /// Which row wears the ring.
    pub target: usize,
    /// The folder's field, when the scope asks for one.
    pub folder: Option<&'a Field>,
    pub line: &'a Field,
    /// Which field has the keyboard, and so the caret.
    pub focus: Hit,
    /// How far down its lines the instruction is scrolled, in px.
    pub scroll: f32,
}

/// What it is drawn with: the glyph atlas and its slot, where the sheet
/// of makers' marks was uploaded — once it has been; until then a row
/// keeps the mark's place empty, so nothing moves when it lands — and
/// the theme.
#[derive(Clone, Copy)]
pub struct Ink<'a> {
    pub atlas: &'a Atlas,
    pub slot: u32,
    pub logos: Option<u32>,
    pub theme: &'a Theme,
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub rect: ScreenRect,
    pub title: ScreenRect,
    /// One per running agent, in the order they were given.
    pub rows: Vec<ScreenRect>,
    /// Where the page lands, shown only when the scope has no name.
    pub folder: Option<ScreenRect>,
    /// The instruction's box.
    pub line: ScreenRect,
    /// How many of the instruction's lines are in sight at once.
    pub shown: usize,
    /// How far apart they stand, in px.
    pub line_h: f32,
    /// The width the instruction wraps at, in px.
    pub text_w: f32,
    /// What the logical px above were laid out at, so what is drawn
    /// inside the rects is measured the same way the rects were.
    pub scale: f32,
}

impl Panel {
    /// How wide the dialog stands: as it was drawn, unless the window is
    /// narrower than that and its margins — then the window's.
    fn width(viewport: Viewport, s: f32) -> f32 {
        (WIDTH * s).min(viewport.w as f32 - MARGIN * 2.0 * s)
    }

    /// The width the instruction wraps at, which the caller needs before
    /// the layout does: how many lines it wraps to is what decides how
    /// tall the box stands.
    pub fn text_width(viewport: Viewport, scale: f64) -> f32 {
        let s = scale as f32;
        Panel::width(viewport, s)
            - PADDING * 2.0 * s
            - field::PADDING * 2.0 * s
            - (BAR_W + BAR_GAP) * s
    }

    /// Lays the dialog out centred in the window. The box shows as many
    /// lines as the instruction has, up to [`MAX_LINES`] — and fewer when
    /// the window is too short for them, since a dialog that ran off the
    /// window would hide the very rows it is for.
    pub fn layout(viewport: Viewport, scale: f64, spec: &Spec) -> Panel {
        let s = scale as f32;
        let w = Panel::width(viewport, s);
        let inset = field::PADDING * s;
        let fields = if spec.folder { 1.0 } else { 0.0 };
        // Everything but the lines of the instruction.
        let fixed = PADDING * 2.0 * s
            + TITLE_H * s
            + spec.rows as f32 * (ROW_H * s + GAP * s)
            + fields * (FIELD_H * s + GAP * s)
            + inset * 2.0;
        let room = viewport.h as f32 - MARGIN * 2.0 * s - fixed;
        let fit = (room / spec.line_h).floor().max(1.0) as usize;
        let shown = spec.lines.clamp(1, MAX_LINES).min(fit);
        let h = fixed + shown as f32 * spec.line_h;
        let x = (viewport.w as f32 - w) / 2.0;
        let y = ((viewport.h as f32 - h) / 2.0).max(MARGIN * s);
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
        let rows = (0..spec.rows)
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
        let folder = spec.folder.then(|| {
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
            h: shown as f32 * spec.line_h + inset * 2.0,
        };
        Panel {
            rect,
            title,
            rows,
            folder,
            line,
            shown,
            line_h: spec.line_h,
            text_w: Panel::text_width(viewport, scale),
            scale: s,
        }
    }

    /// The instruction's box as the field draws itself into it, with
    /// `lines` scrolled down by `scroll` px.
    pub fn boxed<'a>(&self, lines: &'a [Line], scroll: f32) -> Boxed<'a> {
        Boxed {
            rect: self.line,
            lines,
            scroll,
            line_h: self.line_h,
            inset: field::PADDING * self.scale,
        }
    }

    /// The furthest the instruction scrolls, in px, when it wraps to
    /// `lines`: none at all while every line is in sight.
    pub fn max_scroll(&self, lines: usize) -> f32 {
        (lines.saturating_sub(self.shown)) as f32 * self.line_h
    }

    /// The scrollbar's thumb, when there is more text than box: as tall a
    /// share of the box as the box is of the text, and as far down it as
    /// the text is scrolled.
    pub fn thumb(&self, lines: usize, scroll: f32) -> Option<ScreenRect> {
        let most = self.max_scroll(lines);
        if most <= 0.0 {
            return None;
        }
        let s = self.scale;
        let track = self.line.inset(field::PADDING * s);
        let h = (track.h * self.shown as f32 / lines as f32)
            .max(BAR_MIN * s)
            .min(track.h);
        Some(ScreenRect {
            x: self.line.x + self.line.w - (BAR_W + BAR_GAP / 2.0) * s,
            y: track.y + (track.h - h) * (scroll / most).clamp(0.0, 1.0),
            w: BAR_W * s,
            h,
        })
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

    pub fn prims(&self, look: &Look, ink: &Ink) -> Vec<Prim> {
        let (atlas, slot, theme) = (ink.atlas, ink.slot, ink.theme);
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
        let folders = crate::agents::folders(look.agents);
        for (i, ((r, a), folder)) in self.rows.iter().zip(look.agents).zip(&folders).enumerate() {
            let picked = i == look.target;
            out.push(Prim::rounded(
                *r,
                theme.corner(ROW_RADIUS, 1.0),
                if picked { theme.active_bg } else { theme.panel },
            ));
            // An agent nothing can reach still takes the files; the row
            // says so by being muted rather than by being missing.
            let tone = if a.reachable() {
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
            match (a.mark(), ink.logos) {
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
                        out.push(Prim::glyph(g.rect, g.uv, slot, tone).clipped(*r));
                    }
                }
            }
            // Then what it is, and where: the name the person knows it
            // by, and the project it is in, which is what tells two rows
            // of one agent apart.
            let at = mark.x + mark.w + LOGO_GAP * s;
            for g in atlas.layout(a.name(), at, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, tone).clipped(*r));
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
        if let (Some(rect), Some(field)) = (self.folder, look.folder) {
            out.push(Prim::rounded(rect, theme.corner(ROW_RADIUS, 1.0), theme.bg));
            out.extend(field.prims(rect, atlas, slot, theme, look.focus == Hit::Folder));
        }
        out.push(Prim::rounded(
            self.line,
            theme.corner(ROW_RADIUS, 1.0),
            theme.bg,
        ));
        let lines = look.line.wrap(atlas, self.text_w);
        let b = self.boxed(&lines, look.scroll);
        out.extend(
            look.line
                .prims_boxed(&b, atlas, slot, theme, look.focus == Hit::Line),
        );
        if let Some(thumb) = self.thumb(lines.len(), look.scroll) {
            out.push(Prim::rounded(thumb, thumb.w / 2.0, theme.muted));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Agent, Reach};

    fn viewport() -> Viewport {
        Viewport { w: 1200, h: 800 }
    }

    const LINE_H: f32 = 19.0;

    fn spec(rows: usize, folder: bool) -> Spec {
        Spec {
            rows,
            folder,
            lines: 1,
            line_h: LINE_H,
        }
    }

    fn lines(n: usize) -> Spec {
        Spec {
            lines: n,
            ..spec(2, false)
        }
    }

    fn atlas() -> Atlas {
        Atlas::build(&crate::text::Font::bundled(), 13)
    }

    /// Everything a frame of the dialog is drawn with, but the agents.
    struct Fixture {
        atlas: Atlas,
        theme: Theme,
        line: Field,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture {
                atlas: atlas(),
                theme: Theme::light(),
                line: Field::lines(""),
            }
        }

        fn ink(&self, slot: u32, logos: Option<u32>) -> Ink<'_> {
            Ink {
                atlas: &self.atlas,
                slot,
                logos,
                theme: &self.theme,
            }
        }

        fn look<'a>(&'a self, agents: &'a [Agent]) -> Look<'a> {
            Look {
                agents,
                target: 0,
                folder: None,
                line: &self.line,
                focus: Hit::Line,
                scroll: 0.0,
            }
        }
    }

    #[test]
    fn a_row_a_target_and_the_line_are_all_inside_the_panel() {
        let p = Panel::layout(viewport(), 1.0, &spec(3, false));
        assert_eq!(p.rows.len(), 3);
        for r in &p.rows {
            assert!(p.rect.contains_rect(r), "a row outside the panel: {r:?}");
        }
        assert!(p.rect.contains_rect(&p.line));
        assert!(p.folder.is_none());
    }

    #[test]
    fn the_folder_field_appears_only_when_it_is_asked_for_and_makes_the_panel_taller() {
        let without = Panel::layout(viewport(), 1.0, &spec(2, false));
        let with = Panel::layout(viewport(), 1.0, &spec(2, true));
        assert!(with.folder.is_some());
        assert!(with.rect.h > without.rect.h);
        assert!(with.rect.contains_rect(&with.folder.unwrap()));
    }

    #[test]
    fn the_dialog_is_wide_enough_for_a_logo_a_name_a_folder_and_a_status() {
        let p = Panel::layout(Viewport { w: 1600, h: 900 }, 1.0, &spec(2, false));
        assert_eq!(p.rect.w, 640.0);
        let p = Panel::layout(Viewport { w: 3200, h: 1800 }, 2.0, &spec(2, false));
        assert_eq!(p.rect.w, 1280.0, "logical px, like the rest of the chrome");
    }

    #[test]
    fn a_narrow_window_keeps_a_margin_either_side_of_the_dialog() {
        let p = Panel::layout(Viewport { w: 500, h: 800 }, 1.0, &spec(2, false));
        assert!(p.rect.x >= MARGIN - 0.5, "{:?}", p.rect);
        assert!(p.rect.x + p.rect.w <= 500.0 - MARGIN + 0.5, "{:?}", p.rect);
        for r in p.rows.iter().chain([&p.line]) {
            assert!(p.rect.contains_rect(r), "{r:?} outside {:?}", p.rect);
        }
    }

    #[test]
    fn the_panel_is_centred_in_the_window() {
        let p = Panel::layout(viewport(), 1.0, &spec(2, false));
        let (cx, _) = p.rect.center();
        assert!((cx - 600.0).abs() < 1.0);
    }

    #[test]
    fn the_box_grows_a_line_at_a_time() {
        let one = Panel::layout(viewport(), 1.0, &lines(1));
        let three = Panel::layout(viewport(), 1.0, &lines(3));
        assert_eq!(three.line.h - one.line.h, 2.0 * LINE_H);
        assert_eq!(three.rect.h - one.rect.h, 2.0 * LINE_H);
        assert_eq!(three.shown, 3);
        assert!(three.rect.contains_rect(&three.line));
    }

    #[test]
    fn past_twenty_lines_the_box_stops_growing_and_scrolls() {
        let twenty = Panel::layout(viewport(), 1.0, &lines(MAX_LINES));
        let more = Panel::layout(viewport(), 1.0, &lines(MAX_LINES + 15));
        assert_eq!(twenty.shown, 20);
        assert_eq!(more.shown, 20);
        assert_eq!(more.line.h, twenty.line.h);
        assert_eq!(more.max_scroll(MAX_LINES + 15), 15.0 * LINE_H);
        assert_eq!(twenty.max_scroll(MAX_LINES), 0.0);
    }

    #[test]
    fn a_short_window_shows_fewer_lines_and_the_dialog_still_fits() {
        let short = Viewport { w: 1200, h: 420 };
        let p = Panel::layout(short, 1.0, &lines(MAX_LINES + 5));
        assert!(p.shown < MAX_LINES && p.shown >= 1, "{}", p.shown);
        assert!(p.rect.y >= MARGIN - 0.5, "{:?}", p.rect);
        assert!(p.rect.y + p.rect.h <= 420.0 - MARGIN + 0.5, "{:?}", p.rect);
    }

    #[test]
    fn the_text_wraps_short_of_the_scrollbar() {
        let p = Panel::layout(viewport(), 1.0, &lines(3));
        let b = p.boxed(&[], 0.0);
        assert!(p.text_w > 0.0);
        assert!(b.inset + p.text_w + (BAR_W + BAR_GAP) <= b.rect.w + 0.01);
        assert_eq!(Panel::text_width(viewport(), 1.0), p.text_w);
    }

    #[test]
    fn a_thumb_says_how_much_of_the_text_is_in_sight_and_where() {
        let p = Panel::layout(viewport(), 1.0, &lines(40));
        assert!(
            Panel::layout(viewport(), 1.0, &lines(20))
                .thumb(20, 0.0)
                .is_none()
        );
        let top = p.thumb(40, 0.0).unwrap();
        let track = p.line.inset(p.boxed(&[], 0.0).inset);
        assert!(
            (top.h - track.h / 2.0).abs() < 0.01,
            "twenty of forty lines"
        );
        assert!((top.y - track.y).abs() < 0.01);
        let end = p.thumb(40, p.max_scroll(40)).unwrap();
        assert!((end.y + end.h - (track.y + track.h)).abs() < 0.01);
        assert!(p.line.contains_rect(&end));
    }

    #[test]
    fn a_press_finds_the_row_it_landed_on() {
        let p = Panel::layout(viewport(), 1.0, &spec(3, false));
        for (i, r) in p.rows.iter().enumerate() {
            let (x, y) = r.center();
            assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Target(i)));
        }
    }

    #[test]
    fn a_press_on_each_field_names_that_field() {
        let p = Panel::layout(viewport(), 1.0, &spec(1, true));
        let (x, y) = p.line.center();
        assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Line));
        let (x, y) = p.folder.unwrap().center();
        assert_eq!(p.hit(f64::from(x), f64::from(y)), Some(Hit::Folder));
    }

    #[test]
    fn a_press_outside_the_panel_is_not_the_panels() {
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        assert_eq!(p.hit(5.0, 5.0), None);
    }

    /// The caret, where there is one: the text is drawn in the atlas's
    /// slot, and the only box the ink's colour and the caret's width.
    fn carets(prims: &[Prim]) -> usize {
        prims
            .iter()
            .filter(|p| p.kind == crate::scene::KIND_BOX)
            .filter(|p| (p.geom[2] - 1.5).abs() < 0.01)
            .count()
    }

    #[test]
    fn only_the_field_with_the_keyboard_shows_a_caret() {
        let f = Fixture::new();
        let agents = [Agent::at("claude", "/w/a", Reach::None)];
        let folder = Field::new("sketch");
        let p = Panel::layout(viewport(), 1.0, &spec(1, true));
        let on_line = Look {
            folder: Some(&folder),
            ..f.look(&agents)
        };
        let on_folder = Look {
            focus: Hit::Folder,
            ..on_line
        };
        let line_caret = p.prims(&on_line, &f.ink(0, None));
        let folder_caret = p.prims(&on_folder, &f.ink(0, None));
        assert_eq!(carets(&line_caret), 1);
        assert_eq!(carets(&folder_caret), 1);
        let caret_y = |prims: &[Prim]| {
            prims
                .iter()
                .find(|q| q.kind == crate::scene::KIND_BOX && (q.geom[2] - 1.5).abs() < 0.01)
                .map(|q| q.geom[1])
                .unwrap()
        };
        assert!(
            caret_y(&line_caret) > caret_y(&folder_caret),
            "the line is below"
        );
    }

    #[test]
    fn a_row_says_what_the_agent_is_doing_when_the_backend_says() {
        let f = Fixture::new();
        let quiet = Agent::at("claude", "/w/a", Reach::None);
        let busy = Agent {
            status: Some("working".into()),
            ..quiet.clone()
        };
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        let without = p.prims(&f.look(&[quiet]), &f.ink(0, None));
        let with = p.prims(&f.look(&[busy]), &f.ink(0, None));
        assert!(
            with.len() > without.len(),
            "the status is written when there is one"
        );
    }

    #[test]
    fn every_agents_row_is_written_without_running_off_the_panel() {
        // The same promise the properties bar makes: a name in this
        // window is never written with an ellipsis it did not choose.
        let atlas = atlas();
        let agents = [
            Agent::at("claude", "/home/e/Work/board", Reach::None),
            Agent::at("opencode", "/home/e/Work/a/very/deep/project", Reach::None),
        ];
        let p = Panel::layout(viewport(), 1.0, &spec(agents.len(), false));
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
        let f = Fixture::new();
        let a = Agent::at("claude", "/home/e/Work/a/very/deep/project", Reach::None);
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        let prims = p.prims(&f.look(&[a]), &f.ink(7, None));
        let inked = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(
            glyphs_in(&prims, p.rows[0], 7),
            inked("Claude Code") + inked("project")
        );
    }

    #[test]
    fn a_row_wears_its_agents_mark_from_the_logo_sheet() {
        let f = Fixture::new();
        let codex = Agent::at("codex", "/w/a", Reach::None);
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        let prims = p.prims(&f.look(&[codex]), &f.ink(0, Some(9)));
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
        let f = Fixture::new();
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        let row = p.rows[0];
        let at = |kind, logos| {
            let a = [Agent::at(kind, "/w/a", Reach::None)];
            let prims = p.prims(&f.look(&a), &f.ink(7, logos));
            // The letters, not the tile: the tile is the mark's.
            prims
                .iter()
                .filter(|q| q.slot == 7 && q.clip == [row.x, row.y, row.w, row.h])
                .filter(|q| q.bounds().x > row.x + PADDING + LOGO)
                .map(|q| q.bounds().x)
                .fold(f32::INFINITY, f32::min)
        };
        let with = at("claude", Some(9));
        assert!(with >= row.x + PADDING + LOGO + LOGO_GAP - 1.0, "{with}");
        assert_eq!(
            at("claude", None),
            with,
            "a sheet not uploaded yet moves nothing"
        );
        assert_eq!(at("pi", Some(9)), with, "nor does a mark nobody drew");
    }

    #[test]
    fn an_agent_nobody_drew_a_mark_for_gets_its_initial_on_a_tile() {
        let f = Fixture::new();
        let pi = Agent::at("pi", "/w/a", Reach::Herdr("p".into()));
        let p = Panel::layout(viewport(), 1.0, &spec(1, false));
        let prims = p.prims(&f.look(&[pi]), &f.ink(7, Some(9)));
        assert!(prims.iter().all(|q| q.slot != 9), "no cell of the sheet");
        let inked = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(
            glyphs_in(&prims, p.rows[0], 7),
            1 + inked("pi") + inked("a"),
            "the initial, the name, the folder"
        );
    }
}
