//! The file chooser served to apps: ClamShell owns `org.freedesktop.impl.portal.desktop.clamshell` on the
//! session bus and serves xdg-desktop-portal's `org.freedesktop.impl.portal.FileChooser`, which the
//! session's portals config (clamshell-portals.conf) sends there. The rest of what apps ask of the desktop
//! stays with GTK's portal, which also takes the file chooser while ClamShell isn't there.
//!
//! An `OpenFile`, `SaveFile` or `SaveFiles` call is kept, unanswered, and sent on as
//! [`PortalEvent::Asked`], with the asking process (by the request's handle, which names the app's
//! connection) and the folder the app suggests; the state picks it in the Library (clamshell-state's `portal`) and answers with
//! [`Command::Answer`], which replies with the paths' `file://` URIs, or as cancelled. `Close` on the request's handle, from xdg-desktop-portal when the
//! app gives up, is sent on as [`PortalEvent::Closed`] and the call dropped. A request waits for the
//! person, with no time limit.
//!
//! If another program has the name, that is a warning, and the name is asked for again every [`RETRY`].

use std::fmt;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use clamshell_state::{Asks, Event, FileRequest, PortalEvent};

use crate::os::dbus::{self, Bus, Message, PropMap, RefArg};
use crate::os::worker::{self, Bell, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};

const NAME: &str = "org.freedesktop.impl.portal.desktop.clamshell";
const PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER: &str = "org.freedesktop.impl.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.impl.portal.Request";
const BUS_NAME: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// How long to wait before trying the bus, or the name, again.
const RETRY: Duration = Duration::from_secs(5);

/// The portal's responses: chosen, and cancelled by the person.
const CHOSEN: u32 = 0;
const CANCELLED: u32 = 1;

/// The last request number given out. It outlives the thread, so that one started again after a panic
/// doesn't give out a number the state still holds.
static LAST_ID: AtomicU32 = AtomicU32::new(0);

/// What the state asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Answer request `id`: the paths chosen, none if cancelled.
    Answer { id: u32, paths: Vec<PathBuf> },
}

#[derive(Debug)]
pub(crate) enum Error {
    Bus(dbus::Error),
    /// Another program has the name.
    Taken,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::Taken => write!(f, "another program has {NAME}; apps' file choosers are GTK's until it is ours"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

/// A request kept until the state answers it.
struct Pending {
    id: u32,
    /// The object path xdg-desktop-portal closes it by.
    handle: String,
    call: Message,
    /// The request, until it is sent on (`run_while_up`), the asking process found first.
    unsent: Option<FileRequest>,
}

type Requests = Arc<Mutex<Vec<Pending>>>;

/// Serves the file chooser, sending its requests to `sink`; answers them as the commands say.
pub(crate) fn spawn(sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    let bell = Bell::new()?;
    let mut server = Server { sink, bell: bell.clone(), requests: Requests::default() };
    Worker::spawn_belled(clamshell_state::Thread::Portal.name(), bell, move |commands| worker::supervise(&mut server, commands, RETRY))
}

struct Server<S> {
    sink: S,
    bell: Bell,
    requests: Requests,
}

impl<S: Sink + Clone> Service for Server<S> {
    type Command = Command;
    type Session = Bus;
    type Error = Error;
    const NAME: &'static str = "portal";

    fn connect(&mut self) -> Result<Bus, Error> {
        // The calls of a connection that failed can't be answered: their pickers close.
        for gone in std::mem::take(&mut *lock(&self.requests)) {
            log::info!("portal: request {} gone with the bus", gone.id);
            if self.sink.send(Event::Portal(PortalEvent::Closed(gone.id))).is_err() {
                log::debug!("portal: request {} not closed, the UI is gone", gone.id);
            }
        }
        let mut bus = Bus::session()?;
        // Flag 4 is DBUS_NAME_FLAG_DO_NOT_QUEUE. Reply 1 means we are the primary owner, 4 that we already were.
        let (reply,): (u32,) = bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "RequestName", (NAME, 4u32))?;
        if !matches!(reply, 1 | 4) {
            return Err(Error::Taken);
        }
        log::info!("portal: {NAME} is ours");
        let requests = self.requests.clone();
        bus.serve_later(Box::new(move |call| keep(call, &requests)));
        let (sink, requests) = (self.sink.clone(), self.requests.clone());
        bus.serve(Box::new(move |call| answer(call, &requests, &sink)));
        Ok(bus)
    }

    fn run_while_up(&mut self, mut bus: Bus, commands: &Receiver<Command>) -> Result<(), Ended<Error>> {
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => reply(&mut bus, &self.requests, command)?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // Keeps or answers the calls that came, and sends on the requests kept.
            bus.signals()?;
            self.send_kept(&mut bus);
            // Sleep until a command or a call comes.
            let mut fds = [
                rustix::event::PollFd::from_borrowed_fd(self.bell.fd(), rustix::event::PollFlags::IN),
                rustix::event::PollFd::from_borrowed_fd(bus.fd(), rustix::event::PollFlags::IN),
            ];
            if let Err(e) = rustix::event::poll(&mut fds, None) {
                log::debug!("portal: waiting: {e}");
            }
            self.bell.clear();
        }
    }

    /// Without the bus, the call to answer is gone (dropped as it reconnects).
    fn command_while_down(&mut self, command: Command) -> Result<(), Closed> {
        log::debug!("portal: not answered, the bus or the name isn't ours: {command:?}");
        Ok(())
    }
}

impl<S: Sink> Server<S> {
    /// Sends on the requests kept since the last look, each with the asking process and its parents. They
    /// are found here and not as the call is kept, when the bus can't be called.
    fn send_kept(&mut self, bus: &mut Bus) {
        let unsent: Vec<(String, FileRequest)> = lock(&self.requests).iter_mut().filter_map(|p| Some((p.handle.clone(), p.unsent.take()?))).collect();
        for (handle, mut request) in unsent {
            let pid = sender(&handle).and_then(|name| {
                bus.call::<(u32,)>(BUS_NAME, BUS_PATH, BUS_NAME, "GetConnectionUnixProcessID", (name.as_str(),))
                    .inspect_err(|e| log::warn!("portal: request {}: the asking process of {name}: {e}", request.id))
                    .ok()
            });
            request.asker = pid.map(|(pid,)| crate::os::sys::lineage(pid)).unwrap_or_default();
            log::info!(
                "portal: request {}: {:?}, {:?}, asked by {:?}, from {:?}",
                request.id,
                request.asks,
                request.patterns,
                request.asker.first(),
                request.folder
            );
            if self.sink.send(Event::Portal(PortalEvent::Asked(request))).is_err() {
                log::debug!("portal: a request not shown, the UI is gone");
            }
        }
    }
}

/// The unique bus name of the app a request's handle is for: its path is
/// `/org/freedesktop/portal/desktop/request/SENDER/TOKEN`, SENDER the name without its `:` and with `_`
/// for each `.`.
fn sender(handle: &str) -> Option<String> {
    let sender = handle.strip_prefix("/org/freedesktop/portal/desktop/request/")?.split('/').next()?;
    Some(format!(":{}", sender.replace('_', ".")))
}

fn lock(requests: &Requests) -> std::sync::MutexGuard<'_, Vec<Pending>> {
    requests.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The calls' arguments: the request's handle, the app's id, its window, the title and the options.
type Args = (dbus::Path<'static>, String, String, String, PropMap);

/// Keeps an `OpenFile`, `SaveFile` or `SaveFiles` call to answer once the state has, sending it on; hands
/// back any other call.
fn keep(call: Message, requests: &Requests) -> Option<Message> {
    let member = call.member().map(|m| m.to_string()).unwrap_or_default();
    if call.path().as_deref() != Some(PATH)
        || call.interface().as_deref() != Some(FILE_CHOOSER)
        || !matches!(member.as_str(), "OpenFile" | "SaveFile" | "SaveFiles")
    {
        return Some(call);
    }
    // One that can't be read is handed back, and refused as the others are.
    let (handle, app, _window, title, options): Args = match call.read_all() {
        Ok(args) => args,
        Err(e) => {
            log::warn!("portal: a {member} call that can't be read: {e}");
            return Some(call);
        }
    };
    let id = LAST_ID.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let request = request(id, &member, title, &options);
    log::info!("portal: request {id}: a {member} call from {app:?}");
    lock(requests).push(Pending { id, handle: handle.to_string(), call, unsent: Some(request) });
    None
}

/// The reply to a call that isn't kept, or none for one that isn't served.
fn answer(call: &Message, requests: &Requests, sink: &impl Sink) -> Option<Message> {
    match (call.interface().as_deref()?, &*call.member()?) {
        (REQUEST, "Close") => {
            let path = call.path()?.to_string();
            let mut requests = lock(requests);
            if let Some(i) = requests.iter().position(|r| r.handle == path) {
                let given_up = requests.swap_remove(i);
                let id = given_up.id;
                log::info!("portal: request {id} given up by the app");
                // One not sent on yet is just dropped: the state never had it.
                if given_up.unsent.is_none() && sink.send(Event::Portal(PortalEvent::Closed(id))).is_err() {
                    log::debug!("portal: request {id} not closed, the UI is gone");
                }
            }
            Some(call.method_return())
        }
        ("org.freedesktop.DBus.Introspectable", "Introspect") => Some(call.return_with_args((XML,))),
        ("org.freedesktop.DBus.Peer", "Ping") => Some(call.method_return()),
        _ => None,
    }
}

/// Replies to the request `command` answers, if it is still kept.
fn reply(bus: &mut Bus, requests: &Requests, command: Command) -> dbus::Result<()> {
    let Command::Answer { id, paths } = command;
    let pending = {
        let mut requests = lock(requests);
        requests.iter().position(|r| r.id == id).map(|i| requests.swap_remove(i))
    };
    let Some(pending) = pending else {
        log::debug!("portal: request {id} answered, but it was given up");
        return Ok(());
    };
    let message = if paths.is_empty() {
        log::info!("portal: request {id} cancelled, nothing chosen");
        pending.call.return_with_args((CANCELLED, PropMap::new()))
    } else {
        log::info!("portal: request {id} chose {paths:?}");
        let uris: Vec<String> = paths.iter().map(|p| uri(p)).collect();
        pending.call.return_with_args((CHOSEN, PropMap::from([("uris".to_owned(), dbus::value(uris))])))
    };
    bus.send(message)
}

/// The request a call's options make. `OpenFile`: a folder if `directory`, else a file. `SaveFile`: a
/// file, suggested as `current_name`. `SaveFiles`: a folder for the `files` named. A file matches the
/// globs of the filter the app chose (`current_filter`), or of its first; a filter of MIME types only
/// takes any file.
fn request(id: u32, member: &str, title: String, options: &PropMap) -> FileRequest {
    let asks = match member {
        "SaveFile" => {
            let name = options.get("current_name").and_then(|v| v.0.as_str().map(str::to_owned));
            let of_file = || options.get("current_file").and_then(|v| bytes_path(&*v.0)?.file_name().map(|n| n.to_string_lossy().into_owned()));
            Asks::Save { name: name.or_else(of_file).unwrap_or_default() }
        }
        "SaveFiles" => Asks::SaveFiles { names: options.get("files").map(|v| names(&*v.0)).unwrap_or_default() },
        _ => Asks::Open { folder: options.get("directory").and_then(|v| v.0.as_u64()).is_some_and(|b| b != 0) },
    };
    let filter = options.get("current_filter").map(|v| &*v.0).or_else(|| options.get("filters").and_then(|v| v.0.as_iter()?.next()));
    let patterns = filter.map(globs).unwrap_or_default();
    // The folder to start in: the one suggested, or the file's to save over.
    let path = |key: &str| options.get(key).and_then(|v| bytes_path(&*v.0));
    let folder = path("current_folder").or_else(|| path("current_file").and_then(|f| Some(f.parent()?.to_path_buf())));
    FileRequest { id, title, asks, patterns, asker: Vec::new(), folder }
}

/// A path sent as bytes, ending with a nul.
fn bytes_path(bytes: &dyn RefArg) -> Option<PathBuf> {
    let bytes: Vec<u8> = bytes.as_iter()?.filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok())).take_while(|&b| b != 0).collect();
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// The names of `SaveFiles`' `files`: paths as bytes, ending with a nul, each named by its last part.
fn names(files: &dyn RefArg) -> Vec<String> {
    let Some(files) = files.as_iter() else { return Vec::new() };
    files.filter_map(|file| Some(bytes_path(file)?.file_name()?.to_string_lossy().into_owned())).collect()
}

/// A filter's globs: it is `(name, [(kind, pattern)])`, kind 0 a glob and 1 a MIME type.
fn globs(filter: &dyn RefArg) -> Vec<String> {
    let Some(list) = filter.as_iter().and_then(|mut f| f.nth(1)) else { return Vec::new() };
    let Some(items) = list.as_iter() else { return Vec::new() };
    items
        .filter_map(|item| {
            let mut fields = item.as_iter()?;
            let kind = fields.next()?.as_u64()?;
            let pattern = fields.next()?.as_str()?;
            (kind == 0).then(|| pattern.to_owned())
        })
        .collect()
}

/// `path` as a `file://` URI: each byte but the unreserved ones and `/` percent-encoded.
fn uri(path: &Path) -> String {
    use std::fmt::Write;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// What is served, as introspection gives it.
const XML: &str = r#"<node><interface name="org.freedesktop.impl.portal.FileChooser">
<method name="OpenFile"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
<method name="SaveFile"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
<method name="SaveFiles"><arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/><arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/><arg type="a{sv}" name="options" direction="in"/><arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/></method>
</interface><interface name="org.freedesktop.impl.portal.Request"><method name="Close"/></interface></node>"#;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::os::dbus::value;

    #[test]
    fn a_path_is_a_file_uri() {
        assert_eq!(uri(Path::new("/home/thor/My Game (EU).sfc")), "file:///home/thor/My%20Game%20%28EU%29.sfc");
        assert_eq!(uri(Path::new("/h/ä~")), "file:///h/%C3%A4~");
    }

    #[test]
    fn a_folder_or_the_chosen_filters_globs() {
        let filter = |globs: &[(u32, &str)]| ("Images".to_owned(), globs.iter().map(|(k, p)| (*k, (*p).to_owned())).collect::<Vec<_>>());
        let images = filter(&[(0, "*.png"), (1, "image/jpeg"), (0, "*.jpg")]);
        let options = PropMap::from([("filters".to_owned(), value(vec![images.clone(), filter(&[(0, "*")])]))]);
        let r = request(1, "OpenFile", "Open".into(), &options);
        assert_eq!(
            (&r.asks, r.patterns.as_slice()),
            (&Asks::Open { folder: false }, ["*.png", "*.jpg"].map(String::from).as_slice()),
            "the first filter's globs"
        );
        let chosen = PropMap::from([("filters".to_owned(), value(vec![images])), ("current_filter".to_owned(), value(filter(&[(0, "*.txt")])))]);
        assert_eq!(request(1, "OpenFile", String::new(), &chosen).patterns, ["*.txt"], "the one chosen");
        let mime = PropMap::from([("current_filter".to_owned(), value(filter(&[(1, "text/plain")])))]);
        assert!(request(1, "OpenFile", String::new(), &mime).patterns.is_empty(), "MIME types only: any file");
        let folder = PropMap::from([("directory".to_owned(), value(true))]);
        assert_eq!(request(1, "OpenFile", String::new(), &folder).asks, Asks::Open { folder: true });
    }

    #[test]
    fn a_save_is_of_the_name_suggested_or_the_files_named() {
        let named = PropMap::from([("current_name".to_owned(), value("notes.txt".to_owned()))]);
        assert_eq!(request(1, "SaveFile", String::new(), &named).asks, Asks::Save { name: "notes.txt".into() });
        let files = PropMap::from([("files".to_owned(), value(vec![b"/tmp/a.txt\0".to_vec(), b"b.png\0".to_vec()]))]);
        assert_eq!(request(1, "SaveFiles", String::new(), &files).asks, Asks::SaveFiles { names: vec!["a.txt".into(), "b.png".into()] });
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Needs a session bus, so a desktop. An OpenFile call through the bus is kept and sent on, with the
    /// asking process and the folder it suggests, and answered with the chosen file's URI once the state answers. Served on the connection's own name. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a session bus"]
    #[allow(clippy::unwrap_used)]
    fn an_open_call_waits_for_its_answer() {
        let mut server = Bus::session().unwrap();
        let name = server.unique_name().unwrap();
        let (sink, events) = std::sync::mpsc::channel();
        let events_sink = sink.clone();
        let requests = Requests::default();
        let r = requests.clone();
        server.serve_later(Box::new(move |call| keep(call, &r)));
        let r = requests.clone();
        server.serve(Box::new(move |call| answer(call, &r, &sink)));
        let mut portal = Server { sink: events_sink, bell: Bell::new().unwrap(), requests: requests.clone() };
        let caller = {
            let name = name.clone();
            std::thread::spawn(move || {
                let mut client = Bus::session().unwrap().with_timeout(Duration::from_secs(10));
                let folder = b"/h/saves\0".to_vec();
                let options = PropMap::from([("directory".to_owned(), dbus::value(false)), ("current_folder".to_owned(), dbus::value(folder))]);
                // As xdg-desktop-portal names it: by the app's connection.
                let me = client.unique_name().unwrap().trim_start_matches(':').replace('.', "_");
                let handle = dbus::Path::from(format!("/org/freedesktop/portal/desktop/request/{me}/t"));
                client.call::<(u32, PropMap)>(&name, PATH, FILE_CHOOSER, "OpenFile", (handle, "app", "", "Open a save", options)).unwrap()
            })
        };
        let asked = loop {
            server.wait(Some(Duration::from_millis(100))).unwrap();
            portal.send_kept(&mut server);
            if let Ok(Event::Portal(PortalEvent::Asked(asked))) = events.try_recv() {
                break asked;
            }
        };
        assert_eq!((asked.title.as_str(), &asked.asks), ("Open a save", &Asks::Open { folder: false }));
        assert_eq!(asked.asker.first(), Some(&std::process::id()), "asked by this process");
        assert_eq!(asked.folder.as_deref(), Some(std::path::Path::new("/h/saves")));
        reply(&mut server, &requests, Command::Answer { id: asked.id, paths: vec!["/h/a b.sav".into()] }).unwrap();
        let (response, results) = caller.join().unwrap();
        assert_eq!(response, CHOSEN);
        let uris: Vec<String> = results.get("uris").and_then(|v| v.0.as_iter()).unwrap().filter_map(|u| u.as_str().map(str::to_owned)).collect();
        assert_eq!(uris, ["file:///h/a%20b.sav"]);
    }
}
