//! Programs ClamShell didn't start, found by a window of theirs that belongs to no entry, or by a tray icon
//! (see [`crate::session::tray`]), so that anything with a window is on the dock.
//!
//! Each such program runs as an entry ([`Origin::Found`]). That is the library's desktop entry named like
//! the window's app id, or else an entry of its own (`window:<app id>`). An entry of its own gets a picture
//! looked up by the app id in the icon themes, or else the UI's app glyph. The program's windows, those
//! with its app id or of its process, go to its workspace, which is shown if one of them was in front.
//!
//! Its process, and those it started, are moved into a scope of ours as a launch of the entry
//! ([`Session::adopt`]). From then on it is paused, quit and killed through its slice. A process that also
//! has another entry's windows, such as a browser showing a file opened from the Files tab, is not moved:
//! Quit closes its windows, and that is all. Nested in a desktop session this works the same, since a
//! window in ClamShell's sway belongs to that session; the desktop's own programs show their windows on the
//! desktop, not there.
//!
//! A window that a systemd service opened, such as a portal's file chooser or the password prompt, is not
//! found as a program. It belongs to the program that asked for it, and is gone in seconds.

use std::sync::Arc;

use clamshell_library::ids;

use crate::effect::{Effect, WindowCommand};
use crate::out::Out;
use crate::session::instance::Instance;
use crate::session::running::{Origin, Running};
use crate::session::windows::Window;
use crate::{Clock, Entry, Library, Session};

impl Session {
    /// Moves a found program's process, and those it started, into a scope of ours as a launch of entry
    /// `key`. Once systemd lists the scope, it runs as a launch (see [`Session::follow_units`]).
    fn adopt(&mut self, key: &str, pid: u32, clock: &Clock) -> Effect {
        let (secs, n) = (clock.unix(), self.next_instance());
        Effect::Adopt { instance: Instance::Run { key: key.to_owned(), secs, n }, pid }
    }

    /// Makes the programs of windows that still belong to no entry run as entries found by them. `taken`
    /// lists windows just given to a browsed file, which are left out.
    pub(crate) fn follow_found_windows(&mut self, taken: &[i64], clock: &Clock, library: &Library, out: &mut Out) {
        let now = clock.mono();
        // A starting launch's window is left out, such as a Steam game found running whose launch is
        // still to come.
        let starting = |w: &Window| self.launching.iter().any(|l| l.app_id.as_deref() == Some(w.app_id.as_str()));
        let service = |w: &Window| w.unit.as_deref().is_some_and(Instance::is_service);
        let mut new: Vec<Running> = Vec::new();
        for w in self.windows.iter().filter(|w| !taken.contains(&w.id) && !w.has_home() && self.running_of(w).is_none()) {
            let Some(&pid) = w.pids.first().filter(|_| !starting(w) && !service(w)) else {
                continue;
            };
            if new.iter().any(|r| r.owns(w) || r.fathered(w)) {
                continue;
            }
            // Its process is its own if none of the process's windows belongs to another entry.
            let own = !self.windows.iter().any(|o| o.pids.first() == Some(&pid) && self.running_of(o).is_some());
            let app_id: Option<Arc<str>> = (!w.app_id.is_empty()).then(|| w.app_id.as_str().into());
            // Known by neither its app id nor its process, no entry would find it again.
            if app_id.is_none() && !own {
                continue;
            }
            let entry = found_entry(library, &w.app_id, || {
                let name = app_id.as_deref().map_or_else(|| pid.to_string(), str::to_owned);
                let title = if w.app_id.is_empty() { &w.title } else { &w.app_id };
                Entry { app: true, ..Entry::new(&format!("{}{name}", ids::WINDOW), title) }
            });
            if self.running(&entry.id).is_some() {
                continue;
            }
            new.push(Running { app_id, seen: true, ..Running::of(&entry, Origin::Found { pid: own.then_some(pid), tray: None }, now) });
        }
        self.start_found(new, taken, clock, out);
    }

    /// Runs entries just found by windows or tray icons. Their windows, except `taken`, go to their
    /// workspaces, which are shown if one of the windows was in front. An entry of its own gets its
    /// picture looked up by its app id, and each is moved into a scope of ours.
    pub(crate) fn start_found(&mut self, new: Vec<Running>, taken: &[i64], clock: &Clock, out: &mut Out) {
        for r in &new {
            let mine: Vec<&Window> = self.windows.iter().filter(|w| !taken.contains(&w.id) && (r.owns(w) || r.fathered(w))).collect();
            if !mine.is_empty() {
                out.effects.push(Effect::Window(WindowCommand::Move { windows: mine.iter().map(|w| w.id).collect(), to: r.workspace() }));
            }
            if mine.iter().any(|w| w.focused) {
                out.effects.push(Effect::Window(WindowCommand::Focus(r.workspace())));
            }
            if r.id.starts_with(ids::WINDOW)
                && let Some(app_id) = &r.app_id
            {
                out.effects.push(Effect::FindIcon { entry: r.id.clone(), app_id: app_id.to_string() });
            }
        }
        let adopt: Vec<(String, u32)> = new.iter().filter_map(|r| Some((r.key.clone(), r.pid()?))).collect();
        self.running.extend(new);
        for (key, pid) in adopt {
            let adopted = self.adopt(&key, pid, clock);
            out.effects.push(adopted);
        }
    }
}

/// The entry that a program found by `name`, its window's app id or its tray icon's id, runs as. It is the
/// library's desktop entry of that name, ignoring case, or else one of its own made by `own`.
pub(crate) fn found_entry(library: &Library, name: &str, own: impl FnOnce() -> Entry) -> Entry {
    let named = |g: &&Entry| ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(name));
    library.entries.iter().find(named).cloned().unwrap_or_else(own)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Init, State};

    /// A window of process `pid`, whose parent is 1, in `unit`.
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
        let fx = s.applied(Event::Windows(vec![thing.clone(), window(6, "org.x.Thing", 500, "app-thing.scope", false)]));
        let r = s.session().running("window:org.x.Thing").unwrap();
        assert_eq!((r.found(), r.pid(), r.app_id.as_deref()), (true, Some(500), Some("org.x.Thing")));
        let ws = r.workspace();
        assert!(fx.contains(&Effect::Window(WindowCommand::Move { windows: vec![5, 6], to: ws.clone() })), "{fx:?}");
        assert!(fx.contains(&Effect::Window(WindowCommand::Focus(ws))), "it was in front: {fx:?}");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
        assert!(s.dock().items().iter().any(|i| &*i.id == "window:org.x.Thing"), "on the dock");
        // Its picture is looked up by its app id, and once found it is the entry's.
        assert!(fx.contains(&Effect::FindIcon { entry: "window:org.x.Thing".into(), app_id: "org.x.Thing".into() }), "{fx:?}");
        s.applied(Event::Reading(crate::Reading::Icon { entry: "window:org.x.Thing".into(), path: "/icons/thing.svg".into() }));
        assert_eq!(s.entry("window:org.x.Thing").and_then(|g| g.icon.clone()), Some("/icons/thing.svg".into()));
        // The system hasn't moved it yet, so Force kill ends its process. Once its windows are gone, it
        // has ended.
        assert_eq!(force_kill(&mut s, "window:org.x.Thing"), vec![Effect::EndProcesses { pids: vec![500], force: true }]);
        s.applied(Event::Windows(vec![]));
        assert!(s.session().running("window:org.x.Thing").is_none());
        // A program the library has a desktop file for runs as that entry.
        s.applied(Event::Windows(vec![window(7, "firefox", 501, "app-firefox.scope", false)]));
        assert!(s.session().running("desktop:firefox").is_some());
    }

    #[test]
    fn a_window_of_another_process_after_its_entry_closed_is_found_anew() {
        let mut s = device();
        let adopted = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Adopt { instance, pid } = e { Some((instance.clone(), *pid)) } else { None });
        let fx = s.applied(Event::Windows(vec![window(5, "foot", 500, "app-foot.scope", true)]));
        let (instance, _) = adopted(&fx).unwrap();
        s.applied(Event::Units(vec![instance.clone()]));
        // Once moved, its window's unit is ours. Once closed, its scope is still listed for a moment.
        s.applied(Event::Windows(vec![Window { unit: instance.unit(), ..window(5, "foot", 500, "app-foot.scope", true) }]));
        s.applied(Event::Windows(vec![]));
        // Another foot, in another process, appears before systemd's list says the scope is gone.
        let other = window(6, "foot", 600, "session-3.scope", true);
        let fx = s.applied(Event::Windows(vec![other.clone()]));
        let more = s.applied(Event::Units(vec![]));
        let again = s.applied(Event::Windows(vec![other]));
        let all: Vec<Effect> = fx.into_iter().chain(more).chain(again).collect();
        assert_eq!(adopted(&all).map(|(_, pid)| pid), Some(600), "{all:?}");
    }

    #[test]
    fn a_services_window_is_no_entry() {
        let mut s = device();
        let fx = s.applied(Event::Windows(vec![window(5, "xdg-desktop-portal-gtk", 300, "xdg-desktop-portal-gtk.service", true)]));
        assert!(s.session().running_entries().is_empty(), "the program that asked for it is its");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. } | Effect::Window(WindowCommand::Move { .. }))), "{fx:?}");
    }

    #[test]
    fn a_process_with_another_entrys_windows_is_not_moved() {
        let mut library = clamshell_library::demo::library();
        let firefox = library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap();
        firefox.runner = Some(clamshell_library::RunnerOf { handover: true, ..clamshell_library::RunnerOf::new("android", "Android", true) });
        let mut s = State::new(Init { library, device_session: true, ..init() });
        launch_now(&mut s, "desktop:firefox");
        crate::testkit::start(&mut s, "desktop:firefox");
        crate::testkit::end(&mut s, "desktop:firefox");
        let mine = window(5, "firefox", 700, "waydroid-session.scope", false);
        s.applied(Event::Windows(vec![mine.clone()]));
        // Another program's window from the same process: Waydroid's session showing an app the library
        // doesn't have.
        let fx = s.applied(Event::Windows(vec![mine.clone(), window(6, "org.other", 700, "waydroid-session.scope", true)]));
        let r = s.session().running("window:org.other").unwrap();
        assert_eq!(r.pid(), None, "not its own process");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "{fx:?}");
        // Quit closes its windows, and that is all.
        let id: Arc<str> = "window:org.other".into();
        assert_eq!(quit(&mut s, &id), vec![Effect::Window(WindowCommand::Close(vec![6]))]);
        let fx = s.applied(Event::Windows(vec![mine]));
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndProcesses { .. } | Effect::EndRun { .. })), "{fx:?}");
        assert!(s.session().running("window:org.other").is_none());
    }

    #[test]
    fn nested_in_a_desktop_a_window_found_is_adopted_too() {
        // As a guest in a desktop, a window in ClamShell's own sway belongs to that session, as on the
        // device.
        let mut s = state();
        let fx = s.applied(Event::Windows(vec![window(5, "org.x.Thing", 500, "app-thing.scope", true)]));
        assert!(s.session().running("window:org.x.Thing").is_some(), "on the dock");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
    }
}
