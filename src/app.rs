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

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::bitmap::{self, Bitmap};
use crate::brush::{self, Brush};
use crate::clipboard::{self, Clipboard, Paste};
use crate::dialogs::{self, Answer, Reply};
use crate::doc::{Document, Element};
use crate::dock::{Dock, Hit};
use crate::editor::{Button, Change, Editor, Gesture, SCROLL_LINE_PX, Tool};
use crate::geom::Corner;
use crate::gestures;
use crate::gfx::Gfx;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::layers::{self, Panel, PanelHit};
use crate::project::{self, Origin, Project};
use crate::scene::{self, Frame, ImageSlots, View, Viewport, with_alpha};
use crate::select::{self, Handle};
use crate::store::{self, Store};
use crate::tabs::{self, TabHit, Tabs};
use crate::text::{Atlas, Font};
use crate::theme::Theme;

/// The longest step the panel's easing takes in one frame. A window
/// that has been idle wakes with a huge gap since the last frame;
/// without this, whatever just started would be over before it drew.
const MAX_STEP: f32 = 0.05;

/// How much of the ink the brush's ring is drawn with.
const RING_ALPHA: f32 = 0.6;

/// State the server thread reads (replies to `ping`).
struct SharedState {
    board_id: String,
}

#[derive(Debug)]
enum UserEvent {
    Request(Request),
    Gesture(Gesture),
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
    font: Font,
    /// What `B` paints with. One brush for the window, whichever tab is
    /// in front, as in Photoshop.
    brush: Brush,
    /// `Shift+L`, or the handle beside it: the layers panel is up.
    layers_shown: bool,
    /// The layer card the pointer picked up, if any. It outlives the
    /// release, easing back into the stack.
    carry: Option<Carry>,
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
    /// Where a portal dialog sends its answer. Absent before the window.
    dialog_sink: Option<dialogs::Sink>,
    pending: Option<Pending>,
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
            Origin::Board(_) => self.store.save(&project.doc),
            Origin::File(path) => store::save_document_to(path, &project.doc),
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

    /// The document changed. Nothing reaches disk until the user asks.
    fn touch(&mut self) {
        let was_clean = !self.open[self.active].project.dirty;
        self.open[self.active].project.touch();
        if was_clean {
            self.retitle();
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
        let Open { project, .. } = &self.open[self.active];
        self.slides.restack(&project.doc.layers, row);
        self.scrolling.tick(dt);
        // The window may have grown or shrunk under it: the panel is the
        // authority on how far the stack can be scrolled.
        if !self.scrolling.moving()
            && let Some(view) = self.view()
            && let Some(panel) = self.panel(&view)
        {
            self.scroll = panel.scroll();
        }
        self.follow_active();
    }

    /// Brings the active layer's card into the band when it has just
    /// become active — a click on the canvas picks an element, and the
    /// panel goes to where that element lives. Only on the change: the
    /// list stays where the wheel left it otherwise.
    fn follow_active(&mut self) {
        let Some(view) = self.view() else { return };
        let index = self.editor().active_layer(self.doc());
        let id = self.doc().layers.get(index).map(|l| l.id.clone());
        if self.focused == id {
            return;
        }
        self.focused = id;
        // A card in the hand takes the panel where the pointer says.
        let Some(panel) = self.panel(&view).filter(|_| self.carry.is_none()) else {
            return;
        };
        let want = panel.scroll_showing(index, self.doc().layers.len());
        if want != self.scroll {
            self.scrolling.send(self.scroll - want);
            self.scroll = want;
            self.redraw();
        }
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

    /// Closes tab `index`, asking about unsaved work first.
    fn request_close(&mut self, index: usize) {
        match self.open.get(index) {
            Some(open) if open.project.dirty => self.ask_about(index, Then::Close),
            Some(_) => self.close(index),
            None => {}
        }
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
        match self.open.iter().position(|o| o.project.dirty) {
            Some(i) => self.ask_about(i, Then::Quit),
            None => self.closing = true,
        }
    }

    /// Adds a tab and makes it the one in front.
    fn open_project(&mut self, project: Project) {
        self.open.push(Open {
            project,
            editor: Editor::new(),
        });
        self.activate(self.open.len() - 1);
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

    fn dock(&self, view: &View) -> Dock {
        Dock::layout(view.viewport, view.scale, &Tool::ALL)
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
            view.scale,
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
        let top = (tabs::HEIGHT * view.scale as f32).round();
        Some(Panel::layout(
            view.viewport,
            view.scale,
            top,
            atlas,
            &self.doc().layers,
            self.scroll + self.scrolling.offset(),
        ))
    }

    /// The panel's handle, once there is an atlas to letter it with. It
    /// is on screen whether the panel is up or not — closed, it is the
    /// only thing that says the panel is there.
    fn handle(&self, view: &View) -> Option<layers::Handle> {
        let atlas = self.atlas.as_ref()?;
        let top = (tabs::HEIGHT * view.scale as f32).round();
        Some(layers::Handle::layout(
            view.viewport,
            view.scale,
            top,
            atlas,
            self.layers_shown,
        ))
    }

    /// Whether `screen` is over the strip, the handle, the panel or the
    /// dock rather than the canvas.
    fn over_chrome(&self, view: &View, screen: (f64, f64)) -> bool {
        let (x, y) = screen;
        self.tabs(view).and_then(|t| t.hit(x, y)).is_some()
            || self.handle(view).is_some_and(|h| h.hit(x, y))
            || self.panel(view).and_then(|p| p.hit(x, y)).is_some()
            || self.dock(view).hit(x, y).is_some()
    }

    /// A click on the layers panel, handed to the editor.
    fn panel_hit(&mut self, hit: PanelHit) {
        let (editor, doc) = self.active();
        let change = match hit {
            PanelHit::Select(i) => editor.select_layer(doc, i),
            PanelHit::Toggle(i) => editor.toggle_layer(doc, i),
            PanelHit::Add => editor.add_layer(doc),
            PanelHit::Remove => editor.remove_layer(doc),
            PanelHit::Up => editor.move_layer(doc, true),
            PanelHit::Down => editor.move_layer(doc, false),
            PanelHit::Panel => Change::None,
        };
        self.apply(change);
    }

    /// A key that adjusts the brush, while the brush tool is selected:
    /// `[` `]` size, `{` `}` hardness, a digit the opacity. True if it
    /// was one.
    fn brush_key(&mut self, c: char) -> bool {
        if self.editor().tool() != Tool::Brush {
            return false;
        }
        match c {
            '[' => self.brush.shrink(),
            ']' => self.brush.grow(),
            '{' => self.brush.softer(),
            '}' => self.brush.harder(),
            '0'..='9' => self.brush.set_opacity_digit(c as u8 - b'0'),
            _ => return false,
        }
        true
    }

    /// Builds and uploads the glyph atlas for the current scale factor,
    /// unless the one in hand already matches.
    fn ensure_atlas(&mut self) {
        let Some(window) = &self.window else { return };
        let px = Tabs::label_px(window.scale_factor());
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
        frame.append(scene::document_prims(self.doc(), view, images));
        if let Some(stroke) = self.editor().stroke() {
            let prims = scene::stroke_prims(&stroke.points, stroke.tip, self.theme.ink, view);
            frame.stroke(prims, stroke.tip);
        }
        if let Some(selection) = self.editor().selection_frame(self.doc()) {
            frame.extend(select::prims(&selection, view, &self.theme));
        }
        if let Some((a, b)) = self.editor().marquee() {
            frame.extend(select::marquee_prims(a, b, &self.theme));
        }
        // The brush shows its size before it paints: a ring at the
        // pointer, wherever the next press would paint.
        if let Some((x, y)) = self.cursor
            && !self.over_chrome(view, (x, y))
            && self.editor().pointer_tool(self.doc(), view, (x, y)) == Tool::Brush
        {
            let radius = (self.brush.size / 2.0 * view.px_per_world()) as f32;
            let ink = with_alpha(self.theme.ink, RING_ALPHA);
            frame.extend(brush::ring_prims(
                (x as f32, y as f32),
                radius,
                view.scale as f32,
                ink,
            ));
        }
        frame.extend(self.dock(view).prims(self.editor().tool(), &self.theme));
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
                &self.doc().layers,
                &showing,
                atlas,
                self.atlas_slot,
                &self.theme,
            ));
        }
        if let (Some(tabs), Some(atlas)) = (self.tabs(view), self.atlas.as_ref()) {
            frame.extend(tabs.prims(atlas, self.atlas_slot, &self.theme));
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
            Change::Selection => self.redraw(),
            Change::Scene => {
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
                self.panel_hit(hit);
                // A card taken by its name is picked up by the grip the
                // press made, and follows the pointer from there.
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
        match self.dock(&view).hit(x, y) {
            Some(Hit::Tool(tool)) => {
                if button == Button::Left {
                    let (editor, doc) = self.active();
                    editor.set_tool(tool, doc);
                    self.redraw();
                }
            }
            Some(Hit::Panel) => {}
            None => {
                let brush = self.brush;
                let (editor, doc) = self.active();
                let change = editor.press(button, &view, (x, y), doc, &brush);
                self.apply(change);
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_released(&mut self, button: Button) {
        // A carried layer is left where the pointer put it; the canvas
        // never saw the press, so it has nothing to end. The card runs
        // the lift backwards into its row from here.
        if button == Button::Left
            && let Some(carry) = self.carry.as_mut().filter(|c| c.held)
        {
            carry.held = false;
            self.redraw();
            return self.update_cursor_icon();
        }
        let Some(view) = self.view() else { return };
        let (x, y) = self.cursor.unwrap_or_default();
        let ink = self.theme.ink_hex.clone();
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
        let shift = self.modifiers.state().shift_key();
        let camera = self.active().0.scroll(&view, cursor, delta, shift);
        self.apply(Change::Camera(camera));
    }

    fn key(&mut self, key: &Key, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        match key {
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
                    "s" if shift => self.ask_name(self.active, Then::Stay),
                    "s" => self.save_active(),
                    "o" => self.ask_open(),
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
                    self.plain_key(c, mods.shift_key());
                }
            }
            _ => {}
        }
        self.update_cursor_icon();
    }

    /// A character typed with no modifier but Shift: `Shift+L` shows or
    /// hides the layers, a tool's letter selects it, and the brush's
    /// keys adjust it while it is selected.
    fn plain_key(&mut self, c: char, shift: bool) {
        if shift && c.eq_ignore_ascii_case(&'l') {
            self.layers_shown = !self.layers_shown;
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
        if editor.cancel(doc) || carrying {
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
                (Tool::Pencil | Tool::Brush, _) => CursorIcon::Crosshair,
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
                self.load_images();
                self.redraw();
            }
            Err(e) => self.fail(event_loop, e),
        }
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
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
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
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Middle => Button::Middle,
                    MouseButton::Right => Button::Right,
                    _ => return,
                };
                match state {
                    ElementState::Pressed => self.pointer_pressed(button),
                    ElementState::Released => self.pointer_released(button),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.scrolled(delta),
            WindowEvent::ModifiersChanged(m) => self.modifiers_changed(m),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => self.key(&logical_key, state),
            WindowEvent::RedrawRequested => {
                self.tick();
                if self.animating() {
                    self.redraw();
                }
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
            UserEvent::Pasted { bytes, bitmap } => return self.pasted(bytes, bitmap),
            UserEvent::Dialog(reply) => return self.dialog_replied(reply),
        };
        match req {
            Request::Raise => {
                if let Some(w) = &self.window {
                    w.focus_window();
                }
            }
            Request::New => {
                let doc = Document::new("untitled");
                // Written before the window sees it: index.json is the
                // only file the plugin reads (§5), so a board made for
                // the gallery has to be in it.
                if let Err(e) = self.store.save(&doc) {
                    log::error!("creating a new board: {e:#}");
                    return;
                }
                let origin = Origin::Board(doc.id.clone());
                self.open_project(Project::opened(doc, origin));
            }
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
            Request::Shutdown => self.quit(),
            Request::Theme { colors } => {
                self.theme = Theme::from_hex(&colors.bg, &colors.fg, &colors.accent);
                self.redraw();
            }
            // The server answers `denied` without forwarding; never reaches here.
            Request::Export { .. } | Request::Ping => {}
        }
    }
}

/// Brings up server + window and runs until the user closes it (or the
/// smoke test is done).
pub fn run(
    store: Store,
    doc: Document,
    socket_path: PathBuf,
    smoke_frames: Option<u32>,
) -> anyhow::Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let shared = Arc::new(Mutex::new(SharedState {
        board_id: doc.id.clone(),
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

    // Whatever `main` resolved — `--new`, `--open <id>`, or the most
    // recent — came out of the store and was written there first, so the
    // first tab is a board, never an untitled one.
    let id = doc.id.clone();
    let first = Project::opened(doc, Origin::Board(id));
    let mut app = App {
        store,
        open: vec![Open {
            project: first,
            editor: Editor::new(),
        }],
        active: 0,
        shared,
        proxy: event_loop.create_proxy(),
        window: None,
        gfx: None,
        theme: Theme::light(),
        font: Font::bundled(),
        brush: Brush::default(),
        layers_shown: false,
        carry: None,
        slides: layers::Slides::default(),
        scroll: 0.0,
        scrolling: layers::Coming::default(),
        focused: None,
        clock: Instant::now(),
        atlas: None,
        atlas_slot: 0,
        clipboard: None,
        dialog_sink: None,
        pending: None,
        quitting: false,
        closing: false,
        cursor: None,
        modifiers: Modifiers::default(),
        cursor_icon: CursorIcon::Default,
        smoke_frames_left: smoke_frames,
        exit_error: None,
    };
    event_loop.run_app(&mut app)?;

    // Window closed: the final flush happens on the exit paths; the socket
    // dies with the process (the file stays; the next bind detects and
    // replaces it).
    match app.exit_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
