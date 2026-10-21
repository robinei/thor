//! Waking from sleep, seen from the clocks: CLOCK_BOOTTIME counts time asleep, CLOCK_MONOTONIC doesn't,
//! so their difference grows by however long the system slept. The clock thread checks it every second.
//! (No D-Bus: logind's PrepareForSleep(false) didn't reach us after a resume in testing, and this also
//! sees sleep that didn't go through logind.) Suspending itself is logind's: the power button and the
//! lid, per logind.conf (the power key not waking it while the lid is closed: [`crate::lid`]).
//!
//! Before sleep, though, the pad thread closes the gyro's stream (the Thor's sensor hub samples on the
//! ADSP while asleep otherwise, as PB-OS's pbos-motiond found): [`spawn_hold`] keeps a logind delay lock,
//! and at PrepareForSleep(true) tells the pad ([`pad::Command::Sleep`]), waits for it, and lets the sleep
//! go on; after it ([`pad::Command::Woke`]: PrepareForSleep(false), or the clocks, should that signal not
//! come), takes the lock again.

use std::sync::mpsc;
use std::time::Duration;

use crate::dbus::{Bus, Message};
use crate::pad;
use crate::worker::{self, Worker};

use rustix::time::{ClockId, clock_gettime};

/// Time spent asleep since boot.
pub fn slept() -> Duration {
    let as_duration = |t: rustix::time::Timespec| Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec.clamp(0, 999_999_999) as u32);
    as_duration(clock_gettime(ClockId::Boottime)).saturating_sub(as_duration(clock_gettime(ClockId::Monotonic)))
}

/// Sleep shorter than this isn't counted (the two clocks are read one after the other).
const AT_LEAST: Duration = Duration::from_secs(1);

/// Notices a sleep between two checks.
#[derive(Debug)]
pub struct Watch {
    last: Duration,
}

impl Watch {
    pub fn new(slept: Duration) -> Self {
        Watch { last: slept }
    }

    /// With the time slept so far: whether the system slept since the last check.
    pub fn woke(&mut self, slept: Duration) -> bool {
        let woke = slept >= self.last + AT_LEAST;
        if woke || slept < self.last {
            self.last = slept;
        }
        woke
    }
}

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
/// How long the pad may take to close the gyro before the sleep goes on anyway.
const PAD_WAIT: Duration = Duration::from_secs(1);
/// After a failure (no bus, no lock), tried again this often.
const RETRY: Duration = Duration::from_secs(30);

/// The thread holding sleep for the pad (see the module doc).
pub fn spawn_hold(pad: Worker<pad::Command>) -> std::io::Result<()> {
    worker::detached("sleep", move || {
        loop {
            if let Err(e) = hold(&pad) {
                log::warn!("sleep: {e} (again in {}s)", RETRY.as_secs());
            }
            std::thread::sleep(RETRY);
        }
    })
}

/// logind's delay lock: sleep waits until it is closed (dropped).
fn lock(bus: &mut Bus) -> Result<::dbus::arg::OwnedFd, crate::dbus::Error> {
    let what = ("sleep", "ClamShell", "the gyro's stream closed first", "delay");
    bus.call::<(::dbus::arg::OwnedFd,)>(LOGIN1, LOGIN1_PATH, MANAGER, "Inhibit", what).map(|(fd,)| fd)
}

/// PrepareForSleep's argument: true before sleep, false after.
fn before_sleep(m: &Message) -> Option<bool> {
    (m.member().as_deref() == Some("PrepareForSleep")).then(|| m.read1::<bool>().ok()).flatten()
}

fn hold(pad: &Worker<pad::Command>) -> Result<(), crate::dbus::Error> {
    let mut bus = Bus::system()?;
    bus.add_match(&format!("type='signal',sender='{LOGIN1}',path='{LOGIN1_PATH}',interface='{MANAGER}',member='PrepareForSleep'"))?;
    let mut held = Some(lock(&mut bus)?);
    let mut watch = Watch::new(slept());
    loop {
        // (Once a second while asleep, for the clocks; else for the signal alone.)
        bus.wait(held.is_none().then_some(Duration::from_secs(1)))?;
        let signals: Vec<bool> = bus.signals()?.iter().filter_map(before_sleep).collect();
        let woke = signals.contains(&false) || (held.is_none() && watch.woke(slept()));
        if held.is_some() && signals.contains(&true) {
            let (done, wait) = mpsc::channel();
            pad.send(pad::Command::Sleep(pad::Ack(done)));
            if wait.recv_timeout(PAD_WAIT).is_err() {
                log::warn!("sleep: the pad didn't answer in time");
            }
            watch = Watch::new(slept());
            held = None;
        } else if held.is_none() && woke {
            pad.send(pad::Command::Woke);
            held = Some(lock(&mut bus)?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sleep_is_noticed_once() {
        let mut watch = Watch::new(Duration::from_secs(100));
        assert!(!watch.woke(Duration::from_millis(100_400)), "jitter");
        assert!(watch.woke(Duration::from_secs(160)));
        assert!(!watch.woke(Duration::from_secs(160)));
        assert!(watch.woke(Duration::from_secs(170)));
    }

    #[test]
    fn the_clocks_can_be_read() {
        assert!(slept() < Duration::from_secs(10 * 365 * 24 * 3600));
    }
}
