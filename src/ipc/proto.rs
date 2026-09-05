//! Line protocol (§5): closed schema, `v: 1`, frame ≤ 64 KiB.
//!
//! The parser is hand-written on purpose: serde's `deny_unknown_fields`
//! does not cover internally tagged enums with a sibling field (`v`), and
//! "unknown field → error, not best effort" is a spec rule. Each op declares
//! exactly the fields it accepts; any leftover is rejected, naming the field.

use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::export::Card;

pub const PROTOCOL_VERSION: u64 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Plugin → app. Intent, never scene content.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Ping,
    New,
    Open {
        id: String,
    },
    /// A project file the user named, by absolute path. Its own op
    /// rather than a second shape of `open`: the schema is closed, and
    /// guessing whether a string is an id or a path is exactly the
    /// best-effort §5 forbids.
    OpenFile {
        path: PathBuf,
    },
    Raise,
    Export {
        dir: PathBuf,
        formats: Vec<ExportFormat>,
    },
    /// With colours, the host is dressing the board in its own three.
    /// Without them, it is saying the desktop's theme has changed and
    /// the board should read it again — which is what a `theme-set` hook
    /// calls. Optional rather than a second op: the field is declared
    /// either way, so the schema stays closed.
    Theme {
        colors: Option<ThemeColors>,
    },
    Shutdown,
    /// What frames the open board has. The three below are an agent's
    /// own door (§8): their answer *is* the work — the listing wants the
    /// live document and a picture wants the GPU — so unlike every op
    /// above them they are not acked and forgotten.
    Frames,
    /// One frame, exported into `dir` as the page §8 describes. `dir` is
    /// a candidate like any other export destination, measured against
    /// the allowlist (§8.2) before anything is written.
    ReadFrame {
        id: String,
        dir: PathBuf,
    },
    /// A frame handed over, in a file the caller wrote. A path and not
    /// the content: §5 says the socket speaks intent and never scene
    /// content, and a frame with ink in it would not fit the frame
    /// anyway.
    AddFrame {
        path: PathBuf,
    },
}

impl Request {
    /// The op this request goes by on the wire. One mapping, read both
    /// by the line that carries it and by a denial that has to name it.
    pub fn op(&self) -> &'static str {
        match self {
            Request::Ping => "ping",
            Request::New => "new",
            Request::Raise => "raise",
            Request::Shutdown => "shutdown",
            Request::Open { .. } => "open",
            Request::OpenFile { .. } => "open_file",
            Request::Export { .. } => "export",
            Request::Theme { .. } => "theme",
            Request::Frames => "frames",
            Request::ReadFrame { .. } => "read_frame",
            Request::AddFrame { .. } => "add_frame",
        }
    }

    /// Whether the answer to this request *is* the work: the three an
    /// agent asks (§8), which the event loop does rather than acks.
    pub fn is_asked(&self) -> bool {
        matches!(
            self,
            Request::Frames | Request::ReadFrame { .. } | Request::AddFrame { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Json,
    Md,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Json => "json",
            ExportFormat::Md => "md",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeColors {
    pub bg: String,
    pub fg: String,
    pub accent: String,
}

/// App → plugin. `Saved`/`Exported` belong to the §5 contract; the app
/// starts emitting them with autosave and export (§15 items 4–5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
#[allow(dead_code)]
pub enum Event {
    Ready { id: String, pid: u32 },
    Saved { id: String },
    Exported { files: Vec<String> },
    Denied { op: String, reason: String },
    Exited { code: i32 },
    /// The frames the open board has, in paint order.
    Frames { frames: Vec<Card> },
    /// A frame an agent handed over is on the board, under the id and
    /// the name it now goes by — enough to read it straight back.
    Framed { id: String, name: String },
}

/// `ev`, or `denied` in its place when it would not fit a frame. An
/// answer cut short in silence would be a lie about the board, and the
/// cap is the one §5 states.
pub fn fits(ev: Event, op: &str) -> Event {
    if event_line(&ev).len() <= MAX_FRAME_BYTES {
        return ev;
    }
    Event::Denied {
        op: op.to_owned(),
        reason: format!("the answer is above the {MAX_FRAME_BYTES} byte frame"),
    }
}

/// Reads a reply line. The CLI is a client of this protocol as much as
/// the plugin is, and reading its own replies by poking at a `Value`
/// would be a second, looser parser for the schema this module owns.
/// `v` is checked; the event's own fields are serde's.
pub fn parse_event(line: &str) -> anyhow::Result<Event> {
    let value: Value = serde_json::from_str(line).context("invalid JSON")?;
    match value.get("v").and_then(Value::as_u64) {
        Some(PROTOCOL_VERSION) => {}
        other => anyhow::bail!("reply speaks v {other:?}, not {PROTOCOL_VERSION}"),
    }
    serde_json::from_value(value).context("unknown reply")
}

/// Deserializes a request line, enforcing the closed schema.
pub fn parse_request(line: &str) -> anyhow::Result<Request> {
    let value: Value = serde_json::from_str(line).context("invalid JSON")?;
    let Value::Object(mut map) = value else {
        anyhow::bail!("request must be a JSON object");
    };

    match map.remove("v") {
        Some(Value::Number(n)) if n.as_u64() == Some(PROTOCOL_VERSION) => {}
        Some(other) => anyhow::bail!("field v must be {PROTOCOL_VERSION}, got {other}"),
        None => anyhow::bail!("field v missing"),
    }

    let op = match map.remove("op") {
        Some(Value::String(s)) => s,
        Some(other) => anyhow::bail!("field op must be a string, got {other}"),
        None => anyhow::bail!("field op missing"),
    };

    let req = match op.as_str() {
        "ping" => Request::Ping,
        "new" => Request::New,
        "raise" => Request::Raise,
        "shutdown" => Request::Shutdown,
        "open" => Request::Open {
            id: take_string(&mut map, "id")?,
        },
        "open_file" => Request::OpenFile {
            path: absolute(take_string(&mut map, "path")?)?,
        },
        "export" => Request::Export {
            dir: PathBuf::from(take_string(&mut map, "dir")?),
            formats: take_formats(&mut map)?,
        },
        "theme" => Request::Theme {
            colors: take_colors(&mut map)?,
        },
        "frames" => Request::Frames,
        // Absolute, unlike `export`'s: the caller's working directory is
        // not the running instance's, so a relative destination would be
        // resolved against the wrong one. The CLI, which does know the
        // caller's, is where a relative `--to` becomes absolute.
        "read_frame" => Request::ReadFrame {
            id: take_string(&mut map, "id")?,
            dir: absolute(take_string(&mut map, "dir")?)?,
        },
        "add_frame" => Request::AddFrame {
            path: absolute(take_string(&mut map, "path")?)?,
        },
        other => anyhow::bail!("unknown op: {other:?}"),
    };

    reject_leftovers(&map, &op)?;
    Ok(req)
}

/// Serializes a request as a wire line (JSON + `v` + `\n`) — the CLI speaks
/// the same protocol as the plugin (§5).
pub fn request_line(req: &Request) -> String {
    let mut map = Map::new();
    map.insert("v".into(), PROTOCOL_VERSION.into());
    let path = |p: &PathBuf| Value::from(p.to_string_lossy().into_owned());
    match req {
        Request::Ping | Request::New | Request::Raise | Request::Shutdown | Request::Frames => {}
        Request::Open { id } => {
            map.insert("id".into(), id.clone().into());
        }
        Request::OpenFile { path: at } => {
            map.insert("path".into(), path(at));
        }
        Request::Export { dir, formats } => {
            map.insert("dir".into(), path(dir));
            let formats: Vec<Value> = formats.iter().map(|f| Value::from(f.as_str())).collect();
            map.insert("formats".into(), formats.into());
        }
        Request::Theme { colors } => {
            if let Some(colors) = colors {
                let mut c = Map::new();
                c.insert("bg".into(), colors.bg.clone().into());
                c.insert("fg".into(), colors.fg.clone().into());
                c.insert("accent".into(), colors.accent.clone().into());
                map.insert("colors".into(), Value::Object(c));
            }
        }
        Request::ReadFrame { id, dir } => {
            map.insert("id".into(), id.clone().into());
            map.insert("dir".into(), path(dir));
        }
        Request::AddFrame { path: at } => {
            map.insert("path".into(), path(at));
        }
    }
    map.insert("op".into(), req.op().into());
    let mut line = Value::Object(map).to_string();
    line.push('\n');
    line
}

/// Serializes an event as a wire line (JSON + `v` + `\n`).
pub fn event_line(ev: &Event) -> String {
    let mut value = serde_json::to_value(ev).expect("Event always serializes to JSON");
    value
        .as_object_mut()
        .expect("Event serializes as an object")
        .insert("v".into(), PROTOCOL_VERSION.into());
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Reads one frame (line) from `r`, rejecting frames above MAX_FRAME_BYTES.
/// `Ok(None)` on clean EOF.
pub fn read_frame(r: &mut impl std::io::BufRead) -> anyhow::Result<Option<String>> {
    use std::io::{BufRead, Read};
    let mut line = String::new();
    let n = r
        .by_ref()
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_line(&mut line)
        .context("reading frame")?;
    if n == 0 {
        return Ok(None);
    }
    anyhow::ensure!(n <= MAX_FRAME_BYTES, "frame above {MAX_FRAME_BYTES} bytes");
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(Some(line))
}

fn take_string(map: &mut Map<String, Value>, key: &str) -> anyhow::Result<String> {
    match map.remove(key) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => anyhow::bail!("field {key} must be a string, got {other}"),
        None => anyhow::bail!("field {key} missing"),
    }
}

/// A path arriving on the wire, checked for shape before it is a path.
///
/// Not the export allowlist (§8.2): that one guards *destinations*, and
/// this is a source the user picked in a portal dialog and the recents
/// merely remembered. What it does refuse is a path that cannot mean the
/// same thing at both ends — a relative one, since the running instance's
/// working directory is not the caller's, and one carrying `..`, which
/// hides where it lands from anyone reading the line. The document behind
/// it is still parsed through the closed schema, blobs and all.
fn absolute(path: String) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(path);
    anyhow::ensure!(path.is_absolute(), "path must be absolute: {path:?}");
    anyhow::ensure!(
        !path
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "path may not contain `..`: {path:?}"
    );
    Ok(path)
}

fn take_formats(map: &mut Map<String, Value>) -> anyhow::Result<Vec<ExportFormat>> {
    let Some(value) = map.remove("formats") else {
        anyhow::bail!("field formats missing");
    };
    let Value::Array(items) = value else {
        anyhow::bail!("field formats must be a list");
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::String(s) => match s.as_str() {
                "png" => Ok(ExportFormat::Png),
                "json" => Ok(ExportFormat::Json),
                "md" => Ok(ExportFormat::Md),
                other => anyhow::bail!("unknown export format: {other:?}"),
            },
            other => anyhow::bail!("formats must contain strings, got {other}"),
        })
        .collect()
}

fn take_colors(map: &mut Map<String, Value>) -> anyhow::Result<Option<ThemeColors>> {
    let Some(value) = map.remove("colors") else {
        return Ok(None);
    };
    let Value::Object(mut m) = value else {
        anyhow::bail!("field colors must be an object");
    };
    let colors = ThemeColors {
        bg: take_string(&mut m, "bg")?,
        fg: take_string(&mut m, "fg")?,
        accent: take_string(&mut m, "accent")?,
    };
    reject_leftovers(&m, "theme.colors")?;
    Ok(Some(colors))
}

fn reject_leftovers(map: &Map<String, Value>, ctx: &str) -> anyhow::Result<()> {
    if let Some(key) = map.keys().next() {
        anyhow::bail!("unknown field in {ctx}: {key:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_ops() {
        for (line, want) in [
            (r#"{ "v": 1, "op": "ping" }"#, Request::Ping),
            (r#"{ "v": 1, "op": "new" }"#, Request::New),
            (r#"{ "v": 1, "op": "raise" }"#, Request::Raise),
            (r#"{ "v": 1, "op": "shutdown" }"#, Request::Shutdown),
        ] {
            assert_eq!(parse_request(line).unwrap(), want, "line: {line}");
        }
    }

    #[test]
    fn parses_open_with_id() {
        let got = parse_request(r#"{ "v": 1, "op": "open", "id": "01JABC" }"#).unwrap();
        assert_eq!(
            got,
            Request::Open {
                id: "01JABC".into()
            }
        );
    }

    #[test]
    fn parses_open_file_with_an_absolute_path() {
        let got =
            parse_request(r#"{ "v": 1, "op": "open_file", "path": "/home/you/plan.omawhite" }"#)
                .unwrap();
        assert_eq!(
            got,
            Request::OpenFile {
                path: PathBuf::from("/home/you/plan.omawhite")
            }
        );
    }

    #[test]
    fn open_file_refuses_a_path_that_means_two_things() {
        // Relative: the live instance's working directory is not the
        // caller's, so the line would name a different file at each end.
        // `..`: a line whose destination cannot be read off it.
        for line in [
            r#"{ "v": 1, "op": "open_file", "path": "plan.omawhite" }"#,
            r#"{ "v": 1, "op": "open_file", "path": "./plan.omawhite" }"#,
            r#"{ "v": 1, "op": "open_file", "path": "/home/you/../etc/shadow" }"#,
        ] {
            assert!(parse_request(line).is_err(), "{line} should be refused");
        }
    }

    #[test]
    fn open_file_is_as_closed_as_every_other_op() {
        for line in [
            r#"{ "v": 1, "op": "open_file" }"#,
            r#"{ "v": 1, "op": "open_file", "id": "01JABC" }"#,
            r#"{ "v": 1, "op": "open_file", "path": "/a/b", "extra": 1 }"#,
            r#"{ "v": 1, "op": "open_file", "path": 7 }"#,
        ] {
            assert!(parse_request(line).is_err(), "{line} should be refused");
        }
    }

    #[test]
    fn parses_export_with_dir_and_formats() {
        let got = parse_request(
            r#"{ "v": 1, "op": "export", "dir": "/home/you/Work/foo", "formats": ["png", "json", "md"] }"#,
        )
        .unwrap();
        assert_eq!(
            got,
            Request::Export {
                dir: PathBuf::from("/home/you/Work/foo"),
                formats: vec![ExportFormat::Png, ExportFormat::Json, ExportFormat::Md],
            }
        );
    }

    #[test]
    fn parses_theme_colors() {
        let got = parse_request(
            r##"{ "v": 1, "op": "theme", "colors": { "bg": "#1a1a1a", "fg": "#eee", "accent": "#7aa" } }"##,
        )
        .unwrap();
        assert_eq!(
            got,
            Request::Theme {
                colors: Some(ThemeColors {
                    bg: "#1a1a1a".into(),
                    fg: "#eee".into(),
                    accent: "#7aa".into(),
                }),
            }
        );
    }

    /// No colours is not a missing field: it is the host saying the
    /// desktop's own theme has changed and the board should read it
    /// again. The schema stays closed either way.
    #[test]
    fn theme_without_colours_asks_the_desktop() {
        let got = parse_request(r##"{ "v": 1, "op": "theme" }"##).unwrap();
        assert_eq!(got, Request::Theme { colors: None });
        let err = parse_request(r##"{ "v": 1, "op": "theme", "mode": "dark" }"##).unwrap_err();
        assert!(err.to_string().contains("mode"), "{err}");
    }

    #[test]
    fn rejects_unknown_op() {
        let err = parse_request(r#"{ "v": 1, "op": "hack" }"#).unwrap_err();
        assert!(err.to_string().contains("hack"), "{err}");
    }

    #[test]
    fn rejects_unknown_fields_by_name() {
        let err = parse_request(r#"{ "v": 1, "op": "ping", "extra": 1 }"#).unwrap_err();
        assert!(err.to_string().contains("extra"), "{err}");

        let err =
            parse_request(r#"{ "v": 1, "op": "open", "id": "x", "path": "/etc" }"#).unwrap_err();
        assert!(err.to_string().contains("path"), "{err}");

        // An unknown field inside a nested object is rejected too.
        let err = parse_request(
            r##"{ "v": 1, "op": "theme", "colors": { "bg": "#000", "fg": "#fff", "accent": "#7aa", "url": "http://x" } }"##,
        )
        .unwrap_err();
        assert!(err.to_string().contains("url"), "{err}");
    }

    #[test]
    fn rejects_missing_or_wrong_version() {
        for line in [
            r#"{ "op": "ping" }"#,
            r#"{ "v": 2, "op": "ping" }"#,
            r#"{ "v": "1", "op": "ping" }"#,
        ] {
            let err = parse_request(line).unwrap_err();
            assert!(err.to_string().contains('v'), "line {line}: {err}");
        }
    }

    #[test]
    fn rejects_garbage() {
        for line in ["", "42", "\"ping\"", "[1,2]", "{", r#"{ "v": 1 }"#] {
            assert!(parse_request(line).is_err(), "line {line:?} should fail");
        }
    }

    #[test]
    fn rejects_unknown_export_format() {
        let err = parse_request(
            r#"{ "v": 1, "op": "export", "dir": "/tmp/x", "formats": ["png", "exe"] }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("exe"), "{err}");
    }

    #[test]
    fn event_lines_match_wire_format() {
        // serde_json without preserve_order = keys in alphabetical order.
        let line = event_line(&Event::Ready {
            id: "01J".into(),
            pid: 1234,
        });
        assert_eq!(
            line,
            "{\"ev\":\"ready\",\"id\":\"01J\",\"pid\":1234,\"v\":1}\n"
        );

        let line = event_line(&Event::Denied {
            op: "export".into(),
            reason: "path-outside-allowlist".into(),
        });
        assert_eq!(
            line,
            "{\"ev\":\"denied\",\"op\":\"export\",\"reason\":\"path-outside-allowlist\",\"v\":1}\n"
        );
    }

    #[test]
    fn request_lines_roundtrip_through_the_parser() {
        let all = [
            Request::Ping,
            Request::New,
            Request::Raise,
            Request::Shutdown,
            Request::Open {
                id: "01JABC".into(),
            },
            Request::OpenFile {
                path: PathBuf::from("/home/you/Work/plan.omawhite"),
            },
            Request::Export {
                dir: PathBuf::from("/home/you/Work/foo"),
                formats: vec![ExportFormat::Png, ExportFormat::Md],
            },
            Request::Theme {
                colors: Some(ThemeColors {
                    bg: "#1a1a1a".into(),
                    fg: "#eee".into(),
                    accent: "#7aa".into(),
                }),
            },
            Request::Theme { colors: None },
            Request::Frames,
            Request::ReadFrame {
                id: "01JABC".into(),
                dir: PathBuf::from("/home/you/Work/foo"),
            },
            Request::AddFrame {
                path: PathBuf::from("/home/you/Work/foo/frame.json"),
            },
        ];
        for req in all {
            let line = request_line(&req);
            assert!(line.ends_with('\n'), "line must end in \\n: {line:?}");
            assert_eq!(parse_request(line.trim_end()).unwrap(), req, "line: {line}");
        }
    }

    #[test]
    fn request_line_matches_wire_format() {
        assert_eq!(request_line(&Request::Ping), "{\"op\":\"ping\",\"v\":1}\n");
    }

    #[test]
    fn read_frame_returns_lines_then_eof() {
        let mut input = std::io::Cursor::new(b"{\"v\":1,\"op\":\"ping\"}\nrest\n".to_vec());
        assert_eq!(
            read_frame(&mut input).unwrap().as_deref(),
            Some("{\"v\":1,\"op\":\"ping\"}")
        );
        assert_eq!(read_frame(&mut input).unwrap().as_deref(), Some("rest"));
        assert_eq!(read_frame(&mut input).unwrap(), None);
    }

    #[test]
    fn read_frame_rejects_oversized_lines() {
        let big = vec![b'a'; MAX_FRAME_BYTES + 1024];
        let mut input = std::io::Cursor::new(big);
        assert!(read_frame(&mut input).is_err());
    }

    fn card(id: &str) -> Card {
        Card {
            id: id.into(),
            name: "Auth Flow".into(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 50.0,
            elements: 3,
        }
    }

    #[test]
    fn parses_the_ops_an_agent_asks() {
        assert_eq!(
            parse_request(r#"{ "v": 1, "op": "frames" }"#).unwrap(),
            Request::Frames
        );
        assert_eq!(
            parse_request(r#"{ "v": 1, "op": "read_frame", "id": "01J", "dir": "/tmp/w" }"#)
                .unwrap(),
            Request::ReadFrame {
                id: "01J".into(),
                dir: PathBuf::from("/tmp/w"),
            }
        );
        assert_eq!(
            parse_request(r#"{ "v": 1, "op": "add_frame", "path": "/tmp/w/f.json" }"#).unwrap(),
            Request::AddFrame {
                path: PathBuf::from("/tmp/w/f.json"),
            }
        );
    }

    #[test]
    fn an_agents_op_refuses_a_path_that_means_two_things() {
        // A relative destination would be resolved against the running
        // instance's working directory, which is not the caller's.
        for line in [
            r#"{ "v": 1, "op": "read_frame", "id": "01J", "dir": "docs" }"#,
            r#"{ "v": 1, "op": "read_frame", "id": "01J", "dir": "/tmp/../etc" }"#,
            r#"{ "v": 1, "op": "add_frame", "path": "f.json" }"#,
            r#"{ "v": 1, "op": "add_frame", "path": "/tmp/../etc/f.json" }"#,
        ] {
            assert!(parse_request(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn an_agents_op_is_as_closed_as_every_other() {
        for line in [
            r#"{ "v": 1, "op": "frames", "of": "board" }"#,
            r#"{ "v": 1, "op": "read_frame", "id": "01J", "dir": "/tmp", "scale": 2 }"#,
            r#"{ "v": 1, "op": "add_frame", "path": "/tmp/f.json", "at": [0, 0] }"#,
            // A missing field is not a shorter sentence, it is an error.
            r#"{ "v": 1, "op": "read_frame", "id": "01J" }"#,
            r#"{ "v": 1, "op": "add_frame" }"#,
        ] {
            assert!(parse_request(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn the_agents_events_match_the_wire_format() {
        let line = event_line(&Event::Frames {
            frames: vec![card("01J")],
        });
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ev"], "frames");
        assert_eq!(v["v"], 1);
        assert_eq!(v["frames"][0]["id"], "01J");
        assert_eq!(v["frames"][0]["name"], "Auth Flow");
        assert_eq!(v["frames"][0]["elements"], 3);

        let line = event_line(&Event::Framed {
            id: "01K".into(),
            name: "Auth Flow".into(),
        });
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ev"], "framed");
        assert_eq!(v["id"], "01K");
    }

    #[test]
    fn an_answer_too_big_for_a_frame_is_denied_rather_than_cut_short() {
        let small = Event::Frames {
            frames: vec![card("01J")],
        };
        assert_eq!(fits(small.clone(), "frames"), small);

        let many = Event::Frames {
            frames: (0..5000).map(|n| card(&format!("{n:020}"))).collect(),
        };
        let Event::Denied { op, reason } = fits(many, "frames") else {
            panic!("a listing past the frame is denied, never trimmed");
        };
        assert_eq!(op, "frames");
        assert!(reason.contains(&MAX_FRAME_BYTES.to_string()), "{reason}");
    }
}
