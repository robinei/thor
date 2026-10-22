//! The clipboard, kept by ClamShell as the session's shell. It uses the compositor's data control
//! protocol (`ext_data_control_manager_v1`), which is given to a shell and not to apps, and lets the
//! clipboard be read and set without a window. Only text is handled; files copied in the Files tab are
//! kept by that tab.
//!
//! - Each copied text is read as it is copied. On Wayland the data stays with the program that copied
//!   it, so when that program quits the clipboard is emptied. ClamShell then offers the text itself, so
//!   it can still be pasted. Copying something else, such as an image, forgets the text.
//! - The text is also copied to the primary selection, so a middle click (a right click in Foot, see its
//!   config) pastes the last text copied or selected anywhere.
//! - The text is also written to the user's runtime directory, a tmpfs only the user can read that is
//!   gone at a reboot. A ClamShell started again, after an update or a crash, offers it if the clipboard
//!   is empty then.
//! - A password manager's copy, marked `x-kde-passwordManagerHint`, is treated as if it weren't text. It
//!   isn't kept, written, or pasted into ClamShell's fields, and what was kept is forgotten. The manager
//!   clears it itself.
//! - The text is sent to the state ([`Reading::Clipboard`]) for Ctrl+V in ClamShell's own fields.
//! - `clamshell --clipboard get|set` ([`get`], [`set`]) writes out the clipboard's text, or sets it from
//!   stdin. Waydroid's clipboard sharing uses it through a stand-in for its `pyclip`.
//!
//! A text is read on a thread of its own, because the program offering it may be ClamShell itself (its
//! own offer, after the copying program quit), which answers on this thread.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, Mutex};
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

use crate::Sink;
use crate::os::worker;

/// The text types offered, and taken in this order of preference.
const TEXT: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];
/// The most text kept, in bytes. Anything longer is left to the program that copied it.
const MOST: usize = 1 << 20;
/// The type a password manager's copy is marked with, as honoured by KDE's clipboard and others.
const SECRET: &str = "x-kde-passwordManagerHint";
/// How long `--clipboard set` waits for its text to be taken. Once ClamShell takes it, ClamShell keeps it.
const SET_WAIT: Duration = Duration::from_secs(5);

/// Where the kept text is also written, so that a ClamShell started again after an update or a crash
/// offers it. It is in the user's runtime directory, which only they can read and which is gone at a
/// reboot.
fn saved() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join("clamshell/clipboard"))
}

/// Keeps the clipboard (see the module's doc) on a thread of its own, as long as the compositor is
/// there. It starts with the text an earlier ClamShell kept, offered if the clipboard is empty.
pub fn spawn(sink: impl Sink + Clone + 'static) -> io::Result<()> {
    worker::detached("clipboard", move || {
        let before = saved()
            .and_then(|path| {
                std::fs::read_to_string(&path)
                    .inspect_err(|e| {
                        if e.kind() == io::ErrorKind::NotFound {
                            log::debug!("clipboard: {}: {e}", path.display());
                        } else {
                            log::warn!("clipboard: {}: {e}", path.display());
                        }
                    })
                    .ok()
            })
            .filter(|t| t.len() <= MOST);
        if before.is_some() {
            let _ = sink.send(Event::Reading(Reading::Clipboard(before.as_deref().map(Arc::from))));
        }
        // The reading threads send through this.
        let sink = Mutex::new(sink);
        let on_read = move |text: Option<String>| {
            if let Some(path) = saved() {
                let written = match &text {
                    Some(t) => path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
                        use std::os::unix::fs::OpenOptionsExt;
                        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
                        file.write_all(t.as_bytes())
                    }),
                    None => std::fs::remove_file(&path).or(Ok(())),
                };
                if let Err(e) = written {
                    log::debug!("clipboard: {}: {e}", path.display());
                }
            }
            let sink = crate::os::lock(&sink);
            let _ = sink.send(Event::Reading(Reading::Clipboard(text.map(Arc::from))));
        };
        let kept = Arc::new(KeptText { text: Mutex::new(before), on_read: Box::new(on_read) });
        match Keeper::connect(Mode::Keep { kept, own: None }) {
            Ok((mut keeper, mut queue)) => {
                log::info!("clipboard: kept by ClamShell");
                let gone = loop {
                    if let Err(e) = queue.blocking_dispatch(&mut keeper) {
                        break e;
                    }
                };
                log::warn!("clipboard: {gone}");
            }
            Err(e) => log::warn!("clipboard: {e}"),
        }
    })
}

/// The clipboard's text, if it has one (`--clipboard get`).
pub fn get() -> Result<Option<String>, String> {
    let (mut keeper, mut queue) = Keeper::connect(Mode::Get { found: None })?;
    // The compositor sends the clipboard's offer when the device is made.
    for _ in 0..2 {
        queue.roundtrip(&mut keeper).map_err(|e| e.to_string())?;
    }
    let Mode::Get { found: Some((offer, mime)) } = &keeper.mode else {
        return Ok(None);
    };
    let read = receive(&keeper.conn, offer, mime)?;
    Ok(read_text(read))
}

/// Sets the clipboard to `text` (`--clipboard set`) until it is taken, normally by ClamShell, which then
/// keeps it, or for at most [`SET_WAIT`].
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
    // Give the reader a moment to read the written text.
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// The kept text, and who is told when it changes.
struct KeptText {
    text: Mutex<Option<String>>,
    on_read: Box<dyn Fn(Option<String>) + Send + Sync>,
}

impl KeptText {
    fn text(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        crate::os::lock(&self.text)
    }
}

enum Mode {
    /// ClamShell keeping the clipboard. `own` is its own offer while that is the clipboard.
    Keep { kept: Arc<KeptText>, own: Option<ExtDataControlSourceV1> },
    /// `--clipboard get`: the clipboard's text offer, once the compositor has sent it.
    Get { found: Option<(ExtDataControlOfferV1, &'static str)> },
    /// `--clipboard set`: the text offered, and whether it has been taken or replaced.
    Set { text: String, done: bool },
}

struct Keeper {
    conn: Connection,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
    /// Offers announced with their types, kept until the selection says which is current.
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

    /// The best text type `offer` has, if any. None for a secret: a password manager's copy, marked
    /// `x-kde-passwordManagerHint`. The manager clears the clipboard itself after a while, which keeping
    /// the text would undo, and the text must never be written down.
    fn text_type(&self, offer: &ExtDataControlOfferV1) -> Option<&'static str> {
        let types = &self.offers.iter().find(|(o, _)| o == offer)?.1;
        if types.iter().any(|t| t == SECRET) {
            return None;
        }
        TEXT.into_iter().find(|t| types.iter().any(|x| x == t))
    }

    /// The clipboard changed to `offer`, or was emptied if it is none.
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
                // Emptied because the program that copied quit: ClamShell offers its text, if it has one.
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
                        let Some(text) = read.inspect_err(|e| log::warn!("clipboard: reading the copied text: {e}")).ok().and_then(read_text) else {
                            return;
                        };
                        let changed = kept.text().as_ref() != Some(&text);
                        if changed {
                            *kept.text() = Some(text.clone());
                            (kept.on_read)(Some(text));
                            // Set the primary selection too, so a middle click (a right click in Foot)
                            // pastes the last text copied or selected.
                            let source = manager.create_data_source(&qh, ());
                            for t in TEXT {
                                source.offer(t.to_owned());
                            }
                            device.set_primary_selection(Some(&source));
                            if let Err(e) = conn.flush() {
                                log::warn!("clipboard: offering the primary selection: {e}");
                            }
                        }
                    });
                    if let Err(e) = reading {
                        log::warn!("clipboard: {e}");
                    }
                }
            },
        }
    }

    /// Sends the clipboard's text when one of our offers is asked for it. It is written on a thread of its
    /// own, since the program asking may be this one.
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
            if let Err(e) = File::from(fd).write_all(text.as_bytes()) {
                log::debug!("clipboard: sending the text: {e}");
            }
        });
        if let Err(e) = written {
            log::warn!("clipboard: {e}");
        }
    }
}

/// Asks `offer` for its text as `mime`. Returns the end of the pipe to read it from.
fn receive(conn: &Connection, offer: &ExtDataControlOfferV1, mime: &str) -> Result<OwnedFd, String> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|e| e.to_string())?;
    offer.receive(mime.to_owned(), write.as_fd());
    conn.flush().map_err(|e| e.to_string())?;
    drop(write);
    Ok(read)
}

/// Reads a pipe's text to the end. None if it is too long or not text.
fn read_text(read: OwnedFd) -> Option<String> {
    let mut bytes = Vec::new();
    File::from(read).take(MOST as u64 + 1).read_to_end(&mut bytes).inspect_err(|e| log::warn!("clipboard: reading the copied text: {e}")).ok()?;
    if bytes.len() > MOST {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Dispatches the events that arrive within `wait`.
fn dispatch_for(queue: &mut EventQueue<Keeper>, keeper: &mut Keeper, wait: Duration) -> Result<(), String> {
    queue.flush().map_err(|e| e.to_string())?;
    if let Some(guard) = queue.prepare_read() {
        let fd = guard.connection_fd();
        let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
        let timeout = rustix::event::Timespec::try_from(wait).ok();
        if rustix::event::poll(&mut fds, timeout.as_ref()).inspect_err(|e| log::debug!("clipboard: waiting for the compositor: {e}")).is_ok_and(|n| n > 0) {
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
