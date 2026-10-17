//! Programs: what settings, keymaps of their own and launches are about, a game of the library or a dock
//! app, alike. Each is known by an id: a game's own, or a dock app's key after `app:`. The program in
//! force is the one most recently in front (it stays while shelf or a window of no program is): its
//! settings are in force, its keymap is the pad's, its pad mouse setting decides the pad mouse. Once it
//! no longer runs, or (a game) is no longer the game, the game (if any) takes its place.

use std::sync::Arc;

use crate::State;
use crate::effect::Effect;
use crate::windows::Window;

/// What comes before a dock app's key in its program id.
const APP_PREFIX: &str = "app:";

/// A dock app's program id: `app:<key>`.
pub fn app_program(key: &str) -> Arc<str> {
    format!("{APP_PREFIX}{key}").into()
}

/// A program as the settings panel shows it: its id and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program<'a> {
    pub id: Arc<str>,
    pub title: &'a str,
}

impl State {
    /// The program a window belongs to: its running game's, or its dock app's.
    pub(crate) fn program_of(&self, w: &Window) -> Option<Arc<str>> {
        if let Some(g) = self.session.game_of(w) {
            return Some(g.id.clone());
        }
        let key = w.instance()?.app_key().map(str::to_owned)?;
        self.apps.list.iter().any(|a| a.key() == key).then(|| app_program(&key))
    }

    /// A program by its id, with its name: a game of the library, a dock app.
    pub fn program(&self, id: &Arc<str>) -> Option<Program<'_>> {
        if let Some(key) = id.strip_prefix(APP_PREFIX) {
            return self.apps.list.iter().find(|a| a.key() == key).map(|a| Program { id: id.clone(), title: &a.name });
        }
        self.game(id).map(|g| Program { id: id.clone(), title: &g.title })
    }

    /// Whether a program runs: a game, a dock app (any instance).
    fn program_runs(&self, id: &str) -> bool {
        match id.strip_prefix(APP_PREFIX) {
            Some(key) => self.session.units.iter().any(|u| u.app_key() == Some(key)),
            None => self.session.running_game(id).is_some(),
        }
    }

    /// The program in force (see the module's doc), if any.
    pub fn program_in_force(&self) -> Option<&Arc<str>> {
        self.session.program.as_ref()
    }

    /// After any event: the program in force follows the one in front (or, gone, the game running); its
    /// settings applied when it changed. Whether it changed.
    pub(crate) fn follow_program(&mut self) -> (bool, Vec<Effect>) {
        let front = self.session.windows.iter().find(|w| w.focused && !w.down).and_then(|w| self.program_of(w));
        let is_app = |p: &str| p.starts_with(APP_PREFIX);
        let kept = self.session.program.clone().filter(|p| self.program_runs(p) && (is_app(p) || self.session.current.as_ref() == Some(p)));
        let program = front.or(kept).or_else(|| self.session.current.clone());
        if program == self.session.program {
            return (false, vec![]);
        }
        self.session.program = program.clone();
        (true, vec![Effect::Apply { game: program, settings: self.in_force() }])
    }
}
