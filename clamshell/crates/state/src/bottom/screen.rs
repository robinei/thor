//! The bottom screen shared with programs' windows, such as a dual-screen emulator's second screen: the
//! windows' tab shows the window placed there (see [`crate::session::below`]), right stick press switches
//! between it and ClamShell's view, and the screen's power, by hand (Home+Start) or by itself.

use crate::bottom::Bottom;
use crate::settings::BottomWhenIdle;
use crate::{Clock, Session};

/// What the bottom screen shows, besides being on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BottomScreen {
    /// Turned off with Home+Start.
    off: bool,
    /// Turned off by itself, when idle or dark on battery ([`Bottom::follow_power`]).
    idle_off: bool,
}

/// What the second screen's power follows from the rest of the state (see [`Bottom::follow_power`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Power {
    /// Windows are moved to the bottom screen.
    pub(crate) any_down: bool,
    /// A program is in force.
    pub in_game: bool,
    /// The setting in force for the second screen.
    pub(crate) when_idle: BottomWhenIdle,
    /// Running on battery.
    pub discharging: bool,
    /// A refresh calibration is running.
    pub(crate) refreshing: bool,
}

/// How long, in seconds, the second screen goes untouched and without the pad's focus before it dims,
/// and after that before it powers off by itself. It only powers off on battery and when the setting
/// isn't "Screen on" (see [`Bottom::follow_power`]).
const DIM_AFTER: u64 = 60;
const OFF_AFTER: u64 = 180;

impl Bottom {
    /// Whether the second screen is dark. It is when the program in force (`in_game`) wants it dark and
    /// the screen hasn't been tapped since.
    pub(crate) fn dark(&self, in_game: bool, when_idle: BottomWhenIdle) -> bool {
        in_game && when_idle == BottomWhenIdle::DarkInGame && !self.woken
    }

    /// Whether the second screen is drawn dimmed. It is when it shows ClamShell's view and has gone
    /// untouched, without the pad's focus, for [`DIM_AFTER`]. Windows moved there are never dimmed, since
    /// they are watched without touching. This spares the OLED while a game played with the pad on the
    /// main screen keeps the device awake and nobody looks below. A touch on it, or the pad's focus
    /// coming to it, brings it back. Kept by [`Bottom::follow_dim`], which runs after every event.
    pub fn dimmed(&self) -> bool {
        self.dimmed
    }

    /// Seconds since the second screen was last used at `now`, or 0 before the clock's first reading.
    fn idle_for(&self, now: u64) -> u64 {
        self.used_at.map_or(0, |at| now.saturating_sub(at))
    }

    /// The second screen is in use, so it is lit.
    pub(crate) fn used(&mut self, session: &Session, clock: &Clock) {
        self.used_at = Some(clock.mono());
        self.follow_dim(session, clock);
    }

    /// Runs after any event. The time counts from the first clock reading.
    pub(crate) fn follow_dim(&mut self, session: &Session, clock: &Clock) {
        if self.used_at.is_none() && clock.read() {
            self.used_at = Some(clock.mono());
        }
        self.dimmed = !self.shows_windows(session) && self.idle_for(clock.mono()) >= DIM_AFTER;
    }

    /// Whether the bottom screen shows the windows moved there: the windows' tab is open and there are
    /// some. With none, the tab shows how to move one there.
    pub fn shows_windows(&self, session: &Session) -> bool {
        self.shows(session.bottom_window().is_some())
    }

    /// Whether the bottom screen shows the windows moved there, when there are any (`any_down`).
    fn shows(&self, any_down: bool) -> bool {
        self.tab == super::Tab::Windows && any_down
    }

    /// Whether the bottom screen is turned off, by hand or by itself.
    pub fn off(&self) -> bool {
        self.screen.off || self.screen.idle_off
    }

    /// Whether the bottom screen should be off by itself at `now`. That needs all of: running on battery,
    /// the setting in force not "Screen on", no refresh calibration running, ClamShell's view on it rather
    /// than a window moved down, and nothing being typed there. On top of that, the screen must be dark
    /// for a game and not tapped since, or untouched and without the pad's focus for [`DIM_AFTER`] plus
    /// [`OFF_AFTER`]. The panel is driven in video mode and streamed every frame whatever it shows, so
    /// only turning it off saves power.
    fn idle_wanted(&self, p: Power, now: u64) -> bool {
        let untouched = self.idle_for(now) >= DIM_AFTER + OFF_AFTER;
        p.discharging
            && !p.refreshing
            && p.when_idle != BottomWhenIdle::On
            && !self.shows(p.any_down)
            && self.editor().is_none()
            && (self.dark(p.in_game, p.when_idle) || untouched)
    }

    /// Runs after any event. Turns the bottom screen off or on by itself as [`Bottom::idle_wanted`] says.
    /// A touch, the pad's focus, a window moved down or the charger turn it on again. The screen is told
    /// after the event (see [`crate::sync::Sent::outputs`]).
    pub(crate) fn follow_power(&mut self, p: Power, now: u64) {
        self.screen.idle_off = self.idle_wanted(p, now);
    }

    /// The darkened second screen was tapped awake. This lasts until the next game.
    pub(crate) fn wake(&mut self) {
        self.woken = true;
    }

    /// A game started, so the screen may go dark again.
    pub(crate) fn unwake(&mut self) {
        self.woken = false;
    }

    /// Opens the windows' tab as a window moves there. The tab left is settled later (see
    /// [`Bottom::take_left`]), since this can happen while windows are followed.
    fn open_windows_tab(&mut self) {
        if let Some(left) = self.choose_tab(super::Tab::Windows) {
            self.left = Some(left);
        }
    }

    /// The tab left as a window moved down, for State to settle as any tab left (`left_tab`).
    pub(crate) fn take_left(&mut self) -> Option<super::Tab> {
        self.left.take()
    }

    /// The tab Home + right stick press goes to: the windows' tab, or from it the tab it was opened from.
    pub(crate) fn view_toggled(&self) -> super::Tab {
        if self.tab == super::Tab::Windows { self.before_windows } else { super::Tab::Windows }
    }

    /// A window of the program in front on the top screen that is on the bottom screen but hidden by
    /// ClamShell's view. The bar offers to show it. Home groups match all launches of one program.
    /// It does not allocate, since the bar asks every frame.
    pub fn hidden_down(&self, session: &Session) -> Option<i64> {
        if self.shows_windows(session) {
            return None;
        }
        let front = &session.top_window()?.group;
        session.bottom_window().filter(|w| w.group == *front).map(|w| w.id)
    }

    /// Shows the window on the bottom screen, as for a tap on a program whose windows are all there: opens
    /// the windows' tab, turning the bottom screen on again if it was off by hand.
    pub(crate) fn show_below(&mut self) {
        self.open_windows_tab();
        self.screen.off = false;
    }

    /// Home+Start turns the bottom screen off, or on again. If it had turned off by itself, this turns it
    /// on, as a touch would.
    pub(crate) fn toggle_power(&mut self, p: Power, now: u64) {
        if self.screen.idle_off && !self.screen.off {
            self.used_at = Some(now);
            self.woken = true;
            self.follow_power(p, now);
            return;
        }
        self.screen.off = !self.screen.off;
        // Turned on again by hand, so it is in use and shouldn't turn off by itself either.
        if !self.screen.off {
            self.used_at = Some(now);
            self.woken = true;
            self.screen.idle_off = false;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests;
