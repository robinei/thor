//! Rumble on the real controller: one FF_RUMBLE effect uploaded when it is taken over, changed and
//! played as games ask (through the virtual DualSense's output reports). Shared with the thread that
//! reads those reports ([`RumbleSlot`]); empty while no controller with force feedback is there.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use evdev::raw_stream::RawDevice;
use evdev::{FFEffect, FFEffectCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger};

/// The real controller's rumble, when it has one.
pub type RumbleSlot = Arc<Mutex<Option<Rumble>>>;

/// Runs `f` on the rumble in the slot, if there is one; whether there was.
pub fn with(slot: &RumbleSlot, f: impl FnOnce(&mut Rumble)) -> bool {
    let mut held = slot.lock().unwrap_or_else(PoisonError::into_inner);
    held.as_mut().map(f).is_some()
}

/// Puts the controller's rumble in the slot (or empties it).
pub fn set(slot: &RumbleSlot, rumble: Option<Rumble>) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = rumble;
}

pub struct Rumble {
    effect: FFEffect,
    playing: bool,
}

/// An effect with both motors at these strengths (0-65535), lasting until changed.
fn effect(strong: u16, weak: u16) -> FFEffectData {
    FFEffectData {
        direction: 0,
        trigger: FFTrigger { button: 0, interval: 0 },
        // Length 0: plays until stopped.
        replay: FFReplay { length: 0, delay: 0 },
        kind: FFEffectKind::Rumble { strong_magnitude: strong, weak_magnitude: weak },
    }
}

impl Rumble {
    /// The controller's rumble, if it supports FF_RUMBLE (the effect keeps a handle of its own on it).
    pub fn of(real: &mut RawDevice) -> io::Result<Option<Rumble>> {
        if !real.supported_ff().is_some_and(|ff| ff.contains(FFEffectCode::FF_RUMBLE)) {
            return Ok(None);
        }
        let effect = real.upload_ff_effect(effect(0, 0))?;
        Ok(Some(Rumble { effect, playing: false }))
    }

    /// The motors at these strengths (0-255 each, as a DualSense takes them); both 0 stops.
    pub fn set(&mut self, strong: u8, weak: u8) -> io::Result<()> {
        if strong == 0 && weak == 0 {
            if self.playing {
                self.playing = false;
                self.effect.stop()?;
            }
            return Ok(());
        }
        // 0-255 to 0-65535 (255 * 257 = 65535).
        self.effect.update(effect(u16::from(strong) * 257, u16::from(weak) * 257))?;
        self.playing = true;
        self.effect.play(1)
    }
}
