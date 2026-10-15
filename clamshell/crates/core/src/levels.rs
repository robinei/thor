//! The volume and brightness tabs: the output volume and mute, which output plays, each app's volume
//! and mute (PipeWire's streams); the screens' brightness and each screen's share of it. A level row is
//! set by where the finger is on it (and follows the finger), or stepped with the pad's left and right.
//!
//! Brightness is two levels multiplied: the global one (the bar's) and each screen's share, so the bar
//! dims and brightens the screens together and keeps them as they are relative to each other. Both are
//! ours (the kernel only knows each panel's result): saved, and applied at start. Until ClamShell sets a
//! level, the first backlight's reading stands for it.
//!
//! The volume and mute are the system's: readings come in, and the finger's value wins over them while
//! it is on the level (a reading may predate it), see [`Held`].
//!
//! The bar's sliders set the same levels as the tabs' rows (`Level::Volume`, `Level::Brightness`).

use std::fmt::Write;

use crate::State;
use crate::effect::Effect;
use crate::id::Id;
use crate::items::ItemKey;
use crate::scroll::{ListId, RowPart, Rows};
use crate::slider::{Control, Slider};

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

impl Level {
    /// Its row's target: the bar's levels' own, a stream's or a backlight's in its list's row.
    pub fn id(self) -> Id {
        match self {
            Level::Stream(i) => Id::Row { list: ListId::Streams, i, part: RowPart::Level },
            Level::Backlight(i) => Id::Row { list: ListId::Backlights, i, part: RowPart::Level },
            level => Id::Level(level),
        }
    }

    /// The level a target sets, if it is a level's.
    pub fn of(id: &Id) -> Option<Level> {
        match id {
            Id::Level(level) => Some(*level),
            Id::Row { list: ListId::Streams, i, part: RowPart::Level } => Some(Level::Stream(*i)),
            Id::Row { list: ListId::Backlights, i, part: RowPart::Level } => Some(Level::Backlight(*i)),
            _ => None,
        }
    }
}

impl From<Control> for Level {
    fn from(control: Control) -> Level {
        match control {
            Control::Volume => Level::Volume,
            Control::Brightness => Level::Brightness,
        }
    }
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

/// A backlight's share of the brightness (0-100), by its name in /sys/class/backlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub backlight: String,
    pub share: u8,
}

/// The screens' brightness: the global level (0-100; unknown until read or set), and each backlight's
/// share (100 when not set).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brightness {
    pub level: Option<u8>,
    pub shares: Vec<Share>,
}

impl Brightness {
    /// As saved: `level <n>` and `factor <backlight> <n>` lines, tab-separated.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(level) = self.level {
            let _ = writeln!(text, "level\t{level}");
        }
        for s in &self.shares {
            let _ = writeln!(text, "factor\t{}\t{}", s.backlight, s.share);
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
                        b.set_share(name, f.min(100));
                    }
                }
                _ => {}
            }
        }
        b
    }

    pub fn share(&self, backlight: &str) -> u8 {
        self.shares.iter().find(|s| s.backlight == backlight).map_or(100, |s| s.share)
    }

    fn set_share(&mut self, backlight: &str, share: u8) {
        match self.shares.iter_mut().find(|s| s.backlight == backlight) {
            Some(s) => s.share = share,
            None => self.shares.push(Share { backlight: backlight.to_owned(), share }),
        }
    }
}

/// What the sound side is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
    /// The output volume (0-100), and its mute.
    Volume(u8),
    Mute(bool),
    /// An app's stream (by PipeWire's id): its volume, its mute.
    StreamVolume(u32, u8),
    StreamMute(u32, bool),
    /// Sound to this output.
    Output(u32),
}

/// A level the system reports and the finger sets: the latest reading, and while the finger is on the
/// level, the finger's value, which is shown instead (a reading may predate it). When the finger lifts,
/// its value stands until the next reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held<T> {
    read: T,
    set: Option<T>,
}

impl<T: Copy> Held<T> {
    /// The value shown.
    pub fn value(&self) -> T {
        self.set.unwrap_or(self.read)
    }

    fn reading(&mut self, value: T) {
        self.read = value;
    }

    /// The finger's value, kept over readings until released.
    fn hold(&mut self, value: T) {
        self.set = Some(value);
    }

    /// A value ClamShell set: held if the finger is on it, else standing until the next reading.
    fn put(&mut self, value: T) {
        match &mut self.set {
            Some(set) => *set = value,
            None => self.read = value,
        }
    }

    fn release(&mut self) {
        if let Some(value) = self.set.take() {
            self.read = value;
        }
    }
}

/// How far the pad's left and right step a level.
const STEP: i16 = 5;

#[derive(Debug, Clone, Default)]
pub struct Levels {
    volume: Held<Option<u8>>,
    muted: Held<bool>,
    audio: Audio,
    /// The backlights' names (read while the brightness tab is open).
    backlights: Vec<String>,
    brightness: Brightness,
    /// The first backlight's level as last read: the brightness until ClamShell sets one.
    read_brightness: Option<u8>,
    /// Volume or brightness being slid in the bar, if any.
    pub(crate) slider: Option<Slider>,
    /// The level row a finger is on.
    holding: Option<Level>,
}

impl Levels {
    pub(crate) fn new(brightness: Brightness) -> Levels {
        Levels { brightness, ..Levels::default() }
    }

    /// The output volume, if known.
    pub fn volume(&self) -> Option<u8> {
        self.volume.value()
    }

    pub fn muted(&self) -> bool {
        self.muted.value()
    }

    pub fn audio(&self) -> &Audio {
        &self.audio
    }

    pub fn backlights(&self) -> &[String] {
        &self.backlights
    }

    /// The backlights as a list's rows.
    pub(crate) fn backlight_rows(&self) -> &Vec<String> {
        &self.backlights
    }

    pub fn brightness(&self) -> &Brightness {
        &self.brightness
    }

    pub fn slider(&self) -> Option<&Slider> {
        self.slider.as_ref()
    }

    /// A level row's value now.
    pub fn level(&self, level: Level) -> Option<u8> {
        match level {
            Level::Volume => self.volume(),
            Level::Brightness => self.brightness.level.or(self.read_brightness),
            Level::Stream(i) => self.audio.streams.get(usize::from(i)).map(|s| s.volume),
            Level::Backlight(i) => self.backlights.get(usize::from(i)).map(|name| self.brightness.share(name)),
        }
    }

    /// The level a bar control shows now: the slider's while sliding, else the level's.
    pub fn control(&self, control: Control) -> Option<u8> {
        match self.slider {
            Some(s) if s.control == control => Some(s.value),
            _ => self.level(Level::from(control)),
        }
    }

    /// The screens' brightness as it should be, once the level is known (to apply at start too).
    pub fn brightness_effect(&self) -> Option<Effect> {
        let level = self.brightness.level?;
        Some(Effect::Brightness { level, shares: self.brightness.shares.clone() })
    }

    /// The screens dimmed (idle): a third of their brightness, not saved.
    pub fn dimmed_effect(&self) -> Option<Effect> {
        let level = self.brightness.level.or(self.read_brightness)?;
        Some(Effect::Brightness { level: (level / 3).max(1), shares: self.brightness.shares.clone() })
    }

    /// The screens back from dimmed: their brightness, ClamShell's or as it was read.
    pub fn undimmed_effect(&self) -> Option<Effect> {
        let level = self.brightness.level.or(self.read_brightness)?;
        Some(Effect::Brightness { level, shares: self.brightness.shares.clone() })
    }

    /// A status reading of the volume, mute and brightness.
    pub(crate) fn read(&mut self, volume: Option<u8>, muted: bool, brightness: Option<u8>) {
        self.volume.reading(volume);
        self.muted.reading(muted);
        self.read_brightness = brightness;
    }

    /// New readings of the outputs and streams: a stream the finger is on keeps the finger's value.
    /// Whether anything shown changed.
    pub(crate) fn read_audio(&mut self, mut audio: Audio) -> bool {
        if let Some(Level::Stream(i)) = self.holding
            && let (Some(mine), Some(read)) = (self.audio.streams.get(usize::from(i)), audio.streams.get_mut(usize::from(i)))
            && mine.id == read.id
        {
            read.volume = mine.volume;
            read.muted = mine.muted;
        }
        crate::replace(&mut self.audio, audio)
    }

    pub(crate) fn read_backlights(&mut self, backlights: Vec<String>) -> bool {
        crate::replace(&mut self.backlights, backlights)
    }

    /// The finger is on `level`: what it sets is held over readings until it lifts.
    fn hold(&mut self, level: Level) {
        if self.holding != Some(level) {
            self.release();
        }
        self.holding = Some(level);
    }

    /// The finger lifted: what it set stands until the next reading.
    pub(crate) fn release(&mut self) {
        self.holding = None;
        self.volume.release();
        self.muted.release();
    }

    /// A level row set to `value` by the finger on it.
    pub(crate) fn set(&mut self, level: Level, value: u8) -> Vec<Effect> {
        let value = value.min(100);
        if self.level(level) == Some(value) {
            return vec![];
        }
        self.hold(level);
        match level {
            Level::Volume => {
                self.volume.hold(Some(value));
                // Turning it up or down unmutes.
                let was_muted = self.muted();
                self.muted.hold(false);
                let mut effects = vec![];
                if was_muted {
                    effects.push(Effect::Sound(Sound::Mute(false)));
                }
                effects.push(Effect::Sound(Sound::Volume(value)));
                effects
            }
            Level::Stream(i) => match self.audio.streams.get_mut(usize::from(i)) {
                Some(s) => {
                    s.volume = value;
                    s.muted = false;
                    vec![Effect::Sound(Sound::StreamVolume(s.id, value))]
                }
                None => vec![],
            },
            Level::Brightness => {
                self.brightness.level = Some(value);
                self.brightness_effect().into_iter().collect()
            }
            Level::Backlight(i) => match self.backlights.get(usize::from(i)).cloned() {
                Some(name) => {
                    self.brightness.set_share(&name, value);
                    // The level as read, until ClamShell sets one (all shares were 100 then).
                    if self.brightness.level.is_none() {
                        self.brightness.level = self.read_brightness;
                    }
                    self.brightness_effect().into_iter().collect()
                }
                None => vec![],
            },
        }
    }

    /// The pad's left or right on a level row: a step, as if the finger set it and lifted.
    pub(crate) fn step(&mut self, level: Level, direction: i16) -> Vec<Effect> {
        let Some(now) = self.level(level) else {
            return vec![];
        };
        let value = (i16::from(now) + direction.signum() * STEP).clamp(0, 100) as u8;
        let effects = self.set(level, value);
        self.release();
        effects
    }

    pub(crate) fn toggle_mute(&mut self) -> Vec<Effect> {
        if self.volume().is_none() {
            return vec![];
        }
        let muted = !self.muted();
        self.muted.put(muted);
        vec![Effect::Sound(Sound::Mute(muted))]
    }

    pub(crate) fn toggle_stream_mute(&mut self, i: usize) -> Vec<Effect> {
        match self.audio.streams.get_mut(i) {
            Some(s) => {
                s.muted = !s.muted;
                vec![Effect::Sound(Sound::StreamMute(s.id, s.muted))]
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
        vec![Effect::Sound(Sound::Output(id))]
    }

    /// The tab they belong to closed: no level row is being set. (The bar's slider isn't the tab's: a
    /// finger sliding on the bar slides on while another opens a tab; it ends when that finger lifts.)
    pub(crate) fn leave(&mut self) {
        self.release();
    }
}

/// The apps playing as a list's rows (found again by their PipeWire id).
impl Rows for Audio {
    fn len(&self) -> usize {
        self.streams.len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.streams.get(i).map(|s| ItemKey::Number(s.id))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.streams.iter().position(|s| *key == ItemKey::Number(s.id))
    }
}

/// The backlights as a list's rows (by name).
impl Rows for Vec<String> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.get(i).map(|b| ItemKey::Name(b.clone()))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.iter().position(|b| *key == ItemKey::Name(b.clone()))
    }
}

impl State {
    /// A tap on a part of an app's row: its mute button mutes it (its level is set by the finger).
    pub(crate) fn on_stream(&mut self, i: usize, part: RowPart) -> Vec<Effect> {
        match part {
            RowPart::Mute => self.levels.toggle_stream_mute(i),
            RowPart::Main | RowPart::Level | RowPart::Edit => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn audio() -> Audio {
        Audio {
            outputs: vec![Output { id: 50, name: "Speakers".into(), default: true }, Output { id: 51, name: "Headphones".into(), default: false }],
            streams: vec![Stream { id: 80, app: "Firefox".into(), title: String::new(), volume: 60, muted: false }],
        }
    }

    fn set(s: &mut State, level: Level, value: u8) -> Vec<Effect> {
        s.apply(LevelEvent::Set(level, value).into()).1
    }

    #[test]
    fn the_volume_tab_watches_and_sets_levels() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Volume)), vec![Effect::Watch(Watch::Audio, true)]);
        read(&mut s, Reading::Audio(audio()));
        read(&mut s, Reading::Status(Status { volume: Some(40), muted: true, ..Status::default() }));
        assert_eq!(set(&mut s, Level::Volume, 70), vec![Effect::Sound(Sound::Mute(false)), Effect::Sound(Sound::Volume(70))]);
        assert_eq!(set(&mut s, Level::Volume, 70), vec![], "no change, nothing sent");
        // A reading from before the finger moved doesn't jump the level back.
        read(&mut s, Reading::Status(Status { volume: Some(40), ..Status::default() }));
        assert_eq!(s.levels().volume(), Some(70));
        s.apply(PointerEvent::Up(Id::Level(Level::Volume)).into());
        read(&mut s, Reading::Status(Status { volume: Some(71), ..Status::default() }));
        assert_eq!(s.levels().volume(), Some(71), "after the finger lifts, the system's");
        assert_eq!(set(&mut s, Level::Stream(0), 20), vec![Effect::Sound(Sound::StreamVolume(80, 20))]);
        read(&mut s, Reading::Audio(audio()));
        assert_eq!(s.levels().level(Level::Stream(0)), Some(20), "kept while held");
        assert_eq!(tap(&mut s, Id::Row { list: ListId::Streams, i: 0, part: RowPart::Mute }), vec![Effect::Sound(Sound::StreamMute(80, true))]);
        assert_eq!(tap(&mut s, Id::Audio(AudioId::Output(1))), vec![Effect::Sound(Sound::Output(51))]);
        assert_eq!(tap(&mut s, Id::Audio(AudioId::Output(1))), vec![], "already there");
        assert_eq!(tap(&mut s, Id::Tab(Tab::Brightness)), vec![Effect::Watch(Watch::Audio, false), Effect::Watch(Watch::Backlights, true)]);
    }

    #[test]
    fn leaving_the_tab_mid_drag_lets_readings_in_again() {
        let mut s = state();
        read(&mut s, Reading::Status(Status { volume: Some(40), ..Status::default() }));
        tap(&mut s, Id::Tab(Tab::Volume));
        set(&mut s, Level::Volume, 60);
        tap(&mut s, Id::Tab(Tab::Dock));
        read(&mut s, Reading::Status(Status { volume: Some(45), ..Status::default() }));
        assert_eq!(s.levels().volume(), Some(45));
    }

    #[test]
    fn brightness_is_the_level_times_each_screens_share() {
        let mut s = state();
        // The level read from the first panel until ClamShell sets one; shares 100 until set.
        read(&mut s, Reading::Status(Status { brightness: Some(60), ..Status::default() }));
        assert_eq!(s.levels().level(Level::Brightness), Some(60));
        assert_eq!(s.levels().brightness_effect(), None, "nothing to apply before a level is set");
        read(&mut s, Reading::Backlights(vec!["top".into(), "bottom".into()]));
        assert_eq!(s.levels().level(Level::Backlight(1)), Some(100));
        let share = |backlight: &str, share| Share { backlight: backlight.into(), share };
        assert_eq!(set(&mut s, Level::Backlight(1), 70), vec![Effect::Brightness { level: 60, shares: vec![share("bottom", 70)] }]);
        // The bar keeps the screens as they are to each other.
        s.apply(Slide::Start { control: Control::Brightness, x: 300.0, width: 620.0 }.into());
        let fx = s.apply(Slide::To { x: 360.0 }.into()).1;
        assert!(matches!(fx.as_slice(), [Effect::Brightness { level, shares }] if *level > 60 && *shares == [share("bottom", 70)]), "{fx:?}");
        s.apply(Slide::End.into());
        assert_eq!(s.apply(LevelEvent::Step(Level::Brightness, -1).into()).1.len(), 1);
        // Saved a moment later; back after a restart.
        let saved = s.apply(Event::Tick { unix_secs: 10 }).1;
        let text = saved.iter().find_map(|e| match e {
            Effect::Save(Save::Brightness(text)) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(text.map(|t| Brightness::from_text(&t)).as_ref(), Some(s.levels().brightness()));
    }

    #[test]
    fn the_pad_steps_levels() {
        let mut s = state();
        read(&mut s, Reading::Status(Status { volume: Some(98), ..Status::default() }));
        assert_eq!(s.apply(LevelEvent::Step(Level::Volume, 1).into()).1, vec![Effect::Sound(Sound::Volume(100))], "up to 100");
        assert_eq!(s.apply(LevelEvent::Step(Level::Volume, 1).into()).1, vec![], "and no further");
    }
}
