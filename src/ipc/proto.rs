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

use crate::doc::{Align, BlendMode, Tag, TextMode, Valign};
use crate::editor::Listed;
use crate::export::{Card, TextCard};
use crate::tree::{Arrange, Place};

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
    /// The open board's layers, the whole tree top first. It and every
    /// op under it are the command line's hold on the layers: answered by
    /// the loop like an agent's three, a change each one step of the
    /// history, and every layer named by its id — a name is the command
    /// line's to resolve, since two layers may go by one.
    Layers,
    /// A new raster layer — or group — above `above`, or above the
    /// active layer, called `name` or the next free name.
    AddLayer {
        group: bool,
        name: Option<String>,
        above: Option<String>,
    },
    RemoveLayers {
        ids: Vec<String>,
    },
    RenameLayer {
        id: String,
        name: String,
    },
    /// Into a group or a frame, or just above or below a layer.
    MoveLayers {
        ids: Vec<String>,
        to: Place,
    },
    /// Within their own stacks: to the front, a step, to the back.
    ArrangeLayers {
        ids: Vec<String>,
        how: Arrange,
    },
    ShowLayers {
        ids: Vec<String>,
        visible: bool,
    },
    LockLayers {
        ids: Vec<String>,
        locked: bool,
    },
    SetOpacity {
        ids: Vec<String>,
        opacity: f64,
    },
    SetBlend {
        ids: Vec<String>,
        blend: BlendMode,
    },
    SetColor {
        ids: Vec<String>,
        color: Tag,
    },
    GroupLayers {
        ids: Vec<String>,
    },
    /// Each group named, taken apart.
    Ungroup {
        ids: Vec<String>,
    },
    DuplicateLayers {
        ids: Vec<String>,
    },
    /// Siblings into one, or a group alone into a layer.
    MergeLayers {
        ids: Vec<String>,
    },
    MergeDown {
        id: String,
    },
    MergeVisible,
    Flatten,
    /// The layers picked, as the panel picks them — and with the select
    /// tool in hand, their objects.
    SelectLayers {
        ids: Vec<String>,
    },
    /// Groups and frames shown open in the panel, or shut.
    OpenLayers {
        ids: Vec<String>,
        open: bool,
    },
    /// The texts on show on the open board.
    Texts,
    /// A text put on the board, on a text layer of its own: in the frame
    /// `frame` names, its place measured from the frame's corner, or on
    /// the open board. A text's words are carried on the line itself, as
    /// a layer's name is — they are the intent, and the frame's cap is
    /// the most they can be.
    AddText {
        frame: Option<String>,
        spec: TextSpec,
    },
    /// Text `id` changed as `spec` says.
    SetText {
        id: String,
        spec: TextSpec,
    },
}

impl TextSpec {
    /// Writes every field it says onto `t`: the words, the box, the turn
    /// and the style. Where the box lands on the board is the caller's
    /// to say when the text is new — a frame's corner is where `x` and
    /// `y` count from then.
    pub fn apply(&self, t: &mut crate::doc::Text) {
        fn set<T: Clone>(to: &mut T, from: &Option<T>) {
            if let Some(v) = from {
                *to = v.clone();
            }
        }
        // New words carry the runs with them, as typing them would.
        if let Some(words) = &self.text {
            let n = t.text.chars().count();
            let edit = crate::spans::edit_between(&t.text, words);
            t.runs = crate::spans::edit(&t.runs, n, &t.style, edit, None);
            words.clone_into(&mut t.text);
        }
        set(&mut t.mode, &self.mode);
        set(&mut t.x, &self.x);
        set(&mut t.y, &self.y);
        set(&mut t.w, &self.w);
        set(&mut t.h, &self.h);
        set(&mut t.rotation, &self.rotation);
        set(&mut t.style.align, &self.align);
        set(&mut t.style.valign, &self.valign);
        set(&mut t.style.leading, &self.leading);
        // What a run may set: over the range, or the whole text's — the
        // stretches no longer set apart in it.
        let patch = self.patch();
        let n = t.text.chars().count();
        match self.range {
            Some((a, b)) => t.runs = crate::spans::apply(&t.runs, n, &t.style, a.min(n), b.min(n), &patch),
            None => {
                t.style = patch.over(&t.style);
                t.runs = crate::spans::strip(&t.runs, n, &t.style, &patch);
            }
        }
        t.runs = crate::spans::tidy(&t.runs, n, &t.style);
    }

    /// What of it a run may carry.
    pub fn patch(&self) -> crate::doc::RunStyle {
        crate::doc::RunStyle {
            font: self.font.clone(),
            size: self.size,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            color: self.color.clone(),
            tracking: self.tracking,
        }
    }
}

/// What `add_text` and `set_text` say about a text: each field, when it
/// is there, is what the text says or how it is set from now on. Every
/// value is checked on the way in, as the board's own parse checks it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextSpec {
    pub text: Option<String>,
    pub mode: Option<TextMode>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub rotation: Option<f64>,
    pub font: Option<String>,
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub align: Option<Align>,
    pub valign: Option<Valign>,
    pub leading: Option<f64>,
    pub tracking: Option<f64>,
    pub color: Option<String>,
    /// For `set_text`: the stretch, characters `start..end`, that what a
    /// run may carry is set on — the rest of the text left as it is.
    pub range: Option<(usize, usize)>,
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
            Request::Layers => "layers",
            Request::AddLayer { .. } => "add_layer",
            Request::RemoveLayers { .. } => "remove_layers",
            Request::RenameLayer { .. } => "rename_layer",
            Request::MoveLayers { .. } => "move_layers",
            Request::ArrangeLayers { .. } => "arrange_layers",
            Request::ShowLayers { .. } => "show_layers",
            Request::LockLayers { .. } => "lock_layers",
            Request::SetOpacity { .. } => "set_opacity",
            Request::SetBlend { .. } => "set_blend",
            Request::SetColor { .. } => "set_color",
            Request::GroupLayers { .. } => "group_layers",
            Request::Ungroup { .. } => "ungroup",
            Request::DuplicateLayers { .. } => "duplicate_layers",
            Request::MergeLayers { .. } => "merge_layers",
            Request::MergeDown { .. } => "merge_down",
            Request::MergeVisible => "merge_visible",
            Request::Flatten => "flatten",
            Request::SelectLayers { .. } => "select_layers",
            Request::OpenLayers { .. } => "open_layers",
            Request::Texts => "texts",
            Request::AddText { .. } => "add_text",
            Request::SetText { .. } => "set_text",
        }
    }

    /// Whether the answer to this request *is* the work: the three an
    /// agent asks (§8) and every layer op, which the event loop does
    /// rather than acks — a listing wants the live document, and a change
    /// answers with what it left picked.
    pub fn is_asked(&self) -> bool {
        !matches!(
            self,
            Request::Ping
                | Request::New
                | Request::Open { .. }
                | Request::OpenFile { .. }
                | Request::Raise
                | Request::Export { .. }
                | Request::Theme { .. }
                | Request::Shutdown
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
    /// The open board's layers, the whole tree top first.
    Layers { layers: Vec<Listed> },
    /// A change to the layers landed, and these are the layers it left
    /// picked: the new one, the group, the copies, what a merge kept, or
    /// the ones it acted on.
    Done { ids: Vec<String> },
    /// The texts on show, in paint order.
    Texts { texts: Vec<TextCard> },
    /// A text landed or changed: its id, and its layer's.
    Texted { id: String, layer: String },
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
        "layers" => Request::Layers,
        "add_layer" => Request::AddLayer {
            group: match take_optional_string(&mut map, "kind")?.as_deref() {
                None | Some("raster") => false,
                Some("group") => true,
                Some(other) => anyhow::bail!("a layer added is a raster layer or a group, not {other:?}"),
            },
            name: take_optional_string(&mut map, "name")?,
            above: take_optional_string(&mut map, "above")?,
        },
        "remove_layers" => Request::RemoveLayers {
            ids: take_ids(&mut map)?,
        },
        "rename_layer" => Request::RenameLayer {
            id: take_string(&mut map, "id")?,
            name: take_string(&mut map, "name")?,
        },
        "move_layers" => Request::MoveLayers {
            ids: take_ids(&mut map)?,
            to: take_place(&mut map)?,
        },
        "arrange_layers" => Request::ArrangeLayers {
            ids: take_ids(&mut map)?,
            how: match take_string(&mut map, "how")?.as_str() {
                "front" => Arrange::Front,
                "forward" => Arrange::Forward,
                "backward" => Arrange::Backward,
                "back" => Arrange::Back,
                other => anyhow::bail!("unknown arrangement: {other:?}"),
            },
        },
        "show_layers" => Request::ShowLayers {
            ids: take_ids(&mut map)?,
            visible: take_bool(&mut map, "visible")?,
        },
        "lock_layers" => Request::LockLayers {
            ids: take_ids(&mut map)?,
            locked: take_bool(&mut map, "locked")?,
        },
        "set_opacity" => Request::SetOpacity {
            ids: take_ids(&mut map)?,
            opacity: take_opacity(&mut map)?,
        },
        "set_blend" => Request::SetBlend {
            ids: take_ids(&mut map)?,
            blend: take_named(&mut map, "blend")?,
        },
        "set_color" => Request::SetColor {
            ids: take_ids(&mut map)?,
            color: take_named(&mut map, "color")?,
        },
        "group_layers" => Request::GroupLayers {
            ids: take_ids(&mut map)?,
        },
        "ungroup" => Request::Ungroup {
            ids: take_ids(&mut map)?,
        },
        "duplicate_layers" => Request::DuplicateLayers {
            ids: take_ids(&mut map)?,
        },
        "merge_layers" => Request::MergeLayers {
            ids: take_ids(&mut map)?,
        },
        "merge_down" => Request::MergeDown {
            id: take_string(&mut map, "id")?,
        },
        "merge_visible" => Request::MergeVisible,
        "flatten" => Request::Flatten,
        "select_layers" => Request::SelectLayers {
            ids: take_ids(&mut map)?,
        },
        "open_layers" => Request::OpenLayers {
            ids: take_ids(&mut map)?,
            open: take_bool(&mut map, "open")?,
        },
        "texts" => Request::Texts,
        "add_text" => {
            let frame = take_optional_string(&mut map, "frame")?;
            let spec = take_spec(&mut map)?;
            anyhow::ensure!(spec.range.is_none(), "field range is set_text's: a new text is set as a whole");
            anyhow::ensure!(
                spec.text.as_deref().is_some_and(|t| !t.is_empty()),
                "field text missing: a text has to say something"
            );
            Request::AddText { frame, spec }
        }
        "set_text" => {
            let id = take_string(&mut map, "id")?;
            let spec = take_spec(&mut map)?;
            anyhow::ensure!(spec != TextSpec::default(), "set_text says nothing to change");
            anyhow::ensure!(
                spec.range.is_none() || !spec.patch().is_empty(),
                "field range names a stretch, and nothing that a stretch may carry is set on it"
            );
            anyhow::ensure!(
                spec.text.as_deref() != Some(""),
                "field text is empty: a text has to say something — remove its layer to take it away"
            );
            Request::SetText { id, spec }
        }
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
        Request::Ping
        | Request::New
        | Request::Raise
        | Request::Shutdown
        | Request::Frames
        | Request::Layers
        | Request::MergeVisible
        | Request::Flatten => {}
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
        Request::AddLayer { group, name, above } => {
            if *group {
                map.insert("kind".into(), "group".into());
            }
            if let Some(name) = name {
                map.insert("name".into(), name.clone().into());
            }
            if let Some(above) = above {
                map.insert("above".into(), above.clone().into());
            }
        }
        Request::RenameLayer { id, name } => {
            map.insert("id".into(), id.clone().into());
            map.insert("name".into(), name.clone().into());
        }
        Request::MergeDown { id } => {
            map.insert("id".into(), id.clone().into());
        }
        Request::RemoveLayers { ids }
        | Request::GroupLayers { ids }
        | Request::Ungroup { ids }
        | Request::DuplicateLayers { ids }
        | Request::MergeLayers { ids }
        | Request::SelectLayers { ids } => {
            map.insert("ids".into(), ids.clone().into());
        }
        Request::MoveLayers { ids, to } => {
            map.insert("ids".into(), ids.clone().into());
            let (place, target) = match to {
                Place::Into(t) => ("into", t),
                Place::Above(t) => ("above", t),
                Place::Below(t) => ("below", t),
            };
            map.insert("place".into(), place.into());
            map.insert("target".into(), target.clone().into());
        }
        Request::ArrangeLayers { ids, how } => {
            map.insert("ids".into(), ids.clone().into());
            let how = match how {
                Arrange::Front => "front",
                Arrange::Forward => "forward",
                Arrange::Backward => "backward",
                Arrange::Back => "back",
            };
            map.insert("how".into(), how.into());
        }
        Request::ShowLayers { ids, visible } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("visible".into(), (*visible).into());
        }
        Request::LockLayers { ids, locked } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("locked".into(), (*locked).into());
        }
        Request::SetOpacity { ids, opacity } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("opacity".into(), (*opacity).into());
        }
        Request::SetBlend { ids, blend } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("blend".into(), serde_json::to_value(blend).expect("a mode is a name"));
        }
        Request::SetColor { ids, color } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("color".into(), serde_json::to_value(color).expect("a colour is a name"));
        }
        Request::OpenLayers { ids, open } => {
            map.insert("ids".into(), ids.clone().into());
            map.insert("open".into(), (*open).into());
        }
        Request::Texts => {}
        Request::AddText { frame, spec } => {
            if let Some(frame) = frame {
                map.insert("frame".into(), frame.clone().into());
            }
            put_spec(&mut map, spec);
        }
        Request::SetText { id, spec } => {
            map.insert("id".into(), id.clone().into());
            put_spec(&mut map, spec);
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

/// A text's fields, each where it is given.
fn put_spec(map: &mut Map<String, Value>, spec: &TextSpec) {
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(v) = value {
            map.insert(key.into(), v);
        }
    };
    let named = |v: Option<Value>| v;
    put("text", spec.text.clone().map(Value::from));
    put("mode", named(spec.mode.map(|m| serde_json::to_value(m).expect("a mode is a name"))));
    put("x", spec.x.map(Value::from));
    put("y", spec.y.map(Value::from));
    put("w", spec.w.map(Value::from));
    put("h", spec.h.map(Value::from));
    put("rotation", spec.rotation.map(Value::from));
    put("font", spec.font.clone().map(Value::from));
    put("size", spec.size.map(Value::from));
    put("bold", spec.bold.map(Value::from));
    put("italic", spec.italic.map(Value::from));
    put("underline", spec.underline.map(Value::from));
    put("strike", spec.strike.map(Value::from));
    put("align", spec.align.map(|a| serde_json::to_value(a).expect("an alignment is a name")));
    put("valign", spec.valign.map(|v| serde_json::to_value(v).expect("an alignment is a name")));
    put("leading", spec.leading.map(Value::from));
    put("tracking", spec.tracking.map(Value::from));
    put("color", spec.color.clone().map(Value::from));
    put("range", spec.range.map(|(a, b)| Value::from(vec![a, b])));
}

/// How far from the world's origin, in world units, a text the socket
/// places may reach, and how big its box may be: far past any board a
/// hand draws, and short of where a box's far edge or a merge's picture
/// of it overflows to infinity — which a board writes as `null` and
/// never opens again.
const WORLD: f64 = 1e9;

/// A text's fields as the line gives them, every one checked as the
/// board's own parse would: printable words with no control character
/// but a newline, finite numbers inside what a text may be, a family
/// that names something, and a colour that is `#rgb` or `#rrggbb`.
fn take_spec(map: &mut Map<String, Value>) -> anyhow::Result<TextSpec> {
    let text = match map.remove("text") {
        None => None,
        Some(Value::String(s)) => {
            anyhow::ensure!(
                s.chars().all(|c| c == '\n' || !c.is_control()),
                "field text may hold newlines but no other control character"
            );
            Some(s)
        }
        Some(other) => anyhow::bail!("field text must be a string, got {other}"),
    };
    let number = |map: &mut Map<String, Value>, key: &str, range: Option<(f64, f64)>| -> anyhow::Result<Option<f64>> {
        let v = match map.remove(key) {
            None => return Ok(None),
            Some(Value::Number(n)) => n.as_f64().filter(|v| v.is_finite()),
            Some(other) => anyhow::bail!("field {key} must be a number, got {other}"),
        };
        let Some(v) = v else {
            anyhow::bail!("field {key} must be a finite number");
        };
        if let Some((lo, hi)) = range {
            anyhow::ensure!((lo..=hi).contains(&v), "field {key} must be between {lo} and {hi}, got {v}");
        }
        Ok(Some(v))
    };
    let flag = |map: &mut Map<String, Value>, key: &str| -> anyhow::Result<Option<bool>> {
        match map.remove(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(b)),
            Some(other) => anyhow::bail!("field {key} must be true or false, got {other}"),
        }
    };
    let optional_named = |map: &mut Map<String, Value>, key: &str| -> anyhow::Result<Option<Value>> {
        Ok(map.remove(key))
    };
    let mode: Option<TextMode> = match optional_named(map, "mode")? {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| anyhow::anyhow!("unknown mode: {v}"))?),
    };
    let align: Option<Align> = match optional_named(map, "align")? {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| anyhow::anyhow!("unknown align: {v}"))?),
    };
    let valign: Option<Valign> = match optional_named(map, "valign")? {
        None => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| anyhow::anyhow!("unknown valign: {v}"))?),
    };
    let color = take_optional_string(map, "color")?;
    if let Some(c) = &color {
        let hex = c.strip_prefix('#').unwrap_or("");
        anyhow::ensure!(
            matches!(hex.len(), 3 | 6) && hex.chars().all(|d| d.is_ascii_hexdigit()),
            "field color must be #rgb or #rrggbb, got {c:?}"
        );
    }
    let spec = TextSpec {
        text,
        mode,
        x: number(map, "x", Some((-WORLD, WORLD)))?,
        y: number(map, "y", Some((-WORLD, WORLD)))?,
        w: number(map, "w", Some((0.0, WORLD)))?,
        h: number(map, "h", Some((0.0, WORLD)))?,
        rotation: number(map, "rotation", None)?,
        font: take_optional_string(map, "font")?,
        size: number(map, "size", Some((crate::doc::MIN_TEXT_SIZE, crate::doc::MAX_TEXT_SIZE)))?,
        bold: flag(map, "bold")?,
        italic: flag(map, "italic")?,
        underline: flag(map, "underline")?,
        strike: flag(map, "strike")?,
        align,
        valign,
        leading: number(map, "leading", Some((crate::doc::MIN_LEADING, crate::doc::MAX_LEADING)))?,
        tracking: number(map, "tracking", Some((crate::doc::MIN_TRACKING, crate::doc::MAX_TRACKING)))?,
        color,
        range: match map.remove("range") {
            None => None,
            Some(Value::Array(pair)) => match pair.as_slice() {
                [Value::Number(a), Value::Number(b)] => {
                    let (a, b) = (a.as_u64(), b.as_u64());
                    match (a, b) {
                        (Some(a), Some(b)) if a < b => Some((a as usize, b as usize)),
                        _ => anyhow::bail!("field range must be [start, end], two places with start before end"),
                    }
                }
                _ => anyhow::bail!("field range must be [start, end]"),
            },
            Some(other) => anyhow::bail!("field range must be [start, end], got {other}"),
        },
    };
    Ok(spec)
}

fn take_string(map: &mut Map<String, Value>, key: &str) -> anyhow::Result<String> {
    match map.remove(key) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => anyhow::bail!("field {key} must be a string, got {other}"),
        None => anyhow::bail!("field {key} missing"),
    }
}

/// A field that may be left out, but not left empty: an empty name is no
/// name, and an empty id is no layer.
fn take_optional_string(map: &mut Map<String, Value>, key: &str) -> anyhow::Result<Option<String>> {
    match map.remove(key) {
        None => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s)),
        Some(other) => anyhow::bail!("field {key} must be a string that says something, got {other}"),
    }
}

/// The layers an op acts on: a list of ids, none of them empty, and
/// never an empty list — nothing named is nothing to act on.
fn take_ids(map: &mut Map<String, Value>) -> anyhow::Result<Vec<String>> {
    let Some(Value::Array(items)) = map.remove("ids") else {
        anyhow::bail!("field ids must be a list of layer ids");
    };
    anyhow::ensure!(!items.is_empty(), "field ids names no layer");
    items
        .into_iter()
        .map(|item| match item {
            Value::String(s) if !s.is_empty() => Ok(s),
            other => anyhow::bail!("ids must be layer ids, got {other}"),
        })
        .collect()
}

fn take_bool(map: &mut Map<String, Value>, key: &str) -> anyhow::Result<bool> {
    match map.remove(key) {
        Some(Value::Bool(b)) => Ok(b),
        Some(other) => anyhow::bail!("field {key} must be true or false, got {other}"),
        None => anyhow::bail!("field {key} missing"),
    }
}

/// A strength: a fraction, refused rather than clamped outside one.
fn take_opacity(map: &mut Map<String, Value>) -> anyhow::Result<f64> {
    let value = match map.remove("opacity") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(other) => anyhow::bail!("field opacity must be a number, got {other}"),
        None => anyhow::bail!("field opacity missing"),
    };
    match value {
        Some(v) if (0.0..=1.0).contains(&v) => Ok(v),
        _ => anyhow::bail!("field opacity must be between 0 and 1"),
    }
}

/// A blend mode or a colour, by the name a board writes it under.
fn take_named<T: serde::de::DeserializeOwned>(map: &mut Map<String, Value>, key: &str) -> anyhow::Result<T> {
    match map.remove(key) {
        Some(Value::String(s)) => serde_json::from_value(Value::String(s.clone()))
            .map_err(|_| anyhow::anyhow!("unknown {key}: {s:?}")),
        Some(other) => anyhow::bail!("field {key} must be a name, got {other}"),
        None => anyhow::bail!("field {key} missing"),
    }
}

/// Where layers go: into a group or a frame, or above or below a layer.
fn take_place(map: &mut Map<String, Value>) -> anyhow::Result<Place> {
    let place = take_string(map, "place")?;
    let target = take_string(map, "target")?;
    anyhow::ensure!(!target.is_empty(), "field target names no layer");
    Ok(match place.as_str() {
        "into" => Place::Into(target),
        "above" => Place::Above(target),
        "below" => Place::Below(target),
        other => anyhow::bail!("unknown place: {other:?}"),
    })
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
    use crate::doc::Kind;

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
            parse_request(r#"{ "v": 1, "op": "open_file", "path": "/home/you/plan.sinopia" }"#)
                .unwrap();
        assert_eq!(
            got,
            Request::OpenFile {
                path: PathBuf::from("/home/you/plan.sinopia")
            }
        );
    }

    #[test]
    fn open_file_refuses_a_path_that_means_two_things() {
        // Relative: the live instance's working directory is not the
        // caller's, so the line would name a different file at each end.
        // `..`: a line whose destination cannot be read off it.
        for line in [
            r#"{ "v": 1, "op": "open_file", "path": "plan.sinopia" }"#,
            r#"{ "v": 1, "op": "open_file", "path": "./plan.sinopia" }"#,
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
                path: PathBuf::from("/home/you/Work/plan.sinopia"),
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
        for req in all.into_iter().chain(layer_ops()) {
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

    fn two() -> Vec<String> {
        vec!["A".into(), "B".into()]
    }

    /// One of every layer op, every field it takes filled.
    fn layer_ops() -> Vec<Request> {
        vec![
            Request::Layers,
            Request::AddLayer {
                group: true,
                name: Some("Sky".into()),
                above: Some("A".into()),
            },
            Request::AddLayer {
                group: false,
                name: None,
                above: None,
            },
            Request::RemoveLayers { ids: two() },
            Request::RenameLayer {
                id: "A".into(),
                name: "Sky".into(),
            },
            Request::MoveLayers {
                ids: two(),
                to: Place::Into("G".into()),
            },
            Request::MoveLayers {
                ids: two(),
                to: Place::Above("G".into()),
            },
            Request::MoveLayers {
                ids: two(),
                to: Place::Below("G".into()),
            },
            Request::ArrangeLayers {
                ids: two(),
                how: Arrange::Front,
            },
            Request::ArrangeLayers {
                ids: two(),
                how: Arrange::Backward,
            },
            Request::ShowLayers {
                ids: two(),
                visible: false,
            },
            Request::LockLayers {
                ids: two(),
                locked: true,
            },
            Request::SetOpacity {
                ids: two(),
                opacity: 0.5,
            },
            Request::SetBlend {
                ids: two(),
                blend: BlendMode::ColorDodge,
            },
            Request::SetColor {
                ids: two(),
                color: Tag::Violet,
            },
            Request::GroupLayers { ids: two() },
            Request::Ungroup { ids: two() },
            Request::DuplicateLayers { ids: two() },
            Request::MergeLayers { ids: two() },
            Request::MergeDown { id: "B".into() },
            Request::MergeVisible,
            Request::Flatten,
            Request::SelectLayers { ids: two() },
            Request::OpenLayers {
                ids: two(),
                open: true,
            },
        ]
    }

    #[test]
    fn parses_the_layer_ops() {
        for (line, want) in [
            (r#"{ "v": 1, "op": "layers" }"#, Request::Layers),
            (
                r#"{ "v": 1, "op": "add_layer" }"#,
                Request::AddLayer {
                    group: false,
                    name: None,
                    above: None,
                },
            ),
            (
                r#"{ "v": 1, "op": "add_layer", "kind": "group", "name": "Sky", "above": "A" }"#,
                Request::AddLayer {
                    group: true,
                    name: Some("Sky".into()),
                    above: Some("A".into()),
                },
            ),
            (
                r#"{ "v": 1, "op": "move_layers", "ids": ["A", "B"], "place": "into", "target": "G" }"#,
                Request::MoveLayers {
                    ids: two(),
                    to: Place::Into("G".into()),
                },
            ),
            (
                r#"{ "v": 1, "op": "arrange_layers", "ids": ["A", "B"], "how": "back" }"#,
                Request::ArrangeLayers {
                    ids: two(),
                    how: Arrange::Back,
                },
            ),
            (
                r#"{ "v": 1, "op": "set_opacity", "ids": ["A", "B"], "opacity": 0.25 }"#,
                Request::SetOpacity {
                    ids: two(),
                    opacity: 0.25,
                },
            ),
            (
                r#"{ "v": 1, "op": "set_blend", "ids": ["A", "B"], "blend": "passThrough" }"#,
                Request::SetBlend {
                    ids: two(),
                    blend: BlendMode::PassThrough,
                },
            ),
            (
                r#"{ "v": 1, "op": "set_color", "ids": ["A", "B"], "color": "none" }"#,
                Request::SetColor {
                    ids: two(),
                    color: Tag::None,
                },
            ),
            (
                r#"{ "v": 1, "op": "merge_down", "id": "B" }"#,
                Request::MergeDown { id: "B".into() },
            ),
            (r#"{ "v": 1, "op": "flatten" }"#, Request::Flatten),
        ] {
            assert_eq!(parse_request(line).unwrap(), want, "line: {line}");
        }
        for req in layer_ops() {
            assert!(req.is_asked(), "{}: the answer is the work", req.op());
        }
    }

    #[test]
    fn a_layer_op_is_as_closed_as_every_other() {
        for line in [
            r#"{ "v": 1, "op": "layers", "of": "board" }"#,
            r#"{ "v": 1, "op": "remove_layers" }"#,
            r#"{ "v": 1, "op": "remove_layers", "ids": [] }"#,
            r#"{ "v": 1, "op": "remove_layers", "ids": [1] }"#,
            r#"{ "v": 1, "op": "remove_layers", "ids": [""] }"#,
            r#"{ "v": 1, "op": "remove_layers", "ids": "A" }"#,
            r#"{ "v": 1, "op": "add_layer", "kind": "frame" }"#,
            r#"{ "v": 1, "op": "add_layer", "name": "" }"#,
            r#"{ "v": 1, "op": "rename_layer", "id": "A" }"#,
            r#"{ "v": 1, "op": "move_layers", "ids": ["A"], "place": "beside", "target": "B" }"#,
            r#"{ "v": 1, "op": "move_layers", "ids": ["A"], "place": "into" }"#,
            r#"{ "v": 1, "op": "arrange_layers", "ids": ["A"], "how": "sideways" }"#,
            r#"{ "v": 1, "op": "show_layers", "ids": ["A"], "visible": "yes" }"#,
            r#"{ "v": 1, "op": "lock_layers", "ids": ["A"] }"#,
            r#"{ "v": 1, "op": "set_opacity", "ids": ["A"], "opacity": 1.5 }"#,
            r#"{ "v": 1, "op": "set_opacity", "ids": ["A"], "opacity": -0.1 }"#,
            r#"{ "v": 1, "op": "set_opacity", "ids": ["A"], "opacity": "half" }"#,
            r#"{ "v": 1, "op": "set_blend", "ids": ["A"], "blend": "sparkle" }"#,
            r#"{ "v": 1, "op": "set_color", "ids": ["A"], "color": "pink" }"#,
            r#"{ "v": 1, "op": "merge_down", "ids": ["A"] }"#,
            r#"{ "v": 1, "op": "merge_visible", "ids": ["A"] }"#,
            r#"{ "v": 1, "op": "open_layers", "ids": ["A"], "open": 1 }"#,
        ] {
            assert!(parse_request(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn the_layer_events_match_the_wire_format() {
        let listed = Listed {
            id: "A".into(),
            name: "Sky".into(),
            kind: Kind::Raster,
            owner: Some("G".into()),
            depth: 1,
            visible: true,
            shown: false,
            locked: false,
            opacity: 0.5,
            blend: BlendMode::ColorDodge,
            color: Tag::Red,
            active: true,
            picked: true,
            elements: 2,
        };
        let ev = Event::Layers {
            layers: vec![listed],
        };
        let line = event_line(&ev);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ev"], "layers");
        assert_eq!(v["layers"][0]["kind"], "raster");
        assert_eq!(v["layers"][0]["blend"], "colorDodge");
        assert_eq!(v["layers"][0]["color"], "red");
        assert_eq!(v["layers"][0]["owner"], "G");
        assert_eq!(parse_event(line.trim_end()).unwrap(), ev);
        let done = Event::Done { ids: two() };
        let line = event_line(&done);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((v["ev"].as_str(), v["ids"][1].as_str()), (Some("done"), Some("B")));
        assert_eq!(parse_event(line.trim_end()).unwrap(), done);
    }

    fn text_ops() -> Vec<Request> {
        vec![
            Request::Texts,
            Request::AddText {
                frame: None,
                spec: TextSpec {
                    text: Some("Hello".into()),
                    ..TextSpec::default()
                },
            },
            Request::AddText {
                frame: Some("F1".into()),
                spec: TextSpec {
                    text: Some("two\nlines".into()),
                    mode: Some(TextMode::Frame),
                    x: Some(10.0),
                    y: Some(-4.5),
                    w: Some(200.0),
                    h: Some(80.0),
                    rotation: Some(15.0),
                    font: Some("Noto Serif".into()),
                    size: Some(18.0),
                    bold: Some(true),
                    italic: Some(false),
                    underline: Some(true),
                    strike: Some(false),
                    align: Some(Align::Justify),
                    valign: Some(Valign::Middle),
                    leading: Some(1.5),
                    tracking: Some(-20.0),
                    color: Some("#ff8800".into()),
                    range: None,
                },
            },
            Request::SetText {
                id: "T1".into(),
                spec: TextSpec {
                    size: Some(40.0),
                    ..TextSpec::default()
                },
            },
            Request::SetText {
                id: "T1".into(),
                spec: TextSpec {
                    bold: Some(true),
                    range: Some((2, 5)),
                    ..TextSpec::default()
                },
            },
        ]
    }

    #[test]
    fn the_text_ops_round_trip_and_are_asked() {
        for req in text_ops() {
            let line = request_line(&req);
            assert_eq!(parse_request(line.trim_end()).unwrap(), req, "line: {line}");
            assert!(req.is_asked(), "{}: the answer is the work", req.op());
        }
    }

    #[test]
    fn a_text_op_is_as_closed_as_every_other() {
        for line in [
            r#"{ "v": 1, "op": "texts", "of": "board" }"#,
            r#"{ "v": 1, "op": "add_text" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a\u001b[31mred" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a\rb" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "size": 0 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "size": "big" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "leading": 50 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "tracking": 1e9 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "mode": "poster" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "align": "middle" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "color": "red" }"#,
            r##"{ "v": 1, "op": "add_text", "text": "a", "color": "#12345" }"##,
            r#"{ "v": 1, "op": "add_text", "text": "a", "w": -3 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "x": "left" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "font": "" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "frame": "" }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "bold": 1 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "shadow": true }"#,
            r#"{ "v": 1, "op": "set_text", "size": 12 }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T" }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "text": "" }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "bold": true, "range": [3, 3] }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "bold": true, "range": [4, 1] }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "bold": true, "range": [1] }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "bold": true, "range": [-1, 2] }"#,
            r#"{ "v": 1, "op": "set_text", "id": "T", "range": [0, 2] }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "range": [0, 1] }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "x": 1e308 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "y": -1e12 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "w": 1.7e308 }"#,
            r#"{ "v": 1, "op": "add_text", "text": "a", "h": 1e10 }"#,
        ] {
            assert!(parse_request(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn the_text_events_match_the_wire_format() {
        let doc = crate::doc::Document::from_json(
            r##"{ "schema": 1, "id": "01JXXXXXXXXXXXXXXXXXXXXXXX", "title": "t",
                  "camera": { "x": 0, "y": 0, "zoom": 1 },
                  "layers": [ { "id": "L", "name": "Hi", "kind": "text" } ],
                  "elements": [ { "id": "T", "type": "text", "layer": "L", "x": 1, "y": 2,
                      "w": 30, "h": 20, "text": "Hi", "size": 16, "bold": true, "color": "#000" } ] }"##,
        )
        .unwrap();
        let ev = Event::Texts {
            texts: crate::export::texts(&doc),
        };
        let line = event_line(&ev);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ev"], "texts");
        assert_eq!(v["texts"][0]["id"], "T");
        assert_eq!(v["texts"][0]["text"], "Hi");
        assert_eq!(v["texts"][0]["bold"], true);
        assert_eq!(v["texts"][0]["name"], "Hi");
        assert_eq!(parse_event(line.trim_end()).unwrap(), ev);
        let placed = Event::Texted {
            id: "T".into(),
            layer: "L".into(),
        };
        let line = event_line(&placed);
        assert_eq!(parse_event(line.trim_end()).unwrap(), placed);
    }

    #[test]
    fn a_spec_writes_what_it_says_and_leaves_the_rest() {
        let mut t = crate::doc::Text {
            id: "T".into(),
            layer: "L".into(),
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            rotation: 0.0,
            mode: TextMode::Artistic,
            text: "was".into(),
            style: crate::doc::TextStyle::default(),
            runs: Vec::new(),
        };
        TextSpec {
            text: Some("now".into()),
            size: Some(40.0),
            italic: Some(true),
            ..TextSpec::default()
        }
        .apply(&mut t);
        assert_eq!((t.text.as_str(), t.style.size, t.style.italic), ("now", 40.0, true));
        assert_eq!((t.x, t.y, t.w, t.h), (1.0, 2.0, 3.0, 4.0));
        assert!(!t.style.bold);
    }

    #[test]
    fn a_spec_with_a_range_sets_that_stretch_and_the_words_carry_the_runs() {
        let mut t = crate::doc::Text {
            id: "T".into(),
            layer: "L".into(),
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            rotation: 0.0,
            mode: TextMode::Artistic,
            text: "hello world".into(),
            style: crate::doc::TextStyle::default(),
            runs: Vec::new(),
        };
        TextSpec {
            bold: Some(true),
            align: Some(Align::Center),
            range: Some((6, 11)),
            ..TextSpec::default()
        }
        .apply(&mut t);
        assert_eq!(t.style.align, Align::Center, "a paragraph's setting is the whole text's");
        assert!(!t.style.bold);
        assert_eq!((t.runs[0].start, t.runs[0].end, t.runs[0].style.bold), (6, 11, Some(true)));
        TextSpec {
            text: Some("hi world".into()),
            ..TextSpec::default()
        }
        .apply(&mut t);
        assert_eq!((t.runs[0].start, t.runs[0].end), (3, 8), "the stretch follows its words");
        TextSpec {
            bold: Some(false),
            ..TextSpec::default()
        }
        .apply(&mut t);
        assert!(t.runs.is_empty(), "with no range, the whole text");
    }
}
