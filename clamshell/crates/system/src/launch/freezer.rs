//! Freezing a hidden game's slice with the cgroup freezer through systemd, and thawing it when it shows.

use std::thread;
use std::time::{Duration, Instant};

use super::units::{SYSTEMD, call, get_unit, with_manager_on};
use crate::os::cgroup;
use crate::os::dbus::{Bus, Kept};
use crate::os::worker::Worker;

/// A request to the freezer: freeze (true) or thaw a unit. For a slice that means every unit in it.
pub(crate) type Request = (String, bool);

/// Starts the freezer thread. The cgroup freezer stops the processes where they are, without their
/// knowing. The thread does this one unit after another, acting only on each unit's latest request, thaws
/// first. A freeze waits while the slice works without drawing ([`working_without_drawing`]), for example
/// while it loads or compiles shaders and the user has switched away to wait. A later request for the
/// unit, such as a thaw when it is shown again, ends the wait.
pub(crate) fn spawn() -> std::io::Result<Worker<Request>> {
    Worker::spawn(clamshell_state::Thread::Freezer.name(), |requests| follow_requests(requests, &mut Systemd { bus: Kept::session() }))
}

/// What the freezer does to units. It is implemented for systemd's units and for a test's.
trait Freezer {
    fn thaw(&mut self, unit: &str);
    fn freeze(&mut self, unit: &str);
    /// Whether the unit works without drawing, watched for a while. See [`working_without_drawing`].
    fn busy(&mut self, unit: &str) -> bool;
}

/// The user manager's units, over a connection of the freezer's own: FreezeUnit is answered once every
/// process has stopped, which a process stuck in the kernel (in D state) holds up for the call's whole
/// timeout, and over the shared connection that would hold up launches meanwhile.
struct Systemd {
    bus: Kept,
}

impl Freezer for Systemd {
    fn thaw(&mut self, unit: &str) {
        match with_manager_on(&mut self.bus, |bus| Ok(call::<()>(bus, "ThawUnit", (unit,))?)) {
            Ok(()) => log::info!("thawed {unit}"),
            Err(e) => log::warn!("thawing {unit}: {e}"),
        }
    }

    fn freeze(&mut self, unit: &str) {
        match with_manager_on(&mut self.bus, |bus| Ok(call::<()>(bus, "FreezeUnit", (unit,))?)) {
            Ok(()) => log::info!("froze {unit}"),
            Err(e) => log::warn!("freezing {unit}: {e}"),
        }
    }

    fn busy(&mut self, unit: &str) -> bool {
        let group = with_manager_on(&mut self.bus, |bus| control_group(bus, unit))
            .inspect_err(|e| log::warn!("freezer: {unit}'s cgroup: {e}: it is frozen without looking whether it works without drawing"))
            .ok()
            .flatten();
        working_without_drawing(group)
    }
}

/// Carries out the waiting requests on `units`, each unit's latest, until none is left. Thaws happen at
/// once. A freeze happens once its unit isn't busy, unless a different request comes in the meantime. A
/// busy unit is looked at again after the others. Returns when the requests' sender is gone.
fn follow_requests(requests: &std::sync::mpsc::Receiver<(String, bool)>, units: &mut impl Freezer) {
    let mut waiting: Vec<(String, bool)> = Vec::new();
    // The units a freeze is waiting for, so each is logged once.
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

/// A slice using more CPU than this, in cores, while its GPU use is below [`DRAWING_GPU`] is working
/// without drawing.
const BUSY_CPU: f64 = 0.5;

const DRAWING_GPU: f64 = 0.05;

/// How long a slice is watched to decide that.
const WATCH: Duration = Duration::from_secs(2);

/// A slice's cgroup, under /sys/fs/cgroup. `None` if it has none, as a slice that isn't running.
fn control_group(bus: &mut Bus, unit: &str) -> Result<Option<String>, super::units::Error> {
    let path = get_unit(bus, unit)?;
    let group = bus.get::<String>(SYSTEMD, &path, "org.freedesktop.systemd1.Slice", "ControlGroup")?;
    Ok(Some(group).filter(|g| !g.is_empty()))
}

/// Whether the slice with cgroup `group` works without drawing over [`WATCH`]. That means much CPU while
/// the GPU, as its processes' DRM clients, is nearly idle. A hidden game that keeps drawing uses the GPU.
/// If the figures can't be read, the answer is no.
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
        log::warn!("freezer: {}'s cpu.stat unread: it is frozen without looking whether it works without drawing", dir.display());
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

    /// Units in a test. It records what was done to them, in order. Busy units stay busy for as many
    /// looks as given, and a look may let more requests in, as the real 2 s look would.
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
            // Send these once, then drop the sender so the loop ends when the requests are done.
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
        // The fake keeps the sender only to send more during a look. Otherwise it is dropped, so the loop
        // ends when the requests are done.
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
        // a is frozen then thawed, so only the thaw happens. b is frozen.
        let done = run(&[("a", true), ("b", true), ("a", false)], Fake::default());
        assert_eq!(done, ["thaw a", "freeze b"]);
    }

    #[test]
    fn a_busy_unit_waits_and_a_thaw_meanwhile_cancels_its_freeze() {
        // a is busy for two looks and b isn't, so b is frozen first, then a.
        let fake = Fake { busy: vec![("a".into(), 2)], ..Fake::default() };
        assert_eq!(run(&[("a", true), ("b", true)], fake), ["freeze b", "freeze a"]);
        // a is busy and is shown (thawed) during the look, so it is never frozen.
        let fake = Fake { busy: vec![("a".into(), 1)], during_look: vec![("a".into(), false)], ..Fake::default() };
        assert_eq!(run(&[("a", true)], fake), ["thaw a"]);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::process::Command;
    use std::thread;

    /// A slice working without drawing, a busy loop with no GPU, isn't frozen while it does. Once idle it
    /// is. Run it by hand with `--ignored`.
    #[test]
    #[ignore = "needs a user session; starts scopes"]
    #[allow(clippy::unwrap_used)]
    fn a_freeze_waits_while_the_slice_works_without_drawing() {
        let id = std::process::id();
        let slice = format!("{}busytest_{id}.slice", clamshell_state::session::launch_unit::SLICE_PREFIX);
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
        let freezer = spawn().unwrap();
        freezer.send((slice.clone(), true));
        thread::sleep(Duration::from_secs(6));
        assert_eq!(state(), "running", "busy without drawing: not yet");
        // Once idle, with the loop gone and a sleep left, it is frozen.
        let _ = Command::new("systemctl").args(["--user", "stop", &format!("{busy}.scope")]).status();
        let _ = looping.wait();
        let mut sleeping = Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--scope", &format!("--slice={slice}"), &format!("--unit=busytest-{id}-idle"), "sleep", "30"])
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(5));
        assert_eq!(state(), "frozen");
        freezer.send((slice.clone(), false));
        thread::sleep(Duration::from_secs(1));
        assert_eq!(state(), "running");
        let _ = Command::new("systemctl").args(["--user", "stop", &slice]).status();
        let _ = sleeping.wait();
    }
}
