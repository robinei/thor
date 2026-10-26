//! The session's notification server: ClamShell owns `org.freedesktop.Notifications` on the session bus
//! (the Desktop Notifications Specification, version 1.2) and shows the notifications itself, over the
//! bottom screen (clamshell-state's `notifications`).
//!
//! A `Notify` call is answered at once with the notification's id and sent on as
//! [`NotificationEvent::Notified`]: plain text, the urgency, the default action and the timeout as asked.
//! `CloseNotification` is sent on as [`NotificationEvent::Closed`]. The state's closes and taps come back
//! as commands, emitted as the `NotificationClosed` and `ActionInvoked` signals. The server offers `body`
//! and `actions`: no markup, which is taken out of what comes anyway, no images and no sounds.
//!
//! If another server has the name, as on a desktop around a nested ClamShell, that is a warning, and the
//! name is asked for again every [`RETRY`]; meanwhile programs' notifications go to that server.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use clamshell_state::{Event, Notification, NotificationEvent, NotificationId, Urgency};

use crate::os::dbus::{self, Bus, Message, PropMap};
use crate::os::worker::{self, Bell, Ended, Service, Worker, lost_on};
use crate::{Closed, Sink};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const BUS_NAME: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// How long to wait before trying the bus, or the name, again.
const RETRY: Duration = Duration::from_secs(5);

/// The last id given out. It outlives the thread, so that one started again after a panic doesn't give
/// out an id the state still holds.
static LAST_ID: AtomicU32 = AtomicU32::new(0);

/// What the state asks: a signal to emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// `NotificationClosed`, with the specification's reason.
    Closed { id: u32, reason: u32 },
    /// `ActionInvoked`, with the action's key.
    Invoked { id: u32, key: Arc<str> },
}

#[derive(Debug)]
pub(crate) enum Error {
    Bus(dbus::Error),
    /// Another notification server has the name.
    Taken,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "{e}"),
            Error::Taken => write!(f, "another notification server has {NAME}; programs' notifications go there until it is ours"),
        }
    }
}

impl From<dbus::Error> for Error {
    fn from(e: dbus::Error) -> Self {
        Error::Bus(e)
    }
}

lost_on!(Error: dbus::Error);

/// Serves the notifications, sending what comes to `sink`; emits the signals the commands ask for.
pub(crate) fn spawn(sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    let bell = Bell::new()?;
    let mut server = Server { sink, bell: bell.clone() };
    Worker::spawn_belled(clamshell_state::Thread::Notifications.name(), bell, move |commands| worker::supervise(&mut server, commands, RETRY))
}

struct Server<S> {
    sink: S,
    bell: Bell,
}

impl<S: Sink + Clone> Service for Server<S> {
    type Command = Command;
    type Session = Bus;
    type Error = Error;
    const NAME: &'static str = "notifications";

    fn connect(&mut self) -> Result<Bus, Error> {
        let mut bus = Bus::session()?;
        // Flag 4 is DBUS_NAME_FLAG_DO_NOT_QUEUE. Reply 1 means we are the primary owner, 4 that we already were.
        let (reply,): (u32,) = bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "RequestName", (NAME, 4u32))?;
        if !matches!(reply, 1 | 4) {
            return Err(Error::Taken);
        }
        log::info!("notifications: {NAME} is ours");
        let sink = self.sink.clone();
        bus.serve(Box::new(move |call| answer(call, &sink)));
        Ok(bus)
    }

    fn run_while_up(&mut self, mut bus: Bus, commands: &Receiver<Command>) -> Result<(), Ended<Error>> {
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => emit(&mut bus, &command)?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // Answers the calls that came.
            bus.signals()?;
            // Sleep until a command or a call comes.
            let mut fds = [
                rustix::event::PollFd::from_borrowed_fd(self.bell.fd(), rustix::event::PollFlags::IN),
                rustix::event::PollFd::from_borrowed_fd(bus.fd(), rustix::event::PollFlags::IN),
            ];
            if let Err(e) = rustix::event::poll(&mut fds, None) {
                log::debug!("notifications: waiting: {e}");
            }
            self.bell.clear();
        }
    }

    /// A signal can't be emitted without the bus, and the program that would have heard it is gone with
    /// it, or talks to another server.
    fn command_while_down(&mut self, command: Command) -> Result<(), Closed> {
        log::debug!("notifications: not emitted, the bus or the name isn't ours: {command:?}");
        Ok(())
    }
}

/// Emits the signal `command` asks for.
fn emit(bus: &mut Bus, command: &Command) -> dbus::Result<()> {
    let signal = match command {
        Command::Closed { id, reason } => Message::new_signal(PATH, INTERFACE, "NotificationClosed").map(|m| m.append2(*id, *reason)),
        Command::Invoked { id, key } => Message::new_signal(PATH, INTERFACE, "ActionInvoked").map(|m| m.append2(*id, &**key)),
    };
    bus.send(signal.map_err(dbus::Error::Reply)?)
}

/// The interface, as introspection gives it.
const XML: &str = r#"<node><interface name="org.freedesktop.Notifications">
<method name="Notify"><arg name="app_name" type="s" direction="in"/><arg name="replaces_id" type="u" direction="in"/><arg name="app_icon" type="s" direction="in"/><arg name="summary" type="s" direction="in"/><arg name="body" type="s" direction="in"/><arg name="actions" type="as" direction="in"/><arg name="hints" type="a{sv}" direction="in"/><arg name="expire_timeout" type="i" direction="in"/><arg name="id" type="u" direction="out"/></method>
<method name="CloseNotification"><arg name="id" type="u" direction="in"/></method>
<method name="GetCapabilities"><arg name="capabilities" type="as" direction="out"/></method>
<method name="GetServerInformation"><arg name="name" type="s" direction="out"/><arg name="vendor" type="s" direction="out"/><arg name="version" type="s" direction="out"/><arg name="spec_version" type="s" direction="out"/></method>
<signal name="NotificationClosed"><arg name="id" type="u"/><arg name="reason" type="u"/></signal>
<signal name="ActionInvoked"><arg name="id" type="u"/><arg name="action_key" type="s"/></signal>
</interface></node>"#;

/// A `Notify` call's arguments: the program's name, the id it replaces, its icon, the summary, the body,
/// the actions as keys and labels in turn, the hints and the timeout in milliseconds.
type NotifyArgs = (String, u32, String, String, String, Vec<String>, PropMap, i32);

/// The reply to a call on the server's path, or none for a call that isn't the server's.
fn answer(call: &Message, sink: &impl Sink) -> Option<Message> {
    if call.path().as_deref() != Some(PATH) {
        return None;
    }
    match (call.interface().as_deref().unwrap_or(INTERFACE), &*call.member()?) {
        (INTERFACE, "Notify") => {
            let (app, replaces, _icon, summary, body, actions, hints, timeout): NotifyArgs = match call.read_all() {
                Ok(args) => args,
                Err(e) => {
                    log::warn!("notifications: a Notify call that can't be read: {e}");
                    return Some(call.error(&"org.freedesktop.DBus.Error.InvalidArgs".into(), c"not a Notify call"));
                }
            };
            let id = next_id(&LAST_ID, replaces);
            let notification = notification(id, &app, &summary, &body, &actions, &hints, timeout);
            log::info!("notifications: {id} from {app:?}, {:?}", notification.urgency);
            if sink.send(NotificationEvent::Notified(Box::new(notification)).into()).is_err() {
                log::debug!("notifications: {id} not shown, the UI is gone");
            }
            Some(call.return_with_args((id,)))
        }
        (INTERFACE, "CloseNotification") => {
            let id: u32 = call.read1().inspect_err(|e| log::warn!("notifications: a CloseNotification call that can't be read: {e}")).ok()?;
            if sink.send(Event::Notification(NotificationEvent::Closed(id))).is_err() {
                log::debug!("notifications: {id} not closed, the UI is gone");
            }
            Some(call.method_return())
        }
        (INTERFACE, "GetCapabilities") => Some(call.return_with_args((vec!["body", "actions"],))),
        (INTERFACE, "GetServerInformation") => Some(call.return_with_args(("ClamShell", "thor", env!("CARGO_PKG_VERSION"), "1.2"))),
        ("org.freedesktop.DBus.Introspectable", "Introspect") => Some(call.return_with_args((XML,))),
        ("org.freedesktop.DBus.Peer", "Ping") => Some(call.method_return()),
        _ => None,
    }
}

/// The id for a notification that replaces `replaces`: that id if it is one given out, else the next.
/// Ids start at 1 and skip 0, which means none.
fn next_id(last: &AtomicU32, replaces: u32) -> u32 {
    if replaces != 0 && replaces <= last.load(Ordering::Relaxed) {
        return replaces;
    }
    let id = last.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if id == 0 { last.fetch_add(1, Ordering::Relaxed).wrapping_add(1) } else { id }
}

/// The notification a `Notify` call asks for, with `id`: its text without markup, its urgency, its
/// default action and its timeout.
fn notification(id: u32, app: &str, summary: &str, body: &str, actions: &[String], hints: &PropMap, timeout_ms: i32) -> Notification {
    let level = hints.get("urgency").and_then(|v| v.0.as_u64().or_else(|| v.0.as_i64().and_then(|i| u64::try_from(i).ok())));
    let urgency = match level {
        Some(0) => Urgency::Low,
        Some(2) => Urgency::Critical,
        _ => Urgency::Normal,
    };
    // Keys and labels in turn; the default action is the one with the key "default".
    let action = actions.chunks(2).filter_map(|pair| pair.first()).find(|key| *key == "default").map(|key| Arc::from(key.as_str()));
    Notification { id: NotificationId::Bus(id), app: plain(app), summary: plain(summary), body: plain(body), urgency, action, timeout_ms }
}

/// `text` without markup: tags taken out and the five entities, and numbered ones, put back as the
/// characters they stand for. Many programs send markup though the server doesn't offer it.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(rest.get(..i).unwrap_or(""));
        rest = rest.get(i..).unwrap_or("");
        if rest.starts_with('<') {
            // A tag, to its end; a lone `<` stays as it is.
            match rest.find('>') {
                Some(end) => rest = rest.get(end + 1..).unwrap_or(""),
                None => {
                    out.push_str(rest);
                    rest = "";
                }
            }
            continue;
        }
        let entity = rest.find(';').and_then(|end| Some((entity(rest.get(1..end)?)?, end)));
        match entity {
            Some((c, end)) => {
                out.push(c);
                rest = rest.get(end + 1..).unwrap_or("");
            }
            None => {
                out.push('&');
                rest = rest.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character entity `name` (between `&` and `;`) stands for.
fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::os::dbus::value;

    fn hints(urgency: Option<u8>) -> PropMap {
        urgency.map(|u| PropMap::from([("urgency".to_owned(), value(u))])).unwrap_or_default()
    }

    #[test]
    fn a_notify_call_becomes_a_plain_notification() {
        let actions = ["open".to_owned(), "Open".to_owned(), "default".to_owned(), "Show".to_owned()];
        let n = notification(7, "Mail", "<b>New</b> mail", "From Ann &amp; Bob", &actions, &hints(Some(1)), -1);
        assert_eq!(n.id, NotificationId::Bus(7));
        assert_eq!((n.app.as_str(), n.summary.as_str(), n.body.as_str()), ("Mail", "New mail", "From Ann & Bob"));
        assert_eq!(n.urgency, Urgency::Normal);
        assert_eq!(n.action.as_deref(), Some("default"));
        assert_eq!(n.timeout_ms, -1);
    }

    #[test]
    fn the_urgency_hint_and_the_default_action() {
        let urgency = |u| notification(1, "", "", "", &[], &hints(u), 0).urgency;
        assert_eq!(urgency(Some(0)), Urgency::Low);
        assert_eq!(urgency(Some(2)), Urgency::Critical);
        assert_eq!(urgency(None), Urgency::Normal);
        assert_eq!(urgency(Some(9)), Urgency::Normal, "unknown");
        let action = |actions: &[&str]| notification(1, "", "", "", &actions.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(), &hints(None), 0).action;
        assert_eq!(action(&["open", "Open"]), None, "no default action");
        assert_eq!(action(&["Open", "default"]), None, "a label is not a key");
        assert_eq!(action(&["default"]), Some("default".into()), "a key without its label");
        let int = PropMap::from([("urgency".to_owned(), value(2i32))]);
        assert_eq!(notification(1, "", "", "", &[], &int, 0).urgency, Urgency::Critical, "sent as another integer type");
    }

    #[test]
    fn markup_is_taken_out() {
        assert_eq!(plain("<i>a</i> <a href=\"x\">link</a>"), "a link");
        assert_eq!(plain("1 &lt; 2 &gt; 0 &quot;q&quot; &apos;s&apos; &#65;&#x42;"), "1 < 2 > 0 \"q\" 's' AB");
        assert_eq!(plain("a & b; c &bogus; d < e"), "a & b; c &bogus; d < e", "what isn't markup stays");
        assert_eq!(plain("line\nnext"), "line\nnext");
    }

    #[test]
    fn ids_are_new_unless_one_given_out_is_replaced() {
        let last = AtomicU32::new(0);
        assert_eq!(next_id(&last, 0), 1);
        assert_eq!(next_id(&last, 0), 2);
        assert_eq!(next_id(&last, 1), 1, "replaces one given out");
        assert_eq!(next_id(&last, 50), 3, "one never given out gets a new id");
        let last = AtomicU32::new(u32::MAX);
        assert_eq!(next_id(&last, 0), 1, "0 is skipped as it wraps");
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Needs a session bus, so a desktop. A Notify call through the bus is answered with an id and sent
    /// on as a notification; it is served on the connection's own name, since a desktop's server has the
    /// well-known one. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a session bus"]
    #[allow(clippy::unwrap_used, clippy::panic)]
    fn a_notify_call_is_answered_and_sent_on() {
        let mut server = Bus::session().unwrap();
        let name = server.unique_name().unwrap();
        let (sink, events) = std::sync::mpsc::channel();
        server.serve(Box::new(move |call| answer(call, &sink)));
        let caller = std::thread::spawn(move || {
            let mut client = Bus::session().unwrap();
            let hints = PropMap::from([("urgency".to_owned(), dbus::value(2u8))]);
            let args = ("mail", 0u32, "", "New", "<b>hi</b>", vec!["default", "Show"], hints, 3000i32);
            let (id,): (u32,) = client.call(&name, PATH, INTERFACE, "Notify", args).unwrap();
            let (caps,): (Vec<String>,) = client.call(&name, PATH, INTERFACE, "GetCapabilities", ()).unwrap();
            client.call::<()>(&name, PATH, INTERFACE, "CloseNotification", (id,)).unwrap();
            (id, caps)
        });
        while !caller.is_finished() {
            server.wait(Some(Duration::from_millis(100))).unwrap();
        }
        let (id, caps) = caller.join().unwrap();
        assert_eq!(caps, ["body", "actions"]);
        let Ok(Event::Notification(NotificationEvent::Notified(n))) = events.recv() else { panic!("no notification") };
        assert_eq!(
            (n.id, n.body.as_str(), n.urgency, n.action.as_deref(), n.timeout_ms),
            (NotificationId::Bus(id), "hi", Urgency::Critical, Some("default"), 3000)
        );
        assert_eq!(events.recv().ok(), Some(Event::Notification(NotificationEvent::Closed(id))));
    }
}
