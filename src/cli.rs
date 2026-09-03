//! The binary's CLI (§5): mirrors the socket protocol, for the plugin and
//! for humans.

use std::path::PathBuf;

use clap::Parser;

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

    /// Socket path (default: $XDG_RUNTIME_DIR/omawhite.sock)
    #[arg(long, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    /// Render N frames and exit (smoke check, for CI)
    #[arg(long, value_name = "N", hide = true)]
    pub smoke_frames: Option<u32>,
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
}

impl Cli {
    pub fn action(&self) -> Action {
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
            &["--open", "x", "--open-file", "/tmp/a"][..],
            &["--new", "--open-file", "/tmp/a"][..],
        ] {
            assert!(parse(args).is_err(), "combination {args:?} should fail");
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
