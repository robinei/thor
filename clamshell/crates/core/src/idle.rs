//! Idle: with no input for a while, the screens dim, then go off, then the device sleeps; any input
//! brings them back. Times are global settings ([`Setting::ScreenOff`], [`Setting::Sleep`]).
//!
//! Input is read from the input devices themselves (a policy thread reads every one, without taking it
//! away from anyone), plus what reaches ClamShell another way (the bar under a finger in touchpad mode,
//! the pad driving the screens). The compositor's word is an extra where it gives it (the standard
//! Wayland idle protocol): an app that asks it to keep the screen on, a video player, holds idle off. If
//! neither the devices nor the compositor can be heard, nothing goes idle.
//!
//! Sleep is only where the program may (the device's own session: never a desktop that runs ClamShell
//! for testing), and never while a game is in front: the screens go off, the game keeps running.

use crate::State;
use crate::effect::Effect;
use crate::game::Front;
use crate::settings::{Setting, Values};

/// The compositor reports no input after this many seconds without any.
pub const NOTIFY_AFTER: u64 = 10;
/// The screens dim this long before they go off.
const DIM_BEFORE: u64 = 30;
/// Presses that end within this long after the screens came back are taken as the wake-up, not as
/// presses (a tap on a dark screen shouldn't do anything but light it).
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
    /// The input devices are being read (or can't be: no permission).
    Watching(bool),
    /// Input on a device (sent at most every few seconds while there is).
    DeviceInput,
    /// The compositor has seen no input for [`NOTIFY_AFTER`] seconds.
    CompositorQuiet,
    /// The compositor sees input again (or an app asks it to keep the screen on).
    CompositorInput,
}

/// The compositor's word, where it gives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Compositor {
    /// It doesn't report (no idle protocol), or hasn't yet.
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
}

/// What the idle times are, and whether sleeping is allowed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    screen_off: Option<u64>,
    sleep: Option<u64>,
    may_sleep: bool,
}

impl Policy {
    /// From the global settings (minutes; 0: never).
    pub(crate) fn of(global: &Values, may_sleep: bool) -> Policy {
        let secs = |s: Setting| Some(u64::from(global.get(s)) * 60).filter(|&t| t > 0);
        Policy { screen_off: secs(Setting::ScreenOff), sleep: secs(Setting::Sleep), may_sleep }
    }
}

impl Idle {
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// What a source said, at `now`.
    pub(crate) fn heard(&mut self, event: IdleEvent, now: u64) {
        match event {
            IdleEvent::Watching(on) => {
                self.watching = on;
                self.last_input = now;
            }
            IdleEvent::DeviceInput => self.last_input = now,
            IdleEvent::CompositorQuiet => self.compositor = Compositor::QuietSince(now.saturating_sub(NOTIFY_AFTER)),
            IdleEvent::CompositorInput => self.compositor = Compositor::Busy,
        }
    }

    /// Input that reached ClamShell itself (a tap, the pad on its screens), at `now`.
    pub(crate) fn own_input(&mut self, now: u64) {
        self.last_input = now;
    }

    /// Back from sleep (or the clock first known) at `now`: as if there had just been input.
    pub(crate) fn resumed(&mut self, now: u64) {
        self.last_input = now;
        if let Compositor::QuietSince(_) = self.compositor {
            self.compositor = Compositor::QuietSince(now);
        }
    }

    /// How long there has been no input at `now`; none if no source can tell.
    fn idle_for(&self, now: u64) -> Option<u64> {
        let since = match self.compositor {
            Compositor::Busy => return Some(0),
            Compositor::QuietSince(t) => t.max(self.last_input),
            Compositor::Silent if self.watching => self.last_input,
            Compositor::Silent => return None,
        };
        Some(now.saturating_sub(since))
    }

    /// The stage `policy` wants at `now`.
    pub(crate) fn wanted(&self, now: u64, policy: Policy) -> Stage {
        let Some(t) = self.idle_for(now) else {
            return Stage::Awake;
        };
        if policy.may_sleep && policy.sleep.is_some_and(|sleep| t >= sleep) {
            return Stage::Asleep;
        }
        match policy.screen_off {
            Some(off) if t >= off => Stage::Off,
            Some(off) if t + DIM_BEFORE >= off => Stage::Dimmed,
            _ => Stage::Awake,
        }
    }

    /// Moves to `stage`; the stage it was at.
    pub(crate) fn set(&mut self, stage: Stage, now: u64) -> Stage {
        let was = std::mem::replace(&mut self.stage, stage);
        if was >= Stage::Off && stage < Stage::Off {
            self.woke_at = Some(now);
        }
        was
    }

    /// A press ending at `now` is the wake-up, not a press: the screens are off, or just came back.
    pub(crate) fn swallows_press(&self, now: u64) -> bool {
        self.stage >= Stage::Off || self.woke_at.is_some_and(|t| now <= t + WAKE_GRACE)
    }
}

impl State {
    /// Idle as things are now: the stage moves on (dim, screens off, sleep) or back (any input), with
    /// what the screens need for it.
    pub(crate) fn follow_idle(&mut self) -> Vec<Effect> {
        // Sleep only in the device's own session, and not while a game is in front.
        let may_sleep = self.device_session && self.session.front() != Front::Game;
        let policy = Policy::of(&self.settings.global, may_sleep);
        let now = self.unix_secs;
        let wanted = self.idle.wanted(now, policy);
        if wanted == self.idle.stage() {
            return vec![];
        }
        let was = self.idle.set(wanted, now);
        let mut effects = vec![];
        // Brightness: down while dimmed or darker, back when awake.
        if (was >= Stage::Dimmed) != (wanted >= Stage::Dimmed) {
            effects.extend(if wanted >= Stage::Dimmed { self.levels.dimmed_effect() } else { self.levels.undimmed_effect() });
        }
        if (was >= Stage::Off) != (wanted >= Stage::Off) {
            effects.push(Effect::Screens(wanted < Stage::Off));
        }
        if wanted == Stage::Asleep {
            effects.push(Effect::Sleep);
        }
        effects
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
        // The devices still count: the controller (a joystick isn't the compositor's input).
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
        use crate::sys::Status;
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
        s.apply(Event::Tick { unix_secs: 1000 });
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        // Defaults: screens off after 5 minutes, sleep after 15.
        assert_eq!(brightness(&s.apply(Event::Tick { unix_secs: 1270 }).1), Some(20), "dimmed to a third");
        assert_eq!(s.apply(Event::Tick { unix_secs: 1300 }).1, vec![Effect::Screens(false)]);
        assert!(!s.apply(Event::Tick { unix_secs: 1900 }).1.contains(&Effect::Sleep), "not the device's session");
        // Input: the screens come back, at their brightness; the press that woke them does nothing.
        let fx = s.apply(Event::Idle(IdleEvent::DeviceInput));
        assert!(fx.1.contains(&Effect::Screens(true)) && brightness(&fx.1) == Some(60), "{fx:?}");
        // In the device's session it sleeps.
        let mut s = State::new(crate::Init { device_session: true, ..crate::testkit::init() });
        s.apply(Event::Tick { unix_secs: 1000 });
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        assert!(s.apply(Event::Tick { unix_secs: 1900 }).1.contains(&Effect::Sleep));
    }

    #[test]
    fn what_is_heard_before_the_clock_is_known_counts_from_its_first_tick() {
        use crate::Event;
        let mut s = crate::testkit::state();
        // At start the compositor reports quiet before the clock thread's first tick.
        s.apply(Event::Idle(IdleEvent::Watching(true)));
        s.apply(Event::Idle(IdleEvent::CompositorQuiet));
        let fx = s.apply(Event::Tick { unix_secs: 1_792_060_000 }).1;
        assert!(!fx.contains(&Effect::Screens(false)), "not off at once: {fx:?}");
        assert_eq!(s.idle().stage(), Stage::Awake);
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
