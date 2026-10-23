//! Shorthands for the tests.

pub use crate::session::windows::Window;
pub use crate::settings::Values;

use crate::*;

/// [`State::apply`] with a vector of its own for the effects, for tests.
pub trait Applied {
    /// Applies `event`, and returns the effects.
    fn applied(&mut self, event: Event) -> Vec<Effect>;
    /// Reports a backend observation through the same event diff as the sway adapter.
    fn observed(&mut self, windows: Vec<Window>) -> Vec<Effect>;
}

impl Applied for State {
    fn observed(&mut self, windows: Vec<Window>) -> Vec<Effect> {
        self.applied(Event::window_changes(self.session().windows(), &windows))
    }
    fn applied(&mut self, event: Event) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.apply(event, &mut effects);
        effects
    }
}

/// Entry `id`'s launch starts, as systemd reports it: its scope is listed with the others. The scope is
/// the one the launch asked for, or else one of the entry's, as if an earlier ClamShell had started it.
pub fn start(s: &mut State, id: &str) -> Vec<Effect> {
    let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
    let launch = s.session().launching().iter().find(|l| &*l.entry == id).map(|l| l.instance.clone());
    let mut units = s.session().units().to_vec();
    units.push(launch.unwrap_or(Instance::Run { key, secs: s.clock().unix(), n: 99 }));
    s.applied(Event::Units(units))
}

/// Entry `id`'s launches end, as systemd reports it: its scopes are no longer listed.
pub fn end(s: &mut State, id: &str) -> Vec<Effect> {
    let key = s.entry(id).map_or_else(|| clamshell_library::key_of(id), |g| g.key.clone());
    let units = s.session().units().iter().filter(|u| u.key() != Some(key.as_str())).cloned().collect();
    s.applied(Event::Units(units))
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
    started(Init { library, ..Init::default() })
}

/// A test state from `init` that is up, as after the UI's Start, with its watches in force.
pub fn started(init: Init) -> State {
    let mut s = State::new(init);
    s.ui_up();
    s
}

pub fn win(id: i64, app_id: &str, workspace: &str, focused: bool) -> Window {
    Window { id, app_id: app_id.into(), title: String::new(), focused, visible: focused, unit: None, workspace: workspace.into(), ..Window::default() }
}

/// A running entry's first window becomes available and is shown by the backend.
pub fn show_window(s: &mut State, entry: &str, id: i64) -> Vec<Effect> {
    let key = clamshell_library::key_of(entry);
    let unit = s.session().units().iter().find(|u| u.key() == Some(&key)).and_then(Instance::unit);
    s.observed(vec![Window { unit, ..win(id, entry, &Instance::workspace_of(&key), true) }])
}

/// One of the user's terminals in front, not one of ours.
pub fn foot() -> Window {
    win(1001, "foot", "1", true)
}

/// A game in shelf's list.
pub fn entry(id: &str) -> Id {
    Id::Shelf(ShelfId::Entry(id.into()))
}

/// Taps `id` and returns the effects.
pub fn tap(s: &mut State, id: Id) -> Vec<Effect> {
    s.applied(PointerEvent::Activate(id).into())
}

pub fn long(s: &mut State, id: Id) -> Vec<Effect> {
    s.applied(PointerEvent::LongPress(id).into())
}

pub fn read(s: &mut State, reading: Reading) {
    s.applied(reading.into());
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

/// The effect that selects a window in its group.
pub fn focus(window: i64) -> Effect {
    Effect::Window(WindowCommand::FocusWindow(window))
}

/// Launches `entry` from shelf and has it start. Returns its instance.
pub fn launch(s: &mut State, id: &str) -> Instance {
    // Select it if it isn't selected, then play it. The launch is in the effects of one of the two taps.
    let mut fx = tap(s, entry(id));
    fx.extend(tap(s, entry(id)));
    let instance = fx.iter().find_map(|e| match e {
        Effect::Launch { instance, .. } => Some(instance.clone()),
        _ => None,
    });
    crate::testkit::start(s, id);
    instance.unwrap()
}

/// Quit on entry `id`, as from its dock menu, with what it does at once.
pub fn quit(s: &mut State, id: &str) -> Vec<Effect> {
    s.handle(Event::Entry(id.into(), EntryAct::Quit))
}

/// Force kill on entry `id`, with what it does.
pub fn force_kill(s: &mut State, id: &str) -> Vec<Effect> {
    s.handle(Event::Entry(id.into(), EntryAct::ForceKill))
}

/// Launches entry `id` again, as the dock's Open another does, with what that does at once.
pub fn launch_now(s: &mut State, id: &str) -> Vec<Effect> {
    let mut out = crate::out::Out::default();
    s.launch(id.into(), None, crate::session::running::Start::Front, &mut out);
    out.effects
}

/// Shows running entry `id`, as a tap on its dock tile does, with what that does at once.
pub fn show(s: &mut State, id: &str) -> Vec<Effect> {
    s.handle(Event::Entry(id.into(), EntryAct::Show))
}
