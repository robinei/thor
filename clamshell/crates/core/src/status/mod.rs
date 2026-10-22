//! The system as the bar, the sysinfo tab and the battery tab show it: the last status reading, the
//! metrics with a minute of history, the battery's history and what is using it, the time zones and the
//! account. The volume and brightness readings in the status go to [`crate::levels::Levels`], which
//! shows them.

pub mod battery;
pub mod metrics;
pub mod time;

use crate::effect::Effect;
use crate::kit::replace;
use crate::out::Out;
use battery::{Battery, Group};
use metrics::{History, Metrics};
use time::{TimePage, TimeZones};

/// Battery levels at which a notification warns, over whatever is running, while not charging. Each
/// warns once until the battery charges. UPower hibernates the device at 3 % (the image's UPower.conf).
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

    /// A reading of the battery's history and what is using the machine. Returns whether it changed.
    pub(crate) fn read_battery(&mut self, battery: Battery) -> bool {
        replace(&mut self.battery, battery)
    }

    /// A reading of the time zones. Returns whether it changed.
    pub(crate) fn read_time(&mut self, time: TimeZones) -> bool {
        replace(&mut self.time, time)
    }

    /// A status reading. Returns whether it changed, leaving out the volume and mute: what the bar shows
    /// of those comes from the levels.
    pub(crate) fn read_status(&mut self, status: Status) -> bool {
        let shown = |s: Status| Status { volume: None, muted: false, ..s };
        let changed = shown(status) != shown(self.status);
        self.status = status;
        changed
    }

    /// Runs after a status reading. Gives a warning when the battery has fallen to one of [`WARN_AT`],
    /// with the lower one marked urgent.
    pub(crate) fn battery_warning(&mut self, out: &mut Out) {
        let Status { battery: Some(level), charging: false, .. } = self.status else {
            self.warned = None;
            return;
        };
        let Some(at) = WARN_AT.into_iter().filter(|&t| level <= t).min() else {
            return;
        };
        if self.warned.is_some_and(|w| w <= at) {
            return;
        }
        self.warned = Some(at);
        let summary = format!("Battery at {level} %");
        out.effects.push(Effect::Notify { summary, body: "Plug in the charger: at 3 % the device hibernates.".to_owned(), urgent: at == WARN_AT[1] });
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
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn status_redraws_the_bottom_only_when_it_changes() {
        let mut s = state();
        let status = Status { battery: Some(50), ..Status::default() };
        assert_eq!(read(&mut s, Reading::Status(status)), Dirty::BOTTOM);
        assert_eq!(read(&mut s, Reading::Status(status)), Dirty::NONE);
    }

    #[test]
    fn a_tabs_readings_redraw_only_while_it_is_open() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Touchpad));
        assert_eq!(read(&mut s, Reading::Metrics(crate::status::metrics::Metrics::default())), Dirty::NONE, "sysinfo isn't open");
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(read(&mut s, Reading::Frames { fps: Some(60.0), frame_ms: Some(16.7) }), Dirty::BOTTOM);
        assert_eq!(s.sys().metrics().fps, Some(60.0));
        read(&mut s, Reading::Metrics(crate::status::metrics::Metrics::default()));
        assert_eq!(s.sys().metrics().fps, Some(60.0), "the frame rate comes separately");
    }
}
