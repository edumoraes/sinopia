//! Text Properties: the bar a text is set from, in the middle over the
//! canvas as the brush's is.
//!
//! The line carries what a text is judged by at a glance — its kind,
//! artistic or frame; its family; its size; bold, italic, underline and
//! strikethrough; and how its lines stand across it — and a chevron that
//! drops the paragraph's own settings under it: the space between lines,
//! the space between letters, and where a frame stands its lines in its
//! height. What it shows is where the bar is looking — the text being
//! typed, the texts selected, or how the next one will be set — which is
//! the editor's to say; the bar only lays out, hits and draws.
//!
//! Pure — `app` asks where a click landed and what to draw.

use crate::doc::{Align, TextMode, TextStyle, Valign};
use crate::props::{self, HEIGHT, MARGIN};
use crate::scene::{self, Prim, Rgba, ScreenRect, Viewport};
use crate::text::Atlas;
use crate::theme::Theme;

// Logical px.
const PADDING: f32 = 10.0;
const GAP: f32 = 8.0;
/// A square button: a kind, a style, an alignment, the chevron.
const BUTTON: f32 = 26.0;
/// The family's button, which opens the font menu.
const FONT_W: f32 = 150.0;
const RADIUS: f32 = 12.0;
const BUTTON_RADIUS: f32 = 6.0;
/// A slider: its label, its track and its number, as the brush's are.
const LABEL_W: f32 = 44.0;
const TRACK_W: f32 = 80.0;
const VALUE_W: f32 = 44.0;
const FIELD_W: f32 = LABEL_W + GAP + TRACK_W + GAP + VALUE_W;
/// A line of what the chevron drops.
const ROW: f32 = 30.0;
/// The label before the vertical alignment's buttons.
const VALIGN_W: f32 = 64.0;
const TRACK_H: f32 = 4.0;
const KNOB: f32 = 11.0;
const ICON_BOX: f32 = 16.0;
const ICON_STROKE: f32 = 1.5;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_FEATHER: f32 = 14.0;

/// The sizes the Size slider runs over. A text can be set past either end
/// — scaling artistic text does it — and the knob then rests at the end.
const SIZE_MIN: f64 = 4.0;
const SIZE_MAX: f64 = 400.0;
const LEADING_MIN: f64 = 0.8;
const LEADING_MAX: f64 = 3.0;
const TRACKING_MIN: f64 = -200.0;
const TRACKING_MAX: f64 = 1000.0;

/// A value the bar sets by dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slider {
    Size,
    /// Line spacing: the leading.
    Leading,
    /// Letter spacing: the tracking.
    Tracking,
}

impl Slider {
    pub fn label(self) -> &'static str {
        match self {
            Slider::Size => "Size",
            Slider::Leading => "Line",
            Slider::Tracking => "Letter",
        }
    }

    /// Where `style` puts the knob, 0–1. Size runs along a cube, as the
    /// brush's does: 4–400 laid out straight would spend nine tenths of
    /// the track above the sizes anyone sets text in.
    pub fn fraction(self, style: &TextStyle) -> f64 {
        let f = match self {
            Slider::Size => ((style.size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN)).max(0.0).cbrt(),
            Slider::Leading => (style.leading - LEADING_MIN) / (LEADING_MAX - LEADING_MIN),
            Slider::Tracking => (style.tracking - TRACKING_MIN) / (TRACKING_MAX - TRACKING_MIN),
        };
        f.clamp(0.0, 1.0)
    }

    /// Sets the value the knob at `f` stands for: a whole size, a
    /// leading to the hundredth, a whole thousandth of an em.
    pub fn set(self, style: &mut TextStyle, f: f64) {
        let f = f.clamp(0.0, 1.0);
        match self {
            Slider::Size => style.size = (SIZE_MIN + (SIZE_MAX - SIZE_MIN) * f.powi(3)).round(),
            Slider::Leading => {
                style.leading = ((LEADING_MIN + (LEADING_MAX - LEADING_MIN) * f) * 100.0).round() / 100.0;
            }
            Slider::Tracking => style.tracking = (TRACKING_MIN + (TRACKING_MAX - TRACKING_MIN) * f).round(),
        }
    }

    /// The number written beside the track.
    pub fn format(self, style: &TextStyle) -> String {
        match self {
            Slider::Size => format!("{}", (style.size * 10.0).round() / 10.0),
            Slider::Leading => format!("{:.2}", style.leading),
            Slider::Tracking => format!("{}", style.tracking.round()),
        }
    }
}

/// One of the four toggles a text's letters wear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Bold,
    Italic,
    Underline,
    Strike,
}

impl Toggle {
    pub const ALL: [Toggle; 4] = [Toggle::Bold, Toggle::Italic, Toggle::Underline, Toggle::Strike];

    pub fn on(self, style: &TextStyle) -> bool {
        match self {
            Toggle::Bold => style.bold,
            Toggle::Italic => style.italic,
            Toggle::Underline => style.underline,
            Toggle::Strike => style.strike,
        }
    }

    /// Turns it the other way.
    pub fn flip(self, style: &mut TextStyle) {
        let on = self.on(style);
        self.set(style, !on);
    }

    /// Turns it on or off: what a press on the bar does to every text it
    /// is looking at, all of them the same way.
    pub fn set(self, style: &mut TextStyle, on: bool) {
        let at = match self {
            Toggle::Bold => &mut style.bold,
            Toggle::Italic => &mut style.italic,
            Toggle::Underline => &mut style.underline,
            Toggle::Strike => &mut style.strike,
        };
        *at = on;
    }
}

const ALIGNS: [Align; 4] = [Align::Left, Align::Center, Align::Right, Align::Justify];
const VALIGNS: [Valign; 3] = [Valign::Top, Valign::Middle, Valign::Bottom];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Kind(TextMode),
    /// The family's button: the font menu opens under it.
    Font,
    Toggle(Toggle),
    Align(Align),
    Valign(Valign),
    Slider(Slider),
    /// The chevron: the paragraph's settings drop or fold away.
    More,
    /// Bar chrome: swallowed, never reaches the canvas.
    Bar,
}

/// A slider's three parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub slider: Slider,
    pub label: ScreenRect,
    pub track: ScreenRect,
    pub value: ScreenRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextBar {
    /// Everything the bar covers: the line, and what it has dropped.
    pub rect: ScreenRect,
    pub bar: ScreenRect,
    pub kinds: Vec<(TextMode, ScreenRect)>,
    pub font: ScreenRect,
    pub toggles: Vec<(Toggle, ScreenRect)>,
    pub aligns: Vec<(Align, ScreenRect)>,
    /// Empty while it is closed.
    pub valigns: Vec<(Valign, ScreenRect)>,
    /// The label of the vertical alignment, while it is open.
    pub valign_label: Option<ScreenRect>,
    pub fields: Vec<Field>,
    pub more: ScreenRect,
    pub open: bool,
    scale: f32,
}

/// What [`TextBar::prims`] draws the bar as showing.
pub struct Showing<'a> {
    pub style: &'a TextStyle,
    pub kind: TextMode,
}

impl TextBar {
    /// `top` is where the tab strip ends, in physical px.
    pub fn layout(viewport: Viewport, scale: f64, top: f32, open: bool) -> TextBar {
        let s = scale as f32;
        let w = (PADDING
            + 2.0 * BUTTON
            + GAP
            + FONT_W
            + GAP
            + FIELD_W
            + GAP
            + 4.0 * BUTTON
            + GAP
            + 4.0 * BUTTON
            + GAP
            + BUTTON
            + PADDING)
            * s;
        let x = props::centred(viewport, w, s);
        let bar = ScreenRect {
            x,
            y: (top + MARGIN * s).round(),
            w,
            h: HEIGHT * s,
        };
        let button = |at: f32| ScreenRect {
            x: at,
            y: bar.y + (bar.h - BUTTON * s) / 2.0,
            w: BUTTON * s,
            h: BUTTON * s,
        };
        let mut pen = bar.x + PADDING * s;
        let mut run = |n: usize, width: f32| {
            let start = pen;
            pen += n as f32 * width * s + GAP * s;
            start
        };
        let kinds_x = run(2, BUTTON);
        let font_x = run(1, FONT_W);
        let size_x = run(1, FIELD_W);
        let toggles_x = run(4, BUTTON);
        let aligns_x = run(4, BUTTON);
        let more = button(bar.x + bar.w - (PADDING + BUTTON) * s);

        let field = |slider: Slider, fx: f32, row: ScreenRect| Field {
            slider,
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
        let mut fields = vec![field(Slider::Size, size_x, bar)];
        let (mut valigns, mut valign_label, mut dropped) = (Vec::new(), None, 0.0);
        if open {
            let row = |k: f32| ScreenRect {
                x: bar.x + PADDING * s,
                y: bar.y + bar.h + (PADDING / 2.0 + k * ROW) * s,
                w: bar.w - 2.0 * PADDING * s,
                h: ROW * s,
            };
            let first = row(0.0);
            fields.push(field(Slider::Leading, first.x, first));
            fields.push(field(Slider::Tracking, first.x + (FIELD_W + 2.0 * GAP) * s, first));
            let second = row(1.0);
            valign_label = Some(ScreenRect {
                w: VALIGN_W * s,
                ..second
            });
            let vx = second.x + (VALIGN_W + GAP) * s;
            valigns = VALIGNS
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    (
                        v,
                        ScreenRect {
                            x: vx + i as f32 * BUTTON * s,
                            y: second.y + (second.h - BUTTON * s) / 2.0,
                            w: BUTTON * s,
                            h: BUTTON * s,
                        },
                    )
                })
                .collect();
            dropped = (PADDING + 2.0 * ROW) * s;
        }
        TextBar {
            rect: ScreenRect {
                h: bar.h + dropped,
                ..bar
            },
            bar,
            kinds: [TextMode::Artistic, TextMode::Frame]
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, button(kinds_x + i as f32 * BUTTON * s)))
                .collect(),
            font: ScreenRect {
                x: font_x,
                y: bar.y + (bar.h - BUTTON * s) / 2.0,
                w: FONT_W * s,
                h: BUTTON * s,
            },
            toggles: Toggle::ALL
                .iter()
                .enumerate()
                .map(|(i, &t)| (t, button(toggles_x + i as f32 * BUTTON * s)))
                .collect(),
            aligns: ALIGNS
                .iter()
                .enumerate()
                .map(|(i, &a)| (a, button(aligns_x + i as f32 * BUTTON * s)))
                .collect(),
            valigns,
            valign_label,
            fields,
            more,
            open,
            scale: s,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.more.contains(x, y) {
            return Some(Hit::More);
        }
        if self.font.contains(x, y) {
            return Some(Hit::Font);
        }
        fn found<K: Copy>(rects: &[(K, ScreenRect)], x: f64, y: f64) -> Option<K> {
            rects.iter().find(|(_, r)| r.contains(x, y)).map(|(k, _)| *k)
        }
        if let Some(k) = found(&self.kinds, x, y) {
            return Some(Hit::Kind(k));
        }
        if let Some(t) = found(&self.toggles, x, y) {
            return Some(Hit::Toggle(t));
        }
        if let Some(a) = found(&self.aligns, x, y) {
            return Some(Hit::Align(a));
        }
        if let Some(v) = found(&self.valigns, x, y) {
            return Some(Hit::Valign(v));
        }
        // A slider is grabbed by its track and its number together, as
        // the brush's are.
        let grab = |f: &Field| {
            let r = f.track.union(&f.value);
            ScreenRect {
                y: f.value.y,
                h: f.value.h,
                ..r
            }
        };
        Some(match self.fields.iter().find(|f| grab(f).contains(x, y)) {
            Some(f) => Hit::Slider(f.slider),
            None => Hit::Bar,
        })
    }

    /// Where `x` lands along a slider's track, 0–1.
    pub fn fraction(&self, slider: Slider, x: f64) -> f64 {
        let Some(f) = self.fields.iter().find(|f| f.slider == slider) else {
            return 0.0;
        };
        if f.track.w <= 0.0 {
            return 0.0;
        }
        ((x - f64::from(f.track.x)) / f64::from(f.track.w)).clamp(0.0, 1.0)
    }

    pub fn prims(&self, showing: &Showing, atlas: &Atlas, slot: u32, theme: &Theme) -> Vec<Prim> {
        let s = self.scale;
        let b = theme.edge(s);
        let corner = theme.corner(RADIUS, s);
        let style = showing.style;
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
        let lit = |r: ScreenRect| Prim::rounded(r, theme.corner(BUTTON_RADIUS, s), theme.active_bg);
        let text = |out: &mut Vec<Prim>, words: &str, r: ScreenRect, color: Rgba| {
            let baseline = atlas.baseline_in(r);
            let cut = atlas.truncate(words, r.w);
            for g in atlas.layout(&cut, r.x, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, color).clipped(self.rect));
            }
        };

        for (k, r) in &self.kinds {
            let on = *k == showing.kind;
            if on {
                out.push(lit(*r));
            }
            let icon = match k {
                TextMode::Artistic => ARTISTIC,
                TextMode::Frame => FRAME,
            };
            out.extend(icon_prims(icon, *r, s, if on { theme.icon_active } else { theme.icon }));
        }

        // The family's name in a button that opens its menu, with the
        // chevron that says so at its right end.
        out.push(Prim::rounded(self.font.inset(-b), theme.corner(BUTTON_RADIUS, s) + b, theme.border));
        out.push(Prim::rounded(self.font, theme.corner(BUTTON_RADIUS, s), theme.panel));
        let pad = GAP * s;
        let arrow = ScreenRect {
            x: self.font.x + self.font.w - self.font.h,
            w: self.font.h,
            ..self.font
        };
        let name = ScreenRect {
            x: self.font.x + pad,
            w: self.font.w - pad - arrow.w,
            ..self.font
        };
        text(&mut out, &style.font, name, theme.ink);
        out.extend(icon_prims(CHEVRON, arrow, s, theme.icon));

        for (t, r) in &self.toggles {
            let on = t.on(style);
            if on {
                out.push(lit(*r));
            }
            let icon = match t {
                Toggle::Bold => BOLD,
                Toggle::Italic => ITALIC,
                Toggle::Underline => UNDERLINE,
                Toggle::Strike => STRIKE,
            };
            out.extend(icon_prims(icon, *r, s, if on { theme.icon_active } else { theme.icon }));
        }
        for (a, r) in &self.aligns {
            let on = *a == style.align;
            if on {
                out.push(lit(*r));
            }
            out.extend(icon_prims(align_icon(*a), *r, s, if on { theme.icon_active } else { theme.icon }));
        }
        // Where a frame stands its lines is a frame's question: for
        // artistic text, which is as tall as its lines, it is asked of
        // nothing, and the buttons say so.
        let frame = showing.kind == TextMode::Frame;
        if let Some(label) = self.valign_label {
            text(&mut out, "Vertical", label, if frame { theme.ink } else { theme.muted });
        }
        for (v, r) in &self.valigns {
            let on = frame && *v == style.valign;
            if on {
                out.push(lit(*r));
            }
            let color = match (frame, on) {
                (false, _) => theme.muted,
                (true, true) => theme.icon_active,
                (true, false) => theme.icon,
            };
            out.extend(icon_prims(valign_icon(*v), *r, s, color));
        }

        for f in &self.fields {
            text(&mut out, f.slider.label(), f.label, theme.ink);
            let t = f.track;
            out.push(Prim::rounded(t, t.h / 2.0, theme.border));
            let frac = f.slider.fraction(style) as f32;
            out.push(Prim::rounded(
                ScreenRect {
                    w: frac * t.w,
                    ..t
                },
                t.h / 2.0,
                theme.icon_active,
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
            text(&mut out, &f.slider.format(style), f.value, theme.ink);
        }

        let turned: Vec<Vec<(f32, f32)>> = CHEVRON
            .iter()
            .map(|line| {
                line.iter()
                    .map(|&(x, y)| if self.open { (x, 24.0 - y) } else { (x, y) })
                    .collect()
            })
            .collect();
        let lines: Vec<&[(f32, f32)]> = turned.iter().map(Vec::as_slice).collect();
        out.extend(icon_prims(&lines, self.more, s, theme.icon));
        out
    }
}

type Icon = &'static [&'static [(f32, f32)]];

/// Artistic text: a letter A, set at a point.
const ARTISTIC: Icon = &[&[(5.0, 20.0), (12.0, 4.0), (19.0, 20.0)], &[(8.0, 14.0), (16.0, 14.0)]];
/// Frame text: lines inside a box.
const FRAME: Icon = &[
    &[(4.0, 4.0), (20.0, 4.0), (20.0, 20.0), (4.0, 20.0), (4.0, 4.0)],
    &[(7.5, 9.0), (16.5, 9.0)],
    &[(7.5, 12.5), (16.5, 12.5)],
    &[(7.5, 16.0), (13.0, 16.0)],
];
const BOLD: Icon = &[
    &[(7.0, 4.0), (7.0, 20.0)],
    &[(7.0, 4.0), (13.0, 4.0), (16.0, 5.5), (16.5, 8.0), (16.0, 10.5), (13.0, 12.0), (7.0, 12.0)],
    &[(13.0, 12.0), (16.5, 13.5), (17.5, 16.0), (16.5, 18.5), (13.0, 20.0), (7.0, 20.0)],
];
const ITALIC: Icon = &[&[(10.0, 4.0), (18.0, 4.0)], &[(6.0, 20.0), (14.0, 20.0)], &[(14.0, 4.0), (10.0, 20.0)]];
const UNDERLINE: Icon = &[
    &[(7.0, 3.5), (7.0, 12.0), (8.5, 15.5), (12.0, 17.0), (15.5, 15.5), (17.0, 12.0), (17.0, 3.5)],
    &[(5.0, 21.0), (19.0, 21.0)],
];
const STRIKE: Icon = &[
    &[
        (17.0, 6.5),
        (15.0, 4.5),
        (12.0, 4.0),
        (8.5, 5.0),
        (7.0, 7.5),
        (8.5, 10.5),
        (12.0, 12.0),
        (15.5, 13.5),
        (17.0, 16.5),
        (15.5, 19.0),
        (12.0, 20.0),
        (9.0, 19.5),
        (7.0, 17.5),
    ],
    &[(4.0, 12.0), (20.0, 12.0)],
];
const CHEVRON: Icon = &[&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]];

/// Four lines standing as `a` stands them.
fn align_icon(a: Align) -> Icon {
    match a {
        Align::Left => &[&[(4.0, 6.0), (20.0, 6.0)], &[(4.0, 10.0), (14.0, 10.0)], &[(4.0, 14.0), (20.0, 14.0)], &[(4.0, 18.0), (12.0, 18.0)]],
        Align::Center => &[&[(4.0, 6.0), (20.0, 6.0)], &[(7.0, 10.0), (17.0, 10.0)], &[(4.0, 14.0), (20.0, 14.0)], &[(8.0, 18.0), (16.0, 18.0)]],
        Align::Right => &[&[(4.0, 6.0), (20.0, 6.0)], &[(10.0, 10.0), (20.0, 10.0)], &[(4.0, 14.0), (20.0, 14.0)], &[(12.0, 18.0), (20.0, 18.0)]],
        Align::Justify => &[&[(4.0, 6.0), (20.0, 6.0)], &[(4.0, 10.0), (20.0, 10.0)], &[(4.0, 14.0), (20.0, 14.0)], &[(4.0, 18.0), (12.0, 18.0)]],
    }
}

/// Two lines standing in a box where `v` stands them.
fn valign_icon(v: Valign) -> Icon {
    match v {
        Valign::Top => &[&[(4.0, 4.0), (20.0, 4.0)], &[(7.0, 8.5), (17.0, 8.5)], &[(7.0, 12.0), (17.0, 12.0)], &[(4.0, 20.0), (20.0, 20.0)]],
        Valign::Middle => &[&[(4.0, 4.0), (20.0, 4.0)], &[(7.0, 10.0), (17.0, 10.0)], &[(7.0, 14.0), (17.0, 14.0)], &[(4.0, 20.0), (20.0, 20.0)]],
        Valign::Bottom => &[&[(4.0, 4.0), (20.0, 4.0)], &[(7.0, 12.0), (17.0, 12.0)], &[(7.0, 15.5), (17.0, 15.5)], &[(4.0, 20.0), (20.0, 20.0)]],
    }
}

fn icon_prims(lines: &[&[(f32, f32)]], r: ScreenRect, s: f32, color: Rgba) -> Vec<Prim> {
    scene::icon_prims(lines, r, 24.0, ICON_BOX, ICON_STROKE, s, color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::Tabs;
    use crate::text::Font;

    const VP: Viewport = Viewport { w: 1400, h: 800 };
    const TOP: f32 = 34.0;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(crate::tabs::LABEL, 1.0))
    }

    fn mid(r: ScreenRect) -> (f64, f64) {
        (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0))
    }

    fn style() -> TextStyle {
        TextStyle::default()
    }

    fn showing(style: &TextStyle, kind: TextMode) -> Showing<'_> {
        Showing { style, kind }
    }

    #[test]
    fn the_bar_stands_in_the_middle_under_the_strip() {
        let bar = TextBar::layout(VP, 1.0, TOP, false);
        let middle = bar.bar.x + bar.bar.w / 2.0;
        assert!((middle - VP.w as f32 / 2.0).abs() <= 0.5);
        assert_eq!(bar.bar.y, TOP + MARGIN);
        assert_eq!(bar.bar.h, HEIGHT);
        assert_eq!(bar.rect, bar.bar, "closed, the line is the whole of it");
    }

    #[test]
    fn closed_it_carries_what_a_text_is_judged_by() {
        let bar = TextBar::layout(VP, 1.0, TOP, false);
        assert_eq!(bar.kinds.len(), 2);
        assert_eq!(bar.toggles.len(), 4);
        assert_eq!(bar.aligns.len(), 4);
        assert!(bar.valigns.is_empty());
        let sliders: Vec<Slider> = bar.fields.iter().map(|f| f.slider).collect();
        assert_eq!(sliders, [Slider::Size]);
        let everything = bar
            .kinds
            .iter()
            .map(|(_, r)| *r)
            .chain([bar.font, bar.more])
            .chain(bar.toggles.iter().map(|(_, r)| *r))
            .chain(bar.aligns.iter().map(|(_, r)| *r));
        let rects: Vec<ScreenRect> = everything.collect();
        for (i, r) in rects.iter().enumerate() {
            assert!(bar.bar.contains_rect(r), "{r:?} is outside");
            for q in &rects[i + 1..] {
                assert!(r.intersect(q).is_none(), "{r:?} overlaps {q:?}");
            }
        }
    }

    #[test]
    fn opening_drops_the_paragraph_without_moving_the_line() {
        let closed = TextBar::layout(VP, 1.0, TOP, false);
        let open = TextBar::layout(VP, 1.0, TOP, true);
        assert_eq!(open.bar, closed.bar);
        assert_eq!(open.font, closed.font);
        assert!(open.rect.h > closed.rect.h);
        let sliders: Vec<Slider> = open.fields.iter().map(|f| f.slider).collect();
        assert_eq!(sliders, [Slider::Size, Slider::Leading, Slider::Tracking]);
        assert_eq!(open.valigns.len(), 3);
        for f in &open.fields[1..] {
            assert!(f.track.y > open.bar.y + open.bar.h, "under the line");
            assert!(open.rect.contains_rect(&f.value));
        }
        for (_, r) in &open.valigns {
            assert!(open.rect.contains_rect(r));
        }
    }

    #[test]
    fn hit_names_every_control() {
        let bar = TextBar::layout(VP, 1.0, TOP, true);
        let at = |r: ScreenRect| {
            let (x, y) = mid(r);
            bar.hit(x, y)
        };
        assert_eq!(at(bar.kinds[1].1), Some(Hit::Kind(TextMode::Frame)));
        assert_eq!(at(bar.font), Some(Hit::Font));
        assert_eq!(at(bar.toggles[2].1), Some(Hit::Toggle(Toggle::Underline)));
        assert_eq!(at(bar.aligns[3].1), Some(Hit::Align(Align::Justify)));
        assert_eq!(at(bar.valigns[1].1), Some(Hit::Valign(Valign::Middle)));
        assert_eq!(at(bar.fields[0].track), Some(Hit::Slider(Slider::Size)));
        assert_eq!(at(bar.fields[2].value), Some(Hit::Slider(Slider::Tracking)));
        assert_eq!(at(bar.more), Some(Hit::More));
        assert_eq!(bar.hit(f64::from(bar.bar.x + 2.0), f64::from(bar.bar.y + 2.0)), Some(Hit::Bar));
        assert_eq!(bar.hit(1.0, 700.0), None);
    }

    #[test]
    fn a_slider_sets_what_its_knob_stands_for() {
        let mut st = style();
        for slider in [Slider::Size, Slider::Leading, Slider::Tracking] {
            slider.set(&mut st, 0.0);
            assert_eq!(slider.fraction(&st), 0.0, "{slider:?}");
            slider.set(&mut st, 1.0);
            assert_eq!(slider.fraction(&st), 1.0, "{slider:?}");
        }
        assert_eq!(st.size, 400.0);
        assert_eq!(st.leading, 3.0);
        assert_eq!(st.tracking, 1000.0);
        Slider::Size.set(&mut st, 0.5);
        assert!(st.size < 60.0, "most of the track is the sizes text is set in: {}", st.size);
        st.size = 24.0;
        let f = Slider::Size.fraction(&st);
        Slider::Size.set(&mut st, f);
        assert_eq!(st.size, 24.0, "a size set where it stands stays");
        st.size = 5000.0;
        assert_eq!(Slider::Size.fraction(&st), 1.0, "past the end rests at the end");
        let bar = TextBar::layout(VP, 1.0, TOP, false);
        let t = bar.fields[0].track;
        assert_eq!(bar.fraction(Slider::Size, f64::from(t.x + t.w)), 1.0);
        assert_eq!(bar.fraction(Slider::Size, f64::from(t.x) - 50.0), 0.0);
    }

    #[test]
    fn a_toggle_flips_and_says_where_it_stands() {
        let mut st = style();
        for t in Toggle::ALL {
            assert!(!t.on(&st));
            t.flip(&mut st);
            assert!(t.on(&st));
        }
        assert!(st.bold && st.italic && st.underline && st.strike);
    }

    #[test]
    fn what_is_on_is_lit() {
        let bar = TextBar::layout(VP, 1.0, TOP, true);
        let theme = Theme::light();
        let plain = style();
        let mut bold = style();
        bold.bold = true;
        let count = |st: &TextStyle, kind| {
            bar.prims(&showing(st, kind), &atlas(), 3, &theme)
                .iter()
                .filter(|p| p.color == theme.active_bg)
                .count()
        };
        // The kind and the alignment are always lit; the toggles when on.
        assert_eq!(count(&plain, TextMode::Artistic), 2);
        assert_eq!(count(&bold, TextMode::Artistic), 3);
        // A frame lights where it stands its lines too.
        assert_eq!(count(&plain, TextMode::Frame), 3);
    }

    #[test]
    fn a_frames_question_is_muted_for_artistic_text() {
        let bar = TextBar::layout(VP, 1.0, TOP, true);
        let theme = Theme::light();
        let st = style();
        let prims = bar.prims(&showing(&st, TextMode::Artistic), &atlas(), 3, &theme);
        let in_valign = |p: &Prim| bar.valigns.iter().any(|(_, r)| r.contains_rect(&p.bounds()));
        assert!(prims.iter().filter(|p| in_valign(p)).all(|p| p.color == theme.muted));
    }

    #[test]
    fn the_family_is_written_in_its_button() {
        let bar = TextBar::layout(VP, 1.0, TOP, false);
        let theme = Theme::light();
        let st = style();
        let glyphs = bar
            .prims(&showing(&st, TextMode::Artistic), &atlas(), 3, &theme)
            .into_iter()
            .filter(|p| p.kind == scene::KIND_IMAGE && p.slot == 3)
            .filter(|p| bar.font.contains_rect(&p.bounds()))
            .count();
        assert_eq!(glyphs, "LiberationSans".len());
    }

    #[test]
    fn nothing_the_bar_draws_escapes_it() {
        let theme = Theme::light();
        let st = style();
        for open in [false, true] {
            let bar = TextBar::layout(VP, 1.0, TOP, open);
            for p in bar.prims(&showing(&st, TextMode::Frame), &atlas(), 3, &theme).iter().skip(3) {
                let r = p.bounds();
                assert!(bar.rect.inset(-1.0).contains_rect(&r), "{r:?} past {:?}", bar.rect);
            }
        }
    }

    #[test]
    fn every_label_fits_the_room_it_gets() {
        let a = atlas();
        for slider in [Slider::Size, Slider::Leading, Slider::Tracking] {
            assert!(a.measure(slider.label()) <= LABEL_W, "{slider:?}");
        }
        assert!(a.measure("Vertical") <= VALIGN_W);
        let mut st = style();
        st.size = 399.5;
        st.tracking = -200.0;
        for slider in [Slider::Size, Slider::Leading, Slider::Tracking] {
            let v = slider.format(&st);
            assert!(a.measure(&v) <= VALUE_W, "{v}");
        }
    }

    #[test]
    fn layout_scales_with_the_display() {
        let one = TextBar::layout(VP, 1.0, TOP, true);
        let two = TextBar::layout(Viewport { w: 2800, h: 1600 }, 2.0, TOP, true);
        assert_eq!(two.bar.w, one.bar.w * 2.0);
        assert_eq!(two.font.w, one.font.w * 2.0);
    }
}
