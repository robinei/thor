//! Where the settings in force go.

use clamshell_state::settings::Values;

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
}
