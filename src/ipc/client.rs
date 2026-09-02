//! Client side of the socket: single instance by forwarding (§5).
//!
//! A second `omawhite` opens no second window: if the socket answers, the
//! intent is forwarded and the process exits.

use std::io::{BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;

use crate::ipc::proto::{Request, read_frame, request_line};

const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Tries to forward `req` to a live instance.
///
/// - `Ok(None)`: nobody listening (socket missing or stale) — the caller
///   should become the main instance.
/// - `Ok(Some(line))`: an instance existed; `line` is the raw reply event.
pub fn try_forward(socket_path: &Path, req: &Request) -> anyhow::Result<Option<String>> {
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(s) => s,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            return Ok(None);
        }
        Err(e) => return Err(e).with_context(|| format!("connecting to {socket_path:?}")),
    };
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    stream.write_all(request_line(req).as_bytes())?;

    let mut reader = BufReader::new(stream);
    let reply = read_frame(&mut reader)?
        .context("existing instance closed the connection without replying")?;
    Ok(Some(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_returns_none_when_nothing_listens() {
        let tmp = tempfile::tempdir().unwrap();
        // Socket never created.
        let missing = tmp.path().join("omawhite.sock");
        assert_eq!(try_forward(&missing, &Request::Ping).unwrap(), None);

        // Stale socket: listener created and dropped (file stays behind).
        let stale = tmp.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(stale.exists(), "premise: bind leaves a file on disk");
        assert_eq!(try_forward(&stale, &Request::Ping).unwrap(), None);
    }
}
