//! Programs ClamShell didn't start, found by a window of theirs that belongs to no entry, or by a tray icon
//! (see [`crate::session::tray`]), so that anything with a window is on the dock.
//!
//! Each such program runs as an entry ([`Origin::Found`]). That is the library's desktop entry named like
//! the window's app id, or else an entry of its own (`window:<app id>`). An entry of its own gets a picture
//! looked up by the app id in the icon themes, or else the UI's app glyph. The program's windows, those
//! with its app id or of its process, go to its group, which is shown if one of them was in front.
//!
//! Its process, and those it started, are moved into a scope of ours as a launch of the entry
//! ([`Session::adopt`]). From then on it is paused, quit and killed through its slice. A process that also
//! has another entry's windows, such as a browser showing a file opened from the Files tab, is not moved:
//! Quit closes its windows, and that is all. Nested in a desktop session this works the same, since a
//! window in ClamShell's compositor belongs to that session; the desktop's own programs show their windows
//! on the desktop, not there.
//!
//! A window that a systemd service opened, such as a portal's file chooser or the password prompt, is not
//! found as a program. It belongs to the program that asked for it, and is gone in seconds.

use std::sync::Arc;

use clamshell_library::ids;

use crate::effect::Effect;
use crate::out::Out;
use crate::session::instance::Instance;
use crate::session::running::{Origin, Running};
use crate::session::windows::WindowCommand;
use crate::session::windows::{Window, WindowFacts};
use crate::{Clock, Entry, Library, Session};

impl Session {
    /// Moves a found program's process, and those it started, into a scope of ours as a launch of entry
    /// `key`. Once systemd lists the scope, it runs as a launch (see [`Session::follow_units`]).
    fn adopt(&mut self, key: &str, pid: u32, clock: &Clock) -> Effect {
        let (secs, n) = (clock.unix(), self.next_instance());
        Effect::Adopt { instance: Instance { key: key.to_owned(), secs, n }, pid }
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
        for w in self.windows.iter().filter(|w| !taken.contains(&w.id) && !w.launched() && self.running_of(w).is_none()) {
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
            let entry = window_entry(library, w, pid);
            if self.running(&entry.id).is_some() {
                continue;
            }
            new.push(Running { app_id, seen: true, ..Running::of(&entry, Origin::Found { pid: own.then_some(pid) }, now) });
        }
        self.start_found(new, taken, clock, out);
    }

    /// Runs entries just found by windows or tray icons. Their windows, except `taken`, go to their
    /// groups, which are shown if one of the windows was in front. An entry of its own gets its
    /// picture looked up by its app id, and each is moved into a scope of ours.
    pub(crate) fn start_found(&mut self, new: Vec<Running>, taken: &[i64], clock: &Clock, out: &mut Out) {
        for r in &new {
            let mine: Vec<&Window> = self.windows.iter().filter(|w| !taken.contains(&w.id) && (r.owns(w) || r.fathered(w))).collect();
            let focused = mine.iter().any(|w| self.top_window().is_some_and(|top| top.id == w.id));
            let ids: Vec<i64> = mine.iter().map(|w| w.id).collect();
            if !ids.is_empty() {
                self.command(WindowCommand::Move { windows: ids, to: r.group() });
            }
            if focused {
                self.focus(&r.group());
            }
            out.effects.extend(find_icon(r));
        }
        let adopt: Vec<(String, u32)> = new.iter().filter_map(|r| Some((r.key.clone(), r.pid()?))).collect();
        self.running.extend(new);
        for (key, pid) in adopt {
            let adopted = self.adopt(&key, pid, clock);
            out.effects.push(adopted);
        }
    }
}

/// The entry that a program found by window `w` of its process `pid` runs as: the library's desktop entry
/// named like the window's app id, or else one of its own, named by the app id, or without one by the
/// process.
pub(crate) fn window_entry(library: &Library, w: &WindowFacts, pid: u32) -> Entry {
    found_entry(library, &w.app_id, || {
        let name = if w.app_id.is_empty() { pid.to_string() } else { w.app_id.clone() };
        let title = if w.app_id.is_empty() { &w.title } else { &w.app_id };
        Entry { app: true, ..Entry::new(&format!("{}{name}", ids::WINDOW), title) }
    })
}

/// The entry that a program found by `name`, its window's app id or its tray icon's id, runs as. It is the
/// library's desktop entry of that name, ignoring case, or else one of its own made by `own`.
pub(crate) fn found_entry(library: &Library, name: &str, own: impl FnOnce() -> Entry) -> Entry {
    let named = |g: &&Entry| ids::desktop_file(&g.id).is_some_and(|d| d.eq_ignore_ascii_case(name));
    library.entries.iter().find(named).cloned().unwrap_or_else(own)
}

/// The icon to look for, for an entry found by its window.
fn find_icon(r: &Running) -> Option<Effect> {
    let app_id = r.app_id.as_ref().filter(|_| r.id.starts_with(ids::WINDOW))?;
    Some(Effect::FindIcon { entry: r.id.clone(), app_id: app_id.to_string() })
}

impl Session {
    /// The icons thread was started again after a panic, and the lookup it was doing may be lost: the
    /// icons of the entries found by their windows are looked for again.
    pub(crate) fn find_icons_again(&self, out: &mut Out) {
        out.effects.extend(self.running.iter().filter_map(find_icon));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Init, State};

    /// A window of process `pid`, whose parent is 1, in `unit`.
    fn window(id: i64, app_id: &str, pid: u32, unit: &str) -> WindowFacts {
        WindowFacts { pids: vec![pid], unit: Some(unit.into()), ..win(id, app_id) }
    }

    fn device() -> State {
        State::new(Init { device_session: true, ..init() })
    }

    #[test]
    fn clamshells_own_unit_is_no_service() {
        assert!(Instance::is_service("xdg-desktop-portal-gtk.service"));
        assert!(!Instance::is_service("clamshell.service"), "ClamShell's unit on the device: moved out of");
        assert!(!Instance::is_service("clamshell-nested.service"), "and on the desktop");
        assert!(!Instance::is_service("session-2.scope"));
    }

    #[test]
    fn a_window_of_no_entry_makes_its_program_one() {
        let mut s = device();
        let thing = window(5, "org.x.Thing", 500, "app-thing.scope");
        let fx = s.observed(vec![thing.clone(), window(6, "org.x.Thing", 500, "app-thing.scope")]);
        let r = s.session().running("window:org.x.Thing").unwrap();
        assert_eq!((r.found(), r.pid(), r.app_id.as_deref()), (true, Some(500), Some("org.x.Thing")));
        let ws = r.group();
        assert!([5, 6].iter().all(|id| s.session().window(*id).is_some_and(|w| w.group.as_ref() == Some(&ws))));
        assert_eq!(s.session().top_window().map(|w| w.id), Some(6), "the one in front, opened last");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
        assert!(s.dock().items().iter().any(|i| &*i.id == "window:org.x.Thing"), "on the dock");
        // Its picture is looked up by its app id, and once found it is the entry's.
        assert!(fx.contains(&Effect::FindIcon { entry: "window:org.x.Thing".into(), app_id: "org.x.Thing".into() }), "{fx:?}");
        s.applied(Event::Reading(crate::Reading::Icon { entry: "window:org.x.Thing".into(), path: "/icons/thing.svg".into() }));
        assert_eq!(s.entry("window:org.x.Thing").and_then(|g| g.icon.clone()), Some("/icons/thing.svg".into()));
        // The system hasn't moved it yet, so Force kill ends its process. Once its windows are gone, it
        // has ended.
        assert_eq!(force_kill(&mut s, "window:org.x.Thing"), vec![Effect::EndProcesses { pids: vec![500], force: true }]);
        s.observed(vec![]);
        assert!(s.session().running("window:org.x.Thing").is_none());
        // A program the library has a desktop file for runs as that entry.
        s.observed(vec![window(7, "firefox", 501, "app-firefox.scope")]);
        assert!(s.session().running("desktop:firefox").is_some());
    }

    #[test]
    fn a_window_of_another_process_after_its_entry_closed_is_found_anew() {
        let mut s = device();
        let adopted = |fx: &[Effect]| fx.iter().find_map(|e| if let Effect::Adopt { instance, pid } = e { Some((instance.clone(), *pid)) } else { None });
        let fx = s.observed(vec![window(5, "foot", 500, "app-foot.scope")]);
        let (instance, _) = adopted(&fx).unwrap();
        s.applied(Event::Units(vec![instance.clone()]));
        // Once moved, its window's unit is ours. Once closed, its scope is still listed for a moment.
        s.observed(vec![WindowFacts { unit: Some(instance.unit()), ..window(5, "foot", 500, "app-foot.scope") }]);
        s.observed(vec![]);
        // Another foot, in another process, appears before systemd's list says the scope is gone.
        let other = window(6, "foot", 600, "session-3.scope");
        let fx = s.observed(vec![other.clone()]);
        let more = s.applied(Event::Units(vec![]));
        let again = s.observed(vec![other]);
        let all: Vec<Effect> = fx.into_iter().chain(more).chain(again).collect();
        assert_eq!(adopted(&all).map(|(_, pid)| pid), Some(600), "{all:?}");
    }

    #[test]
    fn a_services_window_is_no_entry() {
        let mut s = device();
        let portal = window(5, "xdg-desktop-portal-gtk", 300, "xdg-desktop-portal-gtk.service");
        let fx = s.observed(vec![portal.clone()]);
        assert!(s.session().running_entries().is_empty(), "the program that asked for it is its");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "{fx:?}");
        assert_eq!(s.session().window(5).and_then(|w| w.group.clone()), Some(Group::Window(5)), "a group of its own");
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
        let mine = window(5, "firefox", 700, "waydroid-session.scope");
        s.observed(vec![mine.clone()]);
        // Another program's window from the same process: Waydroid's session showing an app the library
        // doesn't have.
        let fx = s.observed(vec![mine.clone(), window(6, "org.other", 700, "waydroid-session.scope")]);
        let r = s.session().running("window:org.other").unwrap();
        assert_eq!(r.pid(), None, "not its own process");
        assert!(!fx.iter().any(|e| matches!(e, Effect::Adopt { .. })), "{fx:?}");
        // Quit closes its windows, and that is all.
        let id: Arc<str> = "window:org.other".into();
        assert_eq!(quit(&mut s, &id), vec![Effect::Close(vec![6])]);
        let fx = s.observed(vec![mine]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::EndProcesses { .. } | Effect::EndRun { .. })), "{fx:?}");
        assert!(s.session().running("window:org.other").is_none());
    }

    /// A restarted ClamShell finds the scopes of programs the last one found and moved, frozen. The
    /// library has no entry of their keys; each runs again as found, named by its window or its tray icon,
    /// on the dock, held and thawed when shown, and quit through its slice.
    #[test]
    fn a_found_program_in_our_scope_is_found_again_after_a_restart() {
        let mut s = device();
        s.applied(Event::tick(1000));
        let key = |id: &str| clamshell_library::key_of(id);
        let (thing, tray) = (Instance { key: key("window:org.x.Thing"), secs: 1, n: 0 }, Instance { key: key("tray:thing"), secs: 1, n: 1 });
        s.applied(Event::Units(vec![thing.clone(), tray.clone()]));
        assert!(s.session().running_entries().is_empty(), "not known yet");
        s.applied(Event::Frozen(vec![thing.key.as_str().into()]));
        let in_scope = WindowFacts { unit: Some(thing.unit()), ..window(5, "org.x.Thing", 500, "") };
        s.observed(vec![in_scope]);
        let r = s.session().running("window:org.x.Thing").unwrap();
        assert_eq!((r.origin.clone(), r.key.as_str()), (Origin::Ours, thing.key.as_str()));
        assert!(s.dock().items().iter().any(|i| &*i.id == "window:org.x.Thing"), "on the dock");
        assert!(s.session().suspended("window:org.x.Thing"), "held frozen");
        let fx = show(&mut s, "window:org.x.Thing");
        assert_eq!(fx.first(), Some(&Effect::Freeze { slice: Instance::slice_of(&thing.key), frozen: false }), "{fx:?}");
        assert_eq!(force_kill(&mut s, "window:org.x.Thing"), vec![Effect::EndRun { key: thing.key.clone(), force: true }]);
        // A program found by its tray icon is named by it.
        let icon = crate::session::tray::TrayItem {
            key: ":1.7/c".into(),
            id: "thing".into(),
            title: "Thing".into(),
            pid: Some(77),
            unit: Some(tray.unit()),
            icon: None,
            menu: vec![],
        };
        s.applied(Event::Tray(vec![icon]));
        assert_eq!(s.session().running("tray:thing").map(|r| r.origin.clone()), Some(Origin::Ours));
    }

    #[test]
    fn nested_in_a_desktop_a_window_found_is_adopted_too() {
        // As a guest in a desktop, a window in ClamShell's compositor belongs to that session, as on the
        // device.
        let mut s = state();
        let fx = s.observed(vec![window(5, "org.x.Thing", 500, "app-thing.scope")]);
        assert!(s.session().running("window:org.x.Thing").is_some(), "on the dock");
        assert!(fx.iter().any(|e| matches!(e, Effect::Adopt { pid: 500, .. })), "{fx:?}");
    }
}
