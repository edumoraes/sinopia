//! Persistência local (ARCHITECTURE.md §6 e §9.3).
//!
//! Layout em disco:
//! ```text
//! <root>/            0700
//!   index.json       0600  — único arquivo que o plugin lê
//!   boards/<id>.json 0600  — documento
//!   thumbs/<id>.png  0600  — preview (fase posterior)
//!   blobs/<sha256>   0600  — imagens (fase posterior)
//! ```
//! Escrita é sempre atômica: tmp no mesmo diretório + rename.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::doc::Document;

/// Versão do schema do index.json (superfície entre plugin e binário).
pub const INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: String,
    pub title: String,
    /// Unix epoch em segundos do último save.
    pub updated_at: u64,
    /// Path relativo ao root (ex.: `thumbs/<id>.png`), quando existir.
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

/// Resolve o diretório de dados: `$XDG_DATA_HOME/omawhite` ou
/// `~/.local/share/omawhite`. Função pura para ser testável.
pub fn data_root(xdg_data_home: Option<&str>, home: &str) -> PathBuf {
    match xdg_data_home {
        Some(x) if !x.is_empty() => Path::new(x).join("omawhite"),
        _ => Path::new(home).join(".local/share/omawhite"),
    }
}

impl Store {
    /// Abre (criando se necessário) o layout em `root`, com permissões 0700
    /// nos diretórios.
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

    #[allow(dead_code)] // usado nos testes; export (§15.5) o usa em produção
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Grava o documento e atualiza o index. Atômico nos dois arquivos.
    pub fn save(&self, doc: &Document) -> anyhow::Result<()> {
        self.save_at(doc, unix_now())
    }

    /// Como `save`, com timestamp explícito (determinismo nos testes).
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

    /// Carrega um board pelo id. O id vem de CLI/socket: valida antes de
    /// virar path (§9 — nada de `..` nem separador).
    pub fn load(&self, id: &str) -> anyhow::Result<Document> {
        validate_id(id)?;
        let path = self.board_path(id);
        let s = std::fs::read_to_string(&path).with_context(|| format!("lendo board {path:?}"))?;
        Document::from_json(&s)
    }

    /// Entradas do índice, mais recente primeiro.
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
        let s = std::fs::read_to_string(&path).with_context(|| format!("lendo {path:?}"))?;
        let index: Index = serde_json::from_str(&s).context("index.json inválido")?;
        anyhow::ensure!(
            index.schema == INDEX_SCHEMA_VERSION,
            "index.json com schema {} (esperado {INDEX_SCHEMA_VERSION})",
            index.schema
        );
        Ok(index)
    }
}

/// Ids viram nomes de arquivo: só alfanumérico, `-` e `_`, tamanho limitado.
fn validate_id(id: &str) -> anyhow::Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    anyhow::ensure!(ok, "id de board inválido: {id:?}");
    Ok(())
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(dir)
        .with_context(|| format!("criando {dir:?}"))?;
    // `recursive` não aplica mode em diretório pré-existente; garante o §9.3.
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, perms).with_context(|| format!("chmod 0700 {dir:?}"))?;
    Ok(())
}

/// Escrita atômica com 0600: tmp oculto no mesmo diretório + rename.
fn write_private_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().context("destino sem diretório pai")?;
    let name = path
        .file_name()
        .context("destino sem nome")?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("criando temporário {tmp:?}"))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("renomeando para {path:?}"))?;
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
            assert!(dir.is_dir(), "{dir:?} deve existir");
            assert_eq!(mode_of(dir), 0o700, "{dir:?} deve ser 0700");
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

        // Nenhum arquivo temporário sobrando em lugar nenhum.
        for dir in [store.root().to_path_buf(), store.root().join("boards")] {
            for entry in std::fs::read_dir(dir).unwrap() {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy().into_owned();
                assert!(!name.contains(".tmp"), "sobrou temporário: {name}");
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
        let old = doc_with_title("velho");
        let new = doc_with_title("novo");
        store.save_at(&old, 100).unwrap();
        store.save_at(&new, 200).unwrap();

        let titles: Vec<_> = store
            .index()
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, ["novo", "velho"]);
    }

    #[test]
    fn load_rejects_ids_that_are_not_plain_names() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("d")).unwrap();
        for evil in ["../fora", "a/b", "", ".", "id com espaço", "x\0y"] {
            assert!(
                store.load(evil).is_err(),
                "id {evil:?} deveria ser recusado"
            );
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
        // XDG vazio conta como ausente (spec do XDG basedir).
        assert_eq!(
            data_root(Some(""), "/home/edu"),
            PathBuf::from("/home/edu/.local/share/omawhite")
        );
    }
}
