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

        let mut index = self.read_index()?;
        index.boards.retain(|e| e.id != doc.id);
        index.boards.push(IndexEntry {
            id: doc.id.clone(),
            title: doc.title.clone(),
            updated_at,
            thumb: None,
        });
        index.boards.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        write_private_atomic(
            &self.root.join("index.json"),
            serde_json::to_string_pretty(&index)?.as_bytes(),
        )?;
        Ok(())
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

    /// Index entries, most recent first.
    pub fn index(&self) -> anyhow::Result<Vec<IndexEntry>> {
        let mut index = self.read_index()?;
        index.boards.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(index.boards)
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

/// Atomic write at 0600: hidden tmp in the same directory + rename.
fn write_private_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path
        .parent()
        .context("destination has no parent directory")?;
    let name = path
        .file_name()
        .context("destination has no file name")?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("creating temp file {tmp:?}"))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {path:?}"))?;
    Ok(())
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

    fn doc_with_title(title: &str) -> Document {
        let mut d = Document::new(title);
        d.camera = Camera {
            x: 10.0,
            y: -5.0,
            zoom: 2.0,
        };
        d.elements.push(Element::Rect(Rect {
            id: "el_01".into(),
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
}
