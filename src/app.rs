//! Ciclo de vida da janela (ARCHITECTURE.md §11): winit + wgpu + socket.
//!
//! O servidor IPC roda em thread própria e injeta requests no event loop via
//! `EventLoopProxy`. Respostas do handler são acks imediatos (o estado que
//! muda de verdade muda aqui, na thread do loop).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::doc::Document;
use crate::gfx::Gfx;
use crate::ipc::proto::{Event, Request};
use crate::ipc::server::Server;
use crate::scene::{self, Viewport};
use crate::store::Store;

/// Estado visível para a thread do servidor (respostas de `ping`).
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
    /// Modo smoke test: sai limpo depois de N frames apresentados.
    smoke_frames_left: Option<u32>,
    exit_error: Option<anyhow::Error>,
}

impl App {
    fn save(&self) {
        if let Err(e) = self.store.save(&self.doc) {
            log::error!("falha salvando board {}: {e:#}", self.doc.id);
        }
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn switch_to(&mut self, doc: Document) {
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
            Err(e) => return self.fail(event_loop, anyhow::anyhow!("criando janela: {e}")),
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
            WindowEvent::RedrawRequested => {
                let Some(gfx) = &mut self.gfx else { return };
                let (w, h) = gfx.size();
                let instances = scene::rect_instances(&self.doc, Viewport { w, h });
                match gfx.render(&instances) {
                    Ok(presented) => {
                        if let Some(n) = &mut self.smoke_frames_left {
                            if presented {
                                *n = n.saturating_sub(1);
                            }
                            if *n == 0 {
                                log::info!("smoke test ok: frames apresentados");
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
                    log::error!("criando board novo: {e:#}");
                    return;
                }
                self.switch_to(doc);
            }
            Request::Open { id } => match self.store.load(&id) {
                Ok(doc) => {
                    self.save(); // não perder o board atual
                    self.switch_to(doc);
                }
                Err(e) => log::error!("abrindo board {id:?}: {e:#}"),
            },
            Request::Shutdown => {
                self.save();
                event_loop.exit();
            }
            Request::Theme { colors } => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.background = scene::parse_color(&colors.bg);
                }
                self.redraw();
            }
            // O servidor responde `denied` sem encaminhar; nunca chega aqui.
            Request::Export { .. } | Request::Ping => {}
        }
    }
}

/// Sobe servidor + janela e roda até o usuário fechar (ou o smoke acabar).
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
                reason: "export ainda não implementado".into(),
            },
            // Ack genérico: id corrente + pid (§5). `new`/`open` trocam de
            // board de forma assíncrona no event loop; resposta síncrona
            // com o id novo entra quando o plugin precisar dela (§15.3).
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
            log::warn!("event loop encerrado; request ignorado");
        }
        reply
    });

    let mut app = App {
        store,
        doc,
        shared,
        window: None,
        gfx: None,
        smoke_frames_left: smoke_frames,
        exit_error: None,
    };
    event_loop.run_app(&mut app)?;

    // Janela fechou: flush final é feito nos caminhos de saída; o socket
    // morre com o processo (arquivo fica; o próximo bind detecta e substitui).
    match app.exit_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
