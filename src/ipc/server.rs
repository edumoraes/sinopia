//! Server side of the socket (§5, §9.2): bind at 0600, one JSON per line,
//! request → reply event. A malformed request gets `denied` and the
//! connection closes — a closed schema does no best effort.

use std::io::{BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;

use crate::ipc::proto::{Event, Request, event_line, parse_request, read_frame};

/// A stalled client must not hold the server hostage (§9.2): a connection
/// idle beyond this is closed — the plugin reconnects whenever it wants.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    /// Binds at 0600. A stale socket at the path (earlier bind without
    /// unlink) is replaced. A live instance yields an error.
    pub fn bind(path: &Path) -> anyhow::Result<Server> {
        match UnixListener::bind(path) {
            Ok(listener) => Server::finish_bind(listener, path),
            Err(e) if e.kind() == ErrorKind::AddrInUse => match UnixStream::connect(path) {
                Ok(_) => anyhow::bail!("another live instance is listening on {path:?}"),
                Err(ce) if ce.kind() == ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path)
                        .with_context(|| format!("removing stale socket {path:?}"))?;
                    let listener =
                        UnixListener::bind(path).with_context(|| format!("binding {path:?}"))?;
                    Server::finish_bind(listener, path)
                }
                Err(ce) => Err(ce).with_context(|| format!("probing socket {path:?}")),
            },
            Err(e) => Err(e).with_context(|| format!("binding {path:?}")),
        }
    }

    fn finish_bind(listener: UnixListener, path: &Path) -> anyhow::Result<Server> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {path:?}"))?;
        Ok(Server {
            listener,
            path: path.to_path_buf(),
        })
    }

    #[allow(dead_code)] // used in tests; production uses it after §15.3
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the server and serves connections on its own thread (one
    /// connection at a time — the plugin is the only expected client in the MVP).
    /// `on_request` runs on the server thread and returns the reply event.
    pub fn serve(
        self,
        on_request: impl Fn(Request) -> Event + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            for conn in self.listener.incoming() {
                let Ok(stream) = conn else { continue };
                // An error on one connection does not bring the server down (§9.2).
                let _ = handle_conn(stream, &on_request);
            }
        })
    }
}

fn handle_conn(stream: UnixStream, on_request: &impl Fn(Request) -> Event) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        match read_frame(&mut reader) {
            Ok(Some(line)) => match parse_request(&line) {
                Ok(req) => writer.write_all(event_line(&on_request(req)).as_bytes())?,
                Err(e) => {
                    let denied = Event::Denied {
                        op: "?".into(),
                        reason: e.to_string(),
                    };
                    writer.write_all(event_line(&denied).as_bytes())?;
                    break; // closed schema: a connection that speaks wrong is dropped
                }
            },
            Ok(None) => break,
            Err(e) => {
                let denied = Event::Denied {
                    op: "?".into(),
                    reason: e.to_string(),
                };
                let _ = writer.write_all(event_line(&denied).as_bytes());
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::client::try_forward;
    use crate::ipc::proto::Request;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn bind_creates_private_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("omawhite.sock");
        let server = Server::bind(&path).unwrap();
        assert_eq!(server.path(), path);
        let mode = std::fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket must be 0600");
    }

    #[test]
    fn bind_replaces_stale_socket_but_respects_live_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("omawhite.sock");

        // Stale: bind + drop leaves the file → the next bind takes over.
        drop(Server::bind(&path).unwrap());
        assert!(path.exists());
        let live = Server::bind(&path).unwrap();

        // Live: a second bind on the same path fails.
        assert!(Server::bind(&path).is_err());
        drop(live);
    }

    #[test]
    fn request_gets_handler_reply_via_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("omawhite.sock");
        let server = Server::bind(&path).unwrap();
        server.serve(|req| match req {
            Request::Ping => Event::Ready {
                id: "01JBOARD".into(),
                pid: 4321,
            },
            _ => Event::Denied {
                op: "?".into(),
                reason: "unexpected in test".into(),
            },
        });

        let reply = try_forward(&path, &Request::Ping)
            .unwrap()
            .expect("a live instance existed");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ev"], "ready");
        assert_eq!(v["id"], "01JBOARD");
        assert_eq!(v["pid"], 4321);
        assert_eq!(v["v"], 1);
    }

    #[test]
    fn malformed_request_is_denied() {
        use std::io::{BufRead, BufReader, Write};

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("omawhite.sock");
        let server = Server::bind(&path).unwrap();
        server.serve(|_| Event::Ready {
            id: "x".into(),
            pid: 1,
        });

        let mut conn = std::os::unix::net::UnixStream::connect(&path).unwrap();
        conn.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        conn.write_all(b"{ \"v\": 1, \"op\": \"ping\", \"mal\": true }\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(&mut conn).read_line(&mut reply).unwrap();

        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ev"], "denied");
        assert!(v["reason"].as_str().unwrap().contains("mal"), "{reply}");
    }
}
