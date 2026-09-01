//! Lado cliente do socket: single-instance por encaminhamento (§5).
//!
//! Um segundo `omawhite` não abre segunda janela: se o socket responde, a
//! intenção é encaminhada e o processo sai.

use std::io::{BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;

use crate::ipc::proto::{Request, read_frame, request_line};

const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Tenta encaminhar `req` para uma instância viva.
///
/// - `Ok(None)`: ninguém escutando (socket ausente ou morto) — o chamador
///   deve assumir o papel de instância principal.
/// - `Ok(Some(linha))`: havia instância; `linha` é o evento de resposta cru.
pub fn try_forward(socket_path: &Path, req: &Request) -> anyhow::Result<Option<String>> {
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None);
        }
        Err(e) => return Err(e).with_context(|| format!("conectando em {socket_path:?}")),
    };
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    stream.write_all(request_line(req).as_bytes())?;

    let mut reader = BufReader::new(stream);
    let reply =
        read_frame(&mut reader)?.context("instância existente fechou a conexão sem responder")?;
    Ok(Some(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_returns_none_when_nothing_listens() {
        let tmp = tempfile::tempdir().unwrap();
        // Socket nunca criado.
        let missing = tmp.path().join("omawhite.sock");
        assert_eq!(try_forward(&missing, &Request::Ping).unwrap(), None);

        // Socket morto: listener criado e derrubado (arquivo fica para trás).
        let stale = tmp.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(stale.exists(), "premissa: bind deixa arquivo no disco");
        assert_eq!(try_forward(&stale, &Request::Ping).unwrap(), None);
    }
}
