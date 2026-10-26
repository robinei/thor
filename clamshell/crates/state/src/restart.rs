//! A system thread started again after a panic (see clamshell-system's `os::worker::restarted`) has lost
//! what it held and the command it was carrying out. What the state had sent it and still stands is sent
//! again; a request it was carrying out ends as failed where the state waits for one. What is lost and
//! not recovered is said for each thread below.

use crate::out::Out;
use crate::sync::Resend;
use crate::{Effect, Save, State, Thread};

impl State {
    pub(crate) fn restarted(&mut self, thread: Thread, now: u64, out: &mut Out) {
        // A radio's attempt under way is lost with its thread: its busy mark goes.
        match thread {
            Thread::Wifi => self.radios.restarted(true, now),
            Thread::Bluetooth | Thread::BluetoothConnect => self.radios.restarted(false, now),
            _ => {}
        }
        match thread {
            // The keymap, the route, the activity watch and the settings applied (the rumble's strength).
            // The pad thread keeps its virtual controllers and the sticks' calibrations through a restart.
            // Watches: the open tab's, the bar's status and the account. The system thread also reads the
            // time as views open, and is asked to sleep again if it was last asked; a screenshot or a
            // notification it was handling is lost.
            Thread::Pad
            | Thread::Tuning
            | Thread::Mixer
            | Thread::Battery
            | Thread::Metrics
            | Thread::Status
            | Thread::Wifi
            | Thread::Bluetooth
            | Thread::System
            | Thread::Browse
            | Thread::Brightness => self.sent.resend(Resend::Restarted(thread)),
            // Everything saved, written again. A play being recorded is lost.
            Thread::Store => out.effects.extend(self.save_effects()),
            Thread::Icons => self.session.find_icons_again(out),
            Thread::Files => self.shelf.files_interrupted(&mut self.pointer, self.bottom.notice_mut(), now, out),
            Thread::Freezer => self.session.freeze_again(out),
            // A launch being started is its own thread, which ends as failed if it panics; a video's
            // position being read is lost (its progress bar isn't updated once).
            Thread::Launcher => {}
            // A Bluetooth connection under way ends (its busy mark went above); it can be tapped again.
            Thread::BluetoothConnect => {}
            // Nothing the state sent them stands: they read the system afresh as they start (the units
            // are listed again, input devices and the clipboard watched again, the saved clipboard
            // offered again), or carry out one command at a time (a sound, the touchpad's fingers).
            Thread::PadDevices
            | Thread::Tray
            | Thread::Sound
            | Thread::Units
            | Thread::Touchpad
            | Thread::Clock
            | Thread::InputWatch
            | Thread::Clipboard
            | Thread::Sleep
            | Thread::WaylandFilter => {}
        }
    }

    /// Everything the store holds that the state saves, as saves.
    fn save_effects(&self) -> Vec<Effect> {
        let mut saves = vec![
            self.prefs.save_effect(),
            self.keymapping.save_effect(),
            self.curving.save_effect(),
            Effect::Save(Save::Favourites(self.shelf.library().favourite_ids().map(str::to_owned).collect())),
            Effect::Save(Save::Resume(self.resume())),
            self.levels.brightness_save(),
            self.session.window_rules_save(),
            self.dock.save_effect(),
            self.session.autostart_save(),
            Effect::Save(Save::TwelveHour(self.sys.twelve_hour())),
            Effect::Save(Save::Sticks(self.device.calibrations().clone())),
        ];
        if !self.device.first_start() {
            saves.push(Effect::Save(Save::FirstStartDone));
        }
        saves
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::device::idle::Stage;
    use crate::testkit::*;
    use crate::{Effect, Event, Id, Save, Tab, Thread, Watch};

    #[test]
    fn a_restarted_pad_is_sent_its_keymap_route_and_settings() {
        let mut s = started(init());
        let fx = s.applied(Event::Restarted(Thread::Pad));
        assert!(fx.iter().any(|e| matches!(e, Effect::Keymap(_))), "{fx:?}");
        assert!(fx.iter().any(|e| matches!(e, Effect::PadRoute(_))), "{fx:?}");
        assert!(fx.iter().any(|e| matches!(e, Effect::PadActivity(_))), "{fx:?}");
        assert!(fx.iter().any(|e| matches!(e, Effect::Apply { .. })), "{fx:?}");
        assert!(s.applied(Event::tick(1)).is_empty(), "once");
    }

    #[test]
    fn a_restarted_reader_is_sent_the_watches() {
        let mut s = started(init());
        tap(&mut s, Id::Tab(Tab::Volume));
        let fx = s.applied(Event::Restarted(Thread::Mixer));
        assert!(fx.contains(&Effect::Watch(Watch::Audio, true)), "{fx:?}");
        assert!(fx.contains(&Effect::Watch(Watch::Status, true)), "{fx:?}");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Watch(Watch::Audio | Watch::Status, false))), "{fx:?}");
    }

    #[test]
    fn a_restarted_system_thread_is_asked_to_sleep_again() {
        let mut s = started(crate::Init { device_session: true, ..init() });
        s.applied(Event::tick(1000));
        s.applied(Event::Idle(crate::device::idle::IdleEvent::Watching(true)));
        assert!(s.applied(Event::tick(1900)).contains(&Effect::Sleep));
        assert_eq!(s.idle().stage(), Stage::Asleep);
        let fx = s.applied(Event::Restarted(Thread::System));
        assert!(fx.contains(&Effect::Sleep), "{fx:?}");
        assert!(!s.applied(Event::Restarted(Thread::Mixer)).contains(&Effect::Sleep), "only the system thread sleeps");
    }

    #[test]
    fn a_restarted_store_is_sent_everything_saved() {
        let mut s = started(init());
        let fx = s.applied(Event::Restarted(Thread::Store));
        let saved = |f: fn(&Save) -> bool| fx.iter().any(|e| matches!(e, Effect::Save(save) if f(save)));
        assert!(saved(|s| matches!(s, Save::Settings(_))) && saved(|s| matches!(s, Save::Keymaps(_))) && saved(|s| matches!(s, Save::Curves(_))));
        assert!(saved(|s| matches!(s, Save::Favourites(_))) && saved(|s| matches!(s, Save::Resume(_))) && saved(|s| matches!(s, Save::Brightness(_))));
        assert!(saved(|s| matches!(s, Save::WindowRules(_))) && saved(|s| matches!(s, Save::Dock(_))) && saved(|s| matches!(s, Save::Autostart(_))));
        assert!(saved(|s| matches!(s, Save::TwelveHour(_))) && saved(|s| matches!(s, Save::Sticks(_))) && saved(|s| matches!(s, Save::FirstStartDone)));
    }

    #[test]
    fn a_restarted_brightness_thread_is_sent_the_level_clamshell_set() {
        let mut s = started(init());
        s.applied(Event::Reading(crate::Reading::Status(crate::status::Status { brightness: Some(40), ..Default::default() })));
        assert!(s.applied(Event::Restarted(Thread::Brightness)).is_empty(), "a level only read isn't set");
        s.applied(crate::LevelEvent::Set(crate::Level::Brightness, 70).into());
        let fx = s.applied(Event::Restarted(Thread::Brightness));
        assert!(fx.iter().any(|e| matches!(e, Effect::Brightness { level: 70, .. })), "{fx:?}");
    }

    #[test]
    fn a_restarted_icons_thread_is_asked_again() {
        let mut s = started(crate::Init { device_session: true, ..init() });
        s.observed(vec![crate::session::windows::WindowFacts { pids: vec![500], unit: Some("app-thing.scope".into()), ..win(4, "org.x.Thing") }]);
        let fx = s.applied(Event::Restarted(Thread::Icons));
        assert!(fx.iter().any(|e| matches!(e, Effect::FindIcon { app_id, .. } if app_id == "org.x.Thing")), "{fx:?}");
    }
}
