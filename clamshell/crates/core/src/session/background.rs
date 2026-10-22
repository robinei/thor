//! What a program does while none of its windows is shown: its In the background setting
//! ([`crate::settings::Setting::Background`]), set at the settings' levels of All, its runner, and itself.
//!
//! - Runs hidden: nothing is done. This is the default for anything that isn't a game.
//! - Pauses hidden, the default for games: once none of its windows has been shown for [`GRACE`] seconds,
//!   its slice, with all its launches' processes, is frozen. When it is shown again, because a window of
//!   it is visible or it is asked to be shown, it is thawed. A program that is starting and has shown no
//!   window yet isn't paused.
//! - Quits closed: once it has had a window and has none left, it is ended, its slice stopped as Quit does
//!   once the windows are closed. This way a program that would stay in the background, such as Steam in
//!   its tray, doesn't.
//!
//! Only entries ClamShell launched have a slice of their own. Those found by their windows or by a tray
//! icon run in another program's processes, and run on as they are. A paused entry is thawed before
//! anything is asked of it: Quit, which asks its windows to close, Force kill, or being shown.
//!
//! Suspend, in an entry's dock menu, freezes its slice now, whatever its setting, and hides its windows. It
//! stays frozen until it is shown (Resume in its menu, or a tap on its tile), quit or killed. A restarted
//! ClamShell leaves our slices as it finds them: one found frozen, whether suspended or paused hidden by
//! the previous ClamShell, is held as suspended ([`crate::Event::Frozen`]).
//!
//! A runner whose entries share one container, such as Waydroid for Android apps, has commands to pause
//! it all and resume it (its `pause` and `resume`). When the runner is set to Pauses hidden, the container
//! is paused once none of its entries has been shown for [`GRACE`] seconds, including when none is
//! running, and resumed as one is shown or launched.

use std::sync::Arc;

use crate::State;
use clamshell_library::RunnerOf;

use crate::effect::Effect;
use crate::session::instance::Instance;
use crate::session::running::Origin;
use crate::settings::InBackground;

/// How long, in seconds, a program's windows are all hidden before it is paused, so that switching away
/// and back quickly doesn't pause it.
pub const GRACE: u64 = 10;

/// What the session keeps for pausing: entries' slices and runners, both by key. Each is paused once it
/// has been out of use for the grace period.
#[derive(Debug, Default)]
pub(crate) struct Background {
    pub(crate) entries: Pauses,
    /// The runners whose entries are paused together, from the library.
    pub(crate) runners: Vec<RunnerOf>,
    pub(crate) runner_pauses: Pauses,
}

/// Things that can be paused, such as an entry's slice or a runner's container. It keeps since when each
/// has been out of use, and which are paused and why.
#[derive(Debug, Default)]
pub(crate) struct Pauses {
    unused: Vec<(Arc<str>, u64)>,
    paused: Vec<(Arc<str>, Why)>,
}

/// Why something is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// Out of use for the grace period, with In the background set to Pauses hidden.
    Hidden,
    /// Held until shown: suspended, or found frozen at start.
    Held,
}

impl Pauses {
    /// Follows whether `key` is in use now; for one that is held, only being shown counts as in use.
    /// Returns `Some(true)` when it is to be paused, having been out of use for the grace period, and
    /// `Some(false)` when it is to be resumed, being paused and in use again.
    fn follow(&mut self, key: &Arc<str>, in_use: bool, now: u64) -> Option<bool> {
        if in_use {
            return self.resume(key).then_some(false);
        }
        let since = match self.unused.iter().find(|(k, _)| k == key) {
            Some((_, t)) => *t,
            None => {
                self.unused.push((key.clone(), now));
                now
            }
        };
        if self.is_paused(key) || now.saturating_sub(since) < GRACE {
            return None;
        }
        self.paused.push((key.clone(), Why::Hidden));
        Some(true)
    }

    /// Marks `key` in use. Returns whether it was paused, and so is to be resumed. It is paused again only
    /// once it has been out of use for the grace period anew.
    fn resume(&mut self, key: &str) -> bool {
        self.unused.retain(|(k, _)| &**k != key);
        let was = self.is_paused(key);
        self.paused.retain(|(k, _)| &**k != key);
        was
    }

    /// Holds `key` paused from now on, when suspended or found frozen. Returns whether it wasn't paused
    /// before, and so is to be frozen.
    fn hold(&mut self, key: &Arc<str>) -> bool {
        let was = self.is_paused(key);
        self.paused.retain(|(k, _)| k != key);
        self.paused.push((key.clone(), Why::Held));
        !was
    }

    fn is_paused(&self, key: &str) -> bool {
        self.paused.iter().any(|(k, _)| &**k == key)
    }

    fn is_held(&self, key: &str) -> bool {
        self.paused.iter().any(|(k, why)| &**k == key && *why == Why::Held)
    }

    /// Keeps following only these keys. The others' entries ended, so they are forgotten, not resumed.
    fn keep(&mut self, keys: &[Arc<str>]) {
        self.unused.retain(|(k, _)| keys.contains(k));
        self.paused.retain(|(k, _)| keys.contains(k));
    }
}

impl State {
    /// Runs after the windows changed, and every second. Pauses, thaws or ends entries by their setting.
    pub(crate) fn follow_background(&mut self) -> Vec<Effect> {
        let now = self.clock.mono();
        let mut effects = Vec::new();
        let mut end = Vec::new();
        let session = &self.session;
        // Launches of ours, including a Steam game's, which is in its launch's own scope since it started.
        let ours: Vec<&crate::session::running::Running> = session.running.iter().filter(|r| r.origin == Origin::Ours).collect();
        let keys: Vec<Arc<str>> = ours.iter().map(|r| Arc::from(r.key.as_str())).collect();
        self.background.entries.keep(&keys);
        for (r, key) in ours.into_iter().zip(&keys) {
            let mine: Vec<&crate::session::windows::Window> = session.windows_of(&r.id).collect();
            let shown = mine.iter().any(|w| w.visible);
            let setting = self.background_of(&r.id);
            // An entry that is quitting isn't paused, since frozen it couldn't end. Nor is one without a
            // window: it has none hidden, and it is starting, between windows or ending, as with Steam's
            // Stop.
            let quitting = session.is_quitting(&r.id);
            let hidden = setting == InBackground::Pauses && !shown && !mine.is_empty() && !quitting;
            // A suspended entry stays frozen until shown.
            let in_use = if self.background.entries.is_held(key) { shown } else { !hidden };
            if let Some(frozen) = self.background.entries.follow(key, in_use, now) {
                effects.push(Effect::Freeze { slice: Instance::slice_of(key), frozen });
            }
            // Quits closed ends it once it has had a window and has none left.
            if setting == InBackground::Quits && r.seen && mine.is_empty() && !quitting {
                end.push(r.id.clone());
            }
        }
        for id in end {
            effects.extend(self.quit(&id));
        }
        effects.extend(self.follow_runner_pauses());
        effects
    }

    /// The runners whose entries are paused together: those of the library's entries with a `pause`.
    pub(crate) fn follow_pausable_runners(&mut self) {
        let mut runners: Vec<RunnerOf> = Vec::new();
        for r in self.shelf.library().entries.iter().filter_map(|g| g.runner.as_ref()).filter(|r| !r.pause.is_empty()) {
            if !runners.iter().any(|o| o.key == r.key) {
                runners.push(r.clone());
            }
        }
        self.background.runners = runners;
    }

    /// Pauses each runner whose entries are paused together, when it is set to Pauses hidden and none of
    /// its entries has been shown for the grace period. None may be starting either, that is launched with
    /// no window yet, since paused it would never draw one. Resumes the runner as one is shown, or when its
    /// setting changes.
    fn follow_runner_pauses(&mut self) -> Vec<Effect> {
        let now = self.clock.mono();
        let mut effects = Vec::new();
        for runner in &self.background.runners {
            let pauses = self.background_of(&runner.key) == InBackground::Pauses;
            let session = &self.session;
            let of_runner = |r: &crate::session::running::Running| r.entry.as_ref().and_then(|g| g.runner.as_ref()).is_some_and(|o| o.key == runner.key);
            let shown = session.windows.iter().filter(|w| w.visible).any(|w| session.running_of(w).is_some_and(of_runner));
            let starting = session.running.iter().filter(|r| of_runner(r)).any(|r| !r.found() && !r.seen);
            match self.background.runner_pauses.follow(&runner.key, !pauses || shown || starting, now) {
                Some(true) => effects.push(Effect::RunnerCommand(runner.pause.to_vec())),
                Some(false) => effects.push(Effect::RunnerCommand(runner.resume.to_vec())),
                None => {}
            }
        }
        effects
    }

    /// Resumes the runner of entry `id` before the entry is launched or shown, if the runner is paused.
    pub(crate) fn resume_runner_of(&mut self, id: &str) -> Option<Effect> {
        let runner = self.entry(id)?.runner.as_ref()?;
        let resume = runner.resume.to_vec();
        self.background.runner_pauses.resume(&runner.key.clone()).then_some(Effect::RunnerCommand(resume))
    }

    /// Thaws an entry, if it is paused, before it is asked anything: to close, to end or to be shown.
    pub(crate) fn thaw(&mut self, id: &str) -> Option<Effect> {
        let key = self.session.running(id)?.key.clone();
        self.background.entries.resume(&key).then(|| Effect::Freeze { slice: Instance::slice_of(&key), frozen: false })
    }

    /// Whether entry `id` can be suspended: it runs in a slice of ours, and isn't suspended.
    pub fn suspendable(&self, id: &str) -> bool {
        self.session.running(id).is_some_and(|r| r.origin == Origin::Ours && !self.suspended(id))
    }

    pub fn suspended(&self, id: &str) -> bool {
        self.session.running(id).is_some_and(|r| self.background.entries.is_held(&r.key))
    }

    /// Suspends entry `id`: freezes it now (see the module's doc) and hides its windows if one is shown.
    pub(crate) fn suspend(&mut self, id: &str) -> Vec<Effect> {
        if !self.suspendable(id) {
            return vec![];
        }
        let Some(key) = self.session.running(id).map(|r| Arc::<str>::from(r.key.as_str())) else {
            return vec![];
        };
        let mut effects = vec![];
        if self.session.windows_of(id).any(|w| w.visible) {
            effects.push(Effect::Window(crate::effect::WindowCommand::ShowShelf));
        }
        if self.background.entries.hold(&key) {
            effects.push(Effect::Freeze { slice: Instance::slice_of(&key), frozen: true });
        }
        effects
    }

    /// Holds our slices found frozen at start, by key, as suspended, leaving them frozen.
    pub(crate) fn found_frozen(&mut self, keys: Vec<Arc<str>>) {
        for key in keys {
            self.background.entries.hold(&key);
        }
    }

    /// Whether an entry is paused, that is frozen while hidden.
    pub fn paused(&self, id: &str) -> bool {
        self.session.running(id).is_some_and(|r| self.background.entries.is_paused(&r.key))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Event;
    use crate::effect::WindowCommand;
    use crate::session::windows::Window;
    use crate::settings::{Choice, Setting};
    use crate::testkit::*;

    fn freeze(key: &str, frozen: bool) -> Effect {
        Effect::Freeze { slice: Instance::slice_of(key), frozen }
    }

    #[test]
    fn pauses_hidden_thawed_when_shown_or_asked() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.prefs.settings_mut().set(Some(&"zelda-lttp".into()), Setting::Background, Some(InBackground::Pauses.value()));
        let window = |visible| Window { unit: zelda.unit(), visible, ..win(3, "zelda3", &zelda.workspace(), visible) };
        s.apply(Event::tick(100));
        s.apply(Event::Windows(vec![window(true)]));
        // Once hidden for the grace period, it is paused.
        let fx = s.apply(Event::Windows(vec![window(false)])).1;
        assert!(!fx.contains(&freeze(&key, true)));
        assert!(!s.apply(Event::tick(100 + GRACE - 1)).1.contains(&freeze(&key, true)), "within the grace");
        assert!(s.apply(Event::tick(100 + GRACE)).1.contains(&freeze(&key, true)));
        assert!(s.paused("zelda-lttp"));
        assert!(!s.apply(Event::tick(101 + GRACE)).1.contains(&freeze(&key, true)), "once");
        // Shown again, it is thawed.
        assert!(s.apply(Event::Windows(vec![window(true)])).1.contains(&freeze(&key, false)));
        assert!(!s.paused("zelda-lttp"));
        // Paused and then shown from the dock, it is thawed before its workspace is shown.
        s.apply(Event::Windows(vec![window(false)]));
        let t = 101 + 2 * GRACE;
        s.apply(Event::tick(t));
        assert!(s.paused("zelda-lttp"));
        assert_eq!(s.show_running("zelda-lttp"), vec![freeze(&key, false), Effect::Window(WindowCommand::Focus(zelda.workspace()))]);
        // It is paused again after being hidden for the grace period anew. Quitting thaws it, so that it
        // can answer its window's closing.
        s.apply(Event::tick(t + 1));
        s.apply(Event::tick(t + GRACE));
        assert!(!s.paused("zelda-lttp"), "the grace counts again from the thaw");
        s.apply(Event::tick(t + 1 + GRACE));
        assert!(s.paused("zelda-lttp"));
        let id: Arc<str> = "zelda-lttp".into();
        assert_eq!(s.quit(&id), vec![freeze(&key, false), Effect::Window(WindowCommand::Close(vec![3]))]);
        // With its window gone it ends, and it isn't paused meanwhile even though it is hidden.
        s.apply(Event::Windows(vec![]));
        s.apply(Event::tick(t + 3 * GRACE));
        assert!(!s.paused("zelda-lttp"), "quitting");
    }

    #[test]
    fn a_runner_paused_together_pauses_when_none_is_shown_and_resumes_to_launch() {
        let runner = clamshell_library::RunnerOf {
            handover: true,
            pause: vec!["pause".to_owned()].into(),
            resume: vec!["resume".to_owned()].into(),
            ..clamshell_library::RunnerOf::new("android", "Android", true)
        };
        let mut library = clamshell_library::demo::library();
        library.entries.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap().runner = Some(runner.clone());
        let mut s = State::new(crate::Init { library, ..init() });
        s.prefs.settings_mut().set(Some(&runner.key), Setting::Background, Some(InBackground::Pauses.value()));
        let (pause, resume) = (Effect::RunnerCommand(vec!["pause".into()]), Effect::RunnerCommand(vec!["resume".into()]));
        // With nothing of it running, it is paused after the grace period.
        s.apply(Event::tick(100));
        assert!(!s.apply(Event::tick(100 + GRACE - 1)).1.contains(&pause));
        assert!(s.apply(Event::tick(100 + GRACE)).1.contains(&pause));
        assert!(!s.apply(Event::tick(101 + GRACE)).1.contains(&pause), "once");
        // Launching resumes it first.
        let fx = s.launch("desktop:firefox".into());
        assert_eq!(fx.first(), Some(&resume), "{fx:?}");
        crate::testkit::start(&mut s, "desktop:firefox");
        // While starting, first as its launch and then handed over with no window yet, it isn't paused.
        let t = 101 + 2 * GRACE;
        assert!(!s.apply(Event::tick(t)).1.contains(&pause), "its launch runs");
        crate::testkit::end(&mut s, "desktop:firefox");
        assert!(!s.apply(Event::tick(t + GRACE)).1.contains(&pause), "handed over, waiting for its window");
        // Shown, it isn't paused; hidden for the grace period, it is.
        let ws = Instance::workspace_of(&clamshell_library::key_of("desktop:firefox"));
        let window = |visible| Window { visible, ..win(6, "firefox", &ws, visible) };
        assert!(!s.apply(Event::Windows(vec![window(true)])).1.contains(&pause));
        assert!(!s.apply(Event::tick(t + GRACE + 1)).1.contains(&pause));
        s.apply(Event::Windows(vec![window(false)]));
        assert!(s.apply(Event::tick(t + 2 * GRACE + 1)).1.contains(&pause));
        // Shown again, it is resumed.
        assert!(s.apply(Event::Windows(vec![window(true)])).1.contains(&resume));
    }

    #[test]
    fn a_suspended_entry_stays_frozen_until_shown() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        // Set to Runs hidden, it is paused only by Suspend.
        s.prefs.settings_mut().set(Some(&"zelda-lttp".into()), Setting::Background, Some(InBackground::Runs.value()));
        let window = |visible| Window { unit: zelda.unit(), visible, ..win(3, "zelda3", &zelda.workspace(), visible) };
        s.apply(Event::tick(100));
        s.apply(Event::Windows(vec![window(true)]));
        assert!(s.suspendable("zelda-lttp"));
        let fx = s.suspend("zelda-lttp");
        assert!(fx.contains(&freeze(&key, true)) && fx.contains(&Effect::Window(crate::effect::WindowCommand::ShowShelf)), "{fx:?}");
        assert!(s.suspended("zelda-lttp") && !s.suspendable("zelda-lttp"));
        // Hidden, it stays frozen and isn't frozen twice.
        s.apply(Event::Windows(vec![window(false)]));
        assert!(!s.apply(Event::tick(100 + 3 * GRACE)).1.contains(&freeze(&key, true)));
        assert!(s.paused("zelda-lttp"));
        // Resume, in its menu, shows and thaws it.
        let fx = s.show_running("zelda-lttp");
        assert!(fx.contains(&freeze(&key, false)) && !s.suspended("zelda-lttp"), "{fx:?}");
        s.apply(Event::tick(100 + 4 * GRACE));
        assert!(!s.paused("zelda-lttp"), "runs hidden again");
        // A restarted ClamShell finds its slice frozen. It is held as suspended and not frozen again.
        let fx = s.apply(Event::Frozen(vec![key.as_str().into()])).1;
        assert!(!fx.contains(&freeze(&key, true)) && s.suspended("zelda-lttp") && s.paused("zelda-lttp"));
        assert!(!s.apply(Event::tick(100 + 6 * GRACE)).1.contains(&freeze(&key, false)), "stays frozen");
        // Quit thaws it so that it can end, and it is no longer suspended.
        assert!(s.quit(&"zelda-lttp".into()).contains(&freeze(&key, false)));
        assert!(!s.suspended("zelda-lttp"));
    }

    #[test]
    fn one_paused_whose_windows_go_is_thawed_to_end() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.prefs.settings_mut().set(Some(&"zelda-lttp".into()), Setting::Background, Some(InBackground::Pauses.value()));
        let window = |visible| Window { unit: zelda.unit(), visible, ..win(3, "zelda3", &zelda.workspace(), visible) };
        s.apply(Event::tick(100));
        s.apply(Event::Windows(vec![window(false)]));
        s.apply(Event::tick(100 + GRACE));
        assert!(s.paused("zelda-lttp"));
        // Its window is closed from outside, as by Steam's Stop. It is thawed so that it can end, and isn't
        // paused again.
        assert!(s.apply(Event::Windows(vec![])).1.contains(&freeze(&key, false)));
        s.apply(Event::tick(100 + 3 * GRACE));
        assert!(!s.paused("zelda-lttp"), "no window: none hidden");
    }

    #[test]
    fn games_pause_by_default_apps_run_and_all_says_so_for_both() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let fx = s.launch("desktop:foot".into());
        let foot = fx.iter().find_map(|e| if let Effect::Launch { instance, .. } = e { Some(instance.clone()) } else { None }).unwrap();
        crate::testkit::start(&mut s, "desktop:foot");
        let key = zelda.key().unwrap().to_owned();
        let windows = vec![
            Window { unit: zelda.unit(), ..win(3, "zelda3", &zelda.workspace(), false) },
            Window { unit: foot.unit(), ..win(4, "foot", &foot.workspace(), false) },
        ];
        s.apply(Event::tick(100));
        s.apply(Event::Windows(windows));
        let fx = s.apply(Event::tick(100 + GRACE)).1;
        assert!(fx.contains(&freeze(&key, true)), "a game: {fx:?}");
        assert!(!fx.contains(&freeze(foot.key().unwrap(), true)), "an app: {fx:?}");
        // With All set to run, the game runs too.
        s.prefs.settings_mut().set(None, Setting::Background, Some(InBackground::Runs.value()));
        assert!(s.apply(Event::tick(101 + GRACE)).1.contains(&freeze(&key, false)));
        assert_eq!(s.background_of("zelda-lttp"), InBackground::Runs);
    }

    #[test]
    fn quits_closed_ends_once_its_last_window_has_closed() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.prefs.settings_mut().set(Some(&"zelda-lttp".into()), Setting::Background, Some(InBackground::Quits.value()));
        // With no window yet, while starting, it is left alone.
        assert!(!s.apply(Event::Windows(vec![])).1.iter().any(|e| matches!(e, Effect::EndRun { .. })));
        s.apply(Event::Windows(vec![Window { unit: zelda.unit(), ..win(3, "zelda3", &zelda.workspace(), true) }]));
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(fx.contains(&Effect::EndRun { key: key.clone(), force: false }), "{fx:?}");
        // With the default setting, a program without a window runs on.
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Windows(vec![Window { unit: zelda.unit(), ..win(3, "zelda3", &zelda.workspace(), true) }]));
        assert!(!s.apply(Event::Windows(vec![])).1.iter().any(|e| matches!(e, Effect::EndRun { .. })));
        crate::testkit::end(&mut s, "zelda-lttp");
    }
}
