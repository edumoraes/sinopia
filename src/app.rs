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
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::doc::Document;
use crate::dock::{Dock, Hit};
use crate::editor::{Editor, FIT_TOLERANCE_PX, PEN_WIDTH, Tool};
use crate::gfx::Gfx;
use crate::grid;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::scene::{self, Prim, View, Viewport};
use crate::store::Store;
use crate::theme::Theme;

/// State the server thread reads (replies to `ping`).
struct SharedState {
    board_id: String,
}

#[derive(Debug)]
enum UserEvent {
    Request(Request),
}

struct App {
    store: Store,
    doc: Document,
    shared: Arc<Mutex<SharedState>>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    theme: Theme,
    editor: Editor,
    /// Last pointer position in physical px, while inside the window.
    cursor: Option<(f64, f64)>,
    modifiers: Modifiers,
    cursor_icon: CursorIcon,
    /// Smoke-test mode: exit cleanly after N presented frames.
    smoke_frames_left: Option<u32>,
    exit_error: Option<anyhow::Error>,
}

impl App {
    fn save(&self) {
        if let Err(e) = self.store.save(&self.doc) {
            log::error!("saving board {}: {e:#}", self.doc.id);
        }
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn switch_to(&mut self, doc: Document) {
        self.editor.cancel();
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
    /// progress, the dock.
    fn frame(&self, view: &View) -> Vec<Prim> {
        let mut prims = grid::prims(view, self.theme.dot);
        prims.extend(scene::document_prims(&self.doc, view));
        if let Some(points) = self.editor.stroke() {
            prims.extend(scene::stroke_prims(points, PEN_WIDTH, self.theme.ink, view));
        }
        prims.extend(self.dock(view).prims(self.editor.tool(), &self.theme));
        prims
    }

    fn pointer_pressed(&mut self) {
        let (Some(view), Some((x, y))) = (self.view(), self.cursor) else {
            return;
        };
        match self.dock(&view).hit(x, y) {
            Some(Hit::Tool(tool)) => {
                self.editor.set_tool(tool);
                self.redraw();
            }
            Some(Hit::Panel) => {}
            None => {
                let (wx, wy) = view.screen_to_world(x, y);
                if self.editor.pointer_down([wx, wy]) {
                    self.redraw();
                }
            }
        }
        self.update_cursor_icon();
    }

    fn pointer_released(&mut self) {
        let tolerance = self
            .view()
            .map_or(FIT_TOLERANCE_PX, |v| FIT_TOLERANCE_PX / v.px_per_world());
        if self
            .editor
            .pointer_up(&mut self.doc, &self.theme.ink_hex, tolerance)
        {
            self.save();
            self.redraw();
        }
        self.update_cursor_icon();
    }

    fn pointer_moved(&mut self, x: f64, y: f64) {
        self.cursor = Some((x, y));
        if self.editor.is_drawing()
            && let Some(view) = self.view()
        {
            let (wx, wy) = view.screen_to_world(x, y);
            // Anything finer than one physical pixel is jitter.
            let min_step = 1.0 / view.px_per_world();
            if self.editor.pointer_move([wx, wy], min_step) {
                self.redraw();
            }
        }
        self.update_cursor_icon();
    }

    fn key_pressed(&mut self, key: &Key) {
        match key {
            Key::Named(NamedKey::Escape) => {
                if self.editor.cancel() {
                    self.redraw();
                }
            }
            Key::Character(text) => {
                let mods = self.modifiers.state();
                if mods.control_key() || mods.alt_key() || mods.super_key() {
                    return;
                }
                let mut chars = text.chars();
                if let (Some(c), None) = (chars.next(), chars.next())
                    && let Some(tool) = Tool::from_hotkey(c)
                {
                    self.editor.set_tool(tool);
                    self.redraw();
                }
            }
            _ => {}
        }
        self.update_cursor_icon();
    }

    /// Crosshair over the canvas while the pencil is active; arrow elsewhere.
    fn update_cursor_icon(&mut self) {
        let over_dock = match (self.view(), self.cursor) {
            (Some(view), Some((x, y))) => self.dock(&view).hit(x, y).is_some(),
            _ => false,
        };
        let pencil_on_canvas = self.editor.tool() == Tool::Pencil && !over_dock;
        let icon = if self.editor.is_drawing() || pencil_on_canvas {
            CursorIcon::Crosshair
        } else {
            CursorIcon::Default
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
                self.update_cursor_icon();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.pointer_pressed(),
                ElementState::Released => self.pointer_released(),
            },
            WindowEvent::ModifiersChanged(m) => self.modifiers = m,
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => self.key_pressed(&logical_key),
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
        let UserEvent::Request(req) = ev;
        match req {
            Request::Raise => {
                if let Some(w) = &self.window {
                    w.focus_window();
                }
            }
            Request::New => {
                let doc = Document::new("sem título");
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
        window: None,
        gfx: None,
        theme: Theme::light(),
        editor: Editor::new(),
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
