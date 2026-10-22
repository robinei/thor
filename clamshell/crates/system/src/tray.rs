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
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clamshell_core::Event;
use clamshell_core::session::tray::{TrayCommand, TrayEntry, TrayItem};

use crate::os::dbus::{self, Bus, Message, PropMap, Props, RefArg, Value, Variant, value};
use crate::os::worker::{self, Bell, Changed, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink, os::cgroup};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const BUS_NAME: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// How long to wait before trying the bus again.
const RETRY: Duration = Duration::from_secs(5);
/// How long after a signal the icons are read: a burst of them (a menu rebuilt) is one read.
const SETTLE: Duration = Duration::from_millis(100);
/// How long a program may take to answer: one that doesn't keeps the others waiting.
const CALL_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum Error {
    Bus(dbus::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

/// Sends [`Event::Tray`] at start and after every change; carries out [`TrayCommand`]s.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<TrayCommand>> {
    Worker::spawn_belled("tray", move |commands, bell| worker::supervise(&mut Tray { sink, bell }, &commands, RETRY))
}

struct Tray<S> {
    sink: S,
    bell: Bell,
}

/// The watcher's state, where we serve it: the icons registered, and what came since the thread looked.
#[derive(Default)]
struct Watched {
    items: Vec<String>,
    /// Registered since: their keys, to be signalled.
    registered: Vec<String>,
}

/// A connection to the session bus: the icons registered (ours, if we are the watcher; else read from
/// the watcher there).
struct Session {
    bus: Bus,
    ours: Option<Arc<Mutex<Watched>>>,
    /// Each icon's menu's object path, by the icon's key (for clicks).
    menus: HashMap<String, String>,
}

impl<S: Sink> Service for Tray<S> {
    type Command = TrayCommand;
    type Session = Session;
    type Error = Error;
    const NAME: &'static str = "tray";

    fn connect(&mut self) -> Result<Session, Error> {
        let mut bus = Bus::session()?.with_timeout(CALL_LIMIT);
        let ours = serve_watcher(&mut bus)?;
        // The icons are read again at any signal of the watcher, an icon, a menu, or a name's owner
        // changing (a program gone).
        for interface in [WATCHER, ITEM, MENU] {
            bus.add_match(&format!("type='signal',interface='{interface}'"))?;
        }
        bus.add_match(&format!("type='signal',interface='{BUS_NAME}',member='NameOwnerChanged'"))?;
        Ok(Session { bus, ours, menus: HashMap::new() })
    }

    fn run_while_up(&mut self, mut s: Session, commands: &Receiver<TrayCommand>) -> Result<(), Ended<Error>> {
        let mut last = Changed::default();
        let mut dirty = true;
        loop {
            dirty |= s.came()?;
            if dirty {
                std::thread::sleep(SETTLE);
                s.came()?;
                dirty = false;
                let keys = registered(&mut s)?;
                let (items, menus) = read(&mut s.bus, &keys);
                s.menus = menus;
                if let Some(items) = last.offer(items) {
                    let names: Vec<String> = items.iter().map(|t| format!("{} ({} entries)", t.title, t.menu.len())).collect();
                    log::info!("tray: {}", if names.is_empty() { "no icons".to_owned() } else { names.join(", ") });
                    self.sink.send(Event::Tray(items))?;
                }
                // (What came while reading: read again.)
                continue;
            }
            loop {
                match commands.try_recv() {
                    Ok(command) => {
                        if let Err(e) = carry_out(&mut s, &command) {
                            log::warn!("tray: {command:?}: {e}");
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            if s.came()? {
                dirty = true;
                continue;
            }
            // Asleep until a command, a signal or a call.
            let mut fds = [
                rustix::event::PollFd::from_borrowed_fd(self.bell.fd(), rustix::event::PollFlags::IN),
                rustix::event::PollFd::from_borrowed_fd(s.bus.fd(), rustix::event::PollFlags::IN),
            ];
            let _ = rustix::event::poll(&mut fds, None);
            self.bell.clear();
        }
    }

    fn command_while_down(&mut self, _: TrayCommand) -> Result<(), Closed> {
        Ok(())
    }
}

impl Session {
    /// Whether anything came that changes the icons: a signal, or an icon registered with us (which is
    /// signalled in turn).
    fn came(&mut self) -> dbus::Result<bool> {
        let signals = self.bus.signals()?;
        let registered = self.ours.as_ref().map(|w| std::mem::take(&mut crate::os::lock(w).registered)).unwrap_or_default();
        for key in &registered {
            if let Ok(signal) = Message::new_signal(WATCHER_PATH, WATCHER, "StatusNotifierItemRegistered") {
                let _ = self.bus.send(signal.append1(key.as_str()));
            }
        }
        Ok(!signals.is_empty() || !registered.is_empty())
    }
}

/// The watcher's interface, as introspection gives it.
const WATCHER_XML: &str = r#"<node><interface name="org.kde.StatusNotifierWatcher">
<method name="RegisterStatusNotifierItem"><arg name="service" type="s" direction="in"/></method>
<method name="RegisterStatusNotifierHost"><arg name="service" type="s" direction="in"/></method>
<property name="RegisteredStatusNotifierItems" type="as" access="read"/>
<property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
<property name="ProtocolVersion" type="i" access="read"/>
<signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
<signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
<signal name="StatusNotifierHostRegistered"/>
</interface></node>"#;

/// Serves the watcher, if no one else does: the icons registered with it; none if another has the name.
fn serve_watcher(bus: &mut Bus) -> Result<Option<Arc<Mutex<Watched>>>, Error> {
    // DBUS_NAME_FLAG_DO_NOT_QUEUE; replies: 1 the primary owner, 4 already the owner.
    let (reply,): (u32,) = bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "RequestName", (WATCHER, 4u32))?;
    if !matches!(reply, 1 | 4) {
        log::info!("tray: reading the watcher there");
        return Ok(None);
    }
    log::info!("tray: the watcher is ours");
    let watched = Arc::new(Mutex::new(Watched::default()));
    let state = watched.clone();
    bus.serve(Box::new(move |call| answer_watcher(&state, call)));
    // (Programs that started before us wait for a host to show their icons.)
    if let Ok(signal) = Message::new_signal(WATCHER_PATH, WATCHER, "StatusNotifierHostRegistered") {
        bus.send(signal)?;
    }
    Ok(Some(watched))
}

/// The watcher's reply to a call, if the call is its.
fn answer_watcher(watched: &Mutex<Watched>, call: &Message) -> Option<Message> {
    if call.path().as_deref() != Some(WATCHER_PATH) {
        return None;
    }
    let mut w = crate::os::lock(watched);
    let property = |w: &Watched, name: &str| -> Option<Value> {
        match name {
            "RegisteredStatusNotifierItems" => Some(value(w.items.clone())),
            "IsStatusNotifierHostRegistered" => Some(value(true)),
            "ProtocolVersion" => Some(value(0i32)),
            _ => None,
        }
    };
    match (call.interface().as_deref().unwrap_or(WATCHER), &*call.member()?) {
        // An icon registered: by its D-Bus name (its object at the usual path), or by its object path
        // (the sender's). Kept as KDE's watcher has it: the name, then the path.
        (WATCHER, "RegisterStatusNotifierItem") => {
            let service: String = call.read1().ok()?;
            let sender = call.sender().map(|s| s.to_string()).unwrap_or_default();
            let key = if service.starts_with('/') { format!("{sender}{service}") } else { format!("{service}{ITEM_PATH}") };
            if !w.items.contains(&key) {
                w.items.push(key.clone());
            }
            w.registered.push(key);
            Some(call.method_return())
        }
        // A host (a panel) registered: we are the only one shown.
        (WATCHER, "RegisterStatusNotifierHost") => Some(call.method_return()),
        ("org.freedesktop.DBus.Properties", "Get") => {
            let (_, name): (String, String) = call.read2().ok()?;
            Some(call.return_with_args((property(&w, &name)?,)))
        }
        ("org.freedesktop.DBus.Properties", "GetAll") => {
            let names = ["RegisteredStatusNotifierItems", "IsStatusNotifierHostRegistered", "ProtocolVersion"];
            let all: PropMap = names.iter().filter_map(|&n| Some((n.to_owned(), property(&w, n)?))).collect();
            Some(call.return_with_args((all,)))
        }
        ("org.freedesktop.DBus.Introspectable", "Introspect") => Some(call.return_with_args((WATCHER_XML,))),
        ("org.freedesktop.DBus.Peer", "Ping") => Some(call.method_return()),
        _ => None,
    }
}

/// The icons registered: ours (those whose program is gone dropped), or the watcher's there.
fn registered(s: &mut Session) -> dbus::Result<Vec<String>> {
    let Some(ours) = &s.ours else {
        return Ok(s.bus.get::<Vec<String>>(WATCHER, WATCHER_PATH, WATCHER, "RegisteredStatusNotifierItems").unwrap_or_default());
    };
    let items = crate::os::lock(ours).items.clone();
    let mut kept = Vec::with_capacity(items.len());
    for key in items {
        let owned: dbus::Result<(bool,)> = s.bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "NameHasOwner", (split(&key).0,));
        if owned.is_ok_and(|(o,)| o) {
            kept.push(key);
        }
    }
    crate::os::lock(ours).items.retain(|k| kept.contains(k));
    Ok(kept)
}

/// An icon's key as the name and the object path.
fn split(key: &str) -> (&str, &str) {
    match key.find('/') {
        Some(i) => key.split_at(i),
        None => (key, ITEM_PATH),
    }
}

/// Every icon (those that can't be read left out), and their menus' paths.
fn read(bus: &mut Bus, keys: &[String]) -> (Vec<TrayItem>, HashMap<String, String>) {
    let dirs = crate::desktop::data_dirs();
    let mut menus = HashMap::new();
    let items = keys
        .iter()
        .filter_map(|key| match read_item(bus, key, &dirs) {
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

fn read_item(bus: &mut Bus, key: &str, dirs: &[PathBuf]) -> Result<(TrayItem, Option<String>), Error> {
    let (name, path) = split(key);
    let all = bus.get_all(name, path, ITEM)?;
    let props = Props(&all);
    let text = |k: &str| props.str(k).unwrap_or_default();
    let pid = bus.call::<(u32,)>(BUS_NAME, BUS_PATH, BUS_NAME, "GetConnectionUnixProcessID", (name,)).ok().map(|(pid,)| pid);
    // (Remembered as it is now: what is done to it later by its id is checked to be it still.)
    if let Some(pid) = pid {
        crate::os::sys::saw(pid);
    }
    let menu = props.path("Menu").filter(|p| p != "/");
    let entries = menu.as_ref().map(|m| read_menu(bus, name, m)).unwrap_or_default();
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
fn read_menu(bus: &mut Bus, name: &str, path: &str) -> Vec<TrayEntry> {
    let _ = bus.call::<(bool,)>(name, path, MENU, "AboutToShow", (0i32,));
    let layout = bus.call::<(u32, Node)>(name, path, MENU, "GetLayout", (0i32, -1i32, Vec::<String>::new()));
    match layout {
        Ok((_, (_, _, children))) => children.iter().filter_map(|c| entry(c)).collect(),
        Err(e) => {
            log::debug!("tray: {name}{path}: menu: {e}");
            vec![]
        }
    }
}

/// A menu's layout node: an entry's id, its properties, its children (nodes, in variants).
type Node = (i32, PropMap, Vec<Value>);

/// A menu entry from its layout node (`(id, properties, children)`, in a variant); none for a separator
/// or a hidden one.
fn entry(node: &dyn RefArg) -> Option<TrayEntry> {
    let mut fields = unwrap(node)?.as_iter()?;
    let (id, props, children) = (fields.next()?, fields.next()?, fields.next()?);
    let id = i32::try_from(unwrap(id)?.as_i64()?).ok()?;
    // (A dictionary iterates as its keys and values in turn.)
    let mut props_iter = unwrap(props)?.as_iter()?;
    let mut map: HashMap<&str, &dyn RefArg> = HashMap::new();
    while let (Some(k), Some(v)) = (props_iter.next(), props_iter.next()) {
        if let (Some(k), Some(v)) = (k.as_str(), unwrap(v)) {
            map.insert(k, v);
        }
    }
    let text = |key: &str| map.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let number = |key: &str| map.get(key).and_then(|v| v.as_i64());
    let flag = |key: &str| number(key).map(|n| n != 0);
    if text("type").as_deref() == Some("separator") || flag("visible") == Some(false) {
        return None;
    }
    let checked = text("toggle-type").filter(|t| !t.is_empty()).map(|_| number("toggle-state") == Some(1));
    let children = unwrap(children).and_then(|c| c.as_iter()).map(|c| c.filter_map(entry).collect()).unwrap_or_default();
    Some(TrayEntry { id, label: mnemonic_free(&text("label").unwrap_or_default()), enabled: flag("enabled") != Some(false), checked, children })
}

/// A value out of the variants it is wrapped in.
fn unwrap(mut v: &dyn RefArg) -> Option<&dyn RefArg> {
    while v.arg_type() == ::dbus::arg::ArgType::Variant {
        v = v.as_iter()?.next()?;
    }
    Some(v)
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

fn carry_out(s: &mut Session, command: &TrayCommand) -> Result<(), Error> {
    match command {
        TrayCommand::Activate(key) => {
            let (name, path) = split(key);
            s.bus.call_noreply(name, path, ITEM, "Activate", (0i32, 0i32))?;
        }
        TrayCommand::Click { item, id } => {
            let Some(menu) = s.menus.get(&**item) else {
                return Ok(());
            };
            let (name, _) = split(item);
            s.bus.call_noreply(name, menu, MENU, "Event", (*id, "clicked", Variant(0i32), 0u32))?;
        }
    }
    Ok(())
}

/// The tray icons there are now, read once (`clamshell --tray`): as the watcher there has them.
pub fn read_now() -> Result<Vec<TrayItem>, Error> {
    let mut bus = Bus::session()?.with_timeout(CALL_LIMIT);
    let keys = bus.get::<Vec<String>>(WATCHER, WATCHER_PATH, WATCHER, "RegisteredStatusNotifierItems")?;
    Ok(read(&mut bus, &keys).0)
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
        type Props = Vec<(&'static str, Value)>;
        let node = |id: i32, label: &str, extra: Props, kids: Vec<Value>| {
            let mut props: PropMap = PropMap::from([("label".to_owned(), value(label.to_owned()))]);
            props.extend(extra.into_iter().map(|(k, v)| (k.to_owned(), v)));
            value((id, props, kids))
        };
        let sep = node(2, "", vec![("type", value("separator".to_owned()))], vec![]);
        let tick = node(3, "_Big Picture", vec![("toggle-type", value("checkmark".to_owned())), ("toggle-state", value(1i32))], vec![]);
        let off = node(4, "Hidden", vec![("visible", value(false))], vec![]);
        let sub = node(1, "Games", vec![("children-display", value("submenu".to_owned()))], vec![node(5, "Zelda", vec![], vec![])]);
        // (Through a message, as a program's layout comes.)
        let message = Message::new_signal("/", "test.Menu", "Layout").unwrap().append1(vec![sub, sep, tick, off]);
        let nodes: Vec<Value> = message.read1().unwrap();
        let entries: Vec<TrayEntry> = nodes.iter().filter_map(|n| entry(n)).collect();
        assert_eq!(entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Games", "Big Picture"]);
        assert_eq!(entries.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Games", "Big Picture"]);
        assert_eq!(entries[0].children[0].label, "Zelda");
        assert_eq!(entries[1].checked, Some(true));
    }
}
