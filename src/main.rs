//! omawhite — local-first whiteboard for Omarchy (see ARCHITECTURE.md).
//!
//! Single instance: if the socket answers, the intent is forwarded and this
//! process exits; otherwise this process becomes the main instance.

mod app;
mod bitmap;
mod brush;
mod cli;
mod clipboard;
mod curve;
mod dialogs;
mod doc;
mod dock;
mod editor;
mod geom;
mod gestures;
mod gfx;
mod grid;
mod ipc;
mod layers;
mod palette;
mod project;
mod props;
mod scene;
mod select;
mod store;
mod tablet;
mod tabs;
mod text;
mod theme;

use anyhow::Context as _;
use clap::Parser as _;

use cli::{Action, Cli};
use ipc::client::try_forward;
use ipc::proto::{ExportFormat, Request};
use project::{Origin, Project};
use store::Store;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    let socket_path = match &cli.socket {
        Some(p) => p.clone(),
        None => default_socket_path()?,
    };

    let action = cli.action();
    let request = match &action {
        Action::Default => Request::Raise,
        Action::New => Request::New,
        Action::Open(id) => Request::Open { id: id.clone() },
        Action::OpenFile(path) => Request::OpenFile { path: path.clone() },
        Action::Export(dir) => Request::Export {
            dir: dir.clone(),
            formats: vec![ExportFormat::Png, ExportFormat::Json, ExportFormat::Md],
        },
        Action::Shutdown => Request::Shutdown,
    };

    // §5: a second omawhite becomes a command on the socket, not a second window.
    if let Some(reply) = try_forward(&socket_path, &request)? {
        println!("{reply}");
        let denied = serde_json::from_str::<serde_json::Value>(&reply)
            .map(|v| v["ev"] == "denied")
            .unwrap_or(false);
        if denied {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Nobody listening: actions that need a live instance fail explicitly.
    match action {
        Action::Export(_) => {
            anyhow::bail!("no omawhite instance running; open the board before exporting")
        }
        Action::Shutdown => {
            log::info!("no instance running; nothing to shut down");
            Ok(())
        }
        Action::Default | Action::New | Action::Open(_) | Action::OpenFile(_) => {
            let store = open_default_store()?;
            let project = match &action {
                // Nothing is written for a new board. It is a draft, and
                // a draft earns its file the first time it is drawn on —
                // which is what keeps `+` from leaving empty boards in
                // the recents.
                Action::New => Project::untitled(),
                Action::Open(id) => {
                    Project::opened(store.load(id)?, Origin::Board(id.clone()))
                }
                Action::OpenFile(path) => {
                    Project::opened(store::load_document_from(path)?, Origin::File(path.clone()))
                }
                // No flags: the most recent project, whichever kind it
                // is, or an untitled one when there is no history.
                _ => match store.index()?.into_iter().next() {
                    Some(entry) => open_recent(&store, entry)?,
                    None => Project::untitled(),
                },
            };
            app::run(store, project, socket_path, cli.smoke_frames)
        }
    }
}

/// Opens what a recents entry names. A file whose path has gone is
/// dropped from the list here, where the failure actually happened —
/// that is the one thing that removes an entry, so a project on a drive
/// nobody has mounted keeps its place until someone reaches for it.
fn open_recent(store: &Store, entry: store::IndexEntry) -> anyhow::Result<Project> {
    match entry.path {
        None => Ok(Project::opened(
            store.load(&entry.id)?,
            Origin::Board(entry.id),
        )),
        Some(path) => match store::load_document_from(&path) {
            Ok(doc) => Ok(Project::opened(doc, Origin::File(path))),
            Err(e) => {
                log::warn!("dropping {path:?} from the recents: {e:#}");
                store.forget(&entry.id)?;
                Ok(Project::untitled())
            }
        },
    }
}

fn default_socket_path() -> anyhow::Result<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR not set (required for the socket, §5)")?;
    Ok(std::path::PathBuf::from(dir).join("omawhite.sock"))
}

fn open_default_store() -> anyhow::Result<Store> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").context("HOME not set")?;
    Store::open(store::data_root(xdg.as_deref(), &home))
}
