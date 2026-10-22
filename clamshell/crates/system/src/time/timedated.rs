//! The time zone, whether the network sets the clock (NTP), and the clock itself, through timedated
//! (`timedatectl`: setting them needs polkit's permission, which the image gives the session's user).

use clamshell_core::TimeZones;

use crate::os::command;

/// The time zone and the ones to choose from (with `error`, from setting it, if any).
pub fn read_time(error: Option<String>) -> TimeZones {
    let text = |args: &[&str]| command::output("timedatectl", args).map(|out| String::from_utf8_lossy(&out).into_owned());
    let current = text(&["show", "--property=Timezone", "--value"]).map(|z| z.trim().to_owned()).filter(|z| !z.is_empty());
    let zones: Vec<String> = text(&["list-timezones"]).map(|list| list.lines().map(str::to_owned).collect()).unwrap_or_default();
    let error = error.or_else(|| (current.is_none() || zones.is_empty()).then(|| "Couldn't read the time zone (timedatectl)".to_owned()));
    // "CanNTP=yes", "NTP=no", "NTPSynchronized=no".
    let sync = text(&["show", "--property=CanNTP", "--property=NTP", "--property=NTPSynchronized"]).unwrap_or_default();
    let yes = |key: &str| sync.lines().any(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')) == Some("yes"));
    let ntp = yes("CanNTP").then(|| yes("NTP"));
    TimeZones { current, zones, ntp, synced: yes("NTPSynchronized"), error }
}

/// timedatectl with `args`, without asking for a password (polkit allows the session's user); what it
/// said, if it failed.
pub fn timedatectl(args: &[&str]) -> Result<(), String> {
    let out = command::run("timedatectl", &[&["--no-ask-password"], args].concat(), None, command::LIMIT)?;
    if out.status.success() {
        log::info!("timedatectl {}", args.join(" "));
        return Ok(());
    }
    let said = out.said();
    log::warn!("timedatectl {}: {said}", args.join(" "));
    Err(format!("Couldn't: {said}"))
}

/// The clock a step of `field` on (`by` 1) or back (-1), as the local time has it (a month on from the
/// 31st: the month's last day), set by timedated.
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
    timedatectl(&["set-time", &then.strftime("%Y-%m-%d %H:%M:%S").to_string()])
}

/// Sets the system's time zone; what timedatectl said, if it failed (polkit refusing, an unknown zone).
pub fn set_time_zone(zone: &str) -> Result<(), String> {
    timedatectl(&["set-timezone", zone])
}
