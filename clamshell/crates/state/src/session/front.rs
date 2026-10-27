//! What is in front: the Home button, and what takes a closed window's place, which is what was in front
//! before it, until the program of the closed window shows another.

use crate::Compositor;
use crate::out::Out;
use crate::session::InFront;
use crate::session::windows::{Group, WindowCommand};
use crate::{Effect, EntryAct, Event, LaunchUnit, Session};

/// How long a program whose window in front closed, still running, is waited for to show another (see
/// [`Session::follow_front`]): Steam replacing its update dialog with its window takes a while.
pub(crate) const REWAIT_SECS: u64 = 30;

impl Session {
    /// Runs after every event. If the window in front when the last pass ran (see
    /// [`Session::followed`]) closed and nothing of its group is left, goes back to what was in front
    /// before it: a group still shown, or shelf, if shelf was in front since, as when the program was
    /// launched from it; shelf is the last resort. The group is remembered in case a window appears in it
    /// soon after (see [`Session::follow_replacement`]). If its program still runs, its next window is
    /// shown as a launch's first is, for [`REWAIT_SECS`].
    pub(crate) fn follow_front(&mut self, now: u64) {
        let windows = &self.windows;
        let Some(was) = &self.followed_front else { return };
        if self.top.is_some() || windows.iter().any(|w| !self.below(w.id) && w.group.as_ref() == Some(was)) {
            return;
        }
        let was = was.clone();
        let closed = !windows.iter().any(|w| w.group.as_ref() == Some(&was));
        self.go_back();
        if let Some(unit) = closed.then(|| self.still_running(&was)).flatten() {
            self.set_front_launch(unit, Some(now + REWAIT_SECS));
        }
        self.closed_front = closed.then_some((was, now));
    }

    /// Shows what was in front before: the most recent group still shown, or shelf if shelf was in front
    /// since; shelf is the last resort.
    pub(crate) fn go_back(&mut self) {
        let shown = |group: &Group| self.windows.iter().any(|w| w.group.as_ref() == Some(group) && !self.below(w.id));
        let back = self.recent.iter().find(|was| match was {
            InFront::Shelf => true,
            InFront::Group(group) => shown(group),
        });
        match back.cloned() {
            Some(InFront::Group(group)) => self.focus(&group),
            Some(InFront::Shelf) | None => self.command(WindowCommand::ShowShelf),
        }
    }

    /// The launch to wait for again in `group`, if its entry still runs: its unit, or for an entry known
    /// by app id or process (none of whose windows need be in its unit), the entry.
    fn still_running(&self, group: &Group) -> Option<LaunchUnit> {
        let key = group.key()?;
        let running = self.running.iter().find(|r| r.key == key)?;
        let unit = self.units.iter().find(|unit| unit.key == key).cloned();
        unit.or_else(|| running.outside().then(|| LaunchUnit { key: key.to_owned(), secs: running.start, n: 0 }))
    }

    /// Whether shelf is in front on the main screen, as it is before the compositor reports a window.
    pub(crate) fn shelf_in_front(&self) -> bool {
        self.top_window().is_none()
    }

    /// The Home button always goes to shelf. (When the Device view is in shelf's place, Home closes it instead: see
    /// [`crate::setup::Setup::close`].)
    pub(crate) fn home(&mut self) {
        self.cancel_front_launch();
        if !self.shelf_in_front() {
            self.command(WindowCommand::ShowShelf);
        }
    }

    /// Home with B, released at once, asks that window (see [`Session::home_b_window`]) to close.
    pub(crate) fn close_front_window(&mut self, shows_windows: bool, out: &mut Out) {
        if let Some(w) = self.home_b_window(shows_windows) {
            out.effects.push(Effect::Compositor(Compositor::Close(vec![w.id])));
        }
    }

    /// Home with B, held, quits that window's entry, as Quit in its menu would. Any other window is just
    /// closed.
    pub(crate) fn quit_front(&mut self, shows_windows: bool, out: &mut Out) {
        let Some(w) = self.home_b_window(shows_windows) else {
            return;
        };
        match self.running_of(w) {
            Some(r) => out.events.push(Event::Entry(r.id.clone(), EntryAct::Quit)),
            None => out.effects.push(Effect::Compositor(Compositor::Close(vec![w.id]))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn home_always_shows_shelf_and_repeated_home_stays_there() {
        let mut s = state();
        assert!(s.applied(Event::Home).is_empty());
        s.observed(vec![win(7, "app")]);
        assert_eq!(s.applied(Event::Home), vec![Effect::PadRoute(crate::PadRoute::Ui)]);
        assert!(s.session().shelf_in_front());
        assert!(s.applied(Event::Home).is_empty());
    }

    /// A game launched from shelf goes back to shelf when it quits, not to the program in front before,
    /// such as the launcher that ran it; a game started from the launcher goes back to the launcher.
    #[test]
    fn a_closed_window_goes_back_to_shelf_if_shelf_was_in_front_before_it() {
        let mut s = state();
        let (launcher, game) = (launch_of("heroic"), launch_of("zelda"));
        s.observed(vec![win_of(2, "heroic", &launcher)]);
        s.applied(WindowEvent::Activate(2).into());
        s.applied(Event::Home);
        s.observed(vec![win_of(2, "heroic", &launcher), win_of(3, "zelda", &game)]);
        s.applied(WindowEvent::Activate(3).into());
        s.observed(vec![win_of(2, "heroic", &launcher)]);
        assert!(s.session().shelf_in_front(), "where it was launched from");
        // Started from the launcher, it goes back there.
        s.applied(WindowEvent::Activate(2).into());
        s.observed(vec![win_of(2, "heroic", &launcher), win_of(3, "zelda", &game)]);
        s.applied(WindowEvent::Activate(3).into());
        s.observed(vec![win_of(2, "heroic", &launcher)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(2));
    }

    #[test]
    fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
        let mut s = state();
        let (zelda, terminal) = (launch_of("zelda"), launch_of("foot"));
        // Shelf, then the terminal, then the game in front.
        s.observed(vec![win_of(2, "foot", &terminal)]);
        s.applied(WindowEvent::Activate(2).into());
        s.observed(vec![win_of(2, "foot", &terminal), win_of(3, "retroarch", &zelda)]);
        s.applied(WindowEvent::Activate(3).into());
        // When the game quits, the terminal, which was in front before it, comes back.
        s.observed(vec![win_of(2, "foot", &terminal)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(2));
        // When the terminal quits, shelf comes back.
        s.observed(vec![]);
        assert!(s.session().shelf_in_front());
        // When a program's window closes, its other window comes in front.
        s.observed(vec![win_of(3, "retroarch", &zelda), win_of(4, "retroarch", &zelda)]);
        s.applied(WindowEvent::Activate(3).into());
        s.observed(vec![win_of(4, "retroarch", &zelda)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(4));
    }

    #[test]
    fn a_window_in_front_replaced_at_once_is_shown_again() {
        let mut s = state();
        let app = launch_of("fdroid");
        s.applied(Event::tick(100));
        s.observed(vec![win_of(2, "waydroid.org.fdroid.fdroid", &app)]);
        s.applied(WindowEvent::Activate(2).into());
        // Its window closes and shelf shows. Another window comes in its group, not in front, and the
        // group is shown again.
        s.observed(vec![]);
        assert!(s.session().shelf_in_front());
        s.observed(vec![win_of(3, "waydroid.org.fdroid.fdroid", &app)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(3));
        // Later than that, it isn't.
        s.observed(vec![]);
        s.applied(Event::tick(103));
        s.observed(vec![win_of(4, "waydroid.org.fdroid.fdroid", &app)]);
        assert!(s.session().shelf_in_front(), "too late: it stays behind");
    }

    /// A program still running whose window in front closed, as Steam's update dialog does before its
    /// window comes, is waited for: its next window is shown though it comes later than a replacement
    /// would, and another program's window opening meanwhile stays behind. After [`REWAIT_SECS`] it isn't.
    #[test]
    fn a_running_programs_next_window_is_shown_after_its_window_in_front_closed() {
        let mut s = state();
        s.applied(Event::tick(1000));
        let zelda = launch(&mut s, "zelda-lttp");
        let window = |id| win_of(id, "zelda3", &zelda);
        s.observed(vec![window(3)]);
        s.observed(vec![]);
        assert!(s.session().shelf_in_front());
        s.applied(Event::tick(1010));
        assert!(!s.session().opening_focus(&foot()), "another program's window opens behind");
        s.observed(vec![window(4)]);
        assert_eq!(s.session().top_window().map(|w| w.id), Some(4));
        // Closed again, and its next window comes too late.
        s.observed(vec![]);
        s.applied(Event::tick(1011 + super::REWAIT_SECS));
        s.observed(vec![window(5)]);
        assert!(s.session().shelf_in_front(), "too late: it stays behind");
    }

    #[test]
    fn home_doesnt_go_back_to_a_group_whose_window_moved_down() {
        let mut s = state();
        s.observed(vec![win_of(2, "foot", &launch_of("foot_1"))]);
        s.applied(WindowEvent::Activate(2).into());
        // With its window moved to the bottom screen and shelf in front, there is nothing to go back to on
        // the main screen.
        s.applied(Event::HardwareKey(crate::HardwareKey::WindowDown));
        assert!(s.session().shelf_in_front());
        assert_eq!(s.applied(Event::Home), vec![]);
    }

    #[test]
    fn home_stays_on_shelf_with_an_unowned_program_behind_it() {
        let mut s = state();
        s.observed(vec![foot()]);
        s.applied(Event::Home);
        assert!(s.applied(Event::Home).is_empty());
        assert!(s.session().shelf_in_front());
    }
}
