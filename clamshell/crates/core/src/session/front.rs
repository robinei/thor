//! What is in front: the Home button, and what takes a closed window's place, which is what was in front
//! before it.

use crate::out::Out;
use crate::{Effect, EntryAct, Event, Instance, Running, Session, WindowCommand};

impl Session {
    /// Runs after the windows changed. If the window in front when the last pass ran (see
    /// [`Session::followed`]) closed and nothing of its workspace is left, goes back to what was in front
    /// before it, with shelf as the last resort; otherwise sway would leave the empty workspace in front. The workspace is remembered in case a
    /// window appears on it soon after (see [`Session::show_reopened`]).
    pub(crate) fn follow_closed(&mut self, now: u64, out: &mut Out) {
        let windows = &self.windows;
        let Some(was) = &self.followed_front else { return };
        if windows.iter().any(|w| w.focused || w.workspace == *was) {
            return;
        }
        let shown = |ws: &String| windows.iter().any(|w| w.workspace == *ws && !w.down);
        out.effects.push(match self.recent.iter().find(|ws| shown(ws)) {
            Some(ws) if !Instance::Shelf.is(ws) => Effect::Window(WindowCommand::Focus(ws.clone())),
            _ => Effect::Window(WindowCommand::ShowShelf),
        });
        self.closed_front = Some((was.clone(), now));
    }

    /// Whether shelf is in front on the main screen, as it is before the compositor reports a window.
    pub(crate) fn shelf_in_front(&self) -> bool {
        self.windows.iter().find(|w| w.focused).is_none_or(|w| w.app_id == crate::session::windows::SHELF_APP_ID)
    }

    /// The Home button. From anything it goes to shelf. From shelf it goes back to what was in front
    /// before, or, without that history because the compositor hasn't reported any, to the program in
    /// force. (When the Device view is in shelf's place, Home closes it instead: see
    /// [`crate::device::Device::close`].)
    pub(crate) fn home(&self, out: &mut Out) {
        if !self.shelf_in_front() {
            out.effects.push(Effect::Window(WindowCommand::ShowShelf));
            return;
        }
        // Only workspaces with a window on the main screen count; a window moved down leaves its workspace
        // empty.
        let shown = |ws: &String| self.windows.iter().any(|w| w.workspace == *ws && !w.down);
        let before = self.recent.iter().find(|ws| !Instance::Shelf.is(ws) && shown(ws)).cloned();
        let game = self.program.as_deref().and_then(|id| self.running(id)).map(Running::workspace).filter(shown);
        if let Some(workspace) = before.or(game) {
            out.effects.push(Effect::Window(WindowCommand::Focus(workspace)));
        }
    }

    /// Home with B, released at once, asks that window (see [`Session::home_b_window`]) to close.
    pub(crate) fn close_front_window(&self, shows_windows: bool, out: &mut Out) {
        if let Some(w) = self.home_b_window(shows_windows) {
            out.effects.push(Effect::Window(WindowCommand::Close(vec![w.id])));
        }
    }

    /// Home with B, held, quits that window's entry, as Quit in its menu would. Any other window is just
    /// closed.
    pub(crate) fn quit_front(&self, shows_windows: bool, out: &mut Out) {
        let Some(w) = self.home_b_window(shows_windows) else {
            return;
        };
        match self.running_of(w) {
            Some(r) => out.events.push(Event::Entry(r.id.clone(), EntryAct::Quit)),
            None => out.effects.push(Effect::Window(WindowCommand::Close(vec![w.id]))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn home_goes_to_shelf_and_back_to_what_was_in_front() {
        let mut s = state();
        let zelda = Instance::workspace_of(&clamshell_library::key_of("zelda-lttp"));
        let home = |s: &mut State| s.apply(Event::Home).1;
        assert_eq!(home(&mut s), vec![], "nothing before shelf: it stays");
        crate::testkit::start(&mut s, "zelda-lttp");
        assert_eq!(home(&mut s), vec![], "the program in force, no window of it yet: shelf stays");
        s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda.clone()))], "no history: the program in force");
        // A terminal in front, then shelf: home goes back to the terminal.
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::ShowShelf)]);
        s.apply(Event::Windows(vec![shelf_window(true), win(2, "foot", terminal, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(terminal.into()))]);
        // The terminal closed: back to the game, which was in front before.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true)]));
        s.apply(Event::Windows(vec![shelf_window(true), win(3, "retroarch", &zelda, false)]));
        assert_eq!(home(&mut s), vec![Effect::Window(WindowCommand::Focus(zelda))]);
    }

    #[test]
    fn a_closed_window_goes_back_to_what_was_in_front_before_it_shelf_last() {
        let mut s = state();
        let (zelda, terminal) = ("clamshell-run-zelda-1".to_owned(), "clamshell-run-foot-2".to_owned());
        // Shelf, then the terminal, then the game in front.
        s.apply(Event::Windows(vec![shelf_window(true)]));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false), win(3, "retroarch", &zelda, true)]));
        // When the game quits, the terminal, which was in front before it, comes back.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(terminal.clone()))]);
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
        // When the terminal quits, shelf comes back.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
        // Nothing happens when a program's own second window closes, or for a new workspace before its
        // window.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "retroarch", &zelda, true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, true)])).1), vec![]);
        s.apply(Event::Windows(vec![shelf_window(true), win(4, "retroarch", &zelda, false)]));
        assert_eq!(
            window_effects(s.apply(Event::Windows(vec![shelf_window(false), win(4, "retroarch", &zelda, false)])).1),
            vec![],
            "shelf's window is still there"
        );
    }

    #[test]
    fn a_window_in_front_replaced_at_once_is_shown_again() {
        let mut s = state();
        let app = "clamshell-run-fdroid-1".to_owned();
        s.apply(Event::tick(100));
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "waydroid.org.fdroid.fdroid", &app, true)]));
        // Its window closes and shelf shows. Another window comes on its workspace, moved there and not in
        // front, and the workspace is shown again.
        assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false)])).1), vec![Effect::Window(WindowCommand::ShowShelf)]);
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(app.clone()))]);
        // Later than that, it isn't.
        s.apply(Event::Windows(vec![shelf_window(false), win(3, "waydroid.org.fdroid.fdroid", &app, true)]));
        s.apply(Event::Windows(vec![shelf_window(false)]));
        s.apply(Event::tick(103));
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(4, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window_effects(fx), vec![], "too late: it stays behind");
    }

    #[test]
    fn home_doesnt_go_back_to_a_workspace_whose_window_moved_down() {
        let mut s = state();
        let terminal = "clamshell-run-foot_1";
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", terminal, true)]));
        // With its window moved to the bottom screen and shelf in front, there is nothing to go back to on
        // the main screen.
        s.apply(Event::Windows(vec![shelf_window(true), Window { down: true, ..win(2, "foot", terminal, false) }]));
        assert_eq!(s.apply(Event::Home).1, vec![]);
    }

    #[test]
    fn home_goes_back_to_a_workspace_that_isnt_ours() {
        let mut s = state();
        s.apply(Event::Windows(vec![shelf_window(false), foot()]));
        s.apply(Event::Windows(vec![shelf_window(true), Window { focused: false, ..foot() }]));
        assert_eq!(s.apply(Event::Home).1, vec![Effect::Window(WindowCommand::Focus("1".into()))]);
    }
}
