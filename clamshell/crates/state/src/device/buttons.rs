//! The device's own buttons and what they do: volume and brightness up and down, mute, a screenshot, and
//! the Home combinations. Each one says what happened in a notice (see [`crate::bottom::notice`]).

/// A hardware key, or the same key requested through the control pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeviceKey {
    VolumeUp,
    VolumeDown,
    Mute,
    BrightnessUp,
    BrightnessDown,
    /// Save both screens to a file. The pad's Home with RB, or a keyboard's Print.
    Screenshot,
    /// Switch the bottom screen between ClamShell's view and the windows moved there (Home + right stick press).
    BottomView,
    /// Home with B, released at once, closes the window in front.
    CloseWindow,
    /// Home with B, held, quits the program in front.
    Quit,
    /// Turn the bottom screen off, or on again (Home+Start).
    BottomPower,
    /// Move the window in front on the top screen to the bottom screen (Home + right stick down).
    WindowDown,
    /// Move the window the bottom screen shows back to the top screen (Home + right stick up).
    WindowUp,
    /// The previous top window of the program shown on top.
    WindowPrevious,
    /// The next top window of the program shown on top.
    WindowNext,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bottom::notice::NoticeKind;
    use crate::levels::Sound;
    use crate::levels::slider::Control;
    use crate::status::Status;
    use crate::testkit::Applied;
    use crate::testkit::state;
    use crate::{Effect, Event, Reading};

    #[test]
    fn keys_step_the_levels_and_show_them_for_a_moment() {
        let mut s = state();
        s.applied(Event::tick(100));
        s.applied(Reading::Status(Status { volume: Some(50), brightness: Some(40), ..Status::default() }).into());
        assert_eq!(s.applied(Event::Device(DeviceKey::VolumeUp)), vec![Effect::Sound(Sound::Volume(55))]);
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::Level(Control::Volume)));
        assert!(matches!(s.applied(Event::Device(DeviceKey::BrightnessDown)).as_slice(), [Effect::Brightness { level: 35, .. }]));
        assert_eq!(s.applied(Event::Device(DeviceKey::Mute)), vec![Effect::Sound(Sound::Mute(true))]);
        s.applied(Event::tick(101));
        assert!(s.bottom().notice().is_some());
        s.applied(Event::tick(102));
        assert!(s.bottom().notice().is_none(), "gone after two seconds");
    }

    #[test]
    fn the_mute_key_shows_the_level_only_when_it_changes_something() {
        let mut s = state();
        s.applied(Event::tick(100));
        assert_eq!(s.applied(Event::Device(DeviceKey::Mute)), vec![], "the volume unknown");
        assert!(s.bottom().notice().is_none());
        s.applied(Reading::Status(Status { volume: Some(50), ..Status::default() }).into());
        assert_eq!(s.applied(Event::Device(DeviceKey::Mute)), vec![Effect::Sound(Sound::Mute(true))]);
        assert_eq!(s.bottom().notice().map(|n| &n.what), Some(&NoticeKind::Level(Control::Volume)));
    }

    #[test]
    fn a_screenshot_is_asked_for_and_its_result_shown() {
        let mut s = state();
        assert_eq!(s.applied(Event::Device(DeviceKey::Screenshot)), vec![Effect::Screenshot]);
        s.applied(Event::Screenshot(Ok("/home/thor/Pictures/Screenshots/a.png".into())));
        assert!(matches!(s.bottom().notice().map(|n| &n.what), Some(NoticeKind::Screenshot(Ok(_)))));
    }
}
