//! The volume and brightness tabs: the output volume and mute, which output plays, each app's volume
//! and mute (PipeWire's streams), and each screen's backlight. A level row is set by where the finger is
//! on it (and follows the finger), or stepped with the pad's left and right.

use crate::{Effect, State, Status};

/// A level row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// The output volume (the bar's).
    Volume,
    /// An app's stream, by position.
    Stream(u16),
    /// A backlight, by position.
    Backlight(u16),
}

/// An output (a PipeWire sink).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// PipeWire's object id: what it is asked about.
    pub id: u32,
    pub name: String,
    /// Where sound goes now.
    pub default: bool,
}

/// An app playing sound (a PipeWire output stream).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stream {
    pub id: u32,
    /// The app's name, and what it plays (when it says).
    pub app: String,
    pub title: String,
    /// 0-100, as wpctl shows it.
    pub volume: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Audio {
    pub outputs: Vec<Output>,
    pub streams: Vec<Stream>,
}

/// A screen's backlight.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Backlight {
    /// Its name in /sys/class/backlight.
    pub name: String,
    pub level: u8,
}

/// What the audio side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The volume tab opened (read the outputs and streams) or closed.
    Watch(bool),
    /// Sound to this output.
    Output(u32),
    StreamVolume(u32, u8),
    StreamMute(u32, bool),
}

/// How far the pad's left and right step a level.
const STEP: i16 = 5;

impl State {
    /// A level row's value now.
    pub fn level_of(&self, level: Level) -> Option<u8> {
        match level {
            Level::Volume => self.status.volume,
            Level::Stream(i) => self.audio.streams.get(usize::from(i)).map(|s| s.volume),
            Level::Backlight(i) => self.backlights.get(usize::from(i)).map(|b| b.level),
        }
    }

    /// A level row set to `value` (the finger on it, or a step).
    pub(crate) fn set_level(&mut self, level: Level, value: u8) -> Vec<Effect> {
        let value = value.min(100);
        if self.level_of(level) == Some(value) {
            return vec![];
        }
        self.dragging = Some(level);
        match level {
            Level::Volume => {
                self.status.volume = Some(value);
                let mut effects = vec![];
                // Turning it up or down unmutes.
                if self.status.muted {
                    self.status.muted = false;
                    effects.push(Effect::SetMute(false));
                }
                effects.push(Effect::SetVolume(value));
                effects
            }
            Level::Stream(i) => match self.audio.streams.get_mut(usize::from(i)) {
                Some(s) => {
                    s.volume = value;
                    s.muted = false;
                    vec![Effect::Audio(Command::StreamVolume(s.id, value))]
                }
                None => vec![],
            },
            Level::Backlight(i) => match self.backlights.get_mut(usize::from(i)) {
                Some(b) => {
                    b.level = value;
                    // The bar shows the first.
                    if i == 0 {
                        self.status.brightness = Some(value);
                    }
                    vec![Effect::SetBacklight { name: b.name.clone(), level: value }]
                }
                None => vec![],
            },
        }
    }

    /// The pad's left or right on a level row.
    pub(crate) fn step_level(&mut self, level: Level, direction: i16) -> Vec<Effect> {
        let Some(now) = self.level_of(level) else {
            return vec![];
        };
        let value = (i16::from(now) + direction.signum() * STEP).clamp(0, 100) as u8;
        let effects = self.set_level(level, value);
        self.dragging = None;
        effects
    }

    pub(crate) fn toggle_mute(&mut self) -> Vec<Effect> {
        if self.status.volume.is_none() {
            return vec![];
        }
        self.status.muted = !self.status.muted;
        vec![Effect::SetMute(self.status.muted)]
    }

    pub(crate) fn toggle_stream_mute(&mut self, i: usize) -> Vec<Effect> {
        match self.audio.streams.get_mut(i) {
            Some(s) => {
                s.muted = !s.muted;
                vec![Effect::Audio(Command::StreamMute(s.id, s.muted))]
            }
            None => vec![],
        }
    }

    pub(crate) fn choose_output(&mut self, i: usize) -> Vec<Effect> {
        let Some(id) = self.audio.outputs.get(i).filter(|o| !o.default).map(|o| o.id) else {
            return vec![];
        };
        for o in &mut self.audio.outputs {
            o.default = o.id == id;
        }
        vec![Effect::Audio(Command::Output(id))]
    }

    /// New readings: what the finger is setting keeps the finger's value until it lifts (a reading may
    /// predate it).
    pub(crate) fn set_audio(&mut self, mut audio: Audio) -> bool {
        if let Some(Level::Stream(i)) = self.dragging
            && let (Some(mine), Some(read)) = (self.audio.streams.get(usize::from(i)), audio.streams.get_mut(usize::from(i)))
            && mine.id == read.id
        {
            read.volume = mine.volume;
            read.muted = mine.muted;
        }
        let changed = audio != self.audio;
        self.audio = audio;
        changed
    }

    pub(crate) fn set_backlights(&mut self, mut backlights: Vec<Backlight>) -> bool {
        if let Some(Level::Backlight(i)) = self.dragging
            && let (Some(mine), Some(read)) = (self.backlights.get(usize::from(i)), backlights.get_mut(usize::from(i)))
        {
            read.level = mine.level;
        }
        let changed = backlights != self.backlights;
        self.backlights = backlights;
        changed
    }

    /// A status reading, with the output volume kept while the finger sets it.
    pub(crate) fn keep_dragged(&self, mut status: Status) -> Status {
        if self.dragging == Some(Level::Volume) {
            status.volume = self.status.volume;
            status.muted = self.status.muted;
        }
        if self.dragging == Some(Level::Backlight(0)) {
            status.brightness = self.status.brightness;
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::state;
    use crate::{Event, Id, Mode};

    fn audio() -> Audio {
        Audio {
            outputs: vec![Output { id: 50, name: "Speakers".into(), default: true }, Output { id: 51, name: "Headphones".into(), default: false }],
            streams: vec![Stream { id: 80, app: "Firefox".into(), title: String::new(), volume: 60, muted: false }],
        }
    }

    #[test]
    fn the_volume_tab_watches_and_sets_levels() {
        let mut s = state();
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Volume))).1, vec![Effect::Audio(Command::Watch(true))]);
        s.apply(Event::Audio(audio()));
        s.apply(Event::Status(Status { volume: Some(40), muted: true, ..Status::default() }));
        assert_eq!(s.apply(Event::SetLevel(Level::Volume, 70)).1, vec![Effect::SetMute(false), Effect::SetVolume(70)]);
        assert_eq!(s.apply(Event::SetLevel(Level::Volume, 70)).1, vec![], "no change, nothing sent");
        // A reading from before the finger moved doesn't jump the level back.
        s.apply(Event::Status(Status { volume: Some(40), ..Status::default() }));
        assert_eq!(s.status.volume, Some(70));
        s.apply(Event::Up(Id::Level(Level::Volume)));
        s.apply(Event::Status(Status { volume: Some(71), ..Status::default() }));
        assert_eq!(s.status.volume, Some(71), "after the finger lifts, the system's");
        assert_eq!(s.apply(Event::SetLevel(Level::Stream(0), 20)).1, vec![Effect::Audio(Command::StreamVolume(80, 20))]);
        s.apply(Event::Audio(audio()));
        assert_eq!(s.level_of(Level::Stream(0)), Some(20), "kept while dragged");
        assert_eq!(s.apply(Event::Activate(Id::StreamMute(0))).1, vec![Effect::Audio(Command::StreamMute(80, true))]);
        assert_eq!(s.apply(Event::Activate(Id::Output(1))).1, vec![Effect::Audio(Command::Output(51))]);
        assert_eq!(s.apply(Event::Activate(Id::Output(1))).1, vec![], "already there");
        assert_eq!(s.apply(Event::Activate(Id::Mode(Mode::Brightness))).1, vec![Effect::Audio(Command::Watch(false)), Effect::WatchBacklights(true)]);
    }

    #[test]
    fn backlights_are_set_one_by_one_and_the_pad_steps_levels() {
        let mut s = state();
        s.apply(Event::Backlights(vec![Backlight { name: "top".into(), level: 50 }, Backlight { name: "bottom".into(), level: 30 }]));
        assert_eq!(s.apply(Event::SetLevel(Level::Backlight(1), 80)).1, vec![Effect::SetBacklight { name: "bottom".into(), level: 80 }]);
        assert_eq!(s.apply(Event::LevelStep(Level::Backlight(0), -1)).1, vec![Effect::SetBacklight { name: "top".into(), level: 45 }]);
        assert_eq!(s.status.brightness, Some(45), "the bar shows the first");
        s.apply(Event::Status(Status { volume: Some(98), ..Status::default() }));
        assert_eq!(s.apply(Event::LevelStep(Level::Volume, 1)).1, vec![Effect::SetVolume(100)], "up to 100");
    }
}
