//! The Device view. It shows on the main screen in shelf's place ([`TopView`]). It is opened by the
//! Device entry in Tools, from shelf or pinned to the dock, and opens by itself at ClamShell's first
//! start. B, Home or Close go back to shelf.
//!
//! It has a row for each thing a device needs set up once, or now and then:
//!
//! - Wi-Fi, which opens its tab;
//! - the time zone, which opens the time tab;
//! - the password and remote login (see [`crate::device::account`]);
//! - calibrating the controller's sticks ([`crate::device::sticks`]) and gyro;
//! - calibrating the top screen's refresh rate (thor-panel-calibration);
//! - Done, at the first start.

pub mod account;
pub mod buttons;
pub mod idle;
pub mod sticks;

use crate::DeviceId;
use std::sync::Arc;

use crate::bottom::Tab;
use crate::bottom::editor::{Editor, Purpose};
use crate::effect::{Effect, Save, WindowCommand};
use crate::out::Out;
use crate::{Event, Id, Pointer};
use account::{Account, AccountCommand, PasswordStep};
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
    /// The tool's output, with the rates before and after, or the reason it failed.
    Done(Result<String, String>),
}

/// The Device view's own state.
#[derive(Debug, Clone, Default)]
pub struct Device {
    open: bool,
    /// How far a running stick calibration has got.
    sticks: Option<Progress>,
    /// The last stick calibration failed. Cleared when another starts.
    sticks_failed: bool,
    refresh: Refresh,
    /// The stick calibrations by controller. They are saved, and the pad thread has them too.
    calibrations: Calibrations,
    /// ClamShell's first start (see [`crate::Init::first_start`]).
    first_start: bool,
    /// The account, as last read (see [`account`]).
    account: Account,
}

/// What the pad thread reports about a stick calibration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SticksEvent {
    Progress(Progress),
    /// Done, with the controller's name and its sticks, or `None` if it was given up.
    Done(String, Option<Sticks>),
}

impl Device {
    /// The view as ClamShell starts. It is open at the first start.
    pub(crate) fn new(calibrations: Calibrations, first_start: bool) -> Device {
        Device { open: first_start, calibrations, first_start, ..Device::default() }
    }

    pub fn top_view(&self) -> TopView {
        if self.open { TopView::Device } else { TopView::Shelf }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

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

    /// Whether this is ClamShell's first start, which opens the Device view.
    pub fn first_start(&self) -> bool {
        self.first_start
    }

    pub fn account(&self) -> Account {
        self.account
    }

    /// Opens the view on the main screen, bringing shelf's window to the front, with the pad's focus on
    /// its first row. The account is read while it is open ([`crate::sync::Sent::watches`]).
    pub(crate) fn open(&mut self, pointer: &mut Pointer, out: &mut Out) {
        self.open = true;
        pointer.focus_on(Id::Device(DeviceId::Wifi));
        out.effects.push(Effect::Window(WindowCommand::ShowShelf));
    }

    /// Goes back to shelf, with the focus on its `selected` entry.
    pub(crate) fn close(&mut self, pointer: &mut Pointer, selected: Option<&Arc<str>>) {
        self.open = false;
        pointer.focus_shelf(selected);
    }

    /// A reading of the account.
    pub(crate) fn read_account(&mut self, account: Account) {
        self.account = account;
    }

    /// A tap on one of the Device view's targets. Closing it gives the focus back to shelf's
    /// selection (`selected`). A tab to open is queued as an event, since State changes tabs.
    pub(crate) fn tap(&mut self, id: DeviceId, pointer: &mut Pointer, selected: Option<&Arc<str>>, out: &mut Out) {
        let account = self.account;
        match id {
            DeviceId::Wifi => out.events.push(Event::ChooseTab(Tab::Wifi)),
            DeviceId::Zone => out.events.push(Event::ChooseTab(Tab::Time)),
            // Typed in the editor on the second screen's keyboard.
            DeviceId::Password if account.available => {
                let step = if account.password { PasswordStep::Current } else { PasswordStep::New { current: None } };
                out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::AccountPassword(step), text: String::new() }));
            }
            // Remote login needs a password to log in with.
            DeviceId::Ssh if account.available && account.password => out.effects.push(Effect::Account(AccountCommand::Ssh(!account.ssh))),
            DeviceId::Password | DeviceId::Ssh => {}
            DeviceId::Sticks if self.sticks.is_none() => {
                self.sticks_failed = false;
                out.effects.push(Effect::CalibrateSticks);
            }
            DeviceId::Sticks => {}
            DeviceId::Gyro => out.events.push(Event::CalibrateGyro),
            DeviceId::Refresh if self.refresh != Refresh::Running => {
                self.refresh = Refresh::Running;
                out.effects.push(Effect::CalibrateRefresh);
            }
            DeviceId::Refresh => {}
            // The first time, Done ends the first start for good. Shelf shows next, with the default tab
            // below.
            DeviceId::Done if self.first_start => {
                self.first_start = false;
                self.close(pointer, selected);
                out.effects.push(Effect::Save(Save::FirstStartDone));
                out.events.push(Event::ChooseTab(Tab::default()));
            }
            DeviceId::Done | DeviceId::Close => self.close(pointer, selected),
        }
    }

    /// A report from the pad thread on a stick calibration. Progress is shown. A result is kept, and the
    /// pad thread already uses it. A failure is shown. The calibrations are saved after one that worked.
    pub(crate) fn sticks_event(&mut self, event: SticksEvent, out: &mut Out) {
        match event {
            SticksEvent::Progress(p) => self.sticks = Some(p),
            SticksEvent::Done(name, result) => {
                self.sticks = None;
                self.sticks_failed = result.is_none();
                if let Some(sticks) = result {
                    self.calibrations.insert(name, sticks);
                    out.effects.push(Effect::Save(Save::Sticks(self.calibrations.clone())));
                }
            }
        }
    }

    /// The refresh rate calibration ended, with its output.
    pub(crate) fn refresh_done(&mut self, result: Result<String, String>) {
        self.refresh = Refresh::Done(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::DeviceId;
    use crate::testkit::*;
    use crate::{EditorId, Event, Init, Reading, State};

    fn tap(s: &mut State, id: DeviceId) -> Vec<Effect> {
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Device(id))))
    }

    fn type_and_save(s: &mut State, text: &str) -> Vec<Effect> {
        if let Some(e) = s.editor_mut() {
            e.text = text.to_owned();
        }
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Save))))
    }

    fn available(s: &mut State, password: bool) {
        s.applied(Event::Reading(Reading::Account(crate::device::account::Account { available: true, password, ssh: false })));
    }

    #[test]
    fn the_first_start_opens_the_device_view_and_done_ends_it_for_good() {
        let mut s = State::new(Init { first_start: true, ..init() });
        assert_eq!(s.device().top_view(), TopView::Device, "the first start opens it");
        let effects = tap(&mut s, DeviceId::Done);
        assert!(effects.contains(&Effect::Save(Save::FirstStartDone)));
        assert_eq!(s.device().top_view(), TopView::Shelf);
        assert!(!s.device().first_start());
    }

    #[test]
    fn the_device_entry_opens_it_and_close_goes_back_to_shelf() {
        let mut s = State::new(init());
        let effects = launch_now(&mut s, "tool:device");
        assert_eq!(s.device().top_view(), TopView::Device);
        assert_eq!(effects, vec![Effect::Window(WindowCommand::ShowShelf)], "shelf's window in front");
        assert_eq!(s.pointer().focus(), Some(&Id::Device(DeviceId::Wifi)));
        let effects = tap(&mut s, DeviceId::Close);
        assert_eq!(s.device().top_view(), TopView::Shelf);
        assert!(!effects.contains(&Effect::Save(Save::FirstStartDone)), "not the first start: nothing saved");
    }

    #[test]
    fn the_password_is_typed_below_the_current_one_first() {
        let mut s = State::new(init());
        launch_now(&mut s, "tool:device");
        assert!(tap(&mut s, DeviceId::Password).is_empty(), "unavailable off the device");
        available(&mut s, true);
        let tab = s.bottom().tab();
        tap(&mut s, DeviceId::Password);
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "typed on the second screen's keyboard");
        assert_eq!(s.bottom().chosen_tab(), tab, "the tab it came up from is the one chosen");
        assert!(type_and_save(&mut s, "old").is_empty(), "then the new one");
        let effects = type_and_save(&mut s, "newpassword");
        assert!(matches!(effects.first(), Some(Effect::Account(AccountCommand::Password { current: Some(_), .. }))), "{effects:?}");
    }

    #[test]
    fn the_password_editor_goes_back_where_it_came_from() {
        let mut s = State::new(init());
        launch_now(&mut s, "tool:device");
        available(&mut s, true);
        s.applied(Event::ChooseTab(Tab::Dock));
        tap(&mut s, DeviceId::Password);
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Cancel))));
        assert_eq!(s.bottom().tab(), Tab::Dock, "cancelled");
        tap(&mut s, DeviceId::Password);
        type_and_save(&mut s, "old");
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "the new one is asked for");
        type_and_save(&mut s, "newpassword");
        assert_eq!(s.bottom().tab(), Tab::Dock, "set");
        assert!(s.bottom().editor().is_none());
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
        let rest = crate::device::sticks::StickAxis { low: 0, centre: 0, high: 0 };
        let p = Progress { phase: crate::device::sticks::Phase::LetGo, now: [0; 4], seen: [rest; 4], min: -1024, max: 1024 };
        s.applied(Event::Sticks(SticksEvent::Progress(p)));
        assert!(s.device().sticks().is_some());
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "one at a time");
        let sticks = Sticks { axes: [crate::device::sticks::StickAxis { low: -900, centre: 0, high: 900 }; crate::device::sticks::AXES] };
        let effects = s.applied(Event::Sticks(SticksEvent::Done("pad".into(), Some(sticks))));
        assert!(s.device().calibrated("pad"));
        assert!(effects.iter().any(|e| matches!(e, Effect::Save(Save::Sticks(c)) if c.contains_key("pad"))), "{effects:?}");
        s.applied(Event::Sticks(SticksEvent::Done("pad".into(), None)));
        assert!(s.device().sticks_failed());
    }

    #[test]
    fn the_refresh_calibration_runs_once_at_a_time_and_says_how_it_went() {
        let mut s = State::new(init());
        assert_eq!(tap(&mut s, DeviceId::Refresh), vec![Effect::CalibrateRefresh]);
        assert!(tap(&mut s, DeviceId::Refresh).is_empty());
        s.applied(Event::RefreshCalibrated(Ok("120 Hz: 118.630 -> 120.089 Hz".into())));
        assert!(matches!(s.device().refresh(), Refresh::Done(Ok(_))));
    }
}
