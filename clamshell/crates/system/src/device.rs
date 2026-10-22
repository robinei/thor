//! The device as a whole: a screenshot of both screens (grim, through the compositor's screen capture),
//! sleep through logind (suspend-then-hibernate where the system can, a swap file to hibernate to, else
//! suspend; waking is noticed by the clock thread, see [`crate::power::sleep`]), and notifications through the
//! session's notification daemon (mako, on the main screen); the time zone, whether the network sets
//! the clock (NTP) and the clock itself through timedated (`timedatectl`: setting them needs polkit's
//! permission, which the image gives the session's user); the account through the image's helper (see
//! [`crate::account`]).

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clamshell_core::device::account::AccountCommand;
use clamshell_core::device::idle::IdleEvent;
use clamshell_core::{Event, Reading, TimeZones};

use crate::os::worker::Worker;
use crate::os::{command, dbus};
use crate::{Closed, Sink};

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
    /// The account read, or changed (then what came of it, and the account read again).
    Account(clamshell_core::device::account::AccountCommand),
    /// The top screen's refresh rate calibrated (thor-panel-calibration, on a thread of its own: it takes
    /// a couple of minutes); what it said comes back.
    CalibrateRefresh,
}

/// How long the top screen's calibration may take (it measures each rate for a few seconds).
const CALIBRATION_LIMIT: Duration = Duration::from_secs(600);

/// How long a runner's `awake` command may take.
const AWAKE_LIMIT: Duration = Duration::from_secs(10);

/// Runs thor-panel-calibration: the rates it found (its lines saying them), or why it failed.
fn calibrate_refresh() -> Result<String, String> {
    let out = command::run::<&str>("thor-panel-calibration", &[], None, CALIBRATION_LIMIT)?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
    if out.status.success() {
        let rates: Vec<String> = text(&out.stdout).lines().filter(|l| l.contains(" Hz: ")).map(str::to_owned).collect();
        Ok(rates.join("; "))
    } else {
        Err(text(&out.stderr).lines().last().unwrap_or("failed").to_owned())
    }
}

/// Whether `argv` succeeds within [`AWAKE_LIMIT`].
fn awake(argv: &[String]) -> bool {
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    command::run(program, args, None, AWAKE_LIMIT).is_ok_and(|ran| ran.status.success())
}

/// Screenshots go to `screenshots` (a folder, made if needed). What may take seconds (the account's
/// helper, a runner's awake check, the panel's calibration) runs on a thread of its own, so a screenshot
/// or sleep asked for meanwhile doesn't wait for it.
pub fn spawn(screenshots: Option<PathBuf>, sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    Worker::spawn("system", move |commands| {
        // Our last notification, which the next replaces (a lower battery's warning the earlier one's).
        let mut shown = 0;
        for command in commands {
            if carry_out(command, screenshots.as_ref(), &mut shown, &sink).is_err() {
                return;
            }
        }
    })
}

/// One command of the system thread's; `Closed` once the UI is gone.
fn carry_out(command: Command, screenshots: Option<&PathBuf>, shown: &mut u32, sink: &(impl Sink + Clone)) -> Result<(), Closed> {
    match command {
        Command::Screenshot => sink.send(Event::Screenshot(screenshot(screenshots))),
        // (Busy: something works on that the user left it to, a download, a build.)
        Command::Sleep => match busy() {
            Some(why) => {
                log::info!("sleep: not now, {why}");
                sink.send(Event::Idle(IdleEvent::Busy))
            }
            None => {
                sleep();
                Ok(())
            }
        },
        Command::AskAwake(argv) => {
            in_background("awake", sink, move || if awake(&argv) { vec![Event::Idle(IdleEvent::KeptAwake)] } else { vec![] });
            Ok(())
        }
        Command::ReadTime => sink.send(Event::Reading(Reading::Time(read_time(None)))),
        Command::SetTimeZone(zone) => {
            let error = set_time_zone(&zone).err();
            sink.send(Event::Reading(Reading::Time(read_time(error))))
        }
        Command::SetNtp(on) => {
            let error = timedatectl(&["set-ntp", if on { "true" } else { "false" }]).err();
            sink.send(Event::Reading(Reading::Time(read_time(error))))
        }
        Command::StepClock(field, by) => {
            let error = step_clock(field, by).err();
            // (The clock moved: the time now, then the reading.)
            let now = crate::time::clock::unix_secs();
            sink.send(Event::Tick { unix_secs: now, mono_secs: crate::time::clock::mono_secs() })?;
            sink.send(Event::Reading(Reading::Time(read_time(error))))
        }
        Command::CalibrateRefresh => {
            in_background("refresh", sink, || {
                let result = calibrate_refresh();
                log::info!("refresh calibration: {result:?}");
                vec![Event::RefreshCalibrated(result)]
            });
            Ok(())
        }
        Command::Account(command) => {
            in_background("account", sink, move || {
                let mut events = Vec::new();
                if command != AccountCommand::Read {
                    let result = crate::account::change(&command);
                    if let Err(e) = &result {
                        log::warn!("account: {e}");
                    }
                    events.push(Event::AccountChanged(result));
                }
                events.push(Event::Reading(Reading::Account(crate::account::read())));
                events
            });
            Ok(())
        }
        Command::Notify { summary, body, urgent } => {
            match notify(*shown, &summary, &body, urgent) {
                Ok(id) => *shown = id,
                Err(e) => log::warn!("notification \"{summary}\": {e}"),
            }
            Ok(())
        }
    }
}

/// `work` on a thread of its own, its events sent when it is done (a thread that can't start: logged).
fn in_background(name: &str, sink: &(impl Sink + Clone), work: impl FnOnce() -> Vec<Event> + Send + 'static) {
    let sink = sink.clone();
    let started = crate::os::worker::detached(name, move || {
        for event in work() {
            if sink.send(event).is_err() {
                return;
            }
        }
    });
    if let Err(e) = started {
        log::warn!("{name}: {e}");
    }
}

/// Both screens (as the compositor lays them out) into one PNG named by the time; its path, or why not.
fn screenshot(dir: Option<&PathBuf>) -> Result<String, String> {
    let dir = dir.ok_or("no folder for screenshots (no HOME)")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let secs = crate::time::clock::unix_secs();
    let file = dir.join(format!("clamshell-{}.png", timestamp(secs)));
    let path = file.to_string_lossy().into_owned();
    if command::output("grim", &[&path]).is_some() {
        log::info!("screenshot: {path}");
        Ok(path)
    } else {
        Err("grim failed (is it installed, and does the compositor allow screen capture?)".into())
    }
}

/// A UTC timestamp for file names: 20261015-103205.
fn timestamp(secs: u64) -> String {
    let at = i64::try_from(secs).ok().and_then(|s| jiff::Timestamp::from_second(s).ok()).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    at.strftime("%Y%m%d-%H%M%S").to_string()
}

/// Sleep: suspend, then hibernate after a while (logind's HibernateDelaySec) if it can, else suspend.
/// Above this much CPU (cores) for the user's processes, or this many bytes a second on the network, the
/// machine is busy: not put to sleep.
const BUSY_CPU: f64 = 0.3;
const BUSY_NET: f64 = 100_000.0;

/// Whether the machine is busy now (looked at for a few seconds): the user's processes' CPU (the user
/// manager's cgroup, all of them: ClamShell's launches, Steam), or the network's traffic; why.
fn busy() -> Option<String> {
    let user = user_cgroup();
    let cpu = || user.as_deref().and_then(crate::os::cgroup::cpu_usec);
    let net = || {
        let text = crate::os::sys::text("/proc/net/dev").unwrap_or_default();
        // (Each interface but loopback: received bytes, the 1st number, and sent, the 9th.)
        text.lines()
            .skip(2)
            .filter_map(|l| l.split_once(':'))
            .filter(|(name, _)| name.trim() != "lo")
            .map(|(_, v)| {
                let n: Vec<u64> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                n.first().copied().unwrap_or(0) + n.get(8).copied().unwrap_or(0)
            })
            .sum::<u64>()
    };
    let (cpu0, net0, t0) = (cpu(), net(), std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_secs(3));
    let (cpu1, net1) = (cpu(), net());
    let secs = t0.elapsed().as_secs_f64();
    let cores = cpu0.zip(cpu1).map_or(0.0, |(a, b)| b.saturating_sub(a) as f64 / 1e6 / secs);
    let rate = net1.saturating_sub(net0) as f64 / secs;
    if cores > BUSY_CPU {
        return Some(format!("{cores:.1} cores in use"));
    }
    (rate > BUSY_NET).then(|| format!("{:.0} kB/s on the network", rate / 1000.0))
}

/// The user manager's cgroup (`user@<uid>.service`): the cgroup ClamShell runs in, up to it.
fn user_cgroup() -> Option<PathBuf> {
    let path = crate::os::cgroup::path_of(std::process::id())?;
    let end = path.find(".service/").map(|i| i + ".service".len())?;
    Some(crate::os::cgroup::dir(path.get(..end)?))
}

fn sleep() {
    let logind = |bus: &mut dbus::Bus, method: &str| {
        bus.call::<(String,)>("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", method, ())
    };
    let slept = dbus::Bus::system().and_then(|mut bus| {
        let (method, interactive) = if logind(&mut bus, "CanSuspendThenHibernate")?.0 == "yes" {
            log::info!("sleep: suspend, then hibernate");
            ("SuspendThenHibernate", false)
        } else {
            log::info!("sleep: suspend");
            ("Suspend", false)
        };
        bus.call::<()>("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", method, (interactive,))
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
fn notify(replaces: u32, summary: &str, body: &str, urgent: bool) -> dbus::Result<u32> {
    // Urgency: 1 normal, 2 critical (stays until dismissed). Timeout -1: the daemon's own.
    let hints = dbus::PropMap::from([("urgency".to_owned(), dbus::value(if urgent { 2u8 } else { 1u8 }))]);
    let actions: Vec<&str> = Vec::new();
    let (id,): (u32,) = dbus::Bus::session()?.call(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        ("ClamShell", replaces, "", summary, body, actions, hints, -1i32),
    )?;
    Ok(id)
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

#[cfg(test)]
mod live {
    /// On a desktop: whether the machine is busy now, and the cgroup looked at (run by hand: `--ignored
    /// --nocapture`).
    #[test]
    #[ignore = "reads this machine"]
    fn busy_now() {
        println!("{:?}: {:?}", super::user_cgroup(), super::busy());
    }
}
