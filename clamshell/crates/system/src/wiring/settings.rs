//! Where the settings in force go.

use std::path::PathBuf;

use clamshell_core::settings::{Setting, Values};

use crate::input::pad;
use crate::os::worker::Worker;
use crate::readings::mangohud;

/// Where the settings in force go: the hardware thread, the pad, and a game's MangoHud config in
/// `mangohud`.
#[derive(Clone)]
pub struct Tuning {
    pub hardware: Worker<Values>,
    pub pad: Worker<pad::Command>,
    pub mangohud: PathBuf,
}

impl Tuning {
    /// Sends the keymap in force to the pad.
    pub fn keymap(&self, keymap: &clamshell_core::keymap::Keymap) {
        self.pad.send(pad::Command::Keymap(Box::new(keymap.clone())));
    }

    /// Calibrates the gyro from its next second of readings.
    pub fn calibrate_gyro(&self) {
        self.pad.send(pad::Command::CalibrateGyro);
    }

    /// Applies the settings in force, `game`'s or the global ones without a game, to the hardware and the
    /// pad's rumble. For a game it also writes its MangoHud config, which is reloaded live if the game
    /// runs, and returns the config's path for its launch.
    pub fn apply(&self, v: &Values, game: Option<&str>) -> Option<PathBuf> {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.choice()));
        let game = game?;
        mangohud::config(&self.mangohud, game, v.get(Setting::FpsCap), v.choice()).map_err(|e| log::warn!("MangoHud config: {e}")).ok()
    }
}
