//! The application menu: File, Edit, View and Layer, as every desktop
//! application has them. Nothing is done here that the board did not
//! already do — each line is a door onto a key, a button or a row's
//! menu, and is written with the keys that do the same. What a line
//! does is an [`Action`]; [`shortcut`] is the one mapping from `Ctrl`
//! and a key to an action, so the hint a line carries cannot come to
//! disagree with the key it names. Pure — `app` runs the actions.

use crate::editor::Command;
use crate::menu::Item;
use crate::scene::{Prim, ScreenRect};
use crate::tabs;
use crate::text::Atlas;
use crate::theme::Theme;
use crate::tree::Arrange;

/// A menu on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    File,
    Edit,
    View,
    Layer,
}

impl Title {
    pub const ALL: [Title; 4] = [Title::File, Title::Edit, Title::View, Title::Layer];

    pub fn label(self) -> &'static str {
        match self {
            Title::File => "File",
            Title::Edit => "Edit",
            Title::View => "View",
            Title::Layer => "Layer",
        }
    }
}

// Logical px.
/// Between a title's label and either edge of its box.
const PAD: f32 = 9.0;
/// Before the first title, so it does not sit against the window's edge.
const LEAD: f32 = 4.0;
/// After the last one, before the tabs.
const TRAIL: f32 = 6.0;
const RADIUS: f32 = 7.0;

/// The bar: the titles standing at the left end of the tab strip, in
/// physical px. The tabs start where it ends.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub rect: ScreenRect,
    pub titles: Vec<(ScreenRect, Title)>,
    scale: f32,
}

impl Bar {
    /// Laid out at the strip's left end, each title as wide as its label
    /// and a pad either side.
    pub fn layout(scale: f64, atlas: &Atlas) -> Bar {
        let s = scale as f32;
        let h = (tabs::HEIGHT * s).round();
        let mut x = (LEAD * s).round();
        let titles = Title::ALL
            .iter()
            .map(|&title| {
                let w = (atlas.measure(title.label()) + 2.0 * PAD * s).round();
                let rect = ScreenRect { x, y: 0.0, w, h };
                x += w;
                (rect, title)
            })
            .collect();
        Bar {
            rect: ScreenRect {
                x: 0.0,
                y: 0.0,
                w: x + (TRAIL * s).round(),
                h,
            },
            titles,
            scale: s,
        }
    }

    /// Where the tab row starts.
    pub fn end(&self) -> f32 {
        self.rect.x + self.rect.w
    }

    /// The title under `(x, y)`.
    pub fn hit(&self, x: f64, y: f64) -> Option<Title> {
        self.titles
            .iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, t)| *t)
    }

    /// The titles, the one whose menu is `open` lit as the active tab is.
    pub fn prims(&self, atlas: &Atlas, slot: u32, theme: &Theme, open: Option<Title>) -> Vec<Prim> {
        let s = self.scale;
        let mut out = Vec::new();
        for (rect, title) in &self.titles {
            let lit = open == Some(*title);
            if lit {
                out.push(Prim::rounded(
                    rect.inset(2.0 * s),
                    theme.corner(RADIUS, s),
                    theme.active_bg,
                ));
            }
            let baseline = atlas.baseline_in(*rect);
            let ink = if lit { theme.ink } else { theme.icon };
            for g in atlas.layout(title.label(), rect.x + PAD * s, baseline) {
                out.push(Prim::glyph(g.rect, g.uv, slot, ink));
            }
        }
        // The seam between the menu and the tabs, as between two tabs.
        let x = self.end() - (TRAIL * s / 2.0).round();
        out.push(Prim::segment(
            (x, self.rect.y + 8.0 * s),
            (x, self.rect.y + self.rect.h - 8.0 * s),
            theme.edge(s) / 2.0,
            theme.border,
        ));
        out
    }
}

/// What a line of the menu does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    /// What `Del` deletes: the selection on the board, or the picked
    /// layers when the panel had the last press.
    Delete,
    /// The layers panel shown or hidden.
    Layers,
    /// The brush library shown or hidden.
    Library,
    /// The board's decks presented, one stop at a time.
    Present,
    /// The decks' arrows and numbers shown on the board or put away.
    Path,
    /// Hand gestures through the webcam turned on or off.
    Hands,
    NewLayer,
    NewGroup,
    Rename,
    Layer(Command),
}

/// What the menus need to know of the window to say what is offered and
/// what is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub undo: bool,
    pub redo: bool,
    /// The clipboard, or the clip kept on the window, holds something a
    /// paste would take.
    pub paste: bool,
    /// Something is selected on the board: what an export sends.
    pub selection: bool,
    /// `Del` would take something away.
    pub delete: bool,
    pub layers: bool,
    /// Whether the brush library is shown, while the brush it belongs to
    /// is in the hand; `None` with any other tool.
    pub library: Option<bool>,
    /// Every picked layer is locked, or hidden: the line undoes it.
    pub locked: bool,
    pub hidden: bool,
    /// What a merge would be called: `Editor::merge_name`.
    pub merge: &'static str,
    /// The board has a frame on show to present.
    pub present: bool,
    /// Whether hand gestures are on, in a build that has them; `None` in
    /// one that does not, which offers no line for them.
    pub hands: Option<bool>,
    /// Whether the decks' arrows and numbers are on the board.
    pub path: bool,
}

/// The `Ctrl` shortcut for `key` — read lower case, with `Shift` and
/// `Alt` told apart — or none.
pub fn shortcut(key: &str, shift: bool, alt: bool) -> Option<Action> {
    Some(match (key, shift, alt) {
        ("n", false, false) => Action::New,
        ("n", true, false) => Action::NewLayer,
        ("o", _, _) => Action::Open,
        ("s", false, _) => Action::Save,
        ("s", true, _) => Action::SaveAs,
        ("e", false, false) => Action::Export,
        ("w", _, _) => Action::Close,
        ("q", false, false) => Action::Quit,
        ("z", false, _) => Action::Undo,
        ("z", true, _) => Action::Redo,
        ("x", false, _) => Action::Cut,
        ("c", false, _) => Action::Copy,
        ("v", _, _) => Action::Paste,
        // Photoshop's keys for the layers: `G` groups and `Shift+G`
        // ungroups, `J` duplicates, the brackets arrange — a step, and
        // all the way with Shift — `/` locks and `,` hides.
        ("g", false, false) => Action::Layer(Command::Group),
        ("g", true, false) => Action::Layer(Command::Ungroup),
        ("j", false, false) => Action::Layer(Command::Duplicate),
        ("]", false, false) => Action::Layer(Command::Arrange(Arrange::Forward)),
        ("]", true, false) => Action::Layer(Command::Arrange(Arrange::Front)),
        ("[", false, false) => Action::Layer(Command::Arrange(Arrange::Backward)),
        ("[", true, false) => Action::Layer(Command::Arrange(Arrange::Back)),
        ("/", false, false) => Action::Layer(Command::Lock),
        (",", false, false) => Action::Layer(Command::Show),
        ("e", false, true) => Action::Layer(Command::Merge),
        ("e", true, false) => Action::Layer(Command::MergeVisible),
        ("h", true, false) => Action::Hands,
        _ => return None,
    })
}

/// Menu `title`'s lines and what each of them does. A line is offered
/// only where it would do something, `can` answering for a layer
/// command.
pub fn items(
    title: Title,
    state: &State,
    can: impl Fn(Command) -> bool,
) -> (Vec<Item>, Vec<Action>) {
    // A line: its label, its keys, what it does, whether it is offered,
    // and whether a rule stands above it.
    let lines: Vec<(&str, &str, Action, bool, bool)> = match title {
        Title::File => vec![
            ("New", "Ctrl+N", Action::New, true, false),
            ("Open…", "Ctrl+O", Action::Open, true, false),
            ("Save", "Ctrl+S", Action::Save, true, true),
            ("Save As…", "Ctrl+Shift+S", Action::SaveAs, true, false),
            (
                "Export to Agent…",
                "Ctrl+E",
                Action::Export,
                state.selection,
                true,
            ),
            ("Close", "Ctrl+W", Action::Close, true, true),
            ("Quit", "Ctrl+Q", Action::Quit, true, false),
        ],
        Title::Edit => vec![
            ("Undo", "Ctrl+Z", Action::Undo, state.undo, false),
            ("Redo", "Ctrl+Shift+Z", Action::Redo, state.redo, false),
            // What is cut and copied is the picked layers, as the row's
            // menu says: a copy always has the active one to take.
            ("Cut", "Ctrl+X", Action::Cut, can(Command::Remove), true),
            ("Copy", "Ctrl+C", Action::Copy, true, false),
            ("Paste", "Ctrl+V", Action::Paste, state.paste, false),
            ("Delete", "Del", Action::Delete, state.delete, false),
        ],
        Title::View => vec![
            ("Layers", "Shift+L", Action::Layers, true, false),
            (
                "Brush Library",
                "Shift+B",
                Action::Library,
                state.library.is_some(),
                false,
            ),
            ("Present", "F5", Action::Present, state.present, true),
            ("Presentation Path", "", Action::Path, true, false),
            ("Hand Gestures", "Ctrl+Shift+H", Action::Hands, state.hands.is_some(), false),
        ],
        Title::Layer => {
            let run = |label, keys, command, rule| {
                (label, keys, Action::Layer(command), can(command), rule)
            };
            vec![
                ("New Layer", "Ctrl+Shift+N", Action::NewLayer, true, false),
                ("New Group", "", Action::NewGroup, true, false),
                ("Rename", "F2", Action::Rename, true, true),
                run("Duplicate", "Ctrl+J", Command::Duplicate, false),
                run("Delete Layer", "", Command::Remove, false),
                run("Group", "Ctrl+G", Command::Group, true),
                run("Ungroup", "Ctrl+Shift+G", Command::Ungroup, false),
                run(state.merge, "Ctrl+Alt+E", Command::Merge, true),
                run(
                    "Merge Visible",
                    "Ctrl+Shift+E",
                    Command::MergeVisible,
                    false,
                ),
                run("Flatten", "", Command::Flatten, false),
                run(
                    "Bring to Front",
                    "Ctrl+Shift+]",
                    Command::Arrange(Arrange::Front),
                    true,
                ),
                run(
                    "Bring Forward",
                    "Ctrl+]",
                    Command::Arrange(Arrange::Forward),
                    false,
                ),
                run(
                    "Send Backward",
                    "Ctrl+[",
                    Command::Arrange(Arrange::Backward),
                    false,
                ),
                run(
                    "Send to Back",
                    "Ctrl+Shift+[",
                    Command::Arrange(Arrange::Back),
                    false,
                ),
                run(
                    if state.locked { "Unlock" } else { "Lock" },
                    "Ctrl+/",
                    Command::Lock,
                    true,
                ),
                run(
                    if state.hidden { "Show" } else { "Hide" },
                    "Ctrl+,",
                    Command::Show,
                    false,
                ),
            ]
        }
    };
    lines
        .into_iter()
        .filter(|(_, _, action, _, _)| *action != Action::Hands || state.hands.is_some())
        .map(|(label, keys, action, enabled, rule)| {
            let item = Item::new(label).enabled(enabled);
            let item = if keys.is_empty() {
                item
            } else {
                item.hint(keys)
            };
            let item = if rule { item.ruled() } else { item };
            let item = match action {
                Action::Layers => item.checked(state.layers),
                Action::Library => item.checked(state.library == Some(true)),
                Action::Hands => item.checked(state.hands == Some(true)),
                Action::Path => item.checked(state.path),
                _ => item,
            };
            (item, action)
        })
        .unzip()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::Tabs;
    use crate::text::Font;

    fn atlas() -> Atlas {
        Atlas::build(&Font::bundled(), Tabs::label_px(tabs::LABEL, 1.0))
    }

    #[test]
    fn the_titles_stand_in_order_at_the_strips_left_end() {
        let a = atlas();
        let bar = Bar::layout(1.0, &a);
        let order: Vec<Title> = bar.titles.iter().map(|(_, t)| *t).collect();
        assert_eq!(order, Title::ALL);
        assert_eq!(bar.rect.x, 0.0);
        assert_eq!(bar.rect.y, 0.0);
        assert_eq!(bar.rect.h, tabs::HEIGHT);
        for pair in bar.titles.windows(2) {
            let (left, right) = (pair[0].0, pair[1].0);
            assert_eq!(left.x + left.w, right.x, "side by side, no gap");
        }
        for (rect, title) in &bar.titles {
            assert!(rect.w > a.measure(title.label()), "{title:?} has room for its label");
            assert_eq!(rect.h, bar.rect.h);
        }
        let last = bar.titles.last().unwrap().0;
        assert!(bar.end() >= last.x + last.w);
        assert_eq!(bar.end(), bar.rect.x + bar.rect.w);
    }

    #[test]
    fn a_title_is_hit_across_its_whole_box_and_nothing_past_the_bar() {
        let a = atlas();
        let bar = Bar::layout(1.0, &a);
        for (rect, title) in &bar.titles {
            let (x, y) = rect.center();
            assert_eq!(bar.hit(x as f64, y as f64), Some(*title));
            assert_eq!(bar.hit(f64::from(rect.x) + 1.0, 1.0), Some(*title));
        }
        assert_eq!(bar.hit(f64::from(bar.end()) + 5.0, 10.0), None);
        assert_eq!(bar.hit(10.0, f64::from(tabs::HEIGHT) + 1.0), None, "the canvas");
    }

    #[test]
    fn the_bar_scales_with_the_chrome() {
        let one = Bar::layout(1.0, &atlas());
        let two = Bar::layout(2.0, &Atlas::build(&Font::bundled(), Tabs::label_px(tabs::LABEL, 2.0)));
        assert_eq!(two.rect.h, one.rect.h * 2.0);
        assert!((two.end() - 2.0 * one.end()).abs() < 8.0, "{} {}", one.end(), two.end());
    }

    #[test]
    fn the_open_title_is_lit_and_every_label_is_lettered() {
        let a = atlas();
        let bar = Bar::layout(1.0, &a);
        let theme = Theme::light();
        let lit = |open| {
            bar.prims(&a, 3, &theme, open)
                .iter()
                .filter(|p| p.color == theme.active_bg)
                .count()
        };
        assert_eq!(lit(None), 0);
        assert_eq!(lit(Some(Title::Edit)), 1);
        let glyphs = bar
            .prims(&a, 3, &theme, None)
            .iter()
            .filter(|p| p.kind == crate::scene::KIND_IMAGE && p.slot == 3)
            .count();
        let letters: usize = Title::ALL.iter().map(|t| t.label().len()).sum();
        assert_eq!(glyphs, letters);
    }

    fn state() -> State {
        State {
            undo: true,
            redo: true,
            paste: true,
            selection: true,
            delete: true,
            layers: true,
            library: Some(true),
            locked: false,
            hidden: false,
            merge: "Merge Down",
            present: true,
            hands: Some(false),
            path: true,
        }
    }

    fn line<'a>(items: &'a [Item], actions: &[Action], action: Action) -> &'a Item {
        let i = actions
            .iter()
            .position(|a| *a == action)
            .unwrap_or_else(|| panic!("no line for {action:?}"));
        &items[i]
    }

    /// The action a hint's keys ask for, read the way the window reads
    /// them.
    fn keys(hint: &str) -> Option<Action> {
        match hint {
            "Del" => return Some(Action::Delete),
            "F2" => return Some(Action::Rename),
            "F5" => return Some(Action::Present),
            "Shift+L" => return Some(Action::Layers),
            "Shift+B" => return Some(Action::Library),
            _ => {}
        }
        let keys = hint.strip_prefix("Ctrl+")?;
        let (alt, keys) = match keys.strip_prefix("Alt+") {
            Some(k) => (true, k),
            None => (false, keys),
        };
        let (shift, key) = match keys.strip_prefix("Shift+") {
            Some(k) => (true, k),
            None => (false, keys),
        };
        shortcut(&key.to_lowercase(), shift, alt)
    }

    #[test]
    fn every_menu_has_lines_and_each_line_an_action() {
        for title in Title::ALL {
            let (items, actions) = super::items(title, &state(), |_| true);
            assert!(!items.is_empty(), "{title:?}");
            assert_eq!(items.len(), actions.len(), "{title:?}");
        }
    }

    #[test]
    fn every_hint_is_the_key_that_does_the_same() {
        for title in Title::ALL {
            let (items, actions) = super::items(title, &state(), |_| true);
            for (item, action) in items.iter().zip(&actions) {
                let Some(hint) = &item.hint else { continue };
                assert_eq!(
                    keys(hint),
                    Some(*action),
                    "{title:?}: {} says {hint}",
                    item.label
                );
            }
        }
    }

    #[test]
    fn the_file_menu_is_the_one_every_application_has() {
        let (items, actions) = super::items(Title::File, &state(), |_| true);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "New",
                "Open…",
                "Save",
                "Save As…",
                "Export to Agent…",
                "Close",
                "Quit"
            ]
        );
        assert_eq!(
            actions,
            [
                Action::New,
                Action::Open,
                Action::Save,
                Action::SaveAs,
                Action::Export,
                Action::Close,
                Action::Quit
            ]
        );
    }

    #[test]
    fn the_keys_the_window_always_had_keep_their_meaning() {
        assert_eq!(shortcut("s", false, false), Some(Action::Save));
        assert_eq!(shortcut("s", true, false), Some(Action::SaveAs));
        assert_eq!(shortcut("o", false, false), Some(Action::Open));
        assert_eq!(shortcut("w", false, false), Some(Action::Close));
        assert_eq!(shortcut("z", false, false), Some(Action::Undo));
        assert_eq!(shortcut("z", true, false), Some(Action::Redo));
        assert_eq!(shortcut("c", false, false), Some(Action::Copy));
        assert_eq!(shortcut("x", false, false), Some(Action::Cut));
        assert_eq!(shortcut("v", false, false), Some(Action::Paste));
        assert_eq!(shortcut("e", false, false), Some(Action::Export));
        assert_eq!(shortcut("n", true, false), Some(Action::NewLayer));
        assert_eq!(shortcut("c", true, false), None, "Shift+C is nobody's");
        assert_eq!(shortcut("k", false, false), None);
    }

    #[test]
    fn the_layer_shortcuts_ask_for_their_commands() {
        let layer = |k, s, a| {
            shortcut(k, s, a).and_then(|a| match a {
                Action::Layer(c) => Some(c),
                _ => None,
            })
        };
        assert_eq!(layer("g", false, false), Some(Command::Group));
        assert_eq!(layer("g", true, false), Some(Command::Ungroup));
        assert_eq!(layer("j", false, false), Some(Command::Duplicate));
        assert_eq!(layer("/", false, false), Some(Command::Lock));
        assert_eq!(layer(",", false, false), Some(Command::Show));
        assert_eq!(
            layer("]", true, false),
            Some(Command::Arrange(Arrange::Front))
        );
        assert_eq!(
            layer("]", false, false),
            Some(Command::Arrange(Arrange::Forward))
        );
        assert_eq!(
            layer("[", false, false),
            Some(Command::Arrange(Arrange::Backward))
        );
        assert_eq!(
            layer("[", true, false),
            Some(Command::Arrange(Arrange::Back))
        );
        assert_eq!(layer("e", false, true), Some(Command::Merge));
        assert_eq!(layer("e", true, false), Some(Command::MergeVisible));
    }

    #[test]
    fn a_line_is_offered_only_where_it_would_do_something() {
        let none = State {
            undo: false,
            redo: false,
            paste: false,
            selection: false,
            delete: false,
            library: None,
            ..state()
        };
        let (items, actions) = super::items(Title::Edit, &none, |_| false);
        for action in [
            Action::Undo,
            Action::Redo,
            Action::Paste,
            Action::Delete,
            Action::Cut,
        ] {
            assert!(!line(&items, &actions, action).enabled, "{action:?}");
        }
        assert!(
            line(&items, &actions, Action::Copy).enabled,
            "a copy takes the active layer"
        );

        let (items, actions) = super::items(Title::File, &none, |_| false);
        assert!(
            !line(&items, &actions, Action::Export).enabled,
            "nothing to send"
        );
        for action in [
            Action::New,
            Action::Open,
            Action::Save,
            Action::SaveAs,
            Action::Close,
            Action::Quit,
        ] {
            assert!(line(&items, &actions, action).enabled, "{action:?}");
        }

        let (items, actions) = super::items(Title::Layer, &none, |_| false);
        let duplicate = Action::Layer(Command::Duplicate);
        assert!(!line(&items, &actions, duplicate).enabled);
        assert!(line(&items, &actions, Action::NewLayer).enabled);
        assert!(line(&items, &actions, Action::NewGroup).enabled);

        let (items, actions) = super::items(Title::View, &none, |_| false);
        assert!(
            !line(&items, &actions, Action::Library).enabled,
            "the brush's own chrome"
        );
    }

    #[test]
    fn the_view_menu_checks_what_is_shown() {
        let (items, actions) = super::items(Title::View, &state(), |_| true);
        assert!(line(&items, &actions, Action::Layers).checked);
        assert!(line(&items, &actions, Action::Library).checked);
        let hidden = State {
            layers: false,
            library: Some(false),
            ..state()
        };
        let (items, actions) = super::items(Title::View, &hidden, |_| true);
        assert!(!line(&items, &actions, Action::Layers).checked);
        assert!(!line(&items, &actions, Action::Library).checked);
    }

    #[test]
    fn the_presentation_path_is_a_line_of_the_view_menu_checked_while_it_shows() {
        let (items, actions) = super::items(Title::View, &state(), |_| true);
        assert!(line(&items, &actions, Action::Path).checked);
        let hidden = State {
            path: false,
            ..state()
        };
        let (items, actions) = super::items(Title::View, &hidden, |_| true);
        assert!(!line(&items, &actions, Action::Path).checked);
    }

    #[test]
    fn hand_gestures_are_offered_only_by_a_build_that_has_them() {
        let (_, actions) = super::items(Title::View, &State { hands: None, ..state() }, |_| true);
        assert!(!actions.contains(&Action::Hands), "no line for what the build lacks");
        let on = State {
            hands: Some(true),
            ..state()
        };
        let (items, actions) = super::items(Title::View, &on, |_| true);
        assert!(line(&items, &actions, Action::Hands).checked);
        assert_eq!(shortcut("h", true, false), Some(Action::Hands));
    }

    #[test]
    fn the_layer_menu_says_what_its_lines_would_do() {
        let (items, actions) = super::items(Title::Layer, &state(), |_| true);
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Merge)).label,
            "Merge Down"
        );
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Lock)).label,
            "Lock"
        );
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Show)).label,
            "Hide"
        );
        let undone = State {
            locked: true,
            hidden: true,
            merge: "Merge Layers",
            ..state()
        };
        let (items, actions) = super::items(Title::Layer, &undone, |_| true);
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Merge)).label,
            "Merge Layers"
        );
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Lock)).label,
            "Unlock"
        );
        assert_eq!(
            line(&items, &actions, Action::Layer(Command::Show)).label,
            "Show"
        );
        for arrange in [
            Arrange::Front,
            Arrange::Forward,
            Arrange::Backward,
            Arrange::Back,
        ] {
            line(&items, &actions, Action::Layer(Command::Arrange(arrange)));
        }
    }
}
