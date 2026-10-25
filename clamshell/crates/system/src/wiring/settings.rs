//! Where the settings in force go.

use std::path::PathBuf;

use clamshell_state::settings::{Setting, Values};

use crate::input::pad;
use crate::os::worker::Worker;
use crate::readings::mangohud;

/// Where the settings in force go: the hardware thread, the pad, and under sway a game's MangoHud config
/// in `mangohud` (the compositor measures and caps frames itself).
#[derive(Clone)]
pub struct Tuning {
    pub hardware: Worker<Values>,
    pub pad: Worker<pad::Command>,
    pub mangohud: Option<PathBuf>,
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

    /// Applies the settings in force, `game`'s or the global ones without a game, to the hardware and the
    /// pad's rumble. For a game under sway it also writes its MangoHud config, which is reloaded live if
    /// the game runs, and returns the config's path for its launch.
    pub fn apply(&self, v: &Values, game: Option<&str>) -> Option<PathBuf> {
        self.hardware.send(*v);
        self.pad.send(pad::Command::Rumble(v.choice()));
        let (game, dir) = (game?, self.mangohud.as_ref()?);
        mangohud::config(dir, game, v.get(Setting::FpsCap), v.choice()).map_err(|e| log::warn!("MangoHud config: {e}")).ok()
    }

    /// Under the compositor, a capped game presents in FIFO, so that it waits for the frame callbacks
    /// the compositor paces to the cap: in mailbox or immediate it wouldn't. Mesa's Vulkan WSI, and GL's
    /// swap interval kept at 1 or more (`vblank_mode` 3). Set at launch, so a cap set while the game
    /// runs reaches only games presenting in FIFO already.
    pub fn fifo_env(&self, v: &Values) -> Vec<(String, String)> {
        if self.mangohud.is_some() || v.get(Setting::FpsCap) == 0 {
            return Vec::new();
        }
        vec![("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into()), ("vblank_mode".into(), "3".into())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning(mangohud: Option<PathBuf>) -> Tuning {
        Tuning { hardware: Worker::gone("test"), pad: Worker::gone("test"), mangohud }
    }

    #[test]
    fn a_capped_game_presents_in_fifo_only_under_the_compositor() {
        let mut v = Values::default();
        assert!(tuning(None).fifo_env(&v).is_empty(), "no cap");
        v.set(Setting::FpsCap, 40);
        let env = tuning(None).fifo_env(&v);
        assert!(env.contains(&("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into())), "{env:?}");
        assert!(tuning(Some("/tmp".into())).fifo_env(&v).is_empty(), "under sway MangoHud caps it");
    }
}
