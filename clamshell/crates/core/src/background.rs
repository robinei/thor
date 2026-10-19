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

use std::sync::Arc;

use crate::State;
use crate::effect::Effect;
use crate::instance::Instance;
use crate::running::Origin;
use crate::settings::Setting;

/// How long a program's windows are all hidden before it is paused (seconds): switching away and back
/// quickly doesn't pause it.
pub const GRACE: u64 = 3;

/// What the session keeps for it.
#[derive(Debug, Default)]
pub(crate) struct Background {
    /// Entries frozen, by key.
    pub(crate) paused: Vec<String>,
    /// Entries whose windows are all hidden, and since when (epoch seconds).
    pub(crate) hidden: Vec<(Arc<str>, u64)>,
    /// Entries that have had a window (Quits closed waits for one).
    pub(crate) windowed: Vec<Arc<str>>,
}

impl State {
    /// After the windows changed, and every second: entries paused, thawed or ended by their setting.
    pub(crate) fn follow_background(&mut self) -> Vec<Effect> {
        let now = self.unix_secs;
        let mut effects = Vec::new();
        let mut end = Vec::new();
        let session = &self.session;
        let mut kept = Background::default();
        // (Launches of ours: a Steam game's too, in its launch's own scope since it started.)
        for r in session.running.iter().filter(|r| r.origin == Origin::Ours && r.pid.is_none()) {
            let mine: Vec<&crate::windows::Window> = session.windows.iter().filter(|w| session.running_of(w).is_some_and(|o| o.id == r.id)).collect();
            let shown = mine.iter().any(|w| w.visible);
            let had = self.background.windowed.contains(&r.id) || !mine.is_empty();
            if had {
                kept.windowed.push(r.id.clone());
            }
            let paused = self.background.paused.contains(&r.key);
            let setting = self.settings_of(&r.id).get(Setting::Background);
            match setting {
                1 if !shown && had => {
                    let since = self.background.hidden.iter().find(|(id, _)| *id == r.id).map_or(now, |(_, t)| *t);
                    kept.hidden.push((r.id.clone(), since));
                    if paused || now.saturating_sub(since) >= GRACE {
                        if !paused {
                            effects.push(Effect::Freeze { slice: Instance::slice_of(&r.key), frozen: true });
                        }
                        kept.paused.push(r.key.clone());
                    }
                }
                _ if paused => effects.push(Effect::Freeze { slice: Instance::slice_of(&r.key), frozen: false }),
                2 if had && mine.is_empty() && !self.session.quitting.iter().any(|(g, _)| *g == r.id) => end.push(r.id.clone()),
                _ => {}
            }
        }
        self.background = kept;
        for id in end {
            effects.extend(self.quit(&id));
        }
        effects
    }

    /// An entry thawed before it is asked anything (closed, ended, shown), if it is paused; it isn't
    /// paused again until hidden for [`GRACE`] anew.
    pub(crate) fn thaw(&mut self, id: &str) -> Option<Effect> {
        let key = self.session.running(id)?.key.clone();
        let at = self.background.paused.iter().position(|k| *k == key)?;
        self.background.paused.remove(at);
        self.background.hidden.retain(|(e, _)| &**e != id);
        Some(Effect::Freeze { slice: Instance::slice_of(&key), frozen: false })
    }

    /// Whether an entry is paused (frozen while hidden).
    pub fn paused(&self, id: &str) -> bool {
        self.session.running(id).is_some_and(|r| self.background.paused.contains(&r.key))
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
        s.apply(GameEvent::Played { game: "zelda-lttp".into(), start: 0, duration: 1 }.into());
    }
}
