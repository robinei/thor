//! Where the settings in force go.

use clamshell_state::settings::Values;

use crate::input::pad;
use crate::os::worker::Worker;

/// Where the settings in force go: the hardware thread and the pad.
#[derive(Clone)]
pub(crate) struct Tuning {
    pub(crate) hardware: Worker<Values>,
    pub(crate) pad: Worker<pad::Command>,
}

impl Tuning {
    /// Sends the keymap in force to the pad.
    pub(crate) fn keymap(&self, keymap: &clamshell_state::keymap::Keymap) {
        self.pad.send(pad::Command::Keymap(Box::new(keymap.clone())));
    }

    /// Calibrates the gyro from its next second of readings.
    pub(crate) fn calibrate_gyro(&self) {
        self.pad.send(pad::Command::CalibrateGyro);
    }

    /// Applies the settings in force, a game's or the global ones, to the hardware and the pad's rumble.
    pub(crate) fn apply(&self, v: &Values) {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.choice()));
    }
}
