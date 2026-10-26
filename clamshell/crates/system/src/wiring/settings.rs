//! Where the settings in force go.

use clamshell_state::settings::{Setting, Values};

use crate::input::pad;
use crate::os::worker::Worker;

/// Where the settings in force go: the hardware thread and the pad.
#[derive(Clone)]
pub struct Tuning {
    pub hardware: Worker<Values>,
    pub pad: Worker<pad::Command>,
}

impl Tuning {
    /// Sends the keymap in force to the pad.
    pub fn keymap(&self, keymap: &clamshell_state::keymap::Keymap) {
        self.pad.send(pad::Command::Keymap(Box::new(keymap.clone())));
    }

    /// Calibrates the gyro from its next second of readings.
    pub fn calibrate_gyro(&self) {
        self.pad.send(pad::Command::CalibrateGyro);
    }

    /// Applies the settings in force, a game's or the global ones, to the hardware and the pad's rumble.
    pub fn apply(&self, v: &Values) {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.choice()));
    }

    /// A capped game presents in FIFO, so that it waits for the frame callbacks the compositor paces to
    /// the cap: in mailbox or immediate it wouldn't. Mesa's Vulkan WSI, and GL's swap interval kept at 1
    /// or more (`vblank_mode` 3). Set at launch, so a cap set while the game runs reaches only games
    /// presenting in FIFO already.
    pub fn fifo_env(v: &Values) -> Vec<(String, String)> {
        if v.get(Setting::FpsCap) == 0 {
            return Vec::new();
        }
        vec![("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into()), ("vblank_mode".into(), "3".into())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capped_game_presents_in_fifo() {
        let mut v = Values::default();
        assert!(Tuning::fifo_env(&v).is_empty(), "no cap");
        v.set(Setting::FpsCap, 40);
        let env = Tuning::fifo_env(&v);
        assert!(env.contains(&("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into())), "{env:?}");
    }
}
