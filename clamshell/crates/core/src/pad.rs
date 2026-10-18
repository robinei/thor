//! Where the controller's input goes. Something claims the pad by what is in front: shelf, or the program
//! (a game or an app, alike: as a game's pad, through its keymap), or typing into an app while the
//! keyboard tab is open; the pad mouse, while on, claims it over all of them. The second screen gets it when nothing claims it, and View moves it there (and back) from the
//! pad mouse or typing. A game keeps it (View is the game's button there): only a switch from outside
//! the pad (`clamshell ctl pad switch`) moves it to the second screen, and
//! View there back. While a dock entry is being moved, the second screen has it (the D-pad moves it). When the claimant changes,
//! the pad goes to the new one, except that a pad in use on the second screen (its focus ring showing)
//! with nothing claiming it before stays there (the keyboard coming up under it is typed on with the
//! pad).
//!
//! The pad mouse is the user's switch, or follows the program in force (its Setting::PadMouse): turned
//! on for a program that wants it, off again when another (or none) is in force, unless the user
//! switched it meanwhile.

use crate::running::Front;
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
    /// Y: the selected entry in or out of the favourites.
    Y,
    /// X: the selected entry pinned to the dock, or unpinned.
    X,
    /// Start: the selected folder's next video played (a series' episode).
    Start,
    /// The shoulder buttons (L1, R1): a page up or down (shelf), the previous or next tab (the second
    /// screen).
    Shoulder(i32),
    /// The triggers (L2, R2) pulled past half: to the first or last entry (shelf).
    Trigger(i32),
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
            (Front::Program | Front::Other, Tab::Keyboard) => Some(Claimant::Typing),
            (Front::Program | Front::Other, _) => Some(Claimant::Game),
        }
    }

    /// Where the pad goes: to what claims it, unless a switch moved it to the second screen; to the
    /// second screen when nothing claims it.
    fn wanted_route(&self) -> PadRoute {
        match self.last_claimant {
            None | Some(Claimant::Shelf) => PadRoute::Ui,
            Some(_) if self.moved_to_ui => PadRoute::Ui,
            Some(Claimant::Game) => PadRoute::Game,
            Some(Claimant::Typing) => PadRoute::Keys,
            Some(Claimant::Mouse) => PadRoute::Mouse,
        }
    }

    /// After anything changed: the claimant as things are now (what is in front, the tab, whether the
    /// focus ring shows), and where the pad goes; the new route, if it changed.
    pub(crate) fn reconcile(&mut self, front: Front, tab: Tab, focus_shown: bool, listening: bool, moving: bool) -> Option<PadRoute> {
        let claimant = self.claimant(front, tab);
        if claimant != self.last_claimant {
            let in_use = self.route == PadRoute::Ui && focus_shown;
            self.moved_to_ui = self.last_claimant.is_none() && in_use && claimant != Some(Claimant::Mouse);
            self.last_claimant = claimant;
        }
        let route = match self.wanted_route() {
            PadRoute::Ui if listening => PadRoute::Listen,
            // (A dock entry moving: by the D-pad.)
            _ if moving => PadRoute::Ui,
            route => route,
        };
        (route != self.route).then(|| {
            self.route = route;
            route
        })
    }

    /// A switch: the pad between the second screen and the pad mouse, typing or a game (not shelf). (A
    /// game's View is the game's: its switch comes from outside the pad.)
    pub(crate) fn switch(&mut self) {
        if matches!(self.last_claimant, Some(Claimant::Mouse | Claimant::Typing | Claimant::Game)) {
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
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Content)));
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
    fn the_pad_goes_to_shelf_the_program_or_typing_by_what_is_in_front() {
        let mut s = state();
        let window = |app_id: &str, unit: Option<&str>| Window { unit: unit.map(Into::into), workspace: String::new(), ..win(1, app_id, "", true) };
        assert_eq!(route(&mut s, Event::Tick { unix_secs: 1 }), PadRoute::Ui, "nothing known, no game: shelf");
        assert_eq!(route(&mut s, GameEvent::Started("zelda-lttp".into()).into()), PadRoute::Game, "a game runs: in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window(windows::SHELF_APP_ID, None)])), PadRoute::Ui, "shelf in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window("foot", Some("clamshell-run-foot_1-1-0.scope"))])), PadRoute::Game, "an app: its pad, as a game's");
        assert_eq!(route(&mut s, PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into()), PadRoute::Keys, "an app and the keyboard: typing");
        assert_eq!(route(&mut s, Event::Windows(vec![window("retroarch", Some("clamshell-run-zelda_1-1-0.scope"))])), PadRoute::Keys, "a game too");
        assert_eq!(route(&mut s, PointerEvent::Activate(Id::Tab(Tab::Dock)).into()), PadRoute::Game);
        let (_, fx) = s.apply(Event::Windows(vec![window(windows::SHELF_APP_ID, None)]));
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
        // The pad mouse off, another tab: the app in front has it, as a game would (View is its button).
        tap(&mut s, Id::PadMouse);
        tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(s.pad().route(), PadRoute::Game);
        // A switch from outside the pad (its View is the app's) moves it to the second screen and back.
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.apply(Event::PadSwitch).1, vec![Effect::PadRoute(PadRoute::Game)]);
        // Shelf in front, the pad used on the second screen: it stays when the keyboard comes up under it.
        s.apply(Event::Windows(vec![win(2, windows::SHELF_APP_ID, &Instance::Shelf.name(), true)]));
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

    #[test]
    fn moving_a_dock_entry_takes_the_pad_from_the_game() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(s.pad().route(), PadRoute::Game);
        let foot_entry = Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 0 };
        s.apply(Event::Units(vec![foot_entry]));
        long(&mut s, Id::Dock(crate::DockId::Entry(0)));
        let (_, fx) = s.apply(PointerEvent::Activate(Id::Dock(crate::DockId::Move)).into());
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)], "moving: the D-pad's");
        let (_, fx) = s.apply(PointerEvent::Activate(Id::Dock(crate::DockId::Done)).into());
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Game)], "done: the game's again");
    }
}
