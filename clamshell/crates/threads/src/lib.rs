//! Starting ClamShell's threads, and starting a long-lived one again after a panic, by one policy for all
//! of them: the UI's and the compositor's threads (the font finder, the image decoder, the window trace's
//! writer, Xwayland's reaper), the system threads (`clamshell-system`'s `os::worker`), and Xwayland
//! itself (the compositor's `x11`, which waits as [`Backoff`] says before starting it again). It depends
//! on none of ClamShell's crates, so that each of them can use it.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

/// After a run of failures, the waits before starting again grow from none to this, doubling.
pub const RESTART_FIRST: Duration = Duration::from_millis(250);
pub const RESTART_MOST: Duration = Duration::from_secs(5);
/// A run this long before failing is followed by a start at once.
pub const RESTART_CALM: Duration = Duration::from_secs(30);
/// Failures in a row, each sooner than [`RESTART_CALM`] after its start, after which the thread (or
/// Xwayland) is given up: what made it fail would only make it fail again.
pub const RESTART_GIVE_UP: u32 = 5;

/// The wait before starting again after a failure: none at first, then doubling from [`RESTART_FIRST`] to
/// [`RESTART_MOST`] while the runs keep failing soon after starting; none again after a calm run. None at
/// all, to give up, once [`RESTART_GIVE_UP`] runs in a row failed soon after starting.
#[derive(Debug, Default)]
pub struct Backoff {
    delay: Duration,
    /// The runs in a row that failed soon after starting.
    quick: u32,
}

impl Backoff {
    /// The wait after a run of `ran` that failed, or `None` to give up.
    pub fn after(&mut self, ran: Duration) -> Option<Duration> {
        let calm = ran >= RESTART_CALM;
        self.quick = if calm { 0 } else { self.quick + 1 };
        if self.quick >= RESTART_GIVE_UP {
            return None;
        }
        let wait = if calm { Duration::ZERO } else { self.delay };
        self.delay = (wait * 2).clamp(RESTART_FIRST, RESTART_MOST);
        Some(wait)
    }

    /// The runs in a row that failed soon after starting.
    pub fn quick(&self) -> u32 {
        self.quick
    }
}

/// Starts a thread named `name` running `body`, at the ordinary priority whichever thread started it: the
/// compositor's thread runs at a raised nice level (rtkit), which a new thread would inherit, ahead of the
/// game.
pub fn start<T: Send + 'static>(name: &str, body: impl FnOnce() -> T + Send + 'static) -> io::Result<thread::JoinHandle<T>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        ordinary_priority();
        body()
    })
}

/// Starts a long-lived thread named `name` ([`start`]), started again after a panic ([`restarted`], told
/// to no one). It ends when `body` returns.
pub fn lasting(name: &'static str, mut body: impl FnMut() + Send + 'static) -> io::Result<()> {
    start(name, move || restarted(name, &mut body, |_| {})).map(drop)
}

/// Runs `body` until it returns. A panic in it (the panic hook has logged it) is logged and `body` runs
/// again, keeping what it captured: at once, then after the waits of [`Backoff`] while it keeps panicking
/// soon after starting. Before it runs again, `told` is given the thread's name. After
/// [`RESTART_GIVE_UP`] such panics in a row the thread is given up ([`given_up`]).
pub fn restarted(name: &'static str, mut body: impl FnMut(), told: impl Fn(&'static str)) {
    let mut backoff = Backoff::default();
    loop {
        let started = Instant::now();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body)).is_ok() {
            return;
        }
        let Some(wait) = backoff.after(started.elapsed()) else {
            return given_up(name);
        };
        log::error!("{name}: panicked; starting it again in {} ms", wait.as_millis());
        thread::sleep(wait);
        told(name);
    }
}

/// Logs that thread `name` is given up after panicking [`RESTART_GIVE_UP`] times soon after starting.
pub fn given_up(name: &str) {
    log::error!("{name}: panicked {RESTART_GIVE_UP} times soon after starting: given up, so what it does is lost until ClamShell starts again");
}

/// The calling thread back at nice level 0 if it is below (raising the level needs no privilege).
fn ordinary_priority() {
    let thread = rustix::thread::gettid();
    let name = || thread::current().name().map(str::to_owned).unwrap_or_else(|| "a thread".to_owned());
    match rustix::process::getpriority_process(Some(thread)) {
        Ok(level) if level < 0 => {
            if let Err(e) = rustix::process::setpriority_process(Some(thread), 0) {
                log::warn!("{}: nice level {level} kept: {e}", name());
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("{}: nice level unknown: {e}", name()),
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn the_wait_after_a_failure_is_none_then_doubles_and_none_after_a_calm_run() {
        let mut backoff = Backoff::default();
        let soon = Duration::from_secs(1);
        let waits: Vec<Option<Duration>> = (0..4).map(|_| backoff.after(soon)).collect();
        let ms = |n: u64| Some(Duration::from_millis(n));
        assert_eq!(waits, [ms(0), ms(250), ms(500), ms(1000)]);
        assert_eq!(backoff.after(RESTART_CALM), Some(Duration::ZERO), "a calm run starts over");
        assert_eq!(backoff.quick(), 0);
        assert_eq!(backoff.after(soon), Some(RESTART_FIRST));
        let mut backoff = Backoff { delay: RESTART_MOST, quick: 0 };
        assert_eq!(backoff.after(soon), Some(RESTART_MOST), "at most");
    }

    /// Runs that keep failing soon after starting are given up after a few.
    #[test]
    fn runs_that_keep_failing_soon_after_starting_are_given_up() {
        let mut backoff = Backoff::default();
        let soon = Duration::from_secs(1);
        let waits: Vec<Option<Duration>> = (0..RESTART_GIVE_UP).map(|_| backoff.after(soon)).collect();
        assert_eq!(waits.iter().filter(|w| w.is_some()).count() as u32, RESTART_GIVE_UP - 1);
        assert_eq!(waits.last(), Some(&None), "given up at the last");
    }

    /// A thread that keeps panicking is given up, told each time it runs again; one that recovers runs on.
    #[test]
    fn a_thread_that_keeps_panicking_is_given_up_and_one_that_recovers_runs_on() {
        let (mut runs, told) = (0, Cell::new(0));
        restarted(
            "test",
            || {
                runs += 1;
                panic!("always");
            },
            |_| told.set(told.get() + 1),
        );
        assert_eq!((runs, told.get()), (RESTART_GIVE_UP, RESTART_GIVE_UP - 1));
        let mut runs = 0;
        restarted(
            "test",
            || {
                runs += 1;
                if runs < 3 {
                    panic!("twice");
                }
            },
            |_| {},
        );
        assert_eq!(runs, 3);
    }
}
