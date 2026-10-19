//! The clipboard, kept by ClamShell (the session's shell) through the compositor's data control
//! (`ext_data_control_manager_v1`: given to a shell, not to apps; the clipboard read and set without a
//! window). Text only; files copied in the Files tab are that tab's own.
//!
//! - Kept: each text copied is read as it is copied; when the program it came from quits (the clipboard
//!   emptied: on Wayland the data stays with the program that copied it), ClamShell offers it itself, so
//!   it can still be pasted. Something else copied (an image) forgets it.
//! - Copied to the primary selection too, so what a middle click pastes (Foot's right click: see its
//!   config) is the last text copied or selected, wherever.
//! - Sent to the state ([`Reading::Clipboard`]) for ClamShell's own fields' Ctrl+V.
//! - `clamshell --clipboard get|set` ([`get`], [`set`]): the clipboard's text written out, or set from
//!   what is read in (Waydroid's clipboard sharing, through a stand-in for its `pyclip`).
//!
//! A text is read on a thread of its own: the program offering it may be ClamShell itself (its own
//! offer, after a quit), which answers on this one.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use clamshell_core::{Event, Reading};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{self, ExtDataControlDeviceV1};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::{self, ExtDataControlManagerV1};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{self, ExtDataControlOfferV1};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{self, ExtDataControlSourceV1};

use crate::{Sink, worker};

/// The text types offered, and taken in this order of preference.
const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];
/// The most text kept (bytes): more is left to the program that copied it.
const MOST: usize = 1 << 20;
/// How long `--clipboard set` waits for its text to be taken (ClamShell keeps it then).
const SET_WAIT: Duration = Duration::from_secs(5);

/// Keeps the clipboard (see the module's doc), on a thread of its own, as long as the compositor is there.
pub fn spawn(sink: impl Sink + Clone + 'static) -> io::Result<()> {
    worker::detached("clipboard", move || {
        // (Told from the reading threads.)
        let sink = Mutex::new(sink);
        let on_read = move |text: Option<String>| {
            let sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = sink.send(Event::Reading(Reading::Clipboard(text.map(Arc::from))));
        };
        let kept = Arc::new(Kept { text: Mutex::new(None), on_read: Box::new(on_read) });
        match Keeper::connect(Mode::Keep { kept, own: None }) {
            Ok((mut keeper, mut queue)) => {
                log::info!("clipboard: kept by ClamShell");
                while queue.blocking_dispatch(&mut keeper).is_ok() {}
            }
            Err(e) => log::warn!("clipboard: {e}"),
        }
    })
}

/// The clipboard's text, if it has one (`--clipboard get`).
pub fn get() -> Result<Option<String>, String> {
    let (mut keeper, mut queue) = Keeper::connect(Mode::Get { found: None })?;
    // (The compositor tells the clipboard's offer as the device is made.)
    for _ in 0..2 {
        queue.roundtrip(&mut keeper).map_err(|e| e.to_string())?;
    }
    let Mode::Get { found: Some((offer, mime)) } = &keeper.mode else {
        return Ok(None);
    };
    let read = receive(&keeper.conn, offer, mime)?;
    Ok(read_text(read))
}

/// Sets the clipboard to `text` (`--clipboard set`), until it is taken (by ClamShell, which keeps it), or
/// for [`SET_WAIT`].
pub fn set(text: String) -> Result<(), String> {
    let (mut keeper, mut queue) = Keeper::connect(Mode::Set { text, done: false })?;
    let qh = queue.handle();
    let source = keeper.manager.create_data_source(&qh, ());
    for t in TEXT {
        source.offer(t.to_owned());
    }
    keeper.device.set_selection(Some(&source));
    let start = Instant::now();
    while !matches!(keeper.mode, Mode::Set { done: true, .. }) && start.elapsed() < SET_WAIT {
        dispatch_for(&mut queue, &mut keeper, Duration::from_millis(200))?;
    }
    // (A moment for the text written to be read.)
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// What is kept, and who is told.
struct Kept {
    text: Mutex<Option<String>>,
    on_read: Box<dyn Fn(Option<String>) + Send + Sync>,
}

impl Kept {
    fn text(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.text.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

enum Mode {
    /// ClamShell keeping the clipboard; its own offer, while the clipboard is it.
    Keep { kept: Arc<Kept>, own: Option<ExtDataControlSourceV1> },
    /// `--clipboard get`: the clipboard's offer of text, once the compositor has said.
    Get { found: Option<(ExtDataControlOfferV1, &'static str)> },
    /// `--clipboard set`: the text offered, and whether it has been taken (or replaced).
    Set { text: String, done: bool },
}

struct Keeper {
    conn: Connection,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
    /// Offers announced, with their types (until the selection says which is current).
    offers: Vec<(ExtDataControlOfferV1, Vec<String>)>,
    mode: Mode,
}

impl Keeper {
    fn connect(mode: Mode) -> Result<(Keeper, EventQueue<Keeper>), String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, queue) = registry_queue_init::<Keeper>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("no data control: {e}"))?;
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
        let device = manager.get_data_device(&seat, &qh, ());
        Ok((Keeper { conn, manager, device, offers: Vec::new(), mode }, queue))
    }

    /// The best text type `offer` has, if any.
    fn text_type(&self, offer: &ExtDataControlOfferV1) -> Option<&'static str> {
        let types = &self.offers.iter().find(|(o, _)| o == offer)?.1;
        TEXT.into_iter().find(|t| types.iter().any(|x| x == t))
    }

    /// The clipboard changed to `offer` (none: emptied).
    fn selection(&mut self, offer: Option<ExtDataControlOfferV1>, qh: &QueueHandle<Keeper>) {
        let mime = offer.as_ref().and_then(|o| self.text_type(o));
        for (o, _) in std::mem::take(&mut self.offers) {
            if Some(&o) != offer.as_ref() {
                o.destroy();
            }
        }
        match &mut self.mode {
            Mode::Get { found } => *found = offer.zip(mime),
            Mode::Set { .. } => {}
            Mode::Keep { kept, own } => match (offer, mime) {
                // Emptied (the program that copied quit): offered by ClamShell, if it has a text.
                (None, _) => {
                    if kept.text().is_some() {
                        let source = self.manager.create_data_source(qh, ());
                        for t in TEXT {
                            source.offer(t.to_owned());
                        }
                        self.device.set_selection(Some(&source));
                        *own = Some(source);
                    }
                }
                // Not text: what was kept isn't the clipboard any more.
                (Some(offer), None) => {
                    offer.destroy();
                    if kept.text().take().is_some() {
                        (kept.on_read)(None);
                    }
                }
                (Some(offer), Some(mime)) => {
                    let read = receive(&self.conn, &offer, mime);
                    offer.destroy();
                    let kept = kept.clone();
                    let (conn, manager, device, qh) = (self.conn.clone(), self.manager.clone(), self.device.clone(), qh.clone());
                    let reading = worker::detached("clipboard read", move || {
                        let Some(text) = read.ok().and_then(read_text) else {
                            return;
                        };
                        let changed = kept.text().as_ref() != Some(&text);
                        if changed {
                            *kept.text() = Some(text.clone());
                            (kept.on_read)(Some(text));
                            // The primary selection too: what a middle click (Foot: a right click)
                            // pastes is then the last text copied or selected.
                            let source = manager.create_data_source(&qh, ());
                            for t in TEXT {
                                source.offer(t.to_owned());
                            }
                            device.set_primary_selection(Some(&source));
                            let _ = conn.flush();
                        }
                    });
                    if let Err(e) = reading {
                        log::warn!("clipboard: {e}");
                    }
                }
            },
        }
    }

    /// The clipboard's text asked of an offer of ours: written on a thread of its own (the one asking may
    /// be this program).
    fn send(&mut self, fd: OwnedFd) {
        let text = match &mut self.mode {
            Mode::Keep { kept, .. } => kept.text().clone(),
            Mode::Set { text, done } => {
                *done = true;
                Some(text.clone())
            }
            Mode::Get { .. } => None,
        };
        let Some(text) = text else {
            return;
        };
        let written = worker::detached("clipboard send", move || {
            let _ = File::from(fd).write_all(text.as_bytes());
        });
        if let Err(e) = written {
            log::warn!("clipboard: {e}");
        }
    }
}

/// Asks `offer` for its text as `mime`: the pipe's end to read it from.
fn receive(conn: &Connection, offer: &ExtDataControlOfferV1, mime: &str) -> Result<OwnedFd, String> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|e| e.to_string())?;
    offer.receive(mime.to_owned(), write.as_fd());
    conn.flush().map_err(|e| e.to_string())?;
    drop(write);
    Ok(read)
}

/// A pipe's text, to its end (none: too long, or not text).
fn read_text(read: OwnedFd) -> Option<String> {
    let mut bytes = Vec::new();
    File::from(read).take(MOST as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MOST {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Dispatches what comes within `wait`.
fn dispatch_for(queue: &mut EventQueue<Keeper>, keeper: &mut Keeper, wait: Duration) -> Result<(), String> {
    queue.flush().map_err(|e| e.to_string())?;
    if let Some(guard) = queue.prepare_read() {
        let fd = guard.connection_fd();
        let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
        let timeout = rustix::event::Timespec::try_from(wait).ok();
        if rustix::event::poll(&mut fds, timeout.as_ref()).is_ok_and(|n| n > 0) {
            guard.read().map_err(|e| e.to_string())?;
        }
    }
    queue.dispatch_pending(keeper).map_err(|e| e.to_string())?;
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Keeper {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_seat::WlSeat, ()> for Keeper {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlManagerV1, ()> for Keeper {
    fn event(_: &mut Self, _: &ExtDataControlManagerV1, _: ext_data_control_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Keeper {
    fn event(keeper: &mut Self, _: &ExtDataControlDeviceV1, event: ext_data_control_device_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => keeper.offers.push((id, Vec::new())),
            ext_data_control_device_v1::Event::Selection { id } => keeper.selection(id, qh),
            _ => {}
        }
    }

    event_created_child!(Keeper, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for Keeper {
    fn event(keeper: &mut Self, offer: &ExtDataControlOfferV1, event: ext_data_control_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some((_, types)) = keeper.offers.iter_mut().find(|(o, _)| o == offer)
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for Keeper {
    fn event(keeper: &mut Self, source: &ExtDataControlSourceV1, event: ext_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_data_control_source_v1::Event::Send { fd, .. } => keeper.send(fd),
            ext_data_control_source_v1::Event::Cancelled => {
                source.destroy();
                match &mut keeper.mode {
                    Mode::Keep { own, .. } if own.as_ref() == Some(source) => *own = None,
                    Mode::Set { done, .. } => *done = true,
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
