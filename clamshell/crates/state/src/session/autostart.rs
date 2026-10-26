//! Entries started with ClamShell, chosen in an entry's dock menu (Start with ClamShell) and saved. Once
//! ClamShell knows what runs, from systemd's first list of its units, each of these entries that the
//! library has and that doesn't run is launched. Waiting for that list keeps a restarted ClamShell from
//! starting again what is still running. They are launched in the background, in their own groups and
//! not shown, so that a window Steam opens doesn't cover shelf.

use std::sync::Arc;

use crate::effect::{Effect, Save};
use crate::out::Out;
use crate::{Library, Session};

impl Session {
    /// Whether `id` starts with ClamShell.
    pub fn autostarts(&self, id: &str) -> bool {
        self.autostart.iter().any(|a| &**a == id)
    }

    /// Starts `id` with ClamShell, or stops doing so, and saves the choice.
    pub(crate) fn toggle_autostart(&mut self, id: Arc<str>, out: &mut Out) {
        match self.autostart.iter().position(|a| *a == id) {
            Some(i) => {
                self.autostart.remove(i);
            }
            None => self.autostart.push(id),
        }
        out.effects.push(self.autostart_save());
    }

    /// Saves the entries started with ClamShell.
    pub(crate) fn autostart_save(&self) -> Effect {
        Effect::Save(Save::Autostart(self.autostart.clone()))
    }

    /// The first time what runs is known, adds to `due` the entries to start that the library has and
    /// that don't run. They are launched without being shown.
    pub(crate) fn autostart_due(&mut self, library: &Library, due: &mut Vec<Arc<str>>) {
        if !self.units_known || std::mem::replace(&mut self.autostarted, true) {
            return;
        }
        due.extend(self.autostart.iter().filter(|id| library.has(id) && self.running(id).is_none()).cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Init, Instance, State};

    #[test]
    fn chosen_entries_start_once_what_runs_is_known_unless_they_run() {
        let launches = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Launch { .. })).count();
        let init = |ids: &[&str]| Init { autostart: ids.iter().map(|i| Arc::from(*i)).collect(), ..init() };
        // Zelda and Firefox are to start. Firefox's unit is there already, as after a ClamShell restart.
        let mut s = State::new(init(&["zelda-lttp", "desktop:firefox", "gone"]));
        let firefox = Instance { key: clamshell_library::key_of("desktop:firefox"), secs: 1, n: 0 };
        let fx = s.applied(Event::Units(vec![firefox]));
        assert_eq!(launches(&fx), 1, "Zelda only: Firefox runs, `gone` isn't in the library: {fx:?}");
        assert!(s.session().front_launch().is_none(), "not shown");
        assert_eq!(launches(&s.applied(Event::Units(vec![]))), 0, "once");
        // Choosing in the dock menu saves the list.
        let fx = s.handle(Event::Entry("zelda-lttp".into(), crate::EntryAct::ToggleAutostart));
        assert_eq!(fx, vec![Effect::Save(Save::Autostart(vec!["desktop:firefox".into(), "gone".into()]))]);
        assert!(!s.session().autostarts("zelda-lttp"));
    }

    /// The entries to start are looked for in the library, so with what runs known first, they start
    /// once the library is read.
    #[test]
    fn chosen_entries_wait_for_the_library() {
        let launches = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Launch { .. })).count();
        let mut s = State::new(Init { autostart: vec!["zelda-lttp".into()], library: None, ..init() });
        assert_eq!(launches(&s.applied(Event::Units(vec![]))), 0, "not in the empty shelf, and not given up");
        assert_eq!(launches(&s.applied(Event::Library(Box::new(clamshell_library::demo::library())))), 1);
        assert_eq!(launches(&s.applied(Event::Units(vec![]))), 0, "once");
    }
}
