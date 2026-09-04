//! Window lifecycle (ARCHITECTURE.md §11): winit + wgpu + socket.
//!
//! The IPC server runs on its own thread and injects requests into the
//! event loop through `EventLoopProxy`; its replies are immediate acks (the
//! state that actually changes, changes here, on the loop thread). Input is
//! routed to the pure `editor` and `dock`; this file only maps events and
//! assembles frames, so it stays thin and the logic stays testable.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context as _;
use image::ImageEncoder as _;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::agents;
use crate::bitmap::{self, Bitmap};
use crate::brush::{self, Library};
use crate::clipboard::{self, Clipboard, Paste};
use crate::dialogs::{self, Answer, Reply};
use crate::doc::{Document, Element};
use crate::dock::{Dock, Hit};
use crate::editor::{Button, Change, Editor, Gesture, SCROLL_LINE_PX, Stylus, Tool};
use crate::export;
use crate::field::Field;
use crate::geom::Corner;
use crate::gestures;
use crate::history::History;
use crate::gfx::Gfx;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::layers::{self, Panel, PanelHit};
use crate::omarchy::{self, Style};
use crate::palette::{self, Palette};
use crate::slots::{self, Strip};
use crate::props::{self, Props};
use crate::project::{self, Origin, Project};
use crate::scene::{self, Frame, ImageSlots, Prim, Rgba, Shapes, View, Viewport, with_alpha};
use crate::select::{self, Handle};
use crate::send;
use crate::store::{self, Store};
use crate::tablet::{self, Pen};
use crate::tabs::{self, TabHit, Tabs};
use crate::text::{Atlas, Font};
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

/// State the server thread reads (replies to `ping`).
struct SharedState {
    board_id: String,
}

#[derive(Debug)]
enum UserEvent {
    Request(Request),
    Gesture(Gesture),
    /// One step of the tablet's pen, straight off the protocol.
    Pen(Pen),
    /// A clipboard image, already decoded off the loop. The original
    /// bytes go to the blob store; the texels go to the GPU.
    Pasted {
        bytes: Vec<u8>,
        bitmap: Bitmap,
    },
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
    /// Where each nib shape sits on the shape sheet, once it has been
    /// uploaded. Empty until then, and a stroke that names a shape lays
    /// a plain round nib meanwhile.
    shapes: Shapes,
    /// The brush the palette last brought into sight. A change of hand
    /// glides the list to it; scrolling away from it does not snap back.
    shown_brush: Option<(usize, usize)>,
    /// The layer card the pointer picked up, if any. It outlives the
    /// release, easing back into the stack.
    carry: Option<Carry>,
    /// A card being renamed: the row's index and the name being typed.
    /// It is the window's, not a tab's — like every other panel state.
    renaming: Option<(usize, Field)>,
    /// When the last press landed on a card, and on which. A second
    /// press on the same card inside [`DOUBLE_CLICK`] opens the rename.
    last_card: Option<(usize, Instant)>,
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
    /// The frame the panel was last standing in. A different stack under
    /// it means the slides and the scroll are measured against a list
    /// that is not there any more, so they start over.
    standing: Option<String>,
    /// When the panel was last eased, for everything on it that moves.
    clock: Instant,
    /// Built once the scale factor is known, rebuilt when it changes.
    atlas: Option<Atlas>,
    atlas_slot: u32,
    clipboard: Option<Clipboard>,
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
    /// The folder, when the scope has no name of its own.
    folder: Option<Field>,
    line: Field,
    /// Which field the keyboard is writing into. The line, until a press
    /// says otherwise — the folder's prefill is usually right and the
    /// line never is.
    focus: send::Hit,
}

impl Sending {
    /// The field the keyboard is writing into.
    fn writing(&mut self) -> &mut Field {
        match (self.focus, self.folder.as_mut()) {
            (send::Hit::Folder, Some(folder)) => folder,
            _ => &mut self.line,
        }
    }
}

/// A layer card in the pointer's hand: which row it came from, where it
/// is being carried, and how far into the lift it is. It stays after the
/// button comes up, running the lift backwards until the card is a row
/// again.
struct Carry {
    /// Into the document's layers, kept level with the active layer.
    index: usize,
    /// Between the press and the card's top edge: what it is held by.
    grab_dy: f32,
    /// Where the card's top edge is asked to be, in physical px.
    y: f32,
    /// The button is still down.
    held: bool,
    /// The lift, 0 to 1, walked toward `held` by the clock.
    t: f32,
}

impl Carry {
    fn lift(&self) -> layers::Lift {
        layers::Lift {
            index: self.index,
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
        !self.editor().busy() && !self.carry.as_ref().is_some_and(|c| c.held)
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
            true => history.keep(&project.doc, spot),
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
        self.slides
            .restack(project.doc.stack(editor.inside()), row);
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
        let inside = self.editor().inside();
        let stack = self.doc().stack(inside);
        let index = self.editor().active_layer(self.doc());
        let id = stack.get(index).map(|l| l.id.clone());
        let depth = stack.len();
        if self.focused == id {
            return;
        }
        // Going into a frame or out of it puts a different stack under
        // the panel: a slide carried across it would animate a row into a
        // row that is not the same row, and a scroll kept would be
        // measured against a list that is not there any more.
        if self.standing != inside.map(str::to_owned) {
            self.standing = inside.map(str::to_owned);
            self.slides = layers::Slides::default();
            self.scrolling = layers::Coming::default();
            self.scroll = 0.0;
        }
        self.focused = id;
        // A card in the hand takes the panel where the pointer says.
        let Some(panel) = self.panel(&view).filter(|_| self.carry.is_none()) else {
            return;
        };
        let want = panel.scroll_showing(index, depth);
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
        w.set_title(&format!("{mark}Omawhite — {}", project.label()));
    }

    /// Closes tab `index`, asking about unsaved work first. A draft is
    /// kept on the way out rather than asked about: the question is for
    /// work that would otherwise be lost, and a draft's never is.
    fn request_close(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
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
    fn open_project(&mut self, project: Project) {
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
        self.active = index;
        let mods = self.modifiers.state();
        let (editor, _) = self.active();
        editor.hold_ctrl(mods.control_key());
        editor.hold_shift(mods.shift_key());
        editor.hold_space(false);
        self.shared.lock().expect("lock shared").board_id = self.doc().id.clone();
        self.retitle();
        self.load_images();
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

    /// Asks the clipboard for an image. The bytes arrive later, as
    /// [`UserEvent::Pasted`].
    fn paste(&mut self) {
        match &self.clipboard {
            Some(clipboard) => {
                clipboard.paste_image();
            }
            None => log::debug!("paste: no clipboard on this display"),
        }
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
        ))
    }

    /// The layers panel, when it is up and there is an atlas to letter
    /// it with.
    fn panel(&self, view: &View) -> Option<Panel> {
        if !self.layers_shown {
            return None;
        }
        let atlas = self.atlas.as_ref()?;
        let top = self.strip_top(view);
        // The panel shows one flat stack, whichever one the editor is
        // standing in — which is what keeps its lift, its slides and its
        // scroll from having to know that frames exist at all.
        let inside = self.editor().inside();
        let name = inside.and_then(|id| self.frame_name(id));
        Some(Panel::layout(
            view.viewport,
            self.chrome(view),
            top,
            atlas,
            self.doc().stack(inside),
            name,
            self.scroll + self.scrolling.offset(),
        ))
    }

    /// What a frame's card is called: the name of the layer it is the
    /// object of.
    fn frame_name(&self, id: &str) -> Option<&str> {
        let doc = self.doc();
        let layer = &doc.frame(id)?.layer;
        doc.layers
            .iter()
            .find(|l| &l.id == layer)
            .map(|l| l.name.as_str())
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

    /// Where the left-hand panels hang from: the properties bar when the
    /// brush is in hand, the tab strip otherwise.
    fn above(&self, view: &View) -> f32 {
        self.props(view)
            .map_or_else(|| self.strip_top(view), |b| b.rect.y + b.rect.h)
    }

    /// Whether `screen` is over the strip, the handle, either panel or
    /// the dock rather than the canvas.
    fn over_chrome(&self, view: &View, screen: (f64, f64)) -> bool {
        let (x, y) = screen;
        self.tabs(view).and_then(|t| t.hit(x, y)).is_some()
            || self.handle(view).is_some_and(|h| h.hit(x, y))
            || self.panel(view).and_then(|p| p.hit(x, y)).is_some()
            || self.props(view).and_then(|b| b.hit(x, y)).is_some()
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

    /// A click on the layers panel, handed to the editor.
    /// Writes the name being typed onto its layer and shuts the field.
    /// A name of nothing but space leaves the layer as it was, which is
    /// `rename_layer`'s own answer.
    fn commit_rename(&mut self) {
        let Some((index, field)) = self.renaming.take() else {
            return;
        };
        let name = field.value().to_owned();
        let (editor, doc) = self.active();
        let change = editor.rename_layer(doc, index, &name);
        self.apply(change);
    }

    /// What a press on the panel means once the clock is taken into
    /// account: a press on a card that is already selected, inside
    /// [`DOUBLE_CLICK`] of the last one on that same card, asks for the
    /// name rather than for the layer.
    fn second_press(&mut self, hit: PanelHit) -> PanelHit {
        let PanelHit::Select(i) = hit else {
            return hit;
        };
        let now = Instant::now();
        let again = self
            .last_card
            .is_some_and(|(was, at)| was == i && now.duration_since(at) < DOUBLE_CLICK);
        self.last_card = Some((i, now));
        if again { PanelHit::Rename(i) } else { hit }
    }

    fn panel_hit(&mut self, hit: PanelHit) {
        // A rename opens a field rather than changing the document, so
        // it is answered before the editor is borrowed. The name comes
        // off the stack: a row's own label is cut down to what fits.
        if let PanelHit::Rename(i) = hit {
            let inside = self.editor().inside();
            let name = self
                .doc()
                .stack(inside)
                .get(i)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            self.renaming = Some((i, Field::new(&name)));
            return;
        }
        let (editor, doc) = self.active();
        let change = match hit {
            PanelHit::Select(i) => editor.select_layer(doc, i),
            PanelHit::Toggle(i) => editor.toggle_layer(doc, i),
            PanelHit::Add => editor.add_layer(doc),
            PanelHit::Remove => editor.remove_layer(doc),
            PanelHit::Up => editor.move_layer(doc, true),
            PanelHit::Down => editor.move_layer(doc, false),
            PanelHit::Enter(i) => match doc
                .stack(editor.inside())
                .get(i)
                .and_then(|l| doc.frame_on(&l.id))
                .map(|f| f.id.clone())
            {
                Some(frame) => editor.enter_frame(doc, &frame),
                None => Change::None,
            },
            PanelHit::Leave => editor.leave_frame(doc),
            PanelHit::Rename(_) => Change::None,
            PanelHit::Panel => Change::None,
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
        // A frame brings its name; a loose selection is asked for one,
        // counted past whatever the folder already holds.
        let folder = match export::named(self.doc(), &scope) {
            Some(_) => None,
            None => {
                let taken = taken_names(&found[0].cwd);
                Some(Field::new(&export::free_name(&self.doc().title, &taken)))
            }
        };
        self.sending = Some(Sending {
            scope,
            agents: found,
            target: 0,
            folder,
            line: Field::new(""),
            focus: send::Hit::Line,
        });
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
        let name = export::named(self.doc(), &sending.scope)
            .or_else(|| sending.folder.as_ref().map(|f| f.value().to_owned()))
            .unwrap_or_default();
        let slug = match export::slug(&name) {
            s if s.is_empty() => anyhow::bail!("the page needs a name"),
            s => s,
        };
        let bounds = export::bounds(self.doc(), &sending.scope)
            .context("there is nothing in the selection to send")?;
        let sub = export::sub_document(self.doc(), &sending.scope);
        let md = export::inventory(&sub, &bounds);
        let blobs = self.blobs_of(&sub);
        let theme_bg = self.theme.bg;
        let edge = self.theme.muted;
        let shapes = std::mem::take(&mut self.shapes);
        let picture = {
            let images = self.gfx.as_ref().map(Gfx::image_slots);
            let none = ImageSlots::new();
            let images = images.unwrap_or(&none);
            let (view, w, h) = match &self.gfx {
                Some(gfx) => export::view_for(&bounds, gfx.max_dimension()),
                None => (export::view_for(&bounds, 1).0, 1, 1),
            };
            (
                scene::document_prims(self.doc(), &view, images, &shapes, edge, None),
                w,
                h,
            )
        };
        self.shapes = shapes;
        let (picture, w, h) = picture;
        let gfx = self
            .gfx
            .as_mut()
            .context("there is no window to draw with")?;
        let rgba = gfx.render_offscreen(w, h, theme_bg, &picture)?;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(
            &rgba,
            w,
            h,
            image::ExtendedColorType::Rgba8,
        )?;
        let cwd = std::path::Path::new(&agent.cwd);
        let files = export::write(cwd, &slug, &png, &sub, &md, &blobs)?;
        log::info!("exported {} files to {}", files.len(), agent.cwd);
        agents::send(agent, &agents::prompt(&line, &agents::relative(&files, cwd)))
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

    /// The three image sheets the binary ships: illustrated dock tools,
    /// brush icons for the library, and nib shapes for the canvas. They
    /// are raster art, the same at every scale, so each is uploaded once
    /// into a slot of its own and never replaced like the glyph atlas is.
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
            edge,
            live,
        ));
        if let Some(selection) = self.editor().selection_frame(self.doc()) {
            frame.extend(select::prims(&selection, view, &self.theme));
        }
        if let Some((a, b)) = self.editor().marquee() {
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
                None,
                atlas,
                self.atlas_slot,
                self.icon_slot,
                &self.theme,
            ));
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
        if let (Some(handle), Some(atlas)) = (self.handle(view), self.atlas.as_ref()) {
            frame.extend(handle.prims(atlas, self.atlas_slot, &self.theme));
        }
        if let (Some(panel), Some(atlas)) = (self.panel(view), self.atlas.as_ref()) {
            let active = self.editor().active_layer(self.doc());
            let showing = layers::Showing {
                active,
                lift: self.carry.as_ref().map(Carry::lift),
                slides: &self.slides,
            };
            frame.extend(panel.prims(
                self.doc().stack(self.editor().inside()),
                &showing,
                atlas,
                self.atlas_slot,
                &self.theme,
            ));
            // The name being typed is drawn over the card it belongs to,
            // rounded the way the card is: the row underneath goes on
            // showing its eye and its mark, so what is being renamed
            // stays in its place in the stack.
            if let Some((index, field)) = &self.renaming
                && let Some(row) = panel.rows.iter().find(|r| r.index == *index)
            {
                frame.extend([Prim::rounded(
                    row.card,
                    layers::ROW_RADIUS,
                    self.theme.panel,
                )]);
                frame.extend(field.prims(row.card, atlas, self.atlas_slot, &self.theme, true));
            }
        }
        if let (Some(tabs), Some(atlas)) = (self.tabs(view), self.atlas.as_ref()) {
            frame.extend(tabs.prims(atlas, self.atlas_slot, &self.theme));
        }
        // The send panel is modal, so it is drawn last of everything —
        // over the strip the way it is pressed before it.
        if let (Some(sending), Some(atlas)) = (&self.sending, self.atlas.as_ref()) {
            let panel = send::Panel::layout(
                view.viewport,
                self.chrome(view),
                sending.agents.len(),
                sending.folder.is_some(),
            );
            frame.extend(panel.prims(
                &sending.agents,
                sending.target,
                sending.folder.as_ref(),
                &sending.line,
                atlas,
                self.atlas_slot,
                &self.theme,
            ));
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
        match change {
            Change::None => {}
            Change::Selection => {
                self.remember();
                self.redraw();
            }
            Change::Scene => {
                self.remember();
                self.touch();
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
        if let Some(sending) = &self.sending {
            let panel = send::Panel::layout(
                view.viewport,
                self.chrome(&view),
                sending.agents.len(),
                sending.folder.is_some(),
            );
            if button == Button::Left {
                match panel.hit(x, y) {
                    Some(send::Hit::Target(i)) => {
                        if let Some(s) = self.sending.as_mut() {
                            s.target = i;
                        }
                    }
                    Some(hit) => {
                        if let Some(s) = self.sending.as_mut() {
                            s.focus = hit;
                        }
                    }
                    // A press outside a modal panel closes it.
                    None => self.sending = None,
                }
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        // A name being typed is finished by pressing somewhere else, as
        // Enter finishes it: the keyboard cannot be left held by a field
        // the pointer has walked away from.
        if self.renaming.is_some() && button == Button::Left {
            self.commit_rename();
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
            if button == Button::Left {
                // `Panel::hit` cannot see a second press: counting them
                // is the window's. A press on a card that is already
                // selected, soon enough after the last one, is what
                // makes a Select a Rename.
                let hit = self.second_press(hit);
                self.panel_hit(hit);
                // A card taken by its name is picked up by the grip the
                // press made, and follows the pointer from there. A card
                // whose name is open is not also lifted: a field is not
                // dragged.
                if let PanelHit::Select(i) = hit
                    && let Some(row) = panel.rows.iter().find(|r| r.index == i)
                {
                    self.carry = Some(Carry {
                        index: i,
                        grab_dy: y as f32 - row.card.y,
                        y: row.card.y,
                        held: true,
                        // A card caught while it was still settling
                        // carries on from where it had got to.
                        t: self.carry.as_ref().map_or(0.0, |c| c.t),
                    });
                }
                self.redraw();
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
                self.redraw();
            }
            return self.update_cursor_icon();
        }
        match self.dock(&view).hit(x, y) {
            Some(Hit::Tool(tool)) => {
                if button == Button::Left {
                    let (editor, doc) = self.active();
                    editor.set_tool(tool, doc);
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
                let tip = self.brushes.tip();
                let (editor, doc) = self.active();
                let change = editor.press(button, &view, (x, y), doc, &tip);
                self.apply(change);
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_released(&mut self, button: Button) {
        // A brush edit is over when the pointer that made it comes up.
        self.keep_brushes();
        // A slider let go of is just let go of: the canvas never saw the
        // press, so there is nothing under it to end.
        if button == Button::Left && self.grab.take().is_some() {
            self.redraw();
            return self.update_cursor_icon();
        }
        // A carried layer is left where the pointer put it; the canvas
        // never saw the press, so it has nothing to end. The card runs
        // the lift backwards into its row from here.
        if button == Button::Left
            && let Some(carry) = self.carry.as_mut().filter(|c| c.held)
        {
            carry.held = false;
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
        self.update_cursor_icon();
    }

    fn pointer_moved(&mut self, x: f64, y: f64) {
        self.cursor = Some((x, y));
        // A carried layer has the pointer to itself: the card follows
        // it, the stack opens at whichever row is under it, and the
        // canvas sees nothing.
        if self.carry.as_ref().is_some_and(|c| c.held) {
            if let Some(carry) = &mut self.carry {
                carry.y = y as f32 - carry.grab_dy;
            }
            if let Some(view) = self.view()
                && let Some(index) = self.panel(&view).and_then(|p| p.drop_index(y))
            {
                let (editor, doc) = self.active();
                let change = editor.move_layer_to(doc, index);
                self.apply(change);
                let index = self.editor().active_layer(self.doc());
                if let Some(carry) = &mut self.carry {
                    carry.index = index;
                }
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
    fn key(&mut self, key: &Key, bare: &Key, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        match key {
            // The send panel is modal: it takes the keyboard before
            // anything else, including the rename that cannot be open
            // under it.
            _ if self.sending.is_some() && pressed => {
                let Some(sending) = self.sending.as_mut() else {
                    return;
                };
                match key {
                    Key::Named(NamedKey::Escape) => self.sending = None,
                    Key::Named(NamedKey::Enter) => self.do_send(),
                    Key::Named(NamedKey::Tab) => {
                        // Tab walks the targets when there is more than
                        // one, since the fields are two at most and the
                        // list is the thing being chosen from.
                        let n = sending.agents.len();
                        sending.target = (sending.target + 1) % n.max(1);
                    }
                    Key::Named(NamedKey::Backspace) => sending.writing().backspace(),
                    Key::Named(NamedKey::ArrowLeft) => sending.writing().left(),
                    Key::Named(NamedKey::ArrowRight) => sending.writing().right(),
                    Key::Named(NamedKey::Home) => sending.writing().home(),
                    Key::Named(NamedKey::End) => sending.writing().end(),
                    // A space is a named key and never a character, so
                    // without this an instruction is one word long.
                    Key::Named(NamedKey::Space) => sending.writing().insert(' '),
                    Key::Character(text) => {
                        let field = sending.writing();
                        for c in text.chars().filter(|c| !c.is_control()) {
                            field.insert(c);
                        }
                    }
                    _ => {}
                }
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
                    Key::Named(NamedKey::Backspace) => field.backspace(),
                    Key::Named(NamedKey::ArrowLeft) => field.left(),
                    Key::Named(NamedKey::ArrowRight) => field.right(),
                    Key::Named(NamedKey::Home) => field.home(),
                    Key::Named(NamedKey::End) => field.end(),
                    Key::Named(NamedKey::Space) => field.insert(' '),
                    Key::Character(text) => {
                        for c in text.chars().filter(|c| !c.is_control()) {
                            field.insert(c);
                        }
                    }
                    _ => {}
                }
                self.redraw();
            }
            Key::Named(NamedKey::Space) => self.active().0.hold_space(pressed),
            Key::Named(NamedKey::Escape) if pressed => {
                let (editor, doc) = self.active();
                if editor.escape(doc) {
                    self.redraw();
                }
            }
            Key::Named(NamedKey::Delete | NamedKey::Backspace) if pressed => {
                let (editor, doc) = self.active();
                let change = editor.delete_selection(doc);
                self.apply(change);
            }
            Key::Character(text) if pressed && self.modifiers.state().control_key() => {
                // Shift turns the character upper case, so the letter is
                // read case-insensitively and the modifier separately.
                let shift = self.modifiers.state().shift_key();
                match text.to_ascii_lowercase().as_str() {
                    "v" => self.paste(),
                    "z" if shift => self.redo(),
                    "z" => self.undo(),
                    "s" if shift => self.ask_name(self.active, Then::Stay),
                    "s" => self.save_active(),
                    "o" => self.ask_open(),
                    "e" => self.ask_send(),
                    "w" => self.request_close(self.active),
                    _ => {}
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
        if shift && c.eq_ignore_ascii_case(&'l') {
            self.layers_shown = !self.layers_shown;
            self.redraw();
        } else if shift && c.eq_ignore_ascii_case(&'b') {
            self.palette_shown = !self.palette_shown;
            self.redraw();
        } else if let Some(tool) = Tool::from_hotkey(c) {
            let (editor, doc) = self.active();
            editor.set_tool(tool, doc);
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
        // A layer the pointer was carrying stays where the window last
        // saw it: the reorder was applied as it went, so there is
        // nothing half-done to put back. The card still has to settle.
        let carrying = match self.carry.as_mut().filter(|c| c.held) {
            Some(carry) => {
                carry.held = false;
                true
            }
            None => false,
        };
        let (editor, doc) = self.active();
        editor.hold_space(false);
        editor.hold_ctrl(false);
        editor.hold_shift(false);
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
        let (over_chrome, handle, tool) = match (self.view(), self.cursor) {
            (Some(view), Some((x, y))) => (
                self.over_chrome(&view, (x, y)),
                self.editor().hover(self.doc(), &view, (x, y)),
                self.editor().pointer_tool(self.doc(), &view, (x, y)),
            ),
            _ => (false, None, self.editor().active_tool()),
        };
        // A layer card and the canvas are both held in a closed hand.
        let held = self.carry.as_ref().is_some_and(|c| c.held);
        let icon = if held || self.editor().is_panning() {
            CursorIcon::Grabbing
        } else if self.editor().is_drawing() {
            CursorIcon::Crosshair
        } else if self.editor().is_moving() {
            CursorIcon::Move
        } else if over_chrome {
            CursorIcon::Default
        } else {
            match (tool, handle) {
                (Tool::Select, Some(Handle::Resize(Corner::TopLeft | Corner::BottomRight))) => {
                    CursorIcon::NwseResize
                }
                (Tool::Select, Some(Handle::Resize(_))) => CursorIcon::NeswResize,
                (Tool::Select, Some(Handle::Rotate(_))) => CursorIcon::Crosshair,
                (Tool::Select, None) => CursorIcon::Default,
                (Tool::Hand, _) => CursorIcon::Grab,
                (Tool::Pencil | Tool::Brush | Tool::Frame, _) => CursorIcon::Crosshair,
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
        let attrs = Window::default_attributes()
            .with_title(format!("Omawhite — {}", self.project().label()));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => return self.fail(event_loop, anyhow::anyhow!("creating window: {e}")),
        };
        match Gfx::new(window.clone()) {
            Ok(gfx) => {
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
        let Some(due) = self.owed else { return };
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
                        sending.target = sending.target.min(found.len() - 1);
                        sending.agents = found;
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
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                self.key(&event.logical_key, &event.key_without_modifiers(), event.state);
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
                let Some(view) = self.view() else { return };
                let frame = self.frame(&view);
                let Some(gfx) = &mut self.gfx else { return };
                match gfx.render(self.theme.bg, &frame) {
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
            UserEvent::Gesture(g) => return self.gestured(g),
            UserEvent::Pen(p) => return self.pen(p),
            UserEvent::Pasted { bytes, bitmap } => return self.pasted(bytes, bitmap),
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
        }
    }
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

/// The pages `docs/boards/` in `cwd` already holds. A directory that is
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
        shapes: Shapes::default(),
        shown_brush: None,
        carry: None,
        renaming: None,
        last_card: None,
        sending: None,
        slides: layers::Slides::default(),
        scroll: 0.0,
        scrolling: layers::Coming::default(),
        focused: None,
        standing: None,
        clock: Instant::now(),
        atlas: None,
        atlas_slot: 0,
        clipboard: None,
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
