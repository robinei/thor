//! What the threads that talk to system services over D-Bus share: connecting, proxies that read
//! properties fresh (readings are polled, so nothing is cached), signals forwarded to a channel, and
//! reading a property map. The services' interfaces are typed proxies in their modules (wifi::nm,
//! bluetooth::bluez, units, battery, brightness).

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::blocking::proxy::ProxyImpl;
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;

/// The system bus; method calls fail after `timeout` (zbus' default otherwise).
pub fn system(timeout: Option<Duration>) -> zbus::Result<Connection> {
    let builder = zbus::blocking::connection::Builder::system()?;
    match timeout {
        Some(t) => builder.method_timeout(t).build(),
        None => builder.build(),
    }
}

/// A proxy for the object at `path`, reading properties fresh each time.
pub fn proxy<P>(conn: &Connection, path: &str) -> zbus::Result<P>
where
    P: ProxyImpl<'static> + From<zbus::Proxy<'static>>,
{
    P::builder(conn).path(path.to_owned())?.cache_properties(CacheProperties::No).build()
}

/// A proxy for the interface's usual object, reading properties fresh each time.
pub fn default_proxy<P>(conn: &Connection) -> zbus::Result<P>
where
    P: ProxyImpl<'static> + From<zbus::Proxy<'static>>,
{
    P::builder(conn).cache_properties(CacheProperties::No).build()
}

/// Whether an error is the bus itself failing (a closed connection, a timeout), rather than the
/// service's answer (a method error), after which a kept connection is made again.
pub fn is_bus_failure(e: &zbus::Error) -> bool {
    !matches!(e, zbus::Error::MethodError(..) | zbus::Error::FDO(_))
}

/// Whether an error says the service isn't on the bus at all (not installed, or not running).
pub fn is_absent(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, ..) => {
            matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner")
        }
        zbus::Error::FDO(e) => matches!(**e, zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)),
        _ => false,
    }
}

/// A system-bus connection kept between calls (connected when first needed), and made again after the
/// bus failed.
#[derive(Debug, Default)]
pub struct Kept {
    conn: Option<Connection>,
}

impl Kept {
    pub fn get(&mut self) -> zbus::Result<&Connection> {
        let conn = match self.conn.take() {
            Some(conn) => conn,
            None => system(None)?,
        };
        Ok(self.conn.insert(conn))
    }

    /// A call through it failed: if the bus did, the connection goes and the next call connects again.
    pub fn failed(&mut self, error: &zbus::Error) {
        if is_bus_failure(error) {
            self.conn = None;
        }
    }
}

/// Forwards what `parse` makes of each signal to the receiver returned, on a thread of its own. The
/// thread ends with the signals (the connection closed) or at the first signal after the receiver is
/// dropped.
pub fn forward_signals<S, T>(name: &str, signals: S, parse: impl Fn(S::Item) -> Option<T> + Send + 'static) -> io::Result<Receiver<T>>
where
    S: IntoIterator + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    crate::worker::detached(name, move || {
        for value in signals.into_iter().filter_map(parse) {
            if tx.send(value).is_err() {
                return;
            }
        }
    })?;
    Ok(rx)
}

/// An object's properties, as GetManagedObjects or GetSettings give them.
#[derive(Debug, Clone, Copy)]
pub struct Props<'a>(pub &'a HashMap<String, OwnedValue>);

impl Props<'_> {
    pub fn str(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    }

    /// A boolean property; false when missing.
    pub fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
    }

    pub fn bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.0.get(key).and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn properties_by_type() {
        let owned = |v: Value| OwnedValue::try_from(v).unwrap();
        let map = HashMap::from([
            ("Name".to_owned(), owned(Value::from("Pad"))),
            ("Paired".to_owned(), owned(Value::from(true))),
            ("Ssid".to_owned(), owned(Value::from(b"home".to_vec()))),
        ]);
        let p = Props(&map);
        assert_eq!(p.str("Name").as_deref(), Some("Pad"));
        assert!(p.flag("Paired") && !p.flag("Connected"));
        assert_eq!(p.bytes("Ssid").as_deref(), Some(&b"home"[..]));
        assert_eq!(p.str("Paired"), None, "the wrong type");
    }

    #[test]
    fn method_errors_keep_the_connection() {
        assert!(is_bus_failure(&zbus::Error::InputOutput(std::sync::Arc::new(io::Error::other("closed")))));
        assert!(!is_bus_failure(&zbus::Error::FDO(Box::new(zbus::fdo::Error::AccessDenied("no".into())))));
    }

    #[test]
    fn signals_are_forwarded_until_they_end() {
        let rx = forward_signals("test", vec![1, 2, 3], |n| (n != 2).then_some(n * 10)).unwrap();
        assert_eq!(rx.iter().collect::<Vec<_>>(), vec![10, 30]);
    }
}
