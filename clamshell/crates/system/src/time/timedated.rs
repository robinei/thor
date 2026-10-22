//! The time zone, whether the network sets the clock (NTP), and the clock itself, through timedated on
//! the system bus. Changing them needs polkit's permission, which the image gives the session's user, so
//! the calls never ask for a password.

use clamshell_core::TimeZones;

use crate::os::dbus::{self, Bus, Props};

const DEST: &str = "org.freedesktop.timedate1";
const PATH: &str = "/org/freedesktop/timedate1";
const IFACE: &str = "org.freedesktop.timedate1";

/// The time zone and the ones to choose from, with `error` from setting it, if any.
pub fn read_time(error: Option<String>) -> TimeZones {
    let read = || -> dbus::Result<_> {
        let mut bus = Bus::system()?;
        let props = bus.get_all(DEST, PATH, IFACE)?;
        let (zones,): (Vec<String>,) = bus.call(DEST, PATH, IFACE, "ListTimezones", ())?;
        Ok((props, zones))
    };
    match read() {
        Ok((props, zones)) => {
            let props = Props(&props);
            let current = props.str("Timezone").filter(|z| !z.is_empty());
            let ntp = props.flag("CanNTP").then(|| props.flag("NTP"));
            let error = error.or_else(|| current.is_none().then(|| "Couldn't read the time zone".to_owned()));
            TimeZones { current, zones, ntp, synced: props.flag("NTPSynchronized"), error }
        }
        Err(e) => {
            log::warn!("timedated: {e}");
            TimeZones { current: None, zones: Vec::new(), ntp: None, synced: false, error: error.or_else(|| Some(format!("Couldn't read the time zone: {e}"))) }
        }
    }
}

/// Calls `method` with `args`, which end with `interactive` false; `what` names the value for the log.
/// Returns the service's message if it failed.
fn call(method: &str, what: &dyn std::fmt::Display, args: impl dbus::AppendAll) -> Result<(), String> {
    let result = Bus::system().and_then(|mut bus| bus.call::<()>(DEST, PATH, IFACE, method, args));
    match result {
        Ok(()) => {
            log::info!("timedated {method} {what}");
            Ok(())
        }
        Err(e) => {
            log::warn!("timedated {method} {what}: {e}");
            Err(format!("Couldn't: {}", e.message()))
        }
    }
}

pub fn set_ntp(on: bool) -> Result<(), String> {
    call("SetNTP", &on, (on, false))
}

/// Moves the clock one step of `field` forward (`by` 1) or back (-1) in local time. A month on from the
/// 31st lands on the next month's last day.
pub fn step_clock(field: clamshell_core::ClockField, by: i8) -> Result<(), String> {
    use clamshell_core::ClockField;
    let now = jiff::Zoned::now();
    let by = i64::from(by);
    let span = match field {
        ClockField::Minute => jiff::Span::new().minutes(by),
        ClockField::Hour => jiff::Span::new().hours(by),
        ClockField::Day => jiff::Span::new().days(by),
        ClockField::Month => jiff::Span::new().months(by),
        ClockField::Year => jiff::Span::new().years(by),
    };
    let then = now.checked_add(span).map_err(|e| format!("Couldn't: {e}"))?;
    // SetTime takes microseconds since the epoch, and whether that is relative to now.
    call("SetTime", &then, (then.timestamp().as_microsecond(), false, false))
}

/// Sets the system's time zone. Returns the service's message if it failed, as when polkit refuses or the
/// zone is unknown.
pub fn set_time_zone(zone: &str) -> Result<(), String> {
    call("SetTimezone", &zone, (zone, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads this machine's timedated"]
    fn live_read() {
        let time = read_time(None);
        assert!(time.current.is_some(), "{time:?}");
        assert!(time.zones.len() > 100);
    }
}
