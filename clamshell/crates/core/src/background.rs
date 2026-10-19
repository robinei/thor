//! What a program does while none of its windows is shown: its setting In the background
//! ([`Setting::Background`], by the settings' levels: All, its runner, itself).
//!
//! - Runs hidden (the default): nothing is done.
//! - Pauses hidden: once none of its windows has been shown for [`GRACE`] seconds, its slice (all its
//!   launches' processes) is frozen; shown again (a window of it visible, or it is asked to be shown),
//!   it is thawed. A program that has shown no window yet (starting) isn't paused.
//! - Quits closed: once it has had a window and has none left, it is ended (its slice stopped, as Quit
//!   does once the windows are closed): a program that would stay in the background, Steam in its tray,
//!   doesn't.
//!
//! Only entries ClamShell launched have a slice of their own: those found by their windows or by a tray
//! icon run in another program's processes, and run on as they are. A paused entry is thawed before
//! anything is asked of it: Quit (its windows asked to close), Force kill, shown.
//!
//! A runner whose entries share one container (Waydroid's: Android's apps) has commands to pause it
//! all and resume it (its `pause`, `resume`). Set for the runner to Pauses hidden, the container is
//! paused once none of its entries has been shown for [`GRACE`] seconds (none running, too), and resumed
//! as one is shown or launched.

use std::sync::Arc;

use crate::State;
use clamshell_library::RunnerOf;

use crate::effect::Effect;
use crate::instance::Instance;
use crate::running::Origin;
use crate::settings::Setting;

/// How long a program's windows are all hidden before it is paused (seconds): switching away and back
/// quickly doesn't pause it.
pub const GRACE: u64 = 3;

/// What the session keeps for it: entries' slices (by key) and runners (by their key), each paused once
/// out of use for the grace.
#[derive(Debug, Default)]
pub(crate) struct Background {
    pub(crate) entries: Pauses,
    /// The runners whose entries are paused together (from the library).
    pub(crate) runners: Vec<RunnerOf>,
    pub(crate) runner_pauses: Pauses,
}

/// Things paused while out of use (an entry's slice, a runner's container): since when each has been out
/// of use, and those paused.
#[derive(Debug, Default)]
pub(crate) struct Pauses {
    unused: Vec<(Arc<str>, u64)>,
    paused: Vec<Arc<str>>,
}

impl Pauses {
    /// `key`, in use now or not: `Some(true)`: to be paused (out of use for the grace); `Some(false)`: to
    /// be resumed (paused, in use again).
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
        if self.paused.contains(key) || now.saturating_sub(since) < GRACE {
            return None;
        }
        self.paused.push(key.clone());
        Some(true)
    }

    /// `key` in use: whether it was paused (it is to be resumed). Paused again only once out of use for
    /// the grace anew.
    fn resume(&mut self, key: &str) -> bool {
        self.unused.retain(|(k, _)| &**k != key);
        let was = self.paused.iter().any(|k| &**k == key);
        self.paused.retain(|k| &**k != key);
        was
    }

    fn is_paused(&self, key: &str) -> bool {
        self.paused.iter().any(|k| &**k == key)
    }

    /// Only these keys followed (the others' entries ended: forgotten, not resumed).
    fn keep(&mut self, keys: &[Arc<str>]) {
        self.unused.retain(|(k, _)| keys.contains(k));
        self.paused.retain(|k| keys.contains(k));
    }
}

impl State {
    /// After the windows changed, and every second: entries paused, thawed or ended by their setting.
    pub(crate) fn follow_background(&mut self) -> Vec<Effect> {
        let now = self.unix_secs;
        let mut effects = Vec::new();
        let mut end = Vec::new();
        let session = &self.session;
        // (Launches of ours: a Steam game's too, in its launch's own scope since it started.)
        let ours: Vec<&crate::running::Running> = session.running.iter().filter(|r| r.origin == Origin::Ours && r.pid.is_none()).collect();
        let keys: Vec<Arc<str>> = ours.iter().map(|r| Arc::from(r.key.as_str())).collect();
        self.background.entries.keep(&keys);
        for (r, key) in ours.into_iter().zip(&keys) {
            let mine: Vec<&crate::windows::Window> = session.windows_of(&r.id).collect();
            let shown = mine.iter().any(|w| w.visible);
            let setting = self.settings_of(&r.id).get(Setting::Background);
            // (One quitting isn't paused: frozen, it couldn't end. Nor one without a window: none hidden,
            // it is starting, between windows, or ending, Steam's Stop for one.)
            let quitting = session.quitting.iter().any(|(g, ..)| *g == r.id);
            let hidden = setting == 1 && !shown && !mine.is_empty() && !quitting;
            if let Some(frozen) = self.background.entries.follow(key, !hidden, now) {
                effects.push(Effect::Freeze { slice: Instance::slice_of(key), frozen });
            }
            // (Quits closed: once it has had a window and has none left.)
            if setting == 2 && r.seen && mine.is_empty() && !quitting {
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
        for r in self.shelf.library().games.iter().filter_map(|g| g.runner.as_ref()).filter(|r| !r.pause.is_empty()) {
            if !runners.iter().any(|o| o.key == r.key) {
                runners.push(r.clone());
            }
        }
        self.background.runners = runners;
    }

    /// Each runner paused together: paused once none of its entries has been shown for the grace (set to
    /// Pauses hidden), none starting either (launched, no window yet: it would never draw one); resumed
    /// as one is shown (or its setting changed).
    fn follow_runner_pauses(&mut self) -> Vec<Effect> {
        let now = self.unix_secs;
        let mut effects = Vec::new();
        for runner in &self.background.runners {
            let pauses = self.settings.of_in(&runner.key, &[]).get(Setting::Background) == 1;
            let session = &self.session;
            let of_runner = |r: &crate::running::Running| r.entry.as_ref().and_then(|g| g.runner.as_ref()).is_some_and(|o| o.key == runner.key);
            let shown = session.windows.iter().filter(|w| w.visible).any(|w| session.running_of(w).is_some_and(of_runner));
            let starting = session.running.iter().filter(|r| of_runner(r)).any(|r| r.origin != Origin::Tray && !r.seen);
            match self.background.runner_pauses.follow(&runner.key, !pauses || shown || starting, now) {
                Some(true) => effects.push(Effect::RunnerCommand(runner.pause.to_vec())),
                Some(false) => effects.push(Effect::RunnerCommand(runner.resume.to_vec())),
                None => {}
            }
        }
        effects
    }

    /// Before an entry of a paused runner is launched or shown: the runner resumed.
    pub(crate) fn resume_runner_of(&mut self, id: &str) -> Option<Effect> {
        let runner = self.game(id)?.runner.as_ref()?;
        let resume = runner.resume.to_vec();
        self.background.runner_pauses.resume(&runner.key.clone()).then_some(Effect::RunnerCommand(resume))
    }

    /// An entry thawed before it is asked anything (closed, ended, shown), if it is paused.
    pub(crate) fn thaw(&mut self, id: &str) -> Option<Effect> {
        let key = self.session.running(id)?.key.clone();
        self.background.entries.resume(&key).then(|| Effect::Freeze { slice: Instance::slice_of(&key), frozen: false })
    }

    /// Whether an entry is paused (frozen while hidden).
    pub fn paused(&self, id: &str) -> bool {
        self.session.running(id).is_some_and(|r| self.background.entries.is_paused(&r.key))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::effect::WindowCommand;
    use crate::testkit::*;
    use crate::windows::Window;
    use crate::{Event, GameEvent};

    fn freeze(key: &str, frozen: bool) -> Effect {
        Effect::Freeze { slice: Instance::slice_of(key), frozen }
    }

    #[test]
    fn pauses_hidden_thawed_when_shown_or_asked() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.settings.set(Some(&"zelda-lttp".into()), Setting::Background, Some(1));
        let window = |visible| Window { unit: zelda.unit(), visible, ..win(3, "zelda3", &zelda.workspace(), visible) };
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::Windows(vec![window(true)]));
        // Hidden: paused once hidden for the grace period.
        let fx = s.apply(Event::Windows(vec![window(false)])).1;
        assert!(!fx.contains(&freeze(&key, true)));
        assert!(!s.apply(Event::Tick { unix_secs: 102 }).1.contains(&freeze(&key, true)), "within the grace");
        assert!(s.apply(Event::Tick { unix_secs: 103 }).1.contains(&freeze(&key, true)));
        assert!(s.paused("zelda-lttp"));
        assert!(!s.apply(Event::Tick { unix_secs: 104 }).1.contains(&freeze(&key, true)), "once");
        // Shown again: thawed.
        assert!(s.apply(Event::Windows(vec![window(true)])).1.contains(&freeze(&key, false)));
        assert!(!s.paused("zelda-lttp"));
        // Paused, then shown from the dock: thawed before its workspace is.
        s.apply(Event::Windows(vec![window(false)]));
        s.apply(Event::Tick { unix_secs: 110 });
        assert!(s.paused("zelda-lttp"));
        assert_eq!(s.show_running("zelda-lttp"), vec![freeze(&key, false), Effect::Window(WindowCommand::Focus(zelda.workspace()))]);
        // Paused again (hidden for the grace anew since), then quit: thawed, to answer its window's closing.
        s.apply(Event::Tick { unix_secs: 120 });
        assert!(!s.paused("zelda-lttp"), "the grace counts again from the thaw");
        s.apply(Event::Tick { unix_secs: 123 });
        assert!(s.paused("zelda-lttp"));
        let id: Arc<str> = "zelda-lttp".into();
        assert_eq!(s.quit(&id), vec![freeze(&key, false), Effect::Window(WindowCommand::Close(vec![3]))]);
        // Its window gone, it ends: not paused meanwhile, though hidden.
        s.apply(Event::Windows(vec![]));
        s.apply(Event::Tick { unix_secs: 130 });
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
        let mut library = clamshell_library::demo_library();
        library.games.iter_mut().find(|g| &*g.id == "desktop:firefox").unwrap().runner = Some(runner.clone());
        let mut s = State::new(crate::Init { library, ..init() });
        s.settings.set(Some(&runner.key), Setting::Background, Some(1));
        let (pause, resume) = (Effect::RunnerCommand(vec!["pause".into()]), Effect::RunnerCommand(vec!["resume".into()]));
        // Nothing of it running: paused after the grace.
        s.apply(Event::Tick { unix_secs: 100 });
        assert!(!s.apply(Event::Tick { unix_secs: 102 }).1.contains(&pause));
        assert!(s.apply(Event::Tick { unix_secs: 103 }).1.contains(&pause));
        assert!(!s.apply(Event::Tick { unix_secs: 104 }).1.contains(&pause), "once");
        // Launched: resumed first.
        let fx = s.launch("desktop:firefox".into());
        assert_eq!(fx.first(), Some(&resume), "{fx:?}");
        s.apply(GameEvent::Started("desktop:firefox".into()).into());
        // Starting (its launch, then handed over, no window yet): not paused.
        assert!(!s.apply(Event::Tick { unix_secs: 107 }).1.contains(&pause), "its launch runs");
        s.apply(GameEvent::Ended("desktop:firefox".into()).into());
        assert!(!s.apply(Event::Tick { unix_secs: 109 }).1.contains(&pause), "handed over, waiting for its window");
        // Shown: not paused; hidden for the grace: paused.
        let ws = Instance::workspace_of(&clamshell_library::key_of("desktop:firefox"));
        let window = |visible| Window { visible, ..win(6, "firefox", &ws, visible) };
        assert!(!s.apply(Event::Windows(vec![window(true)])).1.contains(&pause));
        assert!(!s.apply(Event::Tick { unix_secs: 110 }).1.contains(&pause));
        s.apply(Event::Windows(vec![window(false)]));
        assert!(s.apply(Event::Tick { unix_secs: 113 }).1.contains(&pause));
        // Shown again: resumed.
        assert!(s.apply(Event::Windows(vec![window(true)])).1.contains(&resume));
    }

    #[test]
    fn one_paused_whose_windows_go_is_thawed_to_end() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.settings.set(Some(&"zelda-lttp".into()), Setting::Background, Some(1));
        let window = |visible| Window { unit: zelda.unit(), visible, ..win(3, "zelda3", &zelda.workspace(), visible) };
        s.apply(Event::Tick { unix_secs: 100 });
        s.apply(Event::Windows(vec![window(false)]));
        s.apply(Event::Tick { unix_secs: 103 });
        assert!(s.paused("zelda-lttp"));
        // Its window closed from outside (Steam's Stop): thawed, to end; not paused again.
        assert!(s.apply(Event::Windows(vec![])).1.contains(&freeze(&key, false)));
        s.apply(Event::Tick { unix_secs: 110 });
        assert!(!s.paused("zelda-lttp"), "no window: none hidden");
    }

    #[test]
    fn quits_closed_ends_once_its_last_window_has_closed() {
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        let key = zelda.key().unwrap().to_owned();
        s.settings.set(Some(&"zelda-lttp".into()), Setting::Background, Some(2));
        // No window yet (starting): left.
        assert!(!s.apply(Event::Windows(vec![])).1.iter().any(|e| matches!(e, Effect::EndRun { .. })));
        s.apply(Event::Windows(vec![Window { unit: zelda.unit(), ..win(3, "zelda3", &zelda.workspace(), true) }]));
        let fx = s.apply(Event::Windows(vec![])).1;
        assert!(fx.contains(&Effect::EndRun { key: key.clone(), force: false }), "{fx:?}");
        // Runs hidden (the default): a program without a window runs on.
        let mut s = state();
        let zelda = launch(&mut s, "zelda-lttp");
        s.apply(Event::Windows(vec![Window { unit: zelda.unit(), ..win(3, "zelda3", &zelda.workspace(), true) }]));
        assert!(!s.apply(Event::Windows(vec![])).1.iter().any(|e| matches!(e, Effect::EndRun { .. })));
        s.apply(GameEvent::Ended("zelda-lttp".into()).into());
    }
}
