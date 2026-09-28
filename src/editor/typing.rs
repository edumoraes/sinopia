//! The Text tool, and a text being typed into.
//!
//! A press with the tool starts a text where it landed — artistic text
//! at the point, or a frame the drag spans, as the tool's kind says —
//! and nothing lands on the board until a character is typed: a text
//! nobody wrote is not a text. The text being typed is a [`Typing`]: the
//! string with its caret and its selection (a [`Field`], the same one
//! every box in the chrome types into), the lines it is set in, and the
//! typing's own undo, which walks back a word at a time without the
//! board's history hearing of it. The board hears of the session as one
//! step, however many characters it took (see `History::keep_as`).
//!
//! Places are counted in characters, as a field's caret is, and read off
//! [`Laid`], the text as `typeset` sets it — so the caret can only stand
//! where a letter is drawn.

use super::{CLICK_SLOP_PX, Change, Editor, HIT_SLOP_PX, MIN_FRAME_PX, Tool, point};
use crate::doc::{Document, Element, Kind, MAX_TEXT_SIZE, MIN_TEXT_SIZE, Text, TextMode, TextStyle, new_id};
use crate::field::Field;
use crate::fonts::Fonts;
use crate::geom::{Frame, Point};
use crate::scene::{Prim, ScreenRect, View, with_alpha};
use crate::select;
use crate::theme::Theme;
use crate::typeset::Laid;

/// The most a text holds, in bytes: a page of a novel is a few thousand;
/// what runs past this is not a label on a board.
pub const TEXT_MAX: usize = 100_000;

/// A click with frame text makes a frame this wide in world units, and
/// this many lines tall — room to start typing in, as Affinity gives.
const FRAME_W: f64 = 240.0;
const FRAME_LINES: f64 = 3.0;

/// The most of what a text says that its layer is named after.
const NAME_CHARS: usize = 40;

/// The caret's width, in logical px.
const CARET_W: f32 = 1.5;
/// The side of the mark a frame too small for its text wears.
const OVERFLOW_PX: f32 = 9.0;
/// Affinity's own warning red.
const OVERFLOW: crate::scene::Rgba = [0.86, 0.16, 0.16, 1.0];

/// Where a key moves the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    /// The start and the end of the line as it is set.
    LineHome,
    LineEnd,
    /// The start and the end of the whole text.
    Home,
    End,
}

/// What a key does to the text being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKey {
    /// The caret goes somewhere; with `true`, carrying the selection.
    Go(Move, bool),
    Backspace,
    /// `Ctrl+Backspace`: the word before the caret.
    WordBackspace,
    Delete,
    /// `Ctrl+Delete`: the word after it.
    WordDelete,
    Newline,
    SelectAll,
    Undo,
    Redo,
}

/// What a drag through the text selects by: what the press that started
/// it took — a character, a word on a double click, a paragraph on a
/// triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Paragraph,
}

/// What an edit did, for the typing's undo: a run of one kind is one
/// step, and a word typed after a space starts a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Insert,
    Delete,
}

/// A state of the text the typing's undo can go back to.
#[derive(Debug, Clone, PartialEq)]
struct Snap {
    value: String,
    caret: usize,
    anchor: Option<usize>,
}

/// A text being typed into.
#[derive(Debug, Clone)]
pub struct Typing {
    /// The text's id — minted at the press for a new one, so the session
    /// names it before it is on the board.
    pub id: String,
    /// A new text, until its first character lands: on no layer, in no
    /// document. `None` once it is on the board.
    pending: Option<Text>,
    /// The frame, by its layer, a new text lands in — read at the press,
    /// as a stroke's is.
    born: Option<String>,
    field: Field,
    laid: Laid,
    /// The x the caret keeps to going up and down, in the text's box.
    goal: Option<f64>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last edit and whether it put down a space, which is what
    /// cuts a run of typing into words.
    last: Option<(Edit, bool)>,
    /// A drag through the text, and the span its press took.
    dragging: Option<(Unit, usize, usize)>,
}

/// The sizes `Ctrl+Shift+>` and `<` step through: Adobe's own list,
/// which is the one hands have learnt.
const SIZES: [f64; 27] = [
    6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 48.0, 56.0, 64.0,
    72.0, 80.0, 96.0, 120.0, 144.0, 192.0, 288.0, 400.0,
];

/// The next size up or down the list from `size` — a size between two
/// steps to the one it is between — and past either end a quarter more or
/// less, within what a text may be set at.
pub fn step_size(size: f64, up: bool) -> f64 {
    let next = if up {
        SIZES.iter().copied().find(|&s| s > size).unwrap_or(size * 1.25)
    } else {
        SIZES.iter().rev().copied().find(|&s| s < size).unwrap_or(size / 1.25)
    };
    next.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE)
}

/// What a text layer is named from what it says: its first line with
/// anything written on it, cut short with an ellipsis — or nothing, for a
/// text that says nothing yet.
pub fn name_of(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut name: String = line.chars().take(NAME_CHARS).collect();
    if line.chars().count() > NAME_CHARS {
        name.push('…');
    }
    Some(name)
}

/// The text `id` names, if it is one.
fn text_of<'a>(doc: &'a Document, id: &str) -> Option<&'a Text> {
    doc.elements.iter().find_map(|el| match el {
        Element::Text(t) if t.id == id => Some(t),
        _ => None,
    })
}

fn text_mut<'a>(doc: &'a mut Document, id: &str) -> Option<&'a mut Text> {
    doc.elements.iter_mut().find_map(|el| match el {
        Element::Text(t) if t.id == id => Some(t),
        _ => None,
    })
}

/// The box a text stands in, turned.
fn frame_of(t: &Text) -> Frame {
    Frame {
        center: [t.x + t.w / 2.0, t.y + t.h / 2.0],
        half: [t.w / 2.0, t.h / 2.0],
        angle: t.rotation.to_radians(),
    }
}

/// Where a point of the world stands in a text's own box, from its
/// top-left corner before the turn.
fn in_box(t: &Text, world: Point) -> (f64, f64) {
    let local = frame_of(t).to_local(world);
    (local[0] + t.w / 2.0, local[1] + t.h / 2.0)
}

/// Artistic text measures its own box, so what it says and how it is set
/// decide how big the box is. The box grows away from where it is
/// anchored — its top left for text standing at the left, its top middle
/// for centred text, its top right for text at the right — so typing
/// runs on from where the text was started, whichever way it is turned.
fn refit(t: &mut Text, laid: &Laid) {
    if t.mode == TextMode::Frame {
        return;
    }
    let (w, h) = (laid.w, laid.h);
    let across = |w: f64| match t.style.align {
        crate::doc::Align::Center => 0.0,
        crate::doc::Align::Right => w / 2.0,
        crate::doc::Align::Left | crate::doc::Align::Justify => -w / 2.0,
    };
    let f = frame_of(t);
    let anchor = f.to_world([across(t.w), -t.h / 2.0]);
    let (sin, cos) = f.angle.sin_cos();
    let (lx, ly) = (across(w), -h / 2.0);
    let center = [anchor[0] - (cos * lx - sin * ly), anchor[1] - (sin * lx + cos * ly)];
    t.x = center[0] - w / 2.0;
    t.y = center[1] - h / 2.0;
    t.w = w;
    t.h = h;
}

/// Fits every artistic text of `doc` to what its lines measure in the
/// faces this machine has — a board written elsewhere, a fragment an
/// agent wrote, a text the command line added: its box is what the
/// pointer, a selection and a frame's claim read, and it has to be the
/// one the letters are drawn in. True when any moved.
pub fn fit_texts(doc: &mut Document, fonts: &Fonts) -> bool {
    let mut moved = false;
    for el in &mut doc.elements {
        let Element::Text(t) = el else { continue };
        if t.mode != TextMode::Artistic {
            continue;
        }
        let laid = Laid::of(t, fonts);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        if close(t.w, laid.w) && close(t.h, laid.h) {
            continue;
        }
        refit(t, &laid);
        moved = true;
    }
    moved
}

/// Whether one of the two changes asks for a save.
fn both(a: Change, b: Change) -> Change {
    match (a, b) {
        (Change::Scene, _) | (_, Change::Scene) => Change::Scene,
        (Change::None, other) => other,
        (other, _) => other,
    }
}

impl Editor {
    /// How the next text is set — what the bar shows with nothing to
    /// look at, and what a text the command line adds starts from.
    pub fn next_style(&self) -> TextStyle {
        self.text_style.clone()
    }

    /// A tool's key: the tool — or, for the Text tool already in hand,
    /// the other kind of text, as Affinity's two text tools share `T`.
    pub fn choose_tool(&mut self, tool: Tool, doc: &mut Document) {
        if tool == Tool::Text && self.tool == Tool::Text {
            self.text_mode = match self.text_mode {
                TextMode::Artistic => TextMode::Frame,
                TextMode::Frame => TextMode::Artistic,
            };
            return;
        }
        self.set_tool(tool, doc);
    }

    /// The text being typed into, if one is.
    pub fn typing(&self) -> Option<&Typing> {
        self.typing.as_ref()
    }

    /// The area a press with the Text tool is dragging out.
    pub fn placing(&self) -> Option<(Point, Point)> {
        self.placing.as_ref().map(|(a, b, _)| (*a, *b))
    }

    /// The typing session the last change belongs to, if a text is being
    /// typed — or has just been left, which is a change of its own.
    pub fn fold(&self) -> Option<u64> {
        self.folding
    }

    /// The session's last change has been written down.
    pub fn let_go_of_fold(&mut self) {
        if self.typing.is_none() {
            self.folding = None;
        }
    }

    /// The text under `screen` that a press would type into: on show,
    /// not locked, topmost.
    pub fn text_at(&self, doc: &Document, view: &View, screen: (f64, f64)) -> Option<String> {
        let world = point(view.screen_to_world(screen.0, screen.1));
        let slop = HIT_SLOP_PX * view.scale / view.px_per_world();
        let id = select::element_at(doc, world, slop)?;
        text_of(doc, id).map(|t| t.id.clone())
    }

    /// Whether a drag through the text being typed is selecting.
    pub(super) fn drags_through_text(&self) -> bool {
        self.typing.as_ref().is_some_and(|t| t.dragging.is_some())
    }

    /// The text being typed, as it stands: on the board, or not yet.
    fn typed<'a>(&'a self, doc: &'a Document) -> Option<&'a Text> {
        let t = self.typing.as_ref()?;
        t.pending.as_ref().or_else(|| text_of(doc, &t.id))
    }

    /// The place in the text being typed nearest a point of the world.
    fn place_at(&self, doc: &Document, world: Point) -> Option<usize> {
        let text = self.typed(doc)?;
        let (x, y) = in_box(text, world);
        Some(self.typing.as_ref()?.laid.index_at(x, y))
    }

    /// A drag through the text being typed carries the selection to the
    /// place under the pointer, by the unit its press took: the far end
    /// of the character, word or paragraph there, the press's own span
    /// always held.
    pub(super) fn select_through_text(&mut self, view: &View, screen: (f64, f64), doc: &Document) -> Change {
        let world = point(view.screen_to_world(screen.0, screen.1));
        let Some(i) = self.place_at(doc, world) else {
            return Change::None;
        };
        let Some(t) = self.typing.as_mut() else {
            return Change::None;
        };
        let Some((unit, a, b)) = t.dragging else {
            return Change::None;
        };
        let (s, e) = match unit {
            Unit::Char => (i, i),
            Unit::Word => t.laid.word_at(i),
            Unit::Paragraph => t.laid.paragraph_at(i),
        };
        if s < a {
            t.field.go(b, false);
            t.field.go(s, true);
        } else {
            t.field.go(a, false);
            t.field.go(e.max(b), true);
        }
        t.goal = None;
        Change::Selection
    }

    pub(super) fn stop_selecting_text(&mut self) -> Change {
        if let Some(t) = self.typing.as_mut() {
            t.dragging = None;
        }
        Change::Selection
    }

    /// Ends the session when what it types into is no longer there to
    /// type into: its text gone from the board — removed, merged, undone
    /// away — or kept by a lock. A text not yet on the board is not gone.
    /// Answers what ending it did, or nothing when it goes on.
    pub fn settle_typing(&mut self, doc: &mut Document) -> Change {
        let Some(t) = &self.typing else {
            return Change::None;
        };
        if t.pending.is_some() {
            return Change::None;
        }
        match text_of(doc, &t.id) {
            None => {
                self.typing = None;
                self.placing = None;
                Change::Selection
            }
            Some(text) if doc.locked(&text.layer) => self.end_typing(doc),
            Some(_) => Change::None,
        }
    }

    /// Whether a press at `screen` lands away from the text being typed —
    /// which leaves it before anything else the press does.
    pub fn leaves_text(&self, view: &View, screen: (f64, f64), doc: &Document) -> bool {
        let Some(text) = self.typed(doc) else {
            return false;
        };
        let world = point(view.screen_to_world(screen.0, screen.1));
        let slop = HIT_SLOP_PX * view.scale / view.px_per_world();
        !select::frame(&Element::Text(text.clone())).is_some_and(|f| f.contains(world, slop))
    }

    /// A press with the Text tool, or a double click on a text with any
    /// tool: `clicks` counts the presses in a row. Inside the text being
    /// typed, it puts the caret there — a double click takes the word, a
    /// triple the paragraph, Shift carries the selection. On another
    /// text it types into that one, where it landed — with another tool,
    /// only on a double click. Anywhere else it leaves the text being
    /// typed, and with the Text tool starts a new one there.
    pub fn text_press(
        &mut self,
        view: &View,
        screen: (f64, f64),
        doc: &mut Document,
        fonts: &Fonts,
        clicks: u32,
        ink: &str,
    ) -> Change {
        let world = point(view.screen_to_world(screen.0, screen.1));
        let slop = HIT_SLOP_PX * view.scale / view.px_per_world();
        if let Some(text) = self.typed(doc)
            && select::frame(&Element::Text(text.clone())).is_some_and(|f| f.contains(world, slop))
        {
            let Some(i) = self.place_at(doc, world) else {
                return Change::None;
            };
            self.caret_by(i, clicks);
            return Change::Selection;
        }
        let left = self.end_typing(doc);
        // Another text is typed into by the Text tool, or by a double
        // click with any other — a single press with the Select tool is a
        // selection, and the tool's own to make.
        let texting = self.tool == Tool::Text;
        if let Some(id) = self.text_at(doc, view, screen).filter(|_| texting || clicks >= 2) {
            let opened = self.edit_text(&id, doc, fonts, Some(world));
            if texting
                && clicks > 1
                && let Some(i) = self.place_at(doc, world)
            {
                self.caret_by(i, clicks);
            }
            return both(left, opened);
        }
        if self.tool == Tool::Text {
            self.placing = Some((world, world, ink.to_owned()));
            return both(left, Change::Selection);
        }
        left
    }

    /// Puts the caret at place `i` as `clicks` presses in a row do, and
    /// starts a drag selecting by that unit.
    fn caret_by(&mut self, i: usize, clicks: u32) {
        let shift = self.shift;
        let Some(t) = self.typing.as_mut() else { return };
        let (unit, (a, b)) = match clicks {
            0 | 1 => (Unit::Char, (i, i)),
            2 => (Unit::Word, t.laid.word_at(i)),
            _ => (Unit::Paragraph, t.laid.paragraph_at(i)),
        };
        if shift && unit == Unit::Char {
            t.field.go(i, true);
            let anchor = t.field.anchor().unwrap_or(i);
            t.dragging = Some((Unit::Char, anchor, anchor));
        } else {
            t.field.go(a, false);
            t.field.go(b, true);
            t.dragging = Some((unit, a, b));
        }
        t.goal = None;
        t.last = None;
    }

    /// The press of a Text tool gesture came back up: a text opens where
    /// it went down. A click is artistic text with its first line
    /// centred on the point, or a frame to type in; a drag is artistic
    /// text as tall as the drag, or a frame of the area dragged.
    pub(super) fn open_text(&mut self, doc: &Document, view: &View, from: Point, to: Point, ink: &str) -> Change {
        let dragged = (to[0] - from[0]).hypot(to[1] - from[1]) * view.px_per_world() / view.scale;
        let area = (to[0] - from[0]).abs().min((to[1] - from[1]).abs()) * view.px_per_world();
        let mut style = self.text_style.clone();
        ink.clone_into(&mut style.color);
        let line_h = |s: &TextStyle| s.size * s.leading;
        let (lo, hi) = (
            [from[0].min(to[0]), from[1].min(to[1])],
            [from[0].max(to[0]), from[1].max(to[1])],
        );
        let (x, y, w, h) = match self.text_mode {
            TextMode::Artistic if dragged >= CLICK_SLOP_PX && hi[1] - lo[1] > 0.0 => {
                style.size = (hi[1] - lo[1]).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
                (lo[0], lo[1], 0.0, line_h(&style))
            }
            TextMode::Artistic => (from[0], from[1] - line_h(&style) / 2.0, 0.0, line_h(&style)),
            TextMode::Frame if area >= MIN_FRAME_PX => (lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]),
            TextMode::Frame => (
                from[0],
                from[1] - line_h(&style) / 2.0,
                FRAME_W,
                FRAME_LINES * line_h(&style),
            ),
        };
        let text = Text {
            id: new_id(),
            layer: String::new(),
            x,
            y,
            w,
            h,
            rotation: 0.0,
            mode: self.text_mode,
            text: String::new(),
            style,
        };
        let born = doc.stack_at(from).map(str::to_owned);
        self.open(text, born, doc)
    }

    /// Starts a session on a new text that will land in `born`. A release
    /// has no faces in hand, so its lines are set in the bundled face
    /// until [`Editor::relay_text`] sets them in the text's own — which
    /// for a text saying nothing moves only the caret's height.
    fn open(&mut self, text: Text, born: Option<String>, doc: &Document) -> Change {
        self.open_with(text, born, doc, &Fonts::bundled())
    }

    /// Sets the text being typed again in the faces it names.
    pub fn relay_text(&mut self, doc: &Document, fonts: &Fonts) {
        let Some(text) = self.typed(doc).cloned() else { return };
        if let Some(t) = self.typing.as_mut() {
            t.laid = Laid::of(&text, fonts);
        }
    }

    fn open_with(&mut self, text: Text, born: Option<String>, doc: &Document, fonts: &Fonts) -> Change {
        let placed = text_of(doc, &text.id).is_some();
        self.sessions += 1;
        let session = self.sessions;
        let laid = Laid::of(&text, fonts);
        self.typing = Some(Typing {
            id: text.id.clone(),
            field: Field::lines(&text.text).limited(TEXT_MAX),
            laid,
            pending: (!placed).then_some(text),
            born,
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
            dragging: None,
        });
        self.folding = Some(session);
        self.selection.clear();
        Change::Selection
    }

    /// Opens text `id` for typing: the caret at `at` when there is one,
    /// or the whole text selected when there is not — what Enter on a
    /// selected text does.
    pub fn edit_text(&mut self, id: &str, doc: &mut Document, fonts: &Fonts, at: Option<Point>) -> Change {
        let Some(text) = text_of(doc, id).cloned() else {
            return Change::None;
        };
        if doc.locked(&text.layer) {
            return Change::None;
        }
        let left = if self.typing.as_ref().is_some_and(|t| t.id != id) {
            self.end_typing(doc)
        } else {
            Change::None
        };
        let layer = text.layer.clone();
        let _ = self.open_with(text, None, doc, fonts);
        self.reveal(doc, &layer);
        self.layer = Some(layer);
        self.picked.clear();
        match at.and_then(|p| self.place_at(doc, p)) {
            Some(i) => self.caret_by(i, 1),
            None => {
                if let Some(t) = self.typing.as_mut() {
                    t.field.select_all();
                }
            }
        }
        both(left, Change::Selection)
    }

    /// Enter with one text selected: typing into it, all of it selected.
    pub fn edit_selected_text(&mut self, doc: &mut Document, fonts: &Fonts) -> Change {
        let [id] = self.selection.as_slice() else {
            return Change::None;
        };
        let id = id.clone();
        if text_of(doc, &id).is_none() {
            return Change::None;
        }
        self.edit_text(&id, doc, fonts, None)
    }

    /// Takes a state the typing's undo can come back to, before an edit
    /// of `edit`'s kind — unless it carries on the run the last one
    /// started. A run of inserts is cut where a word starts after a space.
    fn before(&mut self, edit: Edit, spaced: bool) {
        let Some(t) = self.typing.as_mut() else { return };
        let starts = match t.last {
            Some((last, was_space)) => last != edit || (edit == Edit::Insert && was_space && !spaced),
            None => true,
        };
        if starts {
            t.undo.push(Snap {
                value: t.field.value().to_owned(),
                caret: t.field.caret(),
                anchor: t.field.anchor(),
            });
        }
        t.redo.clear();
        t.last = Some((edit, spaced));
    }

    /// Writes what the field holds into the text, putting a new one on
    /// the board with the first character that lands, and fits artistic
    /// text to what it now says. The layer of a text is named after it
    /// for as long as nobody has named it by hand.
    fn write(&mut self, doc: &mut Document, fonts: &Fonts) -> Change {
        let settled = self.settle_typing(doc);
        if self.typing.is_none() {
            return settled;
        }
        let Some(t) = self.typing.as_mut() else {
            return Change::None;
        };
        t.goal = None;
        let value = t.field.value().to_owned();
        if let Some(pending) = t.pending.as_mut() {
            pending.text = value.clone();
            let laid = Laid::of(pending, fonts);
            refit(pending, &laid);
            t.laid = laid;
            if value.is_empty() {
                return Change::Selection;
            }
            // The frame the press was in, while it is still on the board:
            // one taken away since — an undo, the command line — leaves
            // the text on the board where it was, as a stroke's does.
            let born = t.born.clone().filter(|id| doc.frame_on(id).is_some());
            let mut text = t.pending.take().expect("pending");
            let layer = self.fresh_layer(doc, Kind::Text, born.as_deref());
            if let (Some(l), Some(name)) = (doc.layer_mut(&layer), name_of(&text.text)) {
                l.name = name;
            }
            text.layer = layer;
            doc.elements.push(Element::Text(text));
            return Change::Scene;
        }
        let id = t.id.clone();
        let Some(text) = text_mut(doc, &id) else {
            return Change::None;
        };
        if text.text == value {
            return Change::Selection;
        }
        let was = std::mem::replace(&mut text.text, value);
        let laid = Laid::of(text, fonts);
        refit(text, &laid);
        let (layer, now) = (text.layer.clone(), text.text.clone());
        if let Some(t) = self.typing.as_mut() {
            t.laid = laid;
        }
        rename_after(doc, &layer, &was, &now);
        Change::Scene
    }

    /// Characters typed at the caret, over the selection. A newline among
    /// them breaks the line, as Enter does; nothing else unprintable is
    /// taken.
    pub fn type_str(&mut self, s: &str, doc: &mut Document, fonts: &Fonts) -> Change {
        let clean: String = s.chars().filter(|&c| c == '\n' || !c.is_control()).collect();
        if clean.is_empty() || self.typing.is_none() {
            return Change::None;
        }
        let spaced = clean.chars().last().is_some_and(char::is_whitespace);
        self.before(Edit::Insert, spaced);
        if let Some(t) = self.typing.as_mut() {
            t.field.insert_str(&clean);
        }
        self.write(doc, fonts)
    }

    /// What the clipboard held, pasted at the caret: a line's breaks
    /// stay, a tab is its spaces, anything else unprintable is dropped.
    pub fn paste_text(&mut self, s: &str, doc: &mut Document, fonts: &Fonts) -> Change {
        if self.typing.is_none() {
            return Change::None;
        }
        self.before(Edit::Insert, true);
        if let Some(t) = self.typing.as_mut() {
            t.field.paste(s);
            t.last = None;
        }
        self.write(doc, fonts)
    }

    /// A key, in the text being typed.
    pub fn text_key(&mut self, key: TextKey, doc: &mut Document, fonts: &Fonts) -> Change {
        if self.typing.is_none() {
            return Change::None;
        }
        match key {
            TextKey::Go(to, extend) => {
                let Some(t) = self.typing.as_mut() else {
                    return Change::None;
                };
                t.last = None;
                let caret = t.field.caret();
                let goal = t.goal.unwrap_or_else(|| t.laid.caret(caret).0);
                let mut keep_goal = false;
                match to {
                    Move::Left => t.field.left(extend),
                    Move::Right => t.field.right(extend),
                    Move::WordLeft => t.field.word_left(extend),
                    Move::WordRight => t.field.word_right(extend),
                    Move::Home => t.field.home(extend),
                    Move::End => t.field.end(extend),
                    Move::LineHome => t.field.go(t.laid.home(caret), extend),
                    Move::LineEnd => t.field.go(t.laid.end(caret), extend),
                    Move::Up => {
                        t.field.go(t.laid.up(caret, goal), extend);
                        keep_goal = true;
                    }
                    Move::Down => {
                        t.field.go(t.laid.down(caret, goal), extend);
                        keep_goal = true;
                    }
                }
                t.goal = keep_goal.then_some(goal);
                Change::Selection
            }
            TextKey::Backspace | TextKey::WordBackspace | TextKey::Delete | TextKey::WordDelete => {
                self.before(Edit::Delete, false);
                if let Some(t) = self.typing.as_mut() {
                    let word = matches!(key, TextKey::WordBackspace | TextKey::WordDelete);
                    let back = matches!(key, TextKey::Backspace | TextKey::WordBackspace);
                    if word && t.field.selection().is_none() {
                        if back {
                            t.field.word_left(true);
                        } else {
                            t.field.word_right(true);
                        }
                    }
                    if back {
                        t.field.backspace();
                    } else {
                        t.field.delete();
                    }
                }
                self.write(doc, fonts)
            }
            TextKey::Newline => {
                self.before(Edit::Insert, true);
                if let Some(t) = self.typing.as_mut() {
                    t.field.newline();
                }
                self.write(doc, fonts)
            }
            TextKey::SelectAll => {
                if let Some(t) = self.typing.as_mut() {
                    t.field.select_all();
                    t.last = None;
                }
                Change::Selection
            }
            TextKey::Undo | TextKey::Redo => {
                let Some(t) = self.typing.as_mut() else {
                    return Change::None;
                };
                let (from, to) = if key == TextKey::Undo {
                    (&mut t.undo, &mut t.redo)
                } else {
                    (&mut t.redo, &mut t.undo)
                };
                let Some(snap) = from.pop() else {
                    return Change::None;
                };
                to.push(Snap {
                    value: t.field.value().to_owned(),
                    caret: t.field.caret(),
                    anchor: t.field.anchor(),
                });
                t.field.restore(&snap.value, snap.caret, snap.anchor);
                t.last = None;
                self.write(doc, fonts)
            }
        }
    }

    /// What is selected in the text being typed, for `Ctrl+C`.
    pub fn copied_text(&self) -> Option<String> {
        let t = self.typing.as_ref()?;
        Some(t.field.selected().to_owned()).filter(|s| !s.is_empty())
    }

    /// `Ctrl+X`: the selection, handed over and taken out.
    pub fn cut_text(&mut self, doc: &mut Document, fonts: &Fonts) -> (Option<String>, Change) {
        let Some(text) = self.copied_text() else {
            return (None, Change::None);
        };
        self.before(Edit::Delete, false);
        if let Some(t) = self.typing.as_mut() {
            t.field.cut();
            t.last = None;
        }
        (Some(text), self.write(doc, fonts))
    }

    /// Leaves the text being typed: kept and selected, or taken off the
    /// board — layer and all — when it says nothing.
    pub fn end_typing(&mut self, doc: &mut Document) -> Change {
        self.placing = None;
        let Some(t) = self.typing.take() else {
            return Change::None;
        };
        if t.pending.is_some() {
            return Change::Selection;
        }
        let Some(text) = text_of(doc, &t.id) else {
            return Change::Selection;
        };
        if !text.text.trim().is_empty() {
            self.selection = vec![t.id];
            return Change::Selection;
        }
        let layer = text.layer.clone();
        doc.elements.retain(|el| el.id() != t.id);
        if !doc.elements.iter().any(|el| el.layer() == layer)
            && let Some((stack, i)) = doc.locate(&layer)
        {
            let stack = stack.map(str::to_owned);
            doc.remove_layer(stack.as_deref(), i);
        }
        self.selection.clear();
        Change::Scene
    }

    /// A text made whole somewhere else — the command line's — put on the
    /// board: a text layer of its own in `born`'s stack, a fresh id, its
    /// box fitted to its letters and its layer named after it. Answers
    /// the text's id and its layer's.
    pub fn place_text(&mut self, doc: &mut Document, fonts: &Fonts, mut text: Text, born: Option<&str>) -> (String, String) {
        let _ = self.end_typing(doc);
        text.id = new_id();
        let laid = Laid::of(&text, fonts);
        refit(&mut text, &laid);
        let born = born.filter(|id| doc.frame_on(id).is_some());
        let layer = self.fresh_layer(doc, Kind::Text, born);
        if let (Some(l), Some(name)) = (doc.layer_mut(&layer), name_of(&text.text)) {
            l.name = name;
        }
        text.layer = layer.clone();
        let id = text.id.clone();
        doc.elements.push(Element::Text(text));
        self.selection = vec![id.clone()];
        (id, layer)
    }

    /// Changes text `id` as `change` says, fits it again and names its
    /// layer after what it now says, as typing into it would. Refused for
    /// a text that is not there, or whose layer a lock keeps.
    pub fn change_text(
        &mut self,
        doc: &mut Document,
        fonts: &Fonts,
        id: &str,
        change: impl FnOnce(&mut Text),
    ) -> Result<Change, String> {
        let Some(text) = text_of(doc, id) else {
            return Err(format!("no text {id:?} on the board that is open"));
        };
        if doc.locked(&text.layer) {
            return Err(format!("{:?} is locked, and a lock keeps what it holds", text.layer));
        }
        if self.typing.as_ref().is_some_and(|t| t.id == id) {
            let _ = self.end_typing(doc);
        }
        let Some(text) = text_mut(doc, id) else {
            return Err(format!("no text {id:?} on the board that is open"));
        };
        // Made on a copy and checked before it lands: a change the board
        // cannot set leaves the text as it was.
        let mut next = text.clone();
        change(&mut next);
        next.style.checked()?;
        if next == *text {
            return Ok(Change::None);
        }
        let laid = Laid::of(&next, fonts);
        refit(&mut next, &laid);
        let was = std::mem::replace(text, next);
        let (layer, now) = (text.layer.clone(), text.text.clone());
        rename_after(doc, &layer, &was.text, &now);
        Ok(Change::Scene)
    }

    /// The texts the bar is looking at: the one being typed, or every
    /// text selected.
    fn targets(&self, doc: &Document) -> Vec<String> {
        if let Some(t) = &self.typing {
            return if t.pending.is_some() { Vec::new() } else { vec![t.id.clone()] };
        }
        self.selection
            .iter()
            .filter(|id| text_of(doc, id).is_some_and(|t| !doc.locked(&t.layer)))
            .cloned()
            .collect()
    }

    /// How text is set where the text bar is looking: the text being
    /// typed, else the first text selected, else what the next text is
    /// born with.
    pub fn text_style(&self, doc: &Document) -> TextStyle {
        if let Some(t) = self.typing.as_ref().and_then(|t| t.pending.as_ref()) {
            return t.style.clone();
        }
        self.targets(doc)
            .first()
            .and_then(|id| text_of(doc, id))
            .map_or_else(|| self.text_style.clone(), |t| t.style.clone())
    }

    /// The kind of text the bar is looking at, on the same terms.
    pub fn text_kind(&self, doc: &Document) -> TextMode {
        if let Some(t) = self.typing.as_ref().and_then(|t| t.pending.as_ref()) {
            return t.mode;
        }
        self.targets(doc)
            .first()
            .and_then(|id| text_of(doc, id))
            .map_or(self.text_mode, |t| t.mode)
    }

    /// Whether the text bar is looking at a text rather than at what the
    /// next one is born with.
    pub fn text_targeted(&self, doc: &Document) -> bool {
        self.typing.is_some() || !self.targets(doc).is_empty()
    }

    /// Changes how text is set where the bar is looking — the text being
    /// typed, or every text selected, or the next one — and fits every
    /// artistic text it changed to its new measure.
    pub fn restyle(&mut self, doc: &mut Document, fonts: &Fonts, change: impl Fn(&mut TextStyle)) -> Change {
        let settle = |s: &mut TextStyle| {
            change(s);
            s.size = s.size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
            s.leading = s.leading.clamp(crate::doc::MIN_LEADING, crate::doc::MAX_LEADING);
            s.tracking = s.tracking.clamp(crate::doc::MIN_TRACKING, crate::doc::MAX_TRACKING);
        };
        if let Some(t) = self.typing.as_mut()
            && let Some(pending) = t.pending.as_mut()
        {
            settle(&mut pending.style);
            let laid = Laid::of(pending, fonts);
            refit(pending, &laid);
            t.laid = laid;
            return Change::Selection;
        }
        let targets = self.targets(doc);
        if targets.is_empty() {
            settle(&mut self.text_style);
            return Change::Selection;
        }
        let mut changed = false;
        for id in &targets {
            let Some(text) = text_mut(doc, id) else { continue };
            let was = text.style.clone();
            settle(&mut text.style);
            if text.style == was {
                continue;
            }
            changed = true;
            let laid = Laid::of(text, fonts);
            refit(text, &laid);
            if let Some(t) = self.typing.as_mut().filter(|t| &t.id == id) {
                t.laid = laid;
            }
        }
        if changed { Change::Scene } else { Change::Selection }
    }

    /// Every text the bar is looking at, with the family it is set in: what
    /// the font menu puts back when it is left without a line taken.
    pub fn text_fonts(&self, doc: &Document) -> Vec<(String, String)> {
        if let Some(pending) = self.typing.as_ref().and_then(|t| t.pending.as_ref()) {
            return vec![(pending.id.clone(), pending.style.font.clone())];
        }
        self.targets(doc)
            .into_iter()
            .filter_map(|id| text_of(doc, &id).map(|t| (id.clone(), t.style.font.clone())))
            .collect()
    }

    /// Sets every text in `was` back in the family it names, fitting
    /// artistic text to it again.
    pub fn put_fonts_back(&mut self, doc: &mut Document, fonts: &Fonts, was: &[(String, String)]) {
        for (id, family) in was {
            if let Some(t) = self.typing.as_mut()
                && let Some(pending) = t.pending.as_mut().filter(|p| &p.id == id)
            {
                family.clone_into(&mut pending.style.font);
                let laid = Laid::of(pending, fonts);
                refit(pending, &laid);
                t.laid = laid;
                continue;
            }
            let Some(text) = text_mut(doc, id) else { continue };
            if text.style.font == *family {
                continue;
            }
            family.clone_into(&mut text.style.font);
            let laid = Laid::of(text, fonts);
            refit(text, &laid);
            if let Some(t) = self.typing.as_mut().filter(|t| &t.id == id) {
                t.laid = laid;
            }
        }
    }

    /// The bar's kind switch: the texts it is looking at become that kind
    /// — a frame keeps the box it had, artistic text fits its lines — or,
    /// with none, the tool makes that kind from now on.
    pub fn set_text_kind(&mut self, mode: TextMode, doc: &mut Document, fonts: &Fonts) -> Change {
        if let Some(t) = self.typing.as_mut()
            && let Some(pending) = t.pending.as_mut()
        {
            pending.mode = mode;
            if mode == TextMode::Frame && pending.w <= 0.0 {
                pending.w = FRAME_W;
            }
            let laid = Laid::of(pending, fonts);
            refit(pending, &laid);
            t.laid = laid;
            return Change::Selection;
        }
        let targets = self.targets(doc);
        if targets.is_empty() {
            self.text_mode = mode;
            return Change::Selection;
        }
        let mut changed = false;
        for id in &targets {
            let Some(text) = text_mut(doc, id) else { continue };
            if text.mode == mode {
                continue;
            }
            changed = true;
            text.mode = mode;
            let laid = Laid::of(text, fonts);
            refit(text, &laid);
            if let Some(t) = self.typing.as_mut().filter(|t| &t.id == id) {
                t.laid = laid;
            }
        }
        if changed { Change::Scene } else { Change::Selection }
    }

    /// What the text being typed shows over the board: its box, what is
    /// selected in it, the caret when `caret` says it is on, and the mark
    /// of a frame too small for what it holds.
    pub fn typing_prims(&self, doc: &Document, view: &View, theme: &Theme, caret: bool) -> Vec<Prim> {
        let (Some(t), Some(text)) = (self.typing.as_ref(), self.typed(doc)) else {
            return Vec::new();
        };
        let k = view.px_per_world();
        let s = view.scale as f32;
        let (sx, sy) = view.world_to_screen(text.x, text.y);
        let pivot = ((sx + text.w * k / 2.0) as f32, (sy + text.h * k / 2.0) as f32);
        let angle = text.rotation.to_radians() as f32;
        let rect = |x: f64, y: f64, w: f64, h: f64| ScreenRect {
            x: (sx + x * k) as f32,
            y: (sy + y * k) as f32,
            w: (w * k) as f32,
            h: (h * k) as f32,
        };
        let mut out = Vec::new();
        // The box, a hairline in the selection's colour: what the text
        // wraps inside, or how far artistic text runs.
        let hair = s.max(1.0);
        let b = rect(0.0, 0.0, text.w, text.h);
        for edge in [
            ScreenRect { h: hair, ..b },
            ScreenRect { y: b.y + b.h - hair, h: hair, ..b },
            ScreenRect { w: hair, ..b },
            ScreenRect { x: b.x + b.w - hair, w: hair, ..b },
        ] {
            out.push(Prim::turned(edge, pivot, angle, with_alpha(theme.selection, 0.6)));
        }
        let laid = &t.laid;
        if let Some((a, z)) = t.field.selection() {
            let space = laid.line_h * 0.25;
            for (x, y, w, h) in laid.bands(a, z, space) {
                out.push(Prim::turned(rect(x, y, w, h), pivot, angle, with_alpha(theme.selection, 0.3)));
            }
        }
        if laid.overflows() {
            let side = OVERFLOW_PX * s;
            let mark = ScreenRect {
                x: b.x + b.w - side / 2.0,
                y: b.y + b.h - side / 2.0,
                w: side,
                h: side,
            };
            out.push(Prim::turned(mark, pivot, angle, OVERFLOW));
        }
        if caret {
            let (x, top, bottom) = laid.caret(t.field.caret());
            let mut r = rect(x, top, 0.0, bottom - top);
            r.w = CARET_W * s;
            r.x -= r.w / 2.0;
            out.push(Prim::turned(r, pivot, angle, crate::scene::parse_color(&text.style.color)));
        }
        out
    }
}

/// Names a text layer after what its text now says, when it was named
/// after what the text said before — or was still the name it was born
/// with. A name somebody gave it by hand is theirs.
fn rename_after(doc: &mut Document, layer: &str, was: &str, now: &str) {
    let Some(l) = doc.layer_mut(layer) else { return };
    let born = l
        .name
        .strip_prefix("Text ")
        .is_some_and(|n| n.parse::<u32>().is_ok());
    if (born || name_of(was).as_deref() == Some(l.name.as_str()))
        && let Some(name) = name_of(now)
    {
        l.name = name;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Button, Tool};
    use super::*;
    use crate::doc::{Align, DEFAULT_FONT, Layer};
    use crate::scene::Viewport;

    const INK: &str = "#112233";

    fn view() -> View {
        View {
            camera: crate::doc::Camera {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
            viewport: Viewport { w: 800, h: 600 },
            scale: 1.0,
        }
    }

    /// Screen px for world `(x, y)` in [`view`].
    fn at(x: f64, y: f64) -> (f64, f64) {
        view().world_to_screen(x, y)
    }

    fn fonts() -> Fonts {
        Fonts::bundled()
    }

    fn texting() -> Editor {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Text, &mut doc);
        e
    }

    fn click(e: &mut Editor, doc: &mut Document, x: f64, y: f64) -> Change {
        clicks(e, doc, x, y, 1)
    }

    fn clicks(e: &mut Editor, doc: &mut Document, x: f64, y: f64, n: u32) -> Change {
        let f = fonts();
        let a = e.text_press(&view(), at(x, y), doc, &f, n, INK);
        let b = e.release(Button::Left, &view(), at(x, y), doc, INK);
        both(a, b)
    }

    fn drag(e: &mut Editor, doc: &mut Document, from: (f64, f64), to: (f64, f64)) -> Change {
        let f = fonts();
        let a = e.text_press(&view(), at(from.0, from.1), doc, &f, 1, INK);
        let _ = e.moved(&view(), at((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0), doc);
        let _ = e.moved(&view(), at(to.0, to.1), doc);
        let b = e.release(Button::Left, &view(), at(to.0, to.1), doc, INK);
        both(a, b)
    }

    fn typed(e: &mut Editor, doc: &mut Document, s: &str) -> Change {
        e.type_str(s, doc, &fonts())
    }

    fn key(e: &mut Editor, doc: &mut Document, k: TextKey) -> Change {
        e.text_key(k, doc, &fonts())
    }

    fn texts(doc: &Document) -> Vec<&Text> {
        doc.elements
            .iter()
            .filter_map(|el| match el {
                Element::Text(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn only(doc: &Document) -> &Text {
        let all = texts(doc);
        assert_eq!(all.len(), 1, "{all:?}");
        all[0]
    }

    fn value(e: &Editor) -> &str {
        e.typing().expect("typing").field.value()
    }

    /// A board with one artistic text saying `s` at the origin, typed
    /// there and left.
    fn with_text(s: &str) -> (Editor, Document) {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, s);
        let _ = e.end_typing(&mut doc);
        (e, doc)
    }

    #[test]
    fn the_text_tool_answers_to_t_and_t_again_switches_its_kind() {
        assert_eq!(Tool::from_hotkey('t'), Some(Tool::Text));
        assert!(Tool::ALL.contains(&Tool::Text));
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.choose_tool(Tool::Text, &mut doc);
        assert_eq!((e.tool(), e.text_mode), (Tool::Text, TextMode::Artistic));
        e.choose_tool(Tool::Text, &mut doc);
        assert_eq!((e.tool(), e.text_mode), (Tool::Text, TextMode::Frame));
        e.choose_tool(Tool::Select, &mut doc);
        e.choose_tool(Tool::Text, &mut doc);
        assert_eq!(e.text_mode, TextMode::Frame, "taking it up again keeps the kind");
        e.choose_tool(Tool::Text, &mut doc);
        assert_eq!(e.text_mode, TextMode::Artistic);
    }

    #[test]
    fn a_click_opens_a_text_and_nothing_lands_until_it_is_typed_into() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let layers = doc.layers.len();
        let _ = click(&mut e, &mut doc, 10.0, 20.0);
        let t = e.typing().expect("a text to type into");
        assert!(t.pending.is_some());
        assert_eq!(doc.layers.len(), layers, "no layer yet");
        assert!(texts(&doc).is_empty());

        assert_eq!(typed(&mut e, &mut doc, "Hi"), Change::Scene);
        let t = only(&doc);
        assert_eq!(t.text, "Hi");
        assert_eq!(t.mode, TextMode::Artistic);
        assert_eq!(t.style.color, INK);
        assert_eq!(t.style.font, DEFAULT_FONT);
        assert!(t.w > 0.0 && t.h > 0.0);
        // The click is where the first line's middle stands.
        assert_eq!(t.x, 10.0);
        assert!((t.y + t.h / 2.0 - 20.0).abs() < 1e-9);
        let layer = doc.layer(&t.layer).expect("its layer");
        assert_eq!(layer.kind, Kind::Text);
        assert_eq!(e.active(&doc), t.layer, "the new layer is the active one");
    }

    #[test]
    fn a_text_left_empty_leaves_nothing_behind() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let before = doc.clone();
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = e.end_typing(&mut doc);
        assert!(e.typing().is_none());
        assert_eq!(doc, before);

        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "x");
        let _ = key(&mut e, &mut doc, TextKey::Backspace);
        assert_eq!(e.end_typing(&mut doc), Change::Scene);
        assert!(texts(&doc).is_empty());
        assert!(doc.same_board(&before), "its layer went with it");
    }

    #[test]
    fn leaving_a_text_selects_it_and_names_its_layer_after_what_it_says() {
        let (e, doc) = with_text("Hello there\nsecond line");
        let t = only(&doc);
        assert_eq!(e.selection(), std::slice::from_ref(&t.id));
        assert_eq!(doc.layer(&t.layer).unwrap().name, "Hello there");
        assert_eq!(name_of("   \n  a long title that goes on and on and on past forty"), Some("a long title that goes on and on and on …".into()));
        assert_eq!(name_of(" \n "), None);
    }

    #[test]
    fn a_name_given_by_hand_is_kept_whatever_is_typed() {
        let (mut e, mut doc) = with_text("draft");
        let layer = only(&doc).layer.clone();
        let _ = e.rename_layer(&mut doc, &layer, "Title");
        let id = only(&doc).id.clone();
        let _ = e.edit_text(&id, &mut doc, &fonts(), None);
        let _ = typed(&mut e, &mut doc, "final");
        let _ = e.end_typing(&mut doc);
        assert_eq!(doc.layer(&layer).unwrap().name, "Title");
        assert_eq!(only(&doc).text, "final", "Enter's select-all was typed over");
    }

    #[test]
    fn a_drag_with_artistic_text_sets_its_size() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (30.0, 60.0));
        let _ = typed(&mut e, &mut doc, "Big");
        let t = only(&doc);
        assert_eq!(t.style.size, 60.0);
        assert_eq!((t.x, t.y), (0.0, 0.0));
        assert_eq!(t.mode, TextMode::Artistic);
    }

    #[test]
    fn a_drag_with_frame_text_makes_a_frame_of_that_area() {
        let mut e = texting();
        let mut doc = Document::new("t");
        e.choose_tool(Tool::Text, &mut doc);
        let _ = drag(&mut e, &mut doc, (100.0, 80.0), (20.0, 10.0));
        let _ = typed(&mut e, &mut doc, "words that wrap in the frame");
        let t = only(&doc);
        assert_eq!(t.mode, TextMode::Frame);
        assert_eq!((t.x, t.y, t.w, t.h), (20.0, 10.0, 80.0, 70.0));
    }

    #[test]
    fn a_click_with_frame_text_makes_a_frame_to_type_in() {
        let mut e = texting();
        let mut doc = Document::new("t");
        e.choose_tool(Tool::Text, &mut doc);
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "a");
        let t = only(&doc);
        assert_eq!(t.mode, TextMode::Frame);
        assert_eq!(t.w, FRAME_W);
        assert!((t.h - FRAME_LINES * t.style.size * t.style.leading).abs() < 1e-9);
    }

    #[test]
    fn keys_walk_and_edit_the_text() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "one two");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::WordLeft, false));
        assert_eq!(e.typing().unwrap().field.caret(), 4);
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Right, true));
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Right, true));
        assert_eq!(e.typing().unwrap().field.selection(), Some((4, 6)));
        let _ = typed(&mut e, &mut doc, "T");
        assert_eq!(value(&e), "one To");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Home, false));
        let _ = key(&mut e, &mut doc, TextKey::Delete);
        assert_eq!(value(&e), "ne To");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::End, false));
        let _ = key(&mut e, &mut doc, TextKey::Backspace);
        let _ = key(&mut e, &mut doc, TextKey::Newline);
        let _ = typed(&mut e, &mut doc, "next");
        assert_eq!(value(&e), "ne T\nnext");
        let _ = key(&mut e, &mut doc, TextKey::WordBackspace);
        assert_eq!(value(&e), "ne T\n");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Home, false));
        let _ = key(&mut e, &mut doc, TextKey::WordDelete);
        assert_eq!(value(&e), " T\n");
        let _ = key(&mut e, &mut doc, TextKey::SelectAll);
        assert_eq!(e.typing().unwrap().field.selection(), Some((0, 3)));
        assert_eq!(only(&doc).text, " T\n", "the board has it all along");
    }

    #[test]
    fn up_and_down_keep_to_the_column_they_started_in() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "abcdef\nab\nabcdef");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Home, false));
        for _ in 0..4 {
            let _ = key(&mut e, &mut doc, TextKey::Go(Move::Right, false));
        }
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Down, false));
        assert_eq!(e.typing().unwrap().field.caret(), 9, "the short line's end");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::Down, false));
        assert_eq!(e.typing().unwrap().field.caret(), 14, "back at the column it left");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::LineHome, true));
        assert_eq!(e.typing().unwrap().field.selection(), Some((10, 14)));
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::LineEnd, false));
        assert_eq!(e.typing().unwrap().field.caret(), 16);
    }

    #[test]
    fn undo_while_typing_takes_back_a_word_at_a_time() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        for c in "hello world".chars() {
            let _ = typed(&mut e, &mut doc, &c.to_string());
        }
        let _ = key(&mut e, &mut doc, TextKey::Undo);
        assert_eq!(value(&e), "hello ");
        assert_eq!(only(&doc).text, "hello ");
        let _ = key(&mut e, &mut doc, TextKey::Undo);
        assert_eq!(value(&e), "");
        let _ = key(&mut e, &mut doc, TextKey::Redo);
        assert_eq!(value(&e), "hello ");
        let _ = key(&mut e, &mut doc, TextKey::Backspace);
        let _ = key(&mut e, &mut doc, TextKey::Backspace);
        let _ = key(&mut e, &mut doc, TextKey::Undo);
        assert_eq!(value(&e), "hello ", "a run of deletes is one step");
        let _ = key(&mut e, &mut doc, TextKey::Redo);
        assert_eq!(value(&e), "hell");
    }

    #[test]
    fn the_clipboard_takes_and_gives_text() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "cut me");
        assert_eq!(e.copied_text(), None, "nothing selected, nothing copied");
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::WordLeft, true));
        assert_eq!(e.copied_text().as_deref(), Some("me"));
        let (cut, change) = e.cut_text(&mut doc, &fonts());
        assert_eq!((cut.as_deref(), change), (Some("me"), Change::Scene));
        assert_eq!(value(&e), "cut ");
        let _ = e.paste_text("a\r\nb\tc\u{1b}", &mut doc, &fonts());
        assert_eq!(value(&e), "cut a\nb    c");
    }

    #[test]
    fn artistic_text_grows_away_from_where_it_is_anchored() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 50.0, 50.0);
        let _ = typed(&mut e, &mut doc, "a");
        let (x0, w0) = (only(&doc).x, only(&doc).w);
        let _ = typed(&mut e, &mut doc, "bcd");
        let t = only(&doc);
        assert_eq!(t.x, x0, "left: the left edge stays");
        assert!(t.w > w0);
        let _ = e.restyle(&mut doc, &fonts(), |s| s.align = Align::Right);
        let right = only(&doc).x + only(&doc).w;
        let _ = typed(&mut e, &mut doc, "efg");
        let t = only(&doc).clone();
        assert!((t.x + t.w - right).abs() < 1e-9, "right: the right edge stays");
        let _ = typed(&mut e, &mut doc, "\nnext line");
        assert!(only(&doc).h > t.h, "a line more is taller");
    }

    #[test]
    fn a_turned_text_keeps_its_anchor_where_it_was() {
        let (mut e, mut doc) = with_text("a");
        let id = only(&doc).id.clone();
        if let Some(t) = text_mut(&mut doc, &id) {
            t.rotation = 90.0;
        }
        let corner = |t: &Text| frame_of(t).to_world([-t.w / 2.0, -t.h / 2.0]);
        let before = corner(only(&doc));
        let _ = e.edit_text(&id, &mut doc, &fonts(), None);
        let _ = key(&mut e, &mut doc, TextKey::Go(Move::End, false));
        let _ = typed(&mut e, &mut doc, "longer and longer");
        let after = corner(only(&doc));
        assert!((after[0] - before[0]).abs() < 1e-9 && (after[1] - before[1]).abs() < 1e-9, "{before:?} {after:?}");
    }

    #[test]
    fn a_press_on_a_text_types_into_it_where_it_landed() {
        let (mut e, mut doc) = with_text("hello world");
        let t = only(&doc).clone();
        let laid = Laid::of(&t, &fonts());
        let (x, top, _) = laid.caret(6);
        let _ = click(&mut e, &mut doc, t.x + x, t.y + top + 2.0);
        let typing = e.typing().expect("typing into it");
        assert_eq!(typing.id, t.id);
        assert_eq!(typing.field.caret(), 6);
        assert!(e.selection().is_empty(), "no handles while it is typed into");
        let _ = typed(&mut e, &mut doc, "big ");
        assert_eq!(only(&doc).text, "hello big world");
    }

    #[test]
    fn a_double_click_takes_the_word_and_a_triple_the_paragraph() {
        let (mut e, mut doc) = with_text("one two\nthree");
        let t = only(&doc).clone();
        let laid = Laid::of(&t, &fonts());
        let (x, top, _) = laid.caret(5);
        let (px, py) = (t.x + x + 1.0, t.y + top + 2.0);
        let _ = click(&mut e, &mut doc, px, py);
        let _ = clicks(&mut e, &mut doc, px, py, 2);
        assert_eq!(e.typing().unwrap().field.selection(), Some((4, 7)));
        let _ = clicks(&mut e, &mut doc, px, py, 3);
        assert_eq!(e.typing().unwrap().field.selection(), Some((0, 7)));
    }

    #[test]
    fn a_drag_through_the_text_selects_what_it_passes() {
        let (mut e, mut doc) = with_text("select this much");
        let t = only(&doc).clone();
        let laid = Laid::of(&t, &fonts());
        let y = t.y + laid.rows[0].top + 3.0;
        let from = (t.x + laid.caret(7).0, y);
        let to = (t.x + laid.caret(11).0, y);
        let _ = click(&mut e, &mut doc, from.0, from.1);
        let _ = drag(&mut e, &mut doc, from, to);
        assert_eq!(e.typing().unwrap().field.selection(), Some((7, 11)));
        assert!(!e.busy(), "selecting is not a change in progress");
    }

    #[test]
    fn a_press_away_from_the_text_leaves_it_and_starts_another() {
        let (mut e, mut doc) = with_text("first");
        let first = only(&doc).id.clone();
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        assert_eq!(e.typing().unwrap().id, first);
        let _ = click(&mut e, &mut doc, 300.0, 300.0);
        let t = e.typing().expect("a new one");
        assert_ne!(t.id, first);
        assert!(t.pending.is_some());
        let _ = typed(&mut e, &mut doc, "second");
        assert_eq!(texts(&doc).len(), 2);
    }

    #[test]
    fn a_locked_text_is_not_typed_into() {
        let (mut e, mut doc) = with_text("locked");
        let layer = only(&doc).layer.clone();
        if let Some(l) = doc.layer_mut(&layer) {
            l.locked = true;
        }
        let _ = click(&mut e, &mut doc, 2.0, 2.0);
        let t = e.typing().expect("a new text instead");
        assert!(t.pending.is_some());
    }

    #[test]
    fn a_text_started_in_a_frame_lands_in_its_stack() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Frame, &mut doc);
        let _ = e.press(Button::Left, &view(), at(0.0, 0.0), &mut doc, &crate::brush::Tip::PENCIL);
        let _ = e.moved(&view(), at(200.0, 200.0), &mut doc);
        let _ = e.release(Button::Left, &view(), at(200.0, 200.0), &mut doc, INK);
        let frame_layer = doc.layers.last().unwrap().id.clone();
        e.set_tool(Tool::Text, &mut doc);
        let _ = click(&mut e, &mut doc, 50.0, 50.0);
        let _ = typed(&mut e, &mut doc, "inside");
        let t = only(&doc);
        assert_eq!(doc.context(&t.layer), Some(frame_layer.as_str()));
    }

    #[test]
    fn every_session_is_its_own_and_its_ending_folds_into_it() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let first = e.fold().expect("a session");
        let _ = typed(&mut e, &mut doc, "a");
        let _ = e.end_typing(&mut doc);
        assert_eq!(e.fold(), Some(first), "leaving is the session's last change");
        e.let_go_of_fold();
        assert_eq!(e.fold(), None);
        let _ = click(&mut e, &mut doc, 200.0, 0.0);
        assert_ne!(e.fold(), Some(first));
    }

    #[test]
    fn the_bar_looks_at_the_text_typed_then_the_one_selected_then_the_next() {
        let (mut e, mut doc) = with_text("styled");
        assert!(e.text_targeted(&doc));
        assert_eq!(e.restyle(&mut doc, &fonts(), |s| s.bold = true), Change::Scene);
        assert!(only(&doc).style.bold);
        assert!(!e.text_style(&Document::new("x")).bold, "the next text is not bold");
        let w = only(&doc).w;
        let _ = e.restyle(&mut doc, &fonts(), |s| s.size = 48.0);
        assert!(only(&doc).w > w * 1.5, "artistic text fits its new size");
        e.escape(&mut doc);
        assert!(!e.text_targeted(&doc));
        assert_eq!(e.restyle(&mut doc, &fonts(), |s| s.italic = true), Change::Selection);
        assert!(e.text_style(&doc).italic);
        assert!(!only(&doc).style.italic);
    }

    #[test]
    fn the_bar_turns_a_text_into_the_other_kind() {
        let (mut e, mut doc) = with_text("one two three");
        let w = only(&doc).w;
        let _ = e.set_text_kind(TextMode::Frame, &mut doc, &fonts());
        assert_eq!(only(&doc).mode, TextMode::Frame);
        let id = only(&doc).id.clone();
        if let Some(t) = text_mut(&mut doc, &id) {
            t.w = 10.0;
        }
        let _ = e.set_text_kind(TextMode::Artistic, &mut doc, &fonts());
        let t = only(&doc);
        assert_eq!(t.mode, TextMode::Artistic);
        assert!((t.w - w).abs() < 1e-9, "one line again, as wide as it measures");
        assert_eq!(e.text_mode, TextMode::Artistic, "the tool's own kind is left alone");
    }

    #[test]
    fn a_size_set_from_the_bar_stays_a_size_a_text_may_have() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = e.restyle(&mut doc, &fonts(), |s| s.size = 1e9);
        assert_eq!(e.text_style(&doc).size, MAX_TEXT_SIZE);
        let _ = e.restyle(&mut doc, &fonts(), |s| s.size = 0.0);
        assert_eq!(e.text_style(&doc).size, MIN_TEXT_SIZE);
    }

    #[test]
    fn enter_on_a_selected_text_types_into_all_of_it() {
        let (mut e, mut doc) = with_text("replace me");
        assert_eq!(e.edit_selected_text(&mut doc, &fonts()), Change::Selection);
        assert_eq!(e.typing().unwrap().field.selection(), Some((0, 10)));
        let mut other = Editor::new();
        assert_eq!(other.edit_selected_text(&mut doc, &fonts()), Change::None, "nothing selected");
    }

    #[test]
    fn switching_tools_leaves_the_text() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "kept");
        e.set_tool(Tool::Select, &mut doc);
        assert!(e.typing().is_none());
        assert_eq!(only(&doc).text, "kept");
    }

    #[test]
    fn what_is_being_typed_shows_its_box_its_selection_and_its_caret() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let theme = Theme::light();
        assert!(e.typing_prims(&doc, &view(), &theme, true).is_empty());
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let pending = e.typing_prims(&doc, &view(), &theme, true);
        assert!(!pending.is_empty(), "the caret of a text not yet typed");
        let _ = typed(&mut e, &mut doc, "ab");
        let caret_on = e.typing_prims(&doc, &view(), &theme, true).len();
        let caret_off = e.typing_prims(&doc, &view(), &theme, false).len();
        assert_eq!(caret_on, caret_off + 1);
        let _ = key(&mut e, &mut doc, TextKey::SelectAll);
        assert!(e.typing_prims(&doc, &view(), &theme, false).len() > caret_off);
    }

    #[test]
    fn a_frame_too_small_for_its_text_wears_the_mark() {
        let mut e = texting();
        let mut doc = Document::new("t");
        e.choose_tool(Tool::Text, &mut doc);
        let _ = drag(&mut e, &mut doc, (0.0, 0.0), (60.0, 30.0));
        let theme = Theme::light();
        let _ = typed(&mut e, &mut doc, "a");
        let fits = e.typing_prims(&doc, &view(), &theme, false);
        assert!(fits.iter().all(|p| p.color != OVERFLOW));
        let _ = typed(&mut e, &mut doc, "\nb\nc\nd");
        let over = e.typing_prims(&doc, &view(), &theme, false);
        assert!(over.iter().any(|p| p.color == OVERFLOW));
    }

    #[test]
    fn a_layer_for_a_new_text_is_a_text_layer_named_for_it_while_it_is_typed() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "Live name");
        let t = only(&doc);
        let layer: &Layer = doc.layer(&t.layer).unwrap();
        assert_eq!(layer.name, "Live name");
    }

    #[test]
    fn a_size_steps_through_the_sizes_type_is_set_in() {
        assert_eq!(step_size(12.0, true), 14.0);
        assert_eq!(step_size(13.0, true), 14.0, "from between two, to the next");
        assert_eq!(step_size(14.0, false), 12.0);
        assert_eq!(step_size(13.0, false), 12.0);
        assert_eq!(step_size(400.0, true), 500.0, "past the list, a quarter again");
        assert_eq!(step_size(4.0, false), 3.2, "below the list, a quarter less");
        assert!(step_size(MAX_TEXT_SIZE, true) <= MAX_TEXT_SIZE);
        assert!(step_size(MIN_TEXT_SIZE, false) >= MIN_TEXT_SIZE);
    }

    #[test]
    fn a_family_tried_from_the_menu_is_put_back_as_it_was() {
        let (mut e, mut doc) = with_text("try me");
        let was = e.text_fonts(&doc);
        assert_eq!(was, [(only(&doc).id.clone(), DEFAULT_FONT.to_owned())]);
        let w = only(&doc).w;
        let _ = e.restyle(&mut doc, &fonts(), |s| s.font = "Elsewhere".into());
        let _ = e.restyle(&mut doc, &fonts(), |s| s.size = 60.0);
        e.put_fonts_back(&mut doc, &fonts(), &was);
        assert_eq!(only(&doc).style.font, DEFAULT_FONT);
        assert!(only(&doc).w > w, "only the family goes back");
    }

    #[test]
    fn with_another_tool_a_press_on_another_text_only_leaves_the_one_typed() {
        let (mut e, mut doc) = with_text("first");
        let _ = click(&mut e, &mut doc, 300.0, 0.0);
        let _ = typed(&mut e, &mut doc, "second");
        let _ = e.end_typing(&mut doc);
        let first = texts(&doc)[0].clone();
        e.set_tool(Tool::Select, &mut doc);
        let second = texts(&doc)[1].id.clone();
        let _ = e.edit_text(&second, &mut doc, &fonts(), None);
        let f = fonts();
        let _ = e.text_press(&view(), at(first.x + 2.0, first.y + 2.0), &mut doc, &f, 1, INK);
        assert!(e.typing().is_none(), "a single press with the Select tool types into nothing");
        let _ = e.text_press(&view(), at(first.x + 2.0, first.y + 2.0), &mut doc, &f, 2, INK);
        assert_eq!(e.typing().map(|t| t.id.clone()), Some(first.id), "a double click does");
    }

    #[test]
    fn a_text_whose_box_came_from_elsewhere_is_fitted_to_its_letters() {
        let (_, mut doc) = with_text("fit me");
        let id = only(&doc).id.clone();
        let good = only(&doc).clone();
        if let Some(t) = text_mut(&mut doc, &id) {
            t.w = 5.0;
            t.h = 400.0;
        }
        assert!(fit_texts(&mut doc, &fonts()));
        let t = only(&doc);
        assert!((t.w - good.w).abs() < 1e-9 && (t.h - good.h).abs() < 1e-9);
        assert!((t.x - good.x).abs() < 1e-9 && (t.y - good.y).abs() < 1e-9, "from its anchor");
        assert!(!fit_texts(&mut doc, &fonts()), "a fitted text stays where it is");
    }

    #[test]
    fn a_text_placed_from_outside_lands_on_a_layer_of_its_own_fitted_and_named() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        let text = Text {
            id: "ignored".into(),
            layer: String::new(),
            x: 10.0,
            y: 20.0,
            w: 1.0,
            h: 1.0,
            rotation: 0.0,
            mode: TextMode::Artistic,
            text: "From the command line".into(),
            style: TextStyle::default(),
        };
        let (id, layer) = e.place_text(&mut doc, &fonts(), text, None);
        let t = only(&doc);
        assert_eq!((t.id.as_str(), t.layer.as_str()), (id.as_str(), layer.as_str()));
        assert_ne!(t.id, "ignored", "an id is minted, never trusted");
        assert!(t.w > 100.0, "fitted to what it says");
        assert_eq!((t.x, t.y), (10.0, 20.0));
        assert_eq!(doc.layer(&layer).unwrap().name, "From the command line");
        assert_eq!(doc.layer(&layer).unwrap().kind, Kind::Text);
    }

    #[test]
    fn a_text_changed_from_outside_is_fitted_and_its_layer_renamed_with_it() {
        let (mut e, mut doc) = with_text("before");
        let id = only(&doc).id.clone();
        let w = only(&doc).w;
        let change = e.change_text(&mut doc, &fonts(), &id, |t| {
            t.text = "after, and longer".into();
            t.style.bold = true;
        });
        assert_eq!(change, Ok(Change::Scene));
        let t = only(&doc);
        assert!(t.w > w);
        assert_eq!(doc.layer(&t.layer).unwrap().name, "after, and longer");
        assert!(e.change_text(&mut doc, &fonts(), "nobody", |_| {}).is_err());
        let layer = only(&doc).layer.clone();
        doc.layer_mut(&layer).unwrap().locked = true;
        let refused = e.change_text(&mut doc, &fonts(), &id, |t| t.text = "no".into());
        assert!(refused.unwrap_err().contains("locked"));
        assert_eq!(only(&doc).text, "after, and longer");
        doc.layer_mut(&layer).unwrap().locked = false;
        let bad = e.change_text(&mut doc, &fonts(), &id, |t| {
            t.text = "half".into();
            t.style.size = -1.0;
        });
        assert!(bad.is_err());
        assert_eq!(only(&doc).text, "after, and longer", "nothing of a refused change lands");
    }

    #[test]
    fn a_text_whose_frame_went_before_its_first_letter_lands_on_the_board() {
        let mut e = Editor::new();
        let mut doc = Document::new("t");
        e.set_tool(Tool::Frame, &mut doc);
        let _ = e.press(Button::Left, &view(), at(0.0, 0.0), &mut doc, &crate::brush::Tip::PENCIL);
        let _ = e.moved(&view(), at(200.0, 200.0), &mut doc);
        let _ = e.release(Button::Left, &view(), at(200.0, 200.0), &mut doc, INK);
        e.set_tool(Tool::Text, &mut doc);
        let _ = click(&mut e, &mut doc, 50.0, 50.0);
        // The frame is taken away — an undo, the command line — before
        // anything is typed.
        let frame_layer = doc.layers.last().unwrap().id.clone();
        let (stack, i) = doc.locate(&frame_layer).map(|(s, i)| (s.map(str::to_owned), i)).unwrap();
        doc.remove_layer(stack.as_deref(), i);
        let _ = typed(&mut e, &mut doc, "still here");
        let t = only(&doc);
        assert_eq!(doc.context(&t.layer), None, "on the board, where its frame was");
    }

    #[test]
    fn a_text_whose_layer_is_locked_mid_session_takes_no_more_typing() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "abc");
        let layer = only(&doc).layer.clone();
        doc.layer_mut(&layer).unwrap().locked = true;
        let _ = typed(&mut e, &mut doc, "d");
        assert_eq!(only(&doc).text, "abc", "a lock keeps what it holds");
        assert!(e.typing().is_none(), "and the session is over");
    }

    #[test]
    fn a_session_whose_text_went_is_over() {
        let mut e = texting();
        let mut doc = Document::new("t");
        let _ = click(&mut e, &mut doc, 0.0, 0.0);
        let _ = typed(&mut e, &mut doc, "abc");
        doc.elements.clear();
        assert_eq!(e.settle_typing(&mut doc), Change::Selection);
        assert!(e.typing().is_none(), "nothing left to type into holds the keyboard");
        let mut still = texting();
        let _ = click(&mut still, &mut doc, 0.0, 0.0);
        assert_eq!(still.settle_typing(&mut doc), Change::None, "a text not yet typed is not gone");
        assert!(still.typing().is_some());
    }

    #[test]
    fn a_press_says_whether_it_leaves_the_text_being_typed() {
        let (mut e, mut doc) = with_text("here");
        let t = only(&doc).clone();
        assert!(!e.leaves_text(&view(), at(t.x + 2.0, t.y + 2.0), &doc), "nothing is typed");
        let _ = e.edit_text(&t.id, &mut doc, &fonts(), None);
        assert!(!e.leaves_text(&view(), at(t.x + 2.0, t.y + 2.0), &doc));
        assert!(e.leaves_text(&view(), at(t.x + 300.0, t.y + 300.0), &doc));
    }
}
