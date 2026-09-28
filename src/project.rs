//! What a tab holds besides its document: where the document came from,
//! and whether it has drifted from there.
//!
//! Until now every change went straight to disk, so nothing was ever
//! unsaved and `Ctrl+S` would have had nothing to do. A project keeps
//! that decision in one place: the origin says where a save lands, the
//! dirty flag says whether one is owed.

use std::path::{Path, PathBuf};

use crate::doc::Document;

/// Extension of a project file. The contents are the same schema the
/// store writes — a board is a board wherever it is kept.
pub const EXTENSION: &str = "sinopia";

/// What a project file was called while the board went by its working
/// name, Omawhite. Still offered by Open, since a file saved under it is
/// somebody's work and the schema inside never changed; never written.
pub const LEGACY_EXTENSION: &str = "omawhite";

/// What a document with no name of its own is called.
const UNTITLED: &str = "untitled";

/// Where a document lives, and so where `Ctrl+S` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A project file the user named and placed.
    File(PathBuf),
    /// A board in the XDG store, by id. `--open`, `op: open` and `--new`
    /// all land here: they write to the store before the window sees the
    /// document, and `index.json` is what the plugin reads (§5).
    Board(String),
    /// Never written anywhere. The `+` button answers to nobody, so its
    /// first `Ctrl+S` has to ask for a name.
    Untitled,
}

/// One open document, with its file identity.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub doc: Document,
    pub origin: Origin,
    /// Changed since the last save. Drives the dot on the tab and the
    /// question asked before closing it.
    pub dirty: bool,
}

impl Project {
    /// A document already stored where `origin` says: clean.
    pub fn opened(doc: Document, origin: Origin) -> Project {
        Project {
            doc,
            origin,
            dirty: false,
        }
    }

    /// A fresh board that has never been written. Dirty from birth would
    /// be a lie — there is nothing in it to lose.
    pub fn untitled() -> Project {
        Project::opened(Document::new(UNTITLED), Origin::Untitled)
    }

    /// Identity that survives a tab moving or its neighbours closing. The
    /// dialogs answer long after the click, and an index would be stale by
    /// then.
    pub fn key(&self) -> &str {
        &self.doc.id
    }

    /// What the tab is called: the file's own name, else the document's
    /// title.
    pub fn label(&self) -> String {
        match &self.origin {
            Origin::File(path) => file_label(path),
            Origin::Board(_) | Origin::Untitled => self.title(),
        }
    }

    /// Whether a save needs a destination first.
    pub fn needs_a_name(&self) -> bool {
        self.origin == Origin::Untitled
    }

    /// What to put in the Save As dialog's name field.
    pub fn suggested_name(&self) -> String {
        match &self.origin {
            Origin::File(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{UNTITLED}.{EXTENSION}")),
            Origin::Board(_) | Origin::Untitled => format!("{}.{EXTENSION}", self.title()),
        }
    }

    /// The document changed.
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// The document reached `origin`. A Save As re-homes it there, which
    /// is how an `Untitled` becomes a file.
    pub fn saved(&mut self, origin: Origin) {
        self.origin = origin;
        self.dirty = false;
    }

    fn title(&self) -> String {
        match self.doc.title.trim() {
            "" => UNTITLED.to_owned(),
            title => title.to_owned(),
        }
    }
}

/// Which tab comes forward once `closed` is removed from a strip of
/// `len` tabs that had `active` in front. `None` when nothing is left.
///
/// The case worth naming: closing a tab to the *left* of the active one
/// shifts every index after it down, so keeping `active` would quietly
/// bring the neighbour forward instead of the tab the user was working
/// in.
pub fn active_after_close(len: usize, active: usize, closed: usize) -> Option<usize> {
    let left = len.checked_sub(1).filter(|n| *n > 0 && closed < len)?;
    Some(if closed < active {
        active - 1
    } else {
        active.min(left - 1)
    })
}

/// A path's name without its extension — `~/notes.sinopia` is `notes`.
/// A path that ends in `..` or `/` has no name to show, so it falls back
/// rather than showing an empty tab.
fn file_label(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| UNTITLED.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(title: &str) -> Document {
        Document::new(title)
    }

    fn file(path: &str) -> Project {
        Project::opened(doc("ignored"), Origin::File(PathBuf::from(path)))
    }

    #[test]
    fn a_file_is_called_by_its_own_name_not_the_documents_title() {
        // The title travelled with the JSON; the name is what the user
        // chose in the dialog, and it is what the tab must show.
        assert_eq!(file("/home/edu/notes.sinopia").label(), "notes");
    }

    #[test]
    fn a_file_with_no_extension_keeps_its_whole_name() {
        assert_eq!(file("/home/edu/notes").label(), "notes");
    }

    #[test]
    fn a_dotfile_keeps_its_leading_dot() {
        assert_eq!(file("/home/edu/.scratch").label(), ".scratch");
    }

    #[test]
    fn a_path_with_no_name_falls_back_rather_than_showing_nothing() {
        // `file_name` excludes `.` and `..` by design, so both land on the
        // fallback — a tab labelled ".." would be worse than one that
        // admits it has no name.
        for path in ["/", "..", "."] {
            assert_eq!(file(path).label(), UNTITLED, "{path:?}");
        }
    }

    #[test]
    fn a_stored_board_is_called_by_its_title() {
        let p = Project::opened(doc("auth flow"), Origin::Board("01ABC".into()));
        assert_eq!(p.label(), "auth flow");
    }

    #[test]
    fn a_board_with_a_blank_title_still_has_a_label() {
        let p = Project::opened(doc("   "), Origin::Board("01ABC".into()));
        assert_eq!(p.label(), UNTITLED);
    }

    #[test]
    fn a_new_project_is_clean_and_has_no_home() {
        let p = Project::untitled();
        assert!(!p.dirty);
        assert!(p.needs_a_name());
        assert_eq!(p.origin, Origin::Untitled);
    }

    #[test]
    fn a_board_and_a_file_both_know_where_to_save() {
        assert!(!Project::opened(doc("t"), Origin::Board("01ABC".into())).needs_a_name());
        assert!(!file("/home/edu/notes.sinopia").needs_a_name());
    }

    #[test]
    fn touching_dirties_and_saving_cleans() {
        let mut p = Project::opened(doc("t"), Origin::Board("01ABC".into()));
        p.touch();
        assert!(p.dirty);
        p.saved(Origin::Board("01ABC".into()));
        assert!(!p.dirty);
    }

    #[test]
    fn saving_as_rehomes_an_untitled_into_a_file() {
        let mut p = Project::untitled();
        p.touch();
        let path = PathBuf::from("/home/edu/notes.sinopia");
        p.saved(Origin::File(path.clone()));
        assert_eq!(p.origin, Origin::File(path));
        assert!(!p.dirty);
        assert!(!p.needs_a_name());
        assert_eq!(p.label(), "notes");
    }

    #[test]
    fn saving_a_board_as_a_file_leaves_the_store_behind() {
        let mut p = Project::opened(doc("auth flow"), Origin::Board("01ABC".into()));
        p.saved(Origin::File(PathBuf::from("/home/edu/auth.sinopia")));
        // From here Ctrl+S writes the file; the board in the store keeps
        // whatever it had at the last save and stops being this tab's home.
        assert_eq!(p.label(), "auth");
    }

    #[test]
    fn the_suggested_name_carries_the_extension() {
        assert_eq!(Project::untitled().suggested_name(), "untitled.sinopia");
        let board = Project::opened(doc("auth flow"), Origin::Board("01ABC".into()));
        assert_eq!(board.suggested_name(), "auth flow.sinopia");
    }

    #[test]
    fn saving_a_file_again_suggests_the_name_it_already_has() {
        assert_eq!(
            file("/home/edu/notes.sinopia").suggested_name(),
            "notes.sinopia"
        );
    }

    #[test]
    fn closing_the_only_tab_leaves_nothing() {
        assert_eq!(active_after_close(1, 0, 0), None);
    }

    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_same_tab_in_front() {
        // Tabs a b c with c in front; closing a leaves b c, and c is now
        // at 1. Keeping 2 would be out of range; keeping the number would
        // bring b forward instead.
        assert_eq!(active_after_close(3, 2, 0), Some(1));
        assert_eq!(active_after_close(3, 1, 0), Some(0));
    }

    #[test]
    fn closing_a_tab_right_of_the_active_one_changes_nothing() {
        assert_eq!(active_after_close(3, 0, 1), Some(0));
        assert_eq!(active_after_close(3, 0, 2), Some(0));
        assert_eq!(active_after_close(3, 1, 2), Some(1));
    }

    #[test]
    fn closing_the_active_tab_hands_over_to_its_neighbour() {
        // The one that slid into its place, or the new last one.
        assert_eq!(active_after_close(3, 1, 1), Some(1));
        assert_eq!(active_after_close(3, 2, 2), Some(1));
        assert_eq!(active_after_close(3, 0, 0), Some(0));
    }

    #[test]
    fn every_close_lands_on_a_tab_that_exists() {
        for len in 1..8 {
            for active in 0..len {
                for closed in 0..len {
                    match active_after_close(len, active, closed) {
                        Some(next) => assert!(next < len - 1, "{len} {active} {closed} {next}"),
                        None => assert_eq!(len, 1),
                    }
                }
            }
        }
    }

    #[test]
    fn closing_a_tab_that_is_not_there_moves_nothing() {
        assert_eq!(active_after_close(3, 1, 7), None);
    }

    #[test]
    fn the_key_is_the_documents_id_not_a_position() {
        let p = Project::untitled();
        assert_eq!(p.key(), p.doc.id);
        assert!(!p.key().is_empty());
    }
}
