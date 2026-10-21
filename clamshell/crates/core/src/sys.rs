//! The system as the bar, the sysinfo tab and the battery tab show it: the last status reading, the
//! metrics with a minute of history, the battery's history and what spends it, and whether touchpad mode
//! works. The volume and brightness readings in the status feed [`crate::levels::Levels`], which shows
//! them.

use crate::battery::Battery;
use crate::effect::Effect;
use crate::metrics::{History, Metrics};

/// Battery levels (while not charging) at which a notification warns over whatever runs, once each until
/// it charges; UPower hibernates the device at 3 % (the image's UPower.conf).
const WARN_AT: [u8; 2] = [10, 5];

/// The system's status as system threads read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Battery charge 0-100, if there is a battery.
    pub battery: Option<u8>,
    pub charging: bool,
    /// Wi-Fi link quality 0-100 while connected; `None` when not.
    pub wifi: Option<u8>,
    /// Output volume 0-100, if known, and whether it is muted.
    pub volume: Option<u8>,
    pub muted: bool,
    /// The first backlight's level 0-100 as read, if there is one: the brightness until ClamShell sets
    /// one (see [`crate::levels::Brightness`]).
    pub brightness: Option<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Sys {
    /// The last status reading, as read (the levels shown are [`crate::levels::Levels`]').
    pub status: Status,
    pub metrics: Metrics,
    pub history: History,
    pub battery: Battery,
    /// The time zone and the ones to choose from, as last read.
    pub time: crate::time::TimeZones,
    /// The account, as last read (see [`crate::account`]).
    pub account: crate::account::Account,
    /// The lowest of [`WARN_AT`] warned about since the battery last charged.
    warned: Option<u8>,
}

impl Sys {
    /// After a status reading: a warning when the battery has fallen to one of [`WARN_AT`] (the lower one
    /// marked urgent).
    pub(crate) fn battery_warning(&mut self) -> Option<Effect> {
        let Status { battery: Some(level), charging: false, .. } = self.status else {
            self.warned = None;
            return None;
        };
        let at = WARN_AT.into_iter().filter(|&t| level <= t).min()?;
        if self.warned.is_some_and(|w| w <= at) {
            return None;
        }
        self.warned = Some(at);
        let summary = format!("Battery at {level} %");
        Some(Effect::Notify { summary, body: "Plug in the charger: at 3 % the device hibernates.".to_owned(), urgent: at == WARN_AT[1] })
    }

    /// A metrics reading; the frame rate comes separately (see [`Sys::frames`]) and is kept.
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
        assert_eq!(read(&mut s, Reading::Metrics(metrics::Metrics::default())), Dirty::NONE, "sysinfo isn't open");
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(read(&mut s, Reading::Frames { fps: Some(60.0), frame_ms: Some(16.7) }), Dirty::BOTTOM);
        assert_eq!(s.sys().metrics.fps, Some(60.0));
        read(&mut s, Reading::Metrics(metrics::Metrics::default()));
        assert_eq!(s.sys().metrics.fps, Some(60.0), "the frame rate comes separately");
    }
}
