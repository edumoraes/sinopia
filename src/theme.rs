//! Palette. Light by default — the reference look: off-white canvas, subtle
//! dot grid, near-black ink, white dock — and re-derived from the three
//! colors the plugin sends with `op: theme` (§5).

use crate::scene::{Rgba, mix, parse_color, to_hex, try_parse_color, with_alpha};

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub bg: Rgba,
    pub dot: Rgba,
    pub ink: Rgba,
    /// Ink as written into new path elements (always a valid hex).
    pub ink_hex: String,
    pub panel: Rgba,
    /// The panel colour as a document can hold it — what a new frame's
    /// background is set to (always a valid hex).
    pub panel_hex: String,
    pub border: Rgba,
    pub shadow: Rgba,
    pub icon: Rgba,
    pub icon_active: Rgba,
    /// A control that is off — a hidden layer's eye.
    pub muted: Rgba,
    pub active_bg: Rgba,
    /// Selection outline, handles' border, marquee.
    pub selection: Rgba,
    /// The outline of a card the pointer is carrying. A fixed blue, not
    /// derived: it says "in flight", and it has to mean that against
    /// whatever three colors the plugin sent.
    pub lifted: Rgba,
    /// Handle body.
    pub handle: Rgba,
    /// The inks the dock offers, as hex — the theme's own first, then
    /// the colours a board is marked up in. See [`INKS`].
    pub inks: Vec<String>,
}

const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];
const BLACK: Rgba = [0.0, 0.0, 0.0, 1.0];
/// Hex written to the document when the requested ink does not parse.
const FALLBACK_INK: &str = "#808080";
/// What a card being dragged is outlined in, on any theme.
const LIFTED: &str = "#3b82f6";

/// The colours a board is marked up in, after the theme's own ink. Fixed
/// and not derived: a red drawn out of whatever three colors the plugin
/// sent might not be red, and a person reaching for the red one means
/// red. Enough of them to tell things apart, few enough to fit under
/// the tools without a picker.
pub const INKS: [&str; 5] = ["#e5484d", "#f5a524", "#30a46c", "#3b82f6", "#8e4ec6"];

impl Theme {
    pub fn light() -> Theme {
        Theme::from_hex("#f5f5f4", "#1f1f1f", "#2f6b3d")
    }

    /// Derives the whole palette from canvas, foreground and accent.
    pub fn from_hex(bg: &str, fg: &str, accent: &str) -> Theme {
        let bg_c = parse_color(bg);
        let ink_hex = match try_parse_color(fg) {
            Some(_) => fg.to_owned(),
            None => FALLBACK_INK.to_owned(),
        };
        let ink = parse_color(&ink_hex);
        let ink_hex_for_inks = ink_hex.clone();
        let light = luminance(bg_c) > 0.5;
        // The dock floats above the canvas: lighter than it on both kinds
        // of theme (toward white on light, toward the ink on dark).
        let panel = if light {
            mix(bg_c, WHITE, 0.85)
        } else {
            mix(bg_c, ink, 0.08)
        };
        Theme {
            bg: bg_c,
            dot: mix(bg_c, ink, 0.25),
            ink,
            ink_hex,
            panel,
            panel_hex: to_hex(panel),
            border: mix(panel, ink, 0.12),
            shadow: with_alpha(BLACK, if light { 0.16 } else { 0.5 }),
            icon: mix(ink, bg_c, 0.15),
            icon_active: parse_color(accent),
            muted: mix(panel, ink, 0.35),
            active_bg: mix(panel, ink, 0.09),
            selection: parse_color(accent),
            lifted: parse_color(LIFTED),
            handle: panel,
            inks: std::iter::once(ink_hex_for_inks)
                .chain(INKS.iter().map(|&s| s.to_owned()))
                .collect(),
        }
    }
}

fn luminance(c: Rgba) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{parse_color, try_parse_color};

    fn luminance(c: [f32; 4]) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    fn between(x: f32, a: f32, b: f32) -> bool {
        (a.min(b) < x) && (x < a.max(b))
    }

    #[test]
    fn light_theme_matches_the_reference_palette() {
        let t = Theme::light();
        assert_eq!(t.bg, parse_color("#f5f5f4"));
        assert_eq!(t.ink_hex, "#1f1f1f");
        assert_eq!(t.ink, parse_color("#1f1f1f"));
        assert!(
            luminance(t.panel) > luminance(t.bg),
            "dock panel pops off the canvas"
        );
        for ch in 0..3 {
            assert!(
                between(t.dot[ch], t.bg[ch], t.ink[ch]),
                "dots are a tint of the ink"
            );
        }
        assert!(t.shadow[3] < 1.0, "shadow is translucent");
        assert_eq!(t.bg[3], 1.0, "canvas is opaque");
    }

    #[test]
    fn a_card_in_flight_is_the_same_blue_on_any_theme() {
        let dark = Theme::from_hex("#1a1a1a", "#eee", "#7aa");
        assert_eq!(Theme::light().lifted, parse_color("#3b82f6"));
        assert_eq!(dark.lifted, Theme::light().lifted);
        assert_ne!(dark.lifted, dark.selection, "not the plugin's accent");
    }

    #[test]
    fn plugin_colors_derive_a_consistent_dark_palette() {
        // The three colors of `op: theme` (§5) drive everything else.
        let t = Theme::from_hex("#1a1a1a", "#eee", "#7aa");
        assert_eq!(t.bg, parse_color("#1a1a1a"));
        assert_eq!(t.ink, parse_color("#eee"));
        assert_eq!(t.ink_hex, "#eee");
        assert_eq!(t.icon_active, parse_color("#7aa"));
        assert!(
            luminance(t.panel) > luminance(t.bg),
            "panel lifts off a dark canvas too"
        );
        assert!(luminance(t.active_bg) > luminance(t.panel));
        for ch in 0..3 {
            assert!(between(t.dot[ch], t.bg[ch], t.ink[ch]));
        }
    }

    #[test]
    fn invalid_ink_never_reaches_the_document() {
        let t = Theme::from_hex("#fff", "red", "#000");
        assert!(try_parse_color(&t.ink_hex).is_some(), "{:?}", t.ink_hex);
        assert_eq!(t.ink, parse_color(&t.ink_hex));
    }

    #[test]
    fn the_first_ink_is_the_theme_s_own_and_the_rest_are_fixed() {
        let light = Theme::light();
        assert_eq!(light.inks[0], light.ink_hex, "the board's usual ink");
        assert_eq!(light.inks.len(), 1 + INKS.len());
        // A red drawn out of whatever three colors the plugin sent
        // might not be red, so the rest do not move with the theme.
        let dark = Theme::from_hex("#101010", "#e6e6e6", "#7aa2f7");
        assert_ne!(dark.inks[0], light.inks[0]);
        assert_eq!(dark.inks[1..], light.inks[1..]);
        for hex in &light.inks {
            assert!(
                try_parse_color(hex).is_some(),
                "{hex} is written into documents"
            );
        }
    }

    /// The panel is a mix, and a hex carries eight bits a channel, so
    /// the round trip lands on the nearest byte rather than exactly
    /// where it started. What has to hold is that it is the same colour
    /// to the eye, and that it parses at all — a frame's background is
    /// written into a document.
    #[test]
    fn panel_hex_is_the_panel_a_document_can_hold() {
        for t in [Theme::light(), Theme::from_hex("#1a1a1a", "#eee", "#7aa")] {
            assert!(try_parse_color(&t.panel_hex).is_some(), "{:?}", t.panel_hex);
            let back = parse_color(&t.panel_hex);
            for ch in 0..3 {
                assert!(
                    (back[ch] - t.panel[ch]).abs() < 0.01,
                    "{:?} came back as {back:?}",
                    t.panel
                );
            }
        }
    }
}
