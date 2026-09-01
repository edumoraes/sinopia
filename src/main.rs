//! omawhite — whiteboard local-first para Omarchy (ver ARCHITECTURE.md).
//!
//! Single-instance: se o socket responde, a intenção é encaminhada e este
//! processo sai; senão, este processo vira a instância principal.

mod app;
mod cli;
mod doc;
mod gfx;
mod ipc;
mod scene;
mod store;

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

    // §5: segundo omawhite vira comando no socket, não segunda janela.
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

    // Ninguém escutando: ações que exigem instância viva falham explícito.
    match action {
        Action::Export(_) => {
            anyhow::bail!("nenhuma instância do omawhite rodando; abra o board antes de exportar")
        }
        Action::Shutdown => {
            log::info!("nenhuma instância rodando; nada a encerrar");
            Ok(())
        }
        Action::Default | Action::New | Action::Open(_) => {
            let store = open_default_store()?;
            let doc = match &action {
                Action::New => {
                    let doc = Document::new("sem título");
                    store.save(&doc)?;
                    doc
                }
                Action::Open(id) => store.load(id)?,
                // Sem flags: board mais recente, ou um novo se não há nenhum.
                _ => match store.index()?.first() {
                    Some(entry) => store.load(&entry.id)?,
                    None => {
                        let doc = Document::new("sem título");
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
        .context("XDG_RUNTIME_DIR não definido (necessário para o socket, §5)")?;
    Ok(std::path::PathBuf::from(dir).join("omawhite.sock"))
}

fn open_default_store() -> anyhow::Result<Store> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").context("HOME não definido")?;
    Store::open(store::data_root(xdg.as_deref(), &home))
}
