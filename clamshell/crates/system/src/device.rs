//! The device as a whole: a screenshot of both screens (grim, through the compositor's screen capture),
//! sleep through logind (suspend-then-hibernate where the system can, a swap file to hibernate to, else
//! suspend; waking is noticed by the clock thread, see [`crate::sleep`]), and notifications through the
//! session's notification daemon (mako, on the main screen); the time zone, whether the network sets
//! the clock (NTP) and the clock itself through timedated (`timedatectl`: setting them needs polkit's
//! permission, which the image gives the session's user).

use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clamshell_core::{Event, Reading, TimeZones};

use crate::worker::Worker;
use crate::{Sink, command, dbus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Screenshot,
    Sleep,
    Notify {
        summary: String,
        body: String,
        urgent: bool,
    },
    /// Read the time zone and the ones to choose from.
    ReadTime,
    SetTimeZone(String),
    /// The network sets the clock (NTP), or not.
    SetNtp(bool),
    /// The clock a step of a field on (or back), in local time.
    StepClock(clamshell_core::ClockField, i8),
    /// A runner's `awake` command run: if it succeeds, the shown app keeps the screen on.
    AskAwake(Vec<String>),
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    gen_async = false
)]
trait Manager {
    fn can_suspend_then_hibernate(&self) -> zbus::Result<String>;
    fn suspend_then_hibernate(&self, interactive: bool) -> zbus::Result<()>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

/// Whether `argv` succeeds (within a few seconds).
fn awake(argv: &[String]) -> bool {
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    let mut timed = vec!["10".to_owned(), program.clone()];
    timed.extend(args.iter().cloned());
    std::process::Command::new("timeout")
        .args(&timed)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Screenshots go to `screenshots` (a folder, made if needed).
pub fn spawn(screenshots: Option<PathBuf>, sink: impl Sink) -> io::Result<Worker<Command>> {
    Worker::spawn("system", move |commands| {
        // Our last notification, which the next replaces (a lower battery's warning the earlier one's).
        let mut shown = 0;
        for command in commands {
            match command {
                Command::Screenshot => {
                    let result = screenshot(screenshots.as_ref());
                    if sink.send(Event::Screenshot(result)).is_err() {
                        return;
                    }
                }
                Command::Sleep => sleep(),
                Command::AskAwake(argv) => {
                    if awake(&argv) && sink.send(Event::Idle(clamshell_core::idle::IdleEvent::KeptAwake)).is_err() {
                        return;
                    }
                }
                Command::ReadTime => {
                    if sink.send(Event::Reading(Reading::Time(read_time(None)))).is_err() {
                        return;
                    }
                }
                Command::SetTimeZone(zone) => {
                    let error = set_time_zone(&zone).err();
                    if sink.send(Event::Reading(Reading::Time(read_time(error)))).is_err() {
                        return;
                    }
                }
                Command::SetNtp(on) => {
                    let error = timedatectl(&["set-ntp", if on { "true" } else { "false" }]).err();
                    if sink.send(Event::Reading(Reading::Time(read_time(error)))).is_err() {
                        return;
                    }
                }
                Command::StepClock(field, by) => {
                    let error = step_clock(field, by).err();
                    // (The clock moved: the time now, then the reading.)
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    if sink.send(Event::Tick { unix_secs: now }).is_err() || sink.send(Event::Reading(Reading::Time(read_time(error)))).is_err() {
                        return;
                    }
                }
                Command::Notify { summary, body, urgent } => match notify(shown, &summary, &body, urgent) {
                    Ok(id) => shown = id,
                    Err(e) => log::warn!("notification \"{summary}\": {e}"),
                },
            }
        }
    })
}

/// Both screens (as the compositor lays them out) into one PNG named by the time; its path, or why not.
fn screenshot(dir: Option<&PathBuf>) -> Result<String, String> {
    let dir = dir.ok_or("no folder for screenshots (no HOME)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = dir.join(format!("clamshell-{}.png", timestamp(secs)));
    let path = file.to_string_lossy().into_owned();
    if command::run("grim", &[&path]) {
        log::info!("screenshot: {path}");
        Ok(path)
    } else {
        Err("grim failed (is it installed, and does the compositor allow screen capture?)".into())
    }
}

/// A UTC timestamp for file names: 20261015-103205.
fn timestamp(secs: u64) -> String {
    let (year, month, day, h, m, s) = civil(secs);
    format!("{year:04}{month:02}{day:02}-{h:02}{m:02}{s:02}")
}

/// Seconds since the epoch as a UTC date and time: year, month, day, hour, minute, second.
pub(crate) fn civil(secs: u64) -> (i64, i64, i64, u64, u64, u64) {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Sleep: suspend, then hibernate after a while (logind's HibernateDelaySec) if it can, else suspend.
fn sleep() {
    let slept = dbus::system(None).and_then(|conn| {
        let logind = dbus::default_proxy::<ManagerProxy>(&conn)?;
        if logind.can_suspend_then_hibernate()? == "yes" {
            log::info!("sleep: suspend, then hibernate");
            logind.suspend_then_hibernate(false)
        } else {
            log::info!("sleep: suspend");
            logind.suspend(false)
        }
    });
    if let Err(e) = slept {
        log::warn!("sleep: {e}");
    }
}

/// The time zone and the ones to choose from (with `error`, from setting it, if any).
fn read_time(error: Option<String>) -> TimeZones {
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
fn timedatectl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("timedatectl")
        .arg("--no-ask-password")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("timedatectl: {e}"))?;
    if out.status.success() {
        log::info!("timedatectl {}", args.join(" "));
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    log::warn!("timedatectl {}: {said}", args.join(" "));
    Err(format!("Couldn't: {said}"))
}

/// The clock a step of `field` on (`by` 1) or back (-1), as the local time has it (a month on from the
/// 31st: the month's last day), set by timedated.
fn step_clock(field: clamshell_core::ClockField, by: i8) -> Result<(), String> {
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
fn set_time_zone(zone: &str) -> Result<(), String> {
    timedatectl(&["set-timezone", zone])
}

/// A notification through the session bus' notification daemon, replacing `replaces` (0: none); its id.
fn notify(replaces: u32, summary: &str, body: &str, urgent: bool) -> zbus::Result<u32> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let conn = zbus::blocking::Connection::session()?;
    // Urgency: 1 normal, 2 critical (stays until dismissed). Timeout -1: the daemon's own.
    let hints = HashMap::from([("urgency", Value::U8(if urgent { 2 } else { 1 }))]);
    let actions: Vec<&str> = Vec::new();
    let reply = conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &("ClamShell", replaces, "", summary, body, actions, hints, -1i32),
    )?;
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(0), "19700101-000000");
        assert_eq!(timestamp(1_792_060_325), "20261015-103205");
        assert_eq!(timestamp(951_782_400), "20000229-000000", "a leap day");
    }
}
