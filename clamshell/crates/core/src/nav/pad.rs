//! Where the controller's input goes.
//!
//! What is in front claims the pad. That is shelf, or the program in front, or typing into an app while
//! the keyboard tab is open. A program gets the pad as a game's pad through its keymap, whether it is a
//! game or an app. The pad mouse, while on, claims the pad over all of them. The second screen gets the
//! pad when nothing claims it, and View moves it there and back from the pad mouse or typing.
//!
//! A game keeps the pad, since View is the game's own button there. Only a switch from outside the pad
//! (`clamshell ctl pad switch`) moves it to the second screen, and View there moves it back. While a
//! dock entry is being moved, the second screen has the pad, since the D-pad moves the entry.
//!
//! When the claimant changes, the pad goes to the new one, with one exception. If the pad is in use on
//! the second screen, with its focus ring showing, and nothing claimed it before, it stays there. This
//! way the keyboard that comes up under it can be typed on with the pad.
//!
//! The pad mouse is switched by the user, or follows the program in force (its Setting::PadMouse). It is
//! turned on for a program that wants it, and off again when another program or none is in force, unless
//! the user switched it in the meantime.

use crate::bottom::Tab;
use crate::session::running::Front;

/// Where the controller's input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadRoute {
    /// The game's virtual pad.
    #[default]
    Game,
    /// ClamShell's own screens, as [`crate::Event::Pad`]. Either shelf's screen and the second screen, or
    /// the second screen alone.
    Ui,
    /// Typing into the app in front while the keyboard is open. The D-pad gives arrows, A gives Enter and
    /// B gives Backspace.
    Keys,
    /// The pad mouse.
    Mouse,
    /// The pad's inputs as themselves, for choosing them ([`crate::Event::Listened`]). Used when a
    /// keymap's edit screen is open where the pad would otherwise drive the second screen.
    Listen,
}

/// The controller driving ClamShell's screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadInput {
    /// The D-pad or left stick, repeated while held.
    Up,
    Down,
    Left,
    Right,
    /// A pressed or released. Activates the focused target, and holds act as they do for touch.
    A(bool),
    /// B cancels the editor.
    B,
    /// Y adds the selected entry to the favourites or removes it.
    Y,
    /// X pins the selected entry to the dock or unpins it.
    X,
    /// Start plays the selected folder's next video, such as a series' next episode.
    Start,
    /// The shoulder buttons (L1, R1). They page up or down in shelf, and go to the previous or next tab
    /// on the second screen.
    Shoulder(i32),
    /// The triggers (L2, R2) pulled past half. They go to the first or last entry in shelf.
    Trigger(i32),
    /// View/Select moves focus to the other screen.
    Switch,
    /// The Back button goes back one screen (see [`crate::State::pad_back`]).
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
    /// Typing into the app in front, while the keyboard tab is open.
    Typing,
    Mouse,
}

#[derive(Debug, Clone, Default)]
pub struct Pad {
    route: PadRoute,
    mouse: PadMouse,
    /// View moved the pad to the second screen, away from what claims it, such as the pad mouse or
    /// typing.
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

    /// Whether the pad, while on ClamShell's screens, may go to shelf's screen or only to the second
    /// screen. It may go to shelf's when shelf is in front and nothing else has the pad.
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

    /// Where the pad goes. It goes to what claims it, unless a switch moved it to the second screen. When
    /// nothing claims it, it goes to the second screen.
    fn wanted_route(&self) -> PadRoute {
        match self.last_claimant {
            None | Some(Claimant::Shelf) => PadRoute::Ui,
            Some(_) if self.moved_to_ui => PadRoute::Ui,
            Some(Claimant::Game) => PadRoute::Game,
            Some(Claimant::Typing) => PadRoute::Keys,
            Some(Claimant::Mouse) => PadRoute::Mouse,
        }
    }

    /// Runs after anything changed. Works out the claimant from what is in front, the tab and whether the
    /// focus ring shows, and then where the pad goes. Returns the new route if it changed.
    pub(crate) fn reconcile(&mut self, front: Front, tab: Tab, focus_shown: bool, listening: bool, moving: bool) -> Option<PadRoute> {
        let claimant = self.claimant(front, tab);
        if claimant != self.last_claimant {
            let in_use = self.route == PadRoute::Ui && focus_shown;
            self.moved_to_ui = self.last_claimant.is_none() && in_use && claimant != Some(Claimant::Mouse);
            self.last_claimant = claimant;
        }
        let route = match self.wanted_route() {
            PadRoute::Ui if listening => PadRoute::Listen,
            // A dock entry is being moved with the D-pad.
            _ if moving => PadRoute::Ui,
            route => route,
        };
        (route != self.route).then(|| {
            self.route = route;
            route
        })
    }

    /// Switches the pad between the second screen and the pad mouse, typing or a game, but not shelf. A
    /// game's View belongs to the game, so for a game the switch comes from outside the pad.
    pub(crate) fn switch(&mut self) {
        if matches!(self.last_claimant, Some(Claimant::Mouse | Claimant::Typing | Claimant::Game)) {
            self.moved_to_ui = !self.moved_to_ui;
        }
    }

    /// The user switched the pad mouse.
    pub(crate) fn toggle_mouse(&mut self) {
        self.mouse = if self.mouse_on() { PadMouse::Off } else { PadMouse::ByUser };
    }

    /// The game in front changed. Turns the pad mouse on if the game wants it, and off again if the game
    /// had turned it on and no longer wants it. Returns whether it changed.
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
        s.applied(event);
        s.pad().route()
    }

    #[test]
    fn the_pad_mouse_follows_the_game_in_front_unless_switched_by_hand() {
        let mut s = state();
        tap(&mut s, game("zelda-lttp"));
        tap(&mut s, Id::Settings(SettingsId::Scope(Scope::Content)));
        tap(&mut s, Id::Settings(SettingsId::Tile(crate::settings::Setting::PadMouse)));
        crate::testkit::start(&mut s, "zelda-lttp");
        assert_eq!(s.pad().mouse(), PadMouse::ByGame, "the game wants it");
        // Turned off and on again by hand, it is the user's now, so it stays when the game ends.
        tap(&mut s, Id::PadMouse);
        tap(&mut s, Id::PadMouse);
        end(&mut s, "zelda-lttp");
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
        assert_eq!(route(&mut s, Event::tick(1)), PadRoute::Ui, "nothing known, no game: shelf");
        crate::testkit::start(&mut s, "zelda-lttp");
        assert_eq!(s.pad().route(), PadRoute::Game, "a game runs: in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window(crate::session::windows::SHELF_APP_ID, None)])), PadRoute::Ui, "shelf in front");
        assert_eq!(route(&mut s, Event::Windows(vec![window("foot", Some("clamshell-run-foot_1-1-0.scope"))])), PadRoute::Game, "an app: its pad, as a game's");
        assert_eq!(route(&mut s, PointerEvent::Activate(Id::Tab(Tab::Keyboard)).into()), PadRoute::Keys, "an app and the keyboard: typing");
        assert_eq!(route(&mut s, Event::Windows(vec![window("retroarch", Some("clamshell-run-zelda_1-1-0.scope"))])), PadRoute::Keys, "a game too");
        assert_eq!(route(&mut s, PointerEvent::Activate(Id::Tab(Tab::Dock)).into()), PadRoute::Game);
        let fx = s.applied(Event::Windows(vec![window(crate::session::windows::SHELF_APP_ID, None)]));
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Ui)]);
    }

    #[test]
    fn view_moves_the_pad_between_the_second_screen_and_what_wants_it() {
        let mut s = state();
        s.applied(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.pad().route(), PadRoute::Keys);
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Ui)], "to the keys on screen");
        assert!(!s.pad().on_top());
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Keys)], "and back to typing");
        // The pad mouse takes it, and View moves it to the second screen and back.
        assert_eq!(tap(&mut s, Id::PadMouse), vec![Effect::PadRoute(PadRoute::Mouse)]);
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Mouse)]);
        // With the pad mouse off and another tab open, the app in front has it, as a game would, and View
        // is its button.
        tap(&mut s, Id::PadMouse);
        tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(s.pad().route(), PadRoute::Game);
        // A switch from outside the pad moves it to the second screen and back, since View is the app's.
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Ui)]);
        assert_eq!(s.applied(Event::PadSwitch), vec![Effect::PadRoute(PadRoute::Game)]);
        // With shelf in front and the pad in use on the second screen, the pad stays there when the
        // keyboard comes up under it.
        s.applied(Event::Windows(vec![win(2, crate::session::windows::SHELF_APP_ID, &Instance::Shelf.name(), true)]));
        s.applied(PointerEvent::Focus(Id::Tab(Tab::Dock)).into());
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.pad().route(), PadRoute::Ui);
    }

    #[test]
    fn the_focus_ring_hides_when_the_pad_leaves_the_screens() {
        let mut s = state();
        s.applied(PointerEvent::Focus(game("zelda-lttp")).into());
        assert!(s.pointer().show_focus());
        crate::testkit::start(&mut s, "zelda-lttp");
        assert!(!s.pointer().show_focus());
    }

    #[test]
    fn moving_a_dock_entry_takes_the_pad_from_the_game() {
        let mut s = state();
        s.applied(Event::Windows(vec![foot()]));
        tap(&mut s, Id::Tab(Tab::Dock));
        assert_eq!(s.pad().route(), PadRoute::Game);
        let foot_entry = Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 0 };
        s.applied(Event::Units(vec![foot_entry]));
        long(&mut s, Id::Dock(crate::DockId::Entry(0)));
        let fx = s.applied(PointerEvent::Activate(Id::Dock(crate::DockId::Move)).into());
        assert_eq!(fx, vec![Effect::Watch(crate::effect::Watch::Battery, false), Effect::PadRoute(PadRoute::Ui)], "moving: the D-pad's");
        let fx = s.applied(PointerEvent::Activate(Id::Dock(crate::DockId::Done)).into());
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Game)], "done: the game's again");
    }
}
