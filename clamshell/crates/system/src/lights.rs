//! The stick lights: what the state says they show ([`clamshell_state::Effect::Lights`]) written to the
//! LEDs the device profile names ([`StickLights`]), on a thread of their own. The state sends them only
//! when they change, and again after sleep, which turns them off, so each is written as it comes; while
//! a slider moves only the latest is written. A device without lights, or LEDs that can't be written,
//! is logged once. When ClamShell ends the lights are left off.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use clamshell_device::lights::StickLights;
use clamshell_state::lights::Lit;

use crate::os::sys;
use crate::os::worker::{self, Quiet, Worker};

/// The lights' thread, and the LEDs it writes, which are turned off when this is dropped.
pub(crate) struct Lights {
    worker: Worker<[Lit; 2]>,
    leds: Arc<Mutex<Leds>>,
}

impl Lights {
    /// Starts the thread that writes the LEDs of `lights` (none on a device without them) under `root`,
    /// /sys or a stand-in laid out the same way. If it can't start, that is logged, and the lights are
    /// still turned off at the end.
    pub(crate) fn start(root: PathBuf, lights: Option<StickLights>) -> Lights {
        let leds = Arc::new(Mutex::new(Leds { root, lights, ended: false, quiet: Quiet::default(), said_none: false }));
        let writer = leds.clone();
        let name = clamshell_state::Thread::Lights.name();
        let worker = worker::spawn_coalescing(name, |_| (), move |lit| lock(&writer).write(lit)).unwrap_or_else(|e| {
            log::error!("lights thread: {e}");
            Worker::gone(name)
        });
        Lights { worker, leds }
    }

    pub(crate) fn send(&self, lit: [Lit; 2]) {
        self.worker.send(lit);
    }
}

/// ClamShell is ending: the lights are turned off here, and the thread writes nothing after.
impl Drop for Lights {
    fn drop(&mut self) {
        let mut leds = lock(&self.leds);
        leds.write([Lit::OFF; 2]);
        leds.ended = true;
    }
}

fn lock(leds: &Mutex<Leds>) -> MutexGuard<'_, Leds> {
    // A panic while writing leaves nothing half-done that matters: the next write sets every LED.
    leds.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The LEDs and what was said about them.
struct Leds {
    root: PathBuf,
    lights: Option<StickLights>,
    /// ClamShell ended and the lights were left off.
    ended: bool,
    quiet: Quiet,
    /// That the device has no lights was logged.
    said_none: bool,
}

impl Leds {
    /// Writes each stick's LEDs, left then right. The first failure is logged as a warning, the rest at
    /// debug until a write succeeds again.
    fn write(&mut self, [left, right]: [Lit; 2]) {
        if self.ended {
            return;
        }
        let Some(lights) = self.lights else {
            if !std::mem::replace(&mut self.said_none, true) {
                log::info!("lights: the device has no stick lights; nothing is written");
            }
            return;
        };
        let mut failed = None;
        for (dirs, lit) in [(lights.left, left), (lights.right, right)] {
            for dir in dirs {
                let dir = self.root.join(dir);
                if let Err(e) = write_led(&dir, &lights, lit) {
                    failed.get_or_insert((dir, e));
                }
            }
        }
        match failed {
            Some((dir, e)) => self.quiet.failed(&format!("lights: {}", dir.display()), &e),
            None => self.quiet.ok(),
        }
    }
}

/// Writes `lit` to the LED whose class directory is `dir`: its colour to `multi_intensity` in the
/// device's channel order, then its brightness.
fn write_led(dir: &Path, lights: &StickLights, lit: Lit) -> io::Result<()> {
    let max = sys::read::<u32>(dir.join("max_brightness")).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no max_brightness"))?;
    let ([a, b, c], brightness) = values(lights, lit, max);
    std::fs::write(dir.join("multi_intensity"), format!("{a} {b} {c}"))?;
    std::fs::write(dir.join("brightness"), brightness.to_string())
}

/// What an LED is written for `lit`: `multi_intensity`'s values, the colour at full brightness in the
/// device's channel order, and `brightness`, the percent of `max`. Off is all zeros.
fn values(lights: &StickLights, lit: Lit, max: u32) -> ([u8; 3], u32) {
    if lit.brightness == 0 {
        return ([0; 3], 0);
    }
    let brightness = (f64::from(max) * f64::from(lit.brightness.min(100)) / 100.0).round() as u32;
    // Some light for any brightness above 0.
    (lights.written(lit.rgb), brightness.clamp(1, max.max(1)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_written_in_the_devices_channel_order_and_its_brightness_scaled_to_the_leds() {
        let thor = StickLights::thor();
        // Red at full brightness, as ROCKNIX writes it: 0 0 255, brightness 255.
        assert_eq!(values(&thor, Lit { rgb: [255, 0, 0], brightness: 100 }, 255), ([0, 0, 255], 255));
        assert_eq!(values(&thor, Lit { rgb: [255, 20, 0], brightness: 40 }, 255), ([0, 20, 255], 102));
        assert_eq!(values(&thor, Lit { rgb: [0, 0, 255], brightness: 1 }, 50), ([255, 0, 0], 1), "never 0 while lit");
        assert_eq!(values(&thor, Lit::OFF, 255), ([0; 3], 0));
    }

    /// Each stick's LEDs are written under the root, and turned off when ClamShell ends.
    #[test]
    fn the_leds_are_written_and_left_off_at_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let thor = StickLights::thor();
        for dir in thor.left.iter().chain(thor.right) {
            let dir = tmp.path().join(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("max_brightness"), "255\n").unwrap();
        }
        let read = |dir: &str, file: &str| std::fs::read_to_string(tmp.path().join(dir).join(file)).unwrap();
        let mut leds = Leds { root: tmp.path().to_path_buf(), lights: Some(thor), ended: false, quiet: Quiet::default(), said_none: false };
        leds.write([Lit { rgb: [255, 0, 0], brightness: 100 }, Lit { rgb: [0, 255, 0], brightness: 50 }]);
        assert_eq!((read(thor.left[3], "multi_intensity"), read(thor.left[3], "brightness")), ("0 0 255".into(), "255".into()));
        assert_eq!((read(thor.right[0], "multi_intensity"), read(thor.right[0], "brightness")), ("0 255 0".into(), "128".into()));
        let lights = Lights { worker: Worker::gone("lights test"), leds: Arc::new(Mutex::new(leds)) };
        let leds = lights.leds.clone();
        drop(lights);
        assert_eq!((read(thor.right[2], "multi_intensity"), read(thor.right[2], "brightness")), ("0 0 0".into(), "0".into()));
        // Nothing is written after the end.
        lock(&leds).write([Lit { rgb: [255, 0, 0], brightness: 100 }; 2]);
        assert_eq!(read(thor.left[0], "brightness"), "0");
    }

    /// Without the LEDs (a desktop, the VM) a write fails, logged, and nothing else happens.
    #[test]
    fn missing_leds_are_a_logged_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let mut leds = Leds { root: tmp.path().to_path_buf(), lights: Some(StickLights::thor()), ended: false, quiet: Quiet::default(), said_none: false };
        leds.write([Lit { rgb: [255, 0, 0], brightness: 100 }; 2]);
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none(), "nothing made");
        let mut none = Leds { lights: None, ..leds };
        none.write([Lit::OFF; 2]);
        assert!(none.said_none);
    }
}
