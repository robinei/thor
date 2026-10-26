use super::*;
use crate::HardwareKey;
use crate::idle::{IdleEvent, Stage};
use crate::settings::{BottomWhenIdle, Choice, Setting};
use crate::testkit::{Applied, state};
use crate::{Compositor, Event, State, Status};

fn note(id: u32, urgency: Urgency, timeout_ms: i32) -> Notification {
    Notification { id: NotificationId::Bus(id), app: "mail".into(), summary: format!("note {id}"), body: "body".into(), urgency, action: None, timeout_ms }
}

fn notify(s: &mut State, n: Notification) -> Vec<Effect> {
    s.applied(NotificationEvent::Notified(Box::new(n)).into())
}

fn held(s: &State) -> Vec<NotificationId> {
    s.notifications().iter().map(|n| n.id).collect()
}

fn bus(id: u32) -> NotificationId {
    NotificationId::Bus(id)
}

fn closed(id: u32, reason: CloseReason) -> Effect {
    Effect::NotificationClosed { id, reason }
}

/// A state whose clock has been read, with the screens on.
fn ticking() -> State {
    let mut s = state();
    s.applied(Event::tick(1000));
    s
}

#[test]
fn the_time_a_notification_shows_follows_its_request() {
    assert_eq!(note(1, Urgency::Normal, -1).lasts(), Some(DEFAULT_SECS), "left to us");
    assert_eq!(note(1, Urgency::Normal, 0).lasts(), None, "until dismissed");
    assert_eq!(note(1, Urgency::Low, 1500).lasts(), Some(2), "whole seconds, rounded up");
    assert_eq!(note(1, Urgency::Normal, 1).lasts(), Some(1));
    assert_eq!(note(1, Urgency::Critical, 3000).lasts(), None, "critical stays");
    assert_eq!(note(1, Urgency::Normal, -5).lasts(), Some(DEFAULT_SECS), "any negative is left to us");
}

#[test]
fn notifications_are_held_newest_first_and_one_sent_again_replaces_its_own() {
    let mut s = ticking();
    assert_eq!(notify(&mut s, note(1, Urgency::Normal, 0)), []);
    notify(&mut s, note(2, Urgency::Normal, 0));
    assert_eq!(held(&s), [bus(2), bus(1)]);
    let replaced = Notification { summary: "again".into(), ..note(1, Urgency::Normal, 0) };
    assert_eq!(notify(&mut s, replaced), [], "a replacement closes nothing");
    assert_eq!(held(&s), [bus(1), bus(2)], "the replacement is the newest");
    assert_eq!(s.notifications().get(bus(1)).map(|n| n.summary.as_str()), Some("again"));
}

/// Settle's step 15: a notification's time runs only while the bottom screen is lit, from when it is.
#[test]
fn a_notification_waits_for_the_bottom_screen_and_then_shows_its_time() {
    let mut s = ticking();
    s.applied(Event::HardwareKey(HardwareKey::BottomPower));
    assert!(s.bottom().off());
    notify(&mut s, note(1, Urgency::Normal, -1));
    assert_eq!(s.notifications().expires_at(bus(1)), None, "waiting");
    assert_eq!(s.applied(Event::tick(1100)), [], "kept while the screen is off");
    s.applied(Event::HardwareKey(HardwareKey::BottomPower));
    assert_eq!(s.notifications().expires_at(bus(1)), Some(1100 + DEFAULT_SECS), "seen from now");
    s.applied(Event::tick(1104));
    assert_eq!(held(&s), [bus(1)]);
    assert_eq!(s.applied(Event::tick(1105)), [closed(1, CloseReason::Expired)]);
    assert!(s.notifications().is_empty());
}

#[test]
fn its_time_starts_again_when_the_screen_is_lit_again() {
    let mut s = ticking();
    notify(&mut s, note(1, Urgency::Normal, 4000));
    assert_eq!(s.notifications().expires_at(bus(1)), Some(1004));
    s.applied(Event::tick(1002));
    s.applied(Event::HardwareKey(HardwareKey::BottomPower));
    assert_eq!(s.applied(Event::tick(1010)), [], "not seen, so not expired");
    s.applied(Event::HardwareKey(HardwareKey::BottomPower));
    assert_eq!(s.notifications().expires_at(bus(1)), Some(1014));
}

#[test]
fn the_screens_going_off_by_idle_hold_its_time_too() {
    let mut s = ticking();
    s.applied(Event::Idle(IdleEvent::Watching(true)));
    notify(&mut s, note(1, Urgency::Normal, 0));
    notify(&mut s, note(2, Urgency::Normal, -1));
    // Screens off after the default 5 minutes without input.
    s.applied(Event::tick(1300));
    assert_eq!(s.idle().stage(), Stage::Off);
    assert_eq!(held(&s), [bus(2), bus(1)], "nothing expires while unseen");
    s.applied(Event::Idle(IdleEvent::DeviceInput));
    assert_eq!(s.notifications().expires_at(bus(2)), Some(1300 + DEFAULT_SECS));
    assert_eq!(s.applied(Event::tick(1305)), [closed(2, CloseReason::Expired)]);
    assert_eq!(held(&s), [bus(1)], "the one that stays until dismissed is kept");
}

#[test]
fn a_program_closes_its_own_and_an_unknown_id_closes_nothing() {
    let mut s = ticking();
    notify(&mut s, note(1, Urgency::Normal, 0));
    assert_eq!(s.applied(NotificationEvent::Closed(7).into()), []);
    assert_eq!(s.applied(NotificationEvent::Closed(1).into()), [closed(1, CloseReason::Closed)]);
    assert_eq!(s.applied(NotificationEvent::Closed(1).into()), [], "once");
    assert!(s.notifications().is_empty());
}

/// The action comes before the close: a client such as notify-send's `--wait` ends on the close and
/// would miss an action after it.
#[test]
fn a_tap_invokes_the_default_action_then_dismisses() {
    let mut s = ticking();
    notify(&mut s, Notification { action: Some("default".into()), ..note(1, Urgency::Normal, 0) });
    notify(&mut s, note(2, Urgency::Normal, 0));
    let fx = s.applied(NotificationEvent::Tapped(bus(1)).into());
    assert_eq!(fx, [Effect::ActionInvoked { id: 1, key: "default".into() }, closed(1, CloseReason::Dismissed)]);
    assert_eq!(s.applied(NotificationEvent::Tapped(bus(2)).into()), [closed(2, CloseReason::Dismissed)], "no action: only dismissed");
    assert_eq!(s.applied(NotificationEvent::Tapped(bus(2)).into()), [], "gone already");
}

#[test]
fn the_close_mark_only_dismisses() {
    let mut s = ticking();
    notify(&mut s, Notification { action: Some("default".into()), ..note(1, Urgency::Normal, 0) });
    assert_eq!(s.applied(NotificationEvent::Dismissed(bus(1)).into()), [closed(1, CloseReason::Dismissed)]);
    assert!(s.notifications().is_empty());
}

#[test]
fn a_tap_on_dark_screens_only_wakes_them() {
    let mut s = ticking();
    s.applied(Event::Idle(IdleEvent::Watching(true)));
    notify(&mut s, note(1, Urgency::Normal, 0));
    s.applied(Event::tick(1300));
    let fx = s.applied(NotificationEvent::Dismissed(bus(1)).into());
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: true, bottom: true })), "{fx:?}");
    assert_eq!(held(&s), [bus(1)], "not dismissed");
    s.applied(Event::tick(1302));
    assert_eq!(s.applied(NotificationEvent::Dismissed(bus(1)).into()), [closed(1, CloseReason::Dismissed)], "a tap once awake");
}

#[test]
fn past_the_most_the_oldest_is_closed() {
    let mut s = ticking();
    for id in 1..=MOST as u32 {
        assert_eq!(notify(&mut s, note(id, Urgency::Normal, 0)), []);
    }
    assert_eq!(notify(&mut s, note(100, Urgency::Normal, 0)), [closed(1, CloseReason::Undefined)]);
    assert_eq!(s.notifications().len(), MOST);
    assert_eq!(held(&s).first(), Some(&bus(100)));
}

#[test]
fn a_critical_notification_wakes_the_screens_and_turns_the_bottom_screen_on() {
    let mut s = ticking();
    s.applied(Event::Idle(IdleEvent::Watching(true)));
    s.applied(Event::HardwareKey(HardwareKey::BottomPower));
    s.applied(Event::tick(1300));
    assert_eq!(s.idle().stage(), Stage::Off);
    assert_eq!(notify(&mut s, note(1, Urgency::Normal, -1)), [], "an ordinary one waits");
    let fx = notify(&mut s, note(2, Urgency::Critical, -1));
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: true, bottom: true })), "{fx:?}");
    assert_eq!(s.idle().stage(), Stage::Awake);
    assert!(!s.bottom().off(), "on again, though it was off by hand");
    assert_eq!(s.notifications().expires_at(bus(1)), Some(1300 + DEFAULT_SECS), "the ordinary one is seen with it");
    assert_eq!(s.notifications().expires_at(bus(2)), None, "the critical one stays");
}

#[test]
fn a_critical_notification_lights_a_screen_dark_for_a_game() {
    let mut s = ticking();
    let status = Status { battery: Some(50), discharging: true, ..Status::default() };
    s.applied(Event::Reading(crate::Reading::Status(status)));
    s.settings_mut().global.set(Setting::BottomScreen, BottomWhenIdle::DarkInGame.value());
    crate::testkit::start(&mut s, "zelda-lttp");
    assert!(s.bottom_dark() && s.bottom().off());
    notify(&mut s, note(1, Urgency::Normal, -1));
    assert!(s.bottom().off(), "an ordinary one leaves it dark");
    let fx = notify(&mut s, note(2, Urgency::Critical, 0));
    assert!(fx.contains(&Effect::Compositor(Compositor::Outputs { top: true, bottom: true })), "{fx:?}");
    assert!(!s.bottom_dark() && !s.bottom().off());
}
