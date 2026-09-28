//! The application menu: File, Edit, View and Layer, as every desktop
//! application has them. Nothing is done here that the board did not
//! already do — each line is a door onto a key, a button or a row's
//! menu, and is written with the keys that do the same. What a line
//! does is an [`Action`]; [`shortcut`] is the one mapping from `Ctrl`
//! and a key to an action, so the hint a line carries cannot come to
//! disagree with the key it names. Pure — `app` runs the actions.

use crate::editor::Command;
use crate::menu::Item;
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
                _ => item,
            };
            (item, action)
        })
        .unzip()
}

#[cfg(test)]
mod tests {
    use super::*;

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
