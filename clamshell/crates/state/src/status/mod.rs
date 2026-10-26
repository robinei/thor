//! The system as the bar, the sysinfo tab and the battery tab show it: the last status reading, the
//! metrics with a minute of history, the battery's history and what is using it, the time zones and the
//! account. The volume and brightness readings in the status go to [`crate::levels::Levels`], which
//! shows them.

pub mod battery;
pub mod metrics;
pub mod time;

use crate::notifications::{Notification, NotificationId, Urgency};
use battery::{Battery, Group};
use metrics::{History, Metrics};
use time::{TimePage, TimeZones};

/// Battery levels at which a notification warns, over the bottom screen, while discharging. Each warns
/// once until the battery stops discharging. On the charger with the charge held at a limit, it isn't
/// charging but isn't discharging either. UPower hibernates the device at 3 % (the image's UPower.conf).
const WARN_AT: [u8; 2] = [10, 5];

/// The system's status as system threads read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Battery charge 0-100, if there is a battery.
    pub battery: Option<u8>,
    pub charging: bool,
    /// On battery, meaning the battery is discharging. False on the charger, whether or not the charge
    /// is held at a limit.
    pub discharging: bool,
    /// Wi-Fi link quality 0-100 while connected, `None` when not.
    pub wifi: Option<u8>,
    /// Output volume 0-100, if known, and whether it is muted.
    pub volume: Option<u8>,
    pub muted: bool,
    /// The first backlight's level 0-100 as read, if there is a backlight. It stands for the brightness
    /// until ClamShell sets one (see [`crate::levels::Brightness`]).
    pub brightness: Option<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Sys {
    /// The last status reading, as read. The levels that are shown come from [`crate::levels::Levels`].
    status: Status,
    metrics: Metrics,
    history: History,
    battery: Battery,
    /// The time zone and the ones to choose from, as last read.
    time: TimeZones,
    /// The lowest of [`WARN_AT`] warned about since the battery last charged.
    warned: Option<u8>,
    /// The battery tab row whose menu is open (see [`battery`]).
    spender_menu: Option<Group>,
    /// Show clocks in 12-hour format (see [`time`]).
    twelve_hour: bool,
    /// What the time tab shows: its main page, the regions, or a region's zones.
    time_page: TimePage,
}

impl Sys {
    pub(crate) fn new(twelve_hour: bool) -> Sys {
        Sys { twelve_hour, ..Sys::default() }
    }

    /// The last status reading. Its volume and mute are as read: the levels shown are
    /// [`crate::levels::Levels`]'.
    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn battery(&self) -> &Battery {
        &self.battery
    }

    pub fn time_zones(&self) -> &TimeZones {
        &self.time
    }

    /// A reading of the battery's history and what is using the machine. A row's menu closes once the
    /// row is gone.
    pub(crate) fn read_battery(&mut self, battery: Battery) {
        self.battery = battery;
        if let Some(group) = &self.spender_menu
            && !self.battery.spenders.iter().any(|s| s.group == *group)
        {
            self.spender_menu = None;
        }
    }

    /// A tab was left: the time tab goes back to its main page, and the battery tab's menu closes.
    pub(crate) fn left_tab(&mut self) {
        self.time_page = TimePage::Main;
        self.spender_menu = None;
    }

    /// A reading of the time zones.
    pub(crate) fn read_time(&mut self, time: TimeZones) {
        self.time = time;
    }

    /// A status reading. What the bar shows of the volume and mute comes from the levels.
    pub(crate) fn read_status(&mut self, status: Status) {
        self.status = status;
    }

    /// Runs after a status reading. Returns a warning when the battery has fallen to one of [`WARN_AT`],
    /// the lower one critical, so it stays until dismissed and wakes the screens. Each replaces the one
    /// before.
    pub(crate) fn battery_warning(&mut self) -> Option<Notification> {
        let Status { battery: Some(level), discharging: true, .. } = self.status else {
            self.warned = None;
            return None;
        };
        let at = WARN_AT.into_iter().filter(|&t| level <= t).min()?;
        if self.warned.is_some_and(|w| w <= at) {
            return None;
        }
        self.warned = Some(at);
        Some(Notification {
            id: NotificationId::Battery,
            app: "ClamShell".to_owned(),
            summary: format!("Battery at {level} %"),
            body: "Plug in the charger: at 3 % the device hibernates.".to_owned(),
            urgency: if at == WARN_AT[1] { Urgency::Critical } else { Urgency::Normal },
            action: None,
            timeout_ms: -1,
        })
    }

    /// A metrics reading. The frame rate comes separately (see [`Sys::frames`]) and is kept.
    pub(crate) fn read_metrics(&mut self, mut m: Metrics) {
        m.fps = self.metrics.fps;
        m.frame_ms = self.metrics.frame_ms;
        self.metrics = m;
        self.history.push(&m);
    }

    pub(crate) fn frames(&mut self, fps: Option<f32>, frame_ms: Option<f32>) {
        self.metrics.fps = fps;
        self.metrics.frame_ms = frame_ms;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    use crate::notifications::{NotificationId, Urgency};

    /// The battery's notification a status reading gave, if any. The one before is dismissed first.
    fn warned(s: &mut State, battery: Option<u8>, discharging: bool) -> Option<(String, Urgency)> {
        s.applied(crate::notifications::NotificationEvent::Dismissed(NotificationId::Battery).into());
        s.applied(Reading::Status(Status { battery, discharging, ..Status::default() }).into());
        s.notifications().get(NotificationId::Battery).map(|n| (n.summary.clone(), n.urgency))
    }

    #[test]
    fn the_battery_warns_only_while_discharging() {
        let mut s = state();
        let warns = |s: &mut State, battery, discharging| warned(s, Some(battery), discharging).is_some();
        assert!(!warns(&mut s, 9, false), "on the charger, held at a limit: not charging, not discharging");
        assert!(warns(&mut s, 9, true));
        assert!(!warns(&mut s, 8, true), "once");
        assert!(warns(&mut s, 5, true), "the lower one");
        assert!(!warns(&mut s, 5, false));
        assert!(warns(&mut s, 5, true), "again after the charger");
    }

    /// The warning says the level and what to do; the one at 5 % is critical, so it stays until
    /// dismissed and wakes the screens. A fall past both at once warns once, critically. Each replaces the
    /// one before, and none goes to the bus.
    #[test]
    fn the_battery_warning_says_the_level_and_is_critical_at_5() {
        let mut s = state();
        let notify = |s: &mut State, battery: Option<u8>| warned(s, battery, true);
        assert_eq!(notify(&mut s, Some(11)), None);
        assert_eq!(notify(&mut s, None), None, "no battery");
        assert_eq!(notify(&mut s, Some(10)), Some(("Battery at 10 %".into(), Urgency::Normal)));
        let body = &s.notifications().get(NotificationId::Battery).unwrap().body;
        assert_eq!(body, "Plug in the charger: at 3 % the device hibernates.");
        assert_eq!(notify(&mut s, Some(5)), Some(("Battery at 5 %".into(), Urgency::Critical)));
        assert_eq!(notify(&mut s, Some(4)), None, "once");
        let mut s = state();
        assert_eq!(notify(&mut s, Some(4)), Some(("Battery at 4 %".into(), Urgency::Critical)), "past both at once");
        assert_eq!(notify(&mut s, Some(9)), None, "a higher reading while discharging doesn't warn again");
        // A lower warning replaces the one before.
        let mut s = state();
        let low = |s: &mut State, battery| s.applied(Reading::Status(Status { battery: Some(battery), discharging: true, ..Status::default() }).into());
        low(&mut s, 9);
        low(&mut s, 4);
        assert_eq!(s.notifications().iter().map(|n| n.summary.as_str()).collect::<Vec<_>>(), ["Battery at 4 %"]);
        let fx = s.applied(crate::notifications::NotificationEvent::Dismissed(NotificationId::Battery).into());
        assert!(fx.is_empty() && s.notifications().is_empty(), "ClamShell's own close without a word to the bus: {fx:?}");
    }

    #[test]
    fn the_frame_rate_is_kept_apart_from_the_metrics() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        read(&mut s, Reading::Frames { fps: Some(60.0), frame_ms: Some(16.7) });
        assert_eq!(s.sys().metrics().fps, Some(60.0));
        read(&mut s, Reading::Metrics(crate::status::metrics::Metrics::default()));
        assert_eq!(s.sys().metrics().fps, Some(60.0), "the frame rate comes separately");
    }
}
