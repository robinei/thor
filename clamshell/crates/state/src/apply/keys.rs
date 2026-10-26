//! The device's hardware keys.

use crate::bottom::notice::NoticeKind;
use crate::device::buttons::DeviceKey;
use crate::out::Out;
use crate::{Compositor, Control, Effect, Level, State};

impl State {
    /// A hardware key. A level key steps the level and shows it. The others take a screenshot or run
    /// their Home combination.
    pub(crate) fn on_device_key(&mut self, key: DeviceKey, out: &mut Out) {
        let (level, direction) = match key {
            DeviceKey::VolumeUp => (Level::Volume, 1),
            DeviceKey::VolumeDown => (Level::Volume, -1),
            DeviceKey::BrightnessUp => (Level::Brightness, 1),
            DeviceKey::BrightnessDown => (Level::Brightness, -1),
            // The level is shown only when the mute changed.
            DeviceKey::Mute => {
                if self.levels.toggle_mute(out) {
                    self.bottom.notice_mut().show(NoticeKind::Level(Control::Volume), self.clock.mono());
                }
                return;
            }
            DeviceKey::Screenshot => return out.effects.push(Effect::Compositor(Compositor::Screenshot)),
            DeviceKey::BottomView => return self.choose_tab(self.bottom.view_toggled()),
            DeviceKey::CloseWindow => return self.session.close_front_window(self.bottom.shows_windows(&self.session), out),
            DeviceKey::Quit => return self.session.quit_front(self.bottom.shows_windows(&self.session), out),
            DeviceKey::BottomPower => return self.bottom.toggle_power(self.power(), self.clock.mono()),
            DeviceKey::WindowDown => {
                if self.session.window_down(self.bottom.notice_mut(), self.clock.mono(), out) {
                    self.bottom.show_below();
                }
                return;
            }
            DeviceKey::WindowUp => return self.session.window_up(self.bottom.notice_mut(), self.clock.mono(), out),
            DeviceKey::WindowPrevious => return self.session.step_window(false),
            DeviceKey::WindowNext => return self.session.step_window(true),
        };
        let control = if level == Level::Volume { Control::Volume } else { Control::Brightness };
        self.bottom.notice_mut().show(NoticeKind::Level(control), self.clock.mono());
        self.levels.change(self.clock.mono(), |l| l.step(level, direction, out));
    }
}
