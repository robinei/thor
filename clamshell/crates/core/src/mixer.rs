//! The volume and brightness tabs: the output volume and mute, which output plays, each app's volume
//! and mute (PipeWire's streams); the screens' brightness and each screen's share of it. A level row is
//! set by where the finger is on it (and follows the finger), or stepped with the pad's left and right.
//!
//! Brightness is two levels multiplied: the global one (the bar's) and each screen's factor, so the bar
//! dims and brightens the screens together and keeps them as they are relative to each other. Both are
//! ours (the kernel only knows each panel's result): saved, and applied at start.
//!
//! The bar's sliders set the same levels as the tabs' rows (`Level::Volume`, `Level::Brightness`).

use crate::{Effect, State, Status};

/// A level row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// The output volume (the bar's).
    Volume,
    /// The screens' brightness (the bar's).
    Brightness,
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

/// The screens' brightness: the global level (0-100; unknown until read or set), and each backlight's
/// factor (0-100, by its name in /sys/class/backlight; 100 when not set).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brightness {
    pub level: Option<u8>,
    pub factors: Vec<(String, u8)>,
}

impl From<crate::controls::Control> for Level {
    fn from(control: crate::controls::Control) -> Level {
        match control {
            crate::controls::Control::Volume => Level::Volume,
            crate::controls::Control::Brightness => Level::Brightness,
        }
    }
}

impl Brightness {
    /// As saved: `level <n>` and `factor <backlight> <n>` lines, tab-separated.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(level) = self.level {
            text.push_str(&format!("level\t{level}\n"));
        }
        for (name, factor) in &self.factors {
            text.push_str(&format!("factor\t{name}\t{factor}\n"));
        }
        text
    }

    /// From what [`Brightness::text`] wrote (lines it doesn't know are skipped).
    pub fn from_text(text: &str) -> Brightness {
        let mut b = Brightness::default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["level", n] => b.level = n.parse::<u8>().ok().map(|l| l.min(100)),
                ["factor", name, n] => {
                    if let Ok(f) = n.parse::<u8>() {
                        b.set_factor(name, f.min(100));
                    }
                }
                _ => {}
            }
        }
        b
    }

    pub fn factor(&self, name: &str) -> u8 {
        self.factors.iter().find(|(n, _)| n == name).map_or(100, |(_, f)| *f)
    }

    fn set_factor(&mut self, name: &str, factor: u8) {
        match self.factors.iter_mut().find(|(n, _)| n == name) {
            Some(f) => f.1 = factor,
            None => self.factors.push((name.to_owned(), factor)),
        }
    }
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
            Level::Brightness => self.brightness.level.or(self.status.brightness),
            Level::Stream(i) => self.audio.streams.get(usize::from(i)).map(|s| s.volume),
            Level::Backlight(i) => self.backlights.get(usize::from(i)).map(|name| self.brightness.factor(name)),
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
            Level::Brightness => self.set_brightness(value),
            Level::Backlight(i) => match self.backlights.get(usize::from(i)).cloned() {
                Some(name) => {
                    self.brightness.set_factor(&name, value);
                    // The level as read, until ClamShell sets one (all factors were 100 then).
                    if self.brightness.level.is_none() {
                        self.brightness.level = self.status.brightness;
                    }
                    self.brightness_dirty = Some(self.unix_secs);
                    self.brightness_effect().into_iter().collect()
                }
                None => vec![],
            },
        }
    }

    /// The global brightness to `value` (the bar's slider, the tab's first row).
    pub(crate) fn set_brightness(&mut self, value: u8) -> Vec<Effect> {
        self.brightness.level = Some(value.min(100));
        self.brightness_dirty = Some(self.unix_secs);
        self.brightness_effect().into_iter().collect()
    }

    /// The screens' brightness as it should be, once the level is known (to apply at start too).
    pub fn brightness_effect(&self) -> Option<Effect> {
        let level = self.brightness.level?;
        Some(Effect::Brightness { level, factors: self.brightness.factors.clone() })
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

    pub(crate) fn set_backlights(&mut self, backlights: Vec<String>) -> bool {
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
    fn brightness_is_the_level_times_each_screens_factor() {
        let mut s = state();
        // The level read from the first panel until ClamShell sets one; factors 100 until set.
        s.apply(Event::Status(Status { brightness: Some(60), ..Status::default() }));
        assert_eq!(s.level_of(Level::Brightness), Some(60));
        assert_eq!(s.brightness_effect(), None, "nothing to apply before a level is set");
        s.apply(Event::Backlights(vec!["top".into(), "bottom".into()]));
        assert_eq!(s.level_of(Level::Backlight(1)), Some(100));
        let set = |level, factors: &[(&str, u8)]| vec![Effect::Brightness { level, factors: factors.iter().map(|(n, f)| ((*n).to_owned(), *f)).collect() }];
        assert_eq!(s.apply(Event::SetLevel(Level::Backlight(1), 70)).1, set(60, &[("bottom", 70)]));
        // The bar keeps the screens as they are to each other.
        s.apply(Event::SlideStart { control: crate::controls::Control::Brightness, x: 300.0, width: 620.0 });
        let fx = s.apply(Event::SlideTo { x: 360.0 }).1;
        assert!(matches!(fx.as_slice(), [Effect::Brightness { level, factors }] if *level > 60 && factors == &[("bottom".to_owned(), 70)]), "{fx:?}");
        s.apply(Event::SlideEnd);
        assert_eq!(s.apply(Event::LevelStep(Level::Brightness, -1)).1.len(), 1);
        // Saved a moment later; back after a restart.
        let saved = s.apply(Event::Tick { unix_secs: 10 }).1;
        let text = saved.iter().find_map(|e| match e {
            Effect::SaveBrightness(text) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(text.map(|t| Brightness::from_text(&t)), Some(s.brightness.clone()));
    }

    #[test]
    fn the_pad_steps_levels() {
        let mut s = state();
        s.apply(Event::Status(Status { volume: Some(98), ..Status::default() }));
        assert_eq!(s.apply(Event::LevelStep(Level::Volume, 1)).1, vec![Effect::SetVolume(100)], "up to 100");
        assert_eq!(s.apply(Event::LevelStep(Level::Volume, 1)).1, vec![], "and no further");
    }
}
