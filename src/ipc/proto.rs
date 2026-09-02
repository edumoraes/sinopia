//! Line protocol (§5): closed schema, `v: 1`, frame ≤ 64 KiB.
//!
//! The parser is hand-written on purpose: serde's `deny_unknown_fields`
//! does not cover internally tagged enums with a sibling field (`v`), and
//! "unknown field → error, not best effort" is a spec rule. Each op declares
//! exactly the fields it accepts; any leftover is rejected, naming the field.

use std::path::PathBuf;

use anyhow::Context as _;
use serde::Serialize;
use serde_json::{Map, Value};

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
    Raise,
    Export {
        dir: PathBuf,
        formats: Vec<ExportFormat>,
    },
    Theme {
        colors: ThemeColors,
    },
    Shutdown,
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
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
#[allow(dead_code)]
pub enum Event {
    Ready { id: String, pid: u32 },
    Saved { id: String },
    Exported { files: Vec<String> },
    Denied { op: String, reason: String },
    Exited { code: i32 },
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
        "export" => Request::Export {
            dir: PathBuf::from(take_string(&mut map, "dir")?),
            formats: take_formats(&mut map)?,
        },
        "theme" => Request::Theme {
            colors: take_colors(&mut map)?,
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
    let op = match req {
        Request::Ping => "ping",
        Request::New => "new",
        Request::Raise => "raise",
        Request::Shutdown => "shutdown",
        Request::Open { id } => {
            map.insert("id".into(), id.clone().into());
            "open"
        }
        Request::Export { dir, formats } => {
            map.insert("dir".into(), dir.to_string_lossy().into_owned().into());
            let formats: Vec<Value> = formats.iter().map(|f| Value::from(f.as_str())).collect();
            map.insert("formats".into(), formats.into());
            "export"
        }
        Request::Theme { colors } => {
            let mut c = Map::new();
            c.insert("bg".into(), colors.bg.clone().into());
            c.insert("fg".into(), colors.fg.clone().into());
            c.insert("accent".into(), colors.accent.clone().into());
            map.insert("colors".into(), Value::Object(c));
            "theme"
        }
    };
    map.insert("op".into(), op.into());
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

fn take_colors(map: &mut Map<String, Value>) -> anyhow::Result<ThemeColors> {
    let Some(value) = map.remove("colors") else {
        anyhow::bail!("field colors missing");
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
    Ok(colors)
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
                colors: ThemeColors {
                    bg: "#1a1a1a".into(),
                    fg: "#eee".into(),
                    accent: "#7aa".into(),
                }
            }
        );
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
            Request::Export {
                dir: PathBuf::from("/home/you/Work/foo"),
                formats: vec![ExportFormat::Png, ExportFormat::Md],
            },
            Request::Theme {
                colors: ThemeColors {
                    bg: "#1a1a1a".into(),
                    fg: "#eee".into(),
                    accent: "#7aa".into(),
                },
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
}
