//! The faces the board's text is set in.
//!
//! The chrome letters itself in one face at one size (`text`); a text on
//! the board names a family and a style of its own, and is drawn at
//! whatever size it is seen at. This is where a family and a style
//! become a face: the machine's, found through fontconfig, or the
//! Liberation Sans the binary carries in all four styles — which is the
//! default family, so a board set in it looks the same on every machine,
//! and the fallback for a family this machine does not have.
//!
//! What asks the machine is a locator handed in from outside, so the
//! rest — which face stands for what, loading each one once, reading
//! `fc-list` — is pure and tested with the bundled faces alone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::doc::DEFAULT_FONT;

/// Liberation Sans (SIL OFL 1.1) — see `assets/fonts/`. Regular, bold,
/// italic, bold italic, in the order [`style_index`] counts them.
const BUNDLED: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Italic.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-BoldItalic.ttf"),
];

/// The size metrics are taken at before they are divided back down to
/// one unit: a font's outline scales linearly, so an advance at one size
/// is the same number at every other, and taking it large keeps the
/// digits it would lose at one pixel.
const UNIT: f32 = 1000.0;

fn style_index(bold: bool, italic: bool) -> usize {
    usize::from(bold) + 2 * usize::from(italic)
}

/// One face, loaded. `id` tells two faces apart where a glyph is cached
/// by the face it came from.
pub struct Face {
    font: fontdue::Font,
    pub id: u32,
    /// Advances at one unit of size, as they are asked for.
    advances: RefCell<HashMap<char, f32>>,
}

impl Face {
    fn new(font: fontdue::Font, id: u32) -> Face {
        Face {
            font,
            id,
            advances: RefCell::new(HashMap::new()),
        }
    }

    /// How far the pen moves past `ch` set at `size`.
    pub fn advance(&self, ch: char, size: f32) -> f32 {
        let unit = *self
            .advances
            .borrow_mut()
            .entry(ch)
            .or_insert_with(|| self.font.metrics(ch, UNIT).advance_width / UNIT);
        unit * size
    }

    /// How much closer `right` stands to `left` than their advances say,
    /// as the face's own kerning table has it: negative draws them in.
    pub fn kern(&self, left: char, right: char, size: f32) -> f32 {
        self.font
            .horizontal_kern(left, right, UNIT)
            .map_or(0.0, |k| k / UNIT * size)
    }

    /// How far the face reaches above its baseline and below it at
    /// `size`, both positive. A face with no line metrics would not be a
    /// text face; the em box answers for it rather than nothing.
    pub fn extents(&self, size: f32) -> (f32, f32) {
        match self.font.horizontal_line_metrics(UNIT) {
            Some(m) => (m.ascent / UNIT * size, -m.descent / UNIT * size),
            None => (0.8 * size, 0.2 * size),
        }
    }

    /// `ch` at `px`: where its box stands from the pen, and its coverage.
    pub fn rasterize(&self, ch: char, px: f32) -> (fontdue::Metrics, Vec<u8>) {
        self.font.rasterize(ch, px)
    }

    /// Whether the face draws `ch` at all rather than its missing box.
    pub fn has(&self, ch: char) -> bool {
        self.font.lookup_glyph_index(ch) != 0
    }
}

/// Where a family's file is on this machine, for a style: what fontconfig
/// answers. `None` when the machine has no such family — fontconfig's
/// habit of answering *something* for anything is the caller's to see
/// through, not this module's.
pub type Locate = Box<dyn Fn(&str, bool, bool) -> Option<PathBuf>>;

/// The faces the board has asked for so far, each loaded once.
pub struct Fonts {
    bundled: [Rc<Face>; 4],
    locate: Option<Locate>,
    faces: RefCell<HashMap<(String, usize), Rc<Face>>>,
    /// The families the font menu offers, the default first.
    families: Vec<String>,
    next: RefCell<u32>,
}

impl Fonts {
    /// The bundled family alone: what a machine without fontconfig has,
    /// and what the suite sets text in.
    pub fn bundled() -> Fonts {
        let settings = fontdue::FontSettings::default();
        let bundled = std::array::from_fn(|i| {
            let font = fontdue::Font::from_bytes(BUNDLED[i], settings)
                .expect("the bundled faces parse");
            Rc::new(Face::new(font, i as u32))
        });
        Fonts {
            bundled,
            locate: None,
            faces: RefCell::new(HashMap::new()),
            families: vec![DEFAULT_FONT.to_owned()],
            next: RefCell::new(BUNDLED.len() as u32),
        }
    }

    /// The bundled family, and the machine's through `locate`, offering
    /// `families` in the font menu.
    pub fn with(locate: Locate, families: Vec<String>) -> Fonts {
        Fonts {
            locate: Some(locate),
            families,
            ..Fonts::bundled()
        }
    }

    /// The families there are to choose from, the default first.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// The face `family` is set in, in a style. The default family is
    /// always the bundled one — asking the machine for it could answer
    /// another version, and a board set in it has to look the same
    /// everywhere. A family the machine does not have, or whose file will
    /// not parse, is set in the bundled face in the same style.
    pub fn face(&self, family: &str, bold: bool, italic: bool) -> Rc<Face> {
        let style = style_index(bold, italic);
        let fallback = || Rc::clone(&self.bundled[style]);
        if family == DEFAULT_FONT {
            return fallback();
        }
        let key = (family.to_owned(), style);
        if let Some(face) = self.faces.borrow().get(&key) {
            return Rc::clone(face);
        }
        let loaded = self
            .locate
            .as_ref()
            .and_then(|locate| locate(family, bold, italic))
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok())
            .map(|font| {
                let mut next = self.next.borrow_mut();
                *next += 1;
                Rc::new(Face::new(font, *next - 1))
            });
        let face = loaded.unwrap_or_else(fallback);
        self.faces.borrow_mut().insert(key, Rc::clone(&face));
        face
    }
}

/// The families `fc-list --format '%{family[0]}\n'` names, as the font
/// menu lists them: each once, in alphabetical order whatever the case,
/// the bundled default first and never twice. A hidden family — one
/// whose name starts with a dot, as fontconfig's own internals do — is
/// not offered.
pub fn families_from(listing: &str) -> Vec<String> {
    let mut names: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.starts_with('.') && *n != DEFAULT_FONT)
        .map(str::to_owned)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    std::iter::once(DEFAULT_FONT.to_owned()).chain(names).collect()
}

/// What fontconfig answers a pattern with: the file, then the family it
/// actually is. The family is how a match is told from fontconfig's
/// habit of answering the default sans for a family it does not have.
pub fn matched(answer: &str, family: &str) -> Option<PathBuf> {
    let mut lines = answer.lines();
    let file = lines.next()?.trim();
    let got = lines.next()?.trim();
    (!file.is_empty() && got.eq_ignore_ascii_case(family)).then(|| PathBuf::from(file))
}

/// The machine's faces through fontconfig: `fc-list` for the menu, and
/// `fc-match` for a family and a style. The two subprocesses are the
/// only shell here; what they answer goes through [`families_from`] and
/// [`matched`].
pub fn machine() -> Fonts {
    let listing = std::process::Command::new("fc-list")
        .args(["--format", "%{family[0]}\n", ":"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let locate: Locate = Box::new(|family, bold, italic| {
        let family_only = family.replace(['-', ':', ','], " ");
        let ask = |pattern: &str| {
            let out = std::process::Command::new("fc-match")
                .args(["--format", "%{file}\n%{family[0]}\n", pattern])
                .output()
                .ok()
                .filter(|o| o.status.success())?;
            matched(&String::from_utf8_lossy(&out.stdout), family)
        };
        let styled = format!(
            "{family_only}:weight={}:slant={}",
            if bold { "bold" } else { "regular" },
            if italic { "italic" } else { "roman" },
        );
        // A family without the style asked for is still that family:
        // its plain face is nearer the mark than the bundled one.
        ask(&styled).or_else(|| ask(&family_only))
    });
    Fonts::with(locate, families_from(&listing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_bundled_family_comes_in_four_styles_that_differ() {
        let fonts = Fonts::bundled();
        let regular = fonts.face(DEFAULT_FONT, false, false);
        let bold = fonts.face(DEFAULT_FONT, true, false);
        let italic = fonts.face(DEFAULT_FONT, false, true);
        let both = fonts.face(DEFAULT_FONT, true, true);
        let ids = [regular.id, bold.id, italic.id, both.id];
        assert_eq!(ids, [0, 1, 2, 3]);
        // Bold is wider; italic leans, which the raster shows.
        assert!(bold.advance('m', 20.0) > regular.advance('m', 20.0));
        assert_ne!(italic.rasterize('l', 40.0).1, regular.rasterize('l', 40.0).1);
        assert!(both.has('a'));
    }

    #[test]
    fn an_advance_scales_with_the_size() {
        let fonts = Fonts::bundled();
        let face = fonts.face(DEFAULT_FONT, false, false);
        let (small, big) = (face.advance('W', 10.0), face.advance('W', 30.0));
        assert!((big - 3.0 * small).abs() < 1e-3, "{small} {big}");
        let (up, down) = face.extents(100.0);
        assert!(up > 50.0 && down > 10.0 && up + down < 150.0, "{up} {down}");
    }

    #[test]
    fn a_family_the_machine_lacks_is_set_in_the_bundled_face_of_its_style() {
        let asked = Rc::new(Cell::new(0));
        let counter = Rc::clone(&asked);
        let fonts = Fonts::with(
            Box::new(move |_, _, _| {
                counter.set(counter.get() + 1);
                None
            }),
            vec![DEFAULT_FONT.into()],
        );
        let face = fonts.face("Nowhere Grotesk", true, false);
        assert_eq!(face.id, fonts.face(DEFAULT_FONT, true, false).id);
        fonts.face("Nowhere Grotesk", true, false);
        assert_eq!(asked.get(), 1, "the machine is asked once for each");
        fonts.face("Nowhere Grotesk", false, false);
        assert_eq!(asked.get(), 2, "and once for each style");
    }

    #[test]
    fn the_default_family_never_asks_the_machine() {
        let fonts = Fonts::with(Box::new(|_, _, _| panic!("asked")), vec![]);
        fonts.face(DEFAULT_FONT, false, true);
    }

    #[test]
    fn a_face_off_the_machine_is_loaded_once_and_given_its_own_id() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("face.ttf");
        std::fs::write(&file, BUNDLED[1]).unwrap();
        let broken = dir.path().join("broken.ttf");
        std::fs::write(&broken, b"not a font").unwrap();
        let (good, bad) = (file.clone(), broken.clone());
        let fonts = Fonts::with(
            Box::new(move |family, _, _| match family {
                "Good" => Some(good.clone()),
                _ => Some(bad.clone()),
            }),
            vec![],
        );
        let a = fonts.face("Good", false, false);
        assert!(a.id >= 4, "past the bundled four");
        assert_eq!(fonts.face("Good", false, false).id, a.id);
        let b = fonts.face("Broken", false, false);
        assert_eq!(b.id, 0, "a file that will not parse is the bundled face");
    }

    #[test]
    fn the_menu_lists_each_family_once_the_default_first() {
        let listing = "Noto Sans\nadwaita Sans\n\nNoto Sans\n.Hidden\nLiberation Sans\nBerkeley\n";
        assert_eq!(
            families_from(listing),
            ["Liberation Sans", "adwaita Sans", "Berkeley", "Noto Sans"]
        );
        assert_eq!(families_from(""), ["Liberation Sans"]);
    }

    #[test]
    fn a_match_is_only_the_family_that_was_asked_for() {
        let answer = "/usr/share/fonts/noto/NotoSerif-Bold.ttf\nNoto Serif\n";
        assert_eq!(
            matched(answer, "noto serif"),
            Some(PathBuf::from("/usr/share/fonts/noto/NotoSerif-Bold.ttf"))
        );
        assert_eq!(matched("/x/DejaVuSans.ttf\nDejaVu Sans\n", "Nowhere"), None);
        assert_eq!(matched("", "Noto Serif"), None);
    }
}
