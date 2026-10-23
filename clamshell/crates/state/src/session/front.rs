//! What is in front: the Home button, and what takes a closed window's place, which is what was in front
//! before it.

use crate::out::Out;
use crate::{EntryAct, Event, Instance, Session, WindowCommand};

impl Session {
    /// Runs after every event. If the window in front when the last pass ran (see
    /// [`Session::followed`]) closed and nothing of its workspace is left, goes back to what was in front
    /// before it, with shelf as the last resort; otherwise sway would leave the empty workspace in front. The workspace is remembered in case a
    /// window appears on it soon after (see [`Session::follow_replacement`]).
    pub(crate) fn follow_front(&mut self, now: u64, out: &mut Out) {
        let windows = &self.windows;
        let Some(was) = &self.followed_front else { return };
        if windows.iter().any(|w| !w.down && (w.focused || w.workspace == *was)) {
            return;
        }
        let shown = |ws: &String| windows.iter().any(|w| w.workspace == *ws && !w.down);
        let group = self.recent.iter().find(|ws| shown(ws)).filter(|ws| !Instance::Shelf.is(ws)).cloned();
        let was = was.clone();
        let closed = !windows.iter().any(|w| w.workspace == was);
        match group {
            Some(group) => self.focus(&group, out),
            None => self.command(WindowCommand::ShowShelf, out),
        }
        self.closed_front = closed.then_some((was, now));
    }

    /// Whether shelf is in front on the main screen, as it is before the compositor reports a window.
    pub(crate) fn shelf_in_front(&self) -> bool {
        self.top_window().is_none_or(|w| w.is_shelf())
    }

    /// The Home button always goes to shelf. (When the Device view is in shelf's place, Home closes it instead: see
    /// [`crate::device::Device::close`].)
    pub(crate) fn home(&mut self, out: &mut Out) {
        self.cancel_front_launch();
        if !self.shelf_in_front() {
            self.command(WindowCommand::ShowShelf, out);
        }
    }

    /// Home with B, released at once, asks that window (see [`Session::home_b_window`]) to close.
    pub(crate) fn close_front_window(&mut self, shows_windows: bool, out: &mut Out) {
        if let Some(w) = self.home_b_window(shows_windows) {
            self.command(WindowCommand::Close(vec![w.id]), out);
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
            None => self.command(WindowCommand::Close(vec![w.id]), out),
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
        s.observed(vec![shelf_window(false), win(7, "app", "app", true)]);
        assert_eq!(s.applied(Event::Home), vec![Effect::Window(WindowCommand::ShowShelf), Effect::PadRoute(crate::PadRoute::Ui)]);
        assert!(s.session().top_window().is_some_and(Window::is_shelf));
        assert!(s.applied(Event::Home).is_empty());
    }

    #[test]
    fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
        let mut s = state();
        let (zelda, terminal) = ("clamshell-run-zelda-1".to_owned(), "clamshell-run-foot-2".to_owned());
        // Shelf, then the terminal, then the game in front.
        s.observed(vec![shelf_window(true)]);
        s.observed(vec![shelf_window(false), win(2, "foot", &terminal, true)]);
        s.observed(vec![shelf_window(false), win(2, "foot", &terminal, false), win(3, "retroarch", &zelda, true)]);
        // When the game quits, the terminal, which was in front before it, comes back.
        let fx = s.observed(vec![shelf_window(false), win(2, "foot", &terminal, false)]);
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::FocusWindow(2))]);
        s.observed(vec![shelf_window(false), win(2, "foot", &terminal, true)]);
        // When the terminal quits, shelf comes back.
        let fx = s.observed(vec![shelf_window(false)]);
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
        // Nothing happens when a program's own second window closes, or for a new workspace before its
        // window.
        s.observed(vec![shelf_window(false), win(3, "retroarch", &zelda, true), win(4, "retroarch", &zelda, false)]);
        assert_eq!(
            window_effects(s.observed(vec![shelf_window(false), win(4, "retroarch", &zelda, true)])),
            vec![Effect::Window(WindowCommand::FocusWindow(4))]
        );
        s.observed(vec![shelf_window(true), win(4, "retroarch", &zelda, false)]);
        assert_eq!(window_effects(s.observed(vec![shelf_window(false), win(4, "retroarch", &zelda, false)])), vec![], "shelf's window is still there");
    }

    #[test]
    fn a_window_in_front_replaced_at_once_is_shown_again() {
        let mut s = state();
        let app = "clamshell-run-fdroid-1".to_owned();
        s.applied(Event::tick(100));
        s.observed(vec![shelf_window(false), win(2, "waydroid.org.fdroid.fdroid", &app, true)]);
        // Its window closes and shelf shows. Another window comes on its workspace, moved there and not in
        // front, and the workspace is shown again.
        assert_eq!(window_effects(s.observed(vec![shelf_window(false)])), vec![Effect::Window(WindowCommand::ShowShelf)]);
        let fx = s.observed(vec![shelf_window(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]);
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::FocusWindow(3))]);
        // Later than that, it isn't.
        s.observed(vec![shelf_window(false), win(3, "waydroid.org.fdroid.fdroid", &app, true)]);
        s.observed(vec![shelf_window(false)]);
        s.applied(Event::tick(103));
        let fx = s.observed(vec![shelf_window(true), win(4, "waydroid.org.fdroid.fdroid", &app, false)]);
        assert_eq!(window_effects(fx), vec![], "too late: it stays behind");
    }

    #[test]
    fn home_doesnt_go_back_to_a_workspace_whose_window_moved_down() {
        let mut s = state();
        let terminal = "clamshell-run-foot_1";
        s.observed(vec![shelf_window(false), win(2, "foot", terminal, true)]);
        s.applied(Event::Device(crate::device::buttons::DeviceKey::WindowDown));
        // With its window moved to the bottom screen and shelf in front, there is nothing to go back to on
        // the main screen.
        s.observed(vec![shelf_window(true), Window { down: true, ..win(2, "foot", terminal, false) }]);
        assert_eq!(s.applied(Event::Home), vec![]);
    }

    #[test]
    fn home_stays_on_shelf_with_an_unowned_program_behind_it() {
        let mut s = state();
        s.observed(vec![shelf_window(false), foot()]);
        s.observed(vec![shelf_window(true), Window { focused: false, ..foot() }]);
        assert!(s.applied(Event::Home).is_empty());
    }
}
