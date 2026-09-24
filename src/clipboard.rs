//! The clipboard over `wl_data_device` (ARCHITECTURE.md §7.1).
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
//! A copy goes the other way: the board offers a `wl_data_source`, and
//! whoever pastes it is written the text on a thread of its own. Setting
//! the selection answers an input event by its serial, which winit keeps
//! to itself, so the same queue holds a keyboard and a pointer of its own
//! on the seat — only to note the last serial either one was handed.
//!
//! Not on Wayland, or no `wl_data_device_manager`: no paste, no copy,
//! nothing else lost.

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_offer::{self, WlDataOffer},
    wl_data_source::{self, WlDataSource},
    wl_keyboard::{self, WlKeyboard},
    wl_pointer::{self, WlPointer},
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

/// The board's own layers, as a clip: a document fragment in the board's
/// JSON, which only this program writes and only this program reads.
pub const LAYERS_MIME: &str = "application/x-omawhite-layers+json";

/// What `Ctrl+V` on the board takes, best first: layers copied from a
/// board, then an image from anywhere.
const PASTE_MIMES: [&str; 4] = [LAYERS_MIME, IMAGE_MIMES[0], IMAGE_MIMES[1], IMAGE_MIMES[2]];

/// Text as a clipboard owner may name it, best first: the MIME type with
/// its charset said, then the UTF-8 atom X11 programs offer, then plain
/// text, which on Wayland is UTF-8 in practice.
const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// How many bytes of text one paste may bring in. Every field that takes
/// one is capped far below this; the ceiling is for the read.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// The first of `wanted` that is on offer.
fn best(offered: &[String], wanted: &[&'static str]) -> Option<&'static str> {
    wanted
        .iter()
        .find(|w| offered.iter().any(|m| m == *w))
        .copied()
}

/// Whether a paste that arrived as `mime` is text, and not an image.
pub fn is_text(mime: &str) -> bool {
    TEXT_MIMES.contains(&mime)
}

/// Whether a paste that arrived as `mime` is a board's layers.
pub fn is_layers(mime: &str) -> bool {
    mime == LAYERS_MIME
}

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
    /// What a copy is made through: a source comes from the manager and
    /// is set as the selection on the device, and its events are
    /// dispatched on the clipboard's own queue.
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    queue: QueueHandle<State>,
    /// The last input serial the seat handed this client.
    serial: Arc<AtomicU32>,
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
        // Heard only for their serials: a selection is set in answer to
        // an input event, and a keyboard of our own on the seat is told
        // of every key the window is.
        let keyboard = seat.get_keyboard(&qh, ());
        let pointer = seat.get_pointer(&qh, ());

        let selection = Arc::new(Mutex::new(None));
        let serial = Arc::new(AtomicU32::new(0));
        let mut state = State {
            selection: selection.clone(),
            serial: serial.clone(),
        };
        let held = device.clone();
        std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _held = (held, keyboard, pointer);
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
            manager,
            device,
            queue: qh,
            serial,
        })
    }

    /// Offers `text` as the selection, for any client to paste — this
    /// window's own fields included. The text is held by the source
    /// until another selection replaces it.
    pub fn copy_text(&self, text: &str) {
        self.offer(&TEXT_MIMES, text.as_bytes().to_vec());
    }

    /// Offers a clip of the board's layers, in the board's own JSON, as
    /// the selection: `Ctrl+V` in any board takes it back.
    pub fn copy_layers(&self, json: String) {
        self.offer(&[LAYERS_MIME], json.into_bytes());
    }

    /// Makes `bytes` the selection, as each of `mimes`.
    fn offer(&self, mimes: &[&str], bytes: Vec<u8>) {
        let source = self.manager.create_data_source(&self.queue, Arc::new(bytes));
        for mime in mimes {
            source.offer((*mime).to_owned());
        }
        self.device
            .set_selection(Some(&source), self.serial.load(Ordering::Relaxed));
        if let Err(e) = self.conn.flush() {
            log::warn!("clipboard: {e}");
        }
    }

    /// Asks for the selection as a board's layers or, failing that, as an
    /// image. Returns whether anything was asked for — the bytes arrive
    /// on the sink, later. Nothing happens, and nothing is reported, when
    /// the clipboard holds neither.
    pub fn paste(&self) -> bool {
        self.receive(&PASTE_MIMES, MAX_PASTE_BYTES)
    }

    /// Asks for the selection as text, on the same terms: the text
    /// arrives on the sink, and a clipboard holding none asks nothing.
    pub fn paste_text(&self) -> bool {
        self.receive(&TEXT_MIMES, MAX_TEXT_BYTES)
    }

    /// Whether the selection holds something `Ctrl+V` on the board takes.
    pub fn can_paste(&self) -> bool {
        self.on_offer(&PASTE_MIMES).is_some()
    }

    /// Asks the owner for the selection as the best of `wanted` it offers
    /// and reads the answer, at most `cap` bytes of it, off the loop.
    fn receive(&self, wanted: &[&'static str], cap: u64) -> bool {
        let Some((offer, mime)) = self.on_offer(wanted) else {
            log::debug!("clipboard: nothing of {wanted:?} in the selection");
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
            .spawn(move || match read_all(reader, cap) {
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

    /// The current offer and the best of `wanted` it lists.
    fn on_offer(&self, wanted: &[&'static str]) -> Option<(WlDataOffer, String)> {
        let guard = self.selection.lock().ok()?;
        let offer = guard.as_ref()?;
        let mimes = offer.data::<Mimes>()?.0.lock().ok()?;
        let mime = best(&mimes, wanted)?;
        Some((offer.clone(), mime.to_owned()))
    }
}

use std::os::fd::AsFd as _;

fn read_all(mut reader: std::io::PipeReader, cap: u64) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let read = std::io::Read::by_ref(&mut reader)
        .take(cap + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        read as u64 <= cap,
        "clipboard offered more than {cap} bytes"
    );
    Ok(bytes)
}

struct State {
    selection: Arc<Mutex<Option<WlDataOffer>>>,
    serial: Arc<AtomicU32>,
}

impl Dispatch<WlDataSource, Arc<Vec<u8>>> for State {
    fn event(
        _: &mut State,
        source: &WlDataSource,
        event: wl_data_source::Event,
        text: &Arc<Vec<u8>>,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            // Someone is pasting. The write goes on a thread of its own,
            // so a reader that never reads cannot stall the queue every
            // other clipboard event arrives on.
            wl_data_source::Event::Send { fd, .. } => {
                let text = text.clone();
                let written = std::thread::Builder::new()
                    .name("copy".into())
                    .spawn(move || {
                        let mut out = std::fs::File::from(fd);
                        if let Err(e) = out.write_all(&text) {
                            log::warn!("clipboard: writing the copy: {e}");
                        }
                    });
                if let Err(e) = written {
                    log::warn!("clipboard: {e}");
                }
            }
            // Another selection took its place: nothing will ask again.
            wl_data_source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut State,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } => {
                state.serial.store(serial, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut State,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, .. } | wl_pointer::Event::Button { serial, .. } => {
                state.serial.store(serial, Ordering::Relaxed);
            }
            _ => {}
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(mimes: &[&str]) -> Vec<String> {
        mimes.iter().map(|m| (*m).to_owned()).collect()
    }

    #[test]
    fn text_is_asked_for_as_utf8_first() {
        let all = offered(&[
            "TEXT",
            "text/plain",
            "UTF8_STRING",
            "text/plain;charset=utf-8",
        ]);
        assert_eq!(best(&all, &TEXT_MIMES), Some("text/plain;charset=utf-8"));
        let x11 = offered(&["STRING", "UTF8_STRING"]);
        assert_eq!(best(&x11, &TEXT_MIMES), Some("UTF8_STRING"));
    }

    #[test]
    fn an_image_on_offer_is_no_text_to_paste() {
        let png = offered(&["image/png"]);
        assert_eq!(best(&png, &TEXT_MIMES), None);
        assert_eq!(best(&png, &IMAGE_MIMES), Some("image/png"));
    }

    #[test]
    fn a_paste_takes_the_boards_layers_before_an_image_of_them() {
        let both = offered(&["image/png", LAYERS_MIME]);
        assert_eq!(best(&both, &PASTE_MIMES), Some(LAYERS_MIME));
        let png = offered(&["image/png", "text/plain"]);
        assert_eq!(best(&png, &PASTE_MIMES), Some("image/png"));
        assert_eq!(best(&offered(&["text/plain"]), &PASTE_MIMES), None, "text is a field's");
        assert!(is_layers(LAYERS_MIME) && !is_layers("image/png") && !is_text(LAYERS_MIME));
    }

    #[test]
    fn only_a_text_mime_reads_as_text() {
        assert!(is_text("text/plain;charset=utf-8"));
        assert!(is_text("UTF8_STRING"));
        assert!(!is_text("image/png"));
    }
}
