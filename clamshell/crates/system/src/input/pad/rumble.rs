//! Rumble: what games ask of the virtual pad's force feedback, played on the real controller's motors.
//!
//! A game uploads rumble effects to the virtual pad, each with a strength for the strong and the weak
//! motor and a length, and then plays and stops them. The virtual pad answers the uploads
//! ([`Effects::serve`]) and keeps track of what is playing. A game waits for the answer, so this is done
//! whether or not a controller is there. The real controller, if it has force feedback, gets one effect
//! of its own, set to the strongest of those playing ([`Effects::level`]) and scaled by the Rumble
//! setting ([`scale`]).

use std::io;
use std::time::{Duration, Instant};

use clamshell_core::settings::RumbleStrength;
use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{EventSummary, FFEffect, FFEffectCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger, InputEvent, UInputCode};

/// A short buzz of the controller's own, not a game's. `On` means a layer that stays on came on or was
/// armed. `Off` means the pad is back in the layers beneath, because one went off or was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buzz {
    On,
    Off,
}

impl Buzz {
    pub const LENGTH: Duration = Duration::from_millis(40);

    /// The motors' strengths (0-65535). Coming on uses the light motor, going off the heavy one.
    pub fn level(self) -> (u16, u16) {
        match self {
            Buzz::On => (0, 40_000),
            Buzz::Off => (30_000, 0),
        }
    }
}

/// How many effects a game may upload to the virtual pad at once.
pub const EFFECTS_MAX: u32 = 16;

/// A rumble effect a game uploaded: its id, the motors' strengths (0-65535), and how long it plays. A
/// length of 0 plays until stopped.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Uploaded {
    id: i16,
    strong: u16,
    weak: u16,
    length: Duration,
}

/// The effects games uploaded to the virtual pad, which of them are playing and until when if they end
/// by themselves, and the gain the games set (0-65535).
#[derive(Debug, Clone, PartialEq)]
pub struct Effects {
    uploaded: Vec<Uploaded>,
    playing: Vec<(i16, Option<Instant>)>,
    gain: u16,
}

impl Default for Effects {
    fn default() -> Effects {
        Effects { uploaded: Vec::new(), playing: Vec::new(), gain: u16::MAX }
    }
}

impl Effects {
    /// Answers the virtual pad's waiting force-feedback requests, uploads and erases, and follows its play
    /// and stop events, at `now`. The virtual pad's file is non-blocking.
    pub fn serve(&mut self, pad: &mut VirtualDevice, now: Instant) -> io::Result<()> {
        let events: Vec<InputEvent> = match pad.fetch_events() {
            Ok(events) => events.collect(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };
        for event in events {
            match event.destructure() {
                EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, _) => {
                    let mut upload = pad.process_ff_upload(request)?;
                    let data = upload.effect();
                    self.upload(upload.effect_id(), data);
                    upload.set_retval(0);
                }
                EventSummary::UInput(request, UInputCode::UI_FF_ERASE, _) => {
                    let mut erase = pad.process_ff_erase(request)?;
                    let id = i16::try_from(erase.effect_id()).unwrap_or(-1);
                    self.uploaded.retain(|e| e.id != id);
                    self.playing.retain(|(p, _)| *p != id);
                    erase.set_retval(0);
                }
                EventSummary::ForceFeedback(_, FFEffectCode::FF_GAIN, value) => self.gain = u16::try_from(value).unwrap_or(u16::MAX),
                EventSummary::ForceFeedback(_, FFEffectCode(code), value) => self.play(code, value > 0, now),
                _ => {}
            }
        }
        Ok(())
    }

    /// Handles an uploaded or changed effect. A rumble is kept. Other kinds are accepted and do nothing.
    fn upload(&mut self, id: i16, data: FFEffectData) {
        self.uploaded.retain(|e| e.id != id);
        if let FFEffectKind::Rumble { strong_magnitude, weak_magnitude } = data.kind {
            let length = Duration::from_millis(u64::from(data.replay.length));
            self.uploaded.push(Uploaded { id, strong: strong_magnitude, weak: weak_magnitude, length });
        }
    }

    /// Notes that effect `code` was played or stopped at `now`.
    fn play(&mut self, code: u16, on: bool, now: Instant) {
        let Ok(id) = i16::try_from(code) else {
            return;
        };
        self.playing.retain(|(p, _)| *p != id);
        if on && let Some(e) = self.uploaded.iter().find(|e| e.id == id) {
            let until = (!e.length.is_zero()).then(|| now + e.length);
            self.playing.push((id, until));
        }
    }

    /// The motors' strengths now (0-65535): the strongest of the effects playing, times the gain. Effects
    /// that ended are dropped.
    pub fn level(&mut self, now: Instant) -> (u16, u16) {
        self.playing.retain(|(_, until)| until.is_none_or(|t| now < t));
        let mut level = (0u16, 0u16);
        for (id, _) in &self.playing {
            if let Some(e) = self.uploaded.iter().find(|e| e.id == *id) {
                level = (level.0.max(e.strong), level.1.max(e.weak));
            }
        }
        let gain = |v: u16| (u32::from(v) * u32::from(self.gain) / u32::from(u16::MAX)) as u16;
        (gain(level.0), gain(level.1))
    }

    /// When the next effect playing ends by itself.
    pub fn next_end(&self) -> Option<Instant> {
        self.playing.iter().filter_map(|(_, until)| *until).min()
    }
}

/// The motors' strengths (strong, weak). Each is the stronger of the games' `effects`, while `in_game`
/// says the game has the pad, and the keymap's `buzz`, scaled by the Rumble setting.
pub fn mix(effects: (u16, u16), in_game: bool, buzz: Option<(u16, u16)>, setting: RumbleStrength) -> (u16, u16) {
    let (mut strong, mut weak) = if in_game { effects } else { (0, 0) };
    if let Some((s, w)) = buzz {
        (strong, weak) = (strong.max(s), weak.max(w));
    }
    (scale(strong, setting), scale(weak, setting))
}

/// A strength scaled by the Rumble setting.
pub fn scale(v: u16, setting: RumbleStrength) -> u16 {
    let thirds = match setting {
        RumbleStrength::Off => 0,
        RumbleStrength::Low => 1,
        RumbleStrength::Mid => 2,
        RumbleStrength::Full => 3,
    };
    (u32::from(v) * thirds / 3) as u16
}

/// The real controller's rumble: one FF_RUMBLE effect, uploaded when the controller is taken over and
/// changed and played as the level changes.
pub struct Rumble {
    effect: FFEffect,
    level: (u16, u16),
}

/// An effect with both motors at these strengths (0-65535), lasting until changed.
fn effect(strong: u16, weak: u16) -> FFEffectData {
    FFEffectData {
        direction: 0,
        trigger: FFTrigger { button: 0, interval: 0 },
        // A length of 0 plays until stopped.
        replay: FFReplay { length: 0, delay: 0 },
        kind: FFEffectKind::Rumble { strong_magnitude: strong, weak_magnitude: weak },
    }
}

impl Rumble {
    /// The controller's rumble, if it supports FF_RUMBLE. The effect keeps a handle of its own on the
    /// device.
    pub fn of(real: &mut RawDevice) -> io::Result<Option<Rumble>> {
        if !real.supported_ff().is_some_and(|ff| ff.contains(FFEffectCode::FF_RUMBLE)) {
            return Ok(None);
        }
        let effect = real.upload_ff_effect(effect(0, 0))?;
        Ok(Some(Rumble { effect, level: (0, 0) }))
    }

    /// Sets the motors to these strengths (0-65535 each). Both at 0 stops them. Only a change reaches the
    /// device.
    pub fn set(&mut self, strong: u16, weak: u16) -> io::Result<()> {
        if (strong, weak) == self.level {
            return Ok(());
        }
        self.level = (strong, weak);
        if strong == 0 && weak == 0 {
            return self.effect.stop();
        }
        self.effect.update(effect(strong, weak))?;
        self.effect.play(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rumble(strong: u16, weak: u16, ms: u16) -> FFEffectData {
        FFEffectData { replay: FFReplay { length: ms, delay: 0 }, ..effect(strong, weak) }
    }

    #[test]
    fn the_strongest_effect_playing_until_it_ends() {
        let mut e = Effects::default();
        let t0 = Instant::now();
        e.upload(0, rumble(40_000, 0, 0));
        e.upload(1, rumble(10_000, 30_000, 200));
        assert_eq!(e.level(t0), (0, 0), "uploaded, not playing");
        e.play(0, true, t0);
        e.play(1, true, t0);
        assert_eq!(e.level(t0), (40_000, 30_000), "each motor's strongest");
        assert_eq!(e.next_end(), Some(t0 + Duration::from_millis(200)));
        assert_eq!(e.level(t0 + Duration::from_millis(200)), (40_000, 0), "the short one ended");
        e.play(0, false, t0);
        assert_eq!(e.level(t0), (0, 0), "stopped");
        // The gain scales them, and another kind of effect does nothing.
        e.gain = u16::MAX / 2;
        e.play(0, true, t0);
        assert_eq!(e.level(t0), (19_999, 0));
        e.upload(
            0,
            FFEffectData {
                kind: FFEffectKind::Constant { level: 100, envelope: evdev::FFEnvelope { attack_length: 0, attack_level: 0, fade_length: 0, fade_level: 0 } },
                ..rumble(0, 0, 0)
            },
        );
        assert_eq!(e.level(t0), (0, 0));
        use RumbleStrength::{Full, Low, Off};
        assert_eq!((scale(60_000, Off), scale(60_000, Low), scale(60_000, Full)), (0, 20_000, 60_000));
    }

    #[test]
    fn games_rumble_only_in_game_and_buzzes_always_at_the_settings_strength() {
        use RumbleStrength::{Full, Low, Off};
        let games = (60_000, 30_000);
        assert_eq!(mix(games, true, None, Full), games);
        assert_eq!(mix(games, false, None, Full), (0, 0), "the pad away from the game");
        assert_eq!(mix(games, false, Some((9_000, 3_000)), Full), (9_000, 3_000), "a buzz whatever has the pad");
        assert_eq!(mix(games, true, Some((40_000, 45_000)), Full), (60_000, 45_000), "the stronger of each");
        assert_eq!(mix(games, true, None, Low), (20_000, 10_000));
        assert_eq!(mix(games, true, Some((9_000, 3_000)), Off), (0, 0));
    }
}
