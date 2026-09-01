//! CLI do binário (§5): espelha o protocolo do socket, para o plugin e
//! para humanos.

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "omawhite",
    version,
    about = "Whiteboard local-first com export para o agente"
)]
#[command(group = clap::ArgGroup::new("action").multiple(false))]
pub struct Cli {
    /// Cria um board novo
    #[arg(long, group = "action")]
    pub new: bool,

    /// Abre um board existente pelo id
    #[arg(long, value_name = "ID", group = "action")]
    pub open: Option<String>,

    /// Exporta o board atual para DIR (exige instância viva)
    #[arg(long, value_name = "DIR", group = "action")]
    pub export: Option<PathBuf>,

    /// Encerra a instância viva
    #[arg(long, group = "action")]
    pub shutdown: bool,

    /// Caminho do socket (default: $XDG_RUNTIME_DIR/omawhite.sock)
    #[arg(long, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    /// Renderiza N frames e sai (verificação de fumaça, uso em CI)
    #[arg(long, value_name = "N", hide = true)]
    pub smoke_frames: Option<u32>,
}

/// Intenção de alto nível depois do parse.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Sem flags: levanta a instância viva ou abre o board mais recente.
    Default,
    New,
    Open(String),
    Export(PathBuf),
    Shutdown,
}

impl Cli {
    pub fn action(&self) -> Action {
        if self.new {
            Action::New
        } else if let Some(id) = &self.open {
            Action::Open(id.clone())
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
    }

    #[test]
    fn actions_are_mutually_exclusive() {
        for args in [
            &["--new", "--open", "x"][..],
            &["--new", "--shutdown"][..],
            &["--open", "x", "--export", "/tmp"][..],
            &["--export", "/tmp", "--shutdown"][..],
        ] {
            assert!(parse(args).is_err(), "combinação {args:?} deveria falhar");
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
