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

use clamshell_library::{Entry, Library, RunnerOf};

use crate::Session;
use crate::effect::Effect;
use crate::out::Out;
use crate::session::instance::Instance;
use crate::session::running::{Origin, Running};
use crate::settings::{InBackground, Settings};

/// How long, in seconds, a program's windows are all hidden before it is paused, so that switching away
/// and back quickly doesn't pause it.
pub const GRACE: u64 = 10;

/// What the session keeps for pausing: entries' slices and runners, both by key. Each is paused once it
/// has been out of use for the grace period.
#[derive(Debug, Default)]
pub(crate) struct Background {
    entries: Pauses,
    /// The runners whose entries are paused together, from the library.
    runners: Vec<RunnerOf>,
    runner_pauses: Pauses,
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
    fn follow(&mut self, key: &str, in_use: bool, now: u64) -> Option<bool> {
        if in_use {
            return self.resume(key).then_some(false);
        }
        let since = match self.unused.iter().find(|(k, _)| &**k == key) {
            Some((_, t)) => *t,
            None => {
                self.unused.push((key.into(), now));
                now
            }
        };
        if self.is_paused(key) || now.saturating_sub(since) < GRACE {
            return None;
        }
        self.paused.push((key.into(), Why::Hidden));
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
    fn hold(&mut self, key: &str) -> bool {
        let was = self.is_paused(key);
        self.paused.retain(|(k, _)| &**k != key);
        self.paused.push((key.into(), Why::Held));
        !was
    }

    fn is_paused(&self, key: &str) -> bool {
        self.paused.iter().any(|(k, _)| &**k == key)
    }

    fn is_held(&self, key: &str) -> bool {
        self.paused.iter().any(|(k, why)| &**k == key && *why == Why::Held)
    }

    /// Keeps following only the keys `kept` holds. The others' entries ended, so they are forgotten, not
    /// resumed.
    fn keep(&mut self, kept: impl Fn(&str) -> bool) {
        self.unused.retain(|(k, _)| kept(k));
        self.paused.retain(|(k, _)| kept(k));
    }
}

impl Session {
    /// What entry or level `id` does while hidden ([`crate::settings::Setting::Background`]). If it is set
    /// to Pauses games, the default, it is paused if it is a game (see [`Entry::is_game`]), or for a
    /// runner's level if all of the runner's entries are games, and runs otherwise. A game that minds,
    /// such as one played online, can be set to run.
    pub fn background_of(&self, id: &str, library: &Library, settings: &Settings) -> InBackground {
        let entry = self.entry(library, id);
        let runner = entry.and_then(|g| g.runner.as_ref()).map(|r| &*r.key);
        let games = || match entry {
            Some(g) => g.is_game(),
            None => {
                let mut of_runner = library.entries.iter().filter(|g| g.runner.as_ref().is_some_and(|r| &*r.key == id)).peekable();
                of_runner.peek().is_some() && of_runner.all(Entry::is_game)
            }
        };
        match settings.of_in(id, runner.as_slice()).choice::<InBackground>() {
            InBackground::PausesGames if games() => InBackground::Pauses,
            InBackground::PausesGames => InBackground::Runs,
            b => b,
        }
    }

    /// Runs after the windows changed, and every second. Pauses, thaws or ends entries by their setting.
    pub(crate) fn follow_background(&mut self, now: u64, library: &Library, settings: &Settings, out: &mut Out) {
        // Launches of ours, including a Steam game's, which is in its launch's own scope since it started.
        let ours = |r: &&Running| r.origin == Origin::Ours;
        let running = &self.running;
        self.background.entries.keep(|key| running.iter().filter(ours).any(|r| r.key == key));
        let mut end = Vec::new();
        for r in self.running.iter().filter(ours) {
            let (has, shown) = {
                let mut mine = self.windows_of(&r.id).peekable();
                (mine.peek().is_some(), mine.any(|w| w.visible))
            };
            let setting = self.background_of(&r.id, library, settings);
            // An entry that is quitting isn't paused, since frozen it couldn't end. Nor is one without a
            // window: it has none hidden, and it is starting, between windows or ending, as with Steam's
            // Stop.
            let quitting = self.is_quitting(&r.id);
            let hidden = setting == InBackground::Pauses && !shown && has && !quitting;
            // A suspended entry stays frozen until shown.
            let in_use = if self.background.entries.is_held(&r.key) { shown } else { !hidden };
            if let Some(frozen) = self.background.entries.follow(&r.key, in_use, now) {
                out.effects.push(Effect::Freeze { slice: Instance::slice_of(&r.key), frozen });
            }
            // Quits closed ends it once it has had a window and has none left.
            if setting == InBackground::Quits && r.seen && !has && !quitting {
                end.push(r.id.clone());
            }
        }
        for id in end {
            self.quit(&id, library, out);
        }
        self.follow_runner_pauses(now, library, settings, out);
    }

    /// The runners whose entries are paused together: those of the library's entries with a `pause`.
    pub(crate) fn follow_pausable_runners(&mut self, library: &Library) {
        let runners = &mut self.background.runners;
        runners.clear();
        for r in library.entries.iter().filter_map(|g| g.runner.as_ref()).filter(|r| !r.pause.is_empty()) {
            if !runners.iter().any(|o| o.key == r.key) {
                runners.push(r.clone());
            }
        }
    }

    /// Pauses each runner whose entries are paused together, when it is set to Pauses hidden and none of
    /// its entries has been shown for the grace period. None may be starting either, that is launched with
    /// no window yet, since paused it would never draw one. Resumes the runner as one is shown, or when its
    /// setting changes.
    fn follow_runner_pauses(&mut self, now: u64, library: &Library, settings: &Settings, out: &mut Out) {
        for runner in &self.background.runners {
            let pauses = self.background_of(&runner.key, library, settings) == InBackground::Pauses;
            let of_runner = |r: &Running| r.entry.as_ref().and_then(|g| g.runner.as_ref()).is_some_and(|o| o.key == runner.key);
            let shown = self.windows.iter().filter(|w| w.visible).any(|w| self.running_of(w).is_some_and(of_runner));
            let starting = self.running.iter().filter(|r| of_runner(r)).any(|r| !r.found() && !r.seen);
            match self.background.runner_pauses.follow(&runner.key, !pauses || shown || starting, now) {
                Some(true) => out.effects.push(Effect::RunnerCommand(runner.pause.to_vec())),
                Some(false) => out.effects.push(Effect::RunnerCommand(runner.resume.to_vec())),
                None => {}
            }
        }
    }

    /// Resumes the runner of entry `id` before the entry is launched or shown, if the runner is paused.
    pub(crate) fn resume_runner_of(&mut self, id: &str, library: &Library, out: &mut Out) {
        let Some(runner) = super::entry_in(library, &self.running, id).and_then(|g| g.runner.as_ref()) else {
            return;
        };
        if self.background.runner_pauses.resume(&runner.key) {
            out.effects.push(Effect::RunnerCommand(runner.resume.to_vec()));
        }
    }

    /// Thaws an entry, if it is paused, before it is asked anything: to close, to end or to be shown.
    pub(crate) fn thaw(&mut self, id: &str, out: &mut Out) {
        let Some(r) = self.running.iter().find(|r| &*r.id == id) else {
            return;
        };
        if self.background.entries.resume(&r.key) {
            out.effects.push(Effect::Freeze { slice: Instance::slice_of(&r.key), frozen: false });
        }
    }

    /// Whether entry `id` can be suspended: it runs in a slice of ours, and isn't suspended.
    pub fn suspendable(&self, id: &str) -> bool {
        self.running(id).is_some_and(|r| r.origin == Origin::Ours && !self.suspended(id))
    }

    pub fn suspended(&self, id: &str) -> bool {
        self.running(id).is_some_and(|r| self.background.entries.is_held(&r.key))
    }

    /// Suspends entry `id`: freezes it now (see the module's doc) and hides its windows if one is shown.
    pub(crate) fn suspend(&mut self, id: &str, out: &mut Out) {
        if !self.suspendable(id) {
            return;
        }
        if self.windows_of(id).any(|w| w.visible) {
            out.effects.push(Effect::Window(crate::effect::WindowCommand::ShowShelf));
        }
        let Some(r) = self.running.iter().find(|r| &*r.id == id) else {
            return;
        };
        if self.background.entries.hold(&r.key) {
            out.effects.push(Effect::Freeze { slice: Instance::slice_of(&r.key), frozen: true });
        }
    }

    /// Holds our slices found frozen at start, by key, as suspended, leaving them frozen.
    pub(crate) fn found_frozen(&mut self, keys: Vec<Arc<str>>) {
        for key in keys {
            self.background.entries.hold(&key);
        }
    }

    /// Whether an entry is paused, that is frozen while hidden.
    pub fn paused(&self, id: &str) -> bool {
        self.running(id).is_some_and(|r| self.background.entries.is_paused(&r.key))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::effect::WindowCommand;
    use crate::session::windows::Window;
    use crate::settings::{Choice, Setting};
    use crate::testkit::*;
    use crate::{Event, State};

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
        assert!(s.session().paused("zelda-lttp"));
        assert!(!s.apply(Event::tick(101 + GRACE)).1.contains(&freeze(&key, true)), "once");
        // Shown again, it is thawed.
        assert!(s.apply(Event::Windows(vec![window(true)])).1.contains(&freeze(&key, false)));
        assert!(!s.session().paused("zelda-lttp"));
        // Paused and then shown from the dock, it is thawed before its workspace is shown.
        s.apply(Event::Windows(vec![window(false)]));
        let t = 101 + 2 * GRACE;
        s.apply(Event::tick(t));
        assert!(s.session().paused("zelda-lttp"));
        assert_eq!(show(&mut s, "zelda-lttp"), vec![freeze(&key, false), Effect::Window(WindowCommand::Focus(zelda.workspace()))]);
        // It is paused again after being hidden for the grace period anew. Quitting thaws it, so that it
        // can answer its window's closing.
        s.apply(Event::tick(t + 1));
        s.apply(Event::tick(t + GRACE));
        assert!(!s.session().paused("zelda-lttp"), "the grace counts again from the thaw");
        s.apply(Event::tick(t + 1 + GRACE));
        assert!(s.session().paused("zelda-lttp"));
        let id: Arc<str> = "zelda-lttp".into();
        assert_eq!(quit(&mut s, &id), vec![freeze(&key, false), Effect::Window(WindowCommand::Close(vec![3]))]);
        // With its window gone it ends, and it isn't paused meanwhile even though it is hidden.
        s.apply(Event::Windows(vec![]));
        s.apply(Event::tick(t + 3 * GRACE));
        assert!(!s.session().paused("zelda-lttp"), "quitting");
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
        let fx = launch_now(&mut s, "desktop:firefox");
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
        assert!(s.session().suspendable("zelda-lttp"));
        let mut out = crate::out::Out::default();
        s.session.suspend("zelda-lttp", &mut out);
        let fx = out.effects;
        assert!(fx.contains(&freeze(&key, true)) && fx.contains(&Effect::Window(crate::effect::WindowCommand::ShowShelf)), "{fx:?}");
        assert!(s.session().suspended("zelda-lttp") && !s.session().suspendable("zelda-lttp"));
        // Hidden, it stays frozen and isn't frozen twice.
        s.apply(Event::Windows(vec![window(false)]));
        assert!(!s.apply(Event::tick(100 + 3 * GRACE)).1.contains(&freeze(&key, true)));
        assert!(s.session().paused("zelda-lttp"));
        // Resume, in its menu, shows and thaws it.
        let fx = show(&mut s, "zelda-lttp");
        assert!(fx.contains(&freeze(&key, false)) && !s.session().suspended("zelda-lttp"), "{fx:?}");
        s.apply(Event::tick(100 + 4 * GRACE));
        assert!(!s.session().paused("zelda-lttp"), "runs hidden again");
        // A restarted ClamShell finds its slice frozen. It is held as suspended and not frozen again.
        let fx = s.apply(Event::Frozen(vec![key.as_str().into()])).1;
        assert!(!fx.contains(&freeze(&key, true)) && s.session().suspended("zelda-lttp") && s.session().paused("zelda-lttp"));
        assert!(!s.apply(Event::tick(100 + 6 * GRACE)).1.contains(&freeze(&key, false)), "stays frozen");
        // Quit thaws it so that it can end, and it is no longer suspended.
        assert!(quit(&mut s, "zelda-lttp").contains(&freeze(&key, false)));
        assert!(!s.session().suspended("zelda-lttp"));
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
        assert!(s.session().paused("zelda-lttp"));
        // Its window is closed from outside, as by Steam's Stop. It is thawed so that it can end, and isn't
        // paused again.
        assert!(s.apply(Event::Windows(vec![])).1.contains(&freeze(&key, false)));
        s.apply(Event::tick(100 + 3 * GRACE));
        assert!(!s.session().paused("zelda-lttp"), "no window: none hidden");
    }

    #[test]
    fn games_pause_by_default_apps_run_and_all_says_so_for_both() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let fx = launch_now(&mut s, "desktop:foot");
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
        assert_eq!(s.session().background_of("zelda-lttp", s.shelf().library(), s.prefs().settings()), InBackground::Runs);
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
