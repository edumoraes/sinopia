//! The pen — a graphics tablet's tool over `zwp_tablet_v2`. winit 0.30 has
//! no tablet events on Wayland, so this joins the window's connection as a
//! guest client: its own tablet seat on the same `wl_seat`, its own event
//! queue on its own thread, and each step goes to the event loop through
//! the sink — the same bridge shape as the gestures and the clipboard.
//! Not on Wayland, or no protocol: no pen, nothing else lost.

use std::collections::HashMap;

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
    zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
    zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::ZwpTabletV2,
};
use winit::window::Window;

use crate::editor::Stylus;

/// What the protocol calls a full press.
const PRESSURE_MAX: f64 = 65535.0;

/// One hardware event from the tool, as the event loop reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pen {
    /// How hard the tool presses and which way it is held, as this
    /// frame leaves them. First out of a frame: a sample has to be
    /// taken with the axes that came with it, not with the ones before.
    Axes(Stylus),
    /// Where the tool is, surface-local and in logical px — the units
    /// `wl_surface` speaks, which is not what winit hands the loop.
    Motion { x: f64, y: f64 },
    /// The tip came into logical contact with the tablet.
    Down,
    /// And left it.
    Up,
    /// The tool left the tablet's reach. Whatever it was last saying
    /// stops being true, and the mouse takes the pointer back.
    Away,
}

/// The axis and button updates of one `frame`, held until it closes.
/// The protocol sends them one at a time and calls the whole set a single
/// hardware event, so nothing leaves until the `frame` arrives: a press
/// has to land on the position that came with it, not on the one before.
///
/// The axes are the exception to the clean slate: the protocol sends
/// only the ones that changed, so they stand as they were until the
/// tool moves them, and a frame that says nothing about the pressure
/// means the pressure it already had.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pending {
    motion: Option<(f64, f64)>,
    /// Contact, if it changed this frame.
    tip: Option<bool>,
    /// The axes as they stand, carried from frame to frame.
    stylus: Stylus,
    /// Whether any of them moved this frame.
    leaned: bool,
}

impl Default for Pending {
    fn default() -> Pending {
        Pending {
            motion: None,
            tip: None,
            // A tool that has said nothing yet presses all the way: a
            // pen that never reports pressure draws as a mouse does,
            // rather than not at all.
            stylus: Stylus::MOUSE,
            leaned: false,
        }
    }
}

impl Pending {
    /// The tool moved. Only the last position of a frame is a position.
    pub fn motion(&mut self, x: f64, y: f64) {
        self.motion = Some((x, y));
    }

    /// The tip went down or came up.
    pub fn tip(&mut self, down: bool) {
        self.tip = Some(down);
    }

    /// How hard the tip presses, as the protocol counts it: 0 to 65535.
    pub fn pressure(&mut self, pressure: u32) {
        self.stylus.pressure = (f64::from(pressure) / PRESSURE_MAX).clamp(0.0, 1.0);
        self.leaned = true;
    }

    /// Which way the barrel leans, in degrees off the tablet's z axis.
    pub fn tilt(&mut self, x: f64, y: f64) {
        self.stylus.tilt = (x, y);
        self.leaned = true;
    }

    /// How far the barrel is rolled, in degrees clockwise. Only an art
    /// pen reports one.
    pub fn roll(&mut self, degrees: f64) {
        self.stylus.roll = degrees;
        self.leaned = true;
    }

    /// The frame closed: what it has to say, the axes before the
    /// movement and the movement before the contact, and a clean slate
    /// for the next one — except for the axes, which stand.
    pub fn flush(&mut self) -> Vec<Pen> {
        let mut out = Vec::new();
        if std::mem::take(&mut self.leaned) {
            out.push(Pen::Axes(self.stylus));
        }
        if let Some((x, y)) = self.motion.take() {
            out.push(Pen::Motion { x, y });
        }
        match self.tip.take() {
            Some(true) => out.push(Pen::Down),
            Some(false) => out.push(Pen::Up),
            None => {}
        }
        out
    }
}

/// Delivers one step of the pen to the event loop; `false` once the loop
/// is gone.
pub type Sink = Box<dyn Fn(Pen) -> bool + Send>;

/// The display pointer travels to the thread as an address; it is only
/// ever handed back to libwayland.
struct Display(usize);

/// Starts the bridge for `window` on its own thread. Failures are logged,
/// not fatal: the board works without a tablet.
pub fn spawn(window: &Window, sink: Sink) -> anyhow::Result<()> {
    let RawDisplayHandle::Wayland(handle) = window.display_handle()?.as_raw() else {
        log::info!("not on Wayland: no tablet");
        return Ok(());
    };
    let display = Display(handle.display.as_ptr() as usize);
    std::thread::Builder::new()
        .name("tablet".into())
        .spawn(move || {
            if let Err(e) = run(display, sink) {
                log::warn!("tablet unavailable: {e:#}");
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
    // Version 1: everything the pen says — motion, tip, pressure — is in
    // it, and it spares us the dial a v2 pad group would announce.
    let manager: ZwpTabletManagerV2 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| anyhow::anyhow!("zwp_tablet_manager_v2: {e}"))?;
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
    // The tablet seats are what announce the tools; they live as long as
    // the thread does.
    let _tablet_seats: Vec<ZwpTabletSeatV2> = seats
        .iter()
        .map(|seat| manager.get_tablet_seat(seat, &qh, ()))
        .collect();
    let mut state = State {
        frames: HashMap::new(),
        sink,
        alive: true,
    };
    log::info!("tablet: the pen over zwp_tablet_v2");
    while state.alive {
        queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}

struct State {
    /// One open frame per tool: a pen and its eraser end are two tools,
    /// and each closes its own frames.
    frames: HashMap<ObjectId, Pending>,
    sink: Sink,
    alive: bool,
}

impl State {
    fn emit(&mut self, pen: Pen) {
        if !(self.sink)(pen) {
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

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut State,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        use zwp_tablet_tool_v2::Event;
        match event {
            // The axes and the tip of one hardware event arrive one by
            // one; the frame is what says they are over.
            Event::Motion { x, y } => state.frames.entry(tool.id()).or_default().motion(x, y),
            Event::Down { .. } => state.frames.entry(tool.id()).or_default().tip(true),
            Event::Up => state.frames.entry(tool.id()).or_default().tip(false),
            Event::Pressure { pressure } => {
                state.frames.entry(tool.id()).or_default().pressure(pressure);
            }
            Event::Tilt { tilt_x, tilt_y } => {
                state.frames.entry(tool.id()).or_default().tilt(tilt_x, tilt_y);
            }
            Event::Rotation { degrees } => {
                state.frames.entry(tool.id()).or_default().roll(degrees);
            }
            Event::Frame { .. } => {
                let Some(frame) = state.frames.get_mut(&tool.id()) else {
                    return;
                };
                for pen in frame.flush() {
                    state.emit(pen);
                }
            }
            // The compositor only sends a tool over our own surface, and
            // the protocol puts the `up` before the `proximity_out` that
            // follows it — so there is never a stroke left open here.
            // What the tool was saying, though, stops being true: the
            // loop puts the axes back to a mouse's own, or the next
            // mouse stroke would draw at the pressure the pen left.
            Event::ProximityIn { .. } => log::debug!("tablet: {:?} in reach", tool.id()),
            Event::ProximityOut => {
                state.frames.remove(&tool.id());
                state.emit(Pen::Away);
            }
            Event::Removed => {
                state.frames.remove(&tool.id());
            }
            _ => {}
        }
    }
}

// The seat announces the tablets, the tools and the pads; only the tools
// are ours. The pad and its groups still have to be given a home, or the
// first event about the four express keys has nowhere to land.
impl Dispatch<ZwpTabletSeatV2, ()> for State {
    fn event(
        _: &mut State,
        _: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let zwp_tablet_seat_v2::Event::ToolAdded { id } = event {
            log::debug!("tablet: tool {:?}", id.id());
        }
    }

    event_created_child!(State, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadV2, ()> for State {
    fn event(
        _: &mut State,
        _: &ZwpTabletPadV2,
        _: zwp_tablet_pad_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }

    event_created_child!(State, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for State {
    fn event(
        _: &mut State,
        _: &ZwpTabletPadGroupV2,
        _: zwp_tablet_pad_group_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }

    event_created_child!(State, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
    ]);
}

// Our seat only exists to ask for a tablet seat on; its own events are
// winit's business, and the rest of the tablet tree we never read.
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore ZwpTabletV2);
delegate_noop!(State: ignore ZwpTabletPadRingV2);
delegate_noop!(State: ignore ZwpTabletPadStripV2);
delegate_noop!(State: ZwpTabletManagerV2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_moves_before_it_touches() {
        let mut f = Pending::default();
        f.motion(10.0, 20.0);
        f.tip(true);
        assert_eq!(
            f.flush(),
            vec![Pen::Motion { x: 10.0, y: 20.0 }, Pen::Down],
            "the press has to land where the frame put the tool"
        );
    }

    #[test]
    fn a_lift_comes_after_the_move_that_ended_it() {
        let mut f = Pending::default();
        f.motion(3.0, 4.0);
        f.tip(false);
        assert_eq!(f.flush(), vec![Pen::Motion { x: 3.0, y: 4.0 }, Pen::Up]);
    }

    #[test]
    fn a_frame_of_movement_alone_is_one_move() {
        let mut f = Pending::default();
        f.motion(1.0, 2.0);
        assert_eq!(f.flush(), vec![Pen::Motion { x: 1.0, y: 2.0 }]);
    }

    #[test]
    fn the_last_position_of_a_frame_is_the_one_that_counts() {
        let mut f = Pending::default();
        f.motion(1.0, 2.0);
        f.motion(5.0, 6.0);
        assert_eq!(f.flush(), vec![Pen::Motion { x: 5.0, y: 6.0 }]);
    }

    #[test]
    fn a_touch_without_a_move_stays_where_it_was() {
        let mut f = Pending::default();
        f.tip(true);
        assert_eq!(f.flush(), vec![Pen::Down], "the loop keeps the position");
    }

    #[test]
    fn an_empty_frame_says_nothing() {
        assert!(Pending::default().flush().is_empty());
    }

    #[test]
    fn the_axes_of_a_frame_come_before_its_movement() {
        let mut f = Pending::default();
        f.motion(10.0, 20.0);
        f.pressure(32768);
        f.tip(true);
        let out = f.flush();
        assert!(
            matches!(out[0], Pen::Axes(_)),
            "a sample is taken with the axes that came with it"
        );
        assert_eq!(out[1], Pen::Motion { x: 10.0, y: 20.0 });
        assert_eq!(out[2], Pen::Down);
    }

    #[test]
    fn a_full_press_is_what_the_protocol_calls_one() {
        let mut f = Pending::default();
        f.pressure(65535);
        assert_eq!(f.flush(), vec![Pen::Axes(Stylus::MOUSE)]);
        f.pressure(0);
        let Some(Pen::Axes(s)) = f.flush().first().copied() else {
            panic!("no axes")
        };
        assert_eq!(s.pressure, 0.0, "and nothing is nothing");
    }

    #[test]
    fn the_axes_stand_until_the_tool_moves_them() {
        let mut f = Pending::default();
        f.pressure(16384);
        f.tilt(30.0, 40.0);
        f.flush();
        // A frame that says nothing about them says they have not moved.
        f.motion(1.0, 2.0);
        assert_eq!(
            f.flush(),
            vec![Pen::Motion { x: 1.0, y: 2.0 }],
            "unchanged axes are not news"
        );
        // And one that moves only the tilt still carries the pressure.
        f.tilt(0.0, 10.0);
        let Some(Pen::Axes(s)) = f.flush().first().copied() else {
            panic!("no axes")
        };
        assert_eq!(s.tilt, (0.0, 10.0));
        assert!((s.pressure - 0.25).abs() < 1e-3, "the press it already had");
    }

    #[test]
    fn a_tool_that_never_reports_a_pressure_presses_all_the_way() {
        let mut f = Pending::default();
        f.motion(1.0, 2.0);
        f.tip(true);
        assert_eq!(
            f.flush(),
            vec![Pen::Motion { x: 1.0, y: 2.0 }, Pen::Down],
            "no axes to report, and a mouse's own until there are"
        );
    }

    #[test]
    fn a_frame_does_not_repeat_the_one_before_it() {
        let mut f = Pending::default();
        f.motion(1.0, 2.0);
        f.tip(true);
        assert_eq!(f.flush().len(), 2);
        assert!(
            f.flush().is_empty(),
            "the slate is clean for the next frame"
        );
    }
}
