//! Where the settings in force go.

use std::path::PathBuf;

use clamshell_core::settings::{Setting, Values};

use crate::input::pad;
use crate::os::worker::Worker;
use crate::readings::mangohud;

/// Where the settings in force go: the hardware thread, the pad's keymap, and a
/// game's MangoHud config (in `mangohud`).
#[derive(Clone)]
pub struct Tuning {
    pub hardware: Worker<Values>,
    pub pad: Worker<pad::Command>,
    pub mangohud: PathBuf,
}

impl Tuning {
    /// The keymap in force, for the pad.
    pub fn keymap(&self, keymap: &clamshell_core::keymap::Keymap) {
        self.pad.send(pad::Command::Keymap(Box::new(keymap.clone())));
    }

    /// The gyro calibrated from its next second.
    pub fn calibrate_gyro(&self) {
        self.pad.send(pad::Command::CalibrateGyro);
    }

    /// Applies the settings in force (`game`'s, or the global ones without a game): the hardware, and
    /// for a game its MangoHud config (written, and reloaded live if it runs), whose
    /// path is returned for its launch.
    pub fn apply(&self, v: &Values, game: Option<&str>) -> Option<PathBuf> {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.choice()));
        let game = game?;
        mangohud::config(&self.mangohud, game, v.get(Setting::FpsCap), v.choice()).map_err(|e| log::warn!("MangoHud config: {e}")).ok()
    }
}
