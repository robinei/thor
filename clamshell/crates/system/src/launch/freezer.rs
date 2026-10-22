//! Freezing a hidden game's slice (the cgroup freezer, through systemd), and thawing it when it shows.

use std::thread;
use std::time::{Duration, Instant};

use super::scopes::{SYSTEMD, call, get_unit, with_manager};
use crate::os::cgroup;
use crate::os::dbus::Bus;
use crate::os::worker;

/// Freezes a unit (a slice: every scope in it), or thaws it: the cgroup freezer, which stops its
/// processes where they are, without their knowing. Done by the freezer thread, one unit after another,
/// each unit's latest request only (thaws first). A freeze waits while the slice works without drawing
/// ([`working_without_drawing`]: loading, compiling shaders, which the user switched away to wait for);
/// a later request for the unit (shown again: thawed) ends the wait.
pub fn freeze(unit: String, frozen: bool) {
    static FREEZER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<(String, bool)>>> = std::sync::OnceLock::new();
    let sender = FREEZER.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel();
        worker::detached("freezer", move || freezer(&receive)).map_err(|e| log::warn!("freezer: {e}")).ok().map(|()| send)
    });
    if sender.as_ref().is_none_or(|s| s.send((unit, frozen)).is_err()) {
        log::warn!("freezer: gone");
    }
}

/// What the freezer does to units: systemd's, or a test's.
trait Freezer {
    fn thaw(&mut self, unit: &str);
    fn freeze(&mut self, unit: &str);
    /// Whether the unit works without drawing (looked at for a while: see [`working_without_drawing`]).
    fn busy(&mut self, unit: &str) -> bool;
}

/// The user manager's units.
struct Systemd;

impl Freezer for Systemd {
    fn thaw(&mut self, unit: &str) {
        match with_manager(|bus| Ok(call::<()>(bus, "ThawUnit", (unit,))?)) {
            Ok(()) => log::info!("thawed {unit}"),
            Err(e) => log::warn!("thawing {unit}: {e}"),
        }
    }

    fn freeze(&mut self, unit: &str) {
        match with_manager(|bus| Ok(call::<()>(bus, "FreezeUnit", (unit,))?)) {
            Ok(()) => log::info!("froze {unit}"),
            Err(e) => log::warn!("freezing {unit}: {e}"),
        }
    }

    fn busy(&mut self, unit: &str) -> bool {
        let group = with_manager(|bus| Ok(control_group(bus, unit))).ok().flatten();
        working_without_drawing(group)
    }
}

/// The freezer thread: carried out by systemd (see [`follow_requests`]).
fn freezer(requests: &std::sync::mpsc::Receiver<(String, bool)>) {
    follow_requests(requests, &mut Systemd);
}

/// The requests waiting, each unit's latest, carried out by `units` until none is left: thaws at once; a
/// freeze once its unit isn't busy, unless asked otherwise meanwhile (a busy one looked at again after
/// the others). Returns when the requests' sender is gone.
fn follow_requests(requests: &std::sync::mpsc::Receiver<(String, bool)>, units: &mut impl Freezer) {
    let mut waiting: Vec<(String, bool)> = Vec::new();
    // (Those a freeze waits for, said once each.)
    let mut said: Vec<String> = Vec::new();
    let take = |waiting: &mut Vec<(String, bool)>, (unit, frozen): (String, bool)| {
        waiting.retain(|(u, _)| *u != unit);
        waiting.push((unit, frozen));
    };
    loop {
        if waiting.is_empty() {
            match requests.recv() {
                Ok(r) => take(&mut waiting, r),
                Err(_) => return,
            }
        }
        while let Ok(r) = requests.try_recv() {
            take(&mut waiting, r);
        }
        for (unit, _) in waiting.iter().filter(|(_, frozen)| !frozen) {
            units.thaw(unit);
            said.retain(|u| u != unit);
        }
        waiting.retain(|(_, frozen)| *frozen);
        let Some((unit, _)) = waiting.first().cloned() else {
            continue;
        };
        let busy = units.busy(&unit);
        while let Ok(r) = requests.try_recv() {
            take(&mut waiting, r);
        }
        if !waiting.iter().any(|(u, frozen)| *u == unit && *frozen) {
            log::info!("{unit}: not frozen (asked otherwise since)");
            continue;
        }
        if busy {
            if !said.contains(&unit) {
                log::info!("{unit} works without drawing (loading?): frozen once it doesn't");
                said.push(unit.clone());
            }
            waiting.rotate_left(1);
            continue;
        }
        waiting.retain(|(u, _)| *u != unit);
        said.retain(|u| *u != unit);
        units.freeze(&unit);
    }
}

/// Above this CPU (cores) with the GPU below [`DRAWING_GPU`] (of its time), a slice works without
/// drawing.
const BUSY_CPU: f64 = 0.5;

const DRAWING_GPU: f64 = 0.05;

/// How long a slice is watched for that.
const WATCH: Duration = Duration::from_secs(2);

/// A slice's cgroup, under /sys/fs/cgroup.
fn control_group(bus: &mut Bus, unit: &str) -> Option<String> {
    let path = get_unit(bus, unit).ok()?;
    bus.get::<String>(SYSTEMD, &path, "org.freedesktop.systemd1.Slice", "ControlGroup").ok().filter(|g| !g.is_empty())
}

/// Whether the slice with cgroup `group` works without drawing, over [`WATCH`]: much CPU, the GPU (its
/// processes' DRM clients) nearly idle; a game hidden but drawing on uses the GPU. Unreadable: not.
fn working_without_drawing(group: Option<String>) -> bool {
    let Some(group) = group else {
        thread::sleep(WATCH);
        return false;
    };
    let dir = cgroup::dir(&group);
    let sample = || (cgroup::cpu_usec(&dir), cgroup::gpu_ns(&dir), Instant::now());
    let (cpu0, gpu0, t0) = sample();
    thread::sleep(WATCH);
    let (cpu1, gpu1, t1) = sample();
    let (Some(cpu0), Some(cpu1)) = (cpu0, cpu1) else {
        return false;
    };
    let wall = t1.duration_since(t0).as_secs_f64();
    let cpu = cpu1.saturating_sub(cpu0) as f64 / 1e6 / wall;
    let gpu = gpu1.saturating_sub(gpu0) as f64 / 1e9 / wall;
    cpu > BUSY_CPU && gpu < DRAWING_GPU
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Sender, channel};

    /// Units in a test: what was done to them, in order; busy ones stay busy for as many looks as given,
    /// and a look may let more requests in (as the 2 s look would).
    #[derive(Default)]
    struct Fake {
        done: Vec<String>,
        busy: Vec<(String, u32)>,
        during_look: Vec<(String, bool)>,
        send: Option<Sender<(String, bool)>>,
    }

    impl Freezer for Fake {
        fn thaw(&mut self, unit: &str) {
            self.done.push(format!("thaw {unit}"));
        }
        fn freeze(&mut self, unit: &str) {
            self.done.push(format!("freeze {unit}"));
        }
        fn busy(&mut self, unit: &str) -> bool {
            // (Sent once, then the sender dropped: the loop ends when the requests are done.)
            if let Some(send) = self.send.take() {
                for r in self.during_look.drain(..) {
                    let _ = send.send(r);
                }
            }
            match self.busy.iter_mut().find(|(u, _)| u == unit) {
                Some((_, n)) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            }
        }
    }

    fn run(requests: &[(&str, bool)], mut fake: Fake) -> Vec<String> {
        let (send, receive) = channel();
        for (u, f) in requests {
            let _ = send.send(((*u).to_owned(), *f));
        }
        // (Kept by the fake only to send more during a look; else dropped: the loop ends when the requests
        // are done.)
        if fake.during_look.is_empty() {
            drop(send);
        } else {
            fake.send = Some(send);
        }
        follow_requests(&receive, &mut fake);
        fake.done
    }

    #[test]
    fn each_units_latest_request_thaws_first() {
        // a: freeze then thaw: only the thaw; b: frozen.
        let done = run(&[("a", true), ("b", true), ("a", false)], Fake::default());
        assert_eq!(done, ["thaw a", "freeze b"]);
    }

    #[test]
    fn a_busy_unit_waits_and_a_thaw_meanwhile_cancels_its_freeze() {
        // a busy for two looks, b not: b frozen first, then a.
        let fake = Fake { busy: vec![("a".into(), 2)], ..Fake::default() };
        assert_eq!(run(&[("a", true), ("b", true)], fake), ["freeze b", "freeze a"]);
        // a busy, and shown during the look (thawed): never frozen.
        let fake = Fake { busy: vec![("a".into(), 1)], during_look: vec![("a".into(), false)], ..Fake::default() };
        assert_eq!(run(&[("a", true)], fake), ["thaw a"]);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command;
    use std::thread;

    /// A slice working without drawing (a busy loop, no GPU) isn't frozen while it does; once idle, it is
    /// (run by hand: `--ignored`).
    #[test]
    #[ignore = "needs a user session; starts scopes"]
    #[allow(clippy::unwrap_used)]
    fn a_freeze_waits_while_the_slice_works_without_drawing() {
        let id = std::process::id();
        let slice = format!("{}busytest_{id}.slice", clamshell_core::session::instance::SLICE_PREFIX);
        let state = || {
            let out = Command::new("systemctl").args(["--user", "show", "-p", "FreezerState", "--value", &slice]).output().unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        let busy = format!("busytest-{id}-busy");
        let mut looping = Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--scope", &format!("--slice={slice}"), &format!("--unit={busy}"), "sh", "-c", "while :; do :; done"])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(1));
        freeze(slice.clone(), true);
        thread::sleep(Duration::from_secs(6));
        assert_eq!(state(), "running", "busy without drawing: not yet");
        // Idle (the loop gone, a sleep left): frozen.
        let _ = Command::new("systemctl").args(["--user", "stop", &format!("{busy}.scope")]).status();
        let _ = looping.wait();
        let mut sleeping = Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--scope", &format!("--slice={slice}"), &format!("--unit=busytest-{id}-idle"), "sleep", "30"])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
        assert_eq!(state(), "frozen");
        freeze(slice.clone(), false);
        thread::sleep(Duration::from_secs(1));
        assert_eq!(state(), "running");
        let _ = Command::new("systemctl").args(["--user", "stop", &slice]).status();
        let _ = sleeping.wait();
    }
}
