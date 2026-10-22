//! The Device view: on the main screen in shelf's place ([`TopView`]), opened by the Tools' Device entry
//! (from shelf, or pinned to the dock), and by itself at ClamShell's first start. What a device needs set
//! up once and now and then, a row each: Wi-Fi (its tab), the time zone (the time tab), the password and
//! remote login (see [`crate::device::account`]); the controller calibrated, its sticks ([`crate::device::sticks`]) and
//! its gyro; the top screen's refresh rate calibrated (thor-panel-calibration); Done at the first start.
//! B, Home or Close go back to shelf.

pub mod account;
pub mod buttons;
pub mod idle;
pub mod sticks;

use crate::DeviceId;
use crate::State;
use crate::bottom::Tab;
use crate::bottom::editor::{Editor, Purpose};
use crate::bottom::notice::NoticeKind;
use crate::effect::{Effect, Save, WindowCommand};
use crate::kit::secret::Secret;
use crate::{Id, ShelfId};
use account::{AccountCommand, PasswordStep};
use sticks::{Calibrations, Progress, Sticks};

/// What the main screen shows of ClamShell's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TopView {
    #[default]
    Shelf,
    Device,
}

/// The refresh rate's calibration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Refresh {
    #[default]
    Idle,
    Running,
    /// What it said: the rates before and after, or why it failed.
    Done(Result<String, String>),
}

/// The Device view's own state.
#[derive(Debug, Clone, Default)]
pub struct Device {
    pub(crate) open: bool,
    /// A sticks' calibration going: how far.
    pub(crate) sticks: Option<Progress>,
    /// The last sticks' calibration failed (until another starts).
    pub(crate) sticks_failed: bool,
    pub(crate) refresh: Refresh,
    /// The sticks' calibrations, by controller (saved; the pad thread has them too).
    pub(crate) calibrations: Calibrations,
}

impl Device {
    pub fn sticks(&self) -> Option<&Progress> {
        self.sticks.as_ref()
    }

    pub fn sticks_failed(&self) -> bool {
        self.sticks_failed
    }

    pub fn refresh(&self) -> &Refresh {
        &self.refresh
    }

    /// Whether the sticks of the controller named `name` are calibrated.
    pub fn calibrated(&self, name: &str) -> bool {
        self.calibrations.contains_key(name)
    }

    pub fn calibrations(&self) -> &Calibrations {
        &self.calibrations
    }
}

/// What the pad thread says of a sticks' calibration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SticksEvent {
    Progress(Progress),
    /// Done: the controller's name and its sticks, or none (given up).
    Done(String, Option<Sticks>),
}

impl State {
    pub fn top_view(&self) -> TopView {
        if self.device.open { TopView::Device } else { TopView::Shelf }
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The Device view opened on the main screen (shelf's window brought to the front), the pad's focus
    /// on its first row (the account is read while it is open: [`State::follow_watch`]).
    pub(crate) fn open_device(&mut self) -> Vec<Effect> {
        self.device.open = true;
        self.pointer.focus = Some(Id::Device(DeviceId::Wifi));
        vec![Effect::Window(WindowCommand::ShowShelf)]
    }

    /// Back to shelf, the focus on its selection.
    pub(crate) fn close_device(&mut self) -> Vec<Effect> {
        self.device.open = false;
        self.pointer.focus = self.shelf.selected().cloned().map(|g| Id::Shelf(ShelfId::Entry(g)));
        vec![]
    }

    /// A tap on one of the Device view's targets.
    pub(crate) fn on_device(&mut self, id: DeviceId) -> Vec<Effect> {
        let account = self.sys.account;
        match id {
            DeviceId::Wifi => self.choose_tab(Tab::Wifi),
            DeviceId::Zone => self.choose_tab(Tab::Time),
            // (Typed on the second screen's keyboard, in the editor.)
            DeviceId::Password if account.available => {
                let step = if account.password { PasswordStep::Current } else { PasswordStep::New { current: None } };
                self.bottom.open_editor(Editor { purpose: Purpose::AccountPassword(step), text: String::new() });
                self.choose_tab(Tab::Keyboard)
            }
            // (Remote login needs a password to log in with.)
            DeviceId::Ssh if account.available && account.password => vec![Effect::Account(AccountCommand::Ssh(!account.ssh))],
            DeviceId::Password | DeviceId::Ssh => vec![],
            DeviceId::Sticks if self.device.sticks.is_none() => {
                self.device.sticks_failed = false;
                vec![Effect::CalibrateSticks]
            }
            DeviceId::Sticks => vec![],
            DeviceId::Gyro => {
                self.notify(NoticeKind::Gyro(None));
                vec![Effect::CalibrateGyro]
            }
            DeviceId::Refresh if self.device.refresh != Refresh::Running => {
                self.device.refresh = Refresh::Running;
                vec![Effect::CalibrateRefresh]
            }
            DeviceId::Refresh => vec![],
            // The first time: never again; shelf after it, and the device's default tab below.
            DeviceId::Done if self.first_start => {
                self.first_start = false;
                let mut effects = vec![Effect::Save(Save::FirstStartDone)];
                effects.extend(self.close_device());
                effects.extend(self.choose_tab(Tab::default()));
                effects
            }
            DeviceId::Done | DeviceId::Close => self.close_device(),
        }
    }

    /// The pad thread's word on a sticks' calibration: its progress shown; its result saved (and the pad
    /// thread uses it already), or a failure said.
    pub(crate) fn on_sticks(&mut self, event: SticksEvent) {
        match event {
            SticksEvent::Progress(p) => self.device.sticks = Some(p),
            SticksEvent::Done(name, result) => {
                self.device.sticks = None;
                self.device.sticks_failed = result.is_none();
                if let Some(sticks) = result {
                    self.device.calibrations.insert(name, sticks);
                }
            }
        }
    }

    /// The calibrations to save after a sticks' calibration, if it took.
    pub(crate) fn sticks_saved(&self) -> Effect {
        Effect::Save(Save::Sticks(self.device.calibrations.clone()))
    }

    /// The refresh rate's calibration ended: what it said.
    pub(crate) fn on_refresh(&mut self, result: Result<String, String>) {
        self.device.refresh = Refresh::Done(result);
    }

    /// The password's editor saved: the new one asked for after the current one, or the password set.
    pub(crate) fn password_typed(&mut self, step: PasswordStep, text: String) -> Vec<Effect> {
        match step {
            PasswordStep::Current => {
                let step = PasswordStep::New { current: Some(Secret(text)) };
                self.bottom.open_editor(Editor { purpose: Purpose::AccountPassword(step), text: String::new() });
                vec![]
            }
            PasswordStep::New { current } => vec![Effect::Account(AccountCommand::Password { current, new: Secret(text) })],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::DeviceId;
    use crate::testkit::*;
    use crate::{EditorId, Event, Init, Reading};

    fn tap(s: &mut State, id: DeviceId) -> Vec<Effect> {
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Device(id)))).1
    }

    fn type_and_save(s: &mut State, text: &str) -> Vec<Effect> {
        if let Some(e) = s.bottom.editor_mut() {
            e.text = text.to_owned();
        }
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Save)))).1
    }

    fn available(s: &mut State, password: bool) {
        s.apply(Event::Reading(Reading::Account(crate::device::account::Account { available: true, password, ssh: false })));
    }

    #[test]
    fn the_first_start_opens_the_device_view_and_done_ends_it_for_good() {
        let mut s = State::new(Init { first_start: true, ..init() });
        assert_eq!(s.top_view(), TopView::Device, "the first start opens it");
        let effects = tap(&mut s, DeviceId::Done);
        assert!(effects.contains(&Effect::Save(Save::FirstStartDone)));
        assert_eq!(s.top_view(), TopView::Shelf);
        assert!(!s.first_start());
    }

    #[test]
    fn the_device_entry_opens_it_and_close_goes_back_to_shelf() {
        let mut s = State::new(init());
        let effects = s.open_device();
        assert_eq!(s.top_view(), TopView::Device);
        assert_eq!(effects, vec![Effect::Window(WindowCommand::ShowShelf)], "shelf's window in front");
        assert_eq!(s.pointer.focus, Some(Id::Device(DeviceId::Wifi)));
        let effects = tap(&mut s, DeviceId::Close);
        assert_eq!(s.top_view(), TopView::Shelf);
        assert!(!effects.contains(&Effect::Save(Save::FirstStartDone)), "not the first start: nothing saved");
    }

    #[test]
    fn the_password_is_typed_below_the_current_one_first() {
        let mut s = State::new(init());
        s.open_device();
        assert!(tap(&mut s, DeviceId::Password).is_empty(), "unavailable off the device");
        available(&mut s, true);
        tap(&mut s, DeviceId::Password);
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "typed on the second screen's keyboard");
        assert!(type_and_save(&mut s, "old").is_empty(), "then the new one");
        let effects = type_and_save(&mut s, "newpassword");
        assert!(matches!(effects.first(), Some(Effect::Account(AccountCommand::Password { current: Some(_), .. }))), "{effects:?}");
    }

    #[test]
    fn remote_login_needs_a_password() {
        let mut s = State::new(init());
        available(&mut s, false);
        assert!(tap(&mut s, DeviceId::Ssh).is_empty());
        available(&mut s, true);
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(true))]);
    }

    #[test]
    fn a_sticks_calibration_shows_its_progress_and_keeps_its_result() {
        let mut s = State::new(init());
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks]);
        let p = crate::device::sticks::Calibrator::new(-1024, 1024).progress();
        s.apply(Event::Sticks(SticksEvent::Progress(p)));
        assert!(s.device().sticks().is_some());
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "one at a time");
        let sticks = Sticks { axes: [crate::device::sticks::StickAxis { low: -900, centre: 0, high: 900 }; crate::device::sticks::AXES] };
        let effects = s.apply(Event::Sticks(SticksEvent::Done("pad".into(), Some(sticks)))).1;
        assert!(s.device().calibrated("pad"));
        assert!(effects.iter().any(|e| matches!(e, Effect::Save(Save::Sticks(c)) if c.contains_key("pad"))), "{effects:?}");
        s.apply(Event::Sticks(SticksEvent::Done("pad".into(), None)));
        assert!(s.device().sticks_failed());
    }

    #[test]
    fn the_refresh_calibration_runs_once_at_a_time_and_says_how_it_went() {
        let mut s = State::new(init());
        assert_eq!(tap(&mut s, DeviceId::Refresh), vec![Effect::CalibrateRefresh]);
        assert!(tap(&mut s, DeviceId::Refresh).is_empty());
        s.apply(Event::RefreshCalibrated(Ok("120 Hz: 118.630 -> 120.089 Hz".into())));
        assert!(matches!(s.device().refresh(), Refresh::Done(Ok(_))));
    }
}
