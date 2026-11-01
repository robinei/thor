//! Errands: the one-off requests of the state that belong to no area's own thread, carried out one at a
//! time on the "system" thread: a screenshot, sleep, the time zone and clock, the panel's calibration.
//! Each is done by the module for its area. A runner's awake check and the calibration, which may take
//! seconds, run on a thread of their own each; the account's commands on the account thread, in order.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use clamshell_state::idle::IdleEvent;
use clamshell_state::setup::account::AccountCommand;
use clamshell_state::{Event, Reading};

use crate::display::{refresh, screenshot};
use crate::os::command;
use crate::os::worker::Worker;
use crate::power::sleep;
use crate::time::timedated;
use crate::{Closed, Sink};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    SaveScreenshot(Result<clamshell_state::ScreenshotPixels, String>),
    Sleep,
    /// Read the time zone and the ones to choose from.
    ReadTime,
    ReadInstalled,
    SetTimeZone(String),
    /// Whether the network sets the clock (NTP).
    SetNtp(bool),
    /// Moves one field of the clock a step forward or back, in local time.
    StepClock(clamshell_state::ClockField, i8),
    /// Runs a runner's `awake` command. If it succeeds, the shown app keeps the screen on.
    AskAwake(Vec<String>),
    /// Reads or changes the account. After a change, the result is sent and then the account is read
    /// again.
    Account(clamshell_state::setup::account::AccountCommand),
    /// Calibrates the top screen's refresh rate with thor-panel-calibration and sends back the result. It
    /// runs on a thread of its own because it takes a couple of minutes.
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

/// Screenshots go to the folder `screenshots`, which is made if needed. Anything that may take seconds (a
/// runner's awake check, the panel's calibration) runs on a thread of its own, and the account's commands
/// on the account thread ([`spawn_account`]), so a screenshot or sleep asked for meanwhile doesn't wait
/// for it.
pub(crate) fn spawn(screenshots: Option<PathBuf>, calibration: Option<&'static str>, sink: impl Sink + Clone) -> io::Result<Worker<Command>> {
    let account = spawn_account(crate::session::account::change, crate::session::account::read, sink.clone())?;
    Worker::spawn(clamshell_state::Thread::System.name(), move |commands| {
        if sink.send(Event::Reading(Reading::Installed(crate::components::read(std::path::Path::new("/"))))).is_err() {
            return;
        }
        for command in commands {
            if carry_out(command, screenshots.as_ref(), calibration, &account, &sink).is_err() {
                return;
            }
        }
    })
}

/// Carries out one command. Returns `Closed` once the UI is gone.
fn carry_out(
    command: Command,
    screenshots: Option<&PathBuf>,
    calibration: Option<&'static str>,
    account: &Worker<AccountCommand>,
    sink: &(impl Sink + Clone),
) -> Result<(), Closed> {
    match command {
        Command::SaveScreenshot(pixels) => {
            let saved = pixels.and_then(|pixels| screenshot::save(screenshots, pixels));
            if let Err(e) = &saved {
                log::warn!("screenshot: {e}");
            }
            sink.send(Event::Screenshot(saved))
        }
        // Busy means something is working that the user left running, such as a download or a build. It is
        // measured over seconds, so on a thread of its own, as the suspend that follows.
        Command::Sleep => {
            in_background(
                "sleep",
                sink,
                || match sleep::busy() {
                    Some(why) => {
                        log::info!("sleep: not now, {why}");
                        vec![Event::Idle(IdleEvent::Busy)]
                    }
                    None => {
                        sleep::suspend();
                        vec![]
                    }
                },
                Vec::new,
            );
            Ok(())
        }
        Command::AskAwake(argv) => {
            in_background("awake", sink, move || if awake(&argv) { vec![Event::Idle(IdleEvent::KeptAwake)] } else { vec![] }, Vec::new);
            Ok(())
        }
        Command::ReadInstalled => sink.send(Event::Reading(Reading::Installed(crate::components::read(std::path::Path::new("/"))))),
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
            // The clock moved, so send the new time first and then the reading.
            let now = crate::time::clock::unix_secs();
            sink.send(Event::Tick { unix_secs: now, mono_secs: crate::time::clock::mono_secs() })?;
            sink.send(Event::Reading(Reading::Time(timedated::read_time(error))))
        }
        Command::CalibrateRefresh => {
            in_background(
                "refresh",
                sink,
                move || {
                    let result = refresh::calibrate_refresh(calibration);
                    log::info!("refresh calibration: {result:?}");
                    vec![Event::RefreshCalibrated(result)]
                },
                || vec![Event::RefreshCalibrated(Err("it panicked".into()))],
            );
            Ok(())
        }
        Command::Account(command) => {
            account.send(command);
            Ok(())
        }
    }
}

/// Starts the account thread, which reads and changes the account with `change` and `read` (the account
/// helper, through pkexec), one command after another: a read asked before a change is answered before
/// the change's own read, never after it. A change is followed by a read. A command that panics is
/// answered as failed (the account unread as far as anyone knows), so Setup says so and reads it again.
fn spawn_account(
    change: fn(&AccountCommand) -> Result<String, String>,
    read: fn() -> clamshell_state::setup::account::Account,
    sink: impl Sink,
) -> io::Result<Worker<AccountCommand>> {
    use clamshell_state::setup::account::Account;
    Worker::spawn(clamshell_state::Thread::Account.name(), move |commands| {
        // Whether the last read failed, so that a read that works again is logged (the failure was).
        let mut failing = false;
        for command in commands {
            let changing = command != AccountCommand::Read;
            let events = crate::os::worker::or_on_panic(
                || {
                    let mut events = Vec::new();
                    if changing {
                        let result = change(&command);
                        if let Err(e) = &result {
                            log::warn!("account: {e}");
                        }
                        events.push(Event::AccountChanged(result));
                    }
                    events.push(Event::Reading(Reading::Account(read())));
                    events
                },
                || {
                    let failed = Event::Reading(Reading::Account(Account::Failed("the reading crashed".into())));
                    if changing { vec![Event::AccountChanged(Err("it panicked".into())), failed] } else { vec![failed] }
                },
            );
            for event in events {
                if let Event::Reading(Reading::Account(account)) = &event {
                    let failed = matches!(account, Account::Failed(_));
                    if failing && !failed {
                        log::info!("account: read after failing");
                    }
                    failing = failed;
                }
                if sink.send(event).is_err() {
                    return;
                }
            }
        }
    })
}

/// Runs `work` on a thread of its own and sends its events when it is done, or `fallback`'s if it
/// panicked. If the thread can't start, that is logged.
fn in_background(
    name: &str,
    sink: &(impl Sink + Clone),
    work: impl FnOnce() -> Vec<Event> + Send + 'static,
    fallback: impl FnOnce() -> Vec<Event> + Send + 'static,
) {
    let sink = sink.clone();
    let started = crate::os::worker::detached(name, move || {
        for event in crate::os::worker::or_on_panic(work, fallback) {
            if sink.send(event).is_err() {
                return;
            }
        }
    });
    if let Err(e) = started {
        log::warn!("{name}: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use clamshell_state::setup::account::Account;
    use std::sync::atomic::{AtomicBool, Ordering};

    static SET: AtomicBool = AtomicBool::new(false);

    /// A read slow to answer, which reads the account as it was when it began.
    fn slow_read() -> Account {
        let password = SET.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        Account::Read { password, ssh: false }
    }

    fn set_password(_: &AccountCommand) -> Result<String, String> {
        SET.store(true, Ordering::SeqCst);
        Ok("password set".into())
    }

    /// A read asked before a change is answered before the change and its own read: one begun before
    /// the change can't land after it and say there is no password.
    #[test]
    fn the_account_is_read_and_changed_in_the_order_asked() {
        let (sink, events) = std::sync::mpsc::channel();
        let account = spawn_account(set_password, slow_read, sink).unwrap();
        account.send(AccountCommand::Read);
        account.send(AccountCommand::Password { current: None, new: clamshell_state::kit::secret::Secret("pw".into()) });
        let got: Vec<Event> = (0..3).map(|_| events.recv_timeout(Duration::from_secs(2)).unwrap()).collect();
        let read = |password| Event::Reading(Reading::Account(Account::Read { password, ssh: false }));
        assert_eq!(got, vec![read(false), Event::AccountChanged(Ok("password set".into())), read(true)]);
    }

    fn panics(_: &AccountCommand) -> Result<String, String> {
        panic!("the helper's answer made no sense");
    }

    /// A change that panics is answered as failed, and the thread goes on.
    #[test]
    fn a_change_that_panics_is_answered_as_failed() {
        let (sink, events) = std::sync::mpsc::channel();
        let account = spawn_account(panics, || Account::Read { password: true, ssh: false }, sink).unwrap();
        account.send(AccountCommand::Ssh(true));
        account.send(AccountCommand::Read);
        let got: Vec<Event> = (0..3).map(|_| events.recv_timeout(Duration::from_secs(2)).unwrap()).collect();
        assert_eq!(
            got,
            vec![
                Event::AccountChanged(Err("it panicked".into())),
                Event::Reading(Reading::Account(Account::Failed("the reading crashed".into()))),
                Event::Reading(Reading::Account(Account::Read { password: true, ssh: false })),
            ]
        );
    }
}
