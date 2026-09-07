//! The binary's CLI (§5): mirrors the socket protocol, for the plugin and
//! for humans.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "omawhite",
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

    /// Socket path (default: $XDG_RUNTIME_DIR/omawhite.sock)
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
}

impl Cli {
    /// Parsed, then checked for the one conflict clap's own `ArgGroup`
    /// cannot see: a group reaches args and never a subcommand. Without
    /// it `omawhite --shutdown agent frames` parsed clean and quietly
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
        match (flag, &self.command) {
            (Some(flag), Some(Command::Agent { .. })) => Err(clap::Error::raw(
                clap::error::ErrorKind::ArgumentConflict,
                format!("the argument '{flag}' cannot be used with 'agent'\n"),
            )),
            _ => Ok(self),
        }
    }

    pub fn action(&self) -> Action {
        // A line carrying both is refused by `verified`, so the verb is
        // the whole of the action whenever there is one.
        if let Some(Command::Agent { verb }) = &self.command {
            return match verb {
                Verb::Frames => Action::Frames,
                Verb::Read { frame, to } => Action::Read {
                    frame: frame.clone(),
                    to: to.clone(),
                },
                Verb::Add { file } => Action::Add(file.clone()),
            };
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
        Cli::try_parse_from(std::iter::once("omawhite").chain(args.iter().copied()))
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
            parse(&["--open-file", "/home/you/plan.omawhite"])
                .unwrap()
                .action(),
            Action::OpenFile(PathBuf::from("/home/you/plan.omawhite"))
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
}
