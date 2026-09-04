//! The desktop's own look: what `omarchy-theme-set` leaves on disk, read
//! back the way every other Omarchy app reads it.
//!
//! A theme is staged into `~/.local/state/omarchy/current/theme/` and
//! swapped in whole. Two files there describe a window that draws its own
//! chrome — `colors.toml`, the palette by name, and `shell.toml`, the
//! border widths, fill alphas and type scale the shell's surfaces are
//! built from — and the person's own `~/.config/omarchy/shell.toml` lies
//! over the second. Two more things belong to the session rather than to
//! the theme, and the look is not Omarchy's without them: the family
//! fontconfig resolves `monospace` to, and the corner Hyprland rounds
//! every window to.
//!
//! Everything above [`read`] is pure and carries the tests. `read` itself,
//! the `fc-match` and the `hyprctl` are the only shell in here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Where `omarchy-theme-set` swaps the staged theme in. Omarchy names
/// this path outright rather than through XDG, so this does too: reading
/// it anywhere else would be reading a different desktop's theme.
const THEME_DIR: &str = ".local/state/omarchy/current/theme";
/// Rewritten on every theme set, which is what makes it the stamp.
const THEME_NAME: &str = ".local/state/omarchy/current/theme.name";
const USER_SHELL: &str = ".config/omarchy/shell.toml";

/// `[font] base-size` as the shell's own template ships it. The chrome's
/// dimensions were drawn at this size, so it is what `scale-with-font`
/// measures a change against.
const BASE_SIZE: f32 = 12.0;

/// The `decoration:rounding` the board's radii were drawn to. Hyprland's
/// own is read as a ratio of this, so a desktop at 8 gets exactly the
/// chrome this build has always had.
const ROUNDING: f32 = 8.0;

/// Whether the theme runs light or dark. Named rather than guessed: a
/// theme says so outright, and only a third-party one that says nothing
/// falls back on the luminance test Omarchy uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

/// The colours of `colors.toml`, after the alias cascade — only the ones
/// a board has a use for. Hex as the file holds it, since that is what a
/// document holds too.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub mode: Mode,
    pub background: String,
    pub dark_background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
}

/// The tokens of `shell.toml` a window drawing its own chrome can use.
/// The defaults are the ones the shell's own template ships, so a desktop
/// without the file is styled as one with an untouched one.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    pub base_size: f32,
    pub spacing_scale: f32,
    pub spacing_with_font: bool,
    /// `[controls] normal-border`, when it is a colour this build can
    /// read. `None` leaves the border to the foreground.
    pub border: Option<String>,
    pub border_alpha: f32,
    pub border_width: f32,
    pub selected_fill: f32,
}

impl Default for Shell {
    fn default() -> Shell {
        Shell {
            base_size: BASE_SIZE,
            spacing_scale: 1.0,
            spacing_with_font: true,
            border: None,
            border_alpha: 0.4,
            border_width: 1.0,
            selected_fill: 0.18,
        }
    }
}

/// The whole of what the desktop says. Every field falls back on its own,
/// so a machine with no Omarchy on it produces the same [`Style`] as one
/// whose theme this build cannot read — and the board goes on looking the
/// way it always has.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub palette: Option<Palette>,
    pub shell: Shell,
    /// The face `fc-match` resolves `monospace` to. `None` keeps the
    /// bundled one.
    pub face: Option<PathBuf>,
    /// Hyprland's `decoration:rounding` as a ratio of [`ROUNDING`].
    pub corner: f32,
    /// When the theme was last set. Compared against on focus: the
    /// switcher takes the keyboard and gives it back, which is the moment
    /// a change has to be noticed.
    pub stamp: Option<SystemTime>,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            palette: None,
            shell: Shell::default(),
            face: None,
            corner: 1.0,
            stamp: None,
        }
    }
}

impl Style {
    /// What the chrome is laid out at, over the window's own scale
    /// factor: the theme's spacing scale, times how far the type has
    /// moved from the size the chrome was drawn at when the theme asks
    /// for the two to travel together.
    pub fn chrome_scale(&self) -> f64 {
        let font = if self.shell.spacing_with_font {
            self.shell.base_size / BASE_SIZE
        } else {
            1.0
        };
        f64::from((self.shell.spacing_scale * font).clamp(0.25, 4.0))
    }

    /// The one size the chrome letters itself at, in logical px.
    pub fn text_px(&self) -> f32 {
        self.shell.base_size.clamp(1.0, 96.0)
    }
}

impl Palette {
    /// Reads a `colors.toml` body, running the same alias cascade
    /// `omarchy-theme-color` documents — a theme installed from a repo
    /// may carry nothing but the legacy ANSI names, and it is still a
    /// theme. `light` is whether a `light.mode` marker sits beside the
    /// file, which is the third of the four places a mode can come from.
    ///
    /// `None` when there is no background and no foreground to build on:
    /// that is not a palette, and guessing one would paint the board in
    /// colours nobody chose.
    pub fn read(body: &str, light: bool) -> Option<Palette> {
        let mut c = table(body);

        // The long names win when a theme defines both forms.
        for (long, short) in [
            ("background", "bg"),
            ("dark_background", "dark_bg"),
            ("foreground", "fg"),
            ("dark_foreground", "dark_fg"),
        ] {
            alias(&mut c, long, short);
        }
        alias(&mut c, "background", "color0");
        alias(&mut c, "foreground", "color7");

        let background = c.get("background")?.clone();
        let foreground = c.get("foreground")?.clone();

        alias(&mut c, "muted", "color8");
        alias(&mut c, "muted", "dark_foreground");
        alias(&mut c, "accent", "blue");
        alias(&mut c, "accent", "color4");

        let dark_background = match c.get("dark_background") {
            Some(hex) => hex.clone(),
            None => mix_hex(&background, "#000000", 0.25),
        };

        Some(Palette {
            mode: mode_of(&c, light, &background),
            dark_background,
            muted: c.get("muted").cloned().unwrap_or_else(|| foreground.clone()),
            accent: c.get("accent").cloned().unwrap_or_else(|| foreground.clone()),
            background,
            foreground,
        })
    }
}

/// The mode, from the four places it can come from, in the order
/// `omarchy-theme-color` puts them in: the key, the legacy key, a marker
/// file beside the palette, and finally the background's own brightness —
/// which is the sum of its three bytes against half of 765, as the
/// desktop measures it, so that the board and the desktop cannot come to
/// disagree about which kind of theme is up.
fn mode_of(c: &BTreeMap<String, String>, light: bool, background: &str) -> Mode {
    let named = c.get("mode").or_else(|| c.get("theme_type"));
    match named.map(String::as_str) {
        Some("light") => return Mode::Light,
        Some("dark") => return Mode::Dark,
        _ => {}
    }
    if light {
        return Mode::Light;
    }
    match bytes_of(background) {
        Some([r, g, b]) if u32::from(r) + u32::from(g) + u32::from(b) > 382 => Mode::Light,
        _ => Mode::Dark,
    }
}

impl Shell {
    /// Reads the theme's `shell.toml` with the person's own laid over it,
    /// which is the order the shell itself reads them in. A value may name
    /// another one — `border = "hyprland.active-border"` is how the
    /// generated file ties a card's outline to the window border — so a
    /// reference is followed before it is read.
    pub fn read(theme: &str, user: &str) -> Shell {
        let mut t = sections(theme);
        t.extend(sections(user));

        let mut shell = Shell::default();
        if let Some(v) = number(&t, "font.base-size") {
            shell.base_size = v;
        }
        if let Some(v) = number(&t, "spacing.scale") {
            shell.spacing_scale = v;
        }
        if let Some(v) = flag(&t, "spacing.scale-with-font") {
            shell.spacing_with_font = v;
        }
        if let Some(v) = number(&t, "controls.normal-border-alpha") {
            shell.border_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(v) = width(&t, "controls.normal-border-width") {
            shell.border_width = v.clamp(0.0, 8.0);
        }
        if let Some(v) = number(&t, "controls.selected-fill-alpha") {
            shell.selected_fill = v.clamp(0.0, 1.0);
        }
        shell.border = follow(&t, "controls.normal-border").and_then(|v| color_hex(&v));
        shell
    }
}

/// Every `key = value` of a flat table — which is all `colors.toml` is.
/// Quotes and inline comments come off; a line that is neither is skipped
/// rather than refused, because a palette is not a schema and a key this
/// build does not know is not an error.
fn table(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']).to_owned();
        if key.is_empty() {
            continue;
        }
        out.insert(key, unquote(value));
    }
    out
}

/// The same, but keyed `section.key` — `shell.toml` is sections, and a
/// value in it may name another by exactly that form.
fn sections(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(format!("{section}.{key}"), unquote(value));
    }
    out
}

/// A value with its quotes and any trailing comment taken off. An
/// unquoted value that opens with a `#` is a bare hex and not a comment:
/// a comment there would leave no value at all, and the line it is on has
/// already been skipped.
fn unquote(value: &str) -> String {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix(['"', '\'']) {
        let quote = value.as_bytes()[0] as char;
        return rest.split(quote).next().unwrap_or_default().to_owned();
    }
    if value.starts_with('#') {
        return value.split_whitespace().next().unwrap_or_default().to_owned();
    }
    value
        .split('#')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Fills `key` from `from` when it has nothing of its own.
fn alias(c: &mut BTreeMap<String, String>, key: &str, from: &str) {
    if c.contains_key(key) {
        return;
    }
    if let Some(v) = c.get(from).cloned() {
        c.insert(key.to_owned(), v);
    }
}

/// A value, following it while it names another key. The cap is not
/// defensive dressing: a generated file could name itself, and a window
/// that hangs reading its own theme never draws.
fn follow(t: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let mut value = t.get(key)?.clone();
    for _ in 0..4 {
        match t.get(&value) {
            Some(next) if *next != value => value = next.clone(),
            _ => return Some(value),
        }
    }
    Some(value)
}

fn number(t: &BTreeMap<String, String>, key: &str) -> Option<f32> {
    let v: f32 = t.get(key)?.trim().parse().ok()?;
    v.is_finite().then_some(v)
}

fn flag(t: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    match t.get(key)?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A border width, which the shell writes either as one number or as a
/// CSS-style list (`"Y X"`, `"T X B"`, `"T R B L"`). A rounded rectangle
/// here has one width, so the first is taken and the rest ignored —
/// per-side borders are not a thing this renderer draws.
fn width(t: &BTreeMap<String, String>, key: &str) -> Option<f32> {
    let value = t.get(key)?;
    let first = value.split_whitespace().next()?;
    let v: f32 = first.parse().ok()?;
    v.is_finite().then_some(v)
}

/// A colour token as `#rrggbb`, from any of the forms the shell accepts:
/// a plain hex, a Hyprland `rgba(rrggbbaa)`, or a gradient, whose first
/// stop is the one a flat outline can be drawn in. `None` for anything
/// else, which leaves the caller its own default rather than a guess.
fn color_hex(value: &str) -> Option<String> {
    let first = value.split_whitespace().next()?.trim();
    let inner = first
        .strip_prefix("rgba(")
        .or_else(|| first.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(first);
    let digits = inner.strip_prefix('#').unwrap_or(inner);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => Some(format!("#{digits}")),
        // An alpha pair rides along in the Hyprland form; the outline's
        // strength is the theme's own alpha, not the border colour's.
        6 | 8 => Some(format!("#{}", &digits[..6])),
        _ => None,
    }
}

/// `#rrggbb` → its three bytes.
fn bytes_of(hex: &str) -> Option<[u8; 3]> {
    let s = hex.strip_prefix('#')?;
    let full: String = match s.len() {
        3 => s.chars().flat_map(|c| [c, c]).collect(),
        6 => s.to_owned(),
        _ => return None,
    };
    let mut out = [0u8; 3];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&full[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Mixes two hex colours by byte, which is how the desktop's own shades
/// are derived — a board deriving them in linear light would not land on
/// the same `dark_background` the rest of the session is using.
fn mix_hex(a: &str, b: &str, t: f32) -> String {
    let (Some(a), Some(b)) = (bytes_of(a), bytes_of(b)) else {
        return a.to_owned();
    };
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t + 0.5) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        ch(a[0], b[0]),
        ch(a[1], b[1]),
        ch(a[2], b[2])
    )
}

/// `hyprctl getoption decoration:rounding` prints `int: N` and a line
/// saying whether it was set. The corner is that as a ratio of the one
/// the chrome was drawn to.
fn parse_rounding(out: &str) -> Option<f32> {
    let line = out.lines().find_map(|l| l.trim().strip_prefix("int:"))?;
    let n: f32 = line.trim().parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n / ROUNDING)
}

/// Everything the desktop says, or the defaults where it says nothing.
/// Never fails: a board that will not open because a theme file is
/// missing would be worse than one drawn in its own colours.
pub fn read(home: &str) -> Style {
    Style {
        face: face(),
        corner: rounding().unwrap_or(1.0),
        ..staged(home)
    }
}

/// The half of [`read`] that is files — the theme's palette, its shell
/// tokens under the person's own, and when it was set. Apart so the
/// reading can be tested without a desktop under it.
fn staged(home: &str) -> Style {
    let dir = Path::new(home).join(THEME_DIR);
    let palette = std::fs::read_to_string(dir.join("colors.toml"))
        .ok()
        .and_then(|body| Palette::read(&body, dir.join("light.mode").exists()));
    let shell = Shell::read(
        &std::fs::read_to_string(dir.join("shell.toml")).unwrap_or_default(),
        &std::fs::read_to_string(Path::new(home).join(USER_SHELL)).unwrap_or_default(),
    );
    Style {
        palette,
        shell,
        stamp: stamp(home),
        ..Style::default()
    }
}

/// When the theme was last set, or `None` where Omarchy is not the
/// desktop — in which case nothing ever looks like it changed, which is
/// the truth.
pub fn stamp(home: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(home).join(THEME_NAME))
        .ok()?
        .modified()
        .ok()
}

/// The file behind `monospace`. `omarchy-font-set` writes fontconfig and
/// says outright that the shell and every Qt app resolve through it, so
/// this is the desktop's font, not a guess at one.
fn face() -> Option<PathBuf> {
    let out = Command::new("fc-match")
        .args(["--format=%{file}", "monospace"])
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_file().then_some(path)
}

fn rounding() -> Option<f32> {
    let out = Command::new("hyprctl")
        .args(["getoption", "decoration:rounding"])
        .output()
        .ok()?;
    parse_rounding(&String::from_utf8(out.stdout).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ristretto, as `omarchy-theme-set` stages it.
    const DARK: &str = r##"
mode = "dark"

accent = "#f38d70"
selection = "#403e41"
muted = "#72696a"

background = "#2c2525"
dark_background = "#211b1b"
lighter_background = "#3d2f2a"

foreground = "#e6d9db"
dark_foreground = "#72696a"
"##;

    /// The `white` theme, whose two surfaces are what the board's own
    /// reference look was drawn as.
    const WHITE: &str = r##"
mode = "light"
accent = "#6e6e6e"
muted = "#808080"
background = "#ffffff"
dark_background = "#f5f5f5"
foreground = "#000000"
"##;

    const SHELL: &str = r##"
[bar]
background       = "#2c2525"
text             = "#e6d9db"

[hyprland]
active-border            = "#f38d70"
active-border-foreground = "#e6d9db"

[controls]
normal-color        = "#e6d9db"
normal-fill-alpha   = 0.04
normal-border       = "#e6d9db"
normal-border-width = 1
normal-border-alpha = 0.4

selected-fill-alpha   = 0.18

[spacing]
scale = 1.0
scale-with-font = true
# md                        = 6

[font]
base-size = 12
# caption       = 10

[popups]
border           = "hyprland.active-border"
"##;

    #[test]
    fn a_theme_is_read_by_the_names_it_gives_its_colours() {
        let p = Palette::read(DARK, false).expect("a palette");
        assert_eq!(p.mode, Mode::Dark);
        assert_eq!(p.background, "#2c2525");
        assert_eq!(p.dark_background, "#211b1b");
        assert_eq!(p.foreground, "#e6d9db");
        // The token the theme names, not a mix this build invented.
        assert_eq!(p.muted, "#72696a");
        assert_eq!(p.accent, "#f38d70");
    }

    #[test]
    fn the_canvas_shade_is_a_step_away_from_the_panel_in_both_modes() {
        for body in [DARK, WHITE] {
            let p = Palette::read(body, false).expect("a palette");
            let bg = bytes_of(&p.background).expect("hex");
            let dark = bytes_of(&p.dark_background).expect("hex");
            let sum = |c: [u8; 3]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
            assert!(
                sum(dark) < sum(bg),
                "{} is not below {}",
                p.dark_background,
                p.background
            );
        }
    }

    /// A theme installed from a repo may carry nothing but the ANSI
    /// names, and it is still a theme.
    #[test]
    fn a_palette_of_nothing_but_ansi_names_still_resolves() {
        let p = Palette::read(
            r##"
color0 = "#101010"
color4 = "#7aa2f7"
color7 = "#e6e6e6"
color8 = "#565656"
"##,
            false,
        )
        .expect("a palette");
        assert_eq!(p.background, "#101010");
        assert_eq!(p.foreground, "#e6e6e6");
        assert_eq!(p.muted, "#565656");
        assert_eq!(p.accent, "#7aa2f7", "the accent falls back on blue");
        // Nothing named a darker ground, so one is mixed the way the
        // desktop mixes it.
        assert_eq!(p.dark_background, mix_hex("#101010", "#000000", 0.25));
    }

    #[test]
    fn the_short_names_are_the_long_ones() {
        let p = Palette::read("bg = \"#222\"\nfg = \"#ddd\"", false).expect("a palette");
        assert_eq!(p.background, "#222");
        assert_eq!(p.foreground, "#ddd");
    }

    /// The shell writes its palette quoted, but a theme is a file a
    /// person edits, and an unquoted hex is not a comment.
    #[test]
    fn an_unquoted_hex_survives_the_comment_stripper() {
        let p = Palette::read("background = #101010\nforeground = #eeeeee", false);
        let p = p.expect("a palette");
        assert_eq!(p.background, "#101010");
        assert_eq!(p.foreground, "#eeeeee");
    }

    #[test]
    fn a_palette_without_a_ground_is_not_a_palette() {
        assert!(Palette::read("accent = \"#f00\"", false).is_none());
        assert!(Palette::read("", false).is_none());
    }

    #[test]
    fn the_mode_comes_from_the_four_places_in_order() {
        let named = |line: &str| {
            Palette::read(&format!("{line}\nbackground = \"#101010\"\nfg = \"#eee\""), false)
                .expect("a palette")
                .mode
        };
        assert_eq!(named("mode = \"light\""), Mode::Light);
        assert_eq!(named("theme_type = \"light\""), Mode::Light);
        // Nothing named: the marker file, then the background itself.
        assert_eq!(named(""), Mode::Dark);
        let marked = Palette::read("background = \"#101010\"\nfg = \"#eee\"", true);
        assert_eq!(marked.expect("a palette").mode, Mode::Light);
        let bright = Palette::read("background = \"#eff1f5\"\nfg = \"#4c4f69\"", false);
        assert_eq!(bright.expect("a palette").mode, Mode::Light);
    }

    #[test]
    fn the_shell_is_read_section_by_section() {
        let s = Shell::read(SHELL, "");
        assert_eq!(s.base_size, 12.0);
        assert_eq!(s.spacing_scale, 1.0);
        assert!(s.spacing_with_font);
        assert_eq!(s.border.as_deref(), Some("#e6d9db"));
        assert_eq!(s.border_alpha, 0.4);
        assert_eq!(s.border_width, 1.0);
        assert_eq!(s.selected_fill, 0.18);
    }

    #[test]
    fn the_persons_own_file_lies_over_the_themes() {
        let s = Shell::read(SHELL, "[font]\nbase-size = 16\n");
        assert_eq!(s.base_size, 16.0, "the person's size wins");
        assert_eq!(s.border_alpha, 0.4, "and nothing else moves");
    }

    #[test]
    fn a_value_may_name_another_one() {
        let s = Shell::read(
            "[hyprland]\nactive-border = \"#f38d70\"\n\
             [controls]\nnormal-border = \"hyprland.active-border\"\n",
            "",
        );
        assert_eq!(s.border.as_deref(), Some("#f38d70"));
    }

    #[test]
    fn a_border_width_written_as_a_list_gives_up_its_first_number() {
        let s = Shell::read("[controls]\nnormal-border-width = \"2 4\"\n", "");
        assert_eq!(s.border_width, 2.0);
    }

    #[test]
    fn a_border_is_read_out_of_every_form_the_shell_accepts() {
        assert_eq!(color_hex("#e6d9db").as_deref(), Some("#e6d9db"));
        assert_eq!(color_hex("#abc").as_deref(), Some("#abc"));
        assert_eq!(color_hex("rgba(595959aa)").as_deref(), Some("#595959"));
        assert_eq!(color_hex("rgb(595959)").as_deref(), Some("#595959"));
        // A gradient's first stop is what a flat outline can be drawn in.
        assert_eq!(
            color_hex("rgba(f38d70ff) rgba(595959aa) 45deg").as_deref(),
            Some("#f38d70")
        );
        assert_eq!(color_hex("hyprland.active-border"), None);
        assert_eq!(color_hex(""), None);
    }

    #[test]
    fn an_unreadable_shell_is_the_shipped_one() {
        assert_eq!(Shell::read("", ""), Shell::default());
        assert_eq!(Shell::read("nonsense\n[[[\n= 4", ""), Shell::default());
    }

    #[test]
    fn a_value_that_names_itself_does_not_hang() {
        let s = Shell::read("[controls]\nnormal-border = \"controls.normal-border\"\n", "");
        assert_eq!(s.border, None, "a name is not a colour");
    }

    #[test]
    fn the_corner_is_hyprlands_rounding_against_the_one_the_chrome_was_drawn_to() {
        assert_eq!(parse_rounding("int: 8\nset: true\n"), Some(1.0));
        assert_eq!(parse_rounding("int: 0\nset: true\n"), Some(0.0));
        assert_eq!(parse_rounding("int: 12\nset: false\n"), Some(1.5));
        assert_eq!(parse_rounding(""), None);
        assert_eq!(parse_rounding("custom type: whatever"), None);
    }

    #[test]
    fn the_chrome_travels_with_the_type_when_the_theme_asks_it_to() {
        let mut style = Style::default();
        assert_eq!(style.chrome_scale(), 1.0, "an untouched theme moves nothing");
        assert_eq!(style.text_px(), BASE_SIZE);

        style.shell.base_size = 24.0;
        assert_eq!(style.chrome_scale(), 2.0);
        assert_eq!(style.text_px(), 24.0);

        style.shell.spacing_with_font = false;
        assert_eq!(style.chrome_scale(), 1.0, "the type moves alone");
        assert_eq!(style.text_px(), 24.0);

        style.shell.spacing_scale = 1.5;
        assert_eq!(style.chrome_scale(), 1.5);
    }

    /// A theme is on disk, or it is not; neither is a reason for the
    /// window not to open.
    #[test]
    fn a_desktop_that_is_not_omarchy_reads_as_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let home = dir.path().to_str().expect("utf-8");
        let style = staged(home);
        assert!(style.palette.is_none());
        assert_eq!(style.shell, Shell::default());
        assert_eq!(style.stamp, None);
        assert_eq!(style.corner, 1.0, "no Hyprland leaves the chrome as drawn");
    }

    #[test]
    fn a_staged_theme_is_read_whole_and_stamped() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let home = dir.path();
        let theme = home.join(THEME_DIR);
        std::fs::create_dir_all(&theme).expect("the theme dir");
        std::fs::write(theme.join("colors.toml"), DARK).expect("a palette");
        std::fs::write(theme.join("shell.toml"), SHELL).expect("a shell");
        std::fs::create_dir_all(home.join(".config/omarchy")).expect("the config dir");
        std::fs::write(home.join(USER_SHELL), "[font]\nbase-size = 14\n").expect("an override");
        std::fs::write(home.join(THEME_NAME), "ristretto\n").expect("a name");

        let style = staged(home.to_str().expect("utf-8"));
        let palette = style.palette.expect("a palette");
        assert_eq!(palette.accent, "#f38d70");
        assert_eq!(style.shell.base_size, 14.0, "the person's size wins");
        assert_eq!(style.shell.selected_fill, 0.18, "the theme's is still read");
        assert!(style.stamp.is_some(), "a theme that was set has a stamp");
    }
}
