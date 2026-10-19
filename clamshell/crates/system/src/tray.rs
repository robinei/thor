//! Programs' tray icons: freedesktop's StatusNotifierItem (KDE's names) on the session bus, with their
//! menus (com.canonical.dbusmenu). Programs register their icons with the watcher
//! (`org.kde.StatusNotifierWatcher`): ClamShell is it where no one else is (the device's session: no
//! panel), else (a desktop's session, around a nested ClamShell) reads the one there. The icons are read
//! again after any signal of the watcher, an icon, a menu or the bus (a program gone): each one's title,
//! process (and its systemd unit: whose it is), icon file and menu, sent as [`Event::Tray`].
//!
//! Commands: an icon activated (the program shows itself), a menu entry clicked.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::tray::{TrayCommand, TrayEntry, TrayItem};
use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::worker::{self, Bell, Changed, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink, cgroup};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";

/// How long to wait before trying the bus again.
const RETRY: Duration = Duration::from_secs(5);
/// How long after a signal the icons are read: a burst of them (a menu rebuilt) is one read.
const SETTLE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum Error {
    Bus(zbus::Error),
    SignalsEnded,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::SignalsEnded => write!(f, "the bus' signals ended"),
        }
    }
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Bus(e)
    }
}

impl From<zbus::fdo::Error> for Error {
    fn from(e: zbus::fdo::Error) -> Self {
        Error::Bus(e.into())
    }
}

lost_on!(Error: zbus::Error, zbus::fdo::Error);

/// Sends [`Event::Tray`] at start and after every change; carries out [`TrayCommand`]s.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<TrayCommand>> {
    Worker::spawn_belled("tray", move |commands, bell| worker::supervise(&mut Tray { sink, bell }, &commands, RETRY))
}

struct Tray<S> {
    sink: S,
    bell: Bell,
}

/// That the icons are to be read again: marked by signals and registrations, which ring the thread's
/// bell.
#[derive(Clone)]
struct Dirty {
    flag: Arc<AtomicBool>,
    bell: Bell,
}

impl Dirty {
    fn mark(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.bell.ring();
    }

    fn take(&self) -> bool {
        self.flag.swap(false, Ordering::Relaxed)
    }
}

/// A connection to the session bus: the icons registered (ours, if we are the watcher; else read from
/// the watcher there), and whether a signal came since they were read.
struct Session {
    conn: Connection,
    ours: Option<Arc<Mutex<Vec<String>>>>,
    dirty: Dirty,
    /// Each icon's menu's object path, by the icon's key (for clicks).
    menus: HashMap<String, OwnedObjectPath>,
}

impl<S: Sink> Service for Tray<S> {
    type Command = TrayCommand;
    type Session = Session;
    type Error = Error;
    const NAME: &'static str = "tray";

    fn connect(&mut self) -> Result<Session, Error> {
        let conn = Connection::session()?;
        let dirty = Dirty { flag: Arc::new(AtomicBool::new(true)), bell: self.bell.clone() };
        let ours = serve_watcher(&conn, &dirty)?;
        follow_signals(&conn, &dirty)?;
        Ok(Session { conn, ours, dirty, menus: HashMap::new() })
    }

    fn run_while_up(&mut self, mut s: Session, commands: &Receiver<TrayCommand>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        loop {
            if s.dirty.take() {
                std::thread::sleep(SETTLE);
                s.dirty.take();
                let (items, menus) = read(&s.conn, &registered(&s)?);
                s.menus = menus;
                if let Some(items) = last.offer(items) {
                    let names: Vec<String> = items.iter().map(|t| format!("{} ({} entries)", t.title, t.menu.len())).collect();
                    log::info!("tray: {}", if names.is_empty() { "no icons".to_owned() } else { names.join(", ") });
                    self.sink.send(Event::Tray(items))?;
                }
            }
            loop {
                match commands.try_recv() {
                    Ok(command) => {
                        if let Err(e) = carry_out(&s, &command) {
                            log::warn!("tray: {command:?}: {e}");
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // Asleep until a command or a signal.
            let mut fds = [rustix::event::PollFd::from_borrowed_fd(self.bell.fd(), rustix::event::PollFlags::IN)];
            let _ = rustix::event::poll(&mut fds, None);
            self.bell.clear();
        }
    }

    fn command_while_down(&mut self, _: TrayCommand) -> Result<(), Closed> {
        Ok(())
    }
}

/// The watcher, served where no one else is.
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
    dirty: Dirty,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// An icon registered: by its D-Bus name (its object at the usual path), or by its object path (the
    /// sender's). Kept as KDE's watcher has it: the name, then the path.
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let sender = header.sender().map(ToString::to_string).unwrap_or_default();
        let key = if service.starts_with('/') { format!("{sender}{service}") } else { format!("{service}{ITEM_PATH}") };
        {
            let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
            if !items.contains(&key) {
                items.push(key.clone());
            }
        }
        self.dirty.mark();
        let _ = Self::status_notifier_item_registered(&emitter, &key).await;
    }

    /// A host (a panel) registered: we are the only one shown.
    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Serves the watcher, if no one else does: the icons registered with it; none if another has the name.
fn serve_watcher(conn: &Connection, dirty: &Dirty) -> Result<Option<Arc<Mutex<Vec<String>>>>, Error> {
    let items = Arc::new(Mutex::new(Vec::new()));
    conn.object_server().at(WATCHER_PATH, Watcher { items: items.clone(), dirty: dirty.clone() })?;
    match conn.request_name_with_flags(WATCHER, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner) => {
            log::info!("tray: the watcher is ours");
            // (Programs that started before us wait for a host to show their icons.)
            if let Ok(iface) = conn.object_server().interface::<_, Watcher>(WATCHER_PATH) {
                let _ = zbus::block_on(Watcher::status_notifier_host_registered(iface.signal_emitter()));
            }
            Ok(Some(items))
        }
        _ => {
            let _ = conn.object_server().remove::<Watcher, _>(WATCHER_PATH);
            log::info!("tray: reading the watcher there");
            Ok(None)
        }
    }
}

/// Marks the icons to be read again at any signal of the watcher, an icon, a menu, or a name's owner
/// changing (a program gone), on a thread of its own.
fn follow_signals(conn: &Connection, dirty: &Dirty) -> Result<(), Error> {
    let dbus = DBusProxy::new(conn)?;
    for interface in [WATCHER, ITEM, MENU] {
        dbus.add_match_rule(zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(interface)?.build())?;
    }
    dbus.add_match_rule(
        zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.build(),
    )?;
    let messages = zbus::blocking::MessageIterator::from(conn.clone());
    let dirty = dirty.clone();
    worker::detached("tray signals", move || {
        for message in messages.flatten() {
            if message.message_type() == zbus::message::Type::Signal {
                dirty.mark();
            }
        }
    })
    .map_err(|e| Error::Bus(zbus::Error::InputOutput(Arc::new(e))))
}

/// The icons registered: ours (those whose program is gone dropped), or the watcher's there.
fn registered(s: &Session) -> Result<Vec<String>, Error> {
    let Some(ours) = &s.ours else {
        let watcher = zbus::blocking::Proxy::new(&s.conn, WATCHER, WATCHER_PATH, WATCHER)?;
        return Ok(watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems").unwrap_or_default());
    };
    let dbus = DBusProxy::new(&s.conn)?;
    let mut items = ours.lock().unwrap_or_else(PoisonError::into_inner);
    items.retain(|key| BusName::try_from(split(key).0).is_ok_and(|name| dbus.name_has_owner(name).unwrap_or(false)));
    Ok(items.clone())
}

/// An icon's key as the name and the object path.
fn split(key: &str) -> (&str, &str) {
    match key.find('/') {
        Some(i) => key.split_at(i),
        None => (key, ITEM_PATH),
    }
}

/// Every icon (those that can't be read left out), and their menus' paths.
fn read(conn: &Connection, keys: &[String]) -> (Vec<TrayItem>, HashMap<String, OwnedObjectPath>) {
    let dirs = crate::desktop::data_dirs();
    let mut menus = HashMap::new();
    let items = keys
        .iter()
        .filter_map(|key| match read_item(conn, key, &dirs) {
            Ok((item, menu)) => {
                if let Some(menu) = menu {
                    menus.insert(key.clone(), menu);
                }
                Some(item)
            }
            Err(e) => {
                log::debug!("tray: {key}: {e}");
                None
            }
        })
        .collect();
    (items, menus)
}

fn read_item(conn: &Connection, key: &str, dirs: &[PathBuf]) -> Result<(TrayItem, Option<OwnedObjectPath>), Error> {
    let (name, path) = split(key);
    let props = zbus::blocking::fdo::PropertiesProxy::builder(conn).destination(name)?.path(path)?.build()?;
    let all = props.get_all(ITEM.try_into().map_err(zbus::Error::from)?)?;
    let text = |k: &str| all.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    let pid = DBusProxy::new(conn)?.get_connection_unix_process_id(BusName::try_from(name).map_err(zbus::Error::from)?).ok();
    let menu = all.get("Menu").and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok()).filter(|p| p.as_str() != "/");
    let entries = menu.as_ref().map(|m| read_menu(conn, name, m)).unwrap_or_default();
    let icon_name = text("IconName");
    let icon = (!icon_name.is_empty()).then(|| find_icon(&icon_name, &text("IconThemePath"), dirs)).flatten();
    let item = TrayItem { key: key.into(), id: text("Id"), title: text("Title"), pid, unit: pid.and_then(cgroup::unit_of), icon, menu: entries };
    Ok((item, menu))
}

/// An icon's image file: in the program's own theme dir (as named, or under hicolor), else the themes'.
fn find_icon(name: &str, theme_path: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if !theme_path.is_empty() {
        let base = Path::new(theme_path);
        for ext in ["svg", "png"] {
            let direct = base.join(format!("{name}.{ext}"));
            if direct.is_file() {
                return Some(direct);
            }
        }
        let parent = base.parent().map(Path::to_path_buf);
        if let Some(found) = crate::desktop::find_icon(name, &[base.to_path_buf()].into_iter().chain(parent).collect::<Vec<_>>()) {
            return Some(found);
        }
    }
    crate::desktop::find_icon(name, dirs)
}

/// A menu's entries, as its program has them now (asked to bring the menu up to date first).
fn read_menu(conn: &Connection, name: &str, path: &OwnedObjectPath) -> Vec<TrayEntry> {
    let Ok(menu) = zbus::blocking::Proxy::new(conn, name.to_owned(), path.to_owned(), MENU) else {
        return vec![];
    };
    let _: zbus::Result<bool> = menu.call("AboutToShow", &(0i32,));
    let layout: zbus::Result<(u32, Node)> = menu.call("GetLayout", &(0i32, -1i32, Vec::<String>::new()));
    match layout {
        Ok((_, (_, _, children))) => children.iter().filter_map(|c| entry(c)).collect(),
        Err(e) => {
            log::debug!("tray: {name}{}: menu: {e}", path.as_str());
            vec![]
        }
    }
}

/// A menu's layout node: an entry's id, its properties, its children (nodes, in variants).
type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// A menu entry from its layout node (`(id, properties, children)`, in a variant); none for a separator
/// or a hidden one.
fn entry(node: &Value) -> Option<TrayEntry> {
    let Value::Structure(node) = unwrap(node) else {
        return None;
    };
    let [id, props, children] = node.fields() else {
        return None;
    };
    let Value::I32(id) = unwrap(id) else {
        return None;
    };
    let Value::Dict(props) = unwrap(props) else {
        return None;
    };
    let prop = |key: &str| props.get::<&str, Value>(&key).ok().flatten().and_then(|v| unwrap(&v).try_to_owned().ok());
    let text = |key: &str| prop(key).and_then(|v| String::try_from(v).ok());
    let flag = |key: &str| prop(key).and_then(|v| bool::try_from(v).ok());
    if text("type").as_deref() == Some("separator") || flag("visible") == Some(false) {
        return None;
    }
    let checked = text("toggle-type").filter(|t| !t.is_empty()).map(|_| prop("toggle-state").and_then(|v| i32::try_from(v).ok()) == Some(1));
    let children = match unwrap(children) {
        Value::Array(a) => a.iter().filter_map(entry).collect(),
        _ => vec![],
    };
    Some(TrayEntry { id: *id, label: mnemonic_free(&text("label").unwrap_or_default()), enabled: flag("enabled") != Some(false), checked, children })
}

/// A value out of the variants it is wrapped in.
fn unwrap<'a>(mut v: &'a Value<'a>) -> &'a Value<'a> {
    while let Value::Value(inner) = v {
        v = inner;
    }
    v
}

/// A menu label without its access key marks: `_File` is `File`, `__` an underscore.
fn mnemonic_free(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn carry_out(s: &Session, command: &TrayCommand) -> Result<(), Error> {
    match command {
        TrayCommand::Activate(key) => {
            let (name, path) = split(key);
            let item = zbus::blocking::Proxy::new(&s.conn, name.to_owned(), path.to_owned(), ITEM)?;
            item.call_noreply("Activate", &(0i32, 0i32))?;
        }
        TrayCommand::Click { item, id } => {
            let Some(menu) = s.menus.get(&**item) else {
                return Ok(());
            };
            let (name, _) = split(item);
            let menu = zbus::blocking::Proxy::new(&s.conn, name.to_owned(), menu.clone(), MENU)?;
            menu.call_noreply("Event", &(*id, "clicked", Value::I32(0), 0u32))?;
        }
    }
    Ok(())
}

/// The tray icons there are now, read once (`clamshell --tray`): as the watcher there has them.
pub fn read_now() -> Result<Vec<TrayItem>, Error> {
    let conn = Connection::session()?;
    let watcher = zbus::blocking::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER)?;
    let keys = watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems")?;
    Ok(read(&conn, &keys).0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_labels() {
        assert_eq!(split(":1.5/org/ayatana/NotificationItem/steam"), (":1.5", "/org/ayatana/NotificationItem/steam"));
        assert_eq!(split("org.kde.StatusNotifierItem-12-1"), ("org.kde.StatusNotifierItem-12-1", ITEM_PATH));
        assert_eq!(mnemonic_free("_Exit Steam"), "Exit Steam");
        assert_eq!(mnemonic_free("a__b"), "a_b");
    }

    #[test]
    fn menu_entries_from_their_layout() {
        let props = |label: &str, extra: &[(&str, Value<'static>)]| {
            let mut d: HashMap<String, Value<'static>> = HashMap::from([("label".into(), Value::from(label.to_owned()))]);
            d.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), v.try_clone().unwrap())));
            d
        };
        let node = |id: i32, p: HashMap<String, Value<'static>>, kids: Vec<Value<'static>>| {
            Value::Value(Box::new(Value::from(zbus::zvariant::Structure::from((id, p, kids)))))
        };
        let sep = node(2, props("", &[("type", Value::from("separator"))]), vec![]);
        let tick = node(3, props("_Big Picture", &[("toggle-type", Value::from("checkmark")), ("toggle-state", Value::from(1i32))]), vec![]);
        let off = node(4, props("Hidden", &[("visible", Value::from(false))]), vec![]);
        let sub = node(1, props("Games", &[("children-display", Value::from("submenu"))]), vec![node(5, props("Zelda", &[]), vec![])]);
        let entries: Vec<TrayEntry> = [sub, sep, tick, off].iter().filter_map(entry).collect();
        assert_eq!(entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Games", "Big Picture"]);
        assert_eq!(entries[0].children[0].label, "Zelda");
        assert_eq!(entries[1].checked, Some(true));
    }
}
