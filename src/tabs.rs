//! Tab strip: one tab per open project, across the top of the window.
//!
//! The dock's shape, turned the other way up — sized in logical px so it
//! reads the same on any display, positioned in physical px, floating
//! over the canvas and swallowing whatever it catches. Pure: `app` asks
//! where a click landed and what to draw.

use crate::scene::{Prim, Rgba, ScreenRect, Viewport};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
pub const HEIGHT: f32 = 34.0;
/// A tab never grows past this, however few are open.
const MAX_TAB: f32 = 200.0;
/// Nor below what the marks that must survive need: the dot, the cross,
/// and [`MIN_PAD`] on each side. The label is what gives way first.
const MIN_TAB: f32 = 2.0 * MIN_PAD + 2.0 * DOT_R + DOT_GAP + CROSS;
const PAD: f32 = 10.0;
/// How close a mark may come to the tab's edge once it is squeezed.
const MIN_PAD: f32 = 4.0;
const DOT_R: f32 = 3.0;
const DOT_GAP: f32 = 7.0;
const CROSS: f32 = 12.0;
const CROSS_GAP: f32 = 6.0;
const CROSS_STROKE: f32 = 1.3;
/// The clickable area around the cross, past the glyph itself.
const CROSS_SLOP: f32 = 3.0;
const NEW: f32 = 30.0;
const NEW_STROKE: f32 = 1.4;
const NEW_ARM: f32 = 5.0;
const TAB_RADIUS: f32 = 7.0;
const LABEL: f32 = 13.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabHit {
    Select(usize),
    Close(usize),
    New,
    /// Strip chrome: swallowed, never reaches the canvas.
    Strip,
}

/// One tab, with everything already measured: what a click can land on
/// and what the label has been cut down to.
#[derive(Debug, Clone, PartialEq)]
pub struct TabBox {
    pub rect: ScreenRect,
    /// Absent when the tab is too narrow to hold a cross.
    pub close: Option<ScreenRect>,
    /// Present only while the project is dirty, and only if it fits.
    pub dot: Option<ScreenRect>,
    /// Truncated to what the tab has room for — may be empty.
    pub label: String,
    /// Where the label's pen starts.
    pub label_x: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tabs {
    pub strip: ScreenRect,
    pub tabs: Vec<TabBox>,
    pub new: ScreenRect,
    active: usize,
    scale: f32,
}

impl Tabs {
    /// `labels` is one `(text, dirty)` per open project, in tab order.
    pub fn layout(
        viewport: Viewport,
        scale: f64,
        atlas: &Atlas,
        labels: &[(String, bool)],
        active: usize,
    ) -> Tabs {
        let s = scale as f32;
        let strip = ScreenRect {
            x: 0.0,
            y: 0.0,
            w: viewport.w as f32,
            h: (HEIGHT * s).round(),
        };
        let new_w = NEW * s;
        let room = (strip.w - new_w).max(0.0);
        let n = labels.len().max(1) as f32;
        let tab_w = (room / n).clamp(MIN_TAB * s, MAX_TAB * s);

        // More tabs than the strip holds: slide the row so the one in use
        // stays visible. Nothing scrolls by hand yet — this only keeps the
        // active tab from sliding off the edge.
        let total = tab_w * labels.len() as f32;
        let shift = if total <= room {
            0.0
        } else {
            let left = tab_w * active as f32;
            // Enough to bring its right edge in, but never past its left
            // edge, nor past the end of the row.
            (left + tab_w - room).max(0.0).min(left).min(total - room)
        };

        let tabs = labels
            .iter()
            .enumerate()
            .map(|(i, (text, dirty))| {
                let rect = ScreenRect {
                    x: (strip.x + i as f32 * tab_w - shift).round(),
                    y: strip.y,
                    w: tab_w.round(),
                    h: strip.h,
                };
                tab_box(rect, text, *dirty, s, atlas)
            })
            .collect();

        let new = ScreenRect {
            x: (strip.x + (total - shift).min(room)).round(),
            y: strip.y,
            w: new_w,
            h: strip.h,
        };
        Tabs {
            strip,
            tabs,
            new,
            active: active.min(labels.len().saturating_sub(1)),
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<TabHit> {
        if !self.strip.contains(x, y) {
            return None;
        }
        if self.new.contains(x, y) {
            return Some(TabHit::New);
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            if !tab.rect.contains(x, y) {
                continue;
            }
            let on_cross = tab
                .close
                .is_some_and(|c| c.inset(-CROSS_SLOP * self.scale).contains(x, y));
            return Some(if on_cross {
                TabHit::Close(i)
            } else {
                TabHit::Select(i)
            });
        }
        Some(TabHit::Strip)
    }

    /// Paint order: the strip, then each tab's body, then its marks. The
    /// active tab's body is drawn over its neighbours' separators.
    pub fn prims(&self, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let mut out = vec![
            Prim::rect(self.strip, theme.panel),
            // The seam against the canvas, so the strip has a bottom.
            Prim::rect(
                ScreenRect {
                    y: self.strip.y + self.strip.h - b,
                    h: b,
                    ..self.strip
                },
                theme.border,
            ),
        ];

        for (i, tab) in self.tabs.iter().enumerate() {
            let active = i == self.active;
            if active {
                out.push(Prim::rounded(
                    tab.rect.inset(2.0 * s),
                    theme.corner(TAB_RADIUS, s),
                    theme.active_bg,
                ));
            } else if i > 0 && !self.tabs.get(i - 1).is_some_and(|_| i - 1 == self.active) {
                // A hairline between two inactive tabs; the active one's
                // body already separates it from what is beside it.
                let x = tab.rect.x;
                out.push(Prim::segment(
                    (x, tab.rect.y + 8.0 * s),
                    (x, tab.rect.y + tab.rect.h - 8.0 * s),
                    b / 2.0,
                    theme.border,
                ));
            }
            if let Some(dot) = tab.dot {
                let (cx, cy) = dot.center();
                out.push(Prim::circle(cx, cy, DOT_R * s, theme.icon_active));
            }
            let ink = if active { theme.ink } else { theme.icon };
            if !tab.label.is_empty() {
                let baseline = atlas.baseline_in(tab.rect);
                for g in atlas.layout(&tab.label, tab.label_x, baseline) {
                    out.push(Prim::glyph(g.rect, g.uv, slot, ink));
                }
            }
            if let Some(close) = tab.close {
                out.extend(cross(close, CROSS_STROKE * s, theme.icon));
            }
        }

        out.extend(plus(self.new, NEW_ARM * s, NEW_STROKE * s, theme.icon));
        out
    }

    /// Label size in physical px: what the atlas has to be built at.
    pub fn label_px(scale: f64) -> u32 {
        (LABEL * scale as f32).round().max(1.0) as u32
    }
}

/// Measures one tab's contents into its box.
///
/// The label is what gives way when the tab is narrow: a tab you cannot
/// read still has to say whether its work is saved, and still has to
/// close. So the dot and the cross are placed first, and the padding is
/// squeezed toward [`MIN_PAD`] to make room for them before either is
/// given up.
fn tab_box(rect: ScreenRect, text: &str, dirty: bool, s: f32, atlas: &Atlas) -> TabBox {
    let dot_w = if dirty {
        (2.0 * DOT_R + DOT_GAP) * s
    } else {
        0.0
    };
    let marks = dot_w + CROSS * s;
    let pad = (PAD * s)
        .min(((rect.w - marks) / 2.0).max(MIN_PAD * s))
        .max(0.0);

    let mut left = rect.x + pad;
    let mut dot = None;
    if dirty && rect.w - 2.0 * pad >= dot_w {
        dot = Some(ScreenRect {
            x: left,
            y: rect.y,
            w: 2.0 * DOT_R * s,
            h: rect.h,
        });
        left += dot_w;
    }

    let mut close = None;
    let mut right = rect.x + rect.w - pad;
    if right - left >= CROSS * s {
        let size = CROSS * s;
        close = Some(ScreenRect {
            x: right - size,
            y: rect.y + (rect.h - size) / 2.0,
            w: size,
            h: size,
        });
        right -= size + CROSS_GAP * s;
    }

    let label = match right - left {
        room if room > 0.0 => atlas.truncate(text, room),
        _ => String::new(),
    };
    TabBox {
        rect,
        close,
        dot,
        label,
        label_x: left.round(),
    }
}

/// The close mark: two strokes across `r`.
fn cross(r: ScreenRect, half_width: f32, color: Rgba) -> [Prim; 2] {
    let inset = r.inset(r.w * 0.28);
    let (x0, y0) = (inset.x, inset.y);
    let (x1, y1) = (inset.x + inset.w, inset.y + inset.h);
    [
        Prim::segment((x0, y0), (x1, y1), half_width, color),
        Prim::segment((x0, y1), (x1, y0), half_width, color),
    ]
}

/// The new-tab mark: two strokes crossing at the center of `r`.
fn plus(r: ScreenRect, arm: f32, half_width: f32, color: Rgba) -> [Prim; 2] {
    let (cx, cy) = r.center();
    [
        Prim::segment((cx - arm, cy), (cx + arm, cy), half_width, color),
        Prim::segment((cx, cy - arm), (cx, cy + arm), half_width, color),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(1.0))
    }

    fn viewport(w: u32) -> Viewport {
        Viewport { w, h: 800 }
    }

    fn labels(names: &[&str]) -> Vec<(String, bool)> {
        names.iter().map(|n| ((*n).to_owned(), false)).collect()
    }

    fn tabs_of(names: &[&str]) -> Tabs {
        let a = atlas();
        Tabs::layout(viewport(1200), 1.0, &a, &labels(names), 0)
    }

    #[test]
    fn the_strip_spans_the_window_and_sits_at_the_top() {
        let t = tabs_of(&["notes"]);
        assert_eq!(t.strip.x, 0.0);
        assert_eq!(t.strip.y, 0.0);
        assert_eq!(t.strip.w, 1200.0);
        assert_eq!(t.strip.h, HEIGHT);
    }

    #[test]
    fn a_lone_tab_does_not_stretch_across_the_window() {
        // A single project filling 1200 px of tab would read as a title
        // bar, not a tab.
        let t = tabs_of(&["notes"]);
        assert_eq!(t.tabs[0].rect.w, MAX_TAB);
    }

    #[test]
    fn tabs_sit_side_by_side_with_no_gap() {
        let t = tabs_of(&["a", "b", "c"]);
        for pair in t.tabs.windows(2) {
            assert_eq!(pair[0].rect.x + pair[0].rect.w, pair[1].rect.x);
        }
    }

    #[test]
    fn the_new_button_follows_the_last_tab() {
        let t = tabs_of(&["a", "b"]);
        let last = t.tabs.last().unwrap().rect;
        assert_eq!(t.new.x, last.x + last.w);
        assert_eq!(t.new.w, NEW);
    }

    #[test]
    fn many_tabs_share_what_the_strip_has() {
        let names: Vec<&str> = vec!["board"; 12];
        let t = tabs_of(&names);
        assert!(t.tabs[0].rect.w < MAX_TAB);
        // Still wide enough to aim at.
        assert!(t.tabs[0].rect.w >= MIN_TAB);
        let spanned: f32 = t.tabs.iter().map(|b| b.rect.w).sum();
        assert!(spanned + t.new.w <= t.strip.w + 12.0, "{spanned}");
    }

    #[test]
    fn the_active_tab_stays_in_view_when_there_are_too_many() {
        let names: Vec<&str> = vec!["board"; 60];
        let a = atlas();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &labels(&names), 59);
        let last = t.tabs[59].rect;
        assert!(last.x >= t.strip.x, "{last:?}");
        assert!(last.x + last.w <= t.strip.x + t.strip.w, "{last:?}");
    }

    #[test]
    fn the_first_tab_needs_no_shift() {
        let names: Vec<&str> = vec!["board"; 60];
        let a = atlas();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &labels(&names), 0);
        assert_eq!(t.tabs[0].rect.x, 0.0);
    }

    #[test]
    fn clicking_a_tab_selects_it_and_the_cross_closes_it() {
        let t = tabs_of(&["alpha", "beta"]);
        let second = t.tabs[1].rect;
        let (cx, cy) = second.center();
        assert_eq!(t.hit(cx as f64, cy as f64), Some(TabHit::Select(1)));
        let cross = t.tabs[1].close.unwrap();
        let (kx, ky) = cross.center();
        assert_eq!(t.hit(kx as f64, ky as f64), Some(TabHit::Close(1)));
    }

    #[test]
    fn the_cross_is_easier_to_hit_than_it_looks() {
        let t = tabs_of(&["alpha"]);
        let cross = t.tabs[0].close.unwrap();
        // A pixel outside the glyph still closes: a 12 px target would be
        // a coin toss otherwise.
        let x = (cross.x - 2.0) as f64;
        let y = cross.center().1 as f64;
        assert_eq!(t.hit(x, y), Some(TabHit::Close(0)));
    }

    #[test]
    fn the_new_button_answers_for_itself() {
        let t = tabs_of(&["alpha"]);
        let (x, y) = t.new.center();
        assert_eq!(t.hit(x as f64, y as f64), Some(TabHit::New));
    }

    #[test]
    fn empty_strip_chrome_is_swallowed_not_passed_on() {
        let t = tabs_of(&["alpha"]);
        assert_eq!(t.hit(1100.0, 10.0), Some(TabHit::Strip));
    }

    #[test]
    fn a_click_below_the_strip_belongs_to_the_canvas() {
        let t = tabs_of(&["alpha"]);
        assert_eq!(t.hit(100.0, HEIGHT as f64 + 1.0), None);
    }

    #[test]
    fn a_long_name_is_cut_to_fit_its_tab() {
        let a = atlas();
        let long = "a board with a name far too long for any tab";
        let t = Tabs::layout(viewport(1200), 1.0, &a, &labels(&[long]), 0);
        let shown = &t.tabs[0].label;
        assert!(shown.ends_with('…'), "{shown:?}");
        assert!(shown.chars().count() < long.chars().count());
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        let t = tabs_of(&["notes"]);
        assert_eq!(t.tabs[0].label, "notes");
    }

    #[test]
    fn a_narrow_tab_drops_the_label_before_the_marks() {
        let a = atlas();
        // Enough tabs to squeeze every one down to the floor.
        let many: Vec<(String, bool)> = (0..40).map(|i| ("board".to_owned(), i == 3)).collect();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &many, 0);
        assert_eq!(t.tabs[0].rect.w, MIN_TAB);
        for tab in &t.tabs {
            assert_eq!(tab.label, "");
            // Closing a tab you cannot read still has to work, and it
            // still has to say whether its work is saved.
            assert!(tab.close.is_some(), "{tab:?}");
            assert!(tab.rect.contains_rect(&tab.close.unwrap()), "{tab:?}");
        }
        assert!(t.tabs[3].dot.is_some());
        assert!(t.tabs[3].rect.contains_rect(&t.tabs[3].dot.unwrap()));
        // And the dot never sits on top of the cross.
        let dot = t.tabs[3].dot.unwrap();
        let close = t.tabs[3].close.unwrap();
        assert!(dot.x + dot.w <= close.x, "{dot:?} {close:?}");
    }

    #[test]
    fn the_dot_shows_only_for_unsaved_work() {
        let a = atlas();
        let mixed = vec![("saved".to_owned(), false), ("edited".to_owned(), true)];
        let t = Tabs::layout(viewport(1200), 1.0, &a, &mixed, 0);
        assert!(t.tabs[0].dot.is_none());
        assert!(t.tabs[1].dot.is_some());
    }

    #[test]
    fn the_dot_pushes_the_label_along() {
        let a = atlas();
        let pair = vec![("board".to_owned(), false), ("board".to_owned(), true)];
        let t = Tabs::layout(viewport(1200), 1.0, &a, &pair, 0);
        let clean_x = t.tabs[0].label_x - t.tabs[0].rect.x;
        let dirty_x = t.tabs[1].label_x - t.tabs[1].rect.x;
        assert!(dirty_x > clean_x, "{dirty_x} vs {clean_x}");
    }

    #[test]
    fn every_mark_stays_inside_its_tab() {
        let a = atlas();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &[("notes".to_owned(), true)], 0);
        let tab = &t.tabs[0];
        assert!(tab.rect.contains_rect(&tab.close.unwrap()));
        assert!(tab.rect.contains_rect(&tab.dot.unwrap()));
        let end = tab.label_x + a.measure(&tab.label);
        assert!(end <= tab.rect.x + tab.rect.w, "{end}");
    }

    #[test]
    fn the_scale_factor_carries_through() {
        let a = Atlas::build(&Font::bundled(), Tabs::label_px(2.0));
        let t = Tabs::layout(viewport(2400), 2.0, &a, &labels(&["notes"]), 0);
        assert_eq!(t.strip.h, HEIGHT * 2.0);
        assert_eq!(t.tabs[0].rect.w, MAX_TAB * 2.0);
        assert_eq!(t.new.w, NEW * 2.0);
    }

    #[test]
    fn the_label_size_follows_the_display() {
        assert_eq!(Tabs::label_px(1.0), LABEL as u32);
        assert_eq!(Tabs::label_px(2.0), LABEL as u32 * 2);
    }

    #[test]
    fn nothing_is_drawn_outside_the_strip() {
        let a = atlas();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &labels(&["notes", "auth"]), 1);
        for p in t.prims(&a, 0, &Theme::light()) {
            // A box carries origin and size; a segment carries its two
            // ends. Both have to stay off the canvas below.
            let bottom = if p.kind == crate::scene::KIND_SEGMENT {
                p.geom[1].max(p.geom[3])
            } else {
                p.geom[1] + p.geom[3]
            };
            assert!(bottom <= t.strip.h, "{p:?}");
        }
    }

    #[test]
    fn the_active_tab_is_the_only_one_wearing_the_highlight() {
        let a = atlas();
        let theme = Theme::light();
        let t = Tabs::layout(viewport(1200), 1.0, &a, &labels(&["a", "b", "c"]), 1);
        let highlights = t
            .prims(&a, 0, &theme)
            .into_iter()
            .filter(|p| p.color == theme.active_bg)
            .count();
        assert_eq!(highlights, 1);
    }

    #[test]
    fn a_dirty_tab_paints_its_dot_in_the_accent() {
        let a = atlas();
        let theme = Theme::light();
        let one = vec![("board".to_owned(), true)];
        let t = Tabs::layout(viewport(1200), 1.0, &a, &one, 0);
        assert!(
            t.prims(&a, 0, &theme)
                .iter()
                .any(|p| p.color == theme.icon_active)
        );
    }

    #[test]
    fn labels_are_drawn_out_of_the_atlas_slot_they_were_given() {
        let a = atlas();
        let t = tabs_of(&["notes"]);
        let glyphs: Vec<_> = t
            .prims(&a, 5, &Theme::light())
            .into_iter()
            .filter(|p| p.kind == crate::scene::KIND_IMAGE)
            .collect();
        assert_eq!(glyphs.len(), "notes".len());
        assert!(glyphs.iter().all(|p| p.slot == 5));
        // Each takes its own cell, not the whole sheet.
        assert!(glyphs.iter().all(|p| p.uv != [0.0, 0.0, 1.0, 1.0]));
    }
}
