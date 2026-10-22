//! Sleep. Suspending is done by logind, triggered by the power button and the lid per logind.conf, and by
//! the Sleep action ([`suspend`]), which first checks that the machine isn't busy ([`busy`]). While the
//! lid is closed the power key doesn't wake the device (see [`crate::power::lid`]). Waking is noticed
//! from the clocks ([`crate::time::wake`]).
//!
//! Before sleep, the pad thread closes the gyro's stream. Otherwise the Thor's sensor hub keeps sampling
//! on the ADSP while asleep, as PB-OS's pbos-motiond found. [`spawn_hold`] keeps a logind delay lock. At
//! PrepareForSleep(true) it tells the pad ([`pad::Command::Sleep`]), waits for it, and lets the sleep go
//! on. After the sleep it sends [`pad::Command::Woke`] and takes the lock again. The wake is noticed
//! from PrepareForSleep(false), or from the clocks in case that signal doesn't come.

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
/// If the lock has been released this long (on the monotonic clock) without the system sleeping, the
/// sleep was given up.
const NOT_SLEPT: Duration = Duration::from_secs(10);
/// How long to wait before trying again after a failure, such as no bus or no lock.
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

/// Takes logind's delay lock. Sleep waits until it is closed by dropping it.
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
    // Starting anew, perhaps after a failure while released, so reopen the gyro if it was closed.
    pad.send(pad::Command::Woke);
    let mut watch = Watch::new(slept());
    let mut released = Instant::now();
    loop {
        // While released, wake once a second to check the clocks. Otherwise wait for the signal alone.
        bus.wait(held.is_none().then_some(Duration::from_secs(1)))?;
        let mut woke = false;
        // Handle the signals in order: a sleep given up at once (true, then false) counts as a wake.
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
                // Woken and asleep again at once, before the lock was taken, so it stays released.
                woke = false;
            } else {
                woke = true;
            }
        }
        // Without the signal, the clocks may say it slept. Or it has been released a while without
        // sleeping: CLOCK_MONOTONIC stops while asleep, so this much of it means the sleep didn't happen.
        if held.is_none() && (woke || watch.woke(slept()) || released.elapsed() > NOT_SLEPT) {
            pad.send(pad::Command::Woke);
            held = Some(lock(&mut bus)?);
        }
    }
}

/// Above this much CPU, in cores, for the user's processes, or this many bytes a second on the network,
/// the machine is busy and isn't put to sleep.
const BUSY_CPU: f64 = 0.3;
const BUSY_NET: f64 = 100_000.0;

/// Whether the machine is busy now, measured over a few seconds, and why. It looks at the CPU use of all
/// the user's processes, meaning the user manager's cgroup with ClamShell's launches and Steam, and at
/// the network's traffic.
pub fn busy() -> Option<String> {
    let user = user_cgroup();
    let cpu = || user.as_deref().and_then(cgroup::cpu_usec);
    let net = || {
        let text = sys::text("/proc/net/dev").unwrap_or_default();
        // For each interface except loopback: received bytes, the 1st number, and sent, the 9th.
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

/// The user manager's cgroup (`user@<uid>.service`): the cgroup ClamShell runs in, cut off there.
fn user_cgroup() -> Option<PathBuf> {
    let path = cgroup::path_of(std::process::id())?;
    let end = path.find(".service/").map(|i| i + ".service".len())?;
    Some(cgroup::dir(path.get(..end)?))
}

/// Suspends through logind. Where the system can, because it has a swap file to hibernate to, it
/// suspends and then hibernates after a while (logind's HibernateDelaySec). Otherwise it just suspends.
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
    /// On a desktop, prints whether the machine is busy now and the cgroup looked at. Run it by hand with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore = "reads this machine"]
    fn busy_now() {
        println!("{:?}: {:?}", super::user_cgroup(), super::busy());
    }
}
