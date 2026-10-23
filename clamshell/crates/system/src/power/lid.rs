//! The lid and the power key: while the lid is closed, the power key doesn't wake the device. Otherwise,
//! in a bag, a pressed key would wake it with the screens on until logind put it back to sleep. Opening
//! the lid wakes it, and logind sleeps when it is closed, per logind.conf.
//!
//! The input watch, in the device session only, calls [`closed`] as the lid closes and opens, and with
//! its state when it first sees the lid. This turns the power key's wakeup off and on through its
//! device's `power/wakeup`. That device is the `pwrkey` input device's parent, the PMIC, and the file is
//! writable by the `power` group through thor-system's `73-thor-power.rules`. It works like Armada's
//! `50-armada-powerbutton-suspend`, but follows the lid's state rather than the cause of a suspend.

use std::fs;
use std::path::{Path, PathBuf};

use crate::os::sys;

/// The wakeup files under `input`, the input class, of each device named like a PMIC's power key.
fn wakeup_files(input: &Path) -> Vec<PathBuf> {
    sys::entries(input)
        .into_iter()
        .filter(|dev| dev.file_name().is_some_and(|n| n.to_string_lossy().starts_with("input")))
        .filter(|dev| sys::text(dev.join("name")).is_some_and(|n| n.contains("pwrkey")))
        .map(|dev| dev.join("device/power/wakeup"))
        .filter(|file| file.exists())
        .collect()
}

/// Turns the power key's wakeup off when the lid is closed (`true`) and on when it is open, where it
/// differs.
pub fn closed(closed: bool) {
    set_in(Path::new("/sys/class/input"), closed);
}

fn set_in(input: &Path, closed: bool) {
    let want = if closed { "disabled" } else { "enabled" };
    for file in wakeup_files(input) {
        if sys::text(&file).is_some_and(|now| now.trim() != want) {
            match fs::write(&file, want) {
                Ok(()) => log::info!("lid {}: the power key's wakeup {want}", if closed { "closed" } else { "open" }),
                Err(e) => log::warn!("the power key's wakeup: {}: {e}", file.display()),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_power_keys_wakeup_follows_the_lid() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path();
        for (dev, name) in [("input0", "pm8941_pwrkey"), ("input1", "gpio-keys"), ("input2", "pm8941_resin")] {
            let power = input.join(dev).join("device/power");
            fs::create_dir_all(&power).unwrap();
            fs::write(input.join(dev).join("name"), format!("{name}\n")).unwrap();
            fs::write(power.join("wakeup"), "enabled\n").unwrap();
        }
        let wakeup = |dev: &str| fs::read_to_string(input.join(dev).join("device/power/wakeup")).unwrap();
        set_in(input, true);
        assert_eq!(wakeup("input0"), "disabled");
        assert_eq!(wakeup("input1"), "enabled\n", "only the power key");
        assert_eq!(wakeup("input2"), "enabled\n");
        set_in(input, false);
        assert_eq!(wakeup("input0"), "enabled");
    }
}
