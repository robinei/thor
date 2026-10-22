//! What is in front: the Home button, and a closed window's place taken by what was in front before it.

use crate::{Effect, Instance, Running, State, WindowCommand};

impl State {
    /// After the windows changed: the window in front (`was`, its workspace) closed, nothing of its
    /// workspace left (sway leaves the empty workspace in front): back to what was in front before it,
    /// shelf at the bottom of that; the workspace remembered for a window soon after (see
    /// [`crate::Session::show_reopened`]).
    pub(crate) fn follow_closed(&mut self, was: Option<String>) -> Vec<Effect> {
        let windows = &self.session.windows;
        let Some(was) = was else { return vec![] };
        if windows.iter().any(|w| w.focused || w.workspace == was) {
            return vec![];
        }
        let shown = |ws: &String| windows.iter().any(|w| w.workspace == *ws && !w.down);
        let effects = match self.session.recent.iter().find(|ws| shown(ws)) {
            Some(ws) if !Instance::Shelf.is(ws) => vec![Effect::Window(WindowCommand::Focus(ws.clone()))],
            _ => vec![Effect::Window(WindowCommand::ShowShelf)],
        };
        self.session.closed_front = Some((was, self.mono_secs));
        effects
    }

    /// The home button: from anything to shelf; from shelf back to what was in front before it, or
    /// without that history (no compositor's word) to the program in force.
    pub(crate) fn home(&mut self) -> Vec<Effect> {
        let session = &self.session;
        let front = session.windows.iter().find(|w| w.focused);
        let shelf_in_front = front.is_none_or(|w| w.app_id == crate::session::windows::SHELF_APP_ID);
        if !shelf_in_front {
            return vec![Effect::Window(WindowCommand::ShowShelf)];
        }
        // (The Device view in shelf's place: back to shelf.)
        if self.device.open {
            return self.close_device();
        }
        // (Only where a window shows on the main screen: one moved down leaves its workspace empty.)
        let shown = |ws: &String| session.windows.iter().any(|w| w.workspace == *ws && !w.down);
        let before = session.recent.iter().find(|ws| !Instance::Shelf.is(ws) && shown(ws)).cloned();
        let game = session.program.as_deref().and_then(|id| session.running(id)).map(Running::workspace).filter(shown);
        match before.or(game) {
            Some(workspace) => vec![Effect::Window(WindowCommand::Focus(workspace))],
            None => vec![],
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
        // The game quits: the terminal, in front before it.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(terminal.clone()))]);
        s.apply(Event::Windows(vec![shelf_window(false), win(2, "foot", &terminal, true)]));
        // The terminal quits: shelf.
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::ShowShelf)]);
        // A program's own second window closing, or a new workspace before its window: nothing.
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
        // Its window closes (shelf), another comes on its workspace (moved there, not in front): shown.
        assert_eq!(window_effects(s.apply(Event::Windows(vec![shelf_window(false)])).1), vec![Effect::Window(WindowCommand::ShowShelf)]);
        let (_, fx) = s.apply(Event::Windows(vec![shelf_window(true), win(3, "waydroid.org.fdroid.fdroid", &app, false)]));
        assert_eq!(window_effects(fx), vec![Effect::Window(WindowCommand::Focus(app.clone()))]);
        // Later: not.
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
        // Its window moved to the bottom screen, shelf in front: nothing to go back to on the main screen.
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
