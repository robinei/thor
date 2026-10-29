//! The Device view, where the device is set up: a built-in app ([`clamshell_library::BuiltinApp::Device`]),
//! which the session runs and shows on the main screen. It is opened from Tools or the dock, and runs until
//! Close, B or Quit closes it; Home goes to the Library and leaves it running.
//!
//! It has a row for each thing a device needs set up once, or now and then:
//!
//! - Wi-Fi, which opens its tab;
//! - the time zone, which opens the time tab;
//! - the password and remote login (see [`crate::setup::account`]);
//! - calibrating the controller's sticks ([`sticks`]) and gyro;
//! - the controllers and their players ([`controllers`]);
//! - calibrating the top screen's refresh rate (thor-panel-calibration).

pub mod controllers;
pub mod sticks;

use crate::DeviceId;

use crate::Event;
use crate::bottom::Tab;
use crate::bottom::editor::{Editor, Purpose};
use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::setup::account::{Account, AccountCommand, PasswordStep};
use controllers::{ControllerEvent, Controllers, Saved};
use sticks::{Calibrations, Progress, Sticks};

/// The refresh rate's calibration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Refresh {
    #[default]
    Idle,
    /// Running: the bottom screen is off, so only the main screen's vblanks come, and the main screen is
    /// at the rate the tool asked for, if it has.
    Running(Option<u16>),
    /// The tool's output, with the rates before and after, or the reason it failed.
    Done(Result<String, String>),
}

/// The Device view's own state: its calibrations, and the requests waiting for their answers.
#[derive(Debug, Clone, Default)]
pub struct DeviceView {
    /// When a stick calibration was asked for or last reported progress (monotonic seconds), until it
    /// ends. Another isn't asked for until it has, or until [`ANSWER_WAIT`] has passed with no word from
    /// the pad thread, in case it died.
    sticks_asked: Option<u64>,
    /// How far a running stick calibration has got.
    sticks: Option<Progress>,
    /// The last stick calibration failed. Cleared when another starts.
    sticks_failed: bool,
    refresh: Refresh,
    /// The stick calibrations by controller. They are saved, and the pad thread has them too.
    calibrations: Calibrations,
    /// Every controller seen, with its player.
    controllers: Controllers,
    /// When remote login was turned on or off (monotonic seconds), until the helper answers. Another tap
    /// waits for the answer, or for [`ANSWER_WAIT`] to pass without one.
    ssh_asked: Option<u64>,
}

/// How long a request of the Device view waits for its answer, in seconds, before another tap may ask
/// again: the sticks' calibration, remote login, and the gyro's notice to hold still.
pub(crate) const ANSWER_WAIT: u64 = 30;

/// Whether a request made at `asked` still waits for its answer at `now`.
fn waits(asked: Option<u64>, now: u64) -> bool {
    asked.is_some_and(|at| now < at + ANSWER_WAIT)
}

/// What the pad thread reports about a stick calibration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SticksEvent {
    Progress(Progress),
    /// Done, with the controller's name and its sticks, or `None` if it was given up.
    Done(String, Option<Sticks>),
}

impl DeviceView {
    pub(crate) fn new(calibrations: Calibrations, controllers: Saved) -> DeviceView {
        DeviceView { calibrations, controllers: Controllers::new(controllers), ..DeviceView::default() }
    }

    pub fn controllers(&self) -> &Controllers {
        &self.controllers
    }

    /// What the pad thread says about a controller, or a row stepped (see [`controllers`]).
    pub(crate) fn controller_event(&mut self, event: ControllerEvent, notice: &mut crate::bottom::notice::NoticeSlot, now: u64, out: &mut Out) {
        self.controllers.event(event, notice, now, out);
    }

    /// What is saved of the controllers.
    pub(crate) fn controllers_save(&self) -> Effect {
        self.controllers.save_effect()
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

    /// The account helper answered a change, or can't any more: remote login may be asked again.
    pub(crate) fn account_answered(&mut self) {
        self.ssh_asked = None;
    }

    /// A tap on one of the Device view's targets at `now` (monotonic seconds), with the `account` as read. A tab to open is queued as an
    /// event, since State changes tabs. Close is the session's ([`crate::Session::close_app`]).
    pub(crate) fn tap(&mut self, id: DeviceId, account: &Account, now: u64, out: &mut Out) {
        let (available, password, ssh) = (account.available(), account.password(), account.ssh());
        match id {
            DeviceId::Wifi => out.events.push(Event::ChooseTab(Tab::Wifi)),
            DeviceId::Zone => out.events.push(Event::ChooseTab(Tab::Time)),
            // Typed in the editor on the second screen's keyboard.
            DeviceId::Password if available => {
                let step = if password { PasswordStep::Current } else { PasswordStep::New { current: None } };
                out.events.push(Event::EditOnKeyboard(Editor { purpose: Purpose::AccountPassword(step), text: String::new() }));
            }
            // Remote login needs a password to log in with.
            // One change at a time: the next tap reads the account the answer brings.
            DeviceId::Ssh if available && password && !waits(self.ssh_asked, now) => {
                self.ssh_asked = Some(now);
                out.effects.push(Effect::Account(AccountCommand::Ssh(!ssh)));
            }
            DeviceId::Password | DeviceId::Ssh => {}
            DeviceId::Sticks if !waits(self.sticks_asked, now) => {
                self.sticks_asked = Some(now);
                self.sticks = None;
                self.sticks_failed = false;
                out.effects.push(Effect::CalibrateSticks);
            }
            DeviceId::Sticks => {}
            DeviceId::Gyro => out.events.push(Event::CalibrateGyro),
            DeviceId::Refresh if !self.calibrating() => {
                self.refresh = Refresh::Running(None);
                out.effects.push(Effect::CalibrateRefresh);
            }
            DeviceId::Refresh => {}
            // A controller's row steps its player on.
            DeviceId::Controller(i) => self.controllers.step(usize::from(i), 1, out),
            // The count's row steps how many players there are.
            DeviceId::Players => self.controllers.step_count(1, out),
            DeviceId::Close => out.events.push(Event::Entry(clamshell_library::BuiltinApp::Device.id().into(), crate::EntryAct::Quit)),
            DeviceId::EditLibrary => out.events.push(crate::LibraryViewEvent::Edit.into()),
        }
    }

    /// A report from the pad thread on a stick calibration at `now`. Progress is shown. A result is kept, and the
    /// pad thread already uses it. A failure is shown. The calibrations are saved after one that worked.
    pub(crate) fn sticks_event(&mut self, event: SticksEvent, now: u64, out: &mut Out) {
        match event {
            SticksEvent::Progress(p) => {
                self.sticks = Some(p);
                self.sticks_asked = Some(now);
            }
            SticksEvent::Done(name, result) => {
                self.sticks_asked = None;
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

    /// Whether the refresh rate calibration runs.
    pub fn calibrating(&self) -> bool {
        matches!(self.refresh, Refresh::Running(_))
    }

    /// The main screen's rate the running refresh rate calibration asked for, if it has.
    pub(crate) fn calibration_rate(&self) -> Option<u16> {
        match self.refresh {
            Refresh::Running(hz) => hz,
            _ => None,
        }
    }

    /// The running refresh rate calibration asks for the main screen at `hz`. Without one running, the
    /// screen stays as the settings want it.
    pub(crate) fn calibration_asks(&mut self, hz: u16) {
        if let Refresh::Running(rate) = &mut self.refresh {
            *rate = Some(hz);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Compositor;
    use crate::id::DeviceId;
    use crate::testkit::*;
    use crate::{EditorId, Event, Id, Reading, State};

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
        s.applied(Event::Reading(Reading::Account(crate::setup::account::Account::Read { password, ssh: false })));
    }

    #[test]
    fn the_device_entry_opens_it_and_close_goes_back_to_library_view() {
        let mut s = State::new(init());
        let effects = launch_now(&mut s, "builtin:device");
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Device, view: 0 });
        assert!(effects.is_empty());
        let effects = tap(&mut s, DeviceId::Close);
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Library, view: 0 });
        assert!(!effects.contains(&Effect::Save(Save::FirstStartDone)), "not the first start: nothing saved");
    }

    #[test]
    fn edit_library_brings_the_library_up_in_its_edit_mode() {
        let mut s = State::new(init());
        launch_now(&mut s, "builtin:device");
        tap(&mut s, DeviceId::EditLibrary);
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Library, view: 1 }, "its view 1: the edit mode");
    }

    /// The Device view is an entry that runs while it is open: on the dock, shown again by its tile after
    /// Home went to the Library, gone back to when a window opened over it closes, and closed by Quit.
    #[test]
    fn the_device_view_runs_on_the_dock_until_it_is_closed() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let on_dock = |s: &State| s.dock().items().iter().any(|i| &*i.id == "builtin:device");
        s.applied(Event::Entry("builtin:device".into(), crate::EntryAct::Play));
        assert!(on_dock(&s));
        s.applied(Event::Home);
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Library, view: 0 });
        assert!(on_dock(&s), "still running");
        let tile = s.dock().items().iter().position(|i| &*i.id == "builtin:device").and_then(|i| u16::try_from(i).ok());
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Dock(crate::DockId::Entry(tile.unwrap_or(u16::MAX))))));
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Device, view: 0 });
        // A program launched from it, whose window closes, goes back to it.
        let zelda = launch(&mut s, "zelda-lttp");
        s.observed(vec![crate::testkit::win_of(3, "zelda3", &zelda)]);
        assert_eq!(s.top(), crate::Top::Window { program: true }, "a window in front");
        s.observed(vec![]);
        s.applied(Event::tick(1100));
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Device, view: 0 }, "back where it was launched from");
        s.applied(Event::Entry("builtin:device".into(), crate::EntryAct::Quit));
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Library, view: 0 });
        assert!(!on_dock(&s), "closed");
    }

    /// The Device view is on the main screen: the pad's focus on it isn't using the second screen, which
    /// stays dimmed.
    #[test]
    fn the_focus_on_the_device_view_leaves_the_second_screen_dimmed() {
        let mut s = state();
        s.applied(Event::tick(1000));
        launch_now(&mut s, "builtin:device");
        s.applied(Event::tick(1061));
        assert!(s.bottom_dimmed());
        s.applied(Event::Pointer(crate::PointerEvent::Focus(Id::Device(DeviceId::Zone))));
        assert!(s.bottom_dimmed());
    }

    /// The Wi-Fi and time zone rows open their tabs below, whose readings are then watched.
    #[test]
    fn the_wifi_and_zone_rows_open_their_tabs() {
        let mut s = state();
        launch_now(&mut s, "builtin:device");
        let fx = tap(&mut s, DeviceId::Wifi);
        assert_eq!(s.bottom().tab(), Tab::Wifi);
        assert!(fx.contains(&Effect::Watch(crate::Watch::Wifi, true)), "{fx:?}");
        let fx = tap(&mut s, DeviceId::Zone);
        assert_eq!(s.bottom().tab(), Tab::Time);
        assert!(fx.contains(&Effect::Watch(crate::Watch::Wifi, false)) && fx.contains(&Effect::Watch(crate::Watch::Time, true)), "{fx:?}");
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Device, view: 0 }, "the view stays open");
    }

    /// Home closes the Device view back to the Library. Repeated Home stays on the Library.
    #[test]
    fn home_goes_to_the_library_from_the_device_view() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp");
        s.observed(vec![crate::testkit::win_of(3, "zelda3", &zelda)]);
        launch_now(&mut s, "builtin:device");
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Device, view: 0 });
        s.applied(Event::Home);
        assert!(crate::testkit::on_library(&s), "nothing shown but the Library");
        assert_eq!(s.top(), crate::Top::App { app: crate::BuiltinApp::Library, view: 0 });
        s.applied(Event::Home);
        assert!(crate::testkit::on_library(&s));
    }

    /// The account is read while the view is in front and seen, from when the UI is up (and once at start,
    /// see [`crate::setup::wizard`]), as its definition says.
    #[test]
    fn the_account_is_watched_while_the_view_is_in_front() {
        let mut s = State::new(init());
        assert!(!s.applied(Event::tick(1)).contains(&Effect::Watch(crate::Watch::Account, true)), "the UI isn't up");
        s.applied(Event::Start);
        available(&mut s, true);
        let device = || Event::Entry("builtin:device".into(), crate::EntryAct::Play);
        let fx = s.applied(device());
        assert!(fx.contains(&Effect::Watch(crate::Watch::Account, true)), "{fx:?}");
        // Home leaves it running behind the Library: not read meanwhile.
        assert!(s.applied(Event::Home).contains(&Effect::Watch(crate::Watch::Account, false)));
        assert!(s.applied(device()).contains(&Effect::Watch(crate::Watch::Account, true)), "in front again");
        assert!(tap(&mut s, DeviceId::Close).contains(&Effect::Watch(crate::Watch::Account, false)));
    }

    #[test]
    fn the_password_is_typed_below_the_current_one_first() {
        let mut s = State::new(init());
        s.sent_at_start();
        launch_now(&mut s, "builtin:device");
        assert!(tap(&mut s, DeviceId::Password).is_empty(), "unavailable off the device");
        available(&mut s, true);
        let tab = s.bottom().tab();
        tap(&mut s, DeviceId::Password);
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "typed on the second screen's keyboard");
        assert_eq!(s.bottom().chosen_tab(), tab, "the tab it came up from is the one chosen");
        assert!(type_and_save(&mut s, "old").is_empty(), "then the new one");
        assert!(type_and_save(&mut s, "newpassword").is_empty(), "then the new one again");
        let effects = type_and_save(&mut s, "newpassword");
        assert!(matches!(effects.first(), Some(Effect::Account(AccountCommand::Password { current: Some(_), .. }))), "{effects:?}");
    }

    #[test]
    fn the_password_editor_goes_back_where_it_came_from() {
        let mut s = State::new(init());
        launch_now(&mut s, "builtin:device");
        available(&mut s, true);
        s.applied(Event::ChooseTab(Tab::Dock));
        tap(&mut s, DeviceId::Password);
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Cancel))));
        assert_eq!(s.bottom().tab(), Tab::Dock, "cancelled");
        tap(&mut s, DeviceId::Password);
        type_and_save(&mut s, "old");
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "the new one is asked for");
        type_and_save(&mut s, "newpassword");
        type_and_save(&mut s, "newpassword");
        assert_eq!(s.bottom().tab(), Tab::Dock, "set");
        assert!(s.bottom().editor().is_none());
    }

    #[test]
    fn the_gyros_hold_still_shows_until_it_answers_at_most_30_s() {
        use crate::bottom::notice::NoticeKind;
        let mut s = State::new(init());
        s.applied(Event::tick(1000));
        assert_eq!(tap(&mut s, DeviceId::Gyro), vec![Effect::CalibrateGyro]);
        s.applied(Event::tick(1010));
        assert_eq!(notice(&s), Some(NoticeKind::Gyro(None)), "still calibrating");
        s.applied(Event::GyroCalibrated(true));
        assert_eq!(notice(&s), Some(NoticeKind::Gyro(Some(true))));
        s.applied(Event::tick(1012));
        assert_eq!(notice(&s), None, "the answer shows as any notice");
        tap(&mut s, DeviceId::Gyro);
        s.applied(Event::tick(1041));
        assert_eq!(notice(&s), Some(NoticeKind::Gyro(None)));
        s.applied(Event::tick(1042));
        assert_eq!(notice(&s), None, "no answer: gone after 30 s");
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
    fn remote_login_waits_for_its_answer_before_another_tap() {
        let mut s = State::new(init());
        available(&mut s, true);
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(true))]);
        assert!(tap(&mut s, DeviceId::Ssh).is_empty(), "not sent twice");
        s.applied(Event::AccountChanged(Ok("Remote login on".into())));
        s.applied(Event::Reading(Reading::Account(crate::setup::account::Account::Read { password: true, ssh: true })));
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(false))], "answered: off again");
        s.applied(Event::AccountChanged(Err("failed".into())));
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(false))], "a failure answers too");
    }

    /// The account helper's answer shows for a moment: what it did, or why it failed.
    #[test]
    fn an_account_change_says_how_it_went() {
        use crate::bottom::notice::NoticeKind;
        let mut s = State::new(init());
        s.applied(Event::tick(100));
        s.applied(Event::AccountChanged(Ok("Remote login on".into())));
        assert_eq!(notice(&s), Some(NoticeKind::Account(Ok("Remote login on".into()))));
        s.applied(Event::AccountChanged(Err("wrong password".into())));
        assert_eq!(notice(&s), Some(NoticeKind::Account(Err("wrong password".into()))));
        s.applied(Event::tick(102));
        assert_eq!(notice(&s), None);
    }

    #[test]
    fn the_sticks_calibration_waits_for_its_end_before_another_tap() {
        let mut s = State::new(init());
        s.sent_at_start();
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks]);
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "before the first progress too");
        s.applied(Event::Sticks(SticksEvent::Done("pad".into(), None)));
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks], "ended: again");
    }

    /// A pad thread that died mid-calibration doesn't block the sticks for good: with no word from it for
    /// [`ANSWER_WAIT`], a tap asks again. Progress counts as word, so a slow calibration isn't asked twice.
    #[test]
    fn the_sticks_calibration_is_asked_again_after_30_s_without_word() {
        let mut s = State::new(init());
        s.applied(Event::tick(1000));
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks]);
        s.applied(Event::tick(1020));
        let rest = crate::device_view::sticks::StickAxis { low: 0, centre: 0, high: 0 };
        let p = Progress { phase: crate::device_view::sticks::Phase::Turn, now: [0; 4], seen: [rest; 4], min: -1024, max: 1024 };
        s.applied(Event::Sticks(SticksEvent::Progress(p)));
        s.applied(Event::tick(1049));
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "29 s since its progress: still waiting");
        s.applied(Event::tick(1050));
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks], "30 s without word: asked again");
        assert!(s.device_view().sticks().is_none(), "the old progress is dropped");
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "and that one waits in turn");
    }

    /// A helper that never answers doesn't block remote login for good: after [`ANSWER_WAIT`] a tap asks
    /// again.
    #[test]
    fn remote_login_is_asked_again_after_30_s_without_an_answer() {
        let mut s = State::new(init());
        s.applied(Event::tick(1000));
        available(&mut s, true);
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(true))]);
        s.applied(Event::tick(1029));
        assert!(tap(&mut s, DeviceId::Ssh).is_empty(), "29 s: still waiting");
        s.applied(Event::tick(1030));
        assert_eq!(tap(&mut s, DeviceId::Ssh), vec![Effect::Account(AccountCommand::Ssh(true))], "30 s: asked again");
        assert!(tap(&mut s, DeviceId::Ssh).is_empty(), "and that one waits in turn");
    }

    #[test]
    fn a_sticks_calibration_shows_its_progress_and_keeps_its_result() {
        let mut s = State::new(init());
        s.sent_at_start();
        assert_eq!(tap(&mut s, DeviceId::Sticks), vec![Effect::CalibrateSticks]);
        let rest = crate::device_view::sticks::StickAxis { low: 0, centre: 0, high: 0 };
        let p = Progress { phase: crate::device_view::sticks::Phase::LetGo, now: [0; 4], seen: [rest; 4], min: -1024, max: 1024 };
        s.applied(Event::Sticks(SticksEvent::Progress(p)));
        assert!(s.device_view().sticks().is_some());
        assert!(tap(&mut s, DeviceId::Sticks).is_empty(), "one at a time");
        let sticks = Sticks { axes: [crate::device_view::sticks::StickAxis { low: -900, centre: 0, high: 900 }; crate::device_view::sticks::AXES] };
        let effects = s.applied(Event::Sticks(SticksEvent::Done("pad".into(), Some(sticks))));
        assert!(s.device_view().calibrated("pad"));
        assert!(effects.iter().any(|e| matches!(e, Effect::Save(Save::Sticks(c)) if c.contains_key("pad"))), "{effects:?}");
        s.applied(Event::Sticks(SticksEvent::Done("pad".into(), None)));
        assert!(s.device_view().sticks_failed());
    }

    #[test]
    fn the_refresh_calibration_runs_once_at_a_time_and_says_how_it_went() {
        let mut s = State::new(init());
        s.sent_at_start();
        assert_eq!(tap(&mut s, DeviceId::Refresh), vec![Effect::CalibrateRefresh, Effect::Compositor(Compositor::Outputs { top: true, bottom: false })]);
        assert!(tap(&mut s, DeviceId::Refresh).is_empty());
        s.applied(Event::RefreshCalibrated(Ok("120 Hz: 118.630 -> 120.089 Hz".into())));
        assert!(matches!(s.device_view().refresh(), Refresh::Done(Ok(_))));
    }
}
