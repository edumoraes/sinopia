//! Window lifecycle (ARCHITECTURE.md §11): winit + wgpu + socket.
//!
//! The IPC server runs on its own thread and injects requests into the
//! event loop through `EventLoopProxy`; its replies are immediate acks (the
//! state that actually changes, changes here, on the loop thread). Input is
//! routed to the pure `editor` and `dock`; this file only maps events and
//! assembles frames, so it stays thin and the logic stays testable.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use anyhow::Context as _;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::wayland::WindowAttributesExtWayland as _;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::agents;
use crate::bitmap::{self, Bitmap};
use crate::brush::{self, Library};
use crate::clipboard::{self, Clipboard, Paste};
use crate::dialogs::{self, Answer, Reply};
use crate::doc::{BlendMode, Document, Element, Image, Layer, Tag};
use crate::dock::{Dock, Hit};
use crate::editor::{
    Button, Change, Command, Editor, Gesture, Keeps, Pick, SCROLL_LINE_PX, Stylus, Tool, locked_among,
};
use crate::export;
use crate::merge;
use crate::thumbs;
use crate::field::{self, Field};
use crate::geom::Corner;
use crate::gestures;
use crate::history::History;
use crate::gfx::Gfx;
use crate::graft;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::layers::{self, Panel, PanelHit};
use crate::menu;
use crate::menubar::{self, Action, Bar, Title};
use crate::omarchy::{self, Style};
use crate::palette::{self, Palette};
use crate::slots::{self, Strip};
use crate::props::{self, Props};
use crate::project::{self, Origin, Project};
use crate::scene::{
    self, Frame, ImageSlots, Prim, Rgba, ScreenRect, Shapes, View, Viewport, with_alpha,
};
use crate::select::{self, Handle};
use crate::send;
use crate::skills;
use crate::store::{self, Store};
use crate::tablet::{self, Pen};
use crate::tabs::{self, TabHit, Tabs};
use crate::tree::Place;
use crate::text::{self, Atlas, Font};
use crate::textbar::{self, TextBar};
use crate::shapebar::{self, ShapeBar};
use crate::theme::{INKS, Theme};

/// The longest step the panel's easing takes in one frame. A window
/// that has been idle wakes with a huge gap since the last frame;
/// without this, whatever just started would be over before it drew.
const MAX_STEP: f32 = 0.05;

/// How much of the ink the brush's ring is drawn with.
const RING_ALPHA: f32 = 0.6;

/// How long a draft's changes wait before reaching disk. A stroke fires
/// `Change::Scene` on every sample of the hand, so saving on the change
/// itself would write through the middle of a gesture; waiting for the
/// hand to stop turns a stroke into one write. Short enough that what is
/// lost to a crash is the last breath of drawing, not the drawing.
const SAFETY_DELAY: std::time::Duration = std::time::Duration::from_millis(1200);

/// How close two presses on one card have to be to be a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// How far in from a frame's corner a text the command line puts in it
/// stands, when it is not told where, in world units.
const TEXT_INSET: f64 = 16.0;

/// How wide a text frame the command line makes is, when it is not told.
const TEXT_FRAME_W: f64 = 240.0;

/// How long the caret of a text being typed shows, and then hides.
const BLINK: std::time::Duration = std::time::Duration::from_millis(530);

/// How far apart, in logical px, two presses may land and still count
/// as a double click.
const CLICK_REACH: f64 = 4.0;

/// How long the socket thread waits for the loop to answer one of an
/// agent's three (§8). Generous, because the answer is real work — a
/// picture is rendered before the line goes back — and it is a ceiling
/// on a failure, never a cost on the ordinary path. The client waits
/// longer still, so an agent reads a denial rather than a timeout.
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// State the server thread reads (replies to `ping`).
struct SharedState {
    board_id: String,
}

#[derive(Debug)]
enum UserEvent {
    Request(Request),
    /// A request whose answer *is* the work — an agent's three (§8).
    /// The socket thread waits on the channel while the loop does it,
    /// because the live document and the GPU are both the loop's and
    /// neither can be read from the shared state the other ops are
    /// acked from.
    Ask(Request, mpsc::Sender<Event>),
    Gesture(Gesture),
    /// One step of the tablet's pen, straight off the protocol.
    Pen(Pen),
    /// A clipboard image, already decoded off the loop. The original
    /// bytes go to the blob store; the texels go to the GPU.
    Pasted {
        bytes: Vec<u8>,
        bitmap: Bitmap,
    },
    /// Clipboard text, for the field that asked for it.
    PastedText(String),
    /// A board's layers off the clipboard, already parsed off the loop —
    /// by the board's own parse, schema closed, since any client can put
    /// anything under any type.
    PastedLayers(Box<Document>),
    /// A portal dialog came back, however long the user took.
    Dialog(Reply),
}

/// What happens to a project once the dialog it is waiting on answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// Save it, and leave the tab where it is.
    Stay,
    /// Save it if asked to, then close its tab.
    Close,
    /// Close its tab and carry on closing the window.
    Quit,
}

/// The dialog on screen, and what it is for. Only one is ever up: two
/// portal windows asking about the same board would collect two answers
/// to one question.
#[derive(Debug)]
enum Pending {
    Open,
    SaveAs { then: Then },
    Confirm { then: Then },
}

/// One tab: a project and the editor driving it. Tool, selection and any
/// drag in progress belong to a document, not to the window, so switching
/// tabs must not hand them to the next one.
struct Open {
    project: Project,
    editor: Editor,
    /// What this tab can step back through. Per tab, because a board and
    /// the work done on it go together: switching tabs hands nothing on,
    /// exactly as the tool and the selection do not.
    history: History,
}

struct App {
    store: Store,
    /// Every open project, in tab order. Never empty: closing the last
    /// tab exits.
    open: Vec<Open>,
    active: usize,
    shared: Arc<Mutex<SharedState>>,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    theme: Theme,
    /// What the desktop says the window should look like. Kept beside
    /// the palette it derives because the face, the one text size and
    /// the chrome's own scale are read from it directly.
    style: Style,
    /// Where the desktop's theme is read from, so it can be read again.
    home: String,
    font: Font,
    /// Every brush there is, and which one `B` paints with. One library
    /// for the window, whichever tab is in front, as in Photoshop.
    brushes: Library,
    /// `Shift+L`, or the handle beside it: the layers panel is up.
    layers_shown: bool,
    /// `Shift+B`: the brush strip is up. It only shows with the brush
    /// in hand, so this is what shuts it without putting the brush down.
    /// The library is opened from inside the strip, so it goes too.
    palette_shown: bool,
    /// The strip's chevron: the library stands open beside it.
    library_shown: bool,
    /// How far down the shelf the library is looking, in physical px.
    palette_scroll: f32,
    /// Brush Properties' Advanced layout is dropped under the bar.
    props_open: bool,
    /// Which of the theme's inks the next stroke is laid in. The
    /// window's, like the brush and the tool: it belongs to the person
    /// drawing, and every board they open is drawn in it until they
    /// pick another.
    ink: usize,
    /// Whether the brush library has been changed since it was last
    /// written back. The brushes are the person's, not a board's: they
    /// are kept as soon as a gesture that changed them ends, and never
    /// asked about.
    brushes_dirty: bool,
    /// The properties bar's slider the pointer took, if any. It keeps the
    /// pointer until the button comes up, so a drag off the track still
    /// moves it — as every slider does.
    grab: Option<usize>,
    /// Where the dock's illustrated RGBA sheet was uploaded. `None`
    /// keeps its built-in line-art fallback alive until the upload lands.
    dock_icon_slot: Option<u32>,
    /// Where the brush icon sheet was uploaded, once it has been.
    icon_slot: u32,
    /// Where the agents' logos were uploaded, once they have been.
    agent_logo_slot: Option<u32>,
    /// Where each nib shape sits on the shape sheet, once it has been
    /// uploaded. Empty until then, and a stroke that names a shape lays
    /// a plain round nib meanwhile.
    shapes: Shapes,
    /// The faces the board's text is set in, and the sheet its letters
    /// are rasterized into — with the slot that sheet is uploaded to.
    fonts: crate::fonts::Fonts,
    glyphs: crate::glyphs::Glyphs,
    /// Whether the last frame drawn started the glyph sheet over.
    letters_restarted: std::cell::Cell<bool>,
    letters_slot: u32,
    /// The brush the palette last brought into sight. A change of hand
    /// glides the list to it; scrolling away from it does not snap back.
    shown_brush: Option<(usize, usize)>,
    /// The layer card the pointer picked up, if any. It outlives the
    /// release, easing back into the stack.
    carry: Option<Carry>,
    /// A press on a card that has not travelled far enough to lift it.
    pressed: Option<Press>,
    /// The layers bar's strength is being dragged: the pointer is its
    /// until the button comes up, and the board is not at rest.
    fading: bool,
    /// Whether the text bar has dropped the paragraph's settings, and the
    /// slider of it being dragged — the board is not at rest while one
    /// is, since every step of it restyles the text.
    text_bar_open: bool,
    text_grab: Option<textbar::Slider>,
    /// The shape bar's slider being dragged: the board is not at rest
    /// while one is, since every step of it restyles the shapes.
    shape_grab: Option<shapebar::Slider>,
    /// When the text being typed last heard from the hand: the caret
    /// blinks from there, and shows solid while the hand is at it.
    typed_at: Instant,
    /// Whether the last frame drew the caret: the loop redraws for the
    /// blink only when that is about to change.
    caret_drawn: std::cell::Cell<Option<bool>>,
    /// `Ctrl+V` on the board found no layers and no image, only words:
    /// what arrives is a text of its own, where the pointer is.
    pasting_words: bool,
    /// What has been typed into the font menu, and when last: a name is
    /// looked for as it is typed, and a pause starts it over.
    menu_typed: (String, Instant),
    /// The last press on the canvas, where, and how many in a row it
    /// made: two are a double click, three a triple.
    last_press: Option<(Instant, (f64, f64), u32)>,
    /// A menu standing over the window, and what it is for.
    menu: Option<Opened>,
    /// The panel's thumbnails as last taken, and the slot they are in;
    /// and whether what they show may have changed since.
    thumbs: Option<(thumbs::Sheet, u32)>,
    thumbs_stale: bool,
    /// A brush the pointer is carrying out of the library, if any. It
    /// does not outlive the release: there is nothing to settle.
    drag: Option<Dragging>,
    /// A card being renamed: its layer's id and the name being typed.
    /// It is the window's, not a tab's — like every other panel state.
    renaming: Option<(String, Field)>,
    /// The filter's name being typed: the field has the keyboard, and
    /// every edit narrows the tree as it is made.
    searching: Option<Field>,
    /// When the last press landed on a card, and on which. A second
    /// press on the same card inside [`DOUBLE_CLICK`] opens the rename.
    last_card: Option<(String, Instant)>,
    /// The send in progress: what is going, where it can go, which one
    /// is picked, and the two fields. It is the window's, like the tool
    /// and the ink.
    sending: Option<Sending>,
    /// The cards making room around a carried one.
    slides: layers::Slides,
    /// How far down the stack the panel is looking, in physical px, and
    /// the glide still bringing it there.
    scroll: f32,
    scrolling: layers::Coming,
    /// The active layer the panel last brought into sight. A change of
    /// active layer glides to show it; scrolling away from it does not
    /// snap back, because it has not changed.
    focused: Option<String>,
    /// When the panel was last eased, for everything on it that moves.
    clock: Instant,
    /// Built once the scale factor is known, rebuilt when it changes.
    atlas: Option<Atlas>,
    atlas_slot: u32,
    clipboard: Option<Clipboard>,
    /// The last layers copied, for a display with no clipboard to hold
    /// them: there, `Ctrl+V` pastes this.
    clip: Option<Document>,
    /// Where a portal dialog sends its answer. Absent before the window.
    dialog_sink: Option<dialogs::Sink>,
    pending: Option<Pending>,
    /// When the drafts owe the disk a safety save. Set by every change
    /// and pushed back by the next one, so a stroke lands once the hand
    /// stops rather than on every sample of it; `None` when nothing is
    /// owed. The loop waits until it rather than sleeping through it.
    owed: Option<Instant>,
    /// The window is closing, one dirty tab at a time.
    quitting: bool,
    /// Nothing is left to show: the loop ends at the next event boundary.
    closing: bool,
    /// Last pointer position in physical px, while inside the window.
    cursor: Option<(f64, f64)>,
    modifiers: Modifiers,
    cursor_icon: CursorIcon,
    /// Smoke-test mode: exit cleanly after N presented frames.
    smoke_frames_left: Option<u32>,
    exit_error: Option<anyhow::Error>,
}

/// A send being composed. It holds the agents as they were when the
/// panel opened: a list that changed under the person mid-sentence would
/// move the row they were about to press.
struct Sending {
    scope: export::Scope,
    agents: Vec<agents::Agent>,
    target: usize,
    /// The instruction, the dialog's one field.
    line: Field,
    /// How far down its lines the instruction is scrolled, in physical
    /// px: past twenty lines the box keeps its height and moves them.
    scroll: f32,
    /// A press in the box is being dragged, selecting as it goes.
    selecting: bool,
    /// The target's skills, read when it became the target.
    skills: Vec<skills::Skill>,
    /// Which of the skills answering the call being typed is picked.
    pick: usize,
    /// Where the call started whose menu `Esc` put away: it stays away
    /// for that call, and comes back for the next one.
    dismissed: Option<usize>,
    /// The shape of what leaves, known before its picture is taken.
    shape: Option<f32>,
    /// The picture of it at the dialog's foot: the slot it was uploaded
    /// to, and the size in px it was last taken at — or tried at, so a
    /// picture that will not render is not tried again every frame.
    picture: Option<u32>,
    pictured: Option<(u32, u32)>,
}

impl Sending {
    /// The call being typed in the instruction and the skills answering
    /// it, best first — when the target's harness takes skills, anything
    /// answers, and `Esc` has not put this call's menu away.
    fn menu(&self) -> Option<(skills::Token, Vec<usize>, skills::Call)> {
        let call = skills::harness(&self.agents.get(self.target)?.kind)?.call;
        let token = skills::token(self.line.value(), self.line.caret(), call)?;
        if self.dismissed == Some(token.start) {
            return None;
        }
        let matches = skills::matching(&self.skills, &token.query);
        (!matches.is_empty()).then_some((token, matches, call))
    }

    /// The target is another agent: its own skills, and a fresh menu.
    fn aim(&mut self, target: usize) {
        self.target = target;
        self.skills = self
            .agents
            .get(target)
            .map(|a| skills::list(&a.kind, &a.cwd))
            .unwrap_or_default();
        self.pick = 0;
        self.dismissed = None;
    }

    /// After an edit: a call no longer being typed lets go of the
    /// menu it put away, and the pick stays on a row that exists.
    fn settle(&mut self, edited: bool) {
        let typing_a_call = self.agents.get(self.target).is_some_and(|a| {
            skills::harness(&a.kind).is_some_and(|h| {
                skills::token(self.line.value(), self.line.caret(), h.call).is_some()
            })
        });
        if !typing_a_call {
            self.dismissed = None;
        }
        let n = self.menu().map_or(0, |(_, m, _)| m.len());
        self.pick = if edited || n == 0 {
            0
        } else {
            self.pick.min(n - 1)
        };
    }
}

/// A brush on its way from the library to a seat. The press may still
/// turn out to be a click, so nothing is carried until the pointer has
/// moved past the slop — and a click is what it was if it never does.
/// The brush is taken up at the press either way, as it always was: a
/// drag that ends nowhere leaves the hand where a click would have.
struct Dragging {
    at: (usize, usize),
    /// The brush's cell of the icon sheet, carried behind the pointer.
    icon: u16,
    /// Where the press landed, and where the pointer is now, in
    /// physical px.
    from: (f32, f32),
    to: (f32, f32),
    /// Past the slop: the icon is in the hand.
    carried: bool,
    /// The seat under the pointer, when it is one that can be written.
    over: Option<usize>,
}

/// How far a press has to travel before it is a drag and not a click,
/// in logical px — what it refuses is a hand that did not mean to drag.
const DRAG_SLOP: f64 = 4.0;

/// A layer card in the pointer's hand: which row it came from, where it
/// is being carried, and how far into the lift it is. It stays after the
/// button comes up, running the lift backwards until the card is a row
/// again.
struct Carry {
    /// The layer it is the card of.
    id: String,
    /// Between the press and the card's top edge: what it is held by.
    grab_dy: f32,
    /// Where the card's top edge is asked to be, in physical px.
    y: f32,
    /// The button is still down.
    held: bool,
    /// The lift, 0 to 1, walked toward `held` by the clock.
    t: f32,
    /// Where the card would land if let go of now: the row under the
    /// pointer says, and a place the picked layers may not go is none.
    aim: Option<Place>,
}

/// A menu standing over the window: what it is for, its lines, what it
/// was opened beside, the line under the pointer and how far down it is
/// scrolled.
struct Opened {
    purpose: Purpose,
    items: Vec<menu::Item>,
    at: ScreenRect,
    hover: Option<usize>,
    scroll: f32,
}

/// What a menu is for.
enum Purpose {
    /// The picked layers' blend mode: each line's mode, and what every
    /// picked layer had when the menu opened — put back unless a line is
    /// taken, since the lines are tried on the board as the pointer
    /// passes over them.
    Blend {
        modes: Vec<BlendMode>,
        was: Vec<(String, BlendMode)>,
    },
    /// The colour a layer is tagged with — and the picked with it, when
    /// it is one of them — and the tag on each line.
    Tag { id: String, tags: Vec<Tag> },
    /// A row's own menu: the row, and what each line does.
    Row { id: String, lines: Vec<layers::RowLine> },
    /// One of the application menu's: its title, and what each line does.
    Bar { title: Title, actions: Vec<Action> },
    /// The text bar's families: each line's, and what every text the bar
    /// is looking at was set in — put back unless a line is taken, since
    /// the lines are tried on the text as the pointer passes over them.
    Font { families: Vec<String>, was: crate::editor::Look },
    /// A shape bar's well — the stroke's or the fill's — the paint on
    /// each line, and what every shape the bar is looking at wore when it
    /// opened: put back unless a line is taken, since the lines are tried
    /// on the shapes as the pointer passes over them.
    Paint {
        stroke: bool,
        paints: Vec<Option<String>>,
        was: Vec<(String, Option<String>)>,
    },
    /// A line's head at one end, and the head on each line.
    Head { end: select::End, heads: Vec<crate::doc::Head> },
}

/// A press on a card that may yet be a drag. Nothing is lifted until the
/// pointer has travelled past the slop: a click picks, a second click
/// renames, and neither of them is a card leaving the tree.
struct Press {
    id: String,
    from: (f64, f64),
    /// Between the press and the card's top edge: what it is held by.
    grab_dy: f32,
}

impl Carry {
    fn lift(&self) -> layers::Lift {
        layers::Lift {
            id: self.id.clone(),
            y: self.y,
            t: self.t,
        }
    }
}

impl App {
    fn project(&self) -> &Project {
        &self.open[self.active].project
    }

    fn doc(&self) -> &Document {
        &self.open[self.active].project.doc
    }

    fn editor(&self) -> &Editor {
        &self.open[self.active].editor
    }

    /// The active tab's editor and its document, borrowed apart: the
    /// editor reshapes the document it is driving, so it cannot hold it.
    fn active(&mut self) -> (&mut Editor, &mut Document) {
        let open = &mut self.open[self.active];
        (&mut open.editor, &mut open.project.doc)
    }

    /// Writes tab `index` where its origin says. An untitled project has
    /// nowhere to go and is left alone — asking for a name is the caller's
    /// job.
    fn save_project(&mut self, index: usize) -> bool {
        let Some(origin) = self.open.get(index).map(|o| o.project.origin.clone()) else {
            return false;
        };
        self.save_project_at(index, origin)
    }

    /// Writes tab `index` to `origin`, and records it as the project's
    /// home only if the write lands. A Save As that fails must not leave
    /// a project claiming a file it never reached.
    fn save_project_at(&mut self, index: usize, origin: Origin) -> bool {
        if matches!(origin, Origin::Untitled) || index >= self.open.len() {
            return false;
        }
        // The preview goes first, so the entry the save writes names it.
        self.keep_preview(index);
        let Some(project) = self.open.get(index).map(|o| &o.project) else {
            return false;
        };
        let written = match &origin {
            // Saving a draft records it: the store is both its home and
            // the list it is remembered on.
            Origin::Board(_) => self.store.save(&project.doc),
            // A file is written where the user put it, and *then*
            // remembered. The recents keep the path, never a copy — the
            // list is a memory of projects, not a second store. Failing
            // to remember it is not failing to save it, so it is logged
            // and the save still counts.
            Origin::File(path) => store::save_document_to(path, &project.doc).inspect(|()| {
                if let Err(e) = self.store.remember_file(&project.doc, path) {
                    log::warn!("saved {path:?} but could not remember it: {e:#}");
                }
            }),
            Origin::Untitled => return false,
        };
        match written {
            Ok(()) => {
                self.open[index].project.saved(origin);
                if index == self.active {
                    self.retitle();
                }
                self.redraw();
                true
            }
            Err(e) => {
                log::error!("saving {}: {e:#}", self.open[index].project.label());
                false
            }
        }
    }

    /// `Ctrl+S`: writes the active project, asking for a name the first
    /// time.
    fn save_active(&mut self) {
        if self.project().needs_a_name() {
            self.ask_name(self.active, Then::Stay);
        } else {
            self.save_project(self.active);
        }
    }

    /// Puts up the Save As dialog for tab `index`.
    fn ask_name(&mut self, index: usize, then: Then) {
        if self.pending.is_some() || index >= self.open.len() {
            return;
        }
        let (Some(window), Some(sink)) = (self.window.clone(), self.dialog_sink.clone()) else {
            return;
        };
        let project = &self.open[index].project;
        let key = project.key().to_owned();
        let suggested = project.suggested_name();
        let at = match &project.origin {
            Origin::File(path) => Some(path.clone()),
            _ => None,
        };
        dialogs::save_as(&window, sink, key.clone(), &suggested, at.as_deref());
        self.pending = Some(Pending::SaveAs { then });
    }

    /// `Ctrl+O`: puts up the Open dialog.
    fn ask_open(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let (Some(window), Some(sink)) = (self.window.clone(), self.dialog_sink.clone()) else {
            return;
        };
        dialogs::open(&window, sink);
        self.pending = Some(Pending::Open);
    }

    /// Puts up the unsaved-work question for tab `index`.
    fn ask_about(&mut self, index: usize, then: Then) {
        if self.pending.is_some() || index >= self.open.len() {
            return;
        }
        let (Some(window), Some(sink)) = (self.window.clone(), self.dialog_sink.clone()) else {
            // With no window there is nothing to ask with, and refusing
            // to close is the only answer that loses nothing.
            return;
        };
        let project = &self.open[index].project;
        let key = project.key().to_owned();
        let label = project.label();
        dialogs::confirm_close(&window, sink, key.clone(), &label);
        self.pending = Some(Pending::Confirm { then });
    }

    /// A dialog answered. The project is found by key, never by index:
    /// tabs may have opened or closed while the user was deciding.
    fn dialog_replied(&mut self, reply: Reply) {
        let then = match self.pending.take() {
            Some(Pending::SaveAs { then } | Pending::Confirm { then }) => then,
            _ => Then::Stay,
        };
        match reply {
            Reply::Opened(paths) => self.opened(paths),
            Reply::SaveTo { key, path } => self.named(&key, path, then),
            Reply::Close { key, answer } => self.answered(&key, answer, then),
        }
    }

    fn opened(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if let Some(i) = self.tab_with_path(&path) {
                // Already open: a second tab would give one file two
                // documents, and the later save would win by accident.
                self.activate(i);
                continue;
            }
            match store::load_document_from(&path) {
                Ok(doc) => self.open_project(Project::opened(doc, Origin::File(path))),
                Err(e) => log::error!("opening {path:?}: {e:#}"),
            }
        }
    }

    /// A Save As came back.
    fn named(&mut self, key: &str, path: Option<PathBuf>, then: Then) {
        let Some(index) = self.tab_with_key(key) else {
            return;
        };
        let Some(path) = path else {
            // Naming cancelled: so is whatever the save was for.
            self.quitting = false;
            return;
        };
        if !self.save_project_at(index, Origin::File(path)) {
            self.quitting = false;
            return;
        }
        if then != Then::Stay {
            self.close(index);
            self.step_quit();
        }
    }

    /// The unsaved-work question came back.
    fn answered(&mut self, key: &str, answer: Answer, then: Then) {
        let Some(index) = self.tab_with_key(key) else {
            return;
        };
        match answer {
            Answer::Cancel => self.quitting = false,
            Answer::Discard => {
                self.close(index);
                self.step_quit();
            }
            Answer::Save if self.open[index].project.needs_a_name() => {
                self.ask_name(index, then);
            }
            Answer::Save if self.save_project(index) => {
                self.close(index);
                self.step_quit();
            }
            // The save failed and said so; closing now would lose exactly
            // what the question was about.
            Answer::Save => self.quitting = false,
        }
    }

    /// Nothing is in the middle of happening. A carried layer card is
    /// the one gesture the editor knows nothing about: it lives here and
    /// reorders the document on every pointer move while the editor sits
    /// perfectly still.
    fn settled(&self) -> bool {
        !self.editor().busy()
            && !self.carry.as_ref().is_some_and(|c| c.held)
            && !self.fading
            && self.text_grab.is_none()
            && self.shape_grab.is_none()
            && self.menu.is_none()
    }

    /// Writes down where a change left things.
    ///
    /// A state is only written at rest: a stroke changes the scene on
    /// every sample of the hand and a drag on every step, and neither is
    /// a state anybody meant to arrive at. The *spot* is written either
    /// way, because a press on an object selects it and starts dragging
    /// it in one motion — there is no settled moment between the two,
    /// and an undo that did not know the object had been picked up
    /// would put it back and drop it.
    fn remember(&mut self) {
        let settled = self.settled();
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        let spot = editor.at();
        match settled {
            true => {
                history.keep(&project.doc, spot, editor.fold());
                editor.let_go_of_fold();
            }
            false => history.mark(spot),
        }
    }

    /// `Ctrl+Z`: the board goes back to the state before the last
    /// change, and the hand back to where it was standing then.
    fn undo(&mut self) {
        self.drop_gesture();
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        let Some(entry) = history.undo() else { return };
        entry.restore(&mut project.doc, editor);
        self.thumbs_stale = true;
        self.stepped();
    }

    /// `Ctrl+Shift+Z`: forward again. One shortcut and not two — the
    /// undo key with Shift on it is what every drawing tool uses, and
    /// `Ctrl+Y` would be a second door onto the same room.
    fn redo(&mut self) {
        self.drop_gesture();
        let Open {
            project,
            editor,
            history,
        } = &mut self.open[self.active];
        let Some(entry) = history.redo() else { return };
        entry.restore(&mut project.doc, editor);
        self.thumbs_stale = true;
        self.stepped();
    }

    /// Drops whatever is in progress before a step is taken, exactly as
    /// `Esc` would. A gesture that has not finished is not a change to
    /// step behind — it is a change that has not happened — and a card
    /// still in the hand would be carrying a row the restored stack may
    /// not have.
    fn drop_gesture(&mut self) {
        let (editor, doc) = self.active();
        editor.cancel(doc);
        if let Some(carry) = &mut self.carry {
            carry.held = false;
        }
    }

    /// A step was taken. The board is not what is on disk any more,
    /// whichever way it moved.
    fn stepped(&mut self) {
        self.touch();
        self.redraw();
    }

    /// The document changed. A draft owes the disk a safety save; a file
    /// the user named owes nothing until `Ctrl+S` says so.
    fn touch(&mut self) {
        let was_clean = !self.open[self.active].project.dirty;
        self.open[self.active].project.touch();
        if was_clean {
            self.retitle();
        }
        self.owed = Some(Instant::now() + SAFETY_DELAY);
    }

    /// Takes the picture the recents show of tab `index` — the whole board
    /// as it shows, small — and hands it to the store, or takes the old
    /// one away when there is nothing on show. A preview is a courtesy to
    /// the list, never a condition of the save: one that fails is logged
    /// and the save goes on.
    fn keep_preview(&mut self, index: usize) {
        let Some(doc) = self.open.get(index).map(|o| o.project.doc.clone()) else {
            return;
        };
        let png = match export::preview(&doc) {
            None => None,
            Some((view, w, h)) => {
                match self.render_sub(&doc, &view, w, h).and_then(|rgba| bitmap::encode(&rgba, w, h)) {
                    Ok(png) => Some(png),
                    Err(e) => {
                        log::warn!("taking the preview of {:?}: {e:#}", doc.title);
                        return;
                    }
                }
            }
        };
        if let Err(e) = self.store.set_thumb(&doc.id, png.as_deref()) {
            log::warn!("keeping the preview of {:?}: {e:#}", doc.title);
        }
    }

    /// Writes every dirty draft where the store keeps it, and clears the
    /// debt. A file the user named is not touched: writing into it behind
    /// their back would empty `Ctrl+S`, the dot on the tab and the
    /// question at closing time of all their meaning.
    ///
    /// An untitled project is materialised here rather than at the `+`
    /// that made it — that is what "a draft earns its file the first time
    /// it is drawn on" means, and it is why an empty board never reaches
    /// the recents.
    fn keep_drafts(&mut self) {
        self.owed = None;
        for i in 0..self.open.len() {
            if !self.open[i].project.dirty {
                continue;
            }
            match &self.open[i].project.origin {
                Origin::File(_) => {}
                Origin::Board(_) => {
                    self.save_project(i);
                }
                Origin::Untitled => {
                    let id = self.open[i].project.doc.id.clone();
                    self.save_project_at(i, Origin::Board(id));
                }
            }
        }
    }

    /// Whether tab `index` holds work that only the user can decide about.
    /// A draft never does: it is already on disk.
    fn owes_an_answer(&self, index: usize) -> bool {
        match self.open.get(index) {
            Some(open) => open.project.dirty && matches!(open.project.origin, Origin::File(_)),
            None => false,
        }
    }

    /// Moves the panel on by the time since the last frame: the lift
    /// toward where the button says it should be, and the cards toward
    /// the rows the stack now gives them. A card that has settled all
    /// the way back into its row is forgotten.
    ///
    /// The step is capped, so a frame arriving after a long idle does
    /// not finish an animation before its first frame is seen.
    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.clock).as_secs_f32().min(MAX_STEP);
        self.clock = now;
        if let Some(carry) = &mut self.carry {
            let target = if carry.held { 1.0 } else { 0.0 };
            let step = dt / layers::LIFT_SECONDS;
            carry.t += (target - carry.t).clamp(-step, step);
            if !carry.held && carry.t <= 0.0 {
                self.carry = None;
            }
        }
        let scale = self.view().map_or(1.0, |v| v.scale as f32);
        let row = layers::ROW * scale;
        // Age what is already travelling before noticing what has just
        // set off, or a slide born this frame is a third over before its
        // first frame is drawn.
        self.slides.tick(dt);
        let Open {
            project, editor, ..
        } = &self.open[self.active];
        let rows = editor.rows(&project.doc);
        let ids: Vec<&str> = rows.iter().map(|r| r.layer.id.as_str()).collect();
        self.slides.restack(&ids, row);
        self.scrolling.tick(dt);
        // The window may have grown or shrunk under it: the panel is the
        // authority on how far the stack can be scrolled.
        if !self.scrolling.moving()
            && let Some(view) = self.view()
            && let Some(panel) = self.panel(&view)
        {
            self.scroll = panel.scroll();
        }
        if let Some(view) = self.view()
            && let Some(pal) = self.library(&view)
        {
            self.palette_scroll = pal.scroll();
        }
        self.follow_brush();
        self.follow_active();
    }

    /// Brings the active layer's card into the band when it has just
    /// become active — a click on the canvas picks an element, and the
    /// panel goes to where that element lives. Only on the change: the
    /// list stays where the wheel left it otherwise.
    fn follow_active(&mut self) {
        let Some(view) = self.view() else { return };
        let doc = self.doc();
        let active = self.editor().active(doc).to_owned();
        if self.focused.as_deref() == Some(active.as_str()) {
            return;
        }
        let rows = self.editor().rows(doc);
        let pos = rows.iter().position(|r| r.layer.id == active);
        let depth = rows.len();
        self.focused = Some(active);
        // A card in the hand takes the panel where the pointer says.
        let Some(panel) = self.panel(&view).filter(|_| self.carry.is_none()) else {
            return;
        };
        let Some(pos) = pos else { return };
        let want = panel.scroll_showing(pos, depth);
        if want != self.scroll {
            self.scrolling.send(self.scroll - want);
            self.scroll = want;
            self.redraw();
        }
    }

    /// Brings the brush in the hand into the palette's band when it
    /// changes. Edge-triggered on which brush it is, so picking one on
    /// another shelf shows it while scrolling away from it does not snap
    /// back.
    fn follow_brush(&mut self) {
        let held = self.brushes.selected();
        if self.shown_brush == Some(held) {
            return;
        }
        let Some(view) = self.view() else { return };
        let Some(pal) = self.library(&view) else { return };
        self.shown_brush = Some(held);
        let (set, index) = held;
        self.palette_scroll = pal.scroll_showing(self.brushes.sets(), set, index);
    }

    /// Moves the panel's list by `d` physical px, at once — the wheel
    /// answers under the hand, it does not glide.
    fn scroll_panel(&mut self, d: f64) {
        let Some(view) = self.view() else { return };
        let Some(panel) = self.panel(&view) else { return };
        let next = (self.scroll + d as f32).clamp(0.0, panel.max_scroll());
        if next != self.scroll || self.scrolling.moving() {
            self.scroll = next;
            self.scrolling = layers::Coming::default();
            self.redraw();
        }
    }

    /// Something on the panel is still moving, so the next frame will
    /// not match this one and has to be asked for.
    fn animating(&self) -> bool {
        self.carry.as_ref().is_some_and(|c| !c.held || c.t < 1.0)
            || self.slides.moving()
            || self.scrolling.moving()
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn retitle(&self) {
        let Some(w) = &self.window else { return };
        let project = self.project();
        let mark = if project.dirty { "• " } else { "" };
        w.set_title(&format!("{mark}Sinopia — {}", project.label()));
    }

    /// Closes tab `index`, asking about unsaved work first. A draft is
    /// kept on the way out rather than asked about: the question is for
    /// work that would otherwise be lost, and a draft's never is.
    fn request_close(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        // A text being typed is left first, as a press elsewhere leaves
        // it: one emptied goes, rather than being kept as nothing.
        if index == self.active {
            self.end_typing();
        }
        if self.owes_an_answer(index) {
            return self.ask_about(index, Then::Close);
        }
        self.keep_drafts();
        self.close(index);
    }

    /// Drops tab `index`, whatever state it is in. The caller has already
    /// settled what happens to unsaved work.
    fn close(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        match project::active_after_close(self.open.len(), self.active, index) {
            Some(next) => {
                self.open.remove(index);
                self.activate(next);
            }
            // The last one: with no tab left there is nothing to show,
            // so the window goes instead. It stays in `open` until the
            // loop ends — every accessor here assumes a tab in front,
            // and the rest of this event still has to run.
            None => self.closing = true,
        }
    }

    /// Which tab, if any, is the project `key` names. Opening the same
    /// board twice would give it two documents and one file.
    fn tab_with_key(&self, key: &str) -> Option<usize> {
        self.open.iter().position(|o| o.project.key() == key)
    }

    fn tab_with_path(&self, path: &Path) -> Option<usize> {
        self.open
            .iter()
            .position(|o| matches!(&o.project.origin, Origin::File(p) if p == path))
    }

    /// Starts closing the window: each dirty tab is asked about in turn.
    fn quit(&mut self) {
        self.end_typing();
        self.quitting = true;
        self.step_quit();
    }

    /// Asks about the next tab standing in the way, or lets the window
    /// go when none is left. Emptying `open` is what ends the loop, the
    /// same way closing the last tab does.
    fn step_quit(&mut self) {
        if !self.quitting || self.pending.is_some() {
            return;
        }
        self.keep_drafts();
        match (0..self.open.len()).find(|i| self.owes_an_answer(*i)) {
            Some(i) => self.ask_about(i, Then::Quit),
            None => self.closing = true,
        }
    }

    /// Adds a tab and makes it the one in front.
    fn open_project(&mut self, mut project: Project) {
        // Artistic text measures its box in the faces this machine has: a
        // board written on another is fitted before anything reads it.
        crate::editor::fit_texts(&mut project.doc, &self.fonts);
        let mut editor = Editor::new();
        editor.set_surface(&self.theme.panel_hex);
        let history = History::new(&project.doc, editor.at());
        self.open.push(Open {
            project,
            editor,
            history,
        });
        self.activate(self.open.len() - 1);
    }

    /// Reads the desktop's theme again and puts the window back on in
    /// it: the palette, the border and the corner, the face and the one
    /// size the chrome letters itself at.
    ///
    /// Reading a face is the whole cost of a restyle and the only part of
    /// it anyone can feel — the desktop's `monospace` here is a Nerd
    /// Font, megabytes of outlines, and parsing it blocks the loop for
    /// tens of milliseconds where everything else costs microseconds. A
    /// theme set does not touch the face, so only a face that actually
    /// moved is read again, and the atlas goes with it: `ensure_atlas`
    /// guards on the size, which cannot see a new face at the old one. A
    /// size that changed is that guard's own business.
    fn restyle(&mut self) {
        let face_was = self.style.face.clone();
        self.style = omarchy::read(&self.home);
        log::info!(
            "theme: {}",
            match &self.style.palette {
                Some(p) => format!("{} on {}", p.foreground, p.background),
                None => "the board's own".to_owned(),
            }
        );
        self.theme = Theme::from_style(&self.style);
        if self.style.face != face_was {
            self.font = face(&self.style);
            self.atlas = None;
        }
        self.dress_editors();
        self.redraw();
    }

    /// Tells every tab what a new frame's ground is laid in. The theme is
    /// the window's, so a frame drawn after `op: theme` is born the new
    /// surface and the ones already down keep the colour they were given.
    fn dress_editors(&mut self) {
        let hex = self.theme.panel_hex.clone();
        for open in &mut self.open {
            open.editor.set_surface(&hex);
        }
    }

    /// Brings tab `index` forward. The outgoing editor keeps its own tool
    /// and selection; the incoming one is told what is being held, since
    /// modifiers are physical and would otherwise be stale.
    fn activate(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        if index != self.active {
            self.end_typing();
        }
        self.active = index;
        let mods = self.modifiers.state();
        let (editor, _) = self.active();
        editor.hold_ctrl(mods.control_key());
        editor.hold_shift(mods.shift_key());
        editor.hold_alt(mods.alt_key());
        editor.hold_space(false);
        self.shared.lock().expect("lock shared").board_id = self.doc().id.clone();
        self.retitle();
        self.load_images();
        self.thumbs_stale = true;
        self.redraw();
        self.update_cursor_icon();
    }

    /// Uploads the texture for every image in the current document that
    /// the renderer does not have yet, so a board that is reopened shows
    /// its images instead of placeholders.
    fn load_images(&mut self) {
        let blobs: Vec<String> = self
            .doc()
            .elements
            .iter()
            .filter_map(|el| match el {
                Element::Image(i) => Some(i.blob.clone()),
                _ => None,
            })
            .collect();
        let Some(gfx) = &mut self.gfx else { return };
        for blob in blobs {
            if gfx.image_slots().contains_key(&blob) {
                continue;
            }
            let loaded = self
                .store
                .read_blob(&blob)
                .and_then(|bytes| bitmap::decode(&bytes))
                .and_then(|bmp| gfx.upload_image(&blob, &bmp));
            if let Err(e) = loaded {
                // The element keeps its box and shows as a placeholder.
                log::error!("loading image {blob}: {e:#}");
            }
        }
    }

    /// The export dialog as it stands this frame, and the lines its
    /// instruction wraps to: how many there are is what decides how tall
    /// the box stands, so the two are only ever worked out together.
    fn send_panel(&self, view: &View) -> Option<(send::Panel, Vec<text::Line>)> {
        let (sending, atlas) = (self.sending.as_ref()?, self.atlas.as_ref()?);
        let scale = self.chrome(view);
        let width = send::Panel::text_width(view.viewport, scale);
        let lines = sending.line.wrap(atlas, width);
        let spec = send::Spec {
            rows: sending.agents.len(),
            lines: lines.len(),
            line_h: atlas.line_height(),
            picture: sending.shape,
        };
        Some((send::Panel::layout(view.viewport, scale, &spec), lines))
    }

    /// Keeps the instruction's scroll inside what the text allows, and —
    /// while the box has the keyboard — just far enough along for the
    /// caret's line to be in sight.
    fn follow_caret(&mut self) {
        let Some(view) = self.view() else { return };
        let Some((panel, lines)) = self.send_panel(&view) else {
            return;
        };
        let Some(sending) = self.sending.as_mut() else {
            return;
        };
        let k = sending.line.caret_line(&lines);
        let shown = panel.shown as f32 * panel.line_h;
        sending.scroll = field::follow(sending.scroll, k, panel.line_h, shown)
            .clamp(0.0, panel.max_scroll(lines.len()));
    }

    /// Writes the call to the `at`th skill answering the call being
    /// typed over what was typed of it.
    fn take_skill(&mut self, at: usize) {
        let Some(sending) = self.sending.as_mut() else {
            return;
        };
        let Some((token, matches, call)) = sending.menu() else {
            return;
        };
        let Some(skill) = matches.get(at).and_then(|&i| sending.skills.get(i)) else {
            return;
        };
        let name = skill.name.clone();
        skills::accept(&mut sending.line, &token, call, &name);
        sending.settle(true);
        self.follow_caret();
        self.redraw();
    }

    /// Whether a field has the keyboard.
    fn typing(&self) -> bool {
        self.sending.is_some()
            || self.renaming.is_some()
            || self.searching.is_some()
            || self.editor().typing().is_some()
    }

    /// The field the keyboard is writing into, if one is: the export
    /// dialog's, or a layer's name being typed.
    fn field_in_hand(&mut self) -> Option<&mut Field> {
        if let Some(sending) = self.sending.as_mut() {
            return Some(&mut sending.line);
        }
        if let Some((_, field)) = self.renaming.as_mut() {
            return Some(field);
        }
        self.searching.as_mut()
    }

    /// The filter narrows the tree by what its field now says.
    fn sync_search(&mut self) {
        if let Some(name) = self.searching.as_ref().map(|f| f.value().to_owned()) {
            self.active().0.filter_mut().name = name;
        }
    }

    /// `Ctrl+C`, `Ctrl+X` or `Ctrl+V` with a field in hand: what is
    /// selected goes to the clipboard, and with `X` out of the field;
    /// what the clipboard holds comes back into it. Nothing selected is
    /// nothing to copy, and the clipboard keeps what it had.
    fn clipboard_key(&mut self, c: &str) {
        let key = c.to_ascii_lowercase();
        if key == "v" {
            return self.paste_text();
        }
        // With no clipboard a copy has nowhere to go, and a cut would
        // only be a delete that looked like something else.
        if self.clipboard.is_none() {
            return log::debug!("copy: no clipboard on this display");
        }
        let cut = key == "x";
        let copied = match key.as_str() {
            "c" => self.field_in_hand().map(|f| f.selected().to_owned()),
            "x" => self.field_in_hand().and_then(Field::cut),
            _ => None,
        };
        let Some(text) = copied.filter(|t| !t.is_empty()) else {
            return;
        };
        if let Some(clipboard) = &self.clipboard {
            clipboard.copy_text(&text);
        }
        // A cut is an edit like any other: the box may now be shorter
        // than it was scrolled, and the call the menu was for may be gone.
        if cut {
            if let Some(sending) = self.sending.as_mut() {
                sending.settle(true);
            }
            self.sync_search();
            self.follow_caret();
        }
        self.redraw();
    }

    /// Asks the clipboard for text, for the field in hand. It arrives
    /// later, as [`UserEvent::PastedText`].
    fn paste_text(&self) {
        match &self.clipboard {
            Some(clipboard) => {
                clipboard.paste_text();
            }
            None => log::debug!("paste: no clipboard on this display"),
        }
    }

    /// Clipboard text came back. The field that asked for it may have
    /// closed since, and then the text has nowhere to go and goes nowhere.
    fn pasted_text(&mut self, text: &str) {
        if std::mem::take(&mut self.pasting_words) && self.field_in_hand().is_none() && self.editor().typing().is_none() {
            return self.paste_words(text);
        }
        if self.field_in_hand().is_none() && self.editor().typing().is_some() {
            let change = self.with_text(|editor, doc, fonts| editor.paste_text(text, doc, fonts));
            self.text_input();
            return self.apply(change);
        }
        if let Some(field) = self.field_in_hand() {
            field.paste(text);
            if let Some(sending) = self.sending.as_mut() {
                sending.settle(true);
            }
            self.sync_search();
            self.follow_caret();
            self.redraw();
        }
    }

    /// Words pasted on the board: a text of their own, its first line
    /// centred on the pointer — or in the middle of the window when the
    /// pointer is not over it — set as the next text would be, in the ink
    /// in the hand, as pasting plain text does on every board.
    fn paste_words(&mut self, text: &str) {
        let words: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
            .chars()
            .filter(|&c| c == '\n' || !c.is_control())
            .collect();
        let words = words.trim_end_matches('\n');
        if words.trim().is_empty() {
            return;
        }
        let Some(view) = self.view() else { return };
        let screen = self
            .cursor
            .filter(|&at| !self.over_chrome(&view, at))
            .unwrap_or((f64::from(view.viewport.w) / 2.0, f64::from(view.viewport.h) / 2.0));
        let (x, y) = view.screen_to_world(screen.0, screen.1);
        let mut style = self.editor().next_style();
        self.ink_hex().clone_into(&mut style.color);
        let text = crate::doc::Text {
            id: String::new(),
            layer: String::new(),
            x,
            y: y - style.size * style.leading / 2.0,
            w: 0.0,
            h: 0.0,
            rotation: 0.0,
            mode: crate::doc::TextMode::Artistic,
            text: words.chars().take(crate::editor::TEXT_MAX).collect(),
            style,
            runs: Vec::new(),
        };
        let born = self.doc().stack_at([x, y]).map(str::to_owned);
        self.with_text(|e, d, f| e.place_text(d, f, text, born.as_deref()));
        self.apply(Change::Scene);
    }

    /// Asks the clipboard for an image. The bytes arrive later, as
    /// [`UserEvent::Pasted`].
    fn paste(&mut self) {
        match (&self.clipboard, &self.clip) {
            // Layers and images first, as always; words when that is all
            // the clipboard holds, which land as a text of their own.
            (Some(clipboard), _) => {
                if !clipboard.paste() {
                    self.pasting_words = clipboard.paste_text();
                }
            }
            (None, Some(clip)) => self.pasted_layers(clip.clone()),
            (None, None) => log::debug!("paste: no clipboard on this display"),
        }
    }

    /// A merge from a key or a row's menu. The runs that are not exact are
    /// drawn first, each into a picture of its own; then the editor merges
    /// the lot, one step. A picture that cannot be taken leaves the board
    /// as it was.
    fn merge_layers(&mut self, command: Command) {
        match self.try_merge(command) {
            Ok(change) => self.apply(change),
            Err(e) => log::warn!("merging: {e:#}"),
        }
    }

    /// The merge itself, pictures and all, left for the caller to apply.
    fn try_merge(&mut self, command: Command) -> anyhow::Result<Change> {
        let runs = self.editor().merges(self.doc(), command);
        let mut drawn = Vec::with_capacity(runs.len());
        for run in &runs {
            drawn.push(if self.doc().exact(run) {
                None
            } else {
                Some(self.picture_of(run)?)
            });
        }
        let (editor, doc) = self.active();
        Ok(editor.merge(doc, command, &drawn))
    }

    /// The picture a run is merged into: what its members show, drawn onto
    /// nothing — at the zoom the board is looked at, never less than an
    /// export's — kept in the store as a PNG and put on the GPU as a pasted
    /// image is, and laid over the box it was taken of.
    fn picture_of(&mut self, run: &merge::Run) -> anyhow::Result<Image> {
        let sub = self.doc().run_document(run);
        let bounds = merge::raster_box(&sub).context("there is nothing there to draw")?;
        let zoom = self.view().map_or(1.0, |v| v.px_per_world());
        let most = self.gfx.as_ref().map_or(1, Gfx::max_dimension).min(bitmap::MAX_SIDE);
        let (view, w, h, (lo, hi)) = merge::raster_view(bounds, zoom.max(export::EXPORT_SCALE), most);
        let mut rgba = self.render_onto(&sub, &view, w, h, [0.0; 4])?;
        let srgb = self.gfx.as_ref().is_some_and(Gfx::is_srgb);
        bitmap::unpremultiply(&mut rgba, srgb);
        let png = bitmap::encode(&rgba, w, h)?;
        let blob = self.store.write_blob(&png)?;
        let gfx = self.gfx.as_mut().context("there is no window to draw with")?;
        gfx.upload_image(&blob, &Bitmap { w, h, rgba })?;
        Ok(Image {
            id: crate::doc::new_id(),
            layer: String::new(),
            x: lo[0],
            y: lo[1],
            w: hi[0] - lo[0],
            h: hi[1] - lo[1],
            rotation: 0.0,
            blob,
        })
    }

    /// `Ctrl+C` and `Ctrl+X` on the board: the picked layers become the
    /// clip — on the system's clipboard under the board's own type, and
    /// kept here for a display with none — and with `X` they go.
    fn copy_layers(&mut self, cut: bool) {
        let (editor, doc) = self.active();
        let (clip, change) = if cut {
            editor.cut(doc)
        } else {
            (editor.copy(doc), Change::None)
        };
        let Some(clip) = clip else { return };
        if let Some(clipboard) = &self.clipboard {
            match clip.to_json() {
                Ok(json) => clipboard.copy_layers(json),
                Err(e) => log::warn!("copying layers: {e:#}"),
            }
        }
        self.clip = Some(clip);
        self.apply(change);
    }

    /// Layers came back: planted above the active layer, in place when
    /// that is on show and in the middle of the window otherwise, and
    /// their images put on the GPU the way a reopened board's are.
    fn pasted_layers(&mut self, clip: Document) {
        let Some(view) = self.view() else { return };
        let (x0, y0) = view.screen_to_world(0.0, 0.0);
        let (x1, y1) = view.screen_to_world(f64::from(view.viewport.w), f64::from(view.viewport.h));
        let (editor, doc) = self.active();
        let change = editor.paste(doc, &clip, ([x0, y0], [x1, y1]));
        self.apply(change);
        self.load_images();
    }

    /// A clipboard image came back: keep the original bytes, upload the
    /// texels, and let the editor place it.
    fn pasted(&mut self, bytes: Vec<u8>, bitmap: Bitmap) {
        let Some(view) = self.view() else { return };
        let blob = match self.store.write_blob(&bytes) {
            Ok(blob) => blob,
            Err(e) => return log::error!("storing the pasted image: {e:#}"),
        };
        if let Some(gfx) = &mut self.gfx
            && let Err(e) = gfx.upload_image(&blob, &bitmap)
        {
            log::error!("uploading the pasted image: {e:#}");
        }
        let cursor = self.cursor;
        let (editor, doc) = self.active();
        let change = editor.paste_image(doc, &view, cursor, blob, (bitmap.w, bitmap.h));
        self.apply(change);
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.exit_error = Some(e);
        event_loop.exit();
    }

    /// Current world ↔ screen mapping; `None` before the window exists.
    fn view(&self) -> Option<View> {
        let (w, h) = self.gfx.as_ref()?.size();
        let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        Some(View {
            camera: self.doc().camera,
            viewport: Viewport { w, h },
            scale,
        })
    }

    /// What the chrome is laid out at: the window's own scale factor
    /// times the theme's spacing scale. The canvas, the grid and the
    /// selection handles keep the window's alone — panel density is not
    /// zoom.
    fn chrome(&self, view: &View) -> f64 {
        view.scale * self.style.chrome_scale()
    }

    /// Where the canvas begins: everything that hangs off the strip has
    /// to measure it the same way the strip does.
    fn strip_top(&self, view: &View) -> f32 {
        (tabs::HEIGHT * self.chrome(view) as f32).round()
    }

    fn dock(&self, view: &View) -> Dock {
        Dock::layout(
            view.viewport,
            self.chrome(view),
            &Tool::ALL,
            INKS.len(),
        )
    }

    /// The strip, or `None` before the atlas exists — there is nothing to
    /// measure a label with until then.
    fn tabs(&self, view: &View) -> Option<Tabs> {
        let atlas = self.atlas.as_ref()?;
        let labels: Vec<(String, bool)> = self
            .open
            .iter()
            .map(|o| (o.project.label(), o.project.dirty))
            .collect();
        Some(Tabs::layout(
            view.viewport,
            self.chrome(view),
            atlas,
            &labels,
            self.active,
            self.bar(view).map_or(0.0, |b| b.end()),
        ))
    }

    /// The application menu's titles, at the strip's left end.
    fn bar(&self, view: &View) -> Option<Bar> {
        Some(Bar::layout(self.chrome(view), self.atlas.as_ref()?))
    }

    /// The title whose menu is standing open, if one of the bar's is.
    fn bar_open(&self) -> Option<Title> {
        match self.menu.as_ref()?.purpose {
            Purpose::Bar { title, .. } => Some(title),
            _ => None,
        }
    }

    /// The layers panel, when it is up and there is an atlas to letter
    /// it with. It shows the whole tree, each group and frame open or
    /// shut as the tab's editor keeps it.
    fn panel(&self, view: &View) -> Option<Panel> {
        if !self.layers_shown {
            return None;
        }
        let atlas = self.atlas.as_ref()?;
        let top = self.strip_top(view);
        let editor = self.editor();
        let rows = editor.rows(self.doc());
        Some(Panel::layout(
            view.viewport,
            self.chrome(view),
            top,
            atlas,
            &rows,
            self.scroll + self.scrolling.offset(),
            editor.filtering().is_some(),
        ))
    }

    /// The panel's handle, once there is an atlas to letter it with. It
    /// is on screen whether the panel is up or not — closed, it is the
    /// only thing that says the panel is there.
    fn handle(&self, view: &View) -> Option<layers::Handle> {
        let atlas = self.atlas.as_ref()?;
        let top = self.strip_top(view);
        Some(layers::Handle::layout(
            view.viewport,
            self.chrome(view),
            top,
            atlas,
            self.layers_shown,
        ))
    }

    /// Brush Properties, when the brush is in hand. The bar is the
    /// brush's own chrome like the palette; the chevron and the
    /// palette's sliders button drop the Advanced layout under it.
    fn props(&self, view: &View) -> Option<Props> {
        if self.editor().tool() != Tool::Brush {
            return None;
        }
        let top = self.strip_top(view);
        Some(Props::layout(
            view.viewport,
            self.chrome(view),
            top,
            self.props_open,
        ))
    }

    /// The brush strip, when the brush is in hand and it is up. It is
    /// the tool's own chrome: no other tool has a use for it, so it
    /// comes and goes with the tool rather than being toggled on top of
    /// one that ignores it.
    fn strip(&self, view: &View) -> Option<Strip> {
        if !self.palette_shown || self.editor().tool() != Tool::Brush {
            return None;
        }
        Some(Strip::layout(view.viewport, self.chrome(view), self.above(view)))
    }

    /// The brush library, when the strip's chevron has opened it. It
    /// stands to the right of the strip with canvas between them, so a
    /// brush can be carried from one to the other.
    fn library(&self, view: &View) -> Option<Palette> {
        let strip = self.strip(view).filter(|_| self.library_shown)?;
        Some(Palette::layout(
            view.viewport,
            self.chrome(view),
            self.above(view),
            strip.rect.x + strip.rect.w + palette::MARGIN * self.chrome(view) as f32,
            self.brushes.sets(),
            self.palette_scroll,
        ))
    }

    /// Where the left-hand panels hang from: the tab strip, since the
    /// properties bar stands in the middle — unless the window is so
    /// narrow that the bar reaches over them, when they hang under it.
    fn above(&self, view: &View) -> f32 {
        let top = self.strip_top(view);
        let Some(bar) = self.props(view) else {
            return top;
        };
        let s = self.chrome(view);
        let strip = Strip::layout(view.viewport, s, top).rect;
        let mut right = strip.x + strip.w;
        if self.library_shown {
            let library = Palette::layout(
                view.viewport,
                s,
                top,
                right + palette::MARGIN * s as f32,
                self.brushes.sets(),
                self.palette_scroll,
            );
            right = library.rect.x + library.rect.w;
        }
        if bar.rect.x < right {
            bar.rect.y + bar.rect.h
        } else {
            top
        }
    }

    /// Whether `screen` is over the strip, the handle, either panel or
    /// the dock rather than the canvas.
    fn over_chrome(&self, view: &View, screen: (f64, f64)) -> bool {
        let (x, y) = screen;
        self.menu_laid(view).is_some_and(|m| m.contains(x, y))
            || self.tabs(view).and_then(|t| t.hit(x, y)).is_some()
            || self.handle(view).is_some_and(|h| h.hit(x, y))
            || self.panel(view).and_then(|p| p.hit(x, y)).is_some()
            || self.props(view).and_then(|b| b.hit(x, y)).is_some()
            || self.text_bar(view).and_then(|b| b.hit(x, y)).is_some()
            || self.shape_bar(view).and_then(|b| b.hit(x, y)).is_some()
            || self.strip(view).and_then(|s| s.hit(x, y)).is_some()
            || self.library(view).and_then(|p| p.hit(x, y)).is_some()
            || self.dock(view).hit(x, y).is_some()
    }

    /// The ink the next stroke lays, as the document writes it. The
    /// palette is fixed, so an index can only fall outside it if one is
    /// ever seated from somewhere that is not a click on a cell.
    fn ink_hex(&self) -> &str {
        INKS.get(self.ink).copied().unwrap_or(INKS[0])
    }

    /// The same ink, as the canvas draws it.
    fn ink_rgba(&self) -> Rgba {
        scene::parse_color(self.ink_hex())
    }

    /// A click on the brush library.
    fn palette_hit(&mut self, hit: palette::Hit) {
        match hit {
            palette::Hit::Brush(set, index) => self.brushes.select(set, index),
            palette::Hit::Panel => return,
        }
        self.brushes_dirty = true;
    }

    /// The seat under a point, when it is one a brush can be put in.
    /// Slot 0 is computed from what the hand has been reaching for, so
    /// it is not a place to drop one.
    fn seat_under(&self, x: f64, y: f64) -> Option<usize> {
        let view = self.view()?;
        match self.strip(&view)?.hit(x, y) {
            Some(slots::Hit::Slot(n)) if n != 0 => Some(n),
            _ => None,
        }
    }

    /// A click on the brush strip.
    fn strip_hit(&mut self, hit: slots::Hit) {
        match hit {
            // An empty seat holds nothing to take up, and the library
            // opening is the window's business rather than the brushes'.
            slots::Hit::Slot(n) if !self.brushes.take_slot(n) => return,
            slots::Hit::Slot(_) => {}
            slots::Hit::Properties => self.props_open = !self.props_open,
            slots::Hit::Library => {
                self.library_shown = !self.library_shown;
                return;
            }
            slots::Hit::Panel => return,
        }
        self.brushes_dirty = true;
    }

    /// Writes the brushes back if anything about them changed. Called
    /// when a gesture ends, not while one is running: a slider drag
    /// would otherwise write the file on every frame of it.
    fn keep_brushes(&mut self) {
        if !std::mem::take(&mut self.brushes_dirty) {
            return;
        }
        if let Err(e) = self.store.save_brushes(&self.brushes.edits()) {
            log::warn!("keeping the brushes: {e:#}");
        }
    }

    /// A click on the properties bar.
    fn props_hit(&mut self, bar: &Props, hit: props::Hit, x: f64) {
        match hit {
            props::Hit::Toggle => self.props_open = !self.props_open,
            props::Hit::Slider(i) => {
                self.grab = Some(i);
                self.drag_field(bar, i, x);
            }
            props::Hit::Reset => {
                self.brushes.reset();
                self.brushes_dirty = true;
                self.keep_brushes();
            }
            props::Hit::Bar => {}
        }
    }

    /// The bar's fields lie flat, so a drag on one reads the x. The
    /// field is named by its place, and the bar may have been folded
    /// since the press: a place that is no longer there writes nothing.
    fn drag_field(&mut self, bar: &Props, field: usize, x: f64) {
        let Some(property) = bar.fields.get(field).map(|f| f.property) else {
            return;
        };
        let f = bar.fraction(field, x);
        property.set_fraction(self.brushes.brush_mut(), f);
        self.brushes_dirty = true;
    }

    /// Opens the blend modes' menu beside `at`, on the active layer's
    /// mode, remembering what every picked layer had.
    fn open_blend_menu(&mut self, at: ScreenRect) {
        let doc = self.doc();
        let editor = self.editor();
        let (current, group) = doc
            .layer(editor.active(doc))
            .map_or((BlendMode::Normal, false), |l| {
                (l.blend, l.kind == crate::doc::Kind::Group)
            });
        let (items, modes) = layers::blend_menu(current, group);
        let was = editor
            .picked(doc)
            .into_iter()
            .filter_map(|id| doc.layer(id).map(|l| (id.to_owned(), l.blend)))
            .collect();
        self.menu = Some(Opened {
            purpose: Purpose::Blend { modes, was },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// Opens row `id`'s menu at the pointer, on the picked layers — the
    /// row taken into the pick first unless it is already in it, as a
    /// right click does in Photoshop.
    fn open_row_menu(&mut self, id: String, (x, y): (f64, f64)) {
        let (editor, doc) = self.active();
        if !editor.picked(doc).contains(&id.as_str()) {
            let rows = editor.rows(doc);
            let order: Vec<&str> = rows.iter().map(|r| r.layer.id.as_str()).collect();
            let change = editor.pick_layer(doc, &id, Pick::Only, &order);
            self.apply(change);
        }
        let doc = self.doc();
        let editor = self.editor();
        let picked: Vec<&Layer> = editor.picked(doc).into_iter().filter_map(|p| doc.layer(p)).collect();
        let state = layers::RowState {
            locked: picked.iter().all(|l| l.locked),
            hidden: picked.iter().all(|l| !l.visible),
            tag: doc.layer(&id).map_or(Tag::None, |l| l.color),
            merge: editor.merge_name(doc),
        };
        let paste = match &self.clipboard {
            Some(clipboard) => clipboard.can_paste(),
            None => self.clip.is_some(),
        };
        let (items, lines) = layers::row_menu(|c| editor.can(doc, c), paste, state);
        let at = ScreenRect {
            x: x as f32,
            y: y as f32,
            w: 0.0,
            h: 0.0,
        };
        self.menu = Some(Opened {
            purpose: Purpose::Row { id, lines },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// Opens the colours' menu beside `at` for layer `id`.
    fn open_tag_menu(&mut self, id: String, at: ScreenRect) {
        let current = self.doc().layer(&id).map_or(Tag::None, |l| l.color);
        let (items, tags) = layers::tag_menu(current);
        self.menu = Some(Opened {
            purpose: Purpose::Tag { id, tags },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// Opens the application menu `title` under its title, each line
    /// offered only where it would do something now.
    fn open_bar_menu(&mut self, title: Title) {
        let Some(at) = self
            .view()
            .and_then(|view| self.bar(&view))
            .and_then(|bar| bar.titles.iter().find(|(_, t)| *t == title).map(|(r, _)| *r))
        else {
            return;
        };
        let state = self.bar_state();
        let (editor, doc) = (self.editor(), self.doc());
        let (items, actions) = menubar::items(title, &state, |c| editor.can(doc, c));
        self.menu = Some(Opened {
            purpose: Purpose::Bar { title, actions },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// What the application menu needs to know of the window.
    fn bar_state(&self) -> menubar::State {
        let doc = self.doc();
        let editor = self.editor();
        let history = &self.open[self.active].history;
        let picked: Vec<&Layer> = editor.picked(doc).into_iter().filter_map(|p| doc.layer(p)).collect();
        menubar::State {
            undo: history.can_undo(),
            redo: history.can_redo(),
            paste: match &self.clipboard {
                Some(clipboard) => clipboard.can_paste(),
                None => self.clip.is_some(),
            },
            selection: !editor.selection().is_empty(),
            delete: editor.can_delete(doc),
            layers: self.layers_shown,
            library: (editor.tool() == Tool::Brush).then_some(self.palette_shown),
            locked: !picked.is_empty() && picked.iter().all(|l| l.locked),
            hidden: !picked.is_empty() && picked.iter().all(|l| !l.visible),
            merge: editor.merge_name(doc),
        }
    }

    /// The filter's field takes the keyboard, its caret at `x`.
    fn search_at(&mut self, panel: &Panel, x: f64) {
        let (Some(atlas), Some(text)) = (self.atlas.as_ref(), panel.search_text()) else {
            return;
        };
        let name = self.editor().filtering().map_or(String::new(), |f| f.name.clone());
        let mut field = Field::name(&name);
        let lines = field.wrap(atlas, f32::INFINITY);
        let at = field.index_at(atlas, &lines, 0, x as f32 - text.x - field::PADDING);
        field.go(at, false);
        self.searching = Some(field);
    }

    /// The menu standing over the window, laid out.
    fn menu_laid(&self, view: &View) -> Option<menu::Menu> {
        let opened = self.menu.as_ref()?;
        let atlas = self.atlas.as_ref()?;
        Some(menu::Menu::layout(
            view.viewport,
            self.chrome(view),
            opened.at,
            atlas,
            &opened.items,
            opened.scroll,
        ))
    }

    /// The line under `(x, y)` is tried on the board.
    fn hover_menu_at(&mut self, x: f64, y: f64) {
        let hover = self.view().and_then(|view| {
            let laid = self.menu_laid(&view)?;
            laid.hit(x, y, &self.menu.as_ref()?.items)
        });
        self.hover_menu(hover);
    }

    /// The line under the pointer is `hover` now: a blend mode is tried
    /// on the picked layers as the pointer passes over it, and what they
    /// had comes back when it leaves every line.
    fn hover_menu(&mut self, hover: Option<usize>) {
        let Some(opened) = self.menu.as_mut() else { return };
        if opened.hover == hover {
            return;
        }
        opened.hover = hover;
        // A blend mode is tried on the board — a tag is read in the panel,
        // and the line lit under the pointer already says it.
        // A family is tried on the texts the bar is looking at, as a blend
        // mode is on the layers: set in it as the pointer passes over it,
        // and put back when it leaves.
        let font = match &opened.purpose {
            Purpose::Font { families, was } => Some((hover.and_then(|i| families.get(i).cloned()), was.clone())),
            _ => None,
        };
        if let Purpose::Blend { modes, was } = &opened.purpose {
            let tried = hover.and_then(|i| modes.get(i).copied());
            let was = was.clone();
            let (editor, doc) = self.active();
            restore_blends(doc, &was);
            if let Some(mode) = tried {
                let _ = editor.set_blend(doc, mode);
            }
        }
        if let Some((tried, was)) = font {
            self.with_text(|e, d, f| {
                e.put_look_back(d, f, &was);
                if let Some(family) = tried {
                    let _ = e.restyle(d, f, |s| s.font.clone_from(&family));
                }
            });
        }
        // A paint is tried on the shapes the bar is looking at, and put
        // back when the pointer leaves every line — as long as there are
        // shapes to try it on: with none it would be the next one's, and
        // that is only ever set by a line taken.
        if let Some(Opened {
            purpose: Purpose::Paint { stroke, paints, was },
            ..
        }) = &self.menu
            && !was.is_empty()
        {
            let tried = hover.and_then(|i| paints.get(i).cloned());
            let (stroke, was) = (*stroke, was.clone());
            let opened: Vec<String> = was.iter().map(|(id, _)| id.clone()).collect();
            let (editor, doc) = self.active();
            editor.repaint(doc, &was, stroke);
            if let Some(paint) = tried {
                let _ = editor.paint_shapes(doc, &opened, paint, stroke);
            }
        }
        self.redraw();
    }

    /// Shuts the menu, taking line `take` or none. A blend mode taken is
    /// one step; none put back leaves the board as the menu found it.
    fn close_menu(&mut self, take: Option<usize>) {
        let Some(opened) = self.menu.take() else { return };
        // The application menu's lines are the keys' own actions.
        if let Purpose::Bar { actions, .. } = &opened.purpose {
            if let Some(&action) = take.and_then(|i| actions.get(i)) {
                self.run_action(action);
            }
            return self.redraw();
        }
        // A name opens a field, and the clipboard is the window's: none
        // of them is the editor's to answer.
        if let Purpose::Row { id, lines } = &opened.purpose {
            match take.and_then(|i| lines.get(i)) {
                Some(layers::RowLine::Rename) => self.panel_hit(PanelHit::Rename(id.clone())),
                Some(layers::RowLine::Copy) => self.copy_layers(false),
                Some(&layers::RowLine::Run(command)) if command.merges() => {
                    self.merge_layers(command);
                    self.redraw();
                    return;
                }
                Some(layers::RowLine::Cut) => self.copy_layers(true),
                Some(layers::RowLine::Paste) => self.paste(),
                _ => {}
            }
        }
        if let Purpose::Paint { stroke, paints, was } = &opened.purpose {
            let paint = take.and_then(|i| paints.get(i).cloned());
            let stroke = *stroke;
            // A stroke's ink taken is the ink in the hand from then on, as
            // one clicked in the dock is.
            if stroke
                && let Some(Some(hex)) = &paint
                && let Some(i) = INKS.iter().position(|k| k.eq_ignore_ascii_case(hex))
            {
                self.ink = i;
            }
            // Opened on shapes, the menu's line is theirs alone; opened on
            // none, it is how the next one is drawn.
            let opened: Vec<String> = was.iter().map(|(id, _)| id.clone()).collect();
            let (editor, doc) = self.active();
            editor.repaint(doc, was, stroke);
            let change = match paint {
                Some(paint) if !opened.is_empty() => editor.paint_shapes(doc, &opened, paint, stroke),
                Some(paint) => editor.restyle_shapes(doc, paint_change(stroke, paint)),
                None => Change::None,
            };
            self.apply(change);
            return self.redraw();
        }
        if let Purpose::Font { families, was } = &opened.purpose {
            let family = take.and_then(|i| families.get(i).cloned());
            let change = self.with_text(|e, d, f| {
                e.put_look_back(d, f, was);
                match family {
                    Some(family) => e.restyle(d, f, |s| s.font.clone_from(&family)),
                    None => Change::None,
                }
            });
            self.text_input();
            self.apply(change);
            return self.redraw();
        }
        let (editor, doc) = self.active();
        let change = match opened.purpose {
            Purpose::Blend { modes, was } => {
                restore_blends(doc, &was);
                match take.and_then(|i| modes.get(i).copied()) {
                    Some(mode) => editor.set_blend(doc, mode),
                    None => Change::None,
                }
            }
            Purpose::Tag { id, tags } => match take.and_then(|i| tags.get(i).copied()) {
                Some(tag) => editor.set_tag(doc, &id, tag),
                None => Change::None,
            },
            Purpose::Row { id, lines } => match take.and_then(|i| lines.get(i).copied()) {
                Some(layers::RowLine::Run(command)) => editor.run(doc, command),
                Some(layers::RowLine::Tag(tag)) => editor.set_tag(doc, &id, tag),
                _ => Change::None,
            },
            Purpose::Head { end, heads } => match take.and_then(|i| heads.get(i).copied()) {
                Some(head) => editor.restyle_shapes(
                    doc,
                    match end {
                        select::End::From => crate::editor::Restyle::Start(head),
                        select::End::To => crate::editor::Restyle::End(head),
                    },
                ),
                None => Change::None,
            },
            Purpose::Bar { .. } | Purpose::Font { .. } | Purpose::Paint { .. } => Change::None,
        };
        self.apply(change);
        self.redraw();
    }

    /// Writes the name being typed onto its layer and shuts the field.
    /// A name of nothing but space leaves the layer as it was, which is
    /// `rename_layer`'s own answer.
    fn commit_rename(&mut self) {
        let Some((id, field)) = self.renaming.take() else {
            return;
        };
        let name = field.value().to_owned();
        let (editor, doc) = self.active();
        let change = editor.rename_layer(doc, &id, &name);
        self.apply(change);
    }

    /// What a press on the panel means once the clock is taken into
    /// account: a press on a card that is already picked, inside
    /// [`DOUBLE_CLICK`] of the last one on that same card, asks for the
    /// name rather than for the layer.
    fn second_press(&mut self, hit: PanelHit) -> PanelHit {
        let PanelHit::Pick(id) = hit else {
            return hit;
        };
        // A press with Ctrl or Shift is a pick, never the first half of a
        // rename.
        let mods = self.modifiers.state();
        if mods.control_key() || mods.shift_key() {
            self.last_card = None;
            return PanelHit::Pick(id);
        }
        let now = Instant::now();
        let again = self
            .last_card
            .as_ref()
            .is_some_and(|(was, at)| *was == id && now.duration_since(*at) < DOUBLE_CLICK);
        self.last_card = Some((id.clone(), now));
        if again {
            PanelHit::Rename(id)
        } else {
            PanelHit::Pick(id)
        }
    }

    /// A click on the layers panel, handed to the editor.
    fn panel_hit(&mut self, hit: PanelHit) {
        // A rename opens a field rather than changing the document, so
        // it is answered before the editor is borrowed. The name comes
        // off the layer: a row's own label is cut down to what fits.
        if let PanelHit::Rename(id) = hit {
            let name = self
                .doc()
                .layer(&id)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            self.renaming = Some((id, Field::name(&name)));
            return;
        }
        // Ctrl takes one in or out, Shift the rows from the anchor.
        let mods = self.modifiers.state();
        let how = if mods.control_key() {
            Pick::Toggle
        } else if mods.shift_key() {
            Pick::Range
        } else {
            Pick::Only
        };
        let (editor, doc) = self.active();
        let change = match hit {
            PanelHit::Pick(id) => {
                let rows = editor.rows(doc);
                let order: Vec<&str> = rows.iter().map(|r| r.layer.id.as_str()).collect();
                editor.pick_layer(doc, &id, how, &order)
            }
            PanelHit::Toggle(id) => editor.toggle_layer(doc, &id),
            PanelHit::Open(id) => editor.toggle_open(&id),
            PanelHit::Lock(id) => editor.toggle_lock_of(doc, &id),
            PanelHit::Group => editor.add_group(doc),
            PanelHit::Add => editor.add_layer(doc),
            PanelHit::Remove => editor.remove_layers(doc),
            PanelHit::LockPicked => editor.toggle_lock(doc),
            PanelHit::Filter => editor.toggle_filter(),
            PanelHit::FilterKind(kind) => {
                editor.filter_mut().toggle_kind(kind);
                Change::Selection
            }
            PanelHit::FilterTag(tag) => {
                editor.filter_mut().toggle_tag(tag);
                Change::Selection
            }
            // The strength is read off the pointer, which is the press's
            // own business, and the blend menu is the window's.
            PanelHit::Opacity
            | PanelHit::Blend
            | PanelHit::Search
            | PanelHit::Rename(_)
            | PanelHit::Panel => {
                Change::None
            }
        };
        self.apply(change);
    }

    /// Opens the send panel on what is selected. It needs both halves —
    /// something to send and somewhere to send it — so with either
    /// missing it says which and opens nothing: a shortcut onto a dead
    /// end teaches nothing.
    fn ask_send(&mut self) {
        let selection = self.editor().selection().to_vec();
        let scope = match selection.as_slice() {
            [] => {
                log::info!("nothing selected: select a frame or some objects to send");
                return;
            }
            [one] if self.doc().frame(one).is_some() => export::Scope::Frame(one.clone()),
            ids => export::Scope::Selection(ids.to_vec()),
        };
        let found = agents::list();
        if found.is_empty() {
            log::info!("no agent is running: export to the agent needs one");
            return;
        }
        let shape = export::bounds(self.doc(), &scope).map(|b| export::shape(&b) as f32);
        // What the page is called is not asked: `send_to` names it after
        // what owns the scope, or else after the tab.
        self.sending = Some(Sending {
            scope,
            agents: found,
            target: 0,
            // No longer than the send would take: a field never holds
            // what the thing it feeds would refuse.
            line: Field::lines("").limited(agents::PROMPT_MAX),
            scroll: 0.0,
            selecting: false,
            skills: Vec::new(),
            pick: 0,
            dismissed: None,
            shape,
            picture: None,
            pictured: None,
        });
        if let Some(sending) = self.sending.as_mut() {
            sending.aim(0);
        }
        self.redraw();
    }

    /// Writes the page into the agent's own directory and hands it the
    /// line. Either half failing leaves a message and the panel shut:
    /// the files are on disk whatever the terminal did with them.
    fn do_send(&mut self) {
        let Some(sending) = self.sending.take() else {
            return;
        };
        let Some(agent) = sending.agents.get(sending.target).cloned() else {
            return;
        };
        if let Err(e) = self.send_to(&agent, &sending) {
            log::warn!("export to the agent: {e:#}");
        }
        self.redraw();
    }

    fn send_to(&mut self, agent: &agents::Agent, sending: &Sending) -> anyhow::Result<()> {
        let line = agents::sanitize(sending.line.value())?;
        // Named here and not when the dialog opened: a selection across
        // layers is counted past what this agent's folder holds, and the
        // agent may have been changed since.
        let taken = taken_names(&agent.cwd);
        let slug = export::page_slug(self.doc(), &sending.scope, &self.project().label(), &taken);
        let cwd = PathBuf::from(&agent.cwd);
        let files = self.write_page(&sending.scope, &cwd, &slug)?;
        log::info!("exported {} files to {}", files.len(), agent.cwd);
        agents::send(agent, &agents::prompt(&line, &agents::relative(&files, &cwd)))
    }

    /// The page a scope makes, written under `dir`: the picture, the same
    /// objects in the board's own schema, the inventory, and a copy of
    /// every blob the json names (§8). Both doors go through here — the
    /// `Ctrl+E` that hands a page to an agent's session, and the
    /// `read_frame` an agent asks for itself.
    fn write_page(
        &mut self,
        scope: &export::Scope,
        dir: &Path,
        slug: &str,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let bounds =
            export::bounds(self.doc(), scope).context("there is nothing there to export")?;
        let sub = export::sub_document(self.doc(), scope);
        let md = export::inventory(&sub, &bounds);
        let blobs = self.blobs_of(&sub);
        let most = self.gfx.as_ref().map_or(1, Gfx::max_dimension);
        let (view, w, h) = export::view_for(&bounds, most);
        let rgba = self.render_sub(&sub, &view, w, h)?;
        let png = bitmap::encode(&rgba, w, h)?;
        export::write(dir, slug, &png, &sub, &md, &blobs)
    }

    /// What `view` shows of `sub`, drawn offscreen on the board's own
    /// ground: `w` by `h` px of tight RGBA8. The page and the dialog's
    /// picture of it both come through here, so the two cannot differ.
    ///
    /// The sub-document, not the board: the json beside the picture is
    /// the scope, and the box the picture is taken through is the
    /// scope's plus `EXPORT_MARGIN` — so drawing the whole board put a
    /// neighbour's ink in the margin of a picture whose json says nothing
    /// about it. What leaves the board is one thing, said twice.
    fn render_sub(
        &mut self,
        sub: &Document,
        view: &View,
        w: u32,
        h: u32,
    ) -> anyhow::Result<Vec<u8>> {
        self.render_onto(sub, view, w, h, self.theme.bg)
    }

    /// What `view` shows of `sub`, drawn offscreen on `ground` — which a
    /// merge's picture wants to be nothing at all.
    fn render_onto(
        &mut self,
        sub: &Document,
        view: &View,
        w: u32,
        h: u32,
        ground: scene::Rgba,
    ) -> anyhow::Result<Vec<u8>> {
        let edge = self.theme.muted;
        let shapes = std::mem::take(&mut self.shapes);
        let frame = {
            let none = ImageSlots::new();
            let images = self.gfx.as_ref().map_or(&none, Gfx::image_slots);
            scene::document_prims(sub, view, images, &shapes, &self.letters(), edge, None)
        };
        self.shapes = shapes;
        let gfx = self
            .gfx
            .as_mut()
            .context("there is no window to draw with")?;
        gfx.sync_letters(&self.glyphs);
        let drawn = gfx.render_offscreen(w, h, ground, &frame);
        self.start_letters_over();
        drawn
    }

    /// Takes the panel's thumbnails again when what they show may have
    /// changed — the board came to rest after a change, a step was taken
    /// or undone, another tab came forward — or other rows are on show.
    /// Only at rest: a stroke's picture is taken once it is laid, not on
    /// every sample of it. Every raster and vector row on show is drawn
    /// into a cell of one sheet, on a checker, in one pass.
    fn ensure_thumbs(&mut self) {
        if !self.settled() {
            return;
        }
        let Some(view) = self.view() else { return };
        let Some(panel) = self.panel(&view) else { return };
        let ids: Vec<String> = panel
            .rows
            .iter()
            .filter(|r| matches!(r.kind, crate::doc::Kind::Raster | crate::doc::Kind::Vector))
            .map(|r| r.id.clone())
            .collect();
        if ids.is_empty() {
            self.thumbs = None;
            return;
        }
        let s = self.chrome(&view) as f32;
        let cell = ((layers::GLYPH_W * s).round() as u32, (layers::GLYPH_H * s).round() as u32);
        let sheet = thumbs::Sheet::new(ids, cell);
        if !self.thumbs_stale && self.thumbs.as_ref().is_some_and(|(had, _)| *had == sheet) {
            return;
        }
        let edge = self.theme.muted;
        let shapes = std::mem::take(&mut self.shapes);
        let mut frame = scene::Frame::new();
        {
            let none = ImageSlots::new();
            let images = self.gfx.as_ref().map_or(&none, Gfx::image_slots);
            for (i, id) in sheet.ids().iter().enumerate() {
                frame.extend(sheet.checker(i));
                let subject = thumbs::subject(self.doc(), id);
                if let Some(content) = merge::raster_box(&subject) {
                    let mut picture = scene::document_prims(
                        &subject,
                        &sheet.view(i, content),
                        images,
                        &shapes,
                        &self.letters(),
                        edge,
                        None,
                    );
                    picture.cut(sheet.rect(i));
                    frame.append(picture);
                }
            }
        }
        self.shapes = shapes;
        let Some(gfx) = self.gfx.as_mut() else { return };
        gfx.sync_letters(&self.glyphs);
        let slot = gfx.render_thumbs(sheet.size(), &frame);
        self.start_letters_over();
        self.thumbs = Some((sheet, slot));
        self.thumbs_stale = false;
    }

    /// Takes the dialog's picture of what is leaving again when its place
    /// has changed size — it opened, the window was resized, the scale
    /// changed — at that size in px, so it is drawn one to one. Like the
    /// atlas, it is made here and never while a frame is being built.
    fn ensure_picture(&mut self) {
        let Some(view) = self.view() else { return };
        let Some((panel, _)) = self.send_panel(&view) else {
            return;
        };
        let Some(rect) = panel.picture else { return };
        let want = (rect.w.round() as u32, rect.h.round() as u32);
        let Some(sending) = self.sending.as_ref() else {
            return;
        };
        if sending.pictured == Some(want) {
            return;
        }
        let scope = sending.scope.clone();
        let taken = export::bounds(self.doc(), &scope)
            .context("there is nothing there to picture")
            .and_then(|bounds| {
                let sub = export::sub_document(self.doc(), &scope);
                let (view, w, h) = panel
                    .picture_view(&bounds)
                    .context("there is no place for the picture")?;
                let rgba = self.render_sub(&sub, &view, w, h)?;
                let gfx = self
                    .gfx
                    .as_mut()
                    .context("there is no window to draw with")?;
                gfx.upload_picture(&Bitmap { w, h, rgba })
            });
        let Some(sending) = self.sending.as_mut() else {
            return;
        };
        sending.pictured = Some(want);
        match taken {
            Ok(slot) => sending.picture = Some(slot),
            Err(e) => log::warn!("the dialog's picture: {e:#}"),
        }
    }

    /// The bytes behind every image the sub-document names, so the json
    /// stands on its own where it lands.
    fn blobs_of(&self, sub: &Document) -> Vec<(String, Vec<u8>)> {
        sub.elements
            .iter()
            .filter_map(|e| match e {
                Element::Image(i) => Some(i.blob.clone()),
                _ => None,
            })
            .filter_map(|hash| Some((hash.clone(), self.store.read_blob(&hash).ok()?)))
            .collect()
    }

    /// The three an agent asks (§8), answered here because the live
    /// document and the GPU are both the loop's. A failure comes back as
    /// `denied` carrying the whole chain of it: the caller is a program,
    /// and a program cannot read a log.
    fn answer(&mut self, req: Request) -> Event {
        let op = req.op();
        let denied = |e: &anyhow::Error| Event::Denied {
            op: op.to_owned(),
            reason: format!("{e:#}"),
        };
        match req {
            Request::Frames => Event::Frames {
                frames: export::frames(self.doc()),
            },
            Request::ReadFrame { id, dir } => match self.read_frame(&id, &dir) {
                Ok(files) => Event::Exported {
                    files: files
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                },
                Err(e) => denied(&e),
            },
            Request::AddFrame { path } => match self.add_frame(&path) {
                Ok((id, name)) => Event::Framed { id, name },
                Err(e) => denied(&e),
            },
            Request::Layers => Event::Layers {
                layers: self.editor().listing(self.doc()),
            },
            Request::Texts => Event::Texts {
                texts: export::texts(self.doc()),
            },
            Request::AddText { .. } | Request::SetText { .. } => match self.text_op(req) {
                Ok((id, layer)) => Event::Texted { id, layer },
                Err(e) => denied(&e),
            },
            // What is acked and forwarded never arrives here; every other
            // op is one on the layers.
            Request::Ping
            | Request::New
            | Request::Open { .. }
            | Request::OpenFile { .. }
            | Request::Raise
            | Request::Export { .. }
            | Request::Theme { .. }
            | Request::Shutdown => Event::Denied {
                op: op.to_owned(),
                reason: "not an op the board answers".into(),
            },
            layer => match self.layer_op(layer) {
                Ok(ids) => Event::Done { ids },
                Err(e) => denied(&e),
            },
        }
    }

    /// A text added or changed from the command line: one step of the
    /// history, the text fitted and its layer named as typing would. A
    /// new one stands in the frame named — its place counted from the
    /// frame's corner, a little in from it when none is given — or on
    /// the open board, where the window is looking when no place is.
    fn text_op(&mut self, req: Request) -> anyhow::Result<(String, String)> {
        self.end_typing();
        match req {
            Request::AddText { frame, spec } => {
                let doc = self.doc();
                let (born, corner, inset) = match &frame {
                    Some(id) => {
                        let listed = export::frames(doc);
                        anyhow::ensure!(
                            listed.iter().any(|c| c.id == *id),
                            "no frame {id:?} on show on the board that is open; `sinopia agent frames` lists them"
                        );
                        let f = doc.frame(id).context("the frame listed is on the board")?;
                        anyhow::ensure!(!doc.locked(&f.layer), "frame {id:?} is locked, and a lock keeps what it holds");
                        (Some(f.layer.clone()), (f.x, f.y), TEXT_INSET)
                    }
                    None => {
                        let middle = self.view().map_or((0.0, 0.0), |v| {
                            v.screen_to_world(f64::from(v.viewport.w) / 2.0, f64::from(v.viewport.h) / 2.0)
                        });
                        (None, middle, 0.0)
                    }
                };
                let mut style = self.editor().next_style();
                style.color = self.ink_hex().to_owned();
                let mut text = crate::doc::Text {
                    id: String::new(),
                    layer: String::new(),
                    x: inset,
                    y: inset,
                    w: 0.0,
                    h: 0.0,
                    rotation: 0.0,
                    mode: if spec.w.is_some() {
                        crate::doc::TextMode::Frame
                    } else {
                        crate::doc::TextMode::Artistic
                    },
                    text: String::new(),
                    style,
                    runs: Vec::new(),
                };
                spec.apply(&mut text);
                text.style.checked().map_err(anyhow::Error::msg)?;
                text.x += corner.0;
                text.y += corner.1;
                if text.mode == crate::doc::TextMode::Frame {
                    if text.w <= 0.0 {
                        text.w = TEXT_FRAME_W;
                    }
                    // A frame with no height is as tall as what it holds.
                    if spec.h.is_none() {
                        let laid = crate::typeset::lay(&text.text, &text.style, text.mode, text.w, f64::MAX, &self.fonts);
                        text.h = laid.content;
                    }
                }
                let placed = self.with_text(|e, d, f| e.place_text(d, f, text, born.as_deref()));
                self.apply(Change::Scene);
                Ok(placed)
            }
            Request::SetText { id, spec } => {
                // A stretch is measured in what the text says once the
                // change lands: past its end is a stretch of nothing.
                if let Some((_, end)) = spec.range {
                    let said = match &spec.text {
                        Some(words) => words.chars().count(),
                        None => self
                            .doc()
                            .elements
                            .iter()
                            .find_map(|el| match el {
                                crate::doc::Element::Text(t) if t.id == id => Some(t.text.chars().count()),
                                _ => None,
                            })
                            .unwrap_or(0),
                    };
                    anyhow::ensure!(end <= said, "the range runs to {end}, and the text is {said} characters long");
                }
                let change = self
                    .with_text(|e, d, f| e.change_text(d, f, &id, |t| spec.apply(t)))
                    .map_err(anyhow::Error::msg)?;
                self.apply(change);
                let layer = self
                    .doc()
                    .elements
                    .iter()
                    .find(|el| el.id() == id)
                    .map(|el| el.layer().to_owned())
                    .unwrap_or_default();
                Ok((id, layer))
            }
            _ => anyhow::bail!("not a text op"),
        }
    }

    /// A change to the layers asked for on the command line, made the
    /// way the panel makes it — the layers named are picked, then acted
    /// on — and one step of the history. Answers what it left picked. A
    /// refusal says why and changes nothing but the pick.
    fn layer_op(&mut self, req: Request) -> anyhow::Result<Vec<String>> {
        // What the command line changes is a step of its own, never folded
        // into the text the person is typing.
        self.end_typing();
        let change = match req {
            Request::AddLayer { group, name, above } => {
                let (editor, doc) = self.active();
                editor
                    .add_layer_as(doc, group, name.as_deref(), above.as_deref())
                    .map_err(anyhow::Error::msg)?
            }
            Request::RemoveLayers { ids } => {
                self.pick(&ids)?;
                self.refusing(Command::Remove, "there is nothing there to remove")?
            }
            Request::RenameLayer { id, name } => {
                anyhow::ensure!(!name.trim().is_empty(), "a name has to say something");
                self.pick(std::slice::from_ref(&id))?;
                let (editor, doc) = self.active();
                editor.rename_layer(doc, &id, &name)
            }
            Request::MoveLayers { ids, to } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                let (Place::Into(target) | Place::Above(target) | Place::Below(target)) = &to;
                anyhow::ensure!(doc.layer(target).is_some(), "no layer {target:?} on the board");
                let fits = doc
                    .place(&to)
                    .is_some_and(|(owner, _)| doc.can_move(&ids, owner));
                anyhow::ensure!(
                    fits,
                    "they cannot go there: a frame stays on the board's root, nothing goes into a locked \
                     group or into itself, and only a group or a frame takes layers in"
                );
                editor.drop_layers(doc, &to)
            }
            Request::ArrangeLayers { ids, how } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                if let Some(id) = locked_among(doc, &ids, Keeps::Place) {
                    anyhow::bail!("{id:?} stands in a locked group, and a lock keeps its place");
                }
                editor.run(doc, Command::Arrange(how))
            }
            Request::ShowLayers { ids, visible } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                editor.show_layers(doc, visible)
            }
            Request::LockLayers { ids, locked } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                editor.lock_layers(doc, locked)
            }
            Request::SetOpacity { ids, opacity } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                if let Some(id) = locked_among(doc, &ids, Keeps::Look) {
                    anyhow::bail!("{id:?} is locked, and a lock keeps how a layer draws");
                }
                editor.set_opacity(doc, opacity)
            }
            Request::SetBlend { ids, blend } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                if let Some(id) = locked_among(doc, &ids, Keeps::Look) {
                    anyhow::bail!("{id:?} is locked, and a lock keeps how a layer draws");
                }
                if blend == BlendMode::PassThrough
                    && let Some(id) = ids.iter().find(|id| doc.layer(id).is_some_and(|l| l.kind != crate::doc::Kind::Group))
                {
                    anyhow::bail!("{id:?} is no group, and only a group passes through");
                }
                editor.set_blend(doc, blend)
            }
            Request::SetColor { ids, color } => {
                self.pick(&ids)?;
                let (editor, doc) = self.active();
                editor.set_tag(doc, &ids[0], color)
            }
            Request::GroupLayers { ids } => {
                self.pick(&ids)?;
                self.refusing(
                    Command::Group,
                    "they cannot be grouped: a frame never goes into a group, and a locked group takes nothing in",
                )?
            }
            Request::Ungroup { ids } => {
                let mut left = Vec::new();
                for id in &ids {
                    self.pick(std::slice::from_ref(id))?;
                    let _ = self.refusing(
                        Command::Ungroup,
                        &format!("{id:?} does not come apart: only a group does, and never one that is locked"),
                    )?;
                    left.extend(self.editor().picked(self.doc()).into_iter().map(str::to_owned));
                }
                let (editor, doc) = self.active();
                editor.pick_ids(doc, &left).map_err(anyhow::Error::msg)?;
                Change::Scene
            }
            Request::DuplicateLayers { ids } => {
                self.pick(&ids)?;
                self.refusing(Command::Duplicate, "nothing inside a locked group is duplicated where it stands")?
            }
            Request::MergeLayers { ids } => {
                self.pick(&ids)?;
                let lone = ids.len() == 1
                    && self
                        .doc()
                        .layer(&ids[0])
                        .is_some_and(|l| l.kind != crate::doc::Kind::Group);
                anyhow::ensure!(!lone, "one layer alone merges down, or not at all: ask merge_down");
                self.merging(Command::Merge)?
            }
            Request::MergeDown { id } => {
                self.pick(std::slice::from_ref(&id))?;
                self.merging(Command::MergeDown)?
            }
            Request::MergeVisible => self.merging(Command::MergeVisible)?,
            Request::Flatten => self.merging(Command::Flatten)?,
            Request::SelectLayers { ids } => {
                self.pick(&ids)?;
                Change::Selection
            }
            Request::OpenLayers { ids, open } => {
                let (editor, doc) = self.active();
                for id in &ids {
                    let holds = doc
                        .layer(id)
                        .is_some_and(|l| matches!(l.kind, crate::doc::Kind::Group | crate::doc::Kind::Frame));
                    anyhow::ensure!(holds, "{id:?} is no group or frame to open");
                }
                for id in &ids {
                    let _ = editor.set_open(id, open);
                }
                Change::Selection
            }
            other => anyhow::bail!("{} is not an op on the layers", other.op()),
        };
        self.apply(change);
        self.redraw();
        Ok(self.editor().picked(self.doc()).into_iter().map(str::to_owned).collect())
    }

    /// Picks exactly `ids` on the active board, or says which is missing.
    fn pick(&mut self, ids: &[String]) -> anyhow::Result<()> {
        let (editor, doc) = self.active();
        editor.pick_ids(doc, ids).map_err(anyhow::Error::msg)
    }

    /// Runs `command` on the pick, refusing with `why` what it cannot do.
    fn refusing(&mut self, command: Command, why: &str) -> anyhow::Result<Change> {
        let (editor, doc) = self.active();
        anyhow::ensure!(editor.can(doc, command), "{why}");
        Ok(editor.run(doc, command))
    }

    /// A merge, pictures and all, refused when there is nothing it merges.
    fn merging(&mut self, command: Command) -> anyhow::Result<Change> {
        let can = self.editor().can(self.doc(), command);
        anyhow::ensure!(
            can,
            "nothing merges there: only siblings do, never a frame, nothing locked, and nothing into a group"
        );
        self.try_merge(command)
    }

    /// One frame of the open board, written under `dir` as the page §8
    /// describes. The folder is the frame's own name slugged, exactly as
    /// `Ctrl+E` writes it, so two frames sharing a name share a folder
    /// and re-reading one replaces its page rather than stacking up.
    fn read_frame(&mut self, id: &str, dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
        // Through the listing, which walks `painted()`: a frame on a
        // layer the person hid is not on the board an agent can see, and
        // reading it would answer `exported` over a blank page. The two
        // doors must agree about what is there.
        let card = export::frames(self.doc())
            .into_iter()
            .find(|c| c.id == id)
            .with_context(|| format!("no frame {id:?} on the board that is open"))?;
        let scope = export::Scope::Frame(card.id);
        // Named the way `Ctrl+E` names it — its layer's name, or its id
        // where the name slugs away to nothing, as every non-Latin name
        // does — so a frame keeps one page whichever door it leaves by.
        let slug = export::page_slug(self.doc(), &scope, &self.project().label(), &[]);
        self.write_page(&scope, dir, &slug)
    }

    /// A frame an agent handed over, on the board. It lands as a scene
    /// change like any other, so it is one undo step and a draft owes
    /// the disk a save for it.
    fn add_frame(&mut self, path: &Path) -> anyhow::Result<(String, String)> {
        self.end_typing();
        let mut fragment = read_fragment(path)?;
        // An agent sets text it cannot measure: its artistic boxes are
        // fitted here, before the board picks the spot by them.
        crate::editor::fit_texts(&mut fragment, &self.fonts);
        // Worked out before anything is committed: the bytes below go
        // into the store on the way in, and a fragment refused after
        // that would leave images there that nothing on the board names.
        let planned = graft::planned(self.active().1, &fragment)?;
        // Then the bytes: an image whose blob nobody has would paint a
        // placeholder for as long as the board lives.
        self.keep_blobs(&fragment, path)?;
        let planted = graft::apply(self.active().1, planned);
        // The same door a reopened board goes through: bytes in the
        // store are not a texture, and an image nobody uploaded paints
        // a placeholder for as long as the board is open.
        self.load_images();
        self.apply(Change::Scene);
        Ok(planted)
    }

    /// Every image the fragment names, in the store before its frame is
    /// on the board. Already there, nothing to do; otherwise the bytes
    /// beside the json it arrived in, checked the way a paste is — the
    /// header before a texel is allocated — and content-addressed, so
    /// bytes that are not what they are named by never land.
    fn keep_blobs(&self, fragment: &Document, from: &Path) -> anyhow::Result<()> {
        let beside = from.parent().unwrap_or(Path::new(".")).join("blobs");
        for el in &fragment.elements {
            let Element::Image(i) = el else { continue };
            // Checked before it is a path, and before it is anything
            // else: 64 hex characters can only ever name a file directly
            // inside `blobs/` (§9.3).
            anyhow::ensure!(crate::doc::is_blob_hash(&i.blob), "not a blob name: {:?}", i.blob);
            if self.store.read_blob(&i.blob).is_ok() {
                continue;
            }
            let at = beside.join(&i.blob);
            // Capped on the way in, the way a paste is and at the same
            // ceiling: a bare `read` of a path an agent named would
            // follow a 200 MB file — or a `/dev/zero` — into the loop
            // thread's memory before either guard below could refuse it.
            let bytes = store::read_capped(&at, clipboard::MAX_PASTE_BYTES).with_context(|| {
                format!(
                    "image {} is neither in the store nor at {at:?}",
                    &i.blob[..8]
                )
            })?;
            anyhow::ensure!(
                store::sha256_hex(&bytes) == i.blob,
                "the bytes at {at:?} are not the image they are named by"
            );
            // The header, not the pixels: this asks whether the bytes
            // are an image the board could draw, and `load_images` is
            // what actually decodes them.
            bitmap::checked_size(&bytes).with_context(|| format!("image {}", &i.blob[..8]))?;
            self.store.write_blob(&bytes)?;
        }
        Ok(())
    }

    /// A key that adjusts the brush, while the brush tool is selected:
    /// `[` `]` size, `{` `}` hardness, a digit the opacity. True if it
    /// was one.
    fn brush_key(&mut self, c: char) -> bool {
        if self.editor().tool() != Tool::Brush {
            return false;
        }
        match c {
            '[' => self.brushes.brush_mut().shrink(),
            ']' => self.brushes.brush_mut().grow(),
            '{' => self.brushes.brush_mut().softer(),
            '}' => self.brushes.brush_mut().harder(),
            '0'..='9' => self.brushes.brush_mut().set_opacity_digit(c as u8 - b'0'),
            _ => return false,
        }
        // A key is a whole gesture on its own: it is kept at once.
        self.brushes_dirty = true;
        self.keep_brushes();
        true
    }

    /// What a text on the board is drawn with.
    fn letters(&self) -> scene::Letters<'_> {
        scene::Letters {
            fonts: &self.fonts,
            glyphs: &self.glyphs,
            slot: self.letters_slot,
        }
    }

    /// Runs `f` on the tab's editor and board with the faces in hand: the
    /// three live in different parts of the window, and a text needs all
    /// of them at once.
    fn with_text<R>(
        &mut self,
        f: impl FnOnce(&mut Editor, &mut Document, &crate::fonts::Fonts) -> R,
    ) -> R {
        let Open { project, editor, .. } = &mut self.open[self.active];
        f(editor, &mut project.doc, &self.fonts)
    }

    /// Leaves the text being typed, when one is, and writes down what
    /// leaving did: a text that says nothing goes with its layer.
    fn end_typing(&mut self) {
        let (editor, doc) = self.active();
        let change = editor.end_typing(doc);
        self.apply(change);
    }

    /// The hand did something to the text being typed: the caret shows,
    /// solid, from now.
    fn text_input(&mut self) {
        self.typed_at = Instant::now();
    }

    /// Whether the caret is showing this frame: solid for a moment after
    /// the hand, then on and off.
    fn caret_on(&self) -> bool {
        (self.typed_at.elapsed().as_millis() / BLINK.as_millis()).is_multiple_of(2)
    }

    /// When the caret next turns on or off, while a text is being typed.
    fn next_blink(&self) -> Option<Instant> {
        self.editor().typing()?;
        let n = self.typed_at.elapsed().as_millis() / BLINK.as_millis() + 1;
        Some(self.typed_at + BLINK * n as u32)
    }

    /// The text's own bar: with the Text tool in hand, and wherever a
    /// text is being typed or is selected — with any tool but the brush,
    /// whose bar stands in the same place.
    fn text_bar(&self, view: &View) -> Option<TextBar> {
        let editor = self.editor();
        let shown = editor.tool() == Tool::Text || editor.text_targeted(self.doc());
        if !shown || editor.tool() == Tool::Brush {
            return None;
        }
        Some(TextBar::layout(
            view.viewport,
            self.chrome(view),
            self.strip_top(view),
            self.text_bar_open,
        ))
    }

    /// A click on the text bar.
    fn text_bar_hit(&mut self, bar: &TextBar, hit: textbar::Hit, x: f64) {
        let style = self.editor().text_style(self.doc());
        let change = match hit {
            textbar::Hit::Kind(mode) => self.with_text(|e, d, f| e.set_text_kind(mode, d, f)),
            textbar::Hit::Font => {
                self.open_font_menu(bar.font);
                return;
            }
            textbar::Hit::Toggle(t) => {
                let on = !t.on(&style);
                self.with_text(|e, d, f| e.restyle(d, f, |s| t.set(s, on)))
            }
            textbar::Hit::Align(a) => self.with_text(|e, d, f| e.restyle(d, f, |s| s.align = a)),
            textbar::Hit::Valign(v) => self.with_text(|e, d, f| e.restyle(d, f, |s| s.valign = v)),
            textbar::Hit::Slider(slider) => {
                self.text_grab = Some(slider);
                self.drag_text_field(bar, slider, x);
                return;
            }
            textbar::Hit::More => {
                self.text_bar_open = !self.text_bar_open;
                Change::Selection
            }
            textbar::Hit::Bar => Change::None,
        };
        self.apply(change);
    }

    /// A text slider follows the pointer's x.
    fn drag_text_field(&mut self, bar: &TextBar, slider: textbar::Slider, x: f64) {
        let f = bar.fraction(slider, x);
        let change = self.with_text(|e, d, fonts| e.restyle(d, fonts, |s| slider.set(s, f)));
        self.apply(change);
    }

    /// The shape's own bar: with the Shape tool in hand, and wherever a
    /// shape or a line is selected — with any tool but the brush, whose
    /// bar stands in the same place, and never over the text's.
    fn shape_bar(&self, view: &View) -> Option<ShapeBar> {
        let editor = self.editor();
        let doc = self.doc();
        let shown = editor.tool() == Tool::Shape || editor.shape_targeted(doc);
        if !shown || editor.tool() == Tool::Brush || self.text_bar(view).is_some() {
            return None;
        }
        Some(ShapeBar::layout(
            view.viewport,
            self.chrome(view),
            self.strip_top(view),
            editor.shape_look(doc, self.ink_hex()).figure,
        ))
    }

    /// A click on the shape bar.
    fn shape_bar_hit(&mut self, bar: &ShapeBar, hit: shapebar::Hit, x: f64) {
        let change = match hit {
            shapebar::Hit::Figure(figure) => {
                let (editor, doc) = self.active();
                editor.set_figure(figure, doc)
            }
            shapebar::Hit::Fill => return self.open_paint_menu(bar.fill, false),
            shapebar::Hit::Stroke => return self.open_paint_menu(bar.stroke, true),
            shapebar::Hit::Head(end) => {
                if let Some((_, at)) = bar.heads.iter().find(|(e, _)| *e == end) {
                    self.open_head_menu(end, *at);
                }
                return;
            }
            shapebar::Hit::Slider(slider) => {
                self.shape_grab = Some(slider);
                self.drag_shape_field(bar, slider, x);
                return;
            }
            shapebar::Hit::Bar => Change::None,
        };
        self.apply(change);
    }

    /// A shape slider follows the pointer's x.
    fn drag_shape_field(&mut self, bar: &ShapeBar, slider: shapebar::Slider, x: f64) {
        let f = bar.fraction(slider, x);
        let (editor, doc) = self.active();
        let change = editor.restyle_shapes(doc, slider.at(f));
        self.apply(change);
    }

    /// Opens the dock's inks under the fill's well or the stroke's, on
    /// the paint the bar shows, remembering what every shape it is
    /// looking at wore.
    fn open_paint_menu(&mut self, at: ScreenRect, stroke: bool) {
        let doc = self.doc();
        let editor = self.editor();
        let look = editor.shape_look(doc, self.ink_hex());
        let (current, none) = match stroke {
            true => (look.stroke.as_deref(), "No Stroke"),
            false => (look.fill.as_deref(), "No Fill"),
        };
        // A line is nothing but its ink, and cannot go without it.
        let can_none = !stroke || look.figure.model().is_some();
        let (items, paints) = shapebar::paint_menu(current, none, can_none);
        let was = editor.paints(doc, stroke);
        self.menu = Some(Opened {
            purpose: Purpose::Paint { stroke, paints, was },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// Opens the heads under a line's well at `end`, on the one it wears.
    fn open_head_menu(&mut self, end: select::End, at: ScreenRect) {
        let look = self.editor().shape_look(self.doc(), self.ink_hex());
        let current = match end {
            select::End::From => look.start,
            select::End::To => look.end,
        };
        let (items, heads) = shapebar::head_menu(current);
        self.menu = Some(Opened {
            purpose: Purpose::Head { end, heads },
            items,
            at,
            hover: None,
            scroll: 0.0,
        });
        self.redraw();
    }

    /// Opens the families under the text bar's button, on the one the
    /// bar is showing, remembering what every text it is looking at was
    /// set in.
    fn open_font_menu(&mut self, at: ScreenRect) {
        let doc = self.doc();
        let editor = self.editor();
        let current = editor.text_style(doc).font;
        let families: Vec<String> = self.fonts.families().to_vec();
        let items: Vec<menu::Item> = families
            .iter()
            .map(|f| menu::Item::new(f).checked(*f == current))
            .collect();
        let was = editor.text_look(doc);
        let chrome = self.view().map_or(1.0, |v| self.chrome(&v)) as f32;
        let at_line = families.iter().position(|f| *f == current).unwrap_or(0);
        self.menu = Some(Opened {
            purpose: Purpose::Font { families, was },
            items,
            at,
            hover: None,
            // The family in use stands a few lines from the top.
            scroll: ((at_line as f32 - 3.0) * menu::ITEM * chrome).max(0.0),
        });
        self.redraw();
    }

    /// A key while the font menu stands: `Esc` puts it away, `Enter`
    /// takes the family lit, the arrows walk the lines, and letters look
    /// for a family by its name — each tried on the text as it is lit.
    fn font_menu_key(&mut self, key: &Key, text: Option<&str>) {
        let Some(opened) = self.menu.as_ref() else { return };
        let (n, hover) = (opened.items.len(), opened.hover);
        let target = match key {
            Key::Named(NamedKey::Escape) => return self.close_menu(None),
            Key::Named(NamedKey::Enter) => return self.close_menu(hover),
            Key::Named(NamedKey::ArrowDown) => Some(hover.map_or(0, |h| (h + 1).min(n.saturating_sub(1)))),
            Key::Named(NamedKey::ArrowUp) => Some(hover.map_or(0, |h| h.saturating_sub(1))),
            Key::Named(NamedKey::Space) | Key::Character(_) => {
                let typed = match key {
                    Key::Character(c) => text.unwrap_or(c).to_owned(),
                    _ => " ".to_owned(),
                };
                let (was, at) = &self.menu_typed;
                let mut name = if at.elapsed() < DOUBLE_CLICK * 2 { was.clone() } else { String::new() };
                name.push_str(&typed);
                self.menu_typed = (name.clone(), Instant::now());
                menu::find(&opened.items, &name)
            }
            _ => None,
        };
        let Some(i) = target else { return };
        let scroll = self
            .view()
            .and_then(|view| self.menu_laid(&view))
            .zip(self.menu.as_ref())
            .map(|(laid, opened)| laid.scroll_showing(&opened.items, i));
        if let (Some(opened), Some(scroll)) = (self.menu.as_mut(), scroll) {
            opened.scroll = scroll;
        }
        self.hover_menu(Some(i));
    }

    /// `Ctrl` with a letter while a text is being typed: the text's own
    /// commands, and the window's for the rest.
    fn typing_command(&mut self, c: &str, shift: bool) -> bool {
        use crate::editor::TextKey;
        let key = c.to_ascii_lowercase();
        let change = match (key.as_str(), shift) {
            ("a", false) => self.with_text(|e, d, f| e.text_key(TextKey::SelectAll, d, f)),
            ("z", false) => self.with_text(|e, d, f| e.text_key(TextKey::Undo, d, f)),
            ("z", true) | ("y", false) => self.with_text(|e, d, f| e.text_key(TextKey::Redo, d, f)),
            ("c", false) => {
                self.copy_text();
                Change::None
            }
            ("x", false) => {
                let (text, change) = self.with_text(|e, d, f| e.cut_text(d, f));
                if let (Some(text), Some(clipboard)) = (text, &self.clipboard) {
                    clipboard.copy_text(&text);
                }
                change
            }
            ("v", false) => {
                self.paste_text();
                Change::None
            }
            // Bold, italic and underline, as every text box has them.
            ("b", false) | ("i", false) | ("u", false) => {
                let t = match key.as_str() {
                    "b" => textbar::Toggle::Bold,
                    "i" => textbar::Toggle::Italic,
                    _ => textbar::Toggle::Underline,
                };
                let on = !t.on(&self.editor().text_style(self.doc()));
                self.with_text(|e, d, f| e.restyle(d, f, |s| t.set(s, on)))
            }
            // Adobe's: the alignment under Ctrl+Shift, and the size a
            // step up or down with the angle brackets.
            ("l", true) | ("c", true) | ("r", true) | ("j", true) => {
                let a = match key.as_str() {
                    "l" => crate::doc::Align::Left,
                    "c" => crate::doc::Align::Center,
                    "r" => crate::doc::Align::Right,
                    _ => crate::doc::Align::Justify,
                };
                self.with_text(|e, d, f| e.restyle(d, f, |s| s.align = a))
            }
            (">" | ".", true) | ("<" | ",", true) => {
                let up = matches!(key.as_str(), ">" | ".");
                self.with_text(|e, d, f| {
                    e.restyle(d, f, |s| s.size = crate::editor::step_size(s.size, up))
                })
            }
            _ => return false,
        };
        self.text_input();
        self.apply(change);
        true
    }

    /// `Ctrl+C` in a text: what is selected in it goes to the clipboard.
    fn copy_text(&self) {
        if let (Some(text), Some(clipboard)) = (self.editor().copied_text(), &self.clipboard) {
            clipboard.copy_text(&text);
        }
    }

    /// A key while a text is being typed: it takes the keyboard whole,
    /// as a field does.
    fn typing_key(&mut self, key: &Key, text: Option<&str>) {
        use crate::editor::{Move, TextKey};
        let mods = self.modifiers.state();
        let (ctrl, shift) = (mods.control_key(), mods.shift_key());
        let go = |m: Move| Some(TextKey::Go(m, shift));
        let text_key = match key {
            Key::Named(NamedKey::Escape) => {
                self.end_typing();
                return self.redraw();
            }
            // Ctrl+Enter leaves the text, as Esc does: Enter alone is a
            // new line.
            Key::Named(NamedKey::Enter) if ctrl => {
                self.end_typing();
                return self.redraw();
            }
            Key::Named(NamedKey::Enter) => Some(TextKey::Newline),
            Key::Named(NamedKey::Backspace) if ctrl => Some(TextKey::WordBackspace),
            Key::Named(NamedKey::Backspace) => Some(TextKey::Backspace),
            Key::Named(NamedKey::Delete) if ctrl => Some(TextKey::WordDelete),
            Key::Named(NamedKey::Delete) => Some(TextKey::Delete),
            Key::Named(NamedKey::ArrowLeft) if ctrl => go(Move::WordLeft),
            Key::Named(NamedKey::ArrowLeft) => go(Move::Left),
            Key::Named(NamedKey::ArrowRight) if ctrl => go(Move::WordRight),
            Key::Named(NamedKey::ArrowRight) => go(Move::Right),
            Key::Named(NamedKey::ArrowUp) => go(Move::Up),
            Key::Named(NamedKey::ArrowDown) => go(Move::Down),
            Key::Named(NamedKey::Home) if ctrl => go(Move::Home),
            Key::Named(NamedKey::Home) => go(Move::LineHome),
            Key::Named(NamedKey::End) if ctrl => go(Move::End),
            Key::Named(NamedKey::End) => go(Move::LineEnd),
            Key::Named(NamedKey::Space) if !ctrl => {
                let change = self.with_text(|e, d, f| e.type_str(" ", d, f));
                self.text_input();
                return self.apply(change);
            }
            Key::Character(c) if ctrl => {
                if !self.typing_command(c, shift) {
                    // Anything else under Ctrl is the window's; the text is
                    // left first unless it is a save, which keeps it open.
                    let action = menubar::shortcut(&c.to_ascii_lowercase(), shift, mods.alt_key());
                    if let Some(action) = action {
                        self.run_action(action);
                    }
                }
                return;
            }
            Key::Character(c) if !mods.super_key() => {
                let typed = text.unwrap_or(c).to_owned();
                let change = self.with_text(|e, d, f| e.type_str(&typed, d, f));
                self.text_input();
                return self.apply(change);
            }
            _ => None,
        };
        if let Some(k) = text_key {
            let change = self.with_text(|e, d, f| e.text_key(k, d, f));
            self.text_input();
            self.apply(change);
        }
    }

    /// How many presses in a row a press at `(x, y)` makes: one more than
    /// the last when it lands soon enough after it and near enough to it.
    fn count_clicks(&mut self, x: f64, y: f64) -> u32 {
        let now = Instant::now();
        let reach = CLICK_REACH * self.view().map_or(1.0, |v| v.scale);
        let n = match self.last_press {
            Some((at, (px, py), n))
                if now.duration_since(at) < DOUBLE_CLICK && (x - px).hypot(y - py) <= reach =>
            {
                n % 3 + 1
            }
            _ => 1,
        };
        self.last_press = Some((now, (x, y), n));
        n
    }

    /// Starts the glyph sheet over when a frame found it full, and says
    /// so: the glyphs that did not fit were left out of what was just
    /// drawn, and the next frame rasterizes what it needs into a clean
    /// sheet.
    ///
    /// A frame that fills a sheet it had just started over with needs
    /// more letters than a sheet holds at the size they are seen at: the
    /// sheet's ceiling comes down, so the next frame rasterizes them
    /// smaller and they fit — rather than filling it again forever. A
    /// frame whose letters fit with room to spare gives the ceiling back.
    fn start_letters_over(&self) -> bool {
        let full = self.glyphs.is_full();
        if full {
            if self.letters_restarted.get() {
                self.glyphs.squeeze();
            }
            self.glyphs.clear();
        } else {
            self.glyphs.relax();
        }
        self.letters_restarted.set(full);
        full
    }

    /// The four image sheets the binary ships: illustrated dock tools,
    /// brush icons for the library, the agents' logos for the export
    /// dialog, and nib shapes for the canvas. They are raster art, the
    /// same at every scale, so each is uploaded once into a slot of its
    /// own and never replaced like the glyph atlas is.
    fn ensure_sheets(&mut self) {
        if self.dock_icon_slot.is_none() {
            const DOCK_ICONS: &[u8] = include_bytes!("../assets/dock/icons.png");
            // A failed upload leaves the pure dock's line-art fallback.
            self.upload_sheet(
                "dock icons",
                DOCK_ICONS,
                Gfx::upload_dock_icons,
                |app, slot| app.dock_icon_slot = Some(slot),
            );
        }
        if self.icon_slot == 0 {
            const ICONS: &[u8] = include_bytes!("../assets/brushes/icons.png");
            // Without it the palette draws no icons and the grid is
            // bare; the names and the preview's dab still say what is
            // what.
            self.upload_sheet("brush icons", ICONS, Gfx::upload_icons, |app, slot| {
                app.icon_slot = slot;
            });
        }
        if self.agent_logo_slot.is_none() {
            const LOGOS: &[u8] = include_bytes!("../assets/agents/logos.png");
            // Without it a row keeps its logo's place and says the rest.
            self.upload_sheet(
                "agent logos",
                LOGOS,
                Gfx::upload_agent_logos,
                |app, slot| app.agent_logo_slot = Some(slot),
            );
        }
        if self.shapes.cells.is_empty() {
            const SHAPES: &[u8] = include_bytes!("../assets/brushes/shapes.png");
            // Without it every brush lays a plain round nib, which is
            // what two thirds of them lay anyway.
            self.upload_sheet("nib shapes", SHAPES, Gfx::upload_shapes, |app, slot| {
                app.shapes = app.brushes.sheet(slot);
            });
        }
    }

    /// Decodes one shipped sheet and hands the slot it landed in to
    /// `kept`. A sheet that will not decode or upload is logged and
    /// left out: the window is worth more than the art.
    fn upload_sheet(
        &mut self,
        what: &str,
        bytes: &[u8],
        upload: fn(&mut Gfx, &Bitmap) -> anyhow::Result<u32>,
        kept: fn(&mut App, u32),
    ) {
        let bmp = match bitmap::decode(bytes) {
            Ok(b) => b,
            Err(e) => return log::error!("{what} did not decode: {e}"),
        };
        let Some(gfx) = &mut self.gfx else { return };
        match upload(gfx, &bmp) {
            Ok(slot) => kept(self, slot),
            Err(e) => log::error!("{what} did not upload: {e}"),
        }
    }

    /// Builds and uploads the glyph atlas for the current scale factor,
    /// unless the one in hand already matches.
    fn ensure_atlas(&mut self) {
        let Some(window) = &self.window else { return };
        let px = Tabs::label_px(
            self.style.text_px().unwrap_or(tabs::LABEL),
            window.scale_factor(),
        );
        if self.atlas.as_ref().is_some_and(|a| a.px() == px) {
            return;
        }
        let atlas = Atlas::build(&self.font, px);
        let Some(gfx) = &mut self.gfx else { return };
        match gfx.upload_atlas(&atlas.bitmap) {
            Ok(slot) => {
                self.atlas_slot = slot;
                self.atlas = Some(atlas);
            }
            // Without the atlas the strip draws no labels; the tabs are
            // still there, and so is everything else.
            Err(e) => log::error!("uploading the glyph atlas: {e:#}"),
        }
    }

    /// Everything on screen, back to front: grid, document, the stroke in
    /// progress, the selection frame and marquee, the brush's ring, the
    /// dock, the layers handle and panel, the strip.
    fn frame(&self, view: &View) -> Frame {
        // Before the window exists there are no textures, so every image
        // is a placeholder — which is what an empty map says.
        let none = ImageSlots::new();
        let images = self.gfx.as_ref().map_or(&none, Gfx::image_slots);
        let mut frame = Frame::new();
        frame.extend(grid::prims(view, self.theme.dot));
        // The stroke in progress goes into the document's own frame,
        // on the layer it is going to land on: that is where it meets
        // the ink already there, and the only place an eraser has
        // anything to rub out.
        let live = self.editor().stroke().map(|stroke| scene::Live {
            layer: self.editor().live_layer(self.doc()).unwrap_or_default(),
            prims: scene::stroke_prims(
                &stroke.points,
                &stroke.tip,
                &stroke.envelope(),
                self.ink_rgba(),
                view,
                &self.shapes,
            ),
            tip: &stroke.tip,
        });
        let edge = self.theme.muted;
        frame.append(scene::document_prims(
            self.doc(),
            view,
            images,
            &self.shapes,
            &self.letters(),
            edge,
            live,
        ));
        // What the Shape tool is dragging out, drawn as it will land.
        match self.editor().shaping(view, self.ink_hex()) {
            Some(Element::Shape(shape)) => frame.extend(scene::shape_prims(&shape, view)),
            Some(Element::Line(line)) => frame.extend(scene::line_prims(&line, view)),
            _ => {}
        }
        // A lone line wears its ends; anything else, its frame.
        if let Some(line) = self.editor().lone_line(self.doc()) {
            frame.extend(select::end_prims(line, view, &self.theme));
        } else if let Some(selection) = self.editor().selection_frame(self.doc()) {
            frame.extend(select::prims(&selection, view, &self.theme));
        }
        if let Some((a, b)) = self.editor().marquee() {
            frame.extend(select::marquee_prims(a, b, &self.theme));
        }
        // What the text being typed shows over it: its box, what is
        // selected, and the caret while it is on.
        let caret = self.caret_on();
        self.caret_drawn.set(self.editor().typing().map(|_| caret));
        frame.extend(self.editor().typing_prims(self.doc(), view, &self.theme, caret));
        // The area a press with the Text tool is dragging out.
        if let Some((from, to)) = self.editor().placing() {
            let a = view.world_to_screen(from[0], from[1]);
            let b = view.world_to_screen(to[0], to[1]);
            frame.extend(select::marquee_prims(a, b, &self.theme));
        }
        // The area the Frame tool is dragging out, drawn the way a
        // marquee is: the same hairline rectangle, so the two cannot
        // drift apart.
        if let Some((from, to)) = self.editor().framing() {
            let a = view.world_to_screen(from[0], from[1]);
            let b = view.world_to_screen(to[0], to[1]);
            frame.extend(select::marquee_prims(a, b, &self.theme));
        }
        // The brush shows its size before it paints: a ring at the
        // pointer, wherever the next press would paint.
        if let Some((x, y)) = self.cursor
            && !self.over_chrome(view, (x, y))
            && self.editor().pointer_tool(self.doc(), view, (x, y)) == Tool::Brush
        {
            let (half, angle) = brush::ring_of(self.brushes.brush(), view.px_per_world());
            let ink = with_alpha(self.ink_rgba(), RING_ALPHA);
            frame.extend(brush::ring_prims(
                (x as f32, y as f32),
                half,
                angle,
                view.scale as f32,
                ink,
            ));
        }
        frame.extend(self.dock(view).prims(
            self.editor().tool(),
            self.ink,
            self.dock_icon_slot,
            &self.theme,
        ));
        if let (Some(pal), Some(atlas)) = (self.library(view), self.atlas.as_ref()) {
            frame.extend(pal.prims(
                self.brushes.sets(),
                self.brushes.selected(),
                atlas,
                self.atlas_slot,
                self.icon_slot,
                &self.theme,
            ));
        }
        if let (Some(strip), Some(atlas)) = (self.strip(view), self.atlas.as_ref()) {
            frame.extend(strip.prims(
                self.brushes.sets(),
                self.brushes.selected(),
                self.brushes.slots(),
                self.brushes.brush(),
                self.drag.as_ref().and_then(|d| d.over),
                atlas,
                self.atlas_slot,
                self.icon_slot,
                &self.theme,
            ));
        }
        // The brush in the pointer's hand, drawn last: it passes over
        // every panel between the shelf it came off and its seat.
        if let Some(drag) = self.drag.as_ref().filter(|d| d.carried) {
            let side = palette::ICON * self.chrome(view) as f32;
            frame.extend([Prim::sprite(
                ScreenRect {
                    x: drag.to.0 - side / 2.0,
                    y: drag.to.1 - side / 2.0,
                    w: side,
                    h: side,
                },
                palette::icon_uv(drag.icon),
                self.icon_slot,
            )]);
        }
        if let (Some(bar), Some(atlas)) = (self.props(view), self.atlas.as_ref()) {
            frame.extend(bar.prims(
                self.brushes.name(),
                self.brushes.edited(),
                self.brushes.brush(),
                atlas,
                self.atlas_slot,
                &self.theme,
            ));
        }
        if let (Some(bar), Some(atlas)) = (self.text_bar(view), self.atlas.as_ref()) {
            let doc = self.doc();
            let style = self.editor().text_style(doc);
            let showing = textbar::Showing {
                style: &style,
                kind: self.editor().text_kind(doc),
            };
            frame.extend(bar.prims(&showing, atlas, self.atlas_slot, &self.theme));
        }
        if let (Some(bar), Some(atlas)) = (self.shape_bar(view), self.atlas.as_ref()) {
            let look = self.editor().shape_look(self.doc(), self.ink_hex());
            frame.extend(bar.prims(&look, atlas, self.atlas_slot, &self.theme));
        }
        if let (Some(handle), Some(atlas)) = (self.handle(view), self.atlas.as_ref()) {
            frame.extend(handle.prims(atlas, self.atlas_slot, &self.theme));
        }
        if let (Some(panel), Some(atlas)) = (self.panel(view), self.atlas.as_ref()) {
            let lift = self.carry.as_ref().map(Carry::lift);
            let picked: Vec<String> = self
                .editor()
                .picked(self.doc())
                .into_iter()
                .map(str::to_owned)
                .collect();
            let doc = self.doc();
            let active = self.editor().active(doc);
            let (blend, opacity, locked) = doc.layer(active).map_or(
                (crate::doc::BlendMode::Normal, 1.0, false),
                |l| (l.blend, l.opacity as f32, l.locked),
            );
            let showing = layers::Showing {
                blend: blend.name(),
                opacity,
                locked,
                active,
                picked: &picked,
                lift: lift.as_ref(),
                drop: self
                    .carry
                    .as_ref()
                    .filter(|c| c.held)
                    .and_then(|c| c.aim.as_ref()),
                slides: &self.slides,
                filter: self.editor().filtering(),
                searching: self.searching.is_some(),
                thumbs: self.thumbs.as_ref().map(|(sheet, slot)| (sheet, *slot)),
            };
            frame.extend(panel.prims(&showing, atlas, self.atlas_slot, &self.theme));
            if let (Some(field), Some(text)) = (&self.searching, panel.search_text()) {
                frame.extend(field.prims(text, atlas, self.atlas_slot, &self.theme, true));
            }
            // The name being typed is drawn over the name it replaces,
            // rounded the way the card is: the row goes on showing its
            // eye, its chevron and its glyph, so what is being renamed
            // stays in its place in the tree.
            if let Some((id, field)) = &self.renaming
                && let Some(row) = panel.rows.iter().find(|r| r.id == *id)
            {
                let pad = field::PADDING * self.chrome(view) as f32;
                let left = row.label_x - pad;
                let over = ScreenRect {
                    x: left,
                    w: row.card.x + row.card.w - left,
                    ..row.card
                };
                frame.extend([Prim::rounded(over, layers::ROW_RADIUS, self.theme.panel)]);
                frame.extend(field.prims(over, atlas, self.atlas_slot, &self.theme, true));
            }
        }
        if let (Some(tabs), Some(atlas)) = (self.tabs(view), self.atlas.as_ref()) {
            frame.extend(tabs.prims(atlas, self.atlas_slot, &self.theme));
        }
        if let (Some(bar), Some(atlas)) = (self.bar(view), self.atlas.as_ref()) {
            frame.extend(bar.prims(atlas, self.atlas_slot, &self.theme, self.bar_open()));
        }
        // A menu stands over every panel.
        if let (Some(laid), Some(opened), Some(atlas)) =
            (self.menu_laid(view), self.menu.as_ref(), self.atlas.as_ref())
        {
            frame.extend(laid.prims(&opened.items, opened.hover, atlas, self.atlas_slot, &self.theme));
        }
        // The send panel is modal, so it is drawn last of everything —
        // over the strip the way it is pressed before it.
        if let (Some(sending), Some(atlas), Some((panel, _))) =
            (&self.sending, self.atlas.as_ref(), self.send_panel(view))
        {
            let menu = sending.menu();
            let look = send::Look {
                agents: &sending.agents,
                target: sending.target,
                line: &sending.line,
                scroll: sending.scroll,
                menu: menu.as_ref().map(|(_, matches, call)| send::Menu {
                    skills: &sending.skills,
                    matches,
                    pick: sending.pick,
                    call: *call,
                }),
                picture: sending.picture,
            };
            let ink = send::Ink {
                atlas,
                slot: self.atlas_slot,
                logos: self.agent_logo_slot,
                theme: &self.theme,
            };
            frame.extend(panel.prims(&look, &ink));
        }
        frame
    }

    /// Stores what an editor input changed and redraws if anything did.
    ///
    /// A scene change dirties the tab; a camera change does not. Panning
    /// to look at the far corner of a board is not work to lose, and
    /// being asked to save after merely looking around would teach the
    /// dot to mean nothing.
    fn apply(&mut self, change: Change) {
        // A change that took the text being typed away, or locked it,
        // ends the session before it is written down: nothing is left for
        // it to hold the keyboard for.
        if change != Change::None {
            let (editor, doc) = self.active();
            let _ = editor.settle_typing(doc);
        }
        match change {
            Change::None => {}
            Change::Selection => {
                self.remember();
                self.redraw();
            }
            Change::Scene => {
                self.remember();
                self.touch();
                self.thumbs_stale = true;
                self.redraw();
            }
            Change::Camera(camera) => {
                self.active().1.camera = camera;
                self.redraw();
            }
        }
    }

    fn pointer_pressed(&mut self, button: Button) {
        let (Some(view), Some((x, y))) = (self.view(), self.cursor) else {
            return;
        };
        // The send panel is modal and over everything, the strip
        // included: it is the one thing in this window that is finished
        // by leaving it.
        if self.sending.is_some() {
            let Some((panel, lines)) = self.send_panel(&view) else {
                return;
            };
            if button == Button::Left {
                // The menu stands over the dialog, so it is pressed first.
                let on_menu = self.sending.as_ref().and_then(|s| {
                    let (_, matches, _) = s.menu()?;
                    let k = s.line.caret_line(&lines);
                    let m = panel.menu(&panel.boxed(&lines, s.scroll), k, matches.len(), s.pick)?;
                    m.hit(x, y)
                });
                if let Some(at) = on_menu {
                    self.take_skill(at);
                    return self.update_cursor_icon();
                }
                let shift = self.modifiers.state().shift_key();
                match (panel.hit(x, y), self.sending.as_mut(), self.atlas.as_ref()) {
                    (Some(send::Hit::Target(i)), Some(s), _) => {
                        if i != s.target {
                            s.aim(i);
                        }
                    }
                    // A press in the box puts the caret under it — with
                    // Shift, carries the selection there — and a drag
                    // from it goes on selecting.
                    (Some(send::Hit::Line), Some(s), Some(atlas)) if panel.line.contains(x, y) => {
                        let (k, along) = panel.boxed(&lines, s.scroll).at(x, y);
                        let to = s.line.index_at(atlas, &lines, k, along);
                        s.line.go(to, shift);
                        s.selecting = true;
                        s.settle(false);
                    }
                    // A press outside a modal panel closes it.
                    (None, ..) => self.sending = None,
                    _ => {}
                }
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        // A title of the application menu opens its menu, or shuts it
        // when it is the one standing open — whatever menu was up before.
        if let Some(title) = self.bar(&view).and_then(|b| b.hit(x, y)) {
            if button == Button::Left {
                let open = self.bar_open();
                self.close_menu(None);
                if open != Some(title) {
                    self.open_bar_menu(title);
                }
            }
            return self.update_cursor_icon();
        }
        // A menu stands over everything else: a line is taken, a press on
        // it between lines is nothing, and a press anywhere else puts it
        // away — and is not also a press on what is under it.
        if let Some(laid) = self.menu_laid(&view) {
            if button == Button::Left {
                let items = self.menu.as_ref().map_or(&[][..], |m| &m.items[..]);
                match laid.hit(x, y, items) {
                    Some(i) => self.close_menu(Some(i)),
                    None if laid.contains(x, y) => {}
                    None => self.close_menu(None),
                }
            } else if !laid.contains(x, y) {
                self.close_menu(None);
            }
            return self.update_cursor_icon();
        }
        // A name being typed is finished by pressing somewhere else, as
        // Enter finishes it: the keyboard cannot be left held by a field
        // the pointer has walked away from.
        if self.renaming.is_some() && button == Button::Left {
            self.commit_rename();
        }
        if button == Button::Left {
            self.searching = None;
        }
        // The strip is over the handle is over the panel is over the
        // dock is over the canvas.
        if let Some(hit) = self.tabs(&view).and_then(|t| t.hit(x, y)) {
            if button == Button::Left {
                match hit {
                    TabHit::Select(i) => self.activate(i),
                    TabHit::Close(i) => self.request_close(i),
                    TabHit::New => self.open_project(Project::untitled()),
                    TabHit::Strip => {}
                }
            }
            return self.update_cursor_icon();
        }
        if let Some(handle) = self.handle(&view)
            && handle.hit(x, y)
        {
            if button == Button::Left {
                self.layers_shown = !handle.open;
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        if let Some(panel) = self.panel(&view)
            && let Some(hit) = panel.hit(x, y)
        {
            // What the panel does to the layers is a step of its own: the
            // text being typed is left first, as a press elsewhere leaves it.
            self.end_typing();
            if button == Button::Left {
                // `Panel::hit` cannot see a second press: counting them
                // is the window's. A press on a card that is already
                // selected, soon enough after the last one, is what
                // makes a Select a Rename.
                if hit == PanelHit::Blend {
                    self.open_blend_menu(panel.blend);
                    return self.update_cursor_icon();
                }
                // The field takes the keyboard, the caret where it was
                // pressed.
                if hit == PanelHit::Search {
                    self.search_at(&panel, x);
                    self.redraw();
                    return self.update_cursor_icon();
                }
                // The strength's slider has the pointer to itself from the
                // press to the release, wherever it wanders.
                if hit == PanelHit::Opacity {
                    self.fading = true;
                    let (editor, doc) = self.active();
                    let change = editor.set_opacity(doc, whole_percent(panel.opacity_at(x)));
                    self.apply(change);
                    self.redraw();
                    return self.update_cursor_icon();
                }
                let hit = self.second_press(hit);
                self.panel_hit(hit.clone());
                // A card taken by its name may be about to be carried off,
                // by the grip the press made — once the pointer has gone
                // far enough to mean it. A card whose name is open is not:
                // a field is not dragged.
                if let PanelHit::Pick(id) = hit
                    && let Some(row) = panel.rows.iter().find(|r| r.id == id)
                {
                    self.pressed = Some(Press {
                        id,
                        from: (x, y),
                        grab_dy: y as f32 - row.card.y,
                    });
                }
                self.redraw();
            } else if button == Button::Right {
                match hit {
                    // The eye's own menu is its colours, as in Photoshop.
                    PanelHit::Toggle(id) => {
                        if let Some(row) = panel.rows.iter().find(|r| r.id == id) {
                            self.open_tag_menu(id, row.eye);
                        }
                    }
                    PanelHit::Pick(id) => self.open_row_menu(id, (x, y)),
                    _ => {}
                }
            }
            return self.update_cursor_icon();
        }
        if let Some(bar) = self.props(&view)
            && let Some(hit) = bar.hit(x, y)
        {
            if button == Button::Left {
                self.props_hit(&bar, hit, x);
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        if let Some(bar) = self.text_bar(&view)
            && let Some(hit) = bar.hit(x, y)
        {
            if button == Button::Left {
                self.text_bar_hit(&bar, hit, x);
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        if let Some(bar) = self.shape_bar(&view)
            && let Some(hit) = bar.hit(x, y)
        {
            if button == Button::Left {
                self.shape_bar_hit(&bar, hit, x);
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        if let Some(strip) = self.strip(&view)
            && let Some(hit) = strip.hit(x, y)
        {
            if button == Button::Left {
                self.strip_hit(hit);
                self.keep_brushes();
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        if let Some(pal) = self.library(&view)
            && let Some(hit) = pal.hit(x, y)
        {
            if button == Button::Left {
                self.palette_hit(hit);
                // The same press may yet turn out to be a drag onto a
                // seat. Nothing is carried until it has moved.
                if let palette::Hit::Brush(set, index) = hit
                    && let Some(cell) = pal.cells.iter().find(|c| (c.set, c.index) == (set, index))
                {
                    self.drag = Some(Dragging {
                        at: (set, index),
                        icon: cell.icon,
                        from: (x as f32, y as f32),
                        to: (x as f32, y as f32),
                        carried: false,
                        over: None,
                    });
                }
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        match self.dock(&view).hit(x, y) {
            Some(Hit::Tool(tool)) => {
                if button == Button::Left {
                    self.end_typing();
                    let (editor, doc) = self.active();
                    editor.choose_tool(tool, doc);
                    self.redraw();
                }
            }
            Some(Hit::Ink(i)) => {
                if button == Button::Left && i < INKS.len() {
                    // A colour the window is holding has to go on
                    // something: with a frame selected it is that
                    // frame's ground, and otherwise it is the ink new
                    // strokes are laid in.
                    let hex = INKS[i].to_owned();
                    // A text being typed or selected takes the ink as its
                    // colour, and it is the ink in the hand from then on.
                    if self.editor().text_targeted(self.doc()) {
                        self.ink = i;
                        let change = self.with_text(|e, d, f| e.restyle(d, f, |s| s.color.clone_from(&hex)));
                        self.apply(change);
                        return self.update_cursor_icon();
                    }
                    // So do the shapes and lines selected, on their stroke:
                    // the ink is what they are drawn in.
                    if self.editor().shape_targeted(self.doc()) {
                        self.ink = i;
                        let (editor, doc) = self.active();
                        let change = editor.restyle_shapes(doc, crate::editor::Restyle::Stroke(Some(hex)));
                        self.apply(change);
                        return self.update_cursor_icon();
                    }
                    let (editor, doc) = self.active();
                    let selected: Vec<String> = editor.selection().to_vec();
                    let mut painted = false;
                    for id in &selected {
                        if let Some(f) = doc.frame_mut(id) {
                            f.background = Some(hex.clone());
                            painted = true;
                        }
                    }
                    if painted {
                        self.apply(Change::Scene);
                    } else {
                        self.ink = i;
                        self.redraw();
                    }
                }
            }
            Some(Hit::Panel) => {}
            None => {
                if button == Button::Left && self.text_press(&view, (x, y)) {
                    return self.update_cursor_icon();
                }
                let tip = self.brushes.tip();
                let (editor, doc) = self.active();
                let change = editor.press(button, &view, (x, y), doc, &tip);
                self.apply(change);
            }
        }
        self.update_cursor_icon();
    }

    /// A left press on the canvas, as far as text is concerned: with the
    /// Text tool it places a text or types into one; with any tool it
    /// puts the caret in the text being typed, or leaves it; and a double
    /// click on a text with the Select tool types into it. True when the
    /// press was the text's, and nothing else is to see it.
    fn text_press(&mut self, view: &View, at: (f64, f64)) -> bool {
        let clicks = self.count_clicks(at.0, at.1);
        let editor = self.editor();
        let doc = self.doc();
        let tool = editor.pointer_tool(doc, view, at);
        let typing = editor.typing().is_some();
        let double = clicks >= 2 && tool == Tool::Select && editor.text_at(doc, view, at).is_some();
        if !(tool == Tool::Text || typing || double) || matches!(tool, Tool::Hand | Tool::Zoom) {
            return false;
        }
        // Leaving the text being typed is a change of its own session,
        // written down before the press starts anything else — a new text,
        // or typing into another — which is a session of its own.
        if self.editor().leaves_text(view, at, self.doc()) {
            self.end_typing();
        }
        let ink = self.ink_hex().to_owned();
        let change = self.with_text(|e, d, f| e.text_press(view, at, d, f, clicks, &ink));
        self.text_input();
        self.apply(change);
        // A press away from the text, with a tool that is not the Text
        // tool, only left it: the press is that tool's too.
        tool == Tool::Text || self.editor().typing().is_some()
    }

    fn pointer_released(&mut self, button: Button) {
        // The box's own drag: the canvas never saw its press.
        if let Some(s) = self.sending.as_mut().filter(|s| s.selecting) {
            s.selecting = false;
            return self.update_cursor_icon();
        }
        // A brush carried out of the library is seated where it was let
        // go of, if that was a seat that can be written. Dropped
        // anywhere else it is simply the brush in the hand, which the
        // press already made it.
        if let Some(drag) = self.drag.take() {
            if let Some(n) = drag.over
                && self.brushes.assign_slot(n, drag.at)
            {
                self.brushes_dirty = true;
            }
            self.keep_brushes();
            self.redraw();
            return self.update_cursor_icon();
        }
        // A brush edit is over when the pointer that made it comes up.
        self.keep_brushes();
        // The text's slider let go of is where the text rests: one step
        // back undoes the whole drag.
        if button == Button::Left && self.text_grab.take().is_some() {
            self.remember();
            self.redraw();
            return self.update_cursor_icon();
        }
        // So is the shape's.
        if button == Button::Left && self.shape_grab.take().is_some() {
            self.remember();
            self.redraw();
            return self.update_cursor_icon();
        }
        // A slider let go of is just let go of: the canvas never saw the
        // press, so there is nothing under it to end.
        if button == Button::Left && self.grab.take().is_some() {
            self.redraw();
            return self.update_cursor_icon();
        }
        // A press on a card that never went anywhere was a click.
        if button == Button::Left {
            self.pressed = None;
        }
        // The strength let go of is where the layers rest: one step back
        // undoes the whole drag.
        if button == Button::Left && std::mem::take(&mut self.fading) {
            self.remember();
            self.redraw();
            return self.update_cursor_icon();
        }
        // A carried layer lands where the drop says; the canvas never saw
        // the press, so it has nothing to end. The card runs the lift
        // backwards into its new row from here.
        if button == Button::Left
            && let Some(carry) = self.carry.as_mut().filter(|c| c.held)
        {
            carry.held = false;
            if let Some(place) = carry.aim.take() {
                let (editor, doc) = self.active();
                let change = editor.drop_layers(doc, &place);
                self.apply(change);
            }
            // The stack the card was let go of in is a state to step
            // back to, and this is the only place that can say so: the
            // canvas never saw the press, so no `Change` comes back
            // from the editor to end the gesture the way a release on
            // the canvas does.
            self.remember();
            self.redraw();
            return self.update_cursor_icon();
        }
        let Some(view) = self.view() else { return };
        let (x, y) = self.cursor.unwrap_or_default();
        let ink = self.ink_hex().to_owned();
        let (editor, doc) = self.active();
        let change = editor.release(button, &view, (x, y), doc, &ink);
        self.apply(change);
        // A text opened at the release is set in its own faces now: the
        // release had none in hand.
        if self.editor().typing().is_some() {
            self.with_text(|e, d, f| e.relay_text(d, f));
            self.text_input();
            self.redraw();
        }
        self.update_cursor_icon();
    }

    fn pointer_moved(&mut self, x: f64, y: f64) {
        self.cursor = Some((x, y));
        // A press in the instruction's box is selecting for as long as
        // it is held, wherever the pointer wanders — past the box's top
        // or bottom edge it runs on into the lines scrolled out of sight.
        if self.sending.as_ref().is_some_and(|s| s.selecting) {
            if let Some(view) = self.view()
                && let Some((panel, lines)) = self.send_panel(&view)
                && let (Some(s), Some(atlas)) = (self.sending.as_mut(), self.atlas.as_ref())
            {
                let (k, along) = panel.boxed(&lines, s.scroll).at(x, y);
                let to = s.line.index_at(atlas, &lines, k, along);
                s.line.go(to, true);
            }
            self.follow_caret();
            self.redraw();
            return self.update_cursor_icon();
        }
        // A brush out of the library has the pointer to itself once it
        // has moved far enough to mean it, and the canvas sees nothing.
        if self.drag.is_some() {
            let over = self.seat_under(x, y);
            let slop = DRAG_SLOP * self.view().map_or(1.0, |v| self.chrome(&v));
            if let Some(drag) = &mut self.drag {
                drag.to = (x as f32, y as f32);
                let far = f64::from(drag.to.0 - drag.from.0)
                    .hypot(f64::from(drag.to.1 - drag.from.1));
                drag.carried |= far > slop;
                drag.over = drag.carried.then_some(over).flatten();
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        // With one of the application menus open, passing over another
        // title opens that one instead, as a menu bar does.
        if let Some(open) = self.bar_open()
            && let Some(title) = self.view().and_then(|view| self.bar(&view)?.hit(x, y))
            && title != open
        {
            self.menu = None;
            self.open_bar_menu(title);
        }
        // Over a menu the line under the pointer is tried on the board.
        if self.menu.is_some() {
            self.hover_menu_at(x, y);
            return self.update_cursor_icon();
        }
        // A press on a card lifts it once it has gone far enough to be a
        // drag, and not before: a click or a double click never lifts.
        if let Some(press) = &self.pressed {
            let slop = DRAG_SLOP * self.view().map_or(1.0, |v| self.chrome(&v));
            if (x - press.from.0).hypot(y - press.from.1) > slop {
                let press = self.pressed.take().expect("just read");
                self.carry = Some(Carry {
                    id: press.id,
                    grab_dy: press.grab_dy,
                    y: y as f32 - press.grab_dy,
                    held: true,
                    // A card caught while it was still settling carries
                    // on from where it had got to.
                    t: self.carry.as_ref().map_or(0.0, |c| c.t),
                    aim: None,
                });
            }
        }
        // A carried layer has the pointer to itself: the card follows it,
        // the drop is read off the row under it, and the canvas sees
        // nothing. Nothing moves until the button comes up.
        if self.carry.as_ref().is_some_and(|c| c.held) {
            let aim = self.view().and_then(|view| {
                let place = self.panel(&view)?.aim(y)?;
                let doc = self.doc();
                let (owner, _) = doc.place(&place)?;
                let picked: Vec<String> = self
                    .editor()
                    .picked(doc)
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                doc.can_move(&picked, owner).then_some(place)
            });
            if let Some(carry) = &mut self.carry {
                carry.y = y as f32 - carry.grab_dy;
                carry.aim = aim;
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        // So does the layers' strength, read off the pointer as it goes.
        if self.fading {
            if let Some(view) = self.view()
                && let Some(panel) = self.panel(&view)
            {
                let (editor, doc) = self.active();
                let change = editor.set_opacity(doc, whole_percent(panel.opacity_at(x)));
                self.apply(change);
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        // So does the text bar's slider.
        if let Some(slider) = self.text_grab {
            if let Some(view) = self.view()
                && let Some(bar) = self.text_bar(&view)
            {
                self.drag_text_field(&bar, slider, x);
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        // And the shape bar's.
        if let Some(slider) = self.shape_grab {
            if let Some(view) = self.view()
                && let Some(bar) = self.shape_bar(&view)
            {
                self.drag_shape_field(&bar, slider, x);
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        // A slider has the pointer to itself, wherever it wanders to.
        if let Some(field) = self.grab {
            if let Some(view) = self.view()
                && let Some(bar) = self.props(&view)
            {
                self.drag_field(&bar, field, x);
            }
            self.redraw();
            return self.update_cursor_icon();
        }
        if let Some(view) = self.view() {
            let (editor, doc) = self.active();
            let change = editor.moved(&view, (x, y), doc);
            self.apply(change);
        }
        // The brush's ring follows the pointer, so every move is a frame.
        if self.editor().tool() == Tool::Brush {
            self.redraw();
        }
        self.update_cursor_icon();
    }

    fn scrolled(&mut self, delta: MouseScrollDelta) {
        let Some(view) = self.view() else { return };
        let delta = match delta {
            MouseScrollDelta::LineDelta(x, y) => (
                f64::from(x) * SCROLL_LINE_PX * view.scale,
                f64::from(y) * SCROLL_LINE_PX * view.scale,
            ),
            MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
        };
        let cursor = self.cursor.unwrap_or((
            f64::from(view.viewport.w) / 2.0,
            f64::from(view.viewport.h) / 2.0,
        ));
        // The dialog is modal for the wheel as it is for everything else:
        // over the instruction the wheel moves its lines, and anywhere
        // else it moves nothing — least of all the board behind it.
        if self.sending.is_some() {
            if let Some((panel, lines)) = self.send_panel(&view)
                && panel.line.contains(cursor.0, cursor.1)
                && let Some(s) = self.sending.as_mut()
            {
                let most = panel.max_scroll(lines.len());
                let next = (s.scroll - delta.1 as f32).clamp(0.0, most);
                if next != s.scroll {
                    s.scroll = next;
                    self.redraw();
                }
            }
            return;
        }
        // A menu takes the wheel over itself, and nothing else does while
        // it stands. The line under a pointer that did not move is another
        // one once the lines have moved, and it is the one tried.
        if let Some(laid) = self.menu_laid(&view) {
            if laid.contains(cursor.0, cursor.1)
                && let Some(opened) = self.menu.as_mut()
            {
                opened.scroll = (laid.scroll() - delta.1 as f32).clamp(0.0, laid.max_scroll());
                self.hover_menu_at(cursor.0, cursor.1);
                self.redraw();
            }
            return;
        }
        // The panel takes the wheel when the pointer is over it: the
        // wheel away from the user shows what is further up the stack.
        if self
            .panel(&view)
            .and_then(|p| p.hit(cursor.0, cursor.1))
            .is_some()
        {
            return self.scroll_panel(-delta.1);
        }
        // So does the library, over its own shelf.
        if let Some(pal) = self.library(&view)
            && pal.hit(cursor.0, cursor.1).is_some()
        {
            let next = (self.palette_scroll - delta.1 as f32).clamp(0.0, pal.max_scroll());
            if next != self.palette_scroll {
                self.palette_scroll = next;
                self.redraw();
            }
            return;
        }
        let shift = self.modifiers.state().shift_key();
        let camera = self.active().0.scroll(&view, cursor, delta, shift);
        self.apply(Change::Camera(camera));
    }

    /// `bare` is the key with every modifier taken off it. The letters
    /// are read off `key`, which follows the layout the way a hotkey
    /// should; a seat's digit has to come off `bare`, because `Shift+1`
    /// arrives as a different character on every layout there is.
    fn key(&mut self, key: &Key, bare: &Key, text: Option<&str>, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if pressed {
            let (editor, doc) = self.active();
            let change = editor.settle_typing(doc);
            self.apply(change);
        }
        match key {
            // The clipboard's keys come first for whichever field has the
            // keyboard: they are the window's to answer, not the field's.
            Key::Character(c)
                if pressed
                    && self.field_in_hand().is_some()
                    && self.modifiers.state().control_key()
                    && ["c", "x", "v"].iter().any(|k| c.eq_ignore_ascii_case(k)) =>
            {
                self.clipboard_key(c);
            }
            // The send panel is modal: it takes the keyboard before
            // anything else, including the rename that cannot be open
            // under it.
            _ if self.sending.is_some() && pressed => {
                let mods = self.modifiers.state();
                let laid = self.view().and_then(|view| self.send_panel(&view));
                let Some(sending) = self.sending.as_mut() else {
                    return;
                };
                // While a menu of skills is up, the arrows walk it, Tab
                // or Enter takes the pick, and Esc puts the menu away —
                // the dialog stays. Up means on screen: a menu the wheel
                // scrolled away with its line takes no keys.
                let shown = |s: &Sending, n: usize| {
                    laid.as_ref().is_some_and(|(panel, lines)| {
                        let b = panel.boxed(lines, s.scroll);
                        let k = s.line.caret_line(lines);
                        panel.menu(&b, k, n, s.pick).is_some()
                    })
                };
                let menu = sending.menu().filter(|(_, m, _)| shown(sending, m.len()));
                if let Some((token, matches, _)) = menu {
                    let n = matches.len();
                    match key {
                        Key::Named(NamedKey::ArrowDown) => {
                            sending.pick = (sending.pick + 1) % n;
                            return self.redraw();
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            sending.pick = (sending.pick + n - 1) % n;
                            return self.redraw();
                        }
                        Key::Named(NamedKey::Escape) => {
                            sending.dismissed = Some(token.start);
                            return self.redraw();
                        }
                        Key::Named(NamedKey::Tab) => {
                            let pick = sending.pick;
                            return self.take_skill(pick);
                        }
                        Key::Named(NamedKey::Enter) if !mods.shift_key() => {
                            let pick = sending.pick;
                            return self.take_skill(pick);
                        }
                        _ => {}
                    }
                }
                // Whether the key changed the text or moved the caret:
                // only then is the menu's pick back at its first row. A
                // modifier pressed on its own is neither.
                let mut edited = false;
                match key {
                    Key::Named(NamedKey::Escape) => self.sending = None,
                    // Shift+Enter breaks the line, as it does in the
                    // agent's own box; Enter alone sends.
                    Key::Named(NamedKey::Enter) if mods.shift_key() => {
                        sending.line.newline();
                        edited = true;
                    }
                    Key::Named(NamedKey::Enter) => self.do_send(),
                    Key::Named(NamedKey::Tab) => {
                        // Tab walks the targets when there is more than
                        // one, since the fields are two at most and the
                        // list is the thing being chosen from.
                        let next = (sending.target + 1) % sending.agents.len().max(1);
                        if next != sending.target {
                            sending.aim(next);
                        }
                    }
                    // In the box the arrows and Home and End walk the
                    // lines as it shows them; with Ctrl, Home and End
                    // are the whole text's, which `edit` answers.
                    Key::Named(
                        named @ (NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::Home
                        | NamedKey::End),
                    ) if !mods.control_key() => {
                        if let (Some((_, lines)), Some(atlas)) = (&laid, self.atlas.as_ref()) {
                            let shift = mods.shift_key();
                            match named {
                                NamedKey::ArrowUp => sending.line.up(atlas, lines, shift),
                                NamedKey::ArrowDown => sending.line.down(atlas, lines, shift),
                                NamedKey::Home => sending.line.line_home(lines, shift),
                                _ => sending.line.line_end(lines, shift),
                            }
                            edited = true;
                        }
                    }
                    _ => edited = edit(&mut sending.line, key, mods),
                }
                if let Some(sending) = self.sending.as_mut() {
                    sending.settle(edited);
                }
                self.follow_caret();
                self.redraw();
            }
            // A field being typed into takes the keyboard whole, and so
            // stands first: `Ctrl+S` in the middle of a name would
            // otherwise save mid-word.
            _ if self.renaming.is_some() && pressed => {
                let Some((_, field)) = self.renaming.as_mut() else {
                    return;
                };
                match key {
                    Key::Named(NamedKey::Escape) => self.renaming = None,
                    Key::Named(NamedKey::Enter) => self.commit_rename(),
                    _ => {
                        edit(field, key, self.modifiers.state());
                    }
                }
                self.redraw();
            }
            // The filter's field narrows the tree as it is typed into, and
            // Enter or Esc gives the keyboard back with the name kept.
            _ if self.searching.is_some() && pressed => {
                let Some(field) = self.searching.as_mut() else {
                    return;
                };
                match key {
                    Key::Named(NamedKey::Escape | NamedKey::Enter) => self.searching = None,
                    _ => {
                        edit(field, key, self.modifiers.state());
                    }
                }
                self.sync_search();
                self.redraw();
            }
            // The font menu takes the keyboard while it stands, text being
            // typed or not: the arrows walk it, Enter takes a family, and
            // a name typed is looked for.
            _ if pressed && self.menu.as_ref().is_some_and(|m| matches!(m.purpose, Purpose::Font { .. })) => {
                self.font_menu_key(key, text);
            }
            // A text being typed takes the keyboard whole, as a field does:
            // a letter is written, not a tool taken up.
            _ if pressed && self.editor().typing().is_some() => {
                self.typing_key(key, text);
                self.redraw();
            }
            Key::Named(NamedKey::Space) => self.active().0.hold_space(pressed),
            // Enter on one selected text types into it, all of it
            // selected, as every design tool's Enter does.
            Key::Named(NamedKey::Enter) if pressed && self.menu.is_none() => {
                let change = self.with_text(|e, d, f| e.edit_selected_text(d, f));
                self.text_input();
                self.apply(change);
            }
            // A menu goes away, leaving the board as it found it.
            Key::Named(NamedKey::Escape) if pressed && self.menu.is_some() => {
                self.close_menu(None);
            }
            // A card in the hand is put back where it came from: Esc is
            // the drop that does not happen.
            Key::Named(NamedKey::Escape)
                if pressed && self.carry.as_ref().is_some_and(|c| c.held) =>
            {
                if let Some(carry) = &mut self.carry {
                    carry.held = false;
                    carry.aim = None;
                }
                self.redraw();
            }
            Key::Named(NamedKey::Escape) if pressed => {
                let (editor, doc) = self.active();
                if editor.escape(doc) {
                    self.redraw();
                }
            }
            Key::Named(NamedKey::F2) if pressed => self.rename_active(),
            Key::Named(NamedKey::Delete | NamedKey::Backspace) if pressed => {
                let (editor, doc) = self.active();
                let change = editor.delete(doc);
                self.apply(change);
            }
            Key::Character(text) if pressed && self.modifiers.state().control_key() => {
                // Shift turns the character upper case, so the letter is
                // read case-insensitively and the modifier separately.
                // The letters follow the layout, as a hotkey should; the
                // bare key is asked second, since Shift turns a bracket
                // into a brace on one layout and something else on the
                // next.
                let shift = self.modifiers.state().shift_key();
                let alt = self.modifiers.state().alt_key();
                let action = menubar::shortcut(&text.to_ascii_lowercase(), shift, alt).or_else(|| match bare {
                    Key::Character(c) => menubar::shortcut(&c.to_ascii_lowercase(), shift, alt),
                    _ => None,
                });
                if let Some(action) = action {
                    self.run_action(action);
                }
            }
            Key::Character(text) if pressed => {
                let mods = self.modifiers.state();
                if mods.control_key() || mods.alt_key() || mods.super_key() {
                    return;
                }
                let mut chars = text.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    self.plain_key(c, bare, mods.shift_key());
                }
            }
            _ => {}
        }
        self.update_cursor_icon();
    }

    /// What a key under `Ctrl` or a line of the application menu asks
    /// for: the two are one door, so what is taught cannot disagree with
    /// what is done.
    fn run_action(&mut self, action: Action) {
        // A key taken while a menu stands puts it away first: its lines
        // were written for the board as it was, and a new tab or a step
        // back would leave them answering for another.
        self.close_menu(None);
        // With a text being typed, the edits are the text's own; a save
        // keeps it open, and anything else leaves it first.
        if self.editor().typing().is_some() {
            use crate::editor::TextKey;
            let key = match action {
                Action::Undo => Some(TextKey::Undo),
                Action::Redo => Some(TextKey::Redo),
                Action::Delete => Some(TextKey::Delete),
                _ => None,
            };
            if let Some(k) = key {
                let change = self.with_text(|e, d, f| e.text_key(k, d, f));
                self.text_input();
                return self.apply(change);
            }
            match action {
                Action::Copy => return self.copy_text(),
                Action::Cut => {
                    self.typing_command("x", false);
                    return;
                }
                Action::Paste => return self.paste_text(),
                Action::Save => {}
                _ => self.end_typing(),
            }
        }
        let change = match action {
            Action::New => return self.open_project(Project::untitled()),
            Action::Open => return self.ask_open(),
            Action::Save => return self.save_active(),
            Action::SaveAs => return self.ask_name(self.active, Then::Stay),
            Action::Export => return self.ask_send(),
            Action::Close => return self.request_close(self.active),
            Action::Quit => return self.quit(),
            Action::Undo => return self.undo(),
            Action::Redo => return self.redo(),
            Action::Cut => return self.copy_layers(true),
            Action::Copy => return self.copy_layers(false),
            Action::Paste => return self.paste(),
            Action::Rename => return self.rename_active(),
            Action::Layers => {
                self.layers_shown = !self.layers_shown;
                return self.redraw();
            }
            Action::Library => {
                self.palette_shown = !self.palette_shown;
                return self.redraw();
            }
            Action::Layer(command) if command.merges() => return self.merge_layers(command),
            Action::Delete => {
                let (editor, doc) = self.active();
                editor.delete(doc)
            }
            Action::NewLayer => {
                let (editor, doc) = self.active();
                editor.add_layer(doc)
            }
            Action::NewGroup => {
                let (editor, doc) = self.active();
                editor.add_group(doc)
            }
            Action::Layer(command) => {
                let (editor, doc) = self.active();
                editor.run(doc, command)
            }
        };
        self.apply(change);
    }

    /// The active layer's name, opened where its row is: the panel comes
    /// out, and the list goes to the row.
    fn rename_active(&mut self) {
        let id = self.editor().active(self.doc()).to_owned();
        let (editor, doc) = self.active();
        editor.reveal(doc, &id);
        self.layers_shown = true;
        self.focused = None;
        self.panel_hit(PanelHit::Rename(id));
        self.redraw();
    }

    /// A character typed with no modifier but Shift: `Shift+L` shows or
    /// hides the layers, a tool's letter selects it, and the brush's
    /// keys adjust it while it is selected.
    fn plain_key(&mut self, c: char, bare: &Key, shift: bool) {
        // A seat is numbered, and the number is the key. Taken off the
        // key without its modifiers rather than off the character that
        // arrived: `Shift+1` is `!` on one layout and something else on
        // the next, but the key is the `1` key on both.
        if shift
            && self.editor().tool() == Tool::Brush
            && let Key::Character(digit) = bare
            && let Ok(n) = digit.parse::<usize>()
            && self.brushes.take_slot(n)
        {
            self.brushes_dirty = true;
            self.keep_brushes();
            self.redraw();
            return;
        }
        // With a tool that does not paint, a digit is the picked layers'
        // strength, as in Photoshop: `1` is 10% and `0` is all of it. The
        // brush keeps the digits for its own opacity.
        if !shift
            && matches!(
                self.editor().tool(),
                Tool::Select | Tool::Hand | Tool::Frame | Tool::Shape | Tool::Zoom
            )
            && let Some(d) = c.to_digit(10)
        {
            let strength = if d == 0 { 1.0 } else { f64::from(d) / 10.0 };
            let (editor, doc) = self.active();
            let change = editor.set_opacity(doc, strength);
            self.apply(change);
            return;
        }
        if shift && c.eq_ignore_ascii_case(&'l') {
            self.run_action(Action::Layers);
        } else if shift && c.eq_ignore_ascii_case(&'b') {
            self.run_action(Action::Library);
        } else if let Some(tool) = Tool::from_hotkey(c) {
            self.end_typing();
            let (editor, doc) = self.active();
            editor.choose_tool(tool, doc);
            self.redraw();
        } else if let Some(figure) = crate::editor::figure_for_key(c) {
            self.end_typing();
            let (editor, doc) = self.active();
            editor.choose_figure(figure, doc);
            self.redraw();
        } else if self.brush_key(c) {
            self.redraw();
        }
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
        let state = modifiers.state();
        let (editor, _) = self.active();
        editor.hold_ctrl(state.control_key());
        editor.hold_shift(state.shift_key());
        editor.hold_alt(state.alt_key());
        // Shift and Alt reshape the shape being dragged out as they go
        // down, not at the next move of the pointer.
        if editor.is_drawing() {
            self.redraw();
        }
        self.update_cursor_icon();
    }

    /// The pen, as the pointer it is: it goes down the same funnel as
    /// the mouse — strip, handle, panel, dock, canvas — so it picks tools
    /// and drags cards as well as it draws. `zwp_tablet_tool_v2` speaks
    /// surface-local logical px and the loop speaks physical, so the
    /// window's scale factor is the whole of the conversion.
    fn pen(&mut self, pen: Pen) {
        let Some(scale) = self.window.as_ref().map(|w| w.scale_factor()) else {
            return;
        };
        match pen {
            Pen::Axes(stylus) => self.stylus(stylus),
            Pen::Motion { x, y } => self.pointer_moved(x * scale, y * scale),
            Pen::Down => self.pointer_pressed(Button::Left),
            Pen::Up => self.pointer_released(Button::Left),
            Pen::Away => self.stylus(Stylus::MOUSE),
        }
    }

    /// What the pointer presses with from here on. The pen sets it out
    /// of its own frames; every mouse event puts it back, so a pen
    /// lifted off the tablet — which reads as no pressure at all —
    /// cannot leave the mouse painting nothing.
    fn stylus(&mut self, stylus: Stylus) {
        self.active().0.set_stylus(stylus);
    }

    fn gestured(&mut self, gesture: Gesture) {
        let Some(view) = self.view() else { return };
        let cursor = self.cursor.unwrap_or((
            f64::from(view.viewport.w) / 2.0,
            f64::from(view.viewport.h) / 2.0,
        ));
        if let Some(camera) = self.active().0.gesture(&view, cursor, gesture) {
            self.apply(Change::Camera(camera));
        }
    }

    /// Keys can't be released into a window that lost focus: drop the held
    /// overrides and whatever gesture they were driving.
    fn focus_lost(&mut self) {
        if let Some(s) = self.sending.as_mut() {
            s.selecting = false;
        }
        // A brush half-carried out of the library is put down where it
        // came from: it was never seated, and the hand it is in was the
        // press's doing, not the drag's.
        self.drag = None;
        self.fading = false;
        self.close_menu(None);
        // A card the pointer was carrying goes back where it came from:
        // nothing moves until a drop, and losing the window is not one.
        // The card still has to settle.
        self.pressed = None;
        let carrying = match self.carry.as_mut().filter(|c| c.held) {
            Some(carry) => {
                carry.held = false;
                carry.aim = None;
                true
            }
            None => false,
        };
        let (editor, doc) = self.active();
        editor.hold_space(false);
        editor.hold_ctrl(false);
        editor.hold_shift(false);
        editor.hold_alt(false);
        let cancelled = editor.cancel(doc);
        // Losing the window is as much a resting point as letting go of
        // the button: a card dropped this way left a stack behind, and
        // a cancelled drag put one back. Whichever it was, the board is
        // now at rest — and a board that did not move writes nothing.
        self.remember();
        if cancelled || carrying {
            self.redraw();
        }
        self.update_cursor_icon();
    }

    /// Cursor for the tool the pointer would use over the canvas; arrow
    /// over the chrome; resize and rotate cursors over the selection
    /// handles.
    fn update_cursor_icon(&mut self) {
        let (over_chrome, handle, tool, refused) = match (self.view(), self.cursor) {
            (Some(view), Some((x, y))) => (
                self.over_chrome(&view, (x, y)),
                self.editor().hover(self.doc(), &view, (x, y)),
                self.editor().pointer_tool(self.doc(), &view, (x, y)),
                self.editor().refuses_ink(self.doc(), &view, (x, y)),
            ),
            _ => (false, None, self.editor().active_tool(), false),
        };
        // A layer card and the canvas are both held in a closed hand.
        let held = self.carry.as_ref().is_some_and(|c| c.held);
        // Over the dialog's box the pointer is the I-beam that says a
        // press there puts the caret down; over the rest of it, and over
        // the board behind it, an arrow.
        let over_text = match (self.cursor, self.view()) {
            (Some((x, y)), Some(view)) => {
                self.send_panel(&view).map(|(p, _)| p.line.contains(x, y))
            }
            _ => None,
        };
        // Over the text being typed the pointer is the I-beam whatever the
        // tool, since a press there puts the caret down.
        let over_typed = match (self.cursor, self.view()) {
            (Some(at), Some(view)) if self.editor().typing().is_some() && !over_chrome => {
                self.editor().text_at(self.doc(), &view, at).as_deref()
                    == self.editor().typing().map(|t| t.id.as_str())
            }
            _ => false,
        };
        let icon = if over_text == Some(true) || over_typed {
            CursorIcon::Text
        } else if over_text == Some(false) {
            CursorIcon::Default
        } else if held || self.editor().is_panning() {
            CursorIcon::Grabbing
        } else if self.editor().is_drawing() {
            CursorIcon::Crosshair
        } else if self.editor().is_moving() {
            CursorIcon::Move
        } else if over_chrome {
            CursorIcon::Default
        } else if refused {
            // The layer the brush would paint on is locked: say so before
            // the press, which would be refused.
            CursorIcon::NotAllowed
        } else {
            match (tool, handle) {
                (Tool::Select, Some(Handle::Resize(Corner::TopLeft | Corner::BottomRight))) => {
                    CursorIcon::NwseResize
                }
                (Tool::Select, Some(Handle::Resize(_))) => CursorIcon::NeswResize,
                (Tool::Select, Some(Handle::Rotate(_) | Handle::End(_))) => CursorIcon::Crosshair,
                (Tool::Select, None) => CursorIcon::Default,
                (Tool::Hand, _) => CursorIcon::Grab,
                (Tool::Pencil | Tool::Brush | Tool::Frame | Tool::Shape, _) => CursorIcon::Crosshair,
                (Tool::Text, _) => CursorIcon::Text,
                (Tool::Zoom, _) => CursorIcon::ZoomIn,
            }
        };
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(w) = &self.window {
                w.set_cursor(icon);
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        if self.window.is_some() {
            return;
        }
        // The app id is the desktop entry's name: what the compositor and
        // the launcher match the window to, and so what gives it an icon.
        let attrs = Window::default_attributes()
            .with_title(format!("Sinopia — {}", self.project().label()))
            .with_name("sinopia", "");
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return self.fail(event_loop, anyhow::anyhow!("creating window: {e}")),
        };
        match Gfx::new(window.clone()) {
            Ok(mut gfx) => {
                self.letters_slot = gfx.letters_slot();
                self.gfx = Some(gfx);
                let proxy = self.proxy.clone();
                let sink = move |g| proxy.send_event(UserEvent::Gesture(g)).is_ok();
                if let Err(e) = gestures::spawn(&window, Box::new(sink)) {
                    log::warn!("trackpad gestures unavailable: {e:#}");
                }
                let proxy = self.proxy.clone();
                let sink = move |p| proxy.send_event(UserEvent::Pen(p)).is_ok();
                if let Err(e) = tablet::spawn(&window, Box::new(sink)) {
                    log::warn!("tablet unavailable: {e:#}");
                }
                let proxy = self.proxy.clone();
                let sink: clipboard::Sink = std::sync::Arc::new(move |p: Paste| {
                    if clipboard::is_text(&p.mime) {
                        let text = String::from_utf8_lossy(&p.bytes).into_owned();
                        return proxy.send_event(UserEvent::PastedText(text)).is_ok();
                    }
                    if clipboard::is_layers(&p.mime) {
                        let parsed = std::str::from_utf8(&p.bytes)
                            .map_err(anyhow::Error::from)
                            .and_then(Document::from_json);
                        return match parsed {
                            Ok(clip) => proxy.send_event(UserEvent::PastedLayers(Box::new(clip))).is_ok(),
                            Err(e) => {
                                log::warn!("pasting layers: {e:#}");
                                true
                            }
                        };
                    }
                    // Decoding a 4K screenshot is tens of milliseconds:
                    // it happens here, on the paste thread, not on the loop.
                    match bitmap::decode(&p.bytes) {
                        Ok(bitmap) => proxy
                            .send_event(UserEvent::Pasted {
                                bytes: p.bytes,
                                bitmap,
                            })
                            .is_ok(),
                        Err(e) => {
                            log::warn!("pasting {}: {e:#}", p.mime);
                            true
                        }
                    }
                });
                match Clipboard::spawn(&window, sink) {
                    Ok(clipboard) => self.clipboard = Some(clipboard),
                    Err(e) => log::warn!("clipboard unavailable: {e:#}"),
                }
                let proxy = self.proxy.clone();
                self.dialog_sink = Some(Arc::new(move |reply| {
                    if proxy.send_event(UserEvent::Dialog(reply)).is_err() {
                        log::warn!("event loop gone; dialog answer dropped");
                    }
                }));
                self.window = Some(window);
                self.ensure_atlas();
                self.ensure_sheets();
                self.load_images();
                self.redraw();
            }
            Err(e) => self.fail(event_loop, e),
        }
    }

    /// The loop is over: whatever the last gesture changed about the
    /// brushes is kept, in case it was the one that closed the window.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.keep_brushes();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.handle_window_event(event_loop, event);
        if self.closing {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, ev: UserEvent) {
        self.handle_user_event(ev);
        if self.closing {
            event_loop.exit();
        }
    }

    /// The loop is about to sleep, which is where a debt comes due: the
    /// hand has stopped, so the drafts are written. A loop that sleeps in
    /// `Wait` would never wake for a deadline of its own, so an unpaid
    /// debt asks for `WaitUntil` instead — and nothing else here does,
    /// which is why the sleep goes back to `Wait` once it is paid.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The caret of a text being typed blinks: the loop wakes when it
        // is next due to turn, whatever else it is waiting for.
        if let Some(blink) = self.next_blink() {
            if self.caret_drawn.get() != Some(self.caret_on())
                && let Some(w) = &self.window
            {
                w.request_redraw();
            }
            if self.owed.is_none() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(blink));
                return;
            }
        }
        let Some(due) = self.owed else { return };
        // A board in the middle of a gesture — or of a menu trying modes
        // on it — is not what the person has. The debt waits for the rest,
        // and the event that brings it wakes the loop anyway.
        if !self.settled() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if Instant::now() < due {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
            return;
        }
        self.keep_drafts();
        event_loop.set_control_flow(ControlFlow::Wait);
        if self.closing {
            event_loop.exit();
        }
    }
}

impl App {
    fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // The chrome is sized in logical px: a new scale factor
                // asks for glyphs at a new size.
                self.ensure_atlas();
                self.ensure_sheets();
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.stylus(Stylus::MOUSE);
                self.pointer_moved(position.x, position.y);
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                // The ring goes with the pointer.
                if self.editor().tool() == Tool::Brush {
                    self.redraw();
                }
                self.update_cursor_icon();
            }
            WindowEvent::Focused(false) => self.focus_lost(),
            WindowEvent::Focused(true) => {
                // The theme switcher takes the keyboard and gives it
                // back, so coming back into focus is the moment a theme
                // that changed under the window has to be noticed. One
                // stat, and a re-read only where it moved.
                if omarchy::stamp(&self.home) != self.style.stamp {
                    self.restyle();
                }
                // The person has just come back from the terminal they
                // may have started an agent in.
                if let Some(sending) = self.sending.as_mut() {
                    let found = agents::list();
                    if found.is_empty() {
                        self.sending = None;
                    } else {
                        // Aimed at the same agent, wherever the list now
                        // puts it — it comes back with the focused one
                        // first — or at that one, if it has gone.
                        let aimed = sending
                            .agents
                            .get(sending.target)
                            .and_then(|a| agents::find(&found, a))
                            .unwrap_or(0);
                        sending.agents = found;
                        sending.aim(aimed);
                    }
                    self.redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Middle => Button::Middle,
                    MouseButton::Right => Button::Right,
                    _ => return,
                };
                self.stylus(Stylus::MOUSE);
                match state {
                    ElementState::Pressed => self.pointer_pressed(button),
                    ElementState::Released => self.pointer_released(button),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.scrolled(delta),
            WindowEvent::ModifiersChanged(m) => self.modifiers_changed(m),
            // A key held down repeats inside a field, as it does in any
            // text box — and nowhere else, where a repeat would redo a
            // tool change or a command every thirtieth of a second.
            WindowEvent::KeyboardInput { event, .. }
                if !event.repeat
                    || (self.typing()
                        && repeats(&event.logical_key, self.modifiers.state().shift_key())) =>
            {
                self.key(
                    &event.logical_key,
                    &event.key_without_modifiers(),
                    event.text.as_deref(),
                    event.state,
                );
            }
            WindowEvent::RedrawRequested => {
                self.tick();
                if self.animating() {
                    self.redraw();
                }
                // A frame is drawn with the atlas in hand, and the two
                // things that name one — the theme's face and size, and
                // the window's scale factor — both move under it. Here
                // is where the new one is built, since `frame` only
                // reads: an atlas nobody put back would leave every
                // lettered surface drawing nothing at all.
                self.ensure_atlas();
                self.ensure_picture();
                self.ensure_thumbs();
                let Some(view) = self.view() else { return };
                let frame = self.frame(&view);
                let Some(gfx) = &mut self.gfx else { return };
                gfx.sync_letters(&self.glyphs);
                let drawn = gfx.render(self.theme.bg, &frame);
                if self.start_letters_over() {
                    self.redraw();
                }
                match drawn {
                    Ok(presented) => {
                        if let Some(n) = &mut self.smoke_frames_left {
                            if presented {
                                *n = n.saturating_sub(1);
                            }
                            if *n == 0 {
                                log::info!("smoke test ok: frames presented");
                                event_loop.exit();
                            } else {
                                self.redraw();
                            }
                        }
                    }
                    Err(e) => self.fail(event_loop, e),
                }
            }
            _ => {}
        }
    }

    fn handle_user_event(&mut self, ev: UserEvent) {
        let req = match ev {
            UserEvent::Request(req) => req,
            UserEvent::Ask(req, back) => {
                let ev = self.answer(req);
                // The socket thread may have given up waiting; the work
                // is done either way, and a board it changed stays
                // changed.
                let _ = back.send(ev);
                return;
            }
            UserEvent::Gesture(g) => return self.gestured(g),
            UserEvent::Pen(p) => return self.pen(p),
            UserEvent::Pasted { bytes, bitmap } => return self.pasted(bytes, bitmap),
            UserEvent::PastedText(text) => return self.pasted_text(&text),
            UserEvent::PastedLayers(clip) => return self.pasted_layers(*clip),
            UserEvent::Dialog(reply) => return self.dialog_replied(reply),
        };
        match req {
            Request::Raise => {
                if let Some(w) = &self.window {
                    w.focus_window();
                }
            }
            // Nothing is written yet. A new board is a draft, and a draft
            // earns its file the first time it is drawn on — an empty one
            // has nothing to lose and no business in the recents.
            Request::New => self.open_project(Project::untitled()),
            Request::Open { id } => {
                if let Some(i) = self.tab_with_key(&id) {
                    return self.activate(i);
                }
                match self.store.load(&id) {
                    Ok(doc) => {
                        self.open_project(Project::opened(doc, Origin::Board(id)));
                    }
                    Err(e) => log::error!("opening board {id:?}: {e:#}"),
                }
            }
            Request::OpenFile { path } => {
                if let Some(i) = self.tab_with_path(&path) {
                    return self.activate(i);
                }
                match store::load_document_from(&path) {
                    Ok(doc) => self.open_project(Project::opened(doc, Origin::File(path))),
                    Err(e) => {
                        // The recents pointed somewhere that is no longer
                        // there. Reaching for it is what proves that, so
                        // it is here — and only here — that the entry goes.
                        log::warn!("dropping {path:?} from the recents: {e:#}");
                        if let Err(e) = self.store.forget_path(&path) {
                            log::error!("updating the recents: {e:#}");
                        }
                    }
                }
            }
            Request::Shutdown => self.quit(),
            Request::Theme { colors } => match colors {
                // The plugin's own three, which is how a host that is not
                // Omarchy dresses the board.
                Some(c) => {
                    self.theme = Theme::from_hex(&c.bg, &c.fg, &c.accent).wearing(&self.style);
                    self.dress_editors();
                    self.redraw();
                }
                None => self.restyle(),
            },
            // The server answers `denied` without forwarding; never reaches here.
            Request::Export { .. } | Request::Ping => {}
            // An agent's three and the layers' ops go the other way: the
            // loop *does* them, on the `Ask` path, and the answer is what
            // goes back.
            Request::Frames
            | Request::ReadFrame { .. }
            | Request::AddFrame { .. }
            | Request::Layers
            | Request::AddLayer { .. }
            | Request::RemoveLayers { .. }
            | Request::RenameLayer { .. }
            | Request::MoveLayers { .. }
            | Request::ArrangeLayers { .. }
            | Request::ShowLayers { .. }
            | Request::LockLayers { .. }
            | Request::SetOpacity { .. }
            | Request::SetBlend { .. }
            | Request::SetColor { .. }
            | Request::GroupLayers { .. }
            | Request::Ungroup { .. }
            | Request::DuplicateLayers { .. }
            | Request::MergeLayers { .. }
            | Request::MergeDown { .. }
            | Request::MergeVisible
            | Request::Flatten
            | Request::SelectLayers { .. }
            | Request::OpenLayers { .. }
            | Request::Texts
            | Request::AddText { .. }
            | Request::SetText { .. } => {}
        }
    }
}

/// Puts back the blend modes `was` says every layer had.
/// What a paint menu's line asks of the shapes: their stroke, or their
/// fill.
fn paint_change(stroke: bool, paint: Option<String>) -> crate::editor::Restyle {
    match stroke {
        true => crate::editor::Restyle::Stroke(paint),
        false => crate::editor::Restyle::Fill(paint),
    }
}

fn restore_blends(doc: &mut Document, was: &[(String, BlendMode)]) {
    for (id, mode) in was {
        if let Some(l) = doc.layer_mut(id) {
            l.blend = *mode;
        }
    }
}

/// A strength read off the slider, to the whole percent the bar writes it
/// as: a board keeps `0.19`, not the float the pointer's x made of it.
fn whole_percent(fraction: f32) -> f64 {
    (f64::from(fraction) * 100.0).round() / 100.0
}

/// The face the desktop letters itself with, or the bundled one where
/// there is none this build can read.
fn face(style: &Style) -> Font {
    style
        .face
        .as_deref()
        .and_then(Font::from_file)
        .unwrap_or_else(Font::bundled)
}

/// How long a fragment may be. What arrives is one page of a board, and
/// a page is a document written out pretty-printed: a real hand-drawn
/// frame of a few hundred brush strokes measures a couple of megabytes,
/// so the ceiling is well above what `agent read` itself writes — a cap
/// the documented round trip trips over would be worse than none, since
/// the agent only finds it after doing the work.
const MAX_FRAGMENT_BYTES: u64 = 64 * 1024 * 1024;

/// The fragment `add_frame` names, read and parsed. The parse is the
/// board's own — closed schema, settled layers, blob names checked — so
/// nothing that would not open as a board can be grafted onto one.
/// Whether a key held down in a field may repeat: the ones that write or
/// move, and none that finishes something — Enter sends, Tab changes the
/// target, Esc closes — since a repeat of those does again what the hand
/// asked for once. A held Enter that took a skill off the menu would send
/// the unfinished prompt on its first repeat. `Shift+Enter` writes.
fn repeats(key: &Key, shift: bool) -> bool {
    match key {
        Key::Named(NamedKey::Enter) => shift,
        Key::Named(NamedKey::Tab | NamedKey::Escape) => false,
        _ => true,
    }
}

/// What a key does to the field that has the keyboard, and whether it
/// did anything. Shift carries a selection along with the caret and Ctrl
/// walks a word at a time; a letter held with Ctrl or Super is a command
/// and never text, so `Ctrl+A` selects everything rather than typing an
/// `a` — and the ones this does not know type nothing at all.
fn edit(field: &mut Field, key: &Key, mods: winit::keyboard::ModifiersState) -> bool {
    let (shift, ctrl) = (mods.shift_key(), mods.control_key());
    match key {
        Key::Named(NamedKey::Backspace) => field.backspace(),
        Key::Named(NamedKey::Delete) => field.delete(),
        Key::Named(NamedKey::ArrowLeft) if ctrl => field.word_left(shift),
        Key::Named(NamedKey::ArrowRight) if ctrl => field.word_right(shift),
        Key::Named(NamedKey::ArrowLeft) => field.left(shift),
        Key::Named(NamedKey::ArrowRight) => field.right(shift),
        Key::Named(NamedKey::Home) => field.home(shift),
        Key::Named(NamedKey::End) => field.end(shift),
        // A space is a named key and never a character, so without this
        // an instruction is one word long.
        Key::Named(NamedKey::Space) => field.insert(' '),
        Key::Character(text) if ctrl || mods.super_key() => {
            if !text.eq_ignore_ascii_case("a") {
                return false;
            }
            field.select_all();
        }
        Key::Character(text) => {
            for c in text.chars().filter(|c| !c.is_control()) {
                field.insert(c);
            }
        }
        _ => return false,
    }
    true
}

fn read_fragment(path: &Path) -> anyhow::Result<Document> {
    let text = String::from_utf8(store::read_capped(path, MAX_FRAGMENT_BYTES)?)
        .with_context(|| format!("reading {path:?}"))?;
    Document::from_json(&text).with_context(|| format!("parsing {path:?}"))
}

/// The pages `.sinopia/` in `cwd` already holds. A directory that is
/// not there holds nothing, which is the answer, not an error.
fn taken_names(cwd: &str) -> Vec<String> {
    let at = std::path::Path::new(cwd).join(export::BOARDS_DIR);
    std::fs::read_dir(at)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Brings up server + window and runs until the user closes it (or the
/// smoke test is done).
pub fn run(
    store: Store,
    first: Project,
    socket_path: PathBuf,
    smoke_frames: Option<u32>,
) -> anyhow::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let shared = Arc::new(Mutex::new(SharedState {
        board_id: first.doc.id.clone(),
    }));

    let server = Server::bind(&socket_path)?;
    let shared_for_server = shared.clone();
    server.serve(move |req| {
        // An agent's three are answered by *doing* them, which only the
        // loop can: the live document and the GPU are both over there.
        // So the thread hands the request across and waits — a loop that
        // is gone, or a deadline that passes, is a denial the caller can
        // read rather than a socket dying under it.
        if req.is_asked() {
            let op = req.op().to_owned();
            let (back, wait) = mpsc::channel();
            if proxy.send_event(UserEvent::Ask(req, back)).is_err() {
                return Event::Denied {
                    op,
                    reason: "the board is closing".into(),
                };
            }
            return match wait.recv_timeout(ASK_TIMEOUT) {
                Ok(ev) => ev,
                // A deadline that passes is not the work undone: the
                // loop finishes what it started, and a board it changed
                // stays changed. `add_frame` is not idempotent, so an
                // agent told a flat "no" retries and plants the frame
                // twice — the denial has to say what it actually knows.
                Err(mpsc::RecvTimeoutError::Timeout) => Event::Denied {
                    op,
                    reason: format!(
                        "the board did not answer within {}s; it may still be doing \
                         the work, so look before asking again",
                        ASK_TIMEOUT.as_secs()
                    ),
                },
                // The loop went away with the request still in its
                // queue, which is a different thing and comes back at
                // once rather than after the deadline.
                Err(mpsc::RecvTimeoutError::Disconnected) => Event::Denied {
                    op,
                    reason: "the board closed before it answered".into(),
                },
            };
        }
        let reply = match &req {
            Request::Shutdown => Event::Exited { code: 0 },
            Request::Export { .. } => Event::Denied {
                op: "export".into(),
                reason: "export not implemented yet".into(),
            },
            // Generic ack: current id + pid (§5). `new`/`open` switch boards
            // asynchronously on the event loop; a synchronous reply with the
            // new id comes when the plugin needs it (§15.3).
            _ => Event::Ready {
                id: shared_for_server
                    .lock()
                    .expect("lock shared")
                    .board_id
                    .clone(),
                pid: std::process::id(),
            },
        };
        let forward = !matches!(req, Request::Ping | Request::Export { .. });
        if forward && proxy.send_event(UserEvent::Request(req)).is_err() {
            log::warn!("event loop gone; request dropped");
        }
        reply
    });

    // `main` resolved which project the window opens on: a draft out of
    // the store, a file the recents remembered, or — for `--new` and for
    // a first run with nothing behind it — an untitled one that has not
    // been written anywhere yet, and will not be until it is drawn on.
    let store_brushes = store.brushes();
    // What the desktop is wearing, where the desktop is Omarchy. Every
    // piece falls back on its own, so a machine without it opens the
    // board in the colours it has always had.
    let home = std::env::var("HOME").unwrap_or_default();
    let style = omarchy::read(&home);
    let theme = Theme::from_style(&style);
    let ink = theme.first_ink();
    let mut app = App {
        store,
        open: vec![{
            let editor = Editor::new();
            let history = History::new(&first.doc, editor.at());
            Open {
                project: first,
                editor,
                history,
            }
        }],
        active: 0,
        shared,
        proxy: event_loop.create_proxy(),
        window: None,
        gfx: None,
        theme,
        font: face(&style),
        style,
        home,
        brushes: {
            // The shipped sets, dressed in whatever the person kept of
            // them: a library file is a list of exceptions, so a brush
            // nobody touched is still whatever the assets now say.
            let mut lib = Library::default();
            lib.apply(&store_brushes);
            lib
        },
        layers_shown: false,
        palette_shown: true,
        library_shown: false,
        palette_scroll: 0.0,
        props_open: false,
        ink,
        brushes_dirty: false,
        grab: None,
        dock_icon_slot: None,
        icon_slot: 0,
        agent_logo_slot: None,
        shapes: Shapes::default(),
        fonts: crate::fonts::machine(),
        text_bar_open: false,
        text_grab: None,
        shape_grab: None,
        typed_at: Instant::now(),
        caret_drawn: std::cell::Cell::new(None),
        pasting_words: false,
        menu_typed: (String::new(), Instant::now()),
        last_press: None,
        glyphs: crate::glyphs::Glyphs::default(),
        letters_restarted: std::cell::Cell::new(false),
        letters_slot: 0,
        shown_brush: None,
        carry: None,
        pressed: None,
        fading: false,
        menu: None,
        thumbs: None,
        thumbs_stale: true,
        drag: None,
        renaming: None,
        searching: None,
        last_card: None,
        sending: None,
        slides: layers::Slides::default(),
        scroll: 0.0,
        scrolling: layers::Coming::default(),
        focused: None,
        clock: Instant::now(),
        atlas: None,
        atlas_slot: 0,
        clipboard: None,
        clip: None,
        dialog_sink: None,
        pending: None,
        owed: None,
        quitting: false,
        closing: false,
        cursor: None,
        modifiers: Modifiers::default(),
        cursor_icon: CursorIcon::Default,
        smoke_frames_left: smoke_frames,
        exit_error: None,
    };
    // The first tab is built before the theme is in hand, so it is told
    // what a new frame's ground is once the window owns both.
    app.dress_editors();
    event_loop.run_app(&mut app)?;

    // Window closed: the final flush happens on the exit paths; the socket
    // dies with the process (the file stays; the next bind detects and
    // replaces it).
    match app.exit_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::ModifiersState;

    #[test]
    fn a_strength_off_the_slider_is_a_whole_percent() {
        assert_eq!(whole_percent(0.193_548_38), 0.19);
        assert_eq!(whole_percent(1.0), 1.0);
        assert_eq!(whole_percent(0.0), 0.0);
        assert_eq!(whole_percent(0.506), 0.51);
    }

    #[test]
    fn every_key_a_rows_menu_teaches_is_that_commands() {
        let (items, lines) = layers::row_menu(|_| true, true, layers::RowState {
            locked: false,
            hidden: false,
            tag: Tag::None,
            merge: "Merge Down",
        });
        for (item, line) in items.iter().zip(&lines) {
            let (Some(hint), layers::RowLine::Run(command)) = (&item.hint, line) else {
                continue;
            };
            let Some(keys) = hint.strip_prefix("Ctrl+") else {
                assert_eq!(hint, "Del", "the one key without Ctrl");
                continue;
            };
            let (alt, keys) = match keys.strip_prefix("Alt+") {
                Some(k) => (true, k),
                None => (false, keys),
            };
            let (shift, key) = match keys.strip_prefix("Shift+") {
                Some(k) => (true, k),
                None => (false, keys),
            };
            assert_eq!(
                menubar::shortcut(&key.to_lowercase(), shift, alt),
                Some(Action::Layer(*command)),
                "{hint}"
            );
        }
    }

    #[test]
    fn a_held_key_that_finishes_something_does_not_repeat() {
        // Enter sends, Tab changes the target, Esc closes: a repeat of
        // any of them does again what the hand asked for once — a held
        // Enter taking a skill would send the prompt on its first repeat.
        // Shift+Enter writes a line, and writing repeats.
        assert!(!repeats(&Key::Named(NamedKey::Enter), false));
        assert!(!repeats(&Key::Named(NamedKey::Tab), false));
        assert!(!repeats(&Key::Named(NamedKey::Escape), false));
        assert!(repeats(&Key::Named(NamedKey::Enter), true));
        assert!(repeats(&Key::Named(NamedKey::Backspace), false));
        assert!(repeats(&Key::Named(NamedKey::ArrowLeft), false));
        assert!(repeats(&Key::Character("a".into()), false));
    }

    #[test]
    fn a_letter_held_with_ctrl_is_a_command_and_never_text() {
        let mut f = Field::new("ab");
        let v = Key::Character("v".into());
        assert!(!edit(&mut f, &v, ModifiersState::CONTROL));
        assert_eq!(f.value(), "ab", "Ctrl+V used to type a v");
        let a = Key::Character("a".into());
        assert!(edit(&mut f, &a, ModifiersState::CONTROL));
        assert_eq!(f.selected(), "ab");
    }
}
