//! The bottom screen shared with programs' windows, such as a dual-screen emulator's second screen: the
//! windows' tab shows the window placed there (see [`crate::session::below`]), right stick press switches
//! between it and ClamShell's view, and the screen's power, by hand (Home+Start) or by itself.

use crate::bottom::Bottom;
use crate::settings::BottomInGame;
use crate::{Clock, Session};

/// The bottom screen's power as chosen; whether it is off by itself is worked out (see [`Bottom::off_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BottomScreen {
    /// Turned off with Home+Start.
    off: bool,
}

/// What the second screen's power follows from the rest of the state (see [`Bottom::off_at`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Power {
    /// Windows are moved to the bottom screen.
    pub(crate) any_down: bool,
    /// A program is in force.
    pub in_game: bool,
    /// The setting in force for the second screen in a game.
    pub(crate) in_game_setting: BottomInGame,
    /// How long untouched before it goes off ([`crate::settings::Setting::ScreenOff`]), or never.
    pub(crate) screen_off: Option<u64>,
    /// A refresh calibration is running.
    pub(crate) refreshing: bool,
}

impl Bottom {
    /// Whether the second screen is dark. It is when the program in force (`in_game`) wants it dark and
    /// the screen hasn't been tapped since.
    pub(crate) fn dark(&self, in_game: bool, setting: BottomInGame) -> bool {
        in_game && setting == BottomInGame::Dark && !self.woken
    }

    /// Whether the second screen is drawn dimmed at `now`. It is when it shows ClamShell's view and has
    /// gone untouched, without the pad's focus, for `dim` seconds ([`crate::settings::Setting::DimAfter`],
    /// which the screens dim after without input, so that both dim together when nothing is touched;
    /// none is never). Windows moved there are never dimmed,
    /// since they are watched without touching. This spares the OLED while a game played with the pad on
    /// the main screen keeps the device awake and nobody looks below. A touch on it, or the pad's focus
    /// coming to it, brings it back.
    pub(crate) fn dimmed_at(&self, session: &Session, dim: Option<u64>, now: u64) -> bool {
        !self.shows_windows(session) && dim.is_some_and(|dim| self.idle_for(now) >= dim)
    }

    /// Seconds since the second screen was last used at `now`, or 0 before the clock's first reading.
    fn idle_for(&self, now: u64) -> u64 {
        self.used_at.map_or(0, |at| now.saturating_sub(at))
    }

    /// The second screen is in use, so it is lit. Before the clock's first reading it counts from that
    /// (see [`Bottom::note_clock`]): a time of 0 would be long past by then.
    pub(crate) fn used(&mut self, clock: &Clock) {
        if clock.read() {
            self.used_at = Some(clock.mono());
        }
    }

    /// The first clock reading: the time it goes unused counts from it.
    pub(crate) fn note_clock(&mut self, clock: &Clock) {
        if self.used_at.is_none() && clock.read() {
            self.used_at = Some(clock.mono());
        }
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

    /// Whether the bottom screen is off at `now`: by hand, or by itself as [`Bottom::idle_wanted`] says with
    /// `p`. A touch, the pad's focus or a window moved down turn it on again.
    pub(crate) fn off_at(&self, p: Power, now: u64) -> bool {
        self.screen.off || self.idle_wanted(p, now)
    }

    /// Whether the bottom screen should be off by itself at `now`. That needs all of: no refresh
    /// calibration running, ClamShell's view on it rather than a window moved down, and nothing being typed
    /// there. On top of that, the screen must be dark for a game and not tapped since, or untouched and
    /// without the pad's focus for the screens-off time, as the screens go off after it without input. The
    /// panel is driven in video mode and streamed every frame whatever it shows, so only turning it off
    /// saves power.
    fn idle_wanted(&self, p: Power, now: u64) -> bool {
        let untouched = p.screen_off.is_some_and(|off| self.idle_for(now) >= off);
        !p.refreshing && !self.shows(p.any_down) && self.editor().is_none() && (self.dark(p.in_game, p.in_game_setting) || untouched)
    }

    /// The darkened second screen was tapped awake. This lasts until the next game.
    pub(crate) fn wake(&mut self) {
        self.woken = true;
    }

    /// A game started, so the screen may go dark again.
    pub(crate) fn unwake(&mut self) {
        self.woken = false;
    }

    /// Opens the windows' tab as a window moves there.
    fn open_windows_tab(&mut self) {
        self.choose_tab(super::Tab::Windows);
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

    /// A critical notification came at `now`: the screen turns on if it was off by hand or by itself, and
    /// lights if it was dark for a game, as if tapped.
    pub(crate) fn light(&mut self, clock: &Clock) {
        self.screen.off = false;
        self.used(clock);
        self.woken = true;
    }

    /// Home+Start turns the bottom screen off, or on again. If it had turned off by itself, this turns it
    /// on, as a touch would.
    pub(crate) fn toggle_power(&mut self, p: Power, now: u64) {
        if !self.screen.off && self.idle_wanted(p, now) {
            self.used_at = Some(now);
            self.woken = true;
            return;
        }
        self.screen.off = !self.screen.off;
        // Turned on again by hand, so it is in use and shouldn't turn off by itself either.
        if !self.screen.off {
            self.used_at = Some(now);
            self.woken = true;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests;
