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
use winit::event::{
    ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::doc::Document;
use crate::dock::{Dock, Hit};
use crate::editor::{Button, Change, Editor, Gesture, PEN_WIDTH, SCROLL_LINE_PX, Tool};
use crate::geom::Corner;
use crate::gestures;
use crate::gfx::Gfx;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::scene::{self, Prim, View, Viewport};
use crate::select::{self, Handle};
use crate::store::Store;
use crate::theme::Theme;

/// State the server thread reads (replies to `ping`).
struct SharedState {
    board_id: String,
}

#[derive(Debug)]
enum UserEvent {
    Request(Request),
    Gesture(Gesture),
}

struct App {
    store: Store,
    doc: Document,
    shared: Arc<Mutex<SharedState>>,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    theme: Theme,
    editor: Editor,
    /// Last pointer position in physical px, while inside the window.
    cursor: Option<(f64, f64)>,
    modifiers: Modifiers,
    cursor_icon: CursorIcon,
    /// The camera moved since the last save (pans and zooms are saved when
    /// the gesture ends, not per frame).
    camera_dirty: bool,
    /// Smoke-test mode: exit cleanly after N presented frames.
    smoke_frames_left: Option<u32>,
    exit_error: Option<anyhow::Error>,
}

impl App {
    fn save(&mut self) {
        if let Err(e) = self.store.save(&self.doc) {
            log::error!("saving board {}: {e:#}", self.doc.id);
        }
        self.camera_dirty = false;
    }

    fn flush_camera(&mut self) {
        if self.camera_dirty {
            self.save();
        }
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn switch_to(&mut self, doc: Document) {
        self.flush_camera();
        // Drop the drag, then the selection: neither belongs to the new board.
        while self.editor.escape(&mut self.doc) {}
        self.doc = doc;
        self.shared.lock().expect("lock shared").board_id = self.doc.id.clone();
        if let Some(w) = &self.window {
            w.set_title(&format!("Omawhite — {}", self.doc.title));
        }
        self.redraw();
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
            camera: self.doc.camera,
            viewport: Viewport { w, h },
            scale,
        })
    }

    fn dock(&self, view: &View) -> Dock {
        Dock::layout(view.viewport, view.scale, &Tool::ALL)
    }

    /// Everything on screen, back to front: grid, document, the stroke in
    /// progress, the selection frame and marquee, the dock.
    fn frame(&self, view: &View) -> Vec<Prim> {
        let mut prims = grid::prims(view, self.theme.dot);
        prims.extend(scene::document_prims(&self.doc, view));
        if let Some(points) = self.editor.stroke() {
            prims.extend(scene::stroke_prims(points, PEN_WIDTH, self.theme.ink, view));
        }
        if let Some(frame) = self.editor.selection_frame(&self.doc) {
            prims.extend(select::prims(&frame, view, &self.theme));
        }
        if let Some((a, b)) = self.editor.marquee() {
            prims.extend(select::marquee_prims(a, b, &self.theme));
        }
        prims.extend(self.dock(view).prims(self.editor.tool(), &self.theme));
        prims
    }

    /// Stores what an editor input changed and redraws if anything did.
    fn apply(&mut self, change: Change) {
        match change {
            Change::None => {}
            Change::Scene | Change::Selection => self.redraw(),
            Change::Camera(camera) => {
                self.doc.camera = camera;
                self.camera_dirty = true;
                self.redraw();
            }
        }
    }

    fn pointer_pressed(&mut self, button: Button) {
        let (Some(view), Some((x, y))) = (self.view(), self.cursor) else {
            return;
        };
        self.flush_camera();
        match self.dock(&view).hit(x, y) {
            Some(Hit::Tool(tool)) => {
                if button == Button::Left {
                    self.editor.set_tool(tool, &mut self.doc);
                    self.redraw();
                }
            }
            Some(Hit::Panel) => {}
            None => {
                let change = self.editor.press(button, &view, (x, y), &mut self.doc);
                self.apply(change);
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_released(&mut self, button: Button) {
        let Some(view) = self.view() else { return };
        let (x, y) = self.cursor.unwrap_or_default();
        let ink = self.theme.ink_hex.clone();
        match self
            .editor
            .release(button, &view, (x, y), &mut self.doc, &ink)
        {
            Change::None => self.flush_camera(),
            Change::Scene => {
                self.save();
                self.redraw();
            }
            Change::Selection => self.redraw(),
            change @ Change::Camera(_) => {
                self.apply(change);
                self.flush_camera();
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_moved(&mut self, x: f64, y: f64) {
        self.cursor = Some((x, y));
        if let Some(view) = self.view() {
            let change = self.editor.moved(&view, (x, y), &mut self.doc);
            self.apply(change);
        }
        self.update_cursor_icon();
    }

    fn scrolled(&mut self, delta: MouseScrollDelta, phase: TouchPhase) {
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
        let camera = self.editor.scroll(&view, cursor, delta, shift);
        self.apply(Change::Camera(camera));
        if phase == TouchPhase::Ended {
            self.flush_camera();
        }
    }

    fn key(&mut self, key: &Key, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        match key {
            Key::Named(NamedKey::Space) => {
                self.editor.hold_space(pressed);
                if !pressed {
                    self.flush_camera();
                }
            }
            Key::Named(NamedKey::Escape) if pressed => {
                if self.editor.escape(&mut self.doc) {
                    self.redraw();
                }
            }
            Key::Named(NamedKey::Delete | NamedKey::Backspace) if pressed => {
                if self.editor.delete_selection(&mut self.doc) == Change::Scene {
                    self.save();
                    self.redraw();
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
                    self.editor.set_tool(tool, &mut self.doc);
                    self.redraw();
                }
            }
            _ => {}
        }
        self.update_cursor_icon();
    }

    fn modifiers_changed(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
        let ctrl = modifiers.state().control_key();
        self.editor.hold_ctrl(ctrl);
        self.editor.hold_shift(modifiers.state().shift_key());
        if !ctrl {
            self.flush_camera();
        }
        self.update_cursor_icon();
    }

    fn gestured(&mut self, gesture: Gesture) {
        let Some(view) = self.view() else { return };
        let cursor = self.cursor.unwrap_or((
            f64::from(view.viewport.w) / 2.0,
            f64::from(view.viewport.h) / 2.0,
        ));
        if let Some(camera) = self.editor.gesture(&view, cursor, gesture) {
            self.apply(Change::Camera(camera));
        }
        if gesture == Gesture::End {
            self.flush_camera();
        }
    }

    /// Keys can't be released into a window that lost focus: drop the held
    /// overrides and whatever gesture they were driving.
    fn focus_lost(&mut self) {
        self.editor.hold_space(false);
        self.editor.hold_ctrl(false);
        self.editor.hold_shift(false);
        if self.editor.cancel(&mut self.doc) {
            self.redraw();
        }
        self.flush_camera();
        self.update_cursor_icon();
    }

    /// Cursor for the active tool over the canvas; arrow over the dock;
    /// resize and rotate cursors over the selection handles.
    fn update_cursor_icon(&mut self) {
        let (over_dock, handle) = match (self.view(), self.cursor) {
            (Some(view), Some((x, y))) => (
                self.dock(&view).hit(x, y).is_some(),
                self.editor.hover(&self.doc, &view, (x, y)),
            ),
            _ => (false, None),
        };
        let icon = if self.editor.is_panning() {
            CursorIcon::Grabbing
        } else if self.editor.is_drawing() {
            CursorIcon::Crosshair
        } else if self.editor.is_moving() {
            CursorIcon::Move
        } else if over_dock {
            CursorIcon::Default
        } else {
            match (self.editor.active_tool(), handle) {
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
        let attrs =
            Window::default_attributes().with_title(format!("Omawhite — {}", self.doc.title));
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
                self.window = Some(window);
                self.redraw();
            }
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_moved(position.x, position.y);
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.flush_camera();
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
            WindowEvent::MouseWheel { delta, phase, .. } => self.scrolled(delta, phase),
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, ev: UserEvent) {
        let req = match ev {
            UserEvent::Request(req) => req,
            UserEvent::Gesture(g) => return self.gestured(g),
        };
        match req {
            Request::Raise => {
                if let Some(w) = &self.window {
                    w.focus_window();
                }
            }
            Request::New => {
                let doc = Document::new("untitled");
                if let Err(e) = self.store.save(&doc) {
                    log::error!("creating a new board: {e:#}");
                    return;
                }
                self.switch_to(doc);
            }
            Request::Open { id } => match self.store.load(&id) {
                Ok(doc) => {
                    self.save(); // never lose the current board
                    self.switch_to(doc);
                }
                Err(e) => log::error!("opening board {id:?}: {e:#}"),
            },
            Request::Shutdown => {
                self.save();
                event_loop.exit();
            }
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

    let mut app = App {
        store,
        doc,
        shared,
        proxy: event_loop.create_proxy(),
        window: None,
        gfx: None,
        theme: Theme::light(),
        editor: Editor::new(),
        cursor: None,
        modifiers: Modifiers::default(),
        cursor_icon: CursorIcon::Default,
        camera_dirty: false,
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
