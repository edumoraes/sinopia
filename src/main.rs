//! omawhite — local-first whiteboard for Omarchy (see ARCHITECTURE.md).
//!
//! Single instance: if the socket answers, the intent is forwarded and this
//! process exits; otherwise this process becomes the main instance.

mod app;
mod bitmap;
mod cli;
mod curve;
mod doc;
mod dock;
mod editor;
mod geom;
mod gestures;
mod gfx;
mod grid;
mod ipc;
mod scene;
mod select;
mod store;
mod theme;

use anyhow::Context as _;
use clap::Parser as _;

use cli::{Action, Cli};
use doc::Document;
use ipc::client::try_forward;
use ipc::proto::{ExportFormat, Request};
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
        Action::Default | Action::New | Action::Open(_) => {
            let store = open_default_store()?;
            let doc = match &action {
                Action::New => {
                    let doc = Document::new("untitled");
                    store.save(&doc)?;
                    doc
                }
                Action::Open(id) => store.load(id)?,
                // No flags: the most recent board, or a new one if there is none.
                _ => match store.index()?.first() {
                    Some(entry) => store.load(&entry.id)?,
                    None => {
                        let doc = Document::new("untitled");
                        store.save(&doc)?;
                        doc
                    }
                },
            };
            app::run(store, doc, socket_path, cli.smoke_frames)
        }
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
