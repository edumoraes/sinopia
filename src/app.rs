//! Window lifecycle (ARCHITECTURE.md §11): winit + wgpu + socket.
//!
//! The IPC server runs on its own thread and injects requests into the
//! event loop through `EventLoopProxy`; its replies are immediate acks (the
//! state that actually changes, changes here, on the loop thread). Input is
//! routed to the pure `editor` and `dock`; this file only maps events and
//! assembles frames, so it stays thin and the logic stays testable.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::bitmap::{self, Bitmap};
use crate::clipboard::{self, Clipboard, Paste};
use crate::doc::{Document, Element};
use crate::dock::{Dock, Hit};
use crate::editor::{Button, Change, Editor, Gesture, PEN_WIDTH, SCROLL_LINE_PX, Tool};
use crate::geom::Corner;
use crate::gestures;
use crate::gfx::Gfx;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::project::{Origin, Project};
use crate::scene::{self, ImageSlots, Prim, View, Viewport};
use crate::select::{self, Handle};
use crate::store::{self, Store};
use crate::tabs::{TabHit, Tabs};
use crate::text::{Atlas, Font};
use crate::theme::Theme;

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
    /// Built once the scale factor is known, rebuilt when it changes.
    atlas: Option<Atlas>,
    atlas_slot: u32,
    clipboard: Option<Clipboard>,
    /// Last pointer position in physical px, while inside the window.
    cursor: Option<(f64, f64)>,
    modifiers: Modifiers,
    cursor_icon: CursorIcon,
    /// Smoke-test mode: exit cleanly after N presented frames.
    smoke_frames_left: Option<u32>,
    exit_error: Option<anyhow::Error>,
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
        let Some(project) = self.open.get(index).map(|o| &o.project) else {
            return false;
        };
        let saved = match &project.origin {
            Origin::Board(id) => {
                let origin = Origin::Board(id.clone());
                self.store.save(&project.doc).map(|()| origin)
            }
            Origin::File(path) => {
                let origin = Origin::File(path.clone());
                store::save_document_to(path, &project.doc).map(|()| origin)
            }
            Origin::Untitled => return false,
        };
        match saved {
            Ok(origin) => {
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

    /// The document changed. Nothing reaches disk until the user asks.
    fn touch(&mut self) {
        let was_clean = !self.open[self.active].project.dirty;
        self.open[self.active].project.touch();
        if was_clean {
            self.retitle();
        }
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

    /// Closes tab `index` if nothing would be lost. A dirty tab needs an
    /// answer first, which the confirmation dialog will bring.
    fn request_close(&mut self, index: usize) {
        let Some(open) = self.open.get(index) else {
            return;
        };
        if open.project.dirty {
            log::info!("{} has unsaved changes", open.project.label());
            return;
        }
        self.close(index);
    }

    /// Drops tab `index`, whatever state it is in. The caller has already
    /// settled what happens to unsaved work.
    fn close(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        self.open.remove(index);
        if self.open.is_empty() {
            return;
        }
        // Closing a tab before the active one would otherwise shift the
        // one in front out from under the user.
        let next = if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.open.len() - 1)
        };
        self.activate(next);
    }

    /// Whether every tab is gone — the window has nothing left to show.
    fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Which tab, if any, already shows the board `id` names. Opening the
    /// same board twice would give it two documents and one file.
    fn tab_showing(&self, id: &str) -> Option<usize> {
        self.open.iter().position(|o| o.project.doc.id == id)
    }

    /// Closes the window. Everything with a home on disk is written
    /// first; an untitled board has nowhere to go.
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        for i in 0..self.open.len() {
            if self.open[i].project.dirty {
                if self.open[i].project.needs_a_name() {
                    log::warn!("{} was never saved", self.open[i].project.label());
                } else {
                    self.save_project(i);
                }
            }
        }
        event_loop.exit();
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
    /// progress, the selection frame and marquee, the dock.
    fn frame(&self, view: &View) -> Vec<Prim> {
        // Before the window exists there are no textures, so every image
        // is a placeholder — which is what an empty map says.
        let none = ImageSlots::new();
        let images = self.gfx.as_ref().map_or(&none, Gfx::image_slots);
        let mut prims = grid::prims(view, self.theme.dot);
        prims.extend(scene::document_prims(self.doc(), view, images));
        if let Some(points) = self.editor().stroke() {
            prims.extend(scene::stroke_prims(points, PEN_WIDTH, self.theme.ink, view));
        }
        if let Some(frame) = self.editor().selection_frame(self.doc()) {
            prims.extend(select::prims(&frame, view, &self.theme));
        }
        if let Some((a, b)) = self.editor().marquee() {
            prims.extend(select::marquee_prims(a, b, &self.theme));
        }
        prims.extend(self.dock(view).prims(self.editor().tool(), &self.theme));
        if let (Some(tabs), Some(atlas)) = (self.tabs(view), self.atlas.as_ref()) {
            prims.extend(tabs.prims(atlas, self.atlas_slot, &self.theme));
        }
        prims
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
        // The strip is over the dock is over the canvas.
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
                let (editor, doc) = self.active();
                let change = editor.press(button, &view, (x, y), doc);
                self.apply(change);
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_released(&mut self, button: Button) {
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
        if let Some(view) = self.view() {
            let (editor, doc) = self.active();
            let change = editor.moved(&view, (x, y), doc);
            self.apply(change);
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
                if text.eq_ignore_ascii_case("v") {
                    self.paste();
                }
            }
            Key::Character(text) if pressed => {
                let mods = self.modifiers.state();
                if mods.control_key() || mods.alt_key() || mods.super_key() {
                    return;
                }
                let mut chars = text.chars();
                if let (Some(c), None) = (chars.next(), chars.next())
                    && let Some(tool) = Tool::from_hotkey(c)
                {
                    let (editor, doc) = self.active();
                    editor.set_tool(tool, doc);
                    self.redraw();
                }
            }
            _ => {}
        }
        self.update_cursor_icon();
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
        let (editor, doc) = self.active();
        editor.hold_space(false);
        editor.hold_ctrl(false);
        editor.hold_shift(false);
        if editor.cancel(doc) {
            self.redraw();
        }
        self.update_cursor_icon();
    }

    /// Cursor for the tool the pointer would use over the canvas; arrow
    /// over the dock; resize and rotate cursors over the selection
    /// handles.
    fn update_cursor_icon(&mut self) {
        let (over_chrome, handle, tool) = match (self.view(), self.cursor) {
            (Some(view), Some((x, y))) => (
                self.dock(&view).hit(x, y).is_some()
                    || self.tabs(&view).and_then(|t| t.hit(x, y)).is_some(),
                self.editor().hover(self.doc(), &view, (x, y)),
                self.editor().pointer_tool(self.doc(), &view, (x, y)),
            ),
            _ => (false, None, self.editor().active_tool()),
        };
        let icon = if self.editor().is_panning() {
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
                (Tool::Pencil, _) => CursorIcon::Crosshair,
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
        // The last tab closing leaves nothing to show.
        if self.is_empty() {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, ev: UserEvent) {
        self.handle_user_event(event_loop, ev);
        if self.is_empty() {
            event_loop.exit();
        }
    }
}

impl App {
    fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(event_loop),
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
                let Some(view) = self.view() else { return };
                let prims = self.frame(&view);
                let Some(gfx) = &mut self.gfx else { return };
                match gfx.render(self.theme.bg, &prims) {
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

    fn handle_user_event(&mut self, event_loop: &ActiveEventLoop, ev: UserEvent) {
        let req = match ev {
            UserEvent::Request(req) => req,
            UserEvent::Gesture(g) => return self.gestured(g),
            UserEvent::Pasted { bytes, bitmap } => return self.pasted(bytes, bitmap),
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
                if let Some(i) = self.tab_showing(&id) {
                    return self.activate(i);
                }
                match self.store.load(&id) {
                    Ok(doc) => {
                        self.open_project(Project::opened(doc, Origin::Board(id)));
                    }
                    Err(e) => log::error!("opening board {id:?}: {e:#}"),
                }
            }
            Request::Shutdown => self.quit(event_loop),
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
        atlas: None,
        atlas_slot: 0,
        clipboard: None,
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
