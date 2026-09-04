//! Palette. Light by default — the reference look: off-white canvas, subtle
//! dot grid, near-black ink, white dock — and re-derived either from the
//! three colors the plugin sends with `op: theme` (§5) or from the theme
//! the desktop is actually wearing ([`crate::omarchy`]).
//!
//! It carries two numbers that are not colours, because a theme is not
//! only colours: Omarchy's own `shell.toml` puts border widths and a type
//! scale beside its palette. They ride here because a radius and a border
//! are only ever read while painting, and every `prims()` already takes a
//! `&Theme` — so the desktop reaches the whole chrome without a single
//! signature changing.

use crate::omarchy::{Mode, Palette, Shell, Style};
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
    /// How far a panel's corner is cut, as a share of the family the
    /// chrome was drawn as: Hyprland's `decoration:rounding` against the
    /// 8 it was drawn to. At 0 the chrome is square, like every window on
    /// the desktop. Read through [`Theme::corner`], never directly.
    pub rounding: f32,
    /// A chrome border, in logical px — `[controls] normal-border-width`.
    pub border_px: f32,
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
            inks: inks(ink_hex_for_inks),
            rounding: 1.0,
            border_px: 1.0,
        }
    }

    /// The palette the desktop is wearing, or the board's own where there
    /// is none — dressed either way in the desktop's border and corner,
    /// since those come from the session and not from the theme.
    pub fn from_style(style: &Style) -> Theme {
        let mut theme = match &style.palette {
            Some(p) => Theme::from_omarchy(p, &style.shell),
            None => Theme::light(),
        };
        theme.rounding = style.corner.clamp(0.0, 4.0);
        theme.border_px = style.shell.border_width;
        theme
    }

    /// A panel's or a button's corner, in physical px: the radius it was
    /// drawn with, at this scale, cut to what the desktop rounds a window
    /// to. **A capsule is not a corner** — a slider's track, a scrollbar's
    /// thumb, an ink dot and a round knob are shapes whose radius is half
    /// their own height, and squaring those would not make the board look
    /// like Omarchy, it would make it look broken.
    pub fn corner(&self, radius: f32, scale: f32) -> f32 {
        radius * scale * self.rounding
    }

    /// A chrome border, in physical px. Drawn as a rounded rect behind
    /// the surface, so a panel's outer radius is its own plus this.
    pub fn edge(&self, scale: f32) -> f32 {
        self.border_px * scale
    }

    /// Omarchy paints every surface in `background` and separates it with
    /// a border — its bar, menus, popups and tooltips all name the same
    /// one. That does not close on a board, whose canvas *is* the ground:
    /// a dock painted in `background` over a canvas painted in
    /// `background` is invisible but for its outline. The desktop already
    /// names the way out — the canvas is `dark_background` and every
    /// panel is `background`, which is a step apart whichever way the
    /// theme runs, and which on the `white` theme lands on the very
    /// palette this board's reference look was drawn as.
    fn from_omarchy(p: &Palette, shell: &Shell) -> Theme {
        let bg = parse_color(&p.dark_background);
        let panel = parse_color(&p.background);
        let ink_hex = match try_parse_color(&p.foreground) {
            Some(_) => p.foreground.clone(),
            None => FALLBACK_INK.to_owned(),
        };
        let ink = parse_color(&ink_hex);
        let accent = parse_color(&p.accent);
        // The outline is `[controls]`': its colour at its alpha, over
        // whatever the panel is standing on.
        let edge = shell.border.as_deref().unwrap_or(&ink_hex);
        Theme {
            bg,
            // No Omarchy token names a grid dot, so it stays what it has
            // always been: a tint of the ink on the ground it is on.
            dot: mix(bg, ink, 0.25),
            ink,
            ink_hex: ink_hex.clone(),
            panel,
            // The hex the theme gave, not one derived back out of linear
            // light: a frame's ground is written into a document, and the
            // document should hold the colour the desktop named.
            panel_hex: match try_parse_color(&p.background) {
                Some(_) => p.background.clone(),
                None => to_hex(panel),
            },
            border: with_alpha(parse_color(edge), shell.border_alpha),
            shadow: with_alpha(BLACK, if p.mode == Mode::Light { 0.16 } else { 0.5 }),
            // `[menu] text` — icons and letters are the same ink.
            icon: ink,
            icon_active: accent,
            // The token the theme names, rather than a mix of its own.
            muted: parse_color(&p.muted),
            // A fill alpha over the surface is what Omarchy means by a
            // selected control, and compositing it here rather than
            // leaving it translucent is what lets the rest of the palette
            // be compared against it.
            active_bg: mix(panel, ink, shell.selected_fill),
            selection: accent,
            lifted: parse_color(LIFTED),
            handle: panel,
            // The dock's inks are what a board is *marked up* in, not
            // what the interface is painted with: a fixed red goes on
            // matching itself across every theme.
            inks: inks(ink_hex),
            rounding: 1.0,
            border_px: 1.0,
        }
    }
}

/// The theme's own ink, then the ones a board is marked up in.
fn inks(own: String) -> Vec<String> {
    std::iter::once(own)
        .chain(INKS.iter().map(|&s| s.to_owned()))
        .collect()
}

fn luminance(c: Rgba) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{parse_color, try_parse_color};

    /// Ristretto, as `omarchy-theme-set` stages it.
    const DARK: &str = "mode = \"dark\"\n\
         accent = \"#f38d70\"\nmuted = \"#72696a\"\n\
         background = \"#2c2525\"\ndark_background = \"#211b1b\"\n\
         foreground = \"#e6d9db\"\n";

    /// The `white` theme, whose two grounds are the palette this board's
    /// own reference look was drawn as.
    const WHITE: &str = "mode = \"light\"\n\
         accent = \"#6e6e6e\"\nmuted = \"#808080\"\n\
         background = \"#ffffff\"\ndark_background = \"#f5f5f5\"\n\
         foreground = \"#000000\"\n";

    fn wearing(body: &str) -> Style {
        Style {
            palette: Palette::read(body, false),
            ..Style::default()
        }
    }

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

    /// A board's canvas *is* the ground, so it cannot be the same
    /// `background` every other Omarchy surface is: the desktop's own
    /// darker ground is the canvas, and the panels stand on it in the
    /// colour a menu would be.
    #[test]
    fn the_desktops_two_grounds_are_the_canvas_and_the_panels() {
        for body in [DARK, WHITE] {
            let style = wearing(body);
            let p = style.palette.clone().expect("a palette");
            let t = Theme::from_style(&style);
            assert_eq!(t.bg, parse_color(&p.dark_background));
            assert_eq!(t.panel, parse_color(&p.background));
            assert_ne!(t.bg, t.panel, "a panel has to be seen against it");
        }
    }

    /// Not a coincidence worth hiding: the reference look was an
    /// off-white canvas of #f5f5f4 under a near-white dock, and the
    /// `white` theme names #f5f5f5 and #ffffff.
    #[test]
    fn the_white_theme_lands_on_the_reference_look() {
        let t = Theme::from_style(&wearing(WHITE));
        assert_eq!(t.bg, parse_color("#f5f5f5"));
        assert_eq!(t.panel, parse_color("#ffffff"));
        assert_eq!(t.ink_hex, "#000000");
    }

    #[test]
    fn the_border_is_the_desktops_own_colour_at_its_own_alpha() {
        let mut style = wearing(DARK);
        style.shell.border = Some("#f38d70".into());
        style.shell.border_alpha = 0.25;
        let t = Theme::from_style(&style);
        assert_eq!(t.border, with_alpha(parse_color("#f38d70"), 0.25));

        // A theme that names no border colour leaves it to the ink.
        style.shell.border = None;
        let t = Theme::from_style(&style);
        assert_eq!(t.border, with_alpha(parse_color("#e6d9db"), 0.25));
    }

    /// `muted` stops being a mix this build invented.
    #[test]
    fn the_tokens_the_theme_names_are_taken_at_their_word() {
        let t = Theme::from_style(&wearing(DARK));
        assert_eq!(t.muted, parse_color("#72696a"));
        assert_eq!(t.icon_active, parse_color("#f38d70"));
        assert_eq!(t.selection, parse_color("#f38d70"));
        assert_eq!(t.icon, t.ink, "an icon is lettered in the same ink");
    }

    #[test]
    fn a_selected_control_is_the_ink_at_the_shells_own_fill() {
        let mut style = wearing(DARK);
        style.shell.selected_fill = 0.5;
        let t = Theme::from_style(&style);
        assert_eq!(t.active_bg, mix(t.panel, t.ink, 0.5));
        assert_eq!(t.active_bg[3], 1.0, "composited, not left translucent");
    }

    /// The inks are what a board is marked up in, not what the interface
    /// is painted with. Only the theme's own moves.
    #[test]
    fn the_docks_inks_do_not_move_with_the_interface() {
        let dark = Theme::from_style(&wearing(DARK));
        let white = Theme::from_style(&wearing(WHITE));
        assert_eq!(dark.inks[0], "#e6d9db");
        assert_eq!(white.inks[0], "#000000");
        assert_eq!(dark.inks[1..], white.inks[1..]);
        assert_eq!(dark.inks[1..], *Theme::light().inks[1..].to_vec());
    }

    /// A corner and a border width belong to the session, not to the
    /// palette, so they are worn whether or not there is one.
    #[test]
    fn the_corner_and_the_border_width_come_from_the_session() {
        for body in [DARK, ""] {
            let mut style = wearing(body);
            style.corner = 0.0;
            style.shell.border_width = 2.0;
            let t = Theme::from_style(&style);
            assert_eq!(t.rounding, 0.0, "square, like every window out there");
            assert_eq!(t.corner(12.0, 2.0), 0.0);
            assert_eq!(t.border_px, 2.0);
            assert_eq!(t.edge(2.0), 4.0, "a border is logical px, like the rest");
        }
    }

    #[test]
    fn a_desktop_with_no_palette_leaves_the_board_its_own() {
        let t = Theme::from_style(&Style::default());
        assert_eq!(t.bg, Theme::light().bg);
        assert_eq!(t.ink_hex, Theme::light().ink_hex);
        assert_eq!(t.rounding, 1.0);
        assert_eq!(t.corner(12.0, 2.0), 24.0, "the chrome as it was drawn");
        assert_eq!(t.border_px, 1.0);
    }

    /// A frame's ground is written into a document, so what a panel is
    /// worth as a hex has to parse — and, when the desktop named it, be
    /// exactly what the desktop named.
    #[test]
    fn the_panel_hex_is_the_one_the_desktop_named() {
        let t = Theme::from_style(&wearing(DARK));
        assert_eq!(t.panel_hex, "#2c2525");
        assert!(try_parse_color(&t.panel_hex).is_some());
    }

    #[test]
    fn a_foreground_that_is_not_a_colour_never_reaches_the_document() {
        let style = wearing("background = \"#101010\"\nforeground = \"perhaps\"\n");
        let t = Theme::from_style(&style);
        assert!(try_parse_color(&t.ink_hex).is_some(), "{:?}", t.ink_hex);
        assert_eq!(t.inks[0], t.ink_hex);
    }
}
