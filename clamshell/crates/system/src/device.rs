//! The "system" worker: what the state asks of the device as a whole, one thing at a time (a screenshot,
//! sleep, a notification, the time zone and clock, the account, the panel's calibration), each carried
//! out by its area's module; what may take seconds runs on a thread of its own.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clamshell_core::device::account::AccountCommand;
use clamshell_core::device::idle::IdleEvent;
use clamshell_core::{Event, Reading};

use crate::display::{refresh, screenshot};
use crate::os::command;
use crate::os::worker::Worker;
use crate::power::sleep;
use crate::session::notify;
use crate::time::timedated;
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

/// How long a runner's `awake` command may take.
const AWAKE_LIMIT: Duration = Duration::from_secs(10);

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
        Command::Screenshot => sink.send(Event::Screenshot(screenshot::screenshot(screenshots))),
        // (Busy: something works on that the user left it to, a download, a build.)
        Command::Sleep => match sleep::busy() {
            Some(why) => {
                log::info!("sleep: not now, {why}");
                sink.send(Event::Idle(IdleEvent::Busy))
            }
            None => {
                sleep::suspend();
                Ok(())
            }
        },
        Command::AskAwake(argv) => {
            in_background("awake", sink, move || if awake(&argv) { vec![Event::Idle(IdleEvent::KeptAwake)] } else { vec![] });
            Ok(())
        }
        Command::ReadTime => sink.send(Event::Reading(Reading::Time(timedated::read_time(None)))),
        Command::SetTimeZone(zone) => {
            let error = timedated::set_time_zone(&zone).err();
            sink.send(Event::Reading(Reading::Time(timedated::read_time(error))))
        }
        Command::SetNtp(on) => {
            let error = timedated::set_ntp(on).err();
            sink.send(Event::Reading(Reading::Time(timedated::read_time(error))))
        }
        Command::StepClock(field, by) => {
            let error = timedated::step_clock(field, by).err();
            // (The clock moved: the time now, then the reading.)
            let now = crate::time::clock::unix_secs();
            sink.send(Event::Tick { unix_secs: now, mono_secs: crate::time::clock::mono_secs() })?;
            sink.send(Event::Reading(Reading::Time(timedated::read_time(error))))
        }
        Command::CalibrateRefresh => {
            in_background("refresh", sink, || {
                let result = refresh::calibrate_refresh();
                log::info!("refresh calibration: {result:?}");
                vec![Event::RefreshCalibrated(result)]
            });
            Ok(())
        }
        Command::Account(command) => {
            in_background("account", sink, move || {
                let mut events = Vec::new();
                if command != AccountCommand::Read {
                    let result = crate::session::account::change(&command);
                    if let Err(e) = &result {
                        log::warn!("account: {e}");
                    }
                    events.push(Event::AccountChanged(result));
                }
                events.push(Event::Reading(Reading::Account(crate::session::account::read())));
                events
            });
            Ok(())
        }
        Command::Notify { summary, body, urgent } => {
            match notify::notify(*shown, &summary, &body, urgent) {
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
