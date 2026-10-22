//! D-Bus, through libdbus (the dbus crate's channel, without its dispatchers or async): a connection per
//! thread ([`Bus`]). A method call is sent and its reply waited for here; what comes meanwhile is handled
//! as it comes: a call to us is answered by what the thread serves ([`Bus::serve`]; anything else is
//! refused), a signal is kept for the thread to read ([`Bus::signals`]) if it asked for signals
//! ([`Bus::add_match`]). A thread waits for the bus with [`Bus::wait`], or polls its file descriptor
//! ([`Bus::fd`]) beside others.

use std::collections::VecDeque;
use std::fmt;
use std::os::fd::BorrowedFd;
use std::time::{Duration, Instant};

use ::dbus::arg::{AppendAll, Arg, Get, ReadAll};
pub use ::dbus::arg::{PropMap, RefArg, Variant};
use ::dbus::channel::{BusType, Channel};
pub use ::dbus::message::MessageType;
pub use ::dbus::{Message, Path};

/// How long a call may take unless the bus is given another limit (libdbus' own default).
const TIMEOUT: Duration = Duration::from_secs(25);
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bus itself: not reached, closed, or no reply in time. A kept connection is made again.
    Bus(String),
    /// The service's answer: an error it replied with (its name and message).
    Method { name: String, message: String },
    /// A reply that isn't what was asked for.
    Reply(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) | Error::Reply(e) => write!(f, "{e}"),
            Error::Method { name, message } if message.is_empty() => write!(f, "{name}"),
            Error::Method { name, message } => write!(f, "{name}: {message}"),
        }
    }
}

impl From<::dbus::Error> for Error {
    fn from(e: ::dbus::Error) -> Self {
        Error::Bus(e.message().unwrap_or("D-Bus failed").to_owned())
    }
}

impl From<::dbus::arg::TypeMismatchError> for Error {
    fn from(e: ::dbus::arg::TypeMismatchError) -> Self {
        Error::Reply(e.to_string())
    }
}

impl Error {
    /// Whether the bus failed (rather than the service answering with an error), after which a kept
    /// connection is made again.
    pub fn is_bus_failure(&self) -> bool {
        matches!(self, Error::Bus(_))
    }

    /// Whether the service isn't on the bus at all (not installed, or not running).
    pub fn is_absent(&self) -> bool {
        matches!(self, Error::Method { name, .. } if matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A value of any type, as property maps and variant arguments hold them.
pub type Value = Variant<Box<dyn RefArg>>;

/// What a thread serves: given a call to it, the reply; `None` if it isn't one of its (refused).
type Serve = Box<dyn FnMut(&Message) -> Option<Message> + Send>;

/// A connection to a bus, used by one thread.
pub struct Bus {
    channel: Channel,
    timeout: Duration,
    serve: Option<Serve>,
    /// The signals come, if any were asked for.
    signals: Option<VecDeque<Message>>,
    closed: bool,
}

impl Bus {
    pub fn system() -> Result<Bus> {
        Bus::open(BusType::System)
    }

    pub fn session() -> Result<Bus> {
        Bus::open(BusType::Session)
    }

    fn open(kind: BusType) -> Result<Bus> {
        let mut channel = Channel::get_private(kind)?;
        channel.set_watch_enabled(true);
        Ok(Bus { channel, timeout: TIMEOUT, serve: None, signals: None, closed: false })
    }

    /// Calls fail after `timeout` (else [`TIMEOUT`]).
    pub fn with_timeout(mut self, timeout: Duration) -> Bus {
        self.timeout = timeout;
        self
    }

    /// Calls to this connection answered by `serve` from now on (refused before).
    pub fn serve(&mut self, serve: Serve) {
        self.serve = Some(serve);
    }

    pub fn unique_name(&self) -> Option<String> {
        self.channel.unique_name().map(str::to_owned)
    }

    /// The connection's socket, to poll for reading beside other files; then [`Bus::signals`] (which
    /// also answers calls to us).
    pub fn fd(&self) -> BorrowedFd<'_> {
        // SAFETY: the descriptor is the channel's socket, open as long as the channel is; the borrow
        // can't outlive `self`, which owns the channel.
        #[allow(unsafe_code)]
        unsafe {
            BorrowedFd::borrow_raw(self.channel.watch().fd)
        }
    }

    /// `method` of `interface` on `path` at `dest`, with `args`; its reply's values.
    pub fn call<R: ReadAll>(&mut self, dest: &str, path: &str, interface: &str, method: &str, args: impl AppendAll) -> Result<R> {
        let mut message = Message::new_method_call(dest, path, interface, method).map_err(Error::Reply)?;
        message.append_all(args);
        let reply = self.call_message(message)?;
        Ok(reply.read_all::<R>()?)
    }

    /// As [`Bus::call`], not waiting for (nor asking for) a reply.
    pub fn call_noreply(&mut self, dest: &str, path: &str, interface: &str, method: &str, args: impl AppendAll) -> Result<()> {
        let mut message = Message::new_method_call(dest, path, interface, method).map_err(Error::Reply)?;
        message.append_all(args);
        message.set_no_reply(true);
        self.send(message)
    }

    /// A property, read fresh.
    pub fn get<T: Arg + for<'z> Get<'z>>(&mut self, dest: &str, path: &str, interface: &str, name: &str) -> Result<T> {
        let (Variant(value),): (Variant<T>,) = self.call(dest, path, PROPERTIES, "Get", (interface, name))?;
        Ok(value)
    }

    pub fn set<T: Arg + ::dbus::arg::Append>(&mut self, dest: &str, path: &str, interface: &str, name: &str, value: T) -> Result<()> {
        self.call(dest, path, PROPERTIES, "Set", (interface, name, Variant(value)))
    }

    /// All of an object's properties of `interface`.
    pub fn get_all(&mut self, dest: &str, path: &str, interface: &str) -> Result<PropMap> {
        let (all,): (PropMap,) = self.call(dest, path, PROPERTIES, "GetAll", (interface,))?;
        Ok(all)
    }

    /// The signals `rule` matches come to this connection, kept for [`Bus::signals`].
    pub fn add_match(&mut self, rule: &str) -> Result<()> {
        self.signals.get_or_insert_default();
        self.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch", (rule,))
    }

    /// Sends a message (a signal, a reply).
    pub fn send(&mut self, message: Message) -> Result<()> {
        self.channel.send(message).map_err(|()| Error::Bus("couldn't send".into()))?;
        self.channel.flush();
        Ok(())
    }

    /// What came since the last look, at once: calls to us answered, the signals kept since returned.
    /// Fails once the bus has closed.
    pub fn signals(&mut self) -> Result<Vec<Message>> {
        self.read(Some(Duration::ZERO))?;
        Ok(self.signals.as_mut().map(|s| s.drain(..).collect()).unwrap_or_default())
    }

    /// Waits until something comes (for at most `timeout`; none: until it does), and handles it; then
    /// [`Bus::signals`] has the signals.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<()> {
        if self.signals.as_ref().is_some_and(|s| !s.is_empty()) {
            return Ok(());
        }
        self.read(timeout)
    }

    /// Reads from the socket (waiting up to `timeout`), and handles every message there.
    fn read(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.ensure_open()?;
        if self.channel.read_write(timeout).is_err() {
            self.closed = true;
        }
        while let Some(message) = self.channel.pop_message() {
            self.handle(message);
        }
        self.ensure_open()
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed { Err(Error::Bus("the bus closed".into())) } else { Ok(()) }
    }

    /// Sends `message` and waits for its reply, handling what comes meanwhile.
    fn call_message(&mut self, message: Message) -> Result<Message> {
        self.ensure_open()?;
        let serial = self.channel.send(message).map_err(|()| Error::Bus("couldn't send".into()))?;
        let until = Instant::now() + self.timeout;
        loop {
            while let Some(mut message) = self.channel.pop_message() {
                if message.get_reply_serial() == Some(serial) && matches!(message.msg_type(), MessageType::MethodReturn | MessageType::Error) {
                    return match message.as_result() {
                        Ok(_) => Ok(message),
                        Err(e) => Err(Error::Method { name: e.name().unwrap_or_default().to_owned(), message: e.message().unwrap_or_default().to_owned() }),
                    };
                }
                self.handle(message);
            }
            self.ensure_open()?;
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Bus("no reply in time".into()));
            }
            if self.channel.read_write(Some(left)).is_err() {
                self.closed = true;
            }
        }
    }

    /// A message that isn't a reply waited for: a call to us answered (or refused), a signal kept (if
    /// signals were asked for), the bus closing noted.
    fn handle(&mut self, message: Message) {
        match message.msg_type() {
            MessageType::MethodCall => {
                let reply = self.serve.as_mut().and_then(|serve| serve(&message));
                if message.get_no_reply() {
                    return;
                }
                let reply = reply.unwrap_or_else(|| {
                    let name = ::dbus::strings::ErrorName::from("org.freedesktop.DBus.Error.UnknownMethod");
                    message.error(&name, c"not served here")
                });
                let _ = self.channel.send(reply);
                self.channel.flush();
            }
            MessageType::Signal => {
                if message.interface().as_deref() == Some("org.freedesktop.DBus.Local") && message.member().as_deref() == Some("Disconnected") {
                    self.closed = true;
                } else if let Some(signals) = &mut self.signals {
                    signals.push_back(message);
                }
            }
            MessageType::MethodReturn | MessageType::Error => {}
        }
    }
}

/// A system-bus connection kept between calls (connected when first needed), and made again after the
/// bus failed.
#[derive(Default)]
pub struct Kept {
    bus: Option<Bus>,
}

impl Kept {
    pub fn get(&mut self) -> Result<&mut Bus> {
        let bus = match self.bus.take() {
            Some(bus) => bus,
            None => Bus::system()?,
        };
        Ok(self.bus.insert(bus))
    }

    /// A call through it failed: if the bus did, the connection goes and the next call connects again.
    pub fn failed(&mut self, error: &Error) {
        if error.is_bus_failure() {
            self.bus = None;
        }
    }
}

/// An object's properties, as GetAll, GetManagedObjects or GetSettings give them.
#[derive(Debug, Clone, Copy)]
pub struct Props<'a>(pub &'a PropMap);

impl Props<'_> {
    pub fn str(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(|v| v.0.as_str()).map(str::to_owned)
    }

    /// A boolean property; false when missing.
    pub fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(|v| ::dbus::arg::cast::<bool>(&v.0)).copied().unwrap_or(false)
    }

    pub fn bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.0.get(key).and_then(|v| ::dbus::arg::cast::<Vec<u8>>(&v.0)).cloned()
    }

    pub fn path(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(|v| ::dbus::arg::cast::<Path<'static>>(&v.0)).map(|p| p.to_string())
    }
}

/// A value for a property map or a variant argument.
pub fn value<T: RefArg + 'static>(value: T) -> Value {
    Variant(Box::new(value))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn properties_by_type() {
        let map: PropMap = PropMap::from([
            ("Name".to_owned(), value("Pad".to_owned())),
            ("Paired".to_owned(), value(true)),
            ("Ssid".to_owned(), value(b"home".to_vec())),
            ("Menu".to_owned(), value(Path::from("/Menu"))),
        ]);
        let p = Props(&map);
        assert_eq!(p.str("Name").as_deref(), Some("Pad"));
        assert!(p.flag("Paired") && !p.flag("Connected"));
        assert_eq!(p.bytes("Ssid").as_deref(), Some(&b"home"[..]));
        assert_eq!(p.path("Menu").as_deref(), Some("/Menu"));
        assert_eq!(p.str("Paired"), None, "the wrong type");
    }

    #[test]
    fn method_errors_keep_the_connection() {
        assert!(Error::Bus("closed".into()).is_bus_failure());
        let refused = Error::Method { name: "org.freedesktop.DBus.Error.AccessDenied".into(), message: "no".into() };
        assert!(!refused.is_bus_failure() && !refused.is_absent());
        assert!(Error::Method { name: "org.freedesktop.DBus.Error.ServiceUnknown".into(), message: String::new() }.is_absent());
        assert_eq!(refused.to_string(), "org.freedesktop.DBus.Error.AccessDenied: no");
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// With a session bus (a desktop): a call to ourselves is answered by what we serve, while we wait
    /// for its reply on another connection's thread (run by hand: `--ignored`).
    #[test]
    #[ignore]
    #[allow(clippy::unwrap_used)]
    fn calls_are_served_while_a_call_waits() {
        let mut server = Bus::session().unwrap();
        let name = server.unique_name().unwrap();
        server.serve(Box::new(|m| {
            let said: String = m.read1().ok()?;
            (m.member().as_deref() == Some("Echo")).then(|| m.return_with_args((said,)))
        }));
        let caller = std::thread::spawn(move || {
            let mut client = Bus::session().unwrap();
            let (echo,): (String,) = client.call(&name, "/", "test.Echo", "Echo", ("hi",)).unwrap();
            let refused = client.call::<()>(&name, "/", "test.Echo", "Other", ());
            (echo, refused)
        });
        while !caller.is_finished() {
            server.wait(Some(Duration::from_millis(100))).unwrap();
        }
        let (echo, refused) = caller.join().unwrap();
        assert_eq!(echo, "hi");
        assert!(matches!(refused, Err(Error::Method { .. })));
    }
}
