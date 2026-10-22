//! Shorthands for the tests.

pub use crate::session::windows::Window;
pub use crate::settings::Values;

use crate::*;

/// Entry `id`'s launch starts, as systemd reports it: its scope is listed with the others. The scope is
/// the one the launch asked for, or else one of the entry's, as if an earlier ClamShell had started it.
pub fn start(s: &mut State, id: &str) -> (Dirty, Vec<Effect>) {
    let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
    let launch = s.session.launching.iter().find(|l| &*l.entry == id).map(|l| l.instance.clone());
    let mut units = s.session.units.clone();
    units.push(launch.unwrap_or(Instance::Run { key, secs: s.clock.unix(), n: 99 }));
    s.apply(Event::Units(units))
}

/// Entry `id`'s launches end, as systemd reports it: its scopes are no longer listed.
pub fn end(s: &mut State, id: &str) -> (Dirty, Vec<Effect>) {
    let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
    let units = s.session.units.iter().filter(|u| u.key() != Some(key.as_str())).cloned().collect();
    s.apply(Event::Units(units))
}

/// What a test state starts from: the demo library and nothing else.
pub fn init() -> Init {
    Init { library: clamshell_library::demo::library(), ..Init::default() }
}

/// A test state that is up, as after the UI's Start, with its watches in force.
pub fn state() -> State {
    state_with(init().library)
}

/// A test state with this library.
pub fn state_with(library: Library) -> State {
    let mut s = State::new(Init { library, ..Init::default() });
    s.ui_up = true;
    let _ = s.follow_watch();
    s
}

pub fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
    Window { id, app_id: app_id.into(), title: String::new(), focused, unit: None, workspace: workspace.into(), ..Window::default() }
}

/// One of the user's terminals in front, not one of ours.
pub fn foot() -> Window {
    win(1, "foot", "1", true)
}

/// A game in shelf's list.
pub fn game(id: &str) -> Id {
    Id::Shelf(ShelfId::Entry(id.into()))
}

/// Taps `id` and returns the effects.
pub fn tap(s: &mut State, id: Id) -> Vec<Effect> {
    s.apply(PointerEvent::Activate(id).into()).1
}

pub fn long(s: &mut State, id: Id) -> Vec<Effect> {
    s.apply(PointerEvent::LongPress(id).into()).1
}

pub fn read(s: &mut State, reading: Reading) -> Dirty {
    s.apply(reading.into()).0
}

/// Shelf's window, in front or not.
pub fn shelf_window(focused: bool) -> Window {
    win(1, crate::session::windows::SHELF_APP_ID, &Instance::Shelf.name(), focused)
}

/// The notice shown, if one is.
pub fn notice(s: &State) -> Option<crate::bottom::notice::NoticeKind> {
    s.bottom().notice().map(|n| n.what.clone())
}

/// The window commands among `fx`.
pub fn window_effects(fx: Vec<Effect>) -> Vec<Effect> {
    fx.into_iter().filter(|e| matches!(e, Effect::Window(_))).collect()
}

/// The effect that shows `instance`'s workspace.
pub fn focus(instance: &Instance) -> Effect {
    Effect::Window(WindowCommand::Focus(instance.workspace()))
}

/// Launches `game` from shelf and has it start. Returns its instance.
pub fn launch(s: &mut State, id: &str) -> Instance {
    // Select it if it isn't selected, then play it. The launch is in the effects of one of the two taps.
    let mut fx = tap(s, game(id));
    fx.extend(tap(s, game(id)));
    let instance = fx.iter().find_map(|e| match e {
        Effect::Launch { instance, .. } => Some(instance.clone()),
        _ => None,
    });
    crate::testkit::start(s, id);
    instance.unwrap()
}
