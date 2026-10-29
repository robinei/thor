//! What each screen shows, worked out from the areas' facts when asked. Everything that depends on what
//! is shown (the views, the pad, the rules) reads it here, so they can't disagree.

use std::sync::Arc;

use clamshell_library::BuiltinApp;

use crate::State;
use crate::setup::wizard::Step;

/// What the main screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    /// The first start's steps, in the home app's place (see [`crate::setup::wizard`]).
    Setup(Step),
    /// A window in front: one of a running entry (`program`), or another.
    Window { program: bool },
    /// One of ClamShell's own apps, showing its view `view` (see
    /// [`crate::builtin_apps::BuiltinAppDefinition::view`]).
    App { app: BuiltinApp, view: u8 },
}

impl State {
    /// What the main screen shows.
    pub fn top(&self) -> Top {
        if let Some(step) = self.setup.wizard() {
            return Top::Setup(step);
        }
        if let Some(w) = self.session.top_window() {
            return Top::Window { program: self.session.running_of(w).is_some() };
        }
        let app = self.session.shown_builtin().unwrap_or_else(crate::builtin_apps::home);
        Top::App { app, view: (crate::builtin_apps::definition(app).view)(self) }
    }

    /// The entry the settings panel edits: the program's whose window is in front, or what the app in front
    /// says (see [`crate::builtin_apps::BuiltinAppDefinition::subject`]).
    pub(crate) fn subject(&self) -> Option<Arc<str>> {
        match self.top() {
            Top::Window { program: true } => self.session.top_window().and_then(|w| self.session.owner_of(w)).or_else(|| self.session.program().cloned()),
            Top::App { app, .. } => (crate::builtin_apps::definition(app).subject)(self),
            Top::Setup(_) | Top::Window { program: false } => None,
        }
    }
}
