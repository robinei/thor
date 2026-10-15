//! The system as the bar, the sysinfo tab and the battery tab show it: the last status reading, the
//! metrics with a minute of history, the battery's history and what spends it, and whether touchpad mode
//! works. The volume and brightness readings in the status feed [`crate::levels::Levels`], which shows
//! them.

use crate::battery::Battery;
use crate::metrics::{History, Metrics};

/// The system's status as policy threads read it.
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

/// Why touchpad mode doesn't work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TouchpadProblem {
    /// There is no touchscreen (or not the one asked for).
    NoTouchscreen,
    /// The touchscreen couldn't be taken over (the error).
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct Sys {
    /// The last status reading, as read (the levels shown are [`crate::levels::Levels`]').
    pub status: Status,
    pub metrics: Metrics,
    pub history: History,
    pub battery: Battery,
    /// Why touchpad mode doesn't work, if it doesn't.
    pub touchpad_problem: Option<TouchpadProblem>,
}

impl Sys {
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
        let problem = || Reading::TouchpadProblem(Some(TouchpadProblem::NoTouchscreen));
        assert_eq!(read(&mut s, problem()), Dirty::NONE, "kept, not shown");
        assert_eq!(s.sys().touchpad_problem, Some(TouchpadProblem::NoTouchscreen));
        tap(&mut s, Id::Tab(Tab::Touchpad));
        assert_eq!(read(&mut s, Reading::TouchpadProblem(None)), Dirty::BOTTOM);
        assert_eq!(read(&mut s, Reading::Metrics(metrics::Metrics::default())), Dirty::NONE, "sysinfo isn't open");
        tap(&mut s, Id::Tab(Tab::Sysinfo));
        assert_eq!(read(&mut s, Reading::Frames { fps: Some(60.0), frame_ms: Some(16.7) }), Dirty::BOTTOM);
        assert_eq!(s.sys().metrics.fps, Some(60.0));
        read(&mut s, Reading::Metrics(metrics::Metrics::default()));
        assert_eq!(s.sys().metrics.fps, Some(60.0), "the frame rate comes separately");
    }
}
