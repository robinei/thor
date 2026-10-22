//! Programs ClamShell didn't start, found by a window of theirs that is no entry's (or by a tray icon: see
//! [`crate::tray`]): anything with a window is on the dock. Each runs as an entry ([`Origin::Found`]): the
//! library's desktop entry named as the window's app id, else one of its own (`window:<app id>`, its picture
//! looked up by the app id in the icon themes, else the UI's app glyph); its windows (those with its app id,
//! or of its process) go to its workspace, shown there if one was in front.
//!
//! Its process (and those it started) is moved into a scope of ours, as a launch of the entry's
//! ([`State::adopt`]): then it is paused, quit and killed by its slice. Not a process that also has
//! another entry's windows (a browser showing a file opened from the Files tab): Quit closes its windows,
//! and that is all. Also nested in a desktop's session: a window in ClamShell's sway is that session's
//! (a desktop's own programs show theirs on the desktop, not there).
//!
//! A window a systemd service opened (a portal's file chooser, the password prompt) is none: it belongs
//! to the program that asked for it, and is gone in seconds.

use std::sync::Arc;

use clamshell_library::ids;

use crate::effect::{Effect, WindowCommand};
use crate::instance::Instance;
use crate::running::{Origin, Running};
use crate::windows::Window;
use crate::{Entry, State};

impl State {
    /// A found program's process (and those it started) moved into a scope of ours, as a launch of
    /// entry `key`'s; once systemd lists that, it runs as one (see [`State::follow_units`]).
    pub(crate) fn adopt(&mut self, key: &str, pid: u32) -> Effect {
        let (secs, n) = (self.unix_secs, self.session.next_instance());
        Effect::Adopt { instance: Instance::Run { key: key.to_owned(), secs, n }, pid }
    }

    /// Windows that are still no entry's (`taken`: ones given to a browsed file just now, left out):
    /// their programs run as entries found by them.
    pub(crate) fn follow_found_windows(&mut self, taken: &[i64]) -> Vec<Effect> {
        let now = self.mono_secs;
        let session = &self.session;
        // (Not a starting launch's: a Steam game found running, its launch to come.)
        let starting = |w: &Window| session.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        let service = |w: &Window| w.unit.as_deref().is_some_and(Instance::is_service);
        let mut new: Vec<Running> = Vec::new();
        for w in session.windows.iter().filter(|w| !taken.contains(&w.id) && w.home().is_none() && session.running_of(w).is_none()) {
            let Some(&pid) = w.pids.first().filter(|_| !starting(w) && !service(w)) else {
                continue;
            };
            if new.iter().any(|r| r.owns(w) || r.fathered(w)) {
                continue;
            }
            // Its process its own: no window of it another entry's.
            let own = !session.windows.iter().any(|o| o.pids.first() == Some(&pid) && session.running_of(o).is_some());
            let app_id: Option<Arc<str>> = (!w.app_id.is_empty()).then(|| w.app_id.as_str().into());
            // (Known by neither its app id nor its process: no entry would find it again.)
            if app_id.is_none() && !own {
                continue;
            }
            let game = self.found_entry(&w.app_id, || {
                let name = app_id.as_deref().map_or_else(|| pid.to_string(), str::to_owned);
                let title = if w.app_id.is_empty() { &w.title } else { &w.app_id };
                Entry { app: true, ..Entry::new(&format!("{}{name}", ids::WINDOW), title) }
            });
            if session.running(&game.id).is_some() {
                continue;
            }
            new.push(Running { app_id, seen: true, ..Running::of(&game, Origin::Found { pid: own.then_some(pid), tray: None }, now) });
        }
        self.start_found(new, taken)
    }

    /// The entry a program found by `name` (its window's app id, its tray icon's id) runs as: the
    /// library's desktop entry of that name (any case), else one of its own, made by `own`.
    pub(crate) fn found_entry(&self, name: &str, own: impl FnOnce() -> Entry) -> Entry {
        let named = |g: &&Entry| ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(name));
        self.shelf.library().entries.iter().find(named).cloned().unwrap_or_else(own)
    }

    /// Entries just found (by windows or tray icons) run: their windows (but `taken`) to their
    /// workspaces, shown there if one was in front; an own entry's picture looked up by its app id; each
    /// moved into a scope of ours.
    pub(crate) fn start_found(&mut self, new: Vec<Running>, taken: &[i64]) -> Vec<Effect> {
        let session = &self.session;
        let mut effects = Vec::new();
        for r in &new {
            let mine: Vec<&Window> = session.windows.iter().filter(|w| !taken.contains(&w.id) && (r.owns(w) || r.fathered(w))).collect();
            if !mine.is_empty() {
                effects.push(Effect::Window(WindowCommand::Move { windows: mine.iter().map(|w| w.id).collect(), to: r.workspace() }));
            }
            if mine.iter().any(|w| w.focused) {
                effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
            }
            if r.id.starts_with(ids::WINDOW)
                && let Some(app_id) = &r.app_id
            {
                effects.push(Effect::FindIcon { entry: r.id.clone(), app_id: app_id.to_string() });
            }
        }
        let adopt: Vec<(String, u32)> = new.iter().filter_map(|r| Some((r.key.clone(), r.pid()?))).collect();
        self.session.running.extend(new);
        for (key, pid) in adopt {
            effects.push(self.adopt(&key, pid));
        }
        effects
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Init};

    /// A window of process `pid` (its parent: 1), in `unit`.
    fn window(id: i64, app_id: &str, pid: u32, unit: &str, focused: bool) -> Window {
        Window { pids: vec![pid], unit: Some(unit.into()), ..win(id, app_id, "1", focused) }
    }

    fn device() -> State {
        State::new(Init { device_session: true, ..init() })
    }

    #[test]
    fn what_sways_config_starts_under_uwsm_is_no_service() {
        assert!(Instance::is_service("xdg-desktop-portal-gtk.service"));
        assert!(!Instance::is_service("wayland-wm@sway.service"), "the compositor's unit: moved out of");
        assert!(!Instance::is_service("clamshell-nested-sway.service"), "the desktop's nested sway's too");
        assert!(!Instance::is_service("session-2.scope"));
    }

    #[test]
    fn a_window_of_no_entry_makes_its_program_one() {
        let mut s = device();
        let thing = window(5, "org.x.Thing", 500, "app-thing.scope", true);
        let (_, fx) = s.apply(Event::Windows(vec![thing.clone(), window(6, "org.x.Thing", 500, "app-thing.scope", false)]));
        let r = s.session().running("window:org.x.Thing").unwrap();
        assert_eq!((r.found(), r.pid(), r.app_id.as_deref()), (true, Some(500), Some("org.x.Thing")));
        let ws = r.workspace();
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![5, 6], to: ws.clone() })), "{fx:?}");
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws))), "it was in front: {fx:?}");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
        assert!(s.dock().items().iter().any(|i| &*i.id == "window:org.x.Thing"), "on the dock");
        // Its picture: looked up by its app id; found, it is the entry's.
        assert!(fx.contains(&Effect::FindIcon { entry: "window:org.x.Thing".into(), app_id: "org.x.Thing".into() }), "{fx:?}");
        s.apply(Event::Reading(crate::Reading::Icon { entry: "window:org.x.Thing".into(), path: "/icons/thing.svg".into() }));
        assert_eq!(s.entry("window:org.x.Thing").and_then(|g| g.icon.clone()), Some("/icons/thing.svg".into()));
        // Not moved yet (the system didn't): its process ended; its windows gone, it has ended.
        assert_eq!(s.force_kill("window:org.x.Thing"), vec![Effect::EndProcesses { pids: vec![500], force: true }]);
        s.apply(Event::Windows(vec![]));
        assert!(s.session().running("window:org.x.Thing").is_none());
        // One the library has a desktop file for: that entry.
        s.apply(Event::Windows(vec![window(7, "firefox", 501, "app-firefox.scope", false)]));
        assert!(s.session().running("desktop:firefox").is_some());
    }

    #[test]
    fn a_window_of_another_process_after_its_entry_closed_is_found_anew() {
        let mut s = device();
        let adopted = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Adopt { instance, pid } = e { Some((instance.clone(), *pid)) } else { None });
        let (_, fx) = s.apply(Event::Windows(vec![window(5, "foot", 500, "app-foot.scope", true)]));
        let (instance, _) = adopted(&fx).unwrap();
        s.apply(Event::Units(vec![instance.clone()]));
        // Moved: its window's unit is ours now. Closed: its scope still listed a moment.
        s.apply(Event::Windows(vec![Window { unit: instance.unit(), ..window(5, "foot", 500, "app-foot.scope", true) }]));
        s.apply(Event::Windows(vec![]));
        // Another foot (another process) before systemd's list says the scope is gone.
        let other = window(6, "foot", 600, "session-3.scope", true);
        let (_, fx) = s.apply(Event::Windows(vec![other.clone()]));
        let (_, more) = s.apply(Event::Units(vec![]));
        let (_, again) = s.apply(Event::Windows(vec![other]));
        let all: Vec<Effect> = fx.into_iter().chain(more).chain(again).collect();
        assert_eq!(adopted(&all).map(|(_, pid)| pid), Some(600), "{all:?}");
    }

    #[test]
    fn a_services_window_is_no_entry() {
        let mut s = device();
        let (_, fx) = s.apply(Event::Windows(vec![window(5, "xdg-desktop-portal-gtk", 300, "xdg-desktop-portal-gtk.service", true)]));
        assert!(s.session().running_entries().is_empty(), "the program that asked for it is its");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. } | Effect::Window(WindowCommand::Move { .. }))), "{fx:?}");
    }

    #[test]
    fn a_process_with_another_entrys_windows_is_not_moved() {
        let mut library = clamshell_library::demo::library();
        let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = State::new(Init { library, device_session: true, ..init() });
        s.launch("desktop:firefox".into());
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        let mine = window(5, "firefox", 700, "waydroid-session.scope", false);
        s.apply(Event::Windows(vec![mine.clone()]));
        // Another program's window, of the same process (Waydroid's session: an app the library hasn't).
        let (_, fx) = s.apply(Event::Windows(vec![mine.clone(), window(6, "org.other", 700, "waydroid-session.scope", true)]));
        let r = s.session().running("window:org.other").unwrap();
        assert_eq!(r.pid(), None, "not its own process");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "{fx:?}");
        // Quit: its windows closed, and that is all.
        let id: Arc<str> = "window:org.other".into();
        assert_eq!(s.quit(&id), vec![Effect::Window(WindowCommand::Close(vec![6]))]);
        let (_, fx) = s.apply(Event::Windows(vec![mine]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndProcesses { .. } | Effect::EndRun { .. })), "{fx:?}");
        assert!(s.session().running("window:org.other").is_none());
    }

    #[test]
    fn nested_in_a_desktop_a_window_found_is_adopted_too() {
        // (A guest: a window in ClamShell's own sway is that session's, as on the device.)
        let mut s = state();
        let (_, fx) = s.apply(Event::Windows(vec![window(5, "org.x.Thing", 500, "app-thing.scope", true)]));
        assert!(s.session().running("window:org.x.Thing").is_some(), "on the dock");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
    }
}
