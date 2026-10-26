//! What comes in from the outside and needs State: the clock's ticks, the system's readings, the
//! library read again, the UI's start, back from sleep, Home and quitting.

use crate::bottom::notice::NoticeKind;
use crate::out::Out;
use crate::resume::SAVE_AFTER;
use crate::sync::Resend;
use crate::{Compositor, Effect, Library, Reading, Save, State};

impl State {
    /// The clock ticked. Once a minute, the bar's clock and both screens' burn-in orbit move on. Changes
    /// are saved once they have stopped for a moment.
    pub(crate) fn tick(&mut self, unix_secs: u64, mono_secs: u64, out: &mut Out) {
        // The first tick. Anything idle heard before it was timed at 0, so count from now.
        if !self.clock.read() {
            self.idle.resumed(mono_secs);
        }
        self.clock.tick(unix_secs, mono_secs);
        if self.resume.due(mono_secs, SAVE_AFTER) {
            out.effects.push(Effect::Save(Save::Resume(self.resume())));
        }
        self.levels.save_due(mono_secs, out);
        self.bottom.expire_notice(mono_secs);
    }

    /// A reading of the system from a system thread. It is stored, and after a status reading the
    /// battery warning is checked (a notification of ClamShell's own).
    pub(crate) fn on_reading(&mut self, reading: Reading, out: &mut Out) {
        let status = matches!(reading, Reading::Status(_));
        match reading {
            // The volume, mute and brightness go to the levels, where a finger on a slider wins over the
            // reading.
            Reading::Status(status) => {
                self.levels.read(status.volume, status.muted, status.brightness, self.idle.stage() >= crate::device::idle::Stage::Dimmed);
                self.sys.read_status(status);
            }
            Reading::Clipboard(text) => self.bottom.read_clipboard(text),
            Reading::Icon { entry, path } => self.session.set_icon(&entry, path),
            Reading::Metrics(metrics) => self.sys.read_metrics(metrics),
            Reading::Frames { fps, frame_ms } => self.sys.frames(fps, frame_ms),
            Reading::Screens(screens) => self.presentation.read_screens(*screens),
            Reading::Histogram(histogram) => self.presentation.read_histogram(*histogram),
            Reading::Told { unit, steam_app, resolution } => self.session.told(unit.as_deref(), steam_app, resolution),
            Reading::XwaylandModes(modes) => self.presentation.read_xwayland_modes(modes),
            Reading::Wifi(wifi) => self.radios.read_wifi(wifi),
            Reading::Bluetooth(bluetooth) => self.radios.read_bluetooth(bluetooth),
            Reading::Battery(battery) => self.sys.read_battery(battery),
            Reading::Audio(audio) => self.levels.read_audio(audio),
            Reading::Backlights(backlights) => self.levels.read_backlights(backlights),
            Reading::Time(time) => self.sys.read_time(time),
            Reading::Account(account) => self.device.read_account(account),
        }
        if status && let Some(warning) = self.sys.battery_warning() {
            self.notify(warning, out);
        }
    }

    /// The library was read again. New problems are reported once and shelf is reloaded. Then what
    /// depends on its entries is updated: runners' pauses and keymaps, and pins of entries that are
    /// gone. Shelf's entries shown are asked for first again.
    pub(crate) fn library_read(&mut self, library: Library, out: &mut Out) {
        let problems = &library.problems;
        if !problems.is_empty() && *problems != self.shelf.library().problems {
            let first = problems.first().cloned().unwrap_or_default();
            self.bottom.notice_mut().show(crate::bottom::notice::NoticeKind::Problems(first, problems.len()), self.clock.mono());
        }
        if self.shelf.reload(library) {
            self.shelf.moved(&mut self.pointer);
        }
        self.session.set_pausable_runners(self.shelf.library());
        self.adopt_runner_keymaps(out);
        if self.dock.drop_gone(self.shelf.library(), &self.session) {
            out.effects.push(self.dock.save_effect());
        }
        self.sent.resend(Resend::FetchFirst);
    }

    /// Metadata fetching started or stopped. Shelf's entries shown are asked for first again.
    pub(crate) fn fetching(&mut self, on: bool) {
        self.shelf.set_fetching(on);
        self.sent.resend(Resend::FetchFirst);
    }

    /// Back from sleep: idle counts from now, and the settings in force are applied again.
    pub(crate) fn resumed(&mut self) {
        self.idle.resumed(self.clock.mono());
        self.sent.resend(Resend::Resumed);
    }

    /// The refresh rate calibration ended: the bottom screen comes on again and the main screen's rate
    /// is the settings' again, as sync finds.
    pub(crate) fn refresh_calibrated(&mut self, result: Result<String, String>) {
        self.device.refresh_done(result);
    }

    /// The UI is up. Sends what was waiting for it: the dock if pins were dropped at start and the
    /// runners' keymaps. The open tab's and the account's watches start from now.
    pub(crate) fn ui_started(&mut self, out: &mut Out) {
        self.ui_up = true;
        if self.dock.take_unsaved() {
            out.effects.push(self.dock.save_effect());
        }
        self.adopt_runner_keymaps(out);
    }

    /// Home. On a screen that listens for input, it leaves that screen; Home is never taken as an input.
    /// When the Device view is in shelf's place, it goes back to shelf. Otherwise the session handles it
    /// (see [`crate::Session::home`]).
    pub(crate) fn home(&mut self) {
        if self.listening() {
            self.keymapping.back(&mut self.keyboard);
        } else if self.session.shelf_in_front() && self.device.is_open() {
            self.device.close(&mut self.pointer, self.shelf.selected());
        } else {
            self.session.home();
        }
    }

    /// Quitting ClamShell: where shelf and the second screen are is saved first.
    pub(crate) fn quit(&mut self, out: &mut Out) {
        // What waits a moment to be saved is saved now.
        self.levels.save_due(u64::MAX, out);
        out.effects.push(Effect::Save(Save::Resume(self.resume())));
        out.effects.push(Effect::Compositor(Compositor::Quit));
    }

    /// Calibrates the gyro, with a notice to hold the device still.
    pub(crate) fn calibrate_gyro(&mut self, out: &mut Out) {
        self.bottom.notice_mut().show(NoticeKind::Gyro(None), self.clock.mono());
        out.effects.push(Effect::CalibrateGyro);
    }

    /// For runners that come with one of ClamShell's keymaps (see [`crate::Library::runner_keymaps`]) and
    /// have no keymap chosen and none of their own: makes that keymap their own, chooses it, and saves.
    fn adopt_runner_keymaps(&mut self, out: &mut Out) {
        let mut adopted = false;
        for (key, preset) in &self.shelf.library().runner_keymaps {
            if self.prefs.settings().overrides_of(key).get(crate::settings::Setting::Keymap).is_some() {
                continue;
            }
            if let Some(id) = self.keymapping.adopt_preset(key, preset) {
                self.prefs.choose(Some(key), crate::settings::Setting::Keymap, id);
                adopted = true;
            }
        }
        if adopted {
            out.effects.push(self.keymapping.save_effect());
            out.effects.push(self.prefs.save_effect());
        }
    }
}
