//! The binary's CLI (§5): mirrors the socket protocol, for the plugin and
//! for humans.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::doc::{BlendMode, Tag};
use crate::editor::Listed;
use crate::ipc::proto::Request;
use crate::tree::{Arrange, Place};

#[derive(Debug, Parser)]
#[command(
    name = "sinopia",
    version,
    about = "Local-first whiteboard with export for the agent"
)]
#[command(group = clap::ArgGroup::new("action").multiple(false))]
pub struct Cli {
    /// Create a new board
    #[arg(long, group = "action")]
    pub new: bool,

    /// Open an existing board by id
    #[arg(long, value_name = "ID", group = "action")]
    pub open: Option<String>,

    /// Open a project file by path
    #[arg(long = "open-file", value_name = "PATH", group = "action")]
    pub open_file: Option<PathBuf>,

    /// Export the current board to DIR (requires a live instance)
    #[arg(long, value_name = "DIR", group = "action")]
    pub export: Option<PathBuf>,

    /// Shut down the live instance
    #[arg(long, group = "action")]
    pub shutdown: bool,

    /// Read the desktop's theme again (requires a live instance)
    #[arg(long, group = "action")]
    pub theme: bool,

    /// Socket path (default: $XDG_RUNTIME_DIR/sinopia.sock)
    #[arg(long, value_name = "PATH", global = true)]
    pub socket: Option<PathBuf>,

    /// Render N frames and exit (smoke check, for CI)
    #[arg(long, value_name = "N", hide = true)]
    pub smoke_frames: Option<u32>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The verbs that are not window intent. A subcommand rather than three
/// more flags: everything above says *open this*, *raise that*, *shut
/// down*, and what an agent asks is a different kind of sentence.
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Command {
    /// Read and write frames the way a code agent does (needs a live board)
    Agent {
        #[command(subcommand)]
        verb: Verb,
    },
    /// Drive the layers of the board that is open (needs a live board)
    Layer {
        #[command(subcommand)]
        verb: LayerVerb,
    },
}

/// What can be done to the layers, each named by its id or by a name
/// only it goes by. Every change is one step the board can undo.
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum LayerVerb {
    /// List the layers, the whole tree top first, as JSON
    List,
    /// Add a raster layer — or a group — above the active layer
    Add {
        /// Make a group
        #[arg(long)]
        group: bool,
        /// What it is called
        #[arg(long)]
        name: Option<String>,
        /// Put it right above this layer instead
        #[arg(long, value_name = "LAYER")]
        above: Option<String>,
    },
    /// Take layers off the board, with everything on them
    Remove {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Give a layer a new name
    Rename { layer: String, name: String },
    /// Move layers into a group or a frame, next to a layer, or along their stack
    #[command(group = clap::ArgGroup::new("to").required(true).multiple(false))]
    Move {
        #[arg(required = true)]
        layers: Vec<String>,
        /// Into this group or frame, at the top of it
        #[arg(long, value_name = "LAYER", group = "to")]
        into: Option<String>,
        /// Right above this layer
        #[arg(long, value_name = "LAYER", group = "to")]
        above: Option<String>,
        /// Right below this layer
        #[arg(long, value_name = "LAYER", group = "to")]
        below: Option<String>,
        /// To the top of their stack
        #[arg(long, group = "to")]
        front: bool,
        /// One step up their stack
        #[arg(long, group = "to")]
        forward: bool,
        /// One step down their stack
        #[arg(long, group = "to")]
        backward: bool,
        /// To the bottom of their stack
        #[arg(long, group = "to")]
        back: bool,
    },
    /// Show layers
    Show {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Hide layers
    Hide {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Lock what layers hold
    Lock {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Open the lock on layers
    Unlock {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Set how strongly layers draw, in percent (0–100)
    Opacity {
        #[arg(value_parser = percent, value_name = "PERCENT")]
        opacity: f64,
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Set how layers blend with what is under them (normal, multiply, screen, …)
    Blend {
        #[arg(value_parser = blend_mode, value_name = "MODE")]
        mode: BlendMode,
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Tag layers with a colour: none, red, orange, yellow, green, blue, violet, gray
    Color {
        #[arg(value_parser = tag, value_name = "COLOR")]
        color: Tag,
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Put layers in a new group, where the topmost of them was
    Group {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Take groups apart, leaving what they held where they stood
    Ungroup {
        #[arg(required = true)]
        groups: Vec<String>,
    },
    /// Copy layers, each right above itself
    Duplicate {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Merge sibling layers into the topmost — or a group into one layer
    Merge {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Merge a layer into the one under it
    MergeDown { layer: String },
    /// Merge every visible sibling of every stack, frames standing between
    MergeVisible,
    /// Merge what is visible and drop what is hidden
    Flatten,
    /// Pick layers, as a click in the panel does
    Select {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Show what groups or frames hold, in the panel
    Expand {
        #[arg(required = true)]
        layers: Vec<String>,
    },
    /// Fold groups or frames away, in the panel
    Collapse {
        #[arg(required = true)]
        layers: Vec<String>,
    },
}

/// A strength typed as a percent — `50` or `50%` — as the fraction the
/// board keeps.
fn percent(s: &str) -> Result<f64, String> {
    let n: f64 = s
        .trim_end_matches('%')
        .parse()
        .map_err(|_| format!("{s:?} is not a percent"))?;
    if (0.0..=100.0).contains(&n) {
        Ok(n / 100.0)
    } else {
        Err(format!("{s:?} is not between 0 and 100"))
    }
}

/// A name as a person types it: any case, dashes or none.
fn plain(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The name a board writes `value` under.
fn written<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `colorDodge` as a person would type it: `color-dodge`.
fn dashed(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_uppercase() {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn blend_mode(s: &str) -> Result<BlendMode, String> {
    BlendMode::ALL
        .into_iter()
        .find(|m| plain(&written(m)) == plain(s))
        .ok_or_else(|| {
            let all: Vec<String> = BlendMode::ALL.iter().map(|m| dashed(&written(m))).collect();
            format!("{s:?} is not a blend mode; one of: {}", all.join(", "))
        })
}

fn tag(s: &str) -> Result<Tag, String> {
    std::iter::once(Tag::None)
        .chain(Tag::COLORS)
        .find(|t| plain(&written(t)) == plain(s))
        .ok_or_else(|| format!("{s:?} is not a colour; one of: none, red, orange, yellow, green, blue, violet, gray"))
}

/// The id `asked` names in a listing: an id first, then a name only one
/// layer goes by. Two layers going by one name is not a guess to make.
pub fn resolve_layer(listing: &[Listed], asked: &str) -> anyhow::Result<String> {
    if listing.iter().any(|l| l.id == asked) {
        return Ok(asked.to_owned());
    }
    let by_name: Vec<&Listed> = listing.iter().filter(|l| l.name == asked).collect();
    match by_name.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => anyhow::bail!(
            "no layer {asked:?} on the board that is open; `sinopia layer list` lists them"
        ),
        many => anyhow::bail!(
            "{} layers go by {asked:?} — name one by its id: {}",
            many.len(),
            many.iter().map(|l| l.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The request a layer verb asks, every layer it names turned into an id
/// by `id` — which is where a name that is not there stops it.
pub fn layer_request(verb: &LayerVerb, id: impl Fn(&str) -> anyhow::Result<String>) -> anyhow::Result<Request> {
    let ids = |names: &[String]| names.iter().map(|n| id(n)).collect::<anyhow::Result<Vec<String>>>();
    Ok(match verb {
        LayerVerb::List => Request::Layers,
        LayerVerb::Add { group, name, above } => Request::AddLayer {
            group: *group,
            name: name.clone(),
            above: above.as_deref().map(&id).transpose()?,
        },
        LayerVerb::Remove { layers } => Request::RemoveLayers { ids: ids(layers)? },
        LayerVerb::Rename { layer, name } => Request::RenameLayer {
            id: id(layer)?,
            name: name.clone(),
        },
        LayerVerb::Move {
            layers,
            into,
            above,
            below,
            front,
            forward,
            backward,
            back,
        } => {
            let ids = ids(layers)?;
            let place = |target: &String, to: fn(String) -> Place| anyhow::Ok(to(id(target)?));
            match (into, above, below) {
                (Some(t), _, _) => Request::MoveLayers { ids, to: place(t, Place::Into)? },
                (_, Some(t), _) => Request::MoveLayers { ids, to: place(t, Place::Above)? },
                (_, _, Some(t)) => Request::MoveLayers { ids, to: place(t, Place::Below)? },
                _ => Request::ArrangeLayers {
                    ids,
                    how: if *front {
                        Arrange::Front
                    } else if *forward {
                        Arrange::Forward
                    } else if *backward {
                        Arrange::Backward
                    } else {
                        debug_assert!(*back, "clap asks for one destination");
                        Arrange::Back
                    },
                },
            }
        }
        LayerVerb::Show { layers } => Request::ShowLayers { ids: ids(layers)?, visible: true },
        LayerVerb::Hide { layers } => Request::ShowLayers { ids: ids(layers)?, visible: false },
        LayerVerb::Lock { layers } => Request::LockLayers { ids: ids(layers)?, locked: true },
        LayerVerb::Unlock { layers } => Request::LockLayers { ids: ids(layers)?, locked: false },
        LayerVerb::Opacity { opacity, layers } => Request::SetOpacity {
            ids: ids(layers)?,
            opacity: *opacity,
        },
        LayerVerb::Blend { mode, layers } => Request::SetBlend {
            ids: ids(layers)?,
            blend: *mode,
        },
        LayerVerb::Color { color, layers } => Request::SetColor {
            ids: ids(layers)?,
            color: *color,
        },
        LayerVerb::Group { layers } => Request::GroupLayers { ids: ids(layers)? },
        LayerVerb::Ungroup { groups } => Request::Ungroup { ids: ids(groups)? },
        LayerVerb::Duplicate { layers } => Request::DuplicateLayers { ids: ids(layers)? },
        LayerVerb::Merge { layers } => Request::MergeLayers { ids: ids(layers)? },
        LayerVerb::MergeDown { layer } => Request::MergeDown { id: id(layer)? },
        LayerVerb::MergeVisible => Request::MergeVisible,
        LayerVerb::Flatten => Request::Flatten,
        LayerVerb::Select { layers } => Request::SelectLayers { ids: ids(layers)? },
        LayerVerb::Expand { layers } => Request::OpenLayers { ids: ids(layers)?, open: true },
        LayerVerb::Collapse { layers } => Request::OpenLayers { ids: ids(layers)?, open: false },
    })
}

#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Verb {
    /// List the frames of the board that is open
    Frames,
    /// Export one frame — picture, objects and inventory — into a directory
    Read {
        /// The frame, by its id or by the name on its card
        frame: String,
        /// Where the page lands (default: the current directory)
        #[arg(long = "to", value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// Put a frame on the board, from a fragment written as JSON
    Add {
        /// The fragment: one frame and what stands in it
        file: PathBuf,
    },
}

/// High-level intent after parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// No flags: raise the live instance or open the most recent board.
    Default,
    New,
    Open(String),
    /// A project file the user named, by path. A recents entry that is
    /// not a draft is opened this way.
    OpenFile(PathBuf),
    Export(PathBuf),
    Shutdown,
    /// What frames the open board has (§8).
    Frames,
    /// One frame, named by id or by the name on its card, exported into
    /// a directory. `to` is the caller's, not the instance's: `main`
    /// settles it against the directory the command was run in.
    Read {
        frame: String,
        to: Option<PathBuf>,
    },
    /// A frame handed over, from a fragment file.
    Add(PathBuf),
    /// The desktop's theme has changed. What a `theme-set` hook calls,
    /// and — like `Export` and `Shutdown` — never a reason to open a
    /// window: there is nothing to re-dress until there is one.
    Theme,
    /// Something done to the open board's layers, named as they were
    /// typed: `main` turns the names into ids against the live board.
    Layer(LayerVerb),
}

impl Cli {
    /// Parsed, then checked for the one conflict clap's own `ArgGroup`
    /// cannot see: a group reaches args and never a subcommand. Without
    /// it `sinopia --shutdown agent frames` parsed clean and quietly
    /// did the verb, dropping the flag — so a wrapper that appends
    /// `--new` to whatever it is handed would silently do something
    /// else, while `--new --shutdown` has always been a hard error.
    pub fn checked() -> Cli {
        match Cli::parse().verified() {
            Ok(cli) => cli,
            Err(e) => e.exit(),
        }
    }

    fn verified(self) -> Result<Cli, clap::Error> {
        let flag = [
            self.new.then_some("--new"),
            self.open.is_some().then_some("--open"),
            self.open_file.is_some().then_some("--open-file"),
            self.export.is_some().then_some("--export"),
            self.shutdown.then_some("--shutdown"),
            self.theme.then_some("--theme"),
        ]
        .into_iter()
        .flatten()
        .next();
        let verb = match &self.command {
            Some(Command::Agent { .. }) => "agent",
            Some(Command::Layer { .. }) => "layer",
            None => return Ok(self),
        };
        match flag {
            Some(flag) => Err(clap::Error::raw(
                clap::error::ErrorKind::ArgumentConflict,
                format!("the argument '{flag}' cannot be used with '{verb}'\n"),
            )),
            None => Ok(self),
        }
    }

    pub fn action(&self) -> Action {
        // A line carrying both is refused by `verified`, so the verb is
        // the whole of the action whenever there is one.
        match &self.command {
            Some(Command::Agent { verb }) => {
                return match verb {
                    Verb::Frames => Action::Frames,
                    Verb::Read { frame, to } => Action::Read {
                        frame: frame.clone(),
                        to: to.clone(),
                    },
                    Verb::Add { file } => Action::Add(file.clone()),
                };
            }
            Some(Command::Layer { verb }) => return Action::Layer(verb.clone()),
            None => {}
        }
        if self.new {
            Action::New
        } else if let Some(id) = &self.open {
            Action::Open(id.clone())
        } else if let Some(path) = &self.open_file {
            Action::OpenFile(path.clone())
        } else if let Some(dir) = &self.export {
            Action::Export(dir.clone())
        } else if self.shutdown {
            Action::Shutdown
        } else if self.theme {
            Action::Theme
        } else {
            Action::Default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("sinopia").chain(args.iter().copied()))
            .and_then(Cli::verified)
    }

    #[test]
    fn no_flags_is_the_default_action() {
        assert_eq!(parse(&[]).unwrap().action(), Action::Default);
    }

    #[test]
    fn each_flag_maps_to_its_action() {
        assert_eq!(parse(&["--new"]).unwrap().action(), Action::New);
        assert_eq!(
            parse(&["--open", "01J"]).unwrap().action(),
            Action::Open("01J".into())
        );
        assert_eq!(
            parse(&["--export", "/tmp/proj"]).unwrap().action(),
            Action::Export(PathBuf::from("/tmp/proj"))
        );
        assert_eq!(parse(&["--shutdown"]).unwrap().action(), Action::Shutdown);
        assert_eq!(parse(&["--theme"]).unwrap().action(), Action::Theme);
        assert_eq!(
            parse(&["--open-file", "/home/you/plan.sinopia"])
                .unwrap()
                .action(),
            Action::OpenFile(PathBuf::from("/home/you/plan.sinopia"))
        );
    }

    #[test]
    fn actions_are_mutually_exclusive() {
        for args in [
            &["--new", "--open", "x"][..],
            &["--new", "--shutdown"][..],
            &["--open", "x", "--export", "/tmp"][..],
            &["--export", "/tmp", "--shutdown"][..],
            &["--theme", "--new"][..],
            &["--theme", "--shutdown"][..],
            &["--open", "x", "--open-file", "/tmp/a"][..],
            &["--new", "--open-file", "/tmp/a"][..],
            // A verb is an action too, and clap's `ArgGroup` cannot see
            // one — so these used to parse clean and drop the flag.
            &["--shutdown", "agent", "frames"][..],
            &["--new", "agent", "frames"][..],
            &["--export", "/tmp", "agent", "frames"][..],
            &["--theme", "agent", "add", "/tmp/f.json"][..],
            &["--open", "x", "layer", "list"][..],
        ] {
            assert!(parse(args).is_err(), "combination {args:?} should fail");
        }
    }

    #[test]
    fn a_verb_still_takes_the_arguments_that_are_not_actions() {
        // `--socket` says *which* board, not what to do with it, so it
        // belongs beside a verb and must not be caught by the check.
        let cli = parse(&["--socket", "/tmp/s.sock", "agent", "frames"]).unwrap();
        assert_eq!(cli.action(), Action::Frames);
        assert_eq!(cli.socket.as_deref(), Some(std::path::Path::new("/tmp/s.sock")));
    }

    #[test]
    fn the_agents_verbs_map_to_their_actions() {
        assert_eq!(parse(&["agent", "frames"]).unwrap().action(), Action::Frames);
        assert_eq!(
            parse(&["agent", "read", "Auth Flow"]).unwrap().action(),
            Action::Read {
                frame: "Auth Flow".into(),
                to: None,
            }
        );
        assert_eq!(
            parse(&["agent", "read", "01J", "--to", "/tmp/proj"])
                .unwrap()
                .action(),
            Action::Read {
                frame: "01J".into(),
                to: Some(PathBuf::from("/tmp/proj")),
            }
        );
        assert_eq!(
            parse(&["agent", "add", "frame.json"]).unwrap().action(),
            Action::Add(PathBuf::from("frame.json"))
        );
    }

    #[test]
    fn a_verb_needs_what_it_acts_on() {
        for args in [
            &["agent"][..],
            &["agent", "read"][..],
            &["agent", "add"][..],
            &["agent", "sketch"][..],
            &["agent", "read", "01J", "--to"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn the_socket_reaches_the_verbs_from_either_side() {
        // An agent scripts these, and a script writes the flag where it
        // reads best.
        for args in [
            &["--socket", "/run/x.sock", "agent", "frames"][..],
            &["agent", "frames", "--socket", "/run/x.sock"][..],
        ] {
            let cli = parse(args).unwrap();
            assert_eq!(cli.socket, Some(PathBuf::from("/run/x.sock")), "{args:?}");
            assert_eq!(cli.action(), Action::Frames);
        }
    }

    #[test]
    fn socket_and_smoke_are_orthogonal_options() {
        let cli = parse(&["--socket", "/run/x.sock", "--new", "--smoke-frames", "3"]).unwrap();
        assert_eq!(cli.socket, Some(PathBuf::from("/run/x.sock")));
        assert_eq!(cli.smoke_frames, Some(3));
        assert_eq!(cli.action(), Action::New);
    }

    fn layer(args: &[&str]) -> LayerVerb {
        match parse(&[&["layer"][..], args].concat()).unwrap().action() {
            Action::Layer(verb) => verb,
            other => panic!("{args:?} is a layer verb, not {other:?}"),
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_layer_verbs_parse() {
        assert_eq!(layer(&["list"]), LayerVerb::List);
        assert_eq!(
            layer(&["add", "--group", "--name", "Sky", "--above", "Ground"]),
            LayerVerb::Add {
                group: true,
                name: Some("Sky".into()),
                above: Some("Ground".into()),
            }
        );
        assert_eq!(
            layer(&["opacity", "50%", "Sky", "Ground"]),
            LayerVerb::Opacity {
                opacity: 0.5,
                layers: names(&["Sky", "Ground"]),
            }
        );
        assert_eq!(
            layer(&["blend", "color-dodge", "Sky"]),
            LayerVerb::Blend {
                mode: BlendMode::ColorDodge,
                layers: names(&["Sky"]),
            }
        );
        assert_eq!(
            layer(&["blend", "PassThrough", "G"]),
            LayerVerb::Blend {
                mode: BlendMode::PassThrough,
                layers: names(&["G"]),
            },
            "whatever the case and the dashes"
        );
        assert_eq!(
            layer(&["color", "violet", "Sky"]),
            LayerVerb::Color {
                color: Tag::Violet,
                layers: names(&["Sky"]),
            }
        );
        assert_eq!(layer(&["merge-down", "Sky"]), LayerVerb::MergeDown { layer: "Sky".into() });
        assert_eq!(layer(&["merge-visible"]), LayerVerb::MergeVisible);
        assert_eq!(layer(&["flatten"]), LayerVerb::Flatten);
        let LayerVerb::Move { layers, into, front, .. } = layer(&["move", "A", "B", "--into", "G"]) else {
            panic!("a move");
        };
        assert_eq!((layers, into, front), (names(&["A", "B"]), Some("G".into()), false));
    }

    #[test]
    fn a_layer_verb_needs_what_it_acts_on() {
        for args in [
            &["layer"][..],
            &["layer", "remove"][..],
            &["layer", "rename", "Sky"][..],
            &["layer", "move", "Sky"][..],
            &["layer", "move", "Sky", "--into", "G", "--front"][..],
            &["layer", "opacity", "150", "Sky"][..],
            &["layer", "opacity", "half", "Sky"][..],
            &["layer", "opacity", "50"][..],
            &["layer", "blend", "sparkle", "Sky"][..],
            &["layer", "color", "pink", "Sky"][..],
            &["layer", "merge-down"][..],
            &["--shutdown", "layer", "list"][..],
            &["--new", "layer", "flatten"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?} should fail");
        }
    }

    fn listed(id: &str, name: &str) -> Listed {
        Listed {
            id: id.into(),
            name: name.into(),
            kind: crate::doc::Kind::Raster,
            owner: None,
            depth: 0,
            visible: true,
            shown: true,
            locked: false,
            opacity: 1.0,
            blend: BlendMode::Normal,
            color: Tag::None,
            active: false,
            picked: false,
            elements: 0,
        }
    }

    #[test]
    fn a_layer_is_named_by_its_id_or_by_a_name_only_it_goes_by() {
        let list = [listed("01A", "Sky"), listed("01B", "Ink"), listed("01C", "Ink"), listed("Ink", "Paper")];
        assert_eq!(resolve_layer(&list, "01A").unwrap(), "01A");
        assert_eq!(resolve_layer(&list, "Sky").unwrap(), "01A");
        assert_eq!(resolve_layer(&list, "Ink").unwrap(), "Ink", "an id wins over a name");
        let list = [listed("01A", "Sky"), listed("01B", "Ink"), listed("01C", "Ink")];
        let two = resolve_layer(&list, "Ink").unwrap_err().to_string();
        assert!(two.contains("01B") && two.contains("01C"), "{two}");
        assert!(resolve_layer(&list, "Moon").unwrap_err().to_string().contains("layer list"));
    }

    #[test]
    fn each_layer_verb_asks_its_op_by_ids() {
        let ids = |v: &LayerVerb| layer_request(v, |name| Ok(format!("id:{name}"))).unwrap();
        assert_eq!(ids(&LayerVerb::List), Request::Layers);
        assert_eq!(
            ids(&layer(&["hide", "Sky", "Ink"])),
            Request::ShowLayers {
                ids: names(&["id:Sky", "id:Ink"]),
                visible: false,
            }
        );
        assert_eq!(
            ids(&layer(&["unlock", "Sky"])),
            Request::LockLayers {
                ids: names(&["id:Sky"]),
                locked: false,
            }
        );
        assert_eq!(
            ids(&layer(&["move", "Sky", "--below", "Ink"])),
            Request::MoveLayers {
                ids: names(&["id:Sky"]),
                to: Place::Below("id:Ink".into()),
            }
        );
        assert_eq!(
            ids(&layer(&["move", "Sky", "--backward"])),
            Request::ArrangeLayers {
                ids: names(&["id:Sky"]),
                how: Arrange::Backward,
            }
        );
        assert_eq!(
            ids(&layer(&["rename", "Sky", "Heaven"])),
            Request::RenameLayer {
                id: "id:Sky".into(),
                name: "Heaven".into(),
            }
        );
        assert_eq!(
            ids(&layer(&["add", "--above", "Sky"])),
            Request::AddLayer {
                group: false,
                name: None,
                above: Some("id:Sky".into()),
            }
        );
        assert_eq!(
            ids(&layer(&["collapse", "G"])),
            Request::OpenLayers {
                ids: names(&["id:G"]),
                open: false,
            }
        );
        assert_eq!(ids(&layer(&["merge-down", "Sky"])), Request::MergeDown { id: "id:Sky".into() });
        assert_eq!(ids(&layer(&["flatten"])), Request::Flatten);
        // A name that is not there stops the request before it is sent.
        let refused = layer_request(&layer(&["remove", "Moon"]), |n| anyhow::bail!("no layer {n:?}"));
        assert!(refused.is_err());
    }
}
