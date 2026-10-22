//! Entries started with ClamShell: chosen in an entry's dock menu (Start with ClamShell), saved. Once
//! ClamShell knows what runs (systemd's first list of its scopes: what a ClamShell restarted finds still
//! running isn't started twice), each that the library has and that doesn't run is launched, in the
//! background: on its own workspace, not shown (Steam's window, if it opens one, doesn't cover shelf).

use std::sync::Arc;

use crate::State;
use crate::effect::{Effect, Save};

impl State {
    /// Whether `id` starts with ClamShell.
    pub fn autostarts(&self, id: &str) -> bool {
        self.autostart.iter().any(|a| &**a == id)
    }

    /// Start with ClamShell, or don't: the choice saved.
    pub(crate) fn toggle_autostart(&mut self, id: Arc<str>) -> Vec<Effect> {
        match self.autostart.iter().position(|a| *a == id) {
            Some(i) => {
                self.autostart.remove(i);
            }
            None => self.autostart.push(id),
        }
        vec![Effect::Save(Save::Autostart(self.autostart.clone()))]
    }

    /// The first time what runs is known: the entries to start that don't run, launched without being
    /// shown.
    pub(crate) fn start_autostart(&mut self) -> Vec<Effect> {
        if std::mem::replace(&mut self.autostarted, true) {
            return vec![];
        }
        let ids: Vec<Arc<str>> = self.autostart.iter().filter(|id| self.shelf.library().has(id) && self.session.running(id).is_none()).cloned().collect();
        let mut effects = Vec::new();
        for id in ids {
            let shown = self.session.pending_show.clone();
            effects.extend(self.launch(id));
            // (In the background: what was to be shown still is.)
            self.session.pending_show = shown;
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::{Event, Init, Instance};

    #[test]
    fn chosen_entries_start_once_what_runs_is_known_unless_they_run() {
        let launches = |fx: &[Effect]| fx.iter().filter(|e| matches!(e, Effect::Launch { .. })).count();
        let init = |ids: &[&str]| Init { autostart: ids.iter().map(|i| Arc::from(*i)).collect(), ..init() };
        // Zelda and Firefox to start; Firefox's scope is there already (ClamShell restarted).
        let mut s = State::new(init(&["zelda-lttp", "desktop:firefox", "gone"]));
        let firefox = Instance::Run { key: clamshell_library::key_of("desktop:firefox"), secs: 1, n: 0 };
        let (_, fx) = s.apply(Event::Units(vec![firefox]));
        assert_eq!(launches(&fx), 1, "Zelda only: Firefox runs, `gone` isn't in the library: {fx:?}");
        assert!(s.session().pending_show.is_none(), "not shown");
        assert_eq!(launches(&s.apply(Event::Units(vec![])).1), 0, "once");
        // Chosen in the dock menu: saved.
        assert_eq!(s.toggle_autostart("zelda-lttp".into()), vec![Effect::Save(Save::Autostart(vec!["desktop:firefox".into(), "gone".into()]))]);
        assert!(!s.autostarts("zelda-lttp"));
    }
}
