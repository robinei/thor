//! The device's own buttons and what they do: volume and brightness up and down, mute, a screenshot, and
//! the Home combinations. Each one says what happened in a notice (see [`crate::bottom::notice`]).

use crate::bottom::notice::NoticeKind;
use crate::effect::{Effect, WindowCommand};
use crate::levels::Level;
use crate::levels::slider::Control;
use crate::out::Out;
use crate::session::windows::{SHELF_APP_ID, Window};
use crate::{EntryAct, Event, Session, State};

/// A hardware key, or the same key requested through the control pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKey {
    VolumeUp,
    VolumeDown,
    Mute,
    BrightnessUp,
    BrightnessDown,
    /// Save both screens to a file. The pad's Home with RB, or a keyboard's Print.
    Screenshot,
    /// Switch the bottom screen between ClamShell's view and the windows moved there (Home+Select).
    BottomView,
    /// Home with B, released at once, closes the window in front.
    CloseWindow,
    /// Home with B, held, quits the program in front.
    Quit,
    /// Turn the bottom screen off, or on again (Home+Start).
    BottomPower,
    /// Move the window in front on the top screen to the bottom screen (Home+Down).
    WindowDown,
    /// Move the window the bottom screen shows back to the top screen (Home+Up).
    WindowUp,
}

impl State {
    /// A hardware key. A level key steps the level and shows it. The others take a screenshot or run
    /// their Home combination.
    pub(crate) fn on_device_key(&mut self, key: DeviceKey, out: &mut Out) {
        let (level, direction) = match key {
            DeviceKey::VolumeUp => (Level::Volume, 1),
            DeviceKey::VolumeDown => (Level::Volume, -1),
            DeviceKey::BrightnessUp => (Level::Brightness, 1),
            DeviceKey::BrightnessDown => (Level::Brightness, -1),
            DeviceKey::Mute => {
                self.bottom.notify(NoticeKind::Level(Control::Volume), self.clock.mono());
                return self.levels.toggle_mute(out);
            }
            DeviceKey::Screenshot => return out.effects.push(Effect::Screenshot),
            DeviceKey::BottomView => return self.bottom.toggle_view(&self.session),
            DeviceKey::CloseWindow => return self.close_front_window(out),
            DeviceKey::Quit => return self.quit_front(out),
            DeviceKey::BottomPower => return self.bottom.toggle_power(self.power(), self.clock.mono(), out),
            DeviceKey::WindowDown => return self.bottom.window_down(&self.session, self.clock.mono(), out),
            DeviceKey::WindowUp => return self.bottom.window_up(&self.session, self.clock.mono(), out),
        };
        let control = if level == Level::Volume { Control::Volume } else { Control::Brightness };
        self.bottom.notify(NoticeKind::Level(control), self.clock.mono());
        self.levels.change(self.clock.mono(), |l| l.step(level, direction, out));
    }

    /// Home with B, released at once, asks that window (see [`Session::home_b_window`]) to close.
    pub(crate) fn close_front_window(&self, out: &mut Out) {
        if let Some(w) = self.session.home_b_window(self.bottom.shows_windows(&self.session)) {
            out.effects.push(Effect::Window(WindowCommand::Close(vec![w.id])));
        }
    }

    /// Home with B, held, quits that window's entry, as Quit in its menu would. Any other window is just
    /// closed.
    pub(crate) fn quit_front(&mut self, out: &mut Out) {
        let Some(w) = self.session.home_b_window(self.bottom.shows_windows(&self.session)) else {
            return;
        };
        let (id, entry) = (w.id, self.session.running_of(w).map(|r| r.id.clone()));
        match entry {
            Some(entry) => out.events.push(Event::Entry(entry, EntryAct::Quit)),
            None => out.effects.push(Effect::Window(WindowCommand::Close(vec![id]))),
        }
    }

    /// The screenshot was taken, with its file, or failed.
    pub(crate) fn on_screenshot(&mut self, result: Result<String, String>) {
        self.bottom.notify(NoticeKind::Screenshot(result), self.clock.mono());
    }
}

impl Session {
    /// The window Home with B acts on. It is the one in front on the top screen, other than shelf's. If
    /// there is none, it is the one in view on the bottom screen while it shows the windows moved there
    /// (`shows_windows`).
    pub(crate) fn home_b_window(&self, shows_windows: bool) -> Option<&Window> {
        let windows = &self.windows;
        windows
            .iter()
            .find(|w| w.focused && !w.down && w.app_id != SHELF_APP_ID)
            .or_else(|| shows_windows.then(|| windows.iter().filter(|w| w.down).max_by_key(|w| w.visible)).flatten())
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
