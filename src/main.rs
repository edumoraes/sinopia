//! sinopia — local-first whiteboard for Omarchy (see ARCHITECTURE.md).
//!
//! Single instance: if the socket answers, the intent is forwarded and this
//! process exits; otherwise this process becomes the main instance.

mod agents;
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
mod export;
mod field;
mod fonts;
mod geom;
mod gestures;
mod gfx;
mod glyphs;
mod graft;
mod grid;
mod history;
mod ipc;
mod layers;
mod menu;
mod menubar;
mod merge;
mod omarchy;
mod palette;
mod project;
mod props;
mod scene;
mod select;
mod send;
mod shape;
mod shapebar;
mod skills;
mod slots;
mod spans;
mod store;
mod tablet;
mod tabs;
mod text;
mod textbar;
mod theme;
mod thumbs;
mod tree;
mod typeset;

use anyhow::Context as _;

use cli::{Action, Cli};
use ipc::client::try_forward;
use ipc::proto::{Event, ExportFormat, Request, parse_event};
use project::{Origin, Project};
use store::Store;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::checked();

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
        // No colours: the desktop's own theme, read again by the
        // instance that is drawing with it.
        Action::Theme => Request::Theme { colors: None },
        Action::Frames => Request::Frames,
        Action::Read { frame, to } => Request::ReadFrame {
            id: frame_id(&socket_path, frame)?,
            dir: destination(to.as_deref())?,
        },
        Action::Add(file) => Request::AddFrame {
            path: std::fs::canonicalize(file)
                .with_context(|| format!("reading the fragment {file:?}"))?,
        },
        Action::Layer(verb) => {
            // One listing, fetched the first time a name needs it and
            // read for every name after that.
            let listing = std::cell::OnceCell::new();
            cli::layer_request(verb, |asked| {
                let listing = match listing.get() {
                    Some(l) => l,
                    None => {
                        let fetched = layer_listing(&socket_path)?;
                        listing.get_or_init(|| fetched)
                    }
                };
                cli::resolve_layer(listing, asked)
            })?
        }
        Action::Text(verb) => cli::text_request(
            verb,
            |asked| frame_id(&socket_path, asked),
            |asked| cli::resolve_text(&text_listing(&socket_path)?, asked),
        )?,
    };

    // §5: a second sinopia becomes a command on the socket, not a second window.
    if let Some(reply) = try_forward(&socket_path, &request)? {
        // Read through the module that owns the schema. Poking at a
        // `Value` was a second, looser parser — and it failed open: a
        // line that will not parse printed and exited 0, which a script
        // reading `$?` takes for work that landed.
        let ev = parse_event(&reply).with_context(|| format!("the board answered {reply:?}"))?;
        println!("{reply}");
        if matches!(ev, Event::Denied { .. }) {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Nobody listening: actions that need a live instance fail explicitly.
    match action {
        Action::Export(_) => {
            anyhow::bail!("no sinopia instance running; open the board before exporting")
        }
        // The board that is *open* is the one being read and written,
        // with the work nobody has saved yet inside it. There is nothing
        // to answer without one, and opening a window to answer would
        // answer about a different board.
        Action::Frames | Action::Read { .. } | Action::Add(_) | Action::Layer(_) | Action::Text(_) => {
            anyhow::bail!("no sinopia instance running; open the board first")
        }
        Action::Shutdown => {
            log::info!("no instance running; nothing to shut down");
            Ok(())
        }
        // A board that is not open has nothing to re-dress, and the one
        // opened next reads the theme for itself.
        Action::Theme => {
            log::info!("no instance running; nothing to re-theme");
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

/// The directory a page lands in, as the *caller* meant it: the one
/// named, or the one the command was run in. Resolved here because the
/// running instance's working directory is not the caller's, and
/// canonical because `..` says nothing about where a write lands to
/// whoever reads the line. It is still only a candidate — the binary
/// measures it against the allowlist (§8.2) before writing.
fn destination(to: Option<&std::path::Path>) -> anyhow::Result<std::path::PathBuf> {
    let dir = match to {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().context("reading the current directory")?,
    };
    std::fs::canonicalize(&dir).with_context(|| format!("resolving the destination {dir:?}"))
}

/// The id of the frame the caller named. An id is taken as one; anything
/// else is looked for in the listing by name, and a name two frames
/// answer to is refused naming both.
///
/// The lookup is here and not in the protocol on purpose: `op:
/// read_frame` takes an id and only an id, for the reason `open` and
/// `open_file` are two ops — guessing what a string is, is the best
/// effort §5 forbids. Guessing outside the schema costs one round trip
/// and binds nobody.
fn frame_id(socket: &std::path::Path, asked: &str) -> anyhow::Result<String> {
    let reply = try_forward(socket, &Request::Frames)?
        .context("no sinopia instance running; open the board first")?;
    let frames = match parse_event(&reply)? {
        Event::Frames { frames } => frames,
        Event::Denied { reason, .. } => anyhow::bail!("the board refused the listing: {reason}"),
        other => anyhow::bail!("the board answered {other:?} to a listing"),
    };
    if frames.iter().any(|f| f.id == asked) {
        return Ok(asked.to_owned());
    }
    let by_name: Vec<&export::Card> = frames.iter().filter(|f| f.name == asked).collect();
    match by_name.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => anyhow::bail!(
            "no frame {asked:?} on the board that is open; `sinopia agent frames` lists them"
        ),
        many => anyhow::bail!(
            "{} frames go by {asked:?} — ask for one by id: {}",
            many.len(),
            many.iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The open board's layers, for turning the names a person typed into
/// the ids the protocol speaks: one round trip, outside the schema, like
/// a frame's name.
/// The open board's texts, as `text list` prints them: what a name given
/// to a text verb is resolved against.
fn text_listing(socket: &std::path::Path) -> anyhow::Result<Vec<export::TextCard>> {
    let reply = try_forward(socket, &Request::Texts)?
        .context("no sinopia instance running; open the board first")?;
    match parse_event(&reply)? {
        Event::Texts { texts } => Ok(texts),
        Event::Denied { reason, .. } => anyhow::bail!("the board refused the listing: {reason}"),
        other => anyhow::bail!("the board answered {other:?} to a listing"),
    }
}

fn layer_listing(socket: &std::path::Path) -> anyhow::Result<Vec<editor::Listed>> {
    let reply = try_forward(socket, &Request::Layers)?
        .context("no sinopia instance running; open the board first")?;
    match parse_event(&reply)? {
        Event::Layers { layers } => Ok(layers),
        Event::Denied { reason, .. } => anyhow::bail!("the board refused the listing: {reason}"),
        other => anyhow::bail!("the board answered {other:?} to a listing"),
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
    Ok(std::path::PathBuf::from(dir).join("sinopia.sock"))
}

/// The socket an instance listened on while the board went by its working
/// name, Omawhite.
const LEGACY_SOCKET: &str = "omawhite.sock";

/// Opens the store, bringing the boards over from where the working name
/// kept them the first time round — unless an instance under that name is
/// still running. Its unsaved drafts are written there, so moving the
/// directory would lose them, and opening an empty store beside it would
/// leave every board behind for good.
fn open_default_store() -> anyhow::Result<Store> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").context("HOME not set")?;
    let root = store::data_root(xdg.as_deref(), &home);
    if let Some(old) = store::left_behind(&root) {
        anyhow::ensure!(
            !legacy_instance_running(),
            "the boards are still in {old:?}, where a running omawhite keeps them; \
             close it, then start sinopia again to bring them over"
        );
        std::fs::rename(&old, &root)
            .with_context(|| format!("bringing the boards over from {old:?} to {root:?}"))?;
        log::info!("brought the boards over from {old:?} to {root:?}");
    }
    Store::open(root)
}

/// Whether an instance under the working name answers on its socket. A
/// socket file nobody listens on is what a crash leaves, and does not
/// count.
fn legacy_instance_running() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join(LEGACY_SOCKET))
        .is_some_and(|path| std::os::unix::net::UnixStream::connect(path).is_ok())
}
