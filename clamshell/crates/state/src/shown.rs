//! What each screen shows, worked out from the areas' facts when asked. Everything that depends on what
//! is shown (the views, the pad, the rules) reads it here, so they can't disagree.

use crate::State;
use crate::setup::wizard::Step;

/// What the main screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    /// The first start's steps, in the Library's place (see [`crate::setup::wizard`]).
    Setup(Step),
    /// A window in front: one of a running entry (`program`), or another.
    Window { program: bool },
    /// The Device view.
    Device,
    /// The Library, as it is.
    Library(LibraryMode),
}

/// What the Library shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryMode {
    Browse,
    /// The edit mode (see [`crate::library_view::edit`]).
    Edit,
    /// A pick, in the edit mode or not (see [`crate::library_view::pick`]).
    Pick,
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
        if self.session.shown_builtin() == Some(clamshell_library::BuiltinApp::Device) {
            return Top::Device;
        }
        Top::Library(if self.library_view.pick().is_some() {
            LibraryMode::Pick
        } else if self.library_view.edit().is_some() {
            LibraryMode::Edit
        } else {
            LibraryMode::Browse
        })
    }
}
