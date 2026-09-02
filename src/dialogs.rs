//! System dialogs, over `xdg-desktop-portal`.
//!
//! Opening a file, naming a save and asking about unsaved work each need
//! a file browser, a text field or a modal, and the canvas has none of
//! them. The desktop already does. Going through the portal also makes
//! the destination the user's own choice rather than a path this process
//! guessed — which is why a path from here is not measured against the
//! export allowlist (§8.2), and a path from the socket still is.
//!
//! A dialog blocks the thread it runs on, so it never runs on this one:
//! it is built here, where the window handle is valid, then moved to a
//! thread that answers through the sink. Same shape as `clipboard` and
//! `gestures`, and untested for the same reason.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use winit::window::Window;

use crate::project::EXTENSION;

/// What the user said about work that would be lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

/// Exactly one of these comes back per dialog, cancelled or not, so the
/// caller always learns the dialog is gone.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Empty when the user picked nothing.
    Opened(Vec<PathBuf>),
    /// `None` when the user named nothing. The key says which project
    /// asked: the answer arrives long after the click, by which time the
    /// tab may have moved.
    SaveTo {
        key: String,
        path: Option<PathBuf>,
    },
    Close {
        key: String,
        answer: Answer,
    },
}

pub type Sink = Arc<dyn Fn(Reply) + Send + Sync>;

const FILTER: &str = "Omawhite board";

/// Asks for boards to open. Several at once is allowed: each becomes a
/// tab.
pub fn open(window: &Window, sink: Sink) {
    let dialog = FileDialog::new()
        .set_title("Open board")
        .add_filter(FILTER, &[EXTENSION, "json"])
        .set_parent(window);
    spawn(
        "open",
        move || Reply::Opened(dialog.pick_files().unwrap_or_default()),
        sink,
    );
}

/// Asks where to write the project `key` names. `at` is where it already
/// lives, so saving again starts in the directory it came from.
pub fn save_as(window: &Window, sink: Sink, key: String, suggested: &str, at: Option<&Path>) {
    let mut dialog = FileDialog::new()
        .set_title("Save board as")
        .add_filter(FILTER, &[EXTENSION])
        .set_file_name(suggested)
        .set_can_create_directories(true)
        .set_parent(window);
    if let Some(dir) = at.and_then(Path::parent).filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    spawn(
        "save as",
        move || {
            let path = dialog.save_file().map(|p| with_extension(&p));
            Reply::SaveTo { key, path }
        },
        sink,
    );
}

/// Asks what to do about a board with unsaved changes.
///
/// There is no portal for a question, so this one goes through `zenity`.
/// When it is missing the answer comes back as a cancel, which is the
/// failure that loses nothing: the tab stays, still unsaved.
pub fn confirm_close(window: &Window, sink: Sink, key: String, label: &str) {
    let dialog = MessageDialog::new()
        .set_level(MessageLevel::Warning)
        .set_title("Unsaved changes")
        .set_description(format!("“{label}” has changes that were never saved."))
        .set_buttons(MessageButtons::YesNoCancelCustom(
            "Save".into(),
            "Discard".into(),
            "Cancel".into(),
        ))
        .set_parent(window);
    spawn(
        "confirm close",
        move || {
            let answer = match dialog.show() {
                MessageDialogResult::Custom(button) if button == "Save" => Answer::Save,
                MessageDialogResult::Custom(button) if button == "Discard" => Answer::Discard,
                MessageDialogResult::Yes => Answer::Save,
                MessageDialogResult::No => Answer::Discard,
                // Dismissed with the window button or Esc: the safe reading
                // is that the user did not mean to lose anything.
                _ => Answer::Cancel,
            };
            Reply::Close { key, answer }
        },
        sink,
    );
}

/// Runs `ask` on a thread of its own and hands the answer to the sink. A
/// portal call can take as long as the user takes; the frame loop cannot
/// wait for it.
fn spawn(what: &'static str, ask: impl FnOnce() -> Reply + Send + 'static, sink: Sink) {
    let spawned = std::thread::Builder::new()
        .name(format!("omawhite-{}", what.replace(' ', "-")))
        .spawn(move || sink(ask()));
    if let Err(e) = spawned {
        log::error!("{what} dialog: {e}");
    }
}

/// Gives a named file the board extension unless the user typed one.
/// Portals hand back exactly what was typed, and a board called `notes`
/// is a board nothing will open by double-click.
fn with_extension(path: &Path) -> PathBuf {
    match path.extension() {
        Some(_) => path.to_path_buf(),
        None => path.with_extension(EXTENSION),
    }
}
