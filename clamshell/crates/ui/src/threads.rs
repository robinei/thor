//! The UI's and the compositor's own threads: the font finder, the image decoder, the window trace's
//! writer and Xwayland's reaper. Each starts at the ordinary priority ([`start`]), and a long-lived one is
//! started again after a panic ([`lasting`]).
//!
//! The system crate's `os::worker` does the same for the system threads, with waits between restarts. The
//! UI and the compositor don't depend on that crate, so the little they need is here.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

/// A run shorter than this that ends in a panic counts towards giving up.
const CALM: Duration = Duration::from_secs(30);

/// Panics in a row, each this soon after its start, after which a thread is given up.
const GIVE_UP: u32 = 5;

/// Starts a thread named `name` running `body`, at the ordinary priority whichever thread started it: the
/// compositor's thread runs at a raised nice level (rtkit), which a new thread would inherit, ahead of
/// the game.
pub fn start<T: Send + 'static>(name: &str, body: impl FnOnce() -> T + Send + 'static) -> io::Result<thread::JoinHandle<T>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        ordinary_priority();
        body()
    })
}

/// Starts a long-lived thread named `name` ([`start`]). It ends when `body` returns. After a panic (the
/// panic hook has logged it) `body` runs again at once, keeping what it captured, unless it panicked
/// [`GIVE_UP`] times in a row soon after starting: then the thread is given up, logged as an error.
pub fn lasting(name: &'static str, body: impl FnMut() + Send + 'static) -> io::Result<()> {
    start(name, move || restarted(name, body)).map(drop)
}

/// Runs `body` until it returns, again after each panic, as [`lasting`] says.
fn restarted(name: &str, mut body: impl FnMut()) {
    let mut quick = 0;
    loop {
        let started = Instant::now();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body)).is_ok() {
            return;
        }
        quick = if started.elapsed() < CALM { quick + 1 } else { 0 };
        if quick >= GIVE_UP {
            log::error!("{name}: panicked {quick} times soon after starting: given up");
            return;
        }
        log::error!("{name}: panicked; starting it again");
    }
}

/// The calling thread back at nice level 0 if it is below (raising the level needs no privilege).
fn ordinary_priority() {
    let thread = rustix::thread::gettid();
    let name = || thread::current().name().map(str::to_owned).unwrap_or_default();
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

    #[test]
    fn a_thread_that_keeps_panicking_is_given_up_and_one_that_recovers_runs_on() {
        let mut runs = 0;
        restarted("test", || {
            runs += 1;
            panic!("always");
        });
        assert_eq!(runs, GIVE_UP);
        let mut runs = 0;
        restarted("test", || {
            runs += 1;
            if runs < 3 {
                panic!("twice");
            }
        });
        assert_eq!(runs, 3);
    }
}
