//! The device's own buttons and what they do: volume and brightness up and down, mute, a screenshot; and
//! the short notice under the bar that says what happened (the new level, the screenshot saved).

use crate::State;
use crate::effect::Effect;
use crate::levels::Level;
use crate::slider::Control;

/// A hardware key (or the same asked for through the control pipe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKey {
    VolumeUp,
    VolumeDown,
    Mute,
    BrightnessUp,
    BrightnessDown,
    /// Both screens to a file (the pad's Home with RB, a keyboard's Print).
    Screenshot,
    /// The bottom screen shows the next of ClamShell's view and the windows moved there (Home+Select).
    BottomView,
    /// Home with B, let go at once: the window in front closes.
    CloseWindow,
    /// Home with B, held: the program in front quits.
    Quit,
    /// The bottom screen off, or on again (Home+Start).
    BottomPower,
    /// The window in front on the top screen to the bottom screen (Home+Down).
    WindowDown,
    /// The window the bottom screen shows back to the top screen (Home+Up).
    WindowUp,
}

/// How long a notice shows, in seconds.
const NOTICE_FOR: u64 = 2;

/// A notice under the bar, until it times out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub what: NoticeKind,
    until: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// A level changed by a key: show it.
    Level(Control),
    /// A screenshot was saved (its file), or wasn't (why).
    Screenshot(Result<String, String>),
    /// The gyro calibrating (none: hold still), or done (whether it took).
    Gyro(Option<bool>),
    /// A window moved down is remembered for its program (true), or moved up and forgotten (false).
    BottomRule(bool),
    /// A file operation ended: what was done, or what went wrong.
    Files(Result<String, String>),
    /// Entries enqueued: how many, and to what (the running entry's title).
    Enqueued(usize, String),
    /// A running entry asked for has no window to show (its title).
    NoWindow(String),
    /// A launch didn't start (its entry's title; why is in the log).
    NotStarted(String),
    /// A launch asked for (its entry's title): it is starting.
    Starting(String),
    /// The library read anew has problems (the first, and how many: all are listed on the Tools tab's
    /// Rescan library).
    Problems(String, usize),
}

impl State {
    /// A hardware key: the level stepped (and shown), or a screenshot asked for.
    pub(crate) fn on_device_key(&mut self, key: DeviceKey) -> Vec<Effect> {
        let (level, direction) = match key {
            DeviceKey::VolumeUp => (Level::Volume, 1),
            DeviceKey::VolumeDown => (Level::Volume, -1),
            DeviceKey::BrightnessUp => (Level::Brightness, 1),
            DeviceKey::BrightnessDown => (Level::Brightness, -1),
            DeviceKey::Mute => {
                self.notify(NoticeKind::Level(Control::Volume));
                return self.levels.toggle_mute();
            }
            DeviceKey::Screenshot => return vec![Effect::Screenshot],
            DeviceKey::BottomView => return self.toggle_bottom_view(),
            DeviceKey::CloseWindow => return self.close_front_window(),
            DeviceKey::Quit => return self.quit_front(),
            DeviceKey::BottomPower => return self.toggle_bottom_power(),
            DeviceKey::WindowDown => return self.window_down(),
            DeviceKey::WindowUp => return self.window_up(),
        };
        let control = if level == Level::Volume { Control::Volume } else { Control::Brightness };
        self.notify(NoticeKind::Level(control));
        self.change_levels(|l| l.step(level, direction))
    }

    /// The screenshot is taken (its file), or failed.
    pub(crate) fn on_screenshot(&mut self, result: Result<String, String>) {
        self.notify(NoticeKind::Screenshot(result));
    }

    pub(crate) fn notify(&mut self, what: NoticeKind) {
        self.bottom.notice = Some(Notice { what, until: self.unix_secs + NOTICE_FOR });
    }

    /// The notice timed out at `now`: whether it went.
    pub(crate) fn expire_notice(&mut self, now: u64) -> bool {
        let expired = self.bottom.notice.as_ref().is_some_and(|n| now >= n.until);
        if expired {
            self.bottom.notice = None;
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::Sound;
    use crate::sys::Status;
    use crate::testkit::state;
    use crate::{Event, Reading};

    #[test]
    fn keys_step_the_levels_and_show_them_for_a_moment() {
        let mut s = state();
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Reading::Status(Status { volume: Some(50), brightness: Some(40), ..Status::default() }).into());
        assert_eq!(s.apply(Event::Device(DeviceKey::VolumeUp)).1, vec![Effect::Sound(Sound::Volume(55))]);
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::Level(Control::Volume)));
        assert!(matches!(s.apply(Event::Device(DeviceKey::BrightnessDown)).1.as_slice(), [Effect::Brightness { level: 35, .. }]));
        assert_eq!(s.apply(Event::Device(DeviceKey::Mute)).1, vec![Effect::Sound(Sound::Mute(true))]);
        s.apply(Event::Tick { unix_secs: 101 });
        assert!(s.bottom().notice().is_some());
        s.apply(Event::Tick { unix_secs: 102 });
        assert!(s.bottom().notice().is_none(), "gone after two seconds");
    }

    #[test]
    fn a_screenshot_is_asked_for_and_its_result_shown() {
        let mut s = state();
        assert_eq!(s.apply(Event::Device(DeviceKey::Screenshot)).1, vec![Effect::Screenshot]);
        s.apply(Event::Screenshot(Ok("/home/thor/Pictures/Screenshots/a.png".into())));
        assert!(matches!(s.bottom().notice().map(|n| &n.what), Some(NoticeKind::Screenshot(Ok(_)))));
    }
}
