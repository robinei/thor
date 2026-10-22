//! Sleep. Suspending is logind's: the power button and the lid, per logind.conf (the power key not
//! waking it while the lid is closed: [`crate::power::lid`]), and the Sleep action ([`suspend`]), which
//! first checks that the machine isn't busy ([`busy`]). Waking is noticed from the clocks
//! ([`crate::time::wake`]).
//!
//! Before sleep, the pad thread closes the gyro's stream (the Thor's sensor hub samples on the ADSP while
//! asleep otherwise, as PB-OS's pbos-motiond found): [`spawn_hold`] keeps a logind delay lock, and at
//! PrepareForSleep(true) tells the pad ([`pad::Command::Sleep`]), waits for it, and lets the sleep go
//! on; after it ([`pad::Command::Woke`]: PrepareForSleep(false), or the clocks, should that signal not
//! come), takes the lock again.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::input::pad;
use crate::os::dbus::{self, Bus, Message};
use crate::os::worker::{self, Worker};
use crate::os::{cgroup, sys};
use crate::time::wake::{Watch, slept};

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
/// How long the pad may take to close the gyro before the sleep goes on anyway.
const PAD_WAIT: Duration = Duration::from_secs(1);
/// Released this long (monotonic) without the system sleeping: it didn't (a sleep given up).
const NOT_SLEPT: Duration = Duration::from_secs(10);
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
fn lock(bus: &mut Bus) -> Result<::dbus::arg::OwnedFd, crate::os::dbus::Error> {
    let what = ("sleep", "ClamShell", "the gyro's stream closed first", "delay");
    bus.call::<(::dbus::arg::OwnedFd,)>(LOGIN1, LOGIN1_PATH, MANAGER, "Inhibit", what).map(|(fd,)| fd)
}

/// PrepareForSleep's argument: true before sleep, false after.
fn before_sleep(m: &Message) -> Option<bool> {
    (m.member().as_deref() == Some("PrepareForSleep"))
        .then(|| m.read1::<bool>().inspect_err(|e| log::warn!("sleep: reading PrepareForSleep: {e}")).ok())
        .flatten()
}

fn hold(pad: &Worker<pad::Command>) -> Result<(), crate::os::dbus::Error> {
    let mut bus = Bus::system()?;
    bus.add_match(&format!("type='signal',sender='{LOGIN1}',path='{LOGIN1_PATH}',interface='{MANAGER}',member='PrepareForSleep'"))?;
    let mut held = Some(lock(&mut bus)?);
    // (Started anew, perhaps after a failure while released: the gyro found, if it was closed.)
    pad.send(pad::Command::Woke);
    let mut watch = Watch::new(slept());
    let mut released = Instant::now();
    loop {
        // (Once a second while released, for the clocks; else for the signal alone.)
        bus.wait(held.is_none().then_some(Duration::from_secs(1)))?;
        let mut woke = false;
        // In their order: a sleep given up at once (true, then false) is a wake.
        for before in bus.signals()?.iter().filter_map(before_sleep) {
            if before && held.is_some() {
                let (done, wait) = mpsc::channel();
                pad.send(pad::Command::Sleep(pad::Ack(done)));
                if wait.recv_timeout(PAD_WAIT).is_err() {
                    log::warn!("sleep: the pad didn't answer in time");
                }
                watch = Watch::new(slept());
                released = Instant::now();
                held = None;
                woke = false;
            } else if before {
                // (Woken and asleep again at once, before the lock was taken: still released.)
                woke = false;
            } else {
                woke = true;
            }
        }
        // Without the signal: the clocks say it slept; or it has been released a while without sleeping
        // (CLOCK_MONOTONIC stops while asleep: this much of it means the sleep didn't happen).
        if held.is_none() && (woke || watch.woke(slept()) || released.elapsed() > NOT_SLEPT) {
            pad.send(pad::Command::Woke);
            held = Some(lock(&mut bus)?);
        }
    }
}

/// Sleep: suspend, then hibernate after a while (logind's HibernateDelaySec) if it can, else suspend.
/// Above this much CPU (cores) for the user's processes, or this many bytes a second on the network, the
/// machine is busy: not put to sleep.
const BUSY_CPU: f64 = 0.3;
const BUSY_NET: f64 = 100_000.0;

/// Whether the machine is busy now (looked at for a few seconds): the user's processes' CPU (the user
/// manager's cgroup, all of them: ClamShell's launches, Steam), or the network's traffic; why.
pub fn busy() -> Option<String> {
    let user = user_cgroup();
    let cpu = || user.as_deref().and_then(cgroup::cpu_usec);
    let net = || {
        let text = sys::text("/proc/net/dev").unwrap_or_default();
        // (Each interface but loopback: received bytes, the 1st number, and sent, the 9th.)
        text.lines()
            .skip(2)
            .filter_map(|l| l.split_once(':'))
            .filter(|(name, _)| name.trim() != "lo")
            .map(|(_, v)| {
                let n: Vec<u64> = v.split_whitespace().filter_map(|x| x.parse().inspect_err(|e| log::debug!("busy: /proc/net/dev: {x:?}: {e}")).ok()).collect();
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
    let path = cgroup::path_of(std::process::id())?;
    let end = path.find(".service/").map(|i| i + ".service".len())?;
    Some(cgroup::dir(path.get(..end)?))
}

/// Suspends through logind: suspend-then-hibernate where the system can (a swap file to hibernate to),
/// else suspend.
pub fn suspend() {
    let logind = |bus: &mut dbus::Bus, method: &str| bus.call::<(String,)>(LOGIN1, LOGIN1_PATH, MANAGER, method, ());
    let slept = dbus::Bus::system().and_then(|mut bus| {
        let (method, interactive) = if logind(&mut bus, "CanSuspendThenHibernate")?.0 == "yes" {
            log::info!("sleep: suspend, then hibernate");
            ("SuspendThenHibernate", false)
        } else {
            log::info!("sleep: suspend");
            ("Suspend", false)
        };
        bus.call::<()>(LOGIN1, LOGIN1_PATH, MANAGER, method, (interactive,))
    });
    if let Err(e) = slept {
        log::warn!("sleep: {e}");
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
