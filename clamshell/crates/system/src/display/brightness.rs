//! The screens' brightness. Each backlight is set to the level times its share, through logind, which
//! lets the session's user set it without root (sysfs is root's), or for a stand-in by writing its
//! files. It runs on a thread of its own, so a stuck PipeWire in the sound thread doesn't hold it up.
//! While a finger slides, only the latest level is set.

use std::io;
use std::path::{Path, PathBuf};

use clamshell_state::levels::Share;

use crate::os::dbus::Kept;
use crate::os::sys;
use crate::os::worker::{self, Quiet, Worker};

/// The backlights: the devices of a sysfs class directory. That is /sys/class/backlight, or another
/// directory laid out the same way for tests and for desktops without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Backlights {
    dir: PathBuf,
}

impl Backlights {
    pub(crate) fn new(dir: PathBuf) -> Backlights {
        Backlights { dir }
    }

    fn devices(&self) -> Vec<PathBuf> {
        sys::entries(&self.dir)
    }

    /// The backlights' names, in order.
    pub(crate) fn names(&self) -> Vec<String> {
        self.devices().iter().filter_map(|dir| Some(dir.file_name()?.to_str()?.to_owned())).collect()
    }

    /// The first backlight's brightness, 0-100, or `None` without a backlight.
    pub(crate) fn level(&self) -> Option<u8> {
        let dir = self.devices().into_iter().next()?;
        let max = sys::read::<u32>(dir.join("max_brightness"))?.max(1);
        let now = sys::read::<u32>(dir.join("brightness"))?;
        Some((f64::from(now) * 100.0 / f64::from(max)).round().clamp(0.0, 100.0) as u8)
    }
}

/// Sets each backlight to `level` times its share, both 0-100. Shares are by backlight name, and a
/// backlight not listed has a share of 100.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) level: u8,
    pub(crate) shares: Vec<Share>,
}

/// A level times a share, both 0-100, as a backlight's raw value. It is never 0, which turns some panels
/// off.
fn raw(level: u8, share: u8, max: u32) -> u32 {
    let share = f64::from(level.min(100)) / 100.0 * f64::from(share.min(100)) / 100.0;
    ((f64::from(max) * share).round() as u32).clamp(1, max.max(1))
}

/// Starts the thread that sets the brightness. Only the latest of the waiting commands is carried out.
pub(crate) fn spawn(backlights: Backlights) -> io::Result<Worker<Command>> {
    let mut setter = Setter::default();
    let system = backlights.dir == Path::new(SYSTEM);
    worker::spawn_coalescing(
        clamshell_state::Thread::Brightness.name(),
        |_| (),
        move |Command { level, shares }| {
            for dir in backlights.devices() {
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let share = shares.iter().find(|s| s.backlight == name).map_or(100, |s| s.share);
                setter.set(&dir, level, share, system);
            }
        },
    )
}

/// The system's backlights, which only root may write.
const SYSTEM: &str = "/sys/class/backlight";

/// Sets the system's backlights through logind's session, which lets the session's user set them, and a
/// stand-in's by writing its files. The logind connection is kept, and made again after the bus fails.
struct Setter {
    logind: Kept,
    bus: Quiet,
    set: Quiet,
}

impl Default for Setter {
    fn default() -> Setter {
        Setter { logind: Kept::system(), bus: Quiet::default(), set: Quiet::default() }
    }
}

impl Setter {
    fn set(&mut self, dir: &Path, level: u8, share: u8, system: bool) {
        let (Some(name), Some(max)) = (dir.file_name().and_then(|n| n.to_str()), sys::read::<u32>(dir.join("max_brightness"))) else {
            return;
        };
        let value = raw(level, share, max);
        if !system {
            match std::fs::write(dir.join("brightness"), value.to_string()) {
                Ok(()) => self.set.ok(),
                Err(e) => self.set.failed(&format!("brightness {name}"), &e),
            }
            return;
        }
        let conn = match self.logind.get() {
            Ok(conn) => {
                self.bus.ok();
                conn
            }
            Err(e) => return self.bus.failed("brightness: system bus", &e),
        };
        let set = conn.call::<()>(
            "org.freedesktop.login1",
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
            "SetBrightness",
            ("backlight", name, value),
        );
        match set {
            Ok(()) => self.set.ok(),
            Err(e) => {
                self.logind.failed(&e);
                if powered_down(dir) {
                    // A panel that is off can't take the level, as when the bottom screen is off. The kernel
                    // keeps it and sets it as the panel comes on, so nothing is lost.
                    log::debug!("brightness {name} through logind, its panel off: {e}");
                } else {
                    self.set.failed(&format!("brightness {name} through logind"), &e);
                }
            }
        }
    }
}

/// Whether the backlight's panel is powered down (`bl_power` other than 0, as the kernel sets it while
/// the panel is off).
fn powered_down(dir: &Path) -> bool {
    sys::read::<u32>(dir.join("bl_power")).is_some_and(|power| power != 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn levels_as_raw_values_never_turn_the_panel_off() {
        assert_eq!(raw(100, 100, 4095), 4095);
        assert_eq!(raw(50, 100, 255), 128);
        assert_eq!(raw(50, 50, 400), 100, "the level times the share");
        assert_eq!(raw(0, 100, 255), 1);
        assert_eq!(raw(5, 5, 255), 1, "never off");
        assert_eq!(raw(200, 200, 10), 10);
    }

    #[test]
    fn backlights_by_name_and_the_first_ones_level() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for (name, now) in [("b", 10), ("a", 128)] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("max_brightness"), "255\n").unwrap();
            std::fs::write(dir.join(name).join("brightness"), format!("{now}\n")).unwrap();
        }
        let backlights = Backlights::new(dir.to_path_buf());
        assert_eq!(backlights.names(), vec!["a", "b"]);
        assert_eq!(backlights.level(), Some(50));
        assert_eq!(Backlights::new(dir.join("none")).level(), None);
    }

    #[test]
    fn a_stand_in_is_written_and_a_panel_off_is_known() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_brightness"), "255\n").unwrap();
        std::fs::write(dir.join("bl_power"), "0\n").unwrap();
        Setter::default().set(&dir, 50, 100, false);
        assert_eq!(std::fs::read_to_string(dir.join("brightness")).unwrap(), "128");
        assert!(!powered_down(&dir));
        std::fs::write(dir.join("bl_power"), "4\n").unwrap();
        assert!(powered_down(&dir));
        assert!(!powered_down(&tmp.path().join("none")), "unknown: taken as on");
    }
}
