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
            // A sleep it was asked for may not have happened. It is taken as refused for now (Busy), and
            // asked again after the retry, by when a wake would have come had the device slept.
            Thread::System if self.idle.stage() == crate::device::idle::Stage::Asleep => {
                self.idle.heard(crate::device::idle::IdleEvent::Busy, now);
            }
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
            Thread::Store => self.save_everything(out),
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
            // Nothing: the notifications are the state's, and the thread holds only its bus name, which it
            // asks for again. Its ids come from a counter that outlives it, so it doesn't give out an id
            // the state still holds. A signal it was emitting is lost.
            Thread::Notifications => {}
        }
    }

    /// Saves everything the store holds that the state saves.
    fn save_everything(&self, out: &mut Out) {
        out.effects.extend([
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
        ]);
        if !self.device.first_start() {
            out.effects.push(Effect::Save(Save::FirstStartDone));
        }
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

    /// The settings thread applies the settings in force; started again, it is sent them, once.
    #[test]
    fn a_restarted_tuning_thread_is_sent_the_settings_in_force() {
        let mut s = started(init());
        let settings = s.prefs().settings().global;
        assert_eq!(s.applied(Event::Restarted(Thread::Tuning)), vec![Effect::Apply { entry: None, settings }]);
        assert!(s.applied(Event::tick(1)).is_empty(), "once");
    }

    /// Each reader started again is sent the watch of the tab open that it serves (see the system crate's
    /// `Handles::watch`), and the bar's status watch, whichever reader it is.
    #[test]
    fn each_restarted_reader_is_sent_its_tabs_watch() {
        let cases = [
            (Thread::Mixer, Tab::Volume, Watch::Audio),
            (Thread::Mixer, Tab::Brightness, Watch::Backlights),
            (Thread::Wifi, Tab::Wifi, Watch::Wifi),
            (Thread::Bluetooth, Tab::Bluetooth, Watch::Bluetooth),
            (Thread::Battery, Tab::Battery, Watch::Battery),
            (Thread::Metrics, Tab::Sysinfo, Watch::Metrics),
            (Thread::System, Tab::Time, Watch::Time),
        ];
        for (thread, tab, watch) in cases {
            let mut s = started(init());
            tap(&mut s, Id::Tab(tab));
            let fx = s.applied(Event::Restarted(thread));
            assert!(fx.contains(&Effect::Watch(watch, true)) && fx.contains(&Effect::Watch(Watch::Status, true)), "{thread:?}: {fx:?}");
            assert!(!fx.iter().any(|e| matches!(e, Effect::Watch(w, false) if *w == watch || *w == Watch::Status)), "{thread:?}: {fx:?}");
        }
        let mut s = started(init());
        let fx = s.applied(Event::Restarted(Thread::Status));
        assert!(fx.contains(&Effect::Watch(Watch::Status, true)), "{fx:?}");
    }

    /// The system thread reads the account while the Device view is open; started again, it is told to
    /// read it again.
    #[test]
    fn a_restarted_system_thread_is_sent_the_account_watch() {
        let mut s = started(crate::Init { first_start: true, ..init() });
        assert!(s.device().is_open());
        let fx = s.applied(Event::Restarted(Thread::System));
        assert!(fx.contains(&Effect::Watch(Watch::Account, true)), "{fx:?}");
        assert!(!s.applied(Event::Restarted(Thread::Pad)).contains(&Effect::Watch(Watch::Account, true)), "only the readers' restart");
    }

    #[test]
    fn a_restarted_system_thread_is_asked_to_sleep_again_after_the_retry() {
        let mut s = started(crate::Init { device_session: true, ..init() });
        s.applied(Event::tick(1000));
        s.applied(Event::Idle(crate::device::idle::IdleEvent::Watching(true)));
        assert!(s.applied(Event::tick(1900)).contains(&Effect::Sleep));
        assert_eq!(s.idle().stage(), Stage::Asleep);
        // Not at once: had the device slept, its wake would come first. After the retry, if still asleep.
        assert!(!s.applied(Event::Restarted(Thread::System)).contains(&Effect::Sleep));
        assert!(!s.applied(Event::Restarted(Thread::Mixer)).contains(&Effect::Sleep), "only the system thread sleeps");
        assert!(s.applied(Event::tick(1900 + crate::device::idle::SLEEP_RETRY)).contains(&Effect::Sleep));
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
    fn a_restarted_notifications_thread_is_sent_nothing_and_they_stay() {
        use crate::notifications::{Notification, NotificationEvent, NotificationId, Urgency};
        let mut s = started(init());
        let n = Notification {
            id: NotificationId::Bus(3),
            app: "mail".into(),
            summary: "new".into(),
            body: String::new(),
            urgency: Urgency::Normal,
            action: None,
            timeout_ms: 0,
        };
        s.applied(NotificationEvent::Notified(Box::new(n)).into());
        assert_eq!(s.applied(Event::Restarted(Thread::Notifications)), []);
        assert!(s.notifications().get(NotificationId::Bus(3)).is_some());
    }

    #[test]
    fn a_restarted_icons_thread_is_asked_again() {
        let mut s = started(crate::Init { device_session: true, ..init() });
        s.observed(vec![crate::session::windows::WindowFacts { pids: vec![500], unit: Some("app-thing.scope".into()), ..win(4, "org.x.Thing") }]);
        let fx = s.applied(Event::Restarted(Thread::Icons));
        assert!(fx.iter().any(|e| matches!(e, Effect::FindIcon { app_id, .. } if app_id == "org.x.Thing")), "{fx:?}");
    }
}
