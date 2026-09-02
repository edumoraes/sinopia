//! Trackpad gestures — pinch and multi-finger swipe — over
//! `zwp_pointer_gestures_v1`. winit 0.30 has no gesture events on Wayland,
//! so this joins the window's connection as a guest client: its own
//! `wl_pointer` on the same seat, its own event queue on its own thread,
//! and each step goes to the event loop through the sink — the same bridge
//! shape as the IPC server. Not on Wayland, or no protocol: no gestures,
//! nothing else lost.

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gesture_swipe_v1::{self, ZwpPointerGestureSwipeV1},
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};
use winit::window::Window;

use crate::editor::Gesture;

/// Delivers a gesture step to the event loop; `false` once the loop is gone.
pub type Sink = Box<dyn Fn(Gesture) -> bool + Send>;

/// The display pointer travels to the thread as an address; it is only
/// ever handed back to libwayland.
struct Display(usize);

/// Starts the bridge for `window` on its own thread. Failures are logged,
/// not fatal: the board works without a trackpad.
pub fn spawn(window: &Window, sink: Sink) -> anyhow::Result<()> {
    let RawDisplayHandle::Wayland(handle) = window.display_handle()?.as_raw() else {
        log::info!("not on Wayland: no trackpad gestures");
        return Ok(());
    };
    let display = Display(handle.display.as_ptr() as usize);
    std::thread::Builder::new()
        .name("gestures".into())
        .spawn(move || {
            if let Err(e) = run(display, sink) {
                log::warn!("trackpad gestures unavailable: {e:#}");
            }
        })?;
    Ok(())
}

fn run(display: Display, sink: Sink) -> anyhow::Result<()> {
    // SAFETY: the display belongs to winit's event loop, which outlives every
    // window; this thread stops dispatching as soon as the sink reports the
    // loop gone, and the guest backend never closes the display.
    let backend = unsafe { Backend::from_foreign_display(display.0 as *mut _) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let gestures: ZwpPointerGesturesV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| anyhow::anyhow!("zwp_pointer_gestures_v1: {e}"))?;
    let seat_interface = wl_seat::WlSeat::interface().name;
    let seats: Vec<wl_seat::WlSeat> = globals
        .contents()
        .clone_list()
        .into_iter()
        .filter(|g| g.interface == seat_interface)
        .map(|g| globals.registry().bind(g.name, g.version.min(1), &qh, ()))
        .collect();
    if seats.is_empty() {
        anyhow::bail!("no wl_seat");
    }
    log::debug!(
        "gestures: display {:?}, registry {:?}, {} globals, {} seats",
        conn.display().id(),
        globals.registry().id(),
        globals.contents().clone_list().len(),
        seats.len()
    );
    let mut state = State {
        gestures,
        pointers: Vec::new(),
        pinch_scale: 1.0,
        swipe_fingers: 0,
        sink,
        alive: true,
    };
    log::info!("trackpad gestures: pinch and swipe over zwp_pointer_gestures_v1");
    while state.alive {
        queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}

/// A seat's pointer with its gesture objects.
struct PointerGestures {
    seat: wl_seat::WlSeat,
    pinch: ZwpPointerGesturePinchV1,
    swipe: ZwpPointerGestureSwipeV1,
}

struct State {
    gestures: ZwpPointerGesturesV1,
    pointers: Vec<PointerGestures>,
    /// Cumulative pinch scale at the previous step, so steps are relative.
    pinch_scale: f64,
    swipe_fingers: u32,
    sink: Sink,
    alive: bool,
}

impl State {
    fn emit(&mut self, gesture: Gesture) {
        if !(self.sink)(gesture) {
            self.alive = false;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut State,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut State,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        let has_pointer = caps.contains(wl_seat::Capability::Pointer);
        let bound = state.pointers.iter().position(|p| p.seat == *seat);
        match (has_pointer, bound) {
            (true, None) => {
                log::debug!("gestures: pointer on {:?}", seat.id());
                let pointer = seat.get_pointer(qh, ());
                let pinch = state.gestures.get_pinch_gesture(&pointer, qh, ());
                let swipe = state.gestures.get_swipe_gesture(&pointer, qh, ());
                state.pointers.push(PointerGestures {
                    seat: seat.clone(),
                    pinch,
                    swipe,
                });
            }
            (false, Some(i)) => {
                let p = state.pointers.remove(i);
                p.pinch.destroy();
                p.swipe.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for State {
    fn event(
        state: &mut State,
        _: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwp_pointer_gesture_pinch_v1::Event;
        match event {
            Event::Begin { .. } => state.pinch_scale = 1.0,
            Event::Update { dx, dy, scale, .. } => {
                let factor = if state.pinch_scale > 0.0 {
                    scale / state.pinch_scale
                } else {
                    1.0
                };
                state.pinch_scale = scale;
                state.emit(Gesture::Pinch { dx, dy, factor });
            }
            Event::End { .. } => state.emit(Gesture::End),
            _ => {}
        }
    }
}

impl Dispatch<ZwpPointerGestureSwipeV1, ()> for State {
    fn event(
        state: &mut State,
        _: &ZwpPointerGestureSwipeV1,
        event: zwp_pointer_gesture_swipe_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwp_pointer_gesture_swipe_v1::Event;
        match event {
            Event::Begin { fingers, .. } => state.swipe_fingers = fingers,
            Event::Update { dx, dy, .. } => {
                let fingers = state.swipe_fingers;
                state.emit(Gesture::Swipe { fingers, dx, dy });
            }
            Event::End { .. } => state.emit(Gesture::End),
            _ => {}
        }
    }
}

// Our pointer only exists to hang the gesture objects on; its own events
// are winit's business.
delegate_noop!(State: ignore wl_pointer::WlPointer);
delegate_noop!(State: ZwpPointerGesturesV1);
