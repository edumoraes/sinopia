//! Brush Properties: the bar above the canvas, and what it opens into.
//!
//! Sketchbook keeps a brush's body in a panel with a Basic tab and an
//! Advanced one. Here it is a bar under the tab strip — the brush's name
//! and the pair it is usually judged by, Size and Opacity — with a
//! chevron that drops the Advanced layout underneath: Pressure, Stamp,
//! Nib, Randomness and Paint, every section's sliders under its own
//! heading.
//! Closed and open never show the same slider twice; opening trades the
//! basic pair for the whole body.
//!
//! A property the canvas does not paint with yet is drawn muted. It is
//! still the brush's own value and it still moves — [`Property::honored`]
//! is the one place that knows, and the bar reads it rather than keeping
//! a list of its own.
//!
//! Pure — `app` asks where a click landed and what to draw.

use crate::brush::{Brush, Property, Section};
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
/// The bar itself: the line that is on show whether it is open or not.
pub const HEIGHT: f32 = 40.0;
/// From the window's left edge, and from the strip above.
pub const MARGIN: f32 = 12.0;
pub const PADDING: f32 = 10.0;
pub const RADIUS: f32 = 12.0;
/// One line of the Advanced layout: a heading, or a slider.
pub const ROW: f32 = 26.0;
/// A field is a label, a track and the number beside it. The label is
/// as wide as the longest property name; the number as wide as the top
/// of the widest range — a test holds both to the font, so neither is
/// ever written with an ellipsis.
const LABEL_W: f32 = 70.0;
const TRACK_W: f32 = 80.0;
const VALUE_W: f32 = 44.0;
const GAP: f32 = 8.0;
const FIELD_W: f32 = LABEL_W + GAP + TRACK_W + GAP + VALUE_W;
/// The brush's name, at the head of the bar.
const NAME_W: f32 = 92.0;
/// The dot beside a name that has been moved off its factory settings.
const DOT: f32 = 5.0;
const TRACK_H: f32 = 4.0;
const KNOB: f32 = 11.0;
/// The chevron's button, at the bar's right end.
const TOGGLE: f32 = 24.0;
const ICON_BOX: f32 = 14.0;
const ICON_STROKE: f32 = 1.5;
/// Between the two columns the Advanced layout stands its sections in.
const COL_GAP: f32 = 18.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;

/// How many of `Section::ALL` stand in the first column; the rest stand
/// in the second. Two columns keep the open bar about as wide as it is
/// closed and short enough for a window of any ordinary height — and a
/// section added later falls into the second column rather than off the
/// bar, which is why this is a cut through the one list and not a list
/// of its own.
const FIRST_COLUMN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A field of the bar, by its place in [`Props::fields`]. It is an
    /// index and not the property, because closed and open lay the same
    /// property out in different places.
    Slider(usize),
    /// The chevron: the Advanced layout drops or folds away.
    Toggle,
    /// Bar chrome: swallowed, never reaches the canvas.
    Bar,
}

/// One property's slider: what it is called, where it runs, and where
/// its number is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub property: Property,
    pub label: ScreenRect,
    pub track: ScreenRect,
    pub value: ScreenRect,
}

/// A section's heading over the fields it owns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heading {
    pub section: Section,
    pub rect: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Props {
    /// Everything the bar covers: the line, plus what it has dropped.
    pub rect: ScreenRect,
    /// The line alone — the name, the fields when it is closed, and the
    /// chevron.
    pub bar: ScreenRect,
    pub name: ScreenRect,
    pub toggle: ScreenRect,
    /// The basic pair when it is closed, the whole body when it is open.
    pub fields: Vec<Field>,
    /// Empty while it is closed.
    pub headings: Vec<Heading>,
    pub open: bool,
    scale: f32,
}

impl Props {
    /// `top` is where the tab strip ends, in physical px.
    pub fn layout(_viewport: Viewport, scale: f64, top: f32, open: bool) -> Props {
        let s = scale as f32;
        let x = (MARGIN * s).round();
        let y = (top + MARGIN * s).round();
        // The same width either way: the line must not jump when the
        // chevron drops what is under it.
        let closed_w = PADDING + NAME_W + GAP + 2.0 * FIELD_W + GAP + TOGGLE + PADDING;
        let open_w = PADDING + 2.0 * FIELD_W + COL_GAP + PADDING;
        let w = closed_w.max(open_w) * s;
        let bar = ScreenRect {
            x,
            y,
            w,
            h: HEIGHT * s,
        };
        let toggle = ScreenRect {
            x: bar.x + bar.w - (PADDING + TOGGLE) * s,
            y: bar.y + (bar.h - TOGGLE * s) / 2.0,
            w: TOGGLE * s,
            h: TOGGLE * s,
        };
        let name = ScreenRect {
            x: bar.x + PADDING * s,
            y: bar.y,
            w: NAME_W * s,
            h: bar.h,
        };

        // A field is laid out from its left edge: label, track, number.
        let field = |property: Property, fx: f32, row: ScreenRect| Field {
            property,
            label: ScreenRect {
                x: fx,
                y: row.y,
                w: LABEL_W * s,
                h: row.h,
            },
            track: ScreenRect {
                x: fx + (LABEL_W + GAP) * s,
                y: row.y + (row.h - TRACK_H * s) / 2.0,
                w: TRACK_W * s,
                h: TRACK_H * s,
            },
            value: ScreenRect {
                x: fx + (LABEL_W + GAP + TRACK_W + GAP) * s,
                y: row.y,
                w: VALUE_W * s,
                h: row.h,
            },
        };

        let (fields, headings, panel_h) = if open {
            let mut fields = Vec::new();
            let mut headings = Vec::new();
            let mut rows: f32 = 0.0;
            let panel_y = bar.y + bar.h;
            let cut = FIRST_COLUMN.min(Section::ALL.len());
            let columns = [&Section::ALL[..cut], &Section::ALL[cut..]];
            for (col, sections) in columns.iter().enumerate() {
                let fx = bar.x + PADDING * s + col as f32 * (FIELD_W + COL_GAP) * s;
                let mut line = 0.0;
                let row = |line: &mut f32| {
                    let r = ScreenRect {
                        x: fx,
                        y: panel_y + *line * ROW * s,
                        w: FIELD_W * s,
                        h: ROW * s,
                    };
                    *line += 1.0;
                    r
                };
                for &section in *sections {
                    headings.push(Heading {
                        section,
                        rect: row(&mut line),
                    });
                    for property in Property::ALL.iter().filter(|p| p.section() == section) {
                        let r = row(&mut line);
                        fields.push(field(*property, fx, r));
                    }
                }
                rows = rows.max(line);
            }
            (fields, headings, PADDING * s + rows * ROW * s + PADDING * s)
        } else {
            let fields = Property::BASIC
                .iter()
                .enumerate()
                .map(|(i, &property)| {
                    let fx = name.x + (NAME_W + GAP) * s + i as f32 * FIELD_W * s;
                    field(property, fx, bar)
                })
                .collect();
            (fields, Vec::new(), 0.0)
        };

        Props {
            rect: ScreenRect {
                h: bar.h + panel_h,
                ..bar
            },
            bar,
            name,
            toggle,
            fields,
            headings,
            open,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.toggle.contains(x, y) {
            return Some(Hit::Toggle);
        }
        // A field is grabbed by the whole of it but its label: the track
        // is a hairline, and the number beside it reads as part of it.
        let grab = |f: &Field| {
            let r = f.track.union(&f.value);
            ScreenRect {
                y: f.value.y,
                h: f.value.h,
                ..r
            }
        };
        let field = self.fields.iter().position(|f| grab(f).contains(x, y));
        Some(match field {
            Some(i) => Hit::Slider(i),
            None => Hit::Bar,
        })
    }

    /// Where `x` lands along the field's track, 0–1. A track lying flat
    /// is full at its right end.
    pub fn fraction(&self, field: usize, x: f64) -> f64 {
        let Some(f) = self.fields.get(field) else {
            return 0.0;
        };
        if f.track.w <= 0.0 {
            return 0.0;
        }
        ((x - f64::from(f.track.x)) / f64::from(f.track.w)).clamp(0.0, 1.0)
    }

    pub fn prims(
        &self,
        name: &str,
        edited: bool,
        brush: &Brush,
        atlas: &Atlas,
        slot: u32,
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

        // The brush's name, with a dot after it while it is off the
        // settings it shipped with — the tab strip's own way of saying
        // there is something here that was not here before.
        let room = self.name.w - if edited { (DOT + GAP) * s } else { 0.0 };
        let text = atlas.truncate(name, room);
        let baseline = atlas.baseline_in(self.name);
        let mut pen = self.name.x;
        for g in atlas.layout(&text, pen, baseline) {
            pen = g.rect.x + g.rect.w;
            out.push(Prim::glyph(g.rect, g.uv, slot, theme.ink).clipped(self.name));
        }
        if edited {
            let d = DOT * s;
            out.push(Prim::rounded(
                ScreenRect {
                    x: pen + GAP * s,
                    y: self.name.y + (self.name.h - d) / 2.0,
                    w: d,
                    h: d,
                },
                d / 2.0,
                theme.icon_active,
            ));
        }

        // The chevron points the way the panel would go: down to drop
        // it, up to fold it back.
        let turned: Vec<Vec<(f32, f32)>> = CHEVRON
            .iter()
            .map(|line| {
                line.iter()
                    .map(|&(x, y)| if self.open { (x, 24.0 - y) } else { (x, y) })
                    .collect()
            })
            .collect();
        let lines: Vec<&[(f32, f32)]> = turned.iter().map(Vec::as_slice).collect();
        out.extend(icon_prims(&lines, self.toggle, s, theme.icon));

        for h in &self.headings {
            let baseline = atlas.baseline_in(h.rect);
            for g in atlas.layout(h.section.label(), h.rect.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, theme.muted).clipped(self.rect));
            }
        }

        for f in &self.fields {
            // A slider the canvas does not answer to yet is drawn in the
            // muted color: it is still the brush's own value, and it
            // still moves, but it does not promise ink.
            let live = f.property.honored();
            let ink = if live { theme.ink } else { theme.muted };
            let fill = if live { theme.icon_active } else { theme.muted };

            let baseline = atlas.baseline_in(f.label);
            let label = atlas.truncate(f.property.label(), f.label.w);
            for g in atlas.layout(&label, f.label.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(self.rect));
            }

            let t = f.track;
            out.push(Prim::rounded(t, t.h / 2.0, theme.border));
            let frac = f.property.fraction(brush) as f32;
            out.push(Prim::rounded(
                ScreenRect {
                    w: frac * t.w,
                    ..t
                },
                t.h / 2.0,
                fill,
            ));
            let side = KNOB * s;
            let knob = ScreenRect {
                x: t.x + frac * t.w - side / 2.0,
                y: t.y + (t.h - side) / 2.0,
                w: side,
                h: side,
            };
            out.push(Prim::rounded(knob.inset(-b), side / 2.0 + b, theme.border));
            out.push(Prim::rounded(knob, side / 2.0, theme.panel));

            let baseline = atlas.baseline_in(f.value);
            let value = f.property.format(f.property.get(brush));
            for g in atlas.layout(&atlas.truncate(&value, f.value.w), f.value.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink).clipped(self.rect));
            }
        }
        out
    }
}

/// A chevron pointing down, to drop the Advanced layout; turned over
/// when it is already down.
const CHEVRON: &[&[(f32, f32)]] = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];

fn icon_prims(lines: &[&[(f32, f32)]], r: ScreenRect, s: f32, color: Rgba) -> Vec<Prim> {
    scene::icon_prims(lines, r, 24.0, ICON_BOX, ICON_STROKE, s, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::Tabs;
    use crate::text::Font;

    const VP: Viewport = Viewport { w: 1200, h: 800 };
    /// Where the tab strip ends, in physical px.
    const TOP: f32 = 34.0;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(1.0))
    }

    fn mid(r: ScreenRect) -> (f64, f64) {
        (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0))
    }

    #[test]
    fn the_bar_floats_under_the_strip_at_the_left() {
        let p = Props::layout(VP, 1.0, TOP, false);
        assert_eq!(p.bar.x, MARGIN);
        assert_eq!(p.bar.y, TOP + MARGIN);
        assert_eq!(p.bar.h, HEIGHT);
        assert_eq!(p.rect, p.bar, "closed, the bar is the whole of it");
        assert!(p.bar.contains_rect(&p.name));
        assert!(p.bar.contains_rect(&p.toggle));
    }

    #[test]
    fn closed_it_carries_the_pair_a_brush_is_judged_by() {
        let p = Props::layout(VP, 1.0, TOP, false);
        assert!(!p.open);
        let carried: Vec<Property> = p.fields.iter().map(|f| f.property).collect();
        assert_eq!(carried, Property::BASIC.to_vec());
        assert!(p.headings.is_empty(), "no sections while it is closed");
        for f in &p.fields {
            assert!(p.bar.contains_rect(&f.track), "{:?} is outside", f.property);
            assert!(f.track.w > f.track.h, "a slider lying flat");
            assert!(f.label.x < f.track.x && f.track.x < f.value.x);
        }
    }

    #[test]
    fn open_it_gives_every_property_a_field_under_its_own_heading() {
        let p = Props::layout(VP, 1.0, TOP, true);
        assert!(p.open);
        let mut carried: Vec<Property> = p.fields.iter().map(|f| f.property).collect();
        carried.sort_by_key(|q| format!("{q:?}"));
        let mut all = Property::ALL.to_vec();
        all.sort_by_key(|q| format!("{q:?}"));
        assert_eq!(carried, all, "the whole body, once each");

        let sections: Vec<Section> = p.headings.iter().map(|h| h.section).collect();
        assert_eq!(sections.len(), Section::ALL.len());
        for s in Section::ALL {
            assert!(sections.contains(&s), "{s:?} has no heading");
        }
        // Every field sits under its own section's heading, in its column.
        for f in &p.fields {
            let h = p
                .headings
                .iter()
                .find(|h| h.section == f.property.section())
                .expect("a heading for every section");
            assert!(f.label.y > h.rect.y, "{:?} is above its heading", f.property);
            assert_eq!(h.rect.x, f.label.x, "{:?} left its column", f.property);
        }
    }

    #[test]
    fn opening_drops_a_panel_without_moving_the_line_above_it() {
        let closed = Props::layout(VP, 1.0, TOP, false);
        let open = Props::layout(VP, 1.0, TOP, true);
        assert_eq!(open.bar, closed.bar, "the line does not move or resize");
        assert_eq!(open.rect.w, closed.rect.w, "and the bar does not jump wider");
        assert!(open.rect.h > closed.rect.h, "it only grows downward");
        assert_eq!(open.rect.y, closed.rect.y);
    }

    #[test]
    fn hit_reports_a_slider_the_chevron_and_the_bar() {
        let p = Props::layout(VP, 1.0, TOP, true);
        let (x, y) = mid(p.toggle);
        assert_eq!(p.hit(x, y), Some(Hit::Toggle));
        for (i, f) in p.fields.iter().enumerate() {
            let (x, y) = mid(f.track);
            assert_eq!(p.hit(x, y), Some(Hit::Slider(i)), "{:?}", f.property);
            // A field is grabbed by its number too, not only its track.
            let (x, y) = mid(f.value);
            assert_eq!(p.hit(x, y), Some(Hit::Slider(i)), "{:?}", f.property);
        }
        let (x, y) = mid(p.name);
        assert_eq!(p.hit(x, y), Some(Hit::Bar));
        assert_eq!(p.hit(2.0, 2.0), None, "outside is canvas");
    }

    #[test]
    fn a_track_lying_flat_is_full_at_the_right() {
        let p = Props::layout(VP, 1.0, TOP, false);
        let t = p.fields[0].track;
        assert_eq!(p.fraction(0, f64::from(t.x)), 0.0);
        assert_eq!(p.fraction(0, f64::from(t.x + t.w)), 1.0);
        let half = p.fraction(0, f64::from(t.x + t.w / 2.0));
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        assert_eq!(p.fraction(0, -900.0), 0.0, "past the left");
        assert_eq!(p.fraction(0, 9000.0), 1.0, "past the right");
        assert_eq!(p.fraction(99, 0.0), 0.0, "a field that is not there");
    }

    #[test]
    fn prims_write_the_name_and_mark_a_brush_that_has_been_moved() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = Props::layout(VP, 1.0, TOP, false);
        let b = Brush::default();

        let plain = p.prims("Marker", false, &b, &atlas, 7, &theme);
        let marked = p.prims("Marker", true, &b, &atlas, 7, &theme);
        assert!(plain[0].feather > 0.0, "the soft shadow goes first");
        assert!(
            plain.iter().any(|q| q.color == theme.panel && q.bounds() == p.rect),
            "bar body"
        );
        assert_eq!(
            marked.len(),
            plain.len() + 1,
            "an edited brush is marked with one dot"
        );
    }

    #[test]
    fn a_property_the_canvas_ignores_is_drawn_muted() {
        let theme = Theme::light();
        let atlas = atlas();
        let p = Props::layout(VP, 1.0, TOP, true);
        let mut b = Brush::default();
        // Something to fill on every track, honored or not.
        for q in Property::ALL {
            q.set_fraction(&mut b, 0.6);
        }
        let prims = p.prims("Marker", false, &b, &atlas, 7, &theme);

        for f in &p.fields {
            let fill = prims
                .iter()
                .filter(|q| f.track.contains_rect(&q.bounds()) && q.color != theme.border)
                .map(|q| q.color)
                .next()
                .unwrap_or_else(|| panic!("{:?} has no filled track", f.property));
            let want = if f.property.honored() {
                theme.icon_active
            } else {
                theme.muted
            };
            assert_eq!(fill, want, "{:?}", f.property);
        }
        assert!(
            p.fields.iter().any(|f| !f.property.honored()),
            "the open bar is where the muted ones live"
        );
    }

    #[test]
    fn nothing_the_bar_draws_escapes_it() {
        let theme = Theme::light();
        let atlas = atlas();
        for open in [false, true] {
            let p = Props::layout(VP, 1.0, TOP, open);
            let room = p.rect.inset(-2.0);
            for q in p.prims("Highlighter", true, &Brush::default(), &atlas, 7, &theme).iter().skip(1) {
                assert!(
                    room.contains_rect(&q.bounds()) || q.clip != scene::NO_CLIP,
                    "open={open}: {:?} is loose outside the bar",
                    q.bounds()
                );
            }
        }
    }

    #[test]
    fn every_name_the_bar_writes_fits_the_room_it_gets() {
        let atlas = atlas();
        let p = Props::layout(VP, 1.0, TOP, true);
        for h in &p.headings {
            let label = h.section.label();
            assert!(
                atlas.measure(label) <= h.rect.w,
                "the heading {label} does not fit {}px",
                h.rect.w
            );
        }
        for f in &p.fields {
            let label = f.property.label();
            assert!(
                atlas.measure(label) <= f.label.w,
                "{label} does not fit its {}px label",
                f.label.w
            );
            // The widest a number can be is the top of its own range.
            let (_, hi) = f.property.range();
            let value = f.property.format(hi);
            assert!(
                atlas.measure(&value) <= f.value.w,
                "{value} does not fit the {}px beside {label}",
                f.value.w
            );
        }
    }

    #[test]
    fn layout_scales_with_the_display() {
        let p = Props::layout(VP, 2.0, TOP, true);
        assert_eq!(p.bar.x, MARGIN * 2.0);
        assert_eq!(p.bar.h, HEIGHT * 2.0);
        assert_eq!(p.fields[0].track.h, TRACK_H * 2.0);
    }

    #[test]
    fn a_window_too_narrow_for_the_bar_still_lays_it_out() {
        let p = Props::layout(Viewport { w: 320, h: 800 }, 1.0, TOP, true);
        assert!(!p.fields.is_empty());
        assert!(p.rect.w > 0.0 && p.rect.h > 0.0);
    }
}
