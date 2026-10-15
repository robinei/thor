//! Where the controller's input goes. Something claims the pad by what is in front: shelf, the game, or
//! typing into an app while the keyboard tab is open; the pad mouse, while on, claims it over all of
//! them. The second screen gets it when nothing claims it, and View moves it there (and back) from the
//! pad mouse or typing; a game keeps it (View is the game's button there). When the claimant changes,
//! the pad goes to the new one, except that a pad in use on the second screen (its focus ring showing)
//! with nothing claiming it before stays there (the keyboard coming up under it is typed on with the
//! pad).
//!
//! The pad mouse is the user's switch, or follows the game in front (its Setting::PadMouse): turned on
//! for a game that wants it, off again when another (or none) is in front, unless the user switched it
//! meanwhile.

use crate::game::Front;
use crate::tab::Tab;

/// Where the controller's input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadRoute {
    /// The game's virtual pad.
    #[default]
    Game,
    /// ClamShell's own screens, as [`crate::Event::Pad`]: shelf's and the second screen, or the second
    /// screen alone.
    Ui,
    /// Typing into the app in front (D-pad arrows, A Enter, B Backspace): the keyboard is open.
    Keys,
    /// The pad mouse.
    Mouse,
    /// The pad's inputs as themselves, for choosing them ([`crate::Event::Listened`]): a keymap's edit
    /// screen is open where the pad would drive the second screen.
    Listen,
}

/// The controller driving ClamShell's screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadInput {
    /// The D-pad or left stick (repeated while held).
    Up,
    Down,
    Left,
    Right,
    /// A pressed or released: activates the focused target (holds act as for touch).
    A(bool),
    /// B: cancels the editor.
    B,
    /// Y: the selected game in or out of the favourites.
    Y,
    /// The shoulder buttons: the previous or next collection (shelf) or tab (the second screen).
    Shoulder(i32),
    /// View/Select: focus to the other screen.
    Switch,
    /// The Back button (the keymaps' layer key in a game): back one screen (see
    /// [`crate::State::pad_back`]).
    Back,
}

/// The pad mouse: off, on for the game in front, or switched on by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadMouse {
    #[default]
    Off,
    ByGame,
    ByUser,
}

/// What claims the pad besides the second screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claimant {
    Shelf,
    Game,
    /// Typing into the app in front (the keyboard tab is open).
    Typing,
    Mouse,
}

#[derive(Debug, Clone, Default)]
pub struct Pad {
    route: PadRoute,
    mouse: PadMouse,
    /// View moved the pad to the second screen, away from what claims it (the pad mouse, typing).
    moved_to_ui: bool,
    /// What claimed the pad when last reconciled.
    last_claimant: Option<Claimant>,
}

impl Pad {
    pub fn route(&self) -> PadRoute {
        self.route
    }

    pub fn mouse(&self) -> PadMouse {
        self.mouse
    }

    /// Whether the pad mouse is on.
    pub fn mouse_on(&self) -> bool {
        self.mouse != PadMouse::Off
    }

    /// Whether the pad, on ClamShell's screens, may go to shelf's (shelf is in front and nothing else
    /// has the pad), or only to the second screen.
    pub fn on_top(&self) -> bool {
        self.last_claimant == Some(Claimant::Shelf)
    }

    fn claimant(&self, front: Front, tab: Tab) -> Option<Claimant> {
        if self.mouse_on() {
            return Some(Claimant::Mouse);
        }
        match (front, tab) {
            (Front::Shelf, _) => Some(Claimant::Shelf),
            (Front::Game, _) => Some(Claimant::Game),
            (Front::App, Tab::Keyboard) => Some(Claimant::Typing),
            (Front::App, _) => None,
        }
    }

    /// Where the pad goes: to what claims it, unless View moved it to the second screen; to the second
    /// screen when nothing claims it. A game keeps it.
    fn wanted_route(&self) -> PadRoute {
        match self.last_claimant {
            None | Some(Claimant::Shelf) => PadRoute::Ui,
            Some(Claimant::Game) => PadRoute::Game,
            Some(_) if self.moved_to_ui => PadRoute::Ui,
            Some(Claimant::Typing) => PadRoute::Keys,
            Some(Claimant::Mouse) => PadRoute::Mouse,
        }
    }

    /// After anything changed: the claimant as things are now (what is in front, the tab, whether the
    /// focus ring shows), and where the pad goes; the new route, if it changed.
    pub(crate) fn reconcile(&mut self, front: Front, tab: Tab, focus_shown: bool, listening: bool) -> Option<PadRoute> {
        let claimant = self.claimant(front, tab);
        if claimant != self.last_claimant {
            let in_use = self.route == PadRoute::Ui && focus_shown;
            self.moved_to_ui = self.last_claimant.is_none() && in_use && claimant != Some(Claimant::Mouse);
            self.last_claimant = claimant;
        }
        let route = match self.wanted_route() {
            PadRoute::Ui if listening => PadRoute::Listen,
            route => route,
        };
        (route != self.route).then(|| {
            self.route = route;
            route
        })
    }

    /// View: the pad between the second screen and the pad mouse or typing (not shelf or a game).
    pub(crate) fn switch(&mut self) {
        if matches!(self.last_claimant, Some(Claimant::Mouse | Claimant::Typing)) {
            self.moved_to_ui = !self.moved_to_ui;
        }
    }

    /// The user's switch.
    pub(crate) fn toggle_mouse(&mut self) {
        self.mouse = if self.mouse_on() { PadMouse::Off } else { PadMouse::ByUser };
    }

    /// The game in front changed: the pad mouse on if it wants it, off again if the game had turned it
    /// on and doesn't. Whether it changed.
    pub(crate) fn follow_game(&mut self, wanted: bool) -> bool {
        match (wanted, self.mouse) {
            (true, PadMouse::Off) => self.mouse = PadMouse::ByGame,
            (false, PadMouse::ByGame) => self.mouse = PadMouse::Off,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    fn route(s: &mut State, event: Event) -> PadRoute {
        s.apply(event);
        s.pad().route()
    }

    #[test]
    fn the_pad_mouse_follows_the_game_in_front_unless_switched_by_hand() {
        let mut s = state();
        tap(&mut s, game("zelda-lttp"));
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Game)));
        tap(&mut s, Id::Settings(SettingsId::Tile(Setting::PadMouse)));
        s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert_eq!(s.pad().mouse(), PadMouse::ByGame, "the game wants it");
        // Off and on again by hand: the user's now, it stays when the game ends.
        tap(&mut s, Id::PadMouse);
        tap(&mut s, Id::PadMouse);
        played(&mut s, "zelda-lttp");
        assert_eq!(s.pad().mouse(), PadMouse::ByUser);
    }

    #[test]
    fn pad_mouse_toggles_independently_of_the_tab() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(tap(&mut s, Id::PadMouse), vec![Effect::PadRoute(PadRoute::Mouse)]);
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(tap(&mut s, Id::PadMouse), vec![Effect::PadRoute(PadRoute::Ui)]);
    }

    #[test]
    fn the_pad_goes_to_shelf_the_game_or_typing_by_what_is_in_front() {
        let mut s = state();
        let window = |app_id: &str, unit: Option<&str>| Window { unit: unit.map(Into::into), workspace: String::new(), ..win(1, app_id, "", true) };
        assert_eq!(route(&mut s, Event::Tick { unix_secs: 1 }), PadRoute::Ui, "nothing known, no game: shelf");
        assert_eq!(route(&mut s, GameEvent::Started("zelda-lttp".into()).into()), PadRoute::Game, "a game runs: in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window(apps::SHELF_APP_ID, None)])), PadRoute::Ui, "shelf in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window("foot", Some("clamshell-app-terminal-1-0.scope"))])), PadRoute::Ui, "an app: the second screen");
        assert_eq!(route(&mut s, PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into()), PadRoute::Keys, "an app and the keyboard: typing");
        assert_eq!(route(&mut s, Event::Windows(vec![window("retroarch", Some("clamshell-game-1-0.scope"))])), PadRoute::Game);
        let (_, fx) = s.apply(Event::Windows(vec![window(apps::SHELF_APP_ID, None)]));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
    }

    #[test]
    fn view_moves_the_pad_between_the_second_screen_and_what_wants_it() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.pad().route(), PadRoute::Keys);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Ui)], "to the keys on screen");
        assert!(!s.pad_on_top());
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Keys)], "and back to typing");
        // The pad mouse takes it; View moves it to the second screen and back.
        assert_eq!(tap(&mut s, Id::PadMouse), vec![Effect::PadRoute(PadRoute::Mouse)]);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Mouse)]);
        // Nothing else wants it: View does nothing, the pad stays on the second screen.
        tap(&mut s, Id::PadMouse);
        tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(s.pad().route(), PadRoute::Ui);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![]);
        // Used there, it stays when the keyboard comes up under it.
        s.apply(PointerEvent::Focus(Id::Tab(Tab::Dock)).into());
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.pad().route(), PadRoute::Ui);
    }

    #[test]
    fn the_focus_ring_hides_when_the_pad_leaves_the_screens() {
        let mut s = state();
        s.apply(PointerEvent::Focus(game("zelda-lttp")).into());
        assert!(s.pointer().show_focus());
        let (dirty, _) = s.apply(GameEvent::Started("zelda-lttp".into()).into());
        assert_eq!((dirty, s.pointer().show_focus()), (Dirty::BOTH, false));
    }
}
