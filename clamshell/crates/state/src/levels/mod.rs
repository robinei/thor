//! The volume and brightness tabs. The volume tab covers the output volume and mute, which output
//! plays, and each app's volume and mute (PipeWire's streams). The brightness tab covers the screens'
//! brightness and each screen's share of it. A level row is set by where the finger is on it, and
//! follows the finger, or is stepped with the pad's left and right.
//!
//! Brightness is two levels multiplied together: the global one, set from the bar, and each screen's
//! share. That way the bar dims and brightens the screens together and keeps them the same relative to
//! each other. Both levels are ours, since the kernel only knows each panel's result. They are saved and
//! applied at start. Until ClamShell sets a level, the first backlight's reading stands for it.
//!
//! The volume and mute belong to the system. Readings come in, and while the finger is on the level its
//! value wins over them, since a reading may predate it (see [`Held`]).
//!
//! The bar's sliders set the same levels as the tabs' rows (`Level::Volume`, `Level::Brightness`).

pub mod slider;

use crate::effect::{Effect, Save};
use crate::id::Id;
use crate::kit::debounce::Debounce;
use crate::kit::list::{ItemKey, Rows};
use crate::nav::lists::{ListId, RowPart};
use crate::out::Out;
use crate::resume::SAVE_AFTER;
use crate::{AudioId, LevelEvent};
use slider::{Control, Slider};

/// A level row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// The output volume, also set from the bar.
    Volume,
    /// The screens' brightness, also set from the bar.
    Brightness,
    /// An app's stream, by position.
    Stream(u16),
    /// A backlight, by position.
    Backlight(u16),
}

impl Level {
    /// The target of the level's row. The bar's levels have their own, and a stream or backlight uses
    /// its row in its list.
    pub fn id(self) -> Id {
        match self {
            Level::Stream(i) => Id::Row { list: ListId::Streams, i, part: RowPart::Level },
            Level::Backlight(i) => Id::Row { list: ListId::Backlights, i, part: RowPart::Level },
            level => Id::Level(level),
        }
    }

    /// The level a target sets, if it sets one.
    pub fn of(id: &Id) -> Option<Level> {
        match id {
            Id::Level(level) => Some(*level),
            Id::Row { list: ListId::Streams, i, part: RowPart::Level } => Some(Level::Stream(*i)),
            Id::Row { list: ListId::Backlights, i, part: RowPart::Level } => Some(Level::Backlight(*i)),
            _ => None,
        }
    }
}

/// An output (a PipeWire sink).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioOutput {
    /// PipeWire's object id, used to refer to it.
    pub id: u32,
    pub name: String,
    /// Whether sound goes here now.
    pub default: bool,
}

/// An app playing sound (a PipeWire output stream).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stream {
    pub id: u32,
    /// The app's name, and what it plays when it says.
    pub app: String,
    pub title: String,
    /// 0-100, as wpctl shows it.
    pub volume: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Audio {
    pub outputs: Vec<AudioOutput>,
    pub streams: Vec<Stream>,
}

/// A detected backlight's screen role, or its kernel name when the profile has no role hint.
pub fn backlight_label<'a>(profile: &crate::Profile, name: &'a str) -> &'a str {
    if profile.top.backlight == Some(name) {
        "Top screen"
    } else if profile.bottom.backlight == Some(name) {
        "Bottom screen"
    } else {
        name
    }
}

/// A backlight's share of the brightness (0-100), by its name in /sys/class/backlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub backlight: String,
    pub share: u8,
}

/// The screens' brightness: the global level, 0-100 and unknown until read or set, and each backlight's
/// share, which is 100 when not set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brightness {
    pub level: Option<u8>,
    pub shares: Vec<Share>,
}

impl Brightness {
    pub fn share(&self, backlight: &str) -> u8 {
        self.shares.iter().find(|s| s.backlight == backlight).map_or(100, |s| s.share)
    }

    pub fn set_share(&mut self, backlight: &str, share: u8) {
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
    /// An app's stream, by PipeWire's id: its volume, and its mute.
    StreamVolume(u32, u8),
    StreamMute(u32, bool),
    /// Send sound to this output.
    Output(u32),
}

/// A level the system reports and the finger sets. It holds the latest reading, and while the finger is
/// on the level, the finger's value, which is shown instead because a reading may predate it. When the
/// finger lifts, its value stands until the next reading.
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

    /// A value ClamShell set. It is held if the finger is on the level, and otherwise stands until the
    /// next reading.
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
    /// The backlight names, read while the brightness tab is open.
    backlights: Backlights,
    brightness: Brightness,
    /// The first backlight's level as last read. It stands for the brightness until ClamShell sets one.
    read_brightness: Option<u8>,
    /// Volume or brightness being slid in the bar, if any.
    slider: Option<Slider>,
    /// The level row a finger is on.
    holding: Option<Level>,
    /// A change of the brightness, saved a moment later.
    unsaved: Debounce,
    /// The brightness changed during [`Levels::change`].
    brightness_changed: bool,
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
        &self.backlights.0
    }

    /// The backlights as a list's rows.
    pub(crate) fn backlight_rows(&self) -> &Backlights {
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
            Level::Backlight(i) => self.backlights.0.get(usize::from(i)).map(|name| self.brightness.share(name)),
        }
    }

    /// The level a bar control shows now: the slider's value while sliding, otherwise the level's.
    pub fn control(&self, control: Control) -> Option<u8> {
        match self.slider {
            Some(s) if s.control == control => Some(s.value),
            _ => self.level(control.level()?),
        }
    }

    /// The screens' brightness as it should be at idle stage `dimmed` or not, given whether ClamShell has
    /// set the backlights before (`touched`): ClamShell's level; dimmed, a third of the known level but
    /// not dark; back from dimmed, the level known before. None when there is nothing to set: no level is
    /// known, or ClamShell never set one and hasn't touched the backlights.
    pub(crate) fn wanted_brightness(&self, dimmed: bool, touched: bool) -> Option<u8> {
        let known = self.brightness.level.or(self.read_brightness);
        match (dimmed, self.brightness.level) {
            (true, _) => known.map(|level| (level / 3).max(1).min(level)),
            (false, Some(level)) => Some(level),
            (false, None) => known.filter(|_| touched),
        }
    }

    /// The screens at brightness `level`, each scaled by its share.
    pub(crate) fn brightness_at(&self, level: u8) -> Effect {
        Effect::Brightness { level, shares: self.brightness.shares.clone() }
    }

    /// A status reading of the volume, mute and brightness. A finger on the level keeps what it shows.
    /// While the screens are dimmed (`dimmed`), the backlight shows the dimmed level, not the screens'
    /// brightness, so its reading is ignored.
    pub(crate) fn read(&mut self, volume: Option<u8>, muted: bool, brightness: Option<u8>, dimmed: bool) {
        self.volume.reading(volume);
        self.muted.reading(muted);
        if !dimmed {
            self.read_brightness = brightness;
        }
    }

    /// Changes the levels with `change` at `now`. A changed brightness is saved a moment later.
    pub(crate) fn change(&mut self, now: u64, change: impl FnOnce(&mut Levels)) {
        change(self);
        if std::mem::take(&mut self.brightness_changed) {
            self.unsaved.touch(now);
        }
    }

    /// Saves the brightness once it has stopped changing for a moment.
    pub(crate) fn save_due(&mut self, now: u64, out: &mut Out) {
        if self.unsaved.due(now, SAVE_AFTER) {
            out.effects.push(self.brightness_save());
        }
    }

    /// Saves the brightness.
    pub(crate) fn brightness_save(&self) -> Effect {
        Effect::Save(Save::Brightness(self.brightness.clone()))
    }

    /// The volume tab's buttons.
    pub(crate) fn on_audio(&mut self, id: AudioId, out: &mut Out) {
        match id {
            AudioId::MuteOutput => {
                self.toggle_mute(out);
            }
            AudioId::Output(i) => self.choose_output(usize::from(i), out),
        }
    }

    /// A tap on part of an app's row. Its mute button mutes it; its level is set by the finger.
    pub(crate) fn on_stream(&mut self, i: usize, part: RowPart, out: &mut Out) {
        match part {
            RowPart::Mute => self.toggle_stream_mute(i, out),
            RowPart::Main | RowPart::Level | RowPart::Edit => {}
        }
    }

    /// New readings of the outputs and streams. A stream the finger is on keeps the finger's value.
    pub(crate) fn read_audio(&mut self, mut audio: Audio) {
        if let Some(Level::Stream(i)) = self.holding
            && let (Some(mine), Some(read)) = (self.audio.streams.get(usize::from(i)), audio.streams.get_mut(usize::from(i)))
            && mine.id == read.id
        {
            read.volume = mine.volume;
            read.muted = mine.muted;
        }
        self.audio = audio;
    }

    pub(crate) fn read_backlights(&mut self, backlights: Vec<String>) {
        self.backlights.0 = backlights;
    }

    /// The finger is on `level`. What it sets is kept over readings until it lifts.
    fn hold(&mut self, level: Level) {
        if self.holding != Some(level) {
            self.release();
        }
        self.holding = Some(level);
    }

    /// The finger lifted. What it set stands until the next reading.
    pub(crate) fn release(&mut self) {
        self.holding = None;
        self.volume.release();
        self.muted.release();
    }

    /// A level row set to `value` by the finger on it.
    pub(crate) fn set(&mut self, level: Level, value: u8, out: &mut Out) {
        let value = value.min(100);
        if self.level(level) == Some(value) {
            return;
        }
        self.hold(level);
        match level {
            Level::Volume => {
                self.volume.hold(Some(value));
                // Turning it up or down unmutes.
                let was_muted = self.muted();
                self.muted.hold(false);
                if was_muted {
                    out.effects.push(Effect::Sound(Sound::Mute(false)));
                }
                out.effects.push(Effect::Sound(Sound::Volume(value)));
            }
            Level::Stream(i) => {
                if let Some(s) = self.audio.streams.get_mut(usize::from(i)) {
                    // Turning it up or down unmutes, as the output volume does.
                    if s.muted {
                        out.effects.push(Effect::Sound(Sound::StreamMute(s.id, false)));
                    }
                    s.volume = value;
                    s.muted = false;
                    out.effects.push(Effect::Sound(Sound::StreamVolume(s.id, value)));
                }
            }
            // Sent by sync, dimmed while idle has dimmed the screens.
            Level::Brightness => {
                self.brightness_changed |= self.brightness.level != Some(value);
                self.brightness.level = Some(value);
            }
            Level::Backlight(i) => {
                if let Some(name) = self.backlights.0.get(usize::from(i)).cloned() {
                    self.brightness_changed |= self.brightness.share(&name) != value;
                    self.brightness.set_share(&name, value);
                    // The level as read, until ClamShell sets one. All shares were 100 until then.
                    if self.brightness.level.is_none() {
                        self.brightness_changed = true;
                        self.brightness.level = self.read_brightness;
                    }
                }
            }
        }
    }

    /// A level row set by the finger on it, or stepped by the pad, at `now`.
    pub(crate) fn on_event(&mut self, event: LevelEvent, now: u64, out: &mut Out) {
        match event {
            LevelEvent::Set(level, value) => {
                self.change(now, |l| l.set(level, value, out));
            }
            LevelEvent::Step(level, direction) => {
                self.change(now, |l| l.step(level, direction, out));
            }
        }
    }

    /// The pad's left or right on a level row steps it, as if the finger set it and lifted.
    pub(crate) fn step(&mut self, level: Level, direction: i16, out: &mut Out) {
        let Some(now) = self.level(level) else {
            return;
        };
        let value = (i16::from(now) + direction.signum() * STEP).clamp(0, 100) as u8;
        self.set(level, value, out);
        self.release();
    }

    /// Mutes or unmutes the output, once its volume is known. Returns whether it did.
    pub(crate) fn toggle_mute(&mut self, out: &mut Out) -> bool {
        if self.volume().is_none() {
            return false;
        }
        let muted = !self.muted();
        self.muted.put(muted);
        out.effects.push(Effect::Sound(Sound::Mute(muted)));
        true
    }

    pub(crate) fn toggle_stream_mute(&mut self, i: usize, out: &mut Out) {
        if let Some(s) = self.audio.streams.get_mut(i) {
            s.muted = !s.muted;
            out.effects.push(Effect::Sound(Sound::StreamMute(s.id, s.muted)));
        }
    }

    pub(crate) fn choose_output(&mut self, i: usize, out: &mut Out) {
        let Some(id) = self.audio.outputs.get(i).filter(|o| !o.default).map(|o| o.id) else {
            return;
        };
        for o in &mut self.audio.outputs {
            o.default = o.id == id;
        }
        out.effects.push(Effect::Sound(Sound::Output(id)));
    }
}

/// The apps playing as a list's rows, found again by their PipeWire id.
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

/// The backlight names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Backlights(Vec<String>);

/// The backlights as a list's rows, found again by name.
impl Rows for Backlights {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, i: usize) -> Option<ItemKey> {
        self.0.get(i).map(|b| ItemKey::name(b))
    }

    fn find(&self, key: &ItemKey) -> Option<usize> {
        self.0.iter().position(|b| ItemKey::name(b) == *key)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::*;

    fn audio() -> Audio {
        Audio {
            outputs: vec![AudioOutput { id: 50, name: "Speakers".into(), default: true }, AudioOutput { id: 51, name: "Headphones".into(), default: false }],
            streams: vec![Stream { id: 80, app: "Firefox".into(), title: String::new(), volume: 60, muted: false }],
        }
    }

    fn set(s: &mut State, level: Level, value: u8) -> Vec<Effect> {
        s.applied(LevelEvent::Set(level, value).into())
    }

    #[test]
    fn the_volume_tab_watches_and_sets_levels() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Volume)), vec![Effect::Watch(Watch::Metrics, false), Effect::Watch(Watch::Audio, true)]);
        read(&mut s, Reading::Audio(audio()));
        read(&mut s, Reading::Status(Status { volume: Some(40), muted: true, ..Status::default() }));
        assert_eq!(set(&mut s, Level::Volume, 70), vec![Effect::Sound(Sound::Mute(false)), Effect::Sound(Sound::Volume(70))]);
        assert_eq!(set(&mut s, Level::Volume, 70), vec![], "no change, nothing sent");
        // A reading from before the finger moved doesn't jump the level back.
        read(&mut s, Reading::Status(Status { volume: Some(40), ..Status::default() }));
        assert_eq!(s.levels().volume(), Some(70));
        s.applied(PointerEvent::Up(Id::Level(Level::Volume)).into());
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
    fn setting_a_muted_streams_level_unmutes_it() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Volume));
        let mut muted = audio();
        if let Some(stream) = muted.streams.first_mut() {
            stream.muted = true;
        }
        read(&mut s, Reading::Audio(muted));
        // Unmuted first, then set, as for the output volume.
        assert_eq!(set(&mut s, Level::Stream(0), 30), vec![Effect::Sound(Sound::StreamMute(80, false)), Effect::Sound(Sound::StreamVolume(80, 30))]);
        assert!(s.levels().audio().streams.first().is_some_and(|stream| !stream.muted));
        assert_eq!(set(&mut s, Level::Stream(0), 40), vec![Effect::Sound(Sound::StreamVolume(80, 40))], "unmuted already");
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
        // The level is read from the first panel until ClamShell sets one, and shares are 100 until set.
        read(&mut s, Reading::Status(Status { brightness: Some(60), ..Status::default() }));
        assert_eq!(s.levels().level(Level::Brightness), Some(60));
        assert_eq!(s.levels().wanted_brightness(false, false), None, "nothing to apply before a level is set");
        read(&mut s, Reading::Backlights(vec!["top".into(), "bottom".into()]));
        assert_eq!(s.levels().level(Level::Backlight(1)), Some(100));
        let share = |backlight: &str, share| Share { backlight: backlight.into(), share };
        assert_eq!(set(&mut s, Level::Backlight(1), 70), vec![Effect::Brightness { level: 60, shares: vec![share("bottom", 70)] }]);
        // The bar keeps the screens the same relative to each other.
        s.applied(Slide::Start { control: Control::Brightness, x: 300.0, width: 620.0 }.into());
        let fx = s.applied(Slide::To { x: 360.0 }.into());
        assert!(matches!(fx.as_slice(), [Effect::Brightness { level, shares }] if *level > 60 && *shares == [share("bottom", 70)]), "{fx:?}");
        s.applied(Slide::End.into());
        assert_eq!(s.applied(LevelEvent::Step(Level::Brightness, -1).into()).len(), 1);
        // Saved a moment later, and restored after a restart.
        let saved = s.applied(Event::tick(10));
        let brightness = saved.iter().find_map(|e| match e {
            Effect::Save(Save::Brightness(b)) => Some(b),
            _ => None,
        });
        assert_eq!(brightness, Some(s.levels().brightness()));
    }

    #[test]
    fn dimming_never_raises_the_brightness() {
        let mut s = state();
        let dimmed = |s: &mut State, brightness| {
            read(s, Reading::Status(Status { brightness: Some(brightness), ..Status::default() }));
            s.levels().wanted_brightness(true, false).unwrap()
        };
        assert_eq!(dimmed(&mut s, 60), 20, "a third");
        assert_eq!(dimmed(&mut s, 2), 1, "not dark");
        assert_eq!(dimmed(&mut s, 0), 0, "not raised");
    }

    #[test]
    fn the_pad_steps_levels() {
        let mut s = state();
        read(&mut s, Reading::Status(Status { volume: Some(98), ..Status::default() }));
        assert_eq!(s.applied(LevelEvent::Step(Level::Volume, 1).into()), vec![Effect::Sound(Sound::Volume(100))], "up to 100");
        assert_eq!(s.applied(LevelEvent::Step(Level::Volume, 1).into()), vec![], "and no further");
    }

    #[test]
    fn the_thors_backlights_by_their_screens() {
        assert_eq!(backlight_label(&crate::Profile::thor(), "ae96000.dsi.0"), "Top screen");
        assert_eq!(backlight_label(&crate::Profile::thor(), "ae94000.dsi.0"), "Bottom screen");
        assert_eq!(backlight_label(&crate::Profile::thor(), "intel_backlight"), "intel_backlight");
        assert_eq!(backlight_label(&crate::Profile::generic(), "ae96000.dsi.0"), "ae96000.dsi.0");
    }

    /// The backlight reads the dimmed level while idle has dimmed the screens: that reading isn't taken
    /// for the screens' brightness, so they come back as they were, cycle after cycle.
    #[test]
    fn the_screens_come_back_from_dimming_at_the_brightness_they_had() {
        use crate::device::idle::IdleEvent;
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.applied(Event::tick(1000));
        s.applied(Event::Idle(IdleEvent::Watching(true)));
        let level = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Brightness { level, .. } = e { Some(*level) } else { None });
        for cycle in 0..3u64 {
            let at = 1000 + cycle * 1000;
            read(&mut s, Reading::Status(Status { brightness: Some(60), ..Status::default() }));
            let fx = s.applied(Event::tick(at + 400));
            assert_eq!(level(&fx), Some(20), "dimmed, cycle {cycle}: {fx:?}");
            // The status thread reads the backlight at its dimmed level.
            read(&mut s, Reading::Status(Status { brightness: Some(20), ..Status::default() }));
            let fx = s.applied(Event::Idle(IdleEvent::DeviceInput));
            assert_eq!(level(&fx), Some(60), "back, cycle {cycle}: {fx:?}");
            s.applied(Event::tick(at + 401));
        }
    }

    /// A level set while the screens are dimmed is sent dimmed: the screens don't light up at full.
    #[test]
    fn a_level_set_while_dimmed_is_sent_dimmed() {
        use crate::device::idle::IdleEvent;
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.applied(Event::tick(1000));
        s.applied(Event::Idle(IdleEvent::Watching(true)));
        read(&mut s, Reading::Status(Status { brightness: Some(60), ..Status::default() }));
        s.applied(Event::tick(1400));
        let fx = s.applied(LevelEvent::Set(Level::Brightness, 90).into());
        assert!(fx.contains(&Effect::Brightness { level: 30, shares: vec![] }), "{fx:?}");
    }
}
