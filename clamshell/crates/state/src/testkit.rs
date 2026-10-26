//! Shorthands for the tests.

pub use crate::session::windows::{Group, WindowFacts};
pub use crate::settings::Values;

use crate::*;

/// [`State::apply`] with a vector of its own for the effects, for tests.
pub trait Applied {
    /// Applies `event`, and returns the effects.
    fn applied(&mut self, event: Event) -> Vec<Effect>;
    /// Reports the windows as the compositor does, in one batch: the known windows not in `windows`
    /// close, the others open as given, and a known window's changed facts and title are reported.
    fn observed(&mut self, windows: Vec<WindowFacts>) -> Vec<Effect>;
}

impl Applied for State {
    fn observed(&mut self, windows: Vec<WindowFacts>) -> Vec<Effect> {
        let known = self.session().windows();
        let mut events: Vec<WindowEvent> = known.iter().filter(|w| !windows.iter().any(|n| n.id == w.id)).map(|w| WindowEvent::Closed(w.id)).collect();
        for window in windows {
            let Some(old) = known.iter().find(|w| w.id == window.id) else {
                events.push(WindowEvent::Opened(window));
                continue;
            };
            let title = (old.title != window.title).then(|| WindowEvent::Title { window: window.id, title: window.title.clone() });
            let facts = WindowFacts { title: old.title.clone(), ..window };
            if facts != old.facts {
                events.push(WindowEvent::Changed(facts));
            }
            events.extend(title);
        }
        self.applied(Event::WindowBatch(events))
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

/// A window as the compositor reports it, of no launch of ours.
pub fn win(id: i64, app_id: &str) -> WindowFacts {
    WindowFacts { id, app_id: app_id.into(), ..WindowFacts::default() }
}

/// A window of launch `instance`: in its unit, and so in its entry's group.
pub fn win_of(id: i64, app_id: &str, instance: &Instance) -> WindowFacts {
    WindowFacts { unit: instance.unit(), ..win(id, app_id) }
}

/// A launch of the entry with key `key`, which need not be in the library: its windows ([`win_of`]) are
/// in a group of that key.
pub fn launch_of(key: &str) -> Instance {
    Instance::Run { key: key.into(), secs: 1, n: 0 }
}

/// The group of `instance`'s entry, which holds its windows.
pub fn group_of(instance: &Instance) -> Group {
    Group::entry(instance.key().unwrap_or_default())
}

/// A running entry's first window opens and its client asks to show it.
pub fn show_window(s: &mut State, entry: &str, id: i64) -> Vec<Effect> {
    let key = clamshell_library::key_of(entry);
    let unit = s.session().units().iter().find(|u| u.key() == Some(&key)).and_then(Instance::unit);
    let mut fx = s.observed(vec![WindowFacts { unit, ..win(id, entry) }]);
    fx.extend(s.applied(WindowEvent::Activate(id).into()));
    fx
}

/// One of the user's terminals, not one of ours. Opened, it comes in front.
pub fn foot() -> WindowFacts {
    win(1001, "foot")
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

/// The notice shown, if one is.
pub fn notice(s: &State) -> Option<crate::bottom::notice::NoticeKind> {
    s.bottom().notice().map(|n| n.what.clone())
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
