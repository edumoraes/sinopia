//! A guest client's thread, and the way to end it while the display it
//! borrows still stands.
//!
//! `gestures`, `tablet` and `clipboard` each join winit's Wayland
//! connection as a guest: their own queue on the window's `wl_display`,
//! dispatched on a thread of their own. Ending such a thread touches that
//! display — dropping the guest's `Connection` destroys every proxy it made
//! and its event queue, all of them the display's — so it has to happen
//! before winit disconnects it. "The loop is gone" cannot be the signal:
//! a sink only fails once the event loop has been dropped, and that is the
//! very moment the display may be freed. Waiting for it crashed the board
//! on every close, in `wl_proxy_destroy` on a display that was no longer
//! there.
//!
//! So the loop ends them itself, from `exiting`, where the display is
//! still alive: `Guest::end` wakes the thread, lets it drop everything it
//! holds, and joins it. The wake is a pipe polled beside the display's fd,
//! not a request to the compositor — ending a thread must not depend on
//! anybody answering.

use std::io::{PipeReader, PipeWriter};
use std::os::fd::{AsFd as _, AsRawFd as _};
use std::thread::JoinHandle;

use wayland_client::EventQueue;
use wayland_client::backend::WaylandError;

/// A running guest thread. `end` it while the display stands.
pub struct Guest {
    name: String,
    /// Closing it is what wakes the thread.
    wake: PipeWriter,
    thread: JoinHandle<()>,
}

/// What a guest thread is handed to learn it has been told to end.
pub struct Stop(PipeReader);

impl Guest {
    /// Runs `body` on a thread named `name`, handing it the `Stop` it
    /// dispatches against.
    pub fn spawn(name: &str, body: impl FnOnce(Stop) + Send + 'static) -> std::io::Result<Guest> {
        let (reader, wake) = std::io::pipe()?;
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || body(Stop(reader)))?;
        Ok(Guest {
            name: name.into(),
            wake,
            thread,
        })
    }

    /// Tells the thread to end and waits until it has — until everything
    /// it held on the display has been destroyed.
    pub fn end(self) {
        drop(self.wake);
        if self.thread.join().is_err() {
            log::warn!("{}: the thread panicked", self.name);
        }
    }
}

/// Dispatches `queue` until the thread is told to end or `going` says it
/// is done — `blocking_dispatch` in a loop, but with the stop polled
/// beside the display.
pub fn dispatch<S>(
    queue: &mut EventQueue<S>,
    state: &mut S,
    stop: &Stop,
    going: impl Fn(&S) -> bool,
) -> anyhow::Result<()> {
    loop {
        queue.dispatch_pending(state)?;
        if !going(state) {
            return Ok(());
        }
        queue.flush()?;
        // Events already queued: dispatch them before sleeping.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [
            libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN | libc::POLLERR,
                revents: 0,
            },
            libc::pollfd {
                fd: stop.0.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            // SAFETY: two live fds, and the length is the array's.
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } >= 0 {
                break;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
        // The writer closed: the guard cancels the read as it drops, so
        // the host is never left waiting on a reader that went away.
        if fds[1].revents != 0 {
            return Ok(());
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
    }
}
