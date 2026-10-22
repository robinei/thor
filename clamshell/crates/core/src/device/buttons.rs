//! The device's own buttons and what they do: volume and brightness up and down, mute, a screenshot, and
//! the Home combinations; each says what happened in a notice (see [`crate::bottom::notice`]).

use crate::State;
use crate::bottom::notice::NoticeKind;
use crate::effect::{Effect, WindowCommand};
use crate::levels::Level;
use crate::levels::slider::Control;
use crate::windows::{SHELF_APP_ID, Window};

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

    /// The window Home with B acts on: the one in front on the top screen (not shelf's); with none
    /// there, the one in view on the bottom screen while it shows the windows moved there.
    fn home_b_window(&self) -> Option<&Window> {
        let windows = &self.session.windows;
        windows
            .iter()
            .find(|w| w.focused && !w.down && w.app_id != SHELF_APP_ID)
            .or_else(|| self.bottom_shows_windows().then(|| windows.iter().filter(|w| w.down).max_by_key(|w| w.visible)).flatten())
    }

    /// Home with B, let go at once: that window (see [`State::home_b_window`]) closes, politely.
    pub(crate) fn close_front_window(&self) -> Vec<Effect> {
        match self.home_b_window() {
            Some(w) => vec![Effect::Window(WindowCommand::Close(vec![w.id]))],
            None => vec![],
        }
    }

    /// Home with B, held: that window's entry quits, as its menu's Quit would; any other window just
    /// closes.
    pub(crate) fn quit_front(&mut self) -> Vec<Effect> {
        let Some(w) = self.home_b_window() else {
            return vec![];
        };
        let (id, entry) = (w.id, self.session.running_of(w).map(|r| r.id.clone()));
        match entry {
            Some(entry) => self.quit(&entry),
            None => vec![Effect::Window(WindowCommand::Close(vec![id]))],
        }
    }

    /// The screenshot is taken (its file), or failed.
    pub(crate) fn on_screenshot(&mut self, result: Result<String, String>) {
        self.notify(NoticeKind::Screenshot(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::Sound;
    use crate::status::Status;
    use crate::testkit::state;
    use crate::{Event, Reading};

    #[test]
    fn keys_step_the_levels_and_show_them_for_a_moment() {
        let mut s = state();
        s.apply(Event::tick(100));
        s.apply(Reading::Status(Status { volume: Some(50), brightness: Some(40), ..Status::default() }).into());
        assert_eq!(s.apply(Event::Device(DeviceKey::VolumeUp)).1, vec![Effect::Sound(Sound::Volume(55))]);
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::Level(Control::Volume)));
        assert!(matches!(s.apply(Event::Device(DeviceKey::BrightnessDown)).1.as_slice(), [Effect::Brightness { level: 35, .. }]));
        assert_eq!(s.apply(Event::Device(DeviceKey::Mute)).1, vec![Effect::Sound(Sound::Mute(true))]);
        s.apply(Event::tick(101));
        assert!(s.bottom().notice().is_some());
        s.apply(Event::tick(102));
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
