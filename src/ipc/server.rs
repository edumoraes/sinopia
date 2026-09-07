//! Server side of the socket (§5, §9.2): bind at 0600, one JSON per line,
//! request → reply event. A malformed request gets `denied` and the
//! connection closes — a closed schema does no best effort.

use std::io::{BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

use crate::ipc::proto::{Event, Request, event_line, fits, parse_request, read_frame};

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

    /// Consumes the server and serves connections on its own thread, one
    /// thread to a connection. `on_request` runs on that thread and
    /// returns the reply event.
    ///
    /// A connection at a time was enough while every op was acked and
    /// forwarded. An agent's three are *answered by doing them*, which
    /// only the event loop can, so the thread parks on the loop for as
    /// long as `ASK_TIMEOUT` — five times the deadline an ordinary op
    /// gives itself. Single file, that made `raise`, `ping`, `theme` and
    /// `shutdown` fail outright for the length of a graft or a render.
    pub fn serve(
        self,
        on_request: impl Fn(Request) -> Event + Send + Sync + 'static,
    ) -> std::thread::JoinHandle<()> {
        let on_request = Arc::new(on_request);
        std::thread::spawn(move || {
            for conn in self.listener.incoming() {
                let Ok(stream) = conn else { continue };
                let answer = Arc::clone(&on_request);
                // An error on one connection does not bring the server down (§9.2).
                std::thread::spawn(move || {
                    let _ = handle_conn(stream, &*answer);
                });
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
                // The cap is here, where the bytes go out, and not at
                // each of the places an answer is built: an op that
                // slipped past one of those wrote a line no client can
                // read, which for a listing is a lie about the board.
                Ok(req) => {
                    let op = req.op().to_owned();
                    let reply = fits(on_request(req), &op);
                    writer.write_all(event_line(&reply).as_bytes())?
                }
                Err(e) => {
                    let denied = fits(
                        Event::Denied {
                            op: "?".into(),
                            // The offending value is echoed back, so this
                            // is as long as the caller made it.
                            reason: e.to_string(),
                        },
                        "?",
                    );
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
    fn an_ordinary_op_does_not_wait_behind_one_the_loop_is_answering() {
        // An agent's three are answered by *doing* them, so the
        // connection parks on the event loop — up to `ASK_TIMEOUT`, five
        // times the deadline `raise` and `ping` give themselves. Those
        // two are what the launcher and the Omarchy plugin send, and
        // single file they simply failed for the length of a graft or a
        // render.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("omawhite.sock");
        let server = Server::bind(&path).unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let started = std::sync::Mutex::new(started);
        server.serve(move |req| {
            if req.is_asked() {
                let _ = started.lock().expect("lock").send(());
                std::thread::sleep(Duration::from_millis(500));
                return Event::Frames { frames: Vec::new() };
            }
            Event::Ready {
                id: "01J".into(),
                pid: 1,
            }
        });

        let busy = path.clone();
        let slow = std::thread::spawn(move || try_forward(&busy, &Request::Frames));
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("the loop took the agent's op");

        let began = std::time::Instant::now();
        let reply = try_forward(&path, &Request::Ping).unwrap().unwrap();
        assert!(reply.contains("\"ready\""), "{reply}");
        assert!(
            began.elapsed() < Duration::from_millis(250),
            "a raise waited {:?} behind the agent",
            began.elapsed()
        );
        slow.join().unwrap().unwrap();
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
