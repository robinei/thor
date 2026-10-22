//! Idle. With no input for a while, the screens dim, then go off, then the device sleeps. Any input
//! brings them back. The times are global settings ([`Setting::ScreenOff`], [`Setting::Sleep`]).
//!
//! Input is read from the input devices themselves: a system thread reads every one, without taking it
//! away from anyone. Input that reaches ClamShell another way, such as the pad driving the screens, also
//! counts. Where the compositor supports the standard Wayland idle protocol, its reports are used as
//! well, so an app that asks it to keep the screen on, such as a video player, holds idle off. If neither
//! the devices nor the compositor can be heard, nothing goes idle.
//!
//! The device only sleeps where ClamShell may put it to sleep, which is in the device's own session and
//! never on a desktop that runs ClamShell for testing. A program in front doesn't hold sleep off. One
//! that should, such as a video player, asks the compositor to keep the screen on.

use std::sync::Arc;

use crate::State;
use crate::effect::Effect;
use crate::out::Out;
use crate::settings::{Setting, Values};

/// The compositor reports no input after this many seconds without any.
pub const NOTIFY_AFTER: u64 = 10;
/// The screens dim this long before they go off.
const DIM_BEFORE: u64 = 30;
/// How long before the screens dim to ask a runner whose app is shown whether it keeps the screen on.
const ASK_BEFORE: u64 = 10;
/// Presses that end within this long after the screens came back are taken as the wake-up, not as
/// presses. A tap on a dark screen should only light it.
const WAKE_GRACE: u64 = 1;

/// How far idle has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Stage {
    #[default]
    Awake,
    Dimmed,
    Off,
    Asleep,
}

/// What the input sources say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// Whether the input devices are being read. False when they can't be, for lack of permission.
    Watching(bool),
    /// Input on a device. Sent at most every few seconds while input goes on.
    DeviceInput,
    /// The compositor has seen no input for [`NOTIFY_AFTER`] seconds.
    CompositorQuiet,
    /// The compositor sees input again, or an app asks it to keep the screen on.
    CompositorInput,
    /// The app shown keeps the screen on in a way the compositor doesn't hear, as Android's apps do. Its
    /// runner answered this when asked, and it counts as input.
    KeptAwake,
    /// Sleep was due, but the machine was busy, for example downloading or building. It didn't sleep, and
    /// will try again after [`SLEEP_RETRY`].
    Busy,
}

/// How long sleep waits after the machine was found busy.
const SLEEP_RETRY: u64 = 300;

/// What the compositor reports, where it reports anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Compositor {
    /// It doesn't report, because it lacks the idle protocol, or hasn't reported yet.
    #[default]
    Silent,
    Busy,
    QuietSince(u64),
}

#[derive(Debug, Default)]
pub struct Idle {
    compositor: Compositor,
    /// The input devices are being read.
    watching: bool,
    /// The latest input on a device or to ClamShell itself.
    last_input: u64,
    stage: Stage,
    /// When the screens last came back from off.
    woke_at: Option<u64>,
    /// The start of the idle time a runner was last asked about. A runner is asked once per idle time.
    asked_for: Option<u64>,
    /// No sleep before this time, because the machine was busy when sleep was due.
    sleep_not_before: u64,
}

/// What the idle times are, and whether sleeping is allowed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    screen_off: Option<u64>,
    sleep: Option<u64>,
    may_sleep: bool,
}

impl Policy {
    /// Reads the policy from the global settings, which are in minutes, with 0 meaning never.
    pub(crate) fn of(global: &Values, may_sleep: bool) -> Policy {
        let secs = |s: Setting| Some(u64::from(global.get(s)) * 60).filter(|&t| t > 0);
        Policy { screen_off: secs(Setting::ScreenOff), sleep: secs(Setting::Sleep), may_sleep }
    }
}

impl Idle {
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// What a source reported, at `now`.
    pub(crate) fn heard(&mut self, event: IdleEvent, now: u64) {
        match event {
            IdleEvent::Watching(on) => {
                self.watching = on;
                self.last_input = now;
            }
            IdleEvent::DeviceInput | IdleEvent::KeptAwake => self.last_input = now,
            IdleEvent::CompositorQuiet => self.compositor = Compositor::QuietSince(now.saturating_sub(NOTIFY_AFTER)),
            IdleEvent::CompositorInput => self.compositor = Compositor::Busy,
            IdleEvent::Busy => self.sleep_not_before = now + SLEEP_RETRY,
        }
    }

    /// Input that reached ClamShell itself at `now`, such as a tap or the pad on its screens.
    pub(crate) fn own_input(&mut self, now: u64) {
        self.last_input = now;
    }

    /// Back from sleep, or the clock was first read, at `now`. Treated as if there had just been input.
    pub(crate) fn resumed(&mut self, now: u64) {
        self.last_input = now;
        if let Compositor::QuietSince(_) = self.compositor {
            self.compositor = Compositor::QuietSince(now);
        }
    }

    /// Since when there has been no input. None while there is input, meaning the compositor is busy, or
    /// if no source can tell.
    fn since(&self) -> Option<u64> {
        match self.compositor {
            Compositor::Busy => None,
            Compositor::QuietSince(t) => Some(t.max(self.last_input)),
            Compositor::Silent if self.watching => Some(self.last_input),
            Compositor::Silent => None,
        }
    }

    /// How long there has been no input at `now`, or none if no source can tell.
    fn idle_for(&self, now: u64) -> Option<u64> {
        if self.compositor == Compositor::Busy {
            return Some(0);
        }
        Some(now.saturating_sub(self.since()?))
    }

    /// Whether to ask the shown app's runner if it keeps the screen on. It is asked [`ASK_BEFORE`] the
    /// screens dim, once per idle time.
    pub(crate) fn ask_awake(&mut self, now: u64, policy: Policy) -> bool {
        let (Some(since), Some(off)) = (self.since(), policy.screen_off) else {
            return false;
        };
        let due = self.stage == Stage::Awake && now.saturating_sub(since) + DIM_BEFORE + ASK_BEFORE >= off && self.asked_for != Some(since);
        if due {
            self.asked_for = Some(since);
        }
        due
    }

    /// The stage `policy` wants at `now`.
    pub(crate) fn wanted(&self, now: u64, policy: Policy) -> Stage {
        let Some(t) = self.idle_for(now) else {
            return Stage::Awake;
        };
        if policy.may_sleep && policy.sleep.is_some_and(|sleep| t >= sleep) && now >= self.sleep_not_before {
            return Stage::Asleep;
        }
        match policy.screen_off {
            Some(off) if t >= off => Stage::Off,
            Some(off) if t + DIM_BEFORE >= off => Stage::Dimmed,
            _ => Stage::Awake,
        }
    }

    /// Moves to `stage`. Returns the stage it was at.
    pub(crate) fn set(&mut self, stage: Stage, now: u64) -> Stage {
        let was = std::mem::replace(&mut self.stage, stage);
        if was >= Stage::Off && stage < Stage::Off {
            self.woke_at = Some(now);
        }
        was
    }

    /// Whether a press ending at `now` is the wake-up rather than a press. It is when the screens are
    /// off or just came back.
    pub(crate) fn swallows_press(&self, now: u64) -> bool {
        self.stage >= Stage::Off || self.woke_at.is_some_and(|t| now <= t + WAKE_GRACE)
    }
}

impl Idle {
    /// Updates idle for how things are now, at `now`. The stage moves on to dimmed, screens off or
    /// sleep, or back on any input; the screens get what they need for it after the event (see
    /// [`crate::sync::Sent::idle`]). `refreshing` is a refresh rate calibration running, and `awake` the
    /// shown app's runner's command that says whether it keeps the screen on (see
    /// [`crate::Session::awake_command`]).
    pub(crate) fn follow(&mut self, now: u64, policy: Policy, refreshing: bool, awake: Option<Arc<[String]>>, out: &mut Out) {
        // While the top screen's refresh rate is being calibrated, it is measured on the screen, so the
        // screen is kept on.
        if refreshing {
            self.own_input(now);
        }
        // An app shown that keeps the screen on without the compositor hearing it, as Android's do.
        if let Some(awake) = awake
            && self.ask_awake(now, policy)
        {
            out.effects.push(Effect::AskAwake(awake.to_vec()));
        }
        let wanted = self.wanted(now, policy);
        if wanted != self.stage {
            self.set(wanted, now);
        }
    }
}

impl State {
    /// Idle follows how things are now (see [`Idle::follow`]).
    pub(crate) fn follow_idle(&mut self, out: &mut Out) {
        // Sleep only in the device's own session. A program in front doesn't hold it off; one playing a
        // video asks the compositor to keep the screen on.
        let policy = Policy::of(&self.prefs.settings().global, self.session.device_session());
        let refreshing = *self.device.refresh() == crate::device::Refresh::Running;
        self.idle.follow(self.clock.mono(), policy, refreshing, self.session.awake_command(), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(off: u64, sleep: u64, may_sleep: bool) -> Policy {
        Policy { screen_off: (off > 0).then_some(off * 60), sleep: (sleep > 0).then_some(sleep * 60), may_sleep }
    }

    #[test]
    fn from_the_devices_alone_dims_then_goes_off_then_sleeps() {
        let mut idle = Idle::default();
        let p = policy(5, 15, true);
        idle.heard(IdleEvent::Watching(true), 90);
        assert_eq!(idle.wanted(100, p), Stage::Awake);
        assert_eq!(idle.wanted(90 + 270, p), Stage::Dimmed, "30 s before off");
        assert_eq!(idle.wanted(90 + 300, p), Stage::Off);
        assert_eq!(idle.wanted(90 + 900, p), Stage::Asleep);
        assert_eq!(idle.wanted(90 + 900, policy(5, 15, false)), Stage::Off, "not allowed to sleep");
        assert_eq!(idle.wanted(90 + 900, policy(0, 0, true)), Stage::Awake, "never");
        idle.heard(IdleEvent::DeviceInput, 1000);
        assert_eq!(idle.wanted(1010, p), Stage::Awake, "input");
    }

    #[test]
    fn nothing_goes_idle_unheard() {
        let idle = Idle::default();
        assert_eq!(idle.wanted(100_000, policy(5, 15, true)), Stage::Awake, "no source can tell");
    }

    #[test]
    fn the_compositor_holds_idle_off_while_it_sees_input_or_an_app_asks() {
        let mut idle = Idle::default();
        let p = policy(5, 0, true);
        idle.heard(IdleEvent::Watching(true), 0);
        idle.heard(IdleEvent::CompositorInput, 100);
        assert_eq!(idle.wanted(5000, p), Stage::Awake, "a video plays: the compositor says busy");
        idle.heard(IdleEvent::CompositorQuiet, 5000);
        assert_eq!(idle.wanted(5010, p), Stage::Awake);
        assert_eq!(idle.wanted(4990 + 300, p), Stage::Off);
        // The devices still count, such as the controller, since a joystick isn't the compositor's input.
        idle.heard(IdleEvent::DeviceInput, 5200);
        assert_eq!(idle.wanted(5300, p), Stage::Awake);
    }

    #[test]
    fn a_press_on_dark_screens_only_wakes_them() {
        let mut idle = Idle::default();
        idle.set(Stage::Off, 100);
        assert!(idle.swallows_press(100));
        idle.set(Stage::Awake, 200);
        assert!(idle.swallows_press(201), "just woke");
        assert!(!idle.swallows_press(203));
    }

    #[test]
    fn the_state_dims_turns_the_screens_off_and_back_and_sleeps_only_when_allowed() {
        use crate::status::Status;
        use crate::testkit::state;
        use crate::{Event, Reading};
        let brightness = |e: &[Effect]| {
            e.iter().find_map(|e| match e {
                Effect::Brightness { level, .. } => Some(*level),
                _ => None,
            })
        };
        let mut s = state();
        s.apply(Reading::Status(Status { brightness: Some(60), ..Status::default() }).into());
        s.apply(crate::LevelEvent::Set(crate::Level::Brightness, 60).into());
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        // Defaults: screens off after 5 minutes, sleep after 15.
        assert_eq!(brightness(&s.apply(Event::tick(1270)).1), Some(20), "dimmed to a third");
        assert_eq!(
            s.apply(Event::tick(1300)).1,
            vec![Effect::Screens(false), Effect::Watch(crate::Watch::Status, false), Effect::Watch(crate::Watch::Metrics, false)]
        );
        assert!(!s.apply(Event::tick(1900)).1.contains(&Effect::Sleep), "not the device's session");
        // On input the screens come back at their brightness, and the press that woke them does nothing.
        let fx = s.apply(Event::Idle(IdleEvent::DeviceInput));
        assert!(fx.1.contains(&Effect::Screens(true)) && brightness(&fx.1) == Some(60), "{fx:?}");
        assert!(fx.1.contains(&Effect::Watch(crate::Watch::Metrics, true)), "watched again: {fx:?}");
        assert!(fx.1.contains(&Effect::Watch(crate::Watch::Status, true)), "the bar's status too: {fx:?}");
        // In the device's session it sleeps.
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        assert!(s.apply(Event::tick(1900)).1.contains(&Effect::Sleep));
    }

    #[test]
    fn an_app_shown_that_keeps_the_screen_on_its_own_way_is_asked_before_dimming() {
        use crate::Event;
        use crate::session::instance::Instance;
        use crate::session::windows::Window;
        let runner = clamshell_library::RunnerOf {
            handover: true,
            awake: vec!["awake".to_owned()].into(),
            ..clamshell_library::RunnerOf::new("android", "Android", true)
        };
        let mut library = clamshell_library::demo::library();
        if let Some(g) = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox") {
            g.runner = Some(runner);
        }
        let mut s = State::new(crate::Init { library, ..crate::testkit::init() });
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        crate::testkit::launch_now(&mut s, "desktop:firefox");
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        let ws = Instance::workspace_of(&clamshell_library::key_of("desktop:firefox"));
        s.apply(Event::Windows(vec![Window { visible: true, ..crate::testkit::win(6, "firefox", &ws, true) }]));
        let ask = Effect::AskAwake(vec!["awake".into()]);
        // Asked once, 10 s before the screens dim. They go off at 5 minutes and dim 30 s before that.
        assert!(!s.apply(Event::tick(1259)).1.contains(&ask));
        assert!(s.apply(Event::tick(1260)).1.contains(&ask));
        assert!(!s.apply(Event::tick(1261)).1.contains(&ask), "once");
        // It keeps the screen on, which counts as input, so it doesn't dim. It is asked again as the next
        // dimming nears.
        s.apply(Event::Idle(IdleEvent::KeptAwake));
        s.apply(Event::tick(1280));
        assert_eq!(s.idle().stage(), Stage::Awake);
        assert!(s.apply(Event::tick(1261 + 260)).1.contains(&ask));
        // With no answer, it dims as usual. An app that isn't shown isn't asked.
        s.apply(Event::tick(1261 + 270));
        assert_eq!(s.idle().stage(), Stage::Dimmed);
        let mut s = crate::testkit::state();
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        assert!(!s.apply(Event::tick(1260)).1.contains(&ask));
    }

    #[test]
    fn a_busy_machine_sleeps_later() {
        use crate::Event;
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        assert!(s.apply(Event::tick(1900)).1.contains(&Effect::Sleep));
        // Busy, such as with a download: it doesn't sleep, the screens stay off, and sleep is tried again
        // after the retry time.
        let fx = s.apply(Event::Idle(IdleEvent::Busy)).1;
        assert!(!fx.contains(&Effect::Screens(true)), "{fx:?}");
        assert_eq!(s.idle().stage(), Stage::Off);
        assert!(!s.apply(Event::tick(1900 + SLEEP_RETRY - 1)).1.contains(&Effect::Sleep));
        assert!(s.apply(Event::tick(1900 + SLEEP_RETRY)).1.contains(&Effect::Sleep));
    }

    #[test]
    fn what_is_heard_before_the_clock_is_known_counts_from_its_first_tick() {
        use crate::Event;
        let mut s = crate::testkit::state();
        // At start the compositor reports quiet before the clock thread's first tick.
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        s.apply(Event::Idle(IdleEvent::CompositorQuiet));
        let fx = s.apply(Event::tick(1_792_060_000)).1;
        assert!(!fx.contains(&Effect::Screens(false)), "not off at once: {fx:?}");
        assert_eq!(s.idle().stage(), Stage::Awake);
    }

    #[test]
    fn a_step_of_the_wall_clock_is_no_time_gone_by() {
        use crate::Event;
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.apply(Event::tick(1000));
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        // The clock is set a day forward, by hand or by NTP, and it is neither asleep nor off.
        let fx = s.apply(Event::Tick { unix_secs: 1000 + 86_400, mono_secs: 1001 }).1;
        assert!(!fx.contains(&Effect::Sleep) && !fx.contains(&Effect::Screens(false)), "{fx:?}");
        assert_eq!(s.idle().stage(), Stage::Awake);
        // Set back again, idle still counts on.
        assert!(s.apply(Event::Tick { unix_secs: 500, mono_secs: 1900 }).1.contains(&Effect::Sleep));
    }

    #[test]
    fn back_from_sleep_counts_from_the_wake() {
        let mut idle = Idle::default();
        let p = policy(5, 15, true);
        idle.heard(IdleEvent::Watching(true), 100);
        idle.resumed(5000);
        assert_eq!(idle.wanted(5010, p), Stage::Awake);
        assert_eq!(idle.wanted(5300, p), Stage::Off);
    }
}
