//! Reading the clipboard over `wl_data_device` (ARCHITECTURE.md §7.1).
//!
//! The same guest-client trick as `gestures`: this joins the window's
//! Wayland connection instead of opening one of its own, so the compositor
//! still sees a single client and the `selection` events — which only ever
//! reach whoever holds the keyboard focus — reach us. A thread with its own
//! queue keeps track of what the current offer is; the paste itself is
//! requested from the event loop (Wayland requests are thread-safe) and the
//! bytes are read on a short-lived thread, so a slow or dead clipboard
//! owner cannot stall a frame.
//!
//! Not on Wayland, or no `wl_data_device_manager`: no paste, nothing else
//! lost.

use std::io::Read as _;
use std::sync::{Arc, Mutex};

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_offer::{self, WlDataOffer},
    wl_registry, wl_seat,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use winit::window::Window;

/// What a paste produced: the bytes, as the mime type they arrived as.
#[derive(Debug)]
pub struct Paste {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Delivers a finished paste to the event loop; `false` once the loop is
/// gone.
pub type Sink = Arc<dyn Fn(Paste) -> bool + Send + Sync>;

/// Image types we can decode, best first: PNG is lossless and what a
/// screenshot tool puts there; JPEG last, since it is nobody's first
/// choice for a board.
const IMAGE_MIMES: [&str; 3] = ["image/png", "image/webp", "image/jpeg"];

/// How many bytes one image may bring in. Past this the read gives up
/// instead of following a hostile or broken owner forever. It is the
/// ceiling for image bytes however they arrive — the clipboard here, and
/// the `blobs/` an agent hands over beside a fragment.
pub const MAX_PASTE_BYTES: u64 = 128 * 1024 * 1024;

/// The mime types on offer, filled in as they arrive and read when the
/// selection settles.
#[derive(Default)]
struct Mimes(Mutex<Vec<String>>);

pub struct Clipboard {
    conn: Connection,
    /// The offer the compositor last announced as the selection.
    selection: Arc<Mutex<Option<WlDataOffer>>>,
    sink: Sink,
}

impl Clipboard {
    /// Starts watching the selection for `window`. Failures are logged,
    /// not fatal: the board works without a clipboard.
    pub fn spawn(window: &Window, sink: Sink) -> anyhow::Result<Clipboard> {
        let RawDisplayHandle::Wayland(handle) = window.display_handle()?.as_raw() else {
            anyhow::bail!("not on Wayland");
        };
        // SAFETY: the display belongs to winit's event loop, which outlives
        // every window; the guest backend never closes it. Same contract as
        // `gestures`.
        let backend = unsafe { Backend::from_foreign_display(handle.display.as_ptr() as *mut _) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let manager: WlDataDeviceManager = globals
            .bind(&qh, 1..=3, ())
            .map_err(|e| anyhow::anyhow!("wl_data_device_manager: {e}"))?;
        let seat_interface = wl_seat::WlSeat::interface().name;
        let seat: wl_seat::WlSeat = globals
            .contents()
            .clone_list()
            .into_iter()
            .find(|g| g.interface == seat_interface)
            .map(|g| globals.registry().bind(g.name, g.version.min(1), &qh, ()))
            .ok_or_else(|| anyhow::anyhow!("no wl_seat"))?;
        // Kept alive for as long as the thread runs: dropping the device
        // ends the selection events.
        let device = manager.get_data_device(&seat, &qh, ());

        let selection = Arc::new(Mutex::new(None));
        let mut state = State {
            selection: selection.clone(),
        };
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _device = device;
                loop {
                    if let Err(e) = queue.blocking_dispatch(&mut state) {
                        log::warn!("clipboard: {e:#}");
                        return;
                    }
                }
            })?;
        log::info!("clipboard: watching the selection over wl_data_device");
        Ok(Clipboard {
            conn,
            selection,
            sink,
        })
    }

    /// Asks for the selection as an image. Returns whether anything was
    /// asked for — the bytes themselves arrive on the sink, later. Nothing
    /// happens, and nothing is reported, when the clipboard holds no image.
    pub fn paste_image(&self) -> bool {
        let Some((offer, mime)) = self.image_on_offer() else {
            log::debug!("clipboard: no image in the selection");
            return false;
        };
        let (reader, writer) = match std::io::pipe() {
            Ok(pair) => pair,
            Err(e) => {
                log::warn!("clipboard: no pipe for the paste: {e}");
                return false;
            }
        };
        offer.receive(mime.clone(), writer.as_fd());
        // The owner writes into the fd only once it has the request, and
        // this is not the thread that dispatches.
        if let Err(e) = self.conn.flush() {
            log::warn!("clipboard: {e}");
            return false;
        }
        // Our end of the write side has to go, or the read never sees EOF.
        drop(writer);
        let sink = self.sink.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("paste".into())
            .spawn(move || match read_all(reader) {
                Ok(bytes) => {
                    sink(Paste { mime, bytes });
                }
                Err(e) => log::warn!("clipboard: reading the paste: {e:#}"),
            })
        {
            log::warn!("clipboard: {e}");
            return false;
        }
        true
    }

    /// The current offer and the best image type it lists.
    fn image_on_offer(&self) -> Option<(WlDataOffer, String)> {
        let guard = self.selection.lock().ok()?;
        let offer = guard.as_ref()?;
        let mimes = offer.data::<Mimes>()?.0.lock().ok()?;
        let best = IMAGE_MIMES
            .iter()
            .find(|want| mimes.iter().any(|m| m == *want))
            .map(|m| (*m).to_owned())?;
        Some((offer.clone(), best))
    }
}

use std::os::fd::AsFd as _;

fn read_all(mut reader: std::io::PipeReader) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let read = std::io::Read::by_ref(&mut reader)
        .take(MAX_PASTE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        read as u64 <= MAX_PASTE_BYTES,
        "clipboard offered more than {MAX_PASTE_BYTES} bytes"
    );
    Ok(bytes)
}

struct State {
    selection: Arc<Mutex<Option<WlDataOffer>>>,
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut State,
        _device: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        // Drag and drop (enter/leave/motion/drop) is not ours yet.
        if let wl_data_device::Event::Selection { id } = event
            && let Ok(mut current) = state.selection.lock()
        {
            // The offer we were holding is dead the moment a new
            // selection lands.
            if let Some(old) = std::mem::replace(&mut *current, id) {
                old.destroy();
            }
        }
    }

    wayland_client::event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mimes::default()),
    ]);
}

impl Dispatch<WlDataOffer, Mimes> for State {
    fn event(
        _: &mut State,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        mimes: &Mimes,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Ok(mut list) = mimes.0.lock()
        {
            list.push(mime_type);
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

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: WlDataDeviceManager);
