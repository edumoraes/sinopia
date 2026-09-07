//! Local persistence (ARCHITECTURE.md §6 and §9.3).
//!
//! On-disk layout:
//! ```text
//! <root>/            0700
//!   index.json       0600  — the only file the plugin reads
//!   boards/<id>.json 0600  — document
//!   thumbs/<id>.png  0600  — preview (later phase)
//!   blobs/<sha256>   0600  — images (later phase)
//! ```
//! Writes are always atomic: tmp in the same directory + rename.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::brush::Edits;
use crate::doc::Document;

/// Schema version of index.json (the surface between plugin and binary).
pub const INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: String,
    pub title: String,
    /// Unix epoch seconds of the last save.
    pub updated_at: u64,
    /// Path relative to root (e.g. `thumbs/<id>.png`), when present.
    pub thumb: Option<String>,
    /// Where the project file the user named lives. Absent for a draft,
    /// which lives in `boards/<id>.json` and has no name of its own —
    /// absent on disk the way a raster layer's kind is, so an index
    /// written before recents existed still opens meaning what it meant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Index {
    schema: u32,
    boards: Vec<IndexEntry>,
}

pub struct Store {
    root: PathBuf,
}

/// Resolves the data directory: `$XDG_DATA_HOME/omawhite` or
/// `~/.local/share/omawhite`. Pure so it stays testable.
pub fn data_root(xdg_data_home: Option<&str>, home: &str) -> PathBuf {
    match xdg_data_home {
        Some(x) if !x.is_empty() => Path::new(x).join("omawhite"),
        _ => Path::new(home).join(".local/share/omawhite"),
    }
}

impl Store {
    /// Opens (creating if needed) the layout under `root`, directories at
    /// 0700.
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Store> {
        let root = root.into();
        for dir in [
            root.clone(),
            root.join("boards"),
            root.join("thumbs"),
            root.join("blobs"),
        ] {
            create_private_dir(&dir)?;
        }
        Ok(Store { root })
    }

    #[allow(dead_code)] // used in tests; export (§15.5) uses it in production
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Writes the document and updates the index. Atomic on both files.
    pub fn save(&self, doc: &Document) -> anyhow::Result<()> {
        self.save_at(doc, unix_now())
    }

    /// Like `save`, with an explicit timestamp (deterministic tests).
    pub fn save_at(&self, doc: &Document, updated_at: u64) -> anyhow::Result<()> {
        validate_id(&doc.id)?;
        write_private_atomic(&self.board_path(&doc.id), doc.to_json()?.as_bytes())?;
        self.record(IndexEntry {
            id: doc.id.clone(),
            title: doc.title.clone(),
            updated_at,
            thumb: None,
            path: None,
        })
    }

    /// Records a project file among the recents. The file itself was
    /// already written where the user put it, by `save_document_to`: the
    /// store keeps the memory of it, never a copy — a safety save is a
    /// draft's privilege, and a named file is written only when asked.
    pub fn remember_file(&self, doc: &Document, path: &Path) -> anyhow::Result<()> {
        self.remember_file_at(doc, path, unix_now())
    }

    /// Like `remember_file`, with an explicit timestamp.
    pub fn remember_file_at(
        &self,
        doc: &Document,
        path: &Path,
        updated_at: u64,
    ) -> anyhow::Result<()> {
        validate_id(&doc.id)?;
        self.record(IndexEntry {
            id: doc.id.clone(),
            title: doc.title.clone(),
            updated_at,
            thumb: None,
            path: Some(path.to_path_buf()),
        })
    }

    /// Drops what `id` names from the recents. What a project whose file
    /// has gone earns once opening it has actually failed — a list that
    /// forgot it the moment a drive was unmounted would be worse.
    /// Removing the memory of a draft leaves its board on disk.
    pub fn forget(&self, id: &str) -> anyhow::Result<()> {
        self.drop_entries(|e| e.id == id)
    }

    /// Drops whatever recent points at `path`. What the app has when a
    /// file refuses to open: it knows where it reached, not which entry
    /// sent it there.
    pub fn forget_path(&self, path: &Path) -> anyhow::Result<()> {
        self.drop_entries(|e| e.path.as_deref() == Some(path))
    }

    fn drop_entries(&self, doomed: impl Fn(&IndexEntry) -> bool) -> anyhow::Result<()> {
        let mut index = self.read_index()?;
        let before = index.boards.len();
        index.boards.retain(|e| !doomed(e));
        if index.boards.len() == before {
            return Ok(());
        }
        self.write_index(&index)
    }

    /// Puts `entry` at the head of the recents, replacing whatever stood
    /// for the same project. Identity is the document's id *or* the path:
    /// a draft saved under a name has to stop being two entries, and a
    /// file overwritten by another document has to stop being one.
    fn record(&self, entry: IndexEntry) -> anyhow::Result<()> {
        let mut index = self.read_index()?;
        index
            .boards
            .retain(|e| e.id != entry.id && (e.path.is_none() || e.path != entry.path));
        index.boards.push(entry);
        index.boards.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        self.write_index(&index)
    }

    fn write_index(&self, index: &Index) -> anyhow::Result<()> {
        write_private_atomic(
            &self.root.join("index.json"),
            serde_json::to_string_pretty(index)?.as_bytes(),
        )
    }

    /// Loads a board by id. The id comes from CLI/socket: validated before
    /// it becomes a path (§9 — no `..`, no separators).
    pub fn load(&self, id: &str) -> anyhow::Result<Document> {
        validate_id(id)?;
        let path = self.board_path(id);
        let s =
            std::fs::read_to_string(&path).with_context(|| format!("reading board {path:?}"))?;
        Document::from_json(&s)
    }

    /// What the person has changed about their brushes, or nothing at
    /// all when the file is absent or will not parse. A brush library
    /// is a convenience, never a board: it must not stop the window
    /// opening, so a bad file is logged and passed over.
    pub fn brushes(&self) -> Edits {
        let path = self.brushes_path();
        let Ok(s) = std::fs::read_to_string(&path) else {
            return Edits::default();
        };
        match serde_json::from_str(&s) {
            Ok(edits) => edits,
            Err(e) => {
                log::warn!("ignoring {path:?}: {e}");
                Edits::default()
            }
        }
    }

    /// Writes them back, atomically and at 0600 like everything else
    /// here.
    pub fn save_brushes(&self, edits: &Edits) -> anyhow::Result<()> {
        write_private_atomic(
            &self.brushes_path(),
            serde_json::to_string_pretty(edits)?.as_bytes(),
        )
    }

    fn brushes_path(&self) -> PathBuf {
        self.root.join("brushes.json")
    }

    /// Index entries, most recent first.
    pub fn index(&self) -> anyhow::Result<Vec<IndexEntry>> {
        let mut index = self.read_index()?;
        index.boards.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        Ok(index.boards)
    }

    /// Writes `bytes` under `blobs/<sha256>` and returns that hash.
    /// Content addressed: the same bytes are the same file, so pasting a
    /// screenshot twice costs one blob.
    pub fn write_blob(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let hash = sha256_hex(bytes);
        let path = self.root.join("blobs").join(&hash);
        if !path.exists() {
            write_private_atomic(&path, bytes)?;
        }
        Ok(hash)
    }

    /// Reads the blob `hash` names. The name arrives from a board on disk,
    /// so it is checked before it becomes a path (§9.3).
    pub fn read_blob(&self, hash: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(crate::doc::is_blob_hash(hash), "not a blob name: {hash:?}");
        let path = self.root.join("blobs").join(hash);
        std::fs::read(&path).with_context(|| format!("reading blob {hash}"))
    }

    fn board_path(&self, id: &str) -> PathBuf {
        self.root.join("boards").join(format!("{id}.json"))
    }

    fn read_index(&self) -> anyhow::Result<Index> {
        let path = self.root.join("index.json");
        if !path.exists() {
            return Ok(Index {
                schema: INDEX_SCHEMA_VERSION,
                boards: Vec::new(),
            });
        }
        let s = std::fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
        let index: Index = serde_json::from_str(&s).context("invalid index.json")?;
        anyhow::ensure!(
            index.schema == INDEX_SCHEMA_VERSION,
            "index.json has schema {} (expected {INDEX_SCHEMA_VERSION})",
            index.schema
        );
        Ok(index)
    }
}

/// Writes `doc` as a project file at a path the user chose.
///
/// Atomic like everything else, but it keeps the process umask instead of
/// forcing 0600: that mode belongs to `~/.local/share/omawhite` (§9.3),
/// and a project saved into a shared directory is the user's to share.
///
/// The path is not measured against the export allowlist (§8.2). That
/// list guards destinations arriving over the socket or the CLI; this one
/// came back from a portal dialog the user drove, which is the one place
/// the distinction carries weight.
pub fn save_document_to(path: &Path, doc: &Document) -> anyhow::Result<()> {
    write_atomic(path, doc.to_json()?.as_bytes(), None).with_context(|| format!("saving {path:?}"))
}

/// Reads a project file from a path the user chose.
///
/// Nothing about the parse is loosened for coming from outside the store:
/// the schema is closed, and every `blob` is checked as a bare sha256
/// before it can ever become a path, so a file from elsewhere cannot name
/// one outside `blobs/`.
pub fn load_document_from(path: &Path) -> anyhow::Result<Document> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    Document::from_json(&s).with_context(|| format!("parsing {path:?}"))
}

/// A file, read whole, refused past `max`.
///
/// The handle answers, never the path. Stat-then-open lets a FIFO
/// through — it reports no length and then blocks whoever reads it for
/// as long as nobody writes, which on the event loop's own thread is the
/// window gone with the board's unsaved work in it — and it lets a plain
/// file grow between the two calls. The cap is on the read for the same
/// reason: a character device has no length either, and `/dev/zero`
/// delivers NUL, which is valid UTF-8 all the way to an OOM.
pub fn read_capped(path: &Path, max: u64) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).with_context(|| format!("reading {path:?}"))?;
    let meta = file.metadata().with_context(|| format!("reading {path:?}"))?;
    anyhow::ensure!(meta.is_file(), "{path:?} is not a regular file");
    let mut bytes = Vec::new();
    let read = file
        .take(max + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {path:?}"))?;
    anyhow::ensure!(
        read as u64 <= max,
        "{path:?} is over the {max} bytes it may be"
    );
    Ok(bytes)
}

/// Ids become file names: alphanumeric, `-` and `_` only, bounded length.
fn validate_id(id: &str) -> anyhow::Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    anyhow::ensure!(ok, "invalid board id: {id:?}");
    Ok(())
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(dir)
        .with_context(|| format!("creating {dir:?}"))?;
    // `recursive` does not apply the mode to a pre-existing dir; enforce §9.3.
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, perms).with_context(|| format!("chmod 0700 {dir:?}"))?;
    Ok(())
}

/// Atomic write at 0600: what everything under the root gets (§9.3).
fn write_private_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic(path, bytes, Some(0o600))
}

/// Hidden tmp in the same directory, then rename — so a reader never sees
/// half a file, and a crash never leaves one. `mode` is the permission to
/// create the tmp with, or `None` to leave it to the umask.
///
/// Shared with [`crate::export`], which owes the same terms: same
/// directory, atomic rename, explicit mode.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .context("destination has no parent directory")?;
    let name = path
        .file_name()
        .context("destination has no file name")?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        if let Some(mode) = mode {
            opts.mode(mode);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("creating temp file {tmp:?}"))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // A failed rename would otherwise leave the tmp lying beside the
    // destination, in a directory the user chose and looks at.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e)).with_context(|| format!("renaming to {path:?}"));
    }
    Ok(())
}

/// sha256 of `bytes`, in lowercase hex — a blob's name.
/// The name bytes are kept under. Public so a caller can ask what a
/// file *would* be called before writing it — [`crate::graft`]'s images
/// arrive named, and bytes that are not what they claim never reach the
/// store.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Camera, Document, Element, Rect};
    use std::os::unix::fs::MetadataExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn a_capped_read_takes_a_file_and_refuses_one_that_is_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().join("f");
        std::fs::write(&at, b"hello").unwrap();
        assert_eq!(read_capped(&at, 5).unwrap(), b"hello");
        let err = read_capped(&at, 4).unwrap_err().to_string();
        assert!(err.contains("over the 4 bytes"), "{err}");
    }

    #[test]
    fn a_capped_read_refuses_anything_that_is_not_a_regular_file() {
        // What the length of a path cannot answer. A character device
        // reports none and goes on delivering — `/dev/zero` to an OOM,
        // since NUL is valid UTF-8 — and a FIFO reports none and then
        // blocks the reader for as long as nobody writes, which on the
        // event loop's own thread is the window gone with the board's
        // unsaved work in it. The check is on the open handle, so there
        // is no gap between asking and reading either.
        let dev = Path::new("/dev/zero");
        if dev.exists() {
            let err = read_capped(dev, 1024).unwrap_err().to_string();
            assert!(err.contains("not a regular file"), "{err}");
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(
            read_capped(dir.path(), 1024).is_err(),
            "and a directory is not one either"
        );
    }

    fn doc_with_title(title: &str) -> Document {
        let mut d = Document::new(title);
        d.camera = Camera {
            x: 10.0,
            y: -5.0,
            zoom: 2.0,
        };
        d.elements.push(Element::Rect(Rect {
            id: "el_01".into(),
            layer: d.layers[0].id.clone(),
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            rotation: 0.0,
            stroke: Some("#222".into()),
            fill: None,
            text: None,
        }));
        d
    }

    #[test]
    fn open_creates_private_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("omawhite");
        let store = Store::open(&root).unwrap();
        assert_eq!(store.root(), root);
        for dir in [
            &root,
            &root.join("boards"),
            &root.join("thumbs"),
            &root.join("blobs"),
        ] {
            assert!(dir.is_dir(), "{dir:?} must exist");
            assert_eq!(mode_of(dir), 0o700, "{dir:?} must be 0700");
        }
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let doc = doc_with_title("auth flow");
        store.save(&doc).unwrap();
        assert_eq!(store.load(&doc.id).unwrap(), doc);
    }

    #[test]
    fn the_brushes_are_kept_beside_the_boards_and_just_as_private() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        assert_eq!(store.brushes(), Edits::default(), "nothing kept yet");

        let edits = Edits {
            held: Some(crate::brush::Held {
                set: "Basic".into(),
                name: "Airbrush".into(),
            }),
            brushes: vec![crate::brush::Edit {
                set: "Basic".into(),
                name: "Airbrush".into(),
                brush: crate::brush::Brush {
                    size: 42.0,
                    ..crate::brush::Brush::default()
                },
            }],
            slots: Vec::new(),
        };
        store.save_brushes(&edits).unwrap();
        assert_eq!(mode_of(&store.root().join("brushes.json")), 0o600);
        assert_eq!(store.brushes(), edits);
    }

    #[test]
    fn a_brush_file_that_will_not_parse_does_not_stop_the_window() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        std::fs::write(store.root().join("brushes.json"), "{ not json").unwrap();
        assert_eq!(
            store.brushes(),
            Edits::default(),
            "the brushes are a convenience, never a board"
        );
    }

    #[test]
    fn save_is_atomic_and_private() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let doc = doc_with_title("t");
        store.save(&doc).unwrap();

        let board = store.root().join("boards").join(format!("{}.json", doc.id));
        assert_eq!(mode_of(&board), 0o600);
        assert_eq!(mode_of(&store.root().join("index.json")), 0o600);

        // No temp file left behind anywhere.
        for dir in [store.root().to_path_buf(), store.root().join("boards")] {
            for entry in std::fs::read_dir(dir).unwrap() {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy().into_owned();
                assert!(!name.contains(".tmp"), "leftover temp file: {name}");
            }
        }
    }

    #[test]
    fn saving_twice_updates_index_without_duplicating() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let mut doc = doc_with_title("v1");
        store.save_at(&doc, 100).unwrap();
        doc.title = "v2".into();
        store.save_at(&doc, 200).unwrap();

        let index = store.index().unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].title, "v2");
        assert_eq!(index[0].updated_at, 200);
        assert_eq!(index[0].thumb, None);
    }

    #[test]
    fn index_lists_most_recent_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let old = doc_with_title("old");
        let new = doc_with_title("new");
        store.save_at(&old, 100).unwrap();
        store.save_at(&new, 200).unwrap();

        let titles: Vec<_> = store
            .index()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, ["new", "old"]);
    }

    #[test]
    fn a_file_is_remembered_without_the_store_keeping_a_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let doc = doc_with_title("plan");
        let path = tmp.path().join("plan.omawhite");
        store.remember_file_at(&doc, &path, 100).unwrap();

        let index = store.index().unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].path, Some(path));
        // The recents remember where it is; the safety save is a draft's
        // privilege, so nothing was written under boards/.
        assert!(
            !store.board_path(&doc.id).exists(),
            "remembering a file must not copy it into the store"
        );
    }

    #[test]
    fn a_draft_saved_under_a_name_stops_being_two_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let doc = doc_with_title("plan");
        store.save_at(&doc, 100).unwrap();
        let path = tmp.path().join("plan.omawhite");
        store.remember_file_at(&doc, &path, 200).unwrap();

        let index = store.index().unwrap();
        assert_eq!(index.len(), 1, "the project moved home, it did not fork");
        assert_eq!(index[0].path, Some(path));
        assert_eq!(index[0].updated_at, 200);
    }

    #[test]
    fn two_documents_written_to_one_path_leave_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let path = tmp.path().join("plan.omawhite");
        store
            .remember_file_at(&doc_with_title("first"), &path, 100)
            .unwrap();
        store
            .remember_file_at(&doc_with_title("second"), &path, 200)
            .unwrap();

        let index = store.index().unwrap();
        assert_eq!(index.len(), 1, "one file is one recent, whatever wrote it");
        assert_eq!(index[0].title, "second");
    }

    #[test]
    fn two_drafts_are_not_folded_into_one_by_their_missing_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        store.save_at(&doc_with_title("a"), 100).unwrap();
        store.save_at(&doc_with_title("b"), 200).unwrap();
        assert_eq!(
            store.index().unwrap().len(),
            2,
            "`path: None` is the absence of a name, not a name they share"
        );
    }

    #[test]
    fn forgetting_drops_the_memory_and_leaves_the_board() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let doc = doc_with_title("t");
        store.save(&doc).unwrap();
        store.forget(&doc.id).unwrap();

        assert!(store.index().unwrap().is_empty());
        assert!(
            store.board_path(&doc.id).exists(),
            "forgetting is about the list, not about the board"
        );
        // Forgetting what was never there is not an error.
        store.forget("01NOTHERE").unwrap();
    }

    #[test]
    fn an_index_written_before_recents_still_opens() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        std::fs::write(
            store.root().join("index.json"),
            r#"{ "schema": 1, "boards": [
                 { "id": "01ABC", "title": "old", "updated_at": 7, "thumb": null }
               ] }"#,
        )
        .unwrap();

        let index = store.index().unwrap();
        assert_eq!(index.len(), 1);
        assert!(
            index[0].path.is_none(),
            "no path is what a board has always been"
        );
    }

    #[test]
    fn a_draft_entry_writes_no_path_field_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        store.save(&doc_with_title("t")).unwrap();
        let raw = std::fs::read_to_string(store.root().join("index.json")).unwrap();
        assert!(
            !raw.contains("path"),
            "absent on disk, so an older build reads it as the board it is"
        );
    }

    #[test]
    fn load_rejects_ids_that_are_not_plain_names() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        for evil in ["../outside", "a/b", "", ".", "id with space", "x\0y"] {
            assert!(store.load(evil).is_err(), "id {evil:?} should be rejected");
        }
    }

    #[test]
    fn data_root_prefers_xdg_and_falls_back_to_home() {
        assert_eq!(
            data_root(Some("/custom/data"), "/home/edu"),
            PathBuf::from("/custom/data/omawhite")
        );
        assert_eq!(
            data_root(None, "/home/edu"),
            PathBuf::from("/home/edu/.local/share/omawhite")
        );
        // Empty XDG counts as unset (XDG basedir spec).
        assert_eq!(
            data_root(Some(""), "/home/edu"),
            PathBuf::from("/home/edu/.local/share/omawhite")
        );
    }

    #[test]
    fn writing_a_blob_names_it_by_its_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let hash = store.write_blob(b"omawhite").unwrap();
        assert_eq!(
            hash,
            "6d72b889920b8d63704bf588d33b5a9d09ac16f33f7a8a201dcd618136cd94bf"
        );
        let path = store.root().join("blobs").join(&hash);
        assert_eq!(std::fs::read(&path).unwrap(), b"omawhite");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn writing_the_same_bytes_twice_keeps_one_blob() {
        // Content addressing is the point: pasting the same screenshot
        // twice costs one file.
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let a = store.write_blob(b"same").unwrap();
        let b = store.write_blob(b"same").unwrap();
        assert_eq!(a, b);
        let blobs: Vec<_> = std::fs::read_dir(store.root().join("blobs"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(blobs.len(), 1, "{blobs:?}");
    }

    #[test]
    fn blobs_roundtrip_through_read_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        let bytes = vec![0u8, 1, 2, 250, 251, 255];
        let hash = store.write_blob(&bytes).unwrap();
        assert_eq!(store.read_blob(&hash).unwrap(), bytes);
    }

    #[test]
    fn reading_a_blob_whose_name_is_not_a_hash_is_an_error() {
        // The name reaches here from a board on disk; it never becomes a
        // path without being checked (§9.3).
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        for name in ["../index.json", "..", "", "boards/x"] {
            let err = store.read_blob(name).unwrap_err().to_string();
            assert!(err.contains("blob"), "{name:?}: {err}");
        }
    }

    #[test]
    fn a_project_file_roundtrips_through_a_path_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.omawhite");
        let doc = doc_with_title("auth flow");
        save_document_to(&path, &doc).unwrap();
        assert_eq!(load_document_from(&path).unwrap(), doc);
    }

    #[test]
    fn saving_a_project_leaves_no_temp_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        save_document_to(&tmp.path().join("notes.omawhite"), &doc_with_title("t")).unwrap();
        let left: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["notes.omawhite"]);
    }

    #[test]
    fn a_project_file_keeps_the_umask_not_the_stores_0600() {
        // 0600 is what `~/.local/share/omawhite` is for (§9.3). Forcing it
        // on a file the user placed in a shared directory would quietly
        // undo the sharing they asked for.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.omawhite");
        save_document_to(&path, &doc_with_title("t")).unwrap();
        assert_ne!(mode_of(&path), 0o600);
    }

    #[test]
    fn saving_a_project_replaces_what_was_there() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.omawhite");
        save_document_to(&path, &doc_with_title("first")).unwrap();
        save_document_to(&path, &doc_with_title("second")).unwrap();
        assert_eq!(load_document_from(&path).unwrap().title, "second");
    }

    #[test]
    fn a_project_file_that_is_not_a_board_is_an_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.omawhite");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(
            load_document_from(&path)
                .unwrap_err()
                .to_string()
                .contains("notes.omawhite")
        );
    }

    #[test]
    fn a_project_file_from_a_newer_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("notes.omawhite");
        let json = doc_with_title("t")
            .to_json()
            .unwrap()
            .replace("\"schema\": 1", "\"schema\": 999");
        std::fs::write(&path, json).unwrap();
        let err = load_document_from(&path).unwrap_err().to_string();
        assert!(err.contains("notes.omawhite"), "{err}");
    }

    #[test]
    fn a_project_file_cannot_smuggle_a_blob_name_that_is_a_path() {
        // The parse is the guard, and it does not loosen for a file that
        // came from outside the store.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hostile.omawhite");
        std::fs::write(
            &path,
            r#"{"schema":1,"id":"01","title":"t",
                "camera":{"x":0,"y":0,"zoom":1},
                "elements":[{"type":"image","id":"i","x":0,"y":0,"w":1,"h":1,
                             "blob":"../../../etc/passwd"}]}"#,
        )
        .unwrap();
        assert!(load_document_from(&path).is_err());
    }

    #[test]
    fn a_missing_project_file_is_an_error_that_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gone.omawhite");
        let err = load_document_from(&path).unwrap_err().to_string();
        assert!(err.contains("gone.omawhite"), "{err}");
    }

    #[test]
    fn saving_into_a_directory_that_is_not_there_fails_without_a_stray_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope").join("notes.omawhite");
        assert!(save_document_to(&path, &doc_with_title("t")).is_err());
        assert!(!tmp.path().join("nope").exists());
    }
}
