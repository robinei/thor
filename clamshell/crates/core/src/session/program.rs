//! The program in force: the running entry most recently in front. It stays in force while shelf, or a
//! window that isn't ours, is in front, as long as it runs. Once it stops, the entry started last that
//! still runs takes over. Its settings are in force, its keymap is the pad's, and its pad mouse setting
//! decides the pad mouse.

use std::sync::Arc;

use crate::State;
use crate::effect::Effect;
use crate::session::windows::Window;

/// A program as the settings panel shows it: its entry's id and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program<'a> {
    pub id: Arc<str>,
    pub title: &'a str,
}

impl State {
    /// The running entry a window belongs to, by its id.
    pub(crate) fn program_of(&self, w: &Window) -> Option<Arc<str>> {
        self.session.running_of(w).map(|r| r.id.clone())
    }

    /// An entry by its id, with its name.
    pub fn program(&self, id: &Arc<str>) -> Option<Program<'_>> {
        self.entry(id).map(|g| Program { id: id.clone(), title: &g.title })
    }

    /// The program in force (see the module's doc), if any.
    pub fn program_in_force(&self) -> Option<&Arc<str>> {
        self.session.program.as_ref()
    }

    /// Runs after any event. The program in force follows the one in front, or, when that is gone, the
    /// latest that runs. Its settings are applied when it changes. Returns whether it changed.
    pub(crate) fn follow_program(&mut self) -> (bool, Vec<Effect>) {
        let session = &self.session;
        let front = session.windows.iter().find(|w| w.focused && !w.down).and_then(|w| self.program_of(w));
        let kept = session.program.clone().filter(|p| session.running(p).is_some());
        let program = front.or(kept).or_else(|| session.running.last().map(|r| r.id.clone()));
        if program == session.program {
            return (false, vec![]);
        }
        self.session.program = program.clone();
        (true, vec![Effect::Apply { entry: program, settings: self.in_force() }])
    }
}
