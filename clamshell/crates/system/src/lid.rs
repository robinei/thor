//! The lid and the power key: while the lid is closed, the power key doesn't wake the device. Closed in a
//! bag, a pressed key would wake it, screens on, until logind put it back to sleep; opening the lid
//! wakes it (logind sleeps on closing it, per logind.conf). The input watch (device session only) calls
//! [`closed`] as the lid closes and opens, and for its state when it first sees the lid; this turns the
//! power key's wakeup off and on: its device's `power/wakeup` (the `pwrkey` input device's parent, the
//! PMIC's; writable by the `power` group: thor-system's `73-thor-power.rules`). As Armada's
//! `50-armada-powerbutton-suspend`, but on the lid's state rather than a suspend's cause.

use std::fs;
use std::path::{Path, PathBuf};

use crate::sys;

/// The power keys' wakeup files under `input` (the input class): of each device named like a PMIC's
/// power key.
fn wakeup_files(input: &Path) -> Vec<PathBuf> {
    sys::entries(input)
        .into_iter()
        .filter(|dev| dev.file_name().is_some_and(|n| n.to_string_lossy().starts_with("input")))
        .filter(|dev| sys::text(dev.join("name")).is_some_and(|n| n.contains("pwrkey")))
        .map(|dev| dev.join("device/power/wakeup"))
        .filter(|file| file.exists())
        .collect()
}

/// The lid closed (`true`) or open: the power key's wakeup off or on, where it differs.
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
        let input = clamshell_core::testdir::TempDir::new("lid");
        for (dev, name) in [("input0", "pm8941_pwrkey"), ("input1", "gpio-keys"), ("input2", "pm8941_resin")] {
            let power = input.join(dev).join("device/power");
            fs::create_dir_all(&power).unwrap();
            fs::write(input.join(dev).join("name"), format!("{name}\n")).unwrap();
            fs::write(power.join("wakeup"), "enabled\n").unwrap();
        }
        let wakeup = |dev: &str| fs::read_to_string(input.join(dev).join("device/power/wakeup")).unwrap();
        set_in(&input, true);
        assert_eq!(wakeup("input0"), "disabled");
        assert_eq!(wakeup("input1"), "enabled\n", "only the power key");
        assert_eq!(wakeup("input2"), "enabled\n");
        set_in(&input, false);
        assert_eq!(wakeup("input0"), "enabled");
    }
}
