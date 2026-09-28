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

use crate::omarchy::{Mode, Palette, Shell, Style, mix_hex};
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

/// The colours a board is marked up in. Fixed and not derived — none of
/// them, the two neutrals included: a red drawn out of whatever three
/// colours the plugin sent might not be red, and a person reaching for
/// the red one means red. The same argument reaches black and white,
/// which is why they are here and not taken from the theme's own
/// foreground the way the first ink once was. A board is a document:
/// ink chosen today has to mean the same colour tomorrow, and a palette
/// that moved when the desktop did would quietly break that. The
/// neutrals lead because they are what a board is drawn in and the rest
/// is emphasis. Enough of them to tell things apart, few enough to fit
/// under the tools without a picker.
pub const INKS: [&str; 7] = [
    "#000000", "#ffffff", "#e5484d", "#f5a524", "#30a46c", "#3b82f6", "#8e4ec6",
];

/// What each of [`INKS`] is called, where a menu writes it out.
pub const INK_NAMES: [&str; 7] = ["Black", "White", "Red", "Amber", "Green", "Blue", "Violet"];

/// Where the two neutrals sit in [`INKS`].
const BLACK_INK: usize = 0;
const WHITE_INK: usize = 1;

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
            rounding: 1.0,
            border_px: 1.0,
        }
    }

    /// The palette the desktop is wearing, or the board's own where there
    /// is none — dressed either way in the desktop's border and corner,
    /// since those come from the session and not from the theme.
    pub fn from_style(style: &Style) -> Theme {
        let theme = match &style.palette {
            Some(p) => Theme::from_omarchy(p, &style.shell),
            None => Theme::light(),
        };
        theme.wearing(style)
    }

    /// The border and the corner the session is drawing with, over a
    /// palette that came from somewhere else — the plugin's three
    /// colours are still how a host that is not Omarchy dresses the
    /// board, and they say nothing about either.
    pub fn wearing(mut self, style: &Style) -> Theme {
        self.rounding = style.corner.clamp(0.0, 4.0);
        self.border_px = style.shell.border_width;
        self
    }

    /// The ink a board is born holding. The palette itself does not move
    /// with the theme, so the canvas answers only which of the two
    /// neutrals reads against it — a black pencil on a dark ground draws
    /// a line nobody can see, and a white one on a light ground the
    /// same. Asked once, at birth: a theme set later leaves the ink in
    /// the hand exactly where it is, the way it leaves the tool.
    pub fn first_ink(&self) -> usize {
        if luminance(self.bg) > 0.5 {
            BLACK_INK
        } else {
            WHITE_INK
        }
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
            // selected control. Compositing it here rather than leaving
            // it translucent is what lets the rest of the palette be
            // compared against it — and it is done in the desktop's own
            // sRGB, since that is where the number was chosen.
            active_bg: parse_color(&mix_hex(&p.background, &ink_hex, shell.selected_fill)),
            selection: accent,
            lifted: parse_color(LIFTED),
            handle: panel,
            rounding: 1.0,
            border_px: 1.0,
        }
    }
}

/// The theme's own ink, then the ones a board is marked up in.
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

    /// Every ink is written into a document, so every one of them has to
    /// parse — and the two neutrals have to be exactly the neutrals.
    #[test]
    fn every_ink_has_a_name_a_menu_can_write() {
        assert_eq!(INK_NAMES.len(), INKS.len());
        assert_eq!((INK_NAMES[BLACK_INK], INK_NAMES[WHITE_INK]), ("Black", "White"));
        assert!(INK_NAMES.iter().all(|n| !n.is_empty()));
    }

    #[test]
    fn the_inks_are_a_fixed_list_and_the_neutrals_lead_it() {
        assert_eq!(INKS[BLACK_INK], "#000000");
        assert_eq!(INKS[WHITE_INK], "#ffffff");
        for hex in INKS {
            assert!(
                try_parse_color(hex).is_some(),
                "{hex} is written into documents"
            );
        }
    }

    /// The palette is the board's, not the desktop's: a colour picked
    /// today has to be the same colour tomorrow, whatever the desktop
    /// went and put on. Nothing about a theme can reach this list, which
    /// is why the test can only say that it is a constant — and that is
    /// the whole of the guarantee.
    #[test]
    fn no_theme_moves_the_docks_inks() {
        let themes = [
            Theme::light(),
            Theme::from_hex("#101010", "#e6e6e6", "#7aa2f7"),
            Theme::from_style(&wearing(DARK)),
            Theme::from_style(&wearing(WHITE)),
        ];
        for t in themes {
            // The ink the chrome letters itself in still follows the
            // theme; what a board is marked up in does not.
            assert_eq!(t.ink, parse_color(&t.ink_hex));
            assert_eq!(INKS.len(), 7, "black, white, and the five");
        }
    }

    /// A black pencil on a dark ground draws a line nobody can see, so
    /// the board is born on the neutral that reads against its canvas.
    /// The list itself never moves — only which of the two it opens on.
    #[test]
    fn a_board_is_born_on_the_neutral_that_reads_against_its_canvas() {
        assert_eq!(Theme::light().first_ink(), BLACK_INK);
        assert_eq!(
            Theme::from_hex("#101010", "#e6e6e6", "#7aa2f7").first_ink(),
            WHITE_INK
        );
        assert_eq!(Theme::from_style(&wearing(DARK)).first_ink(), WHITE_INK);
        assert_eq!(Theme::from_style(&wearing(WHITE)).first_ink(), BLACK_INK);
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

    /// Mixed where the number was chosen: sRGB over sRGB, as Qt
    /// composites. In linear light the same fraction comes out half
    /// again as bright, and the chrome would not match the shell.
    #[test]
    fn a_selected_control_is_the_ink_at_the_shells_own_fill() {
        let mut style = wearing(DARK);
        style.shell.selected_fill = 0.5;
        let t = Theme::from_style(&style);
        assert_eq!(t.active_bg, parse_color("#897f80"));
        assert_eq!(t.active_bg[3], 1.0, "composited, not left translucent");
        assert_ne!(
            t.active_bg,
            mix(t.panel, t.ink, 0.5),
            "and not in linear light, which would be brighter"
        );
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
    }
}
