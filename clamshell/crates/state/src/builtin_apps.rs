//! What ClamShell's own apps on the main screen do, each declared in one place: whether it always runs or
//! only while open, where it goes on the dock, whether it is the home, what is read from the system while
//! it runs, and its hooks: which of its views shows, what a key typed does, and what the settings panel
//! edits while it is in front. Shared code (the session, the dock, sync, the keyboard, the settings
//! panel) reads these instead of asking which app it is.
//! An app's rules are its own area's (the Device view's in [`crate::setup`]), its targets its own `Id`
//! variant, and how it looks its view in clamshell-ui, found by its [`BuiltinApp`]. Who each is, as a
//! library entry (its id, title and summary), is [`BuiltinApp`]'s.

use std::sync::Arc;

use clamshell_library::BuiltinApp;

use crate::State;
use crate::bottom::keyboard::Stroke;
use crate::effect::Watch;
use crate::out::Out;

/// What one of ClamShell's own apps does.
pub struct BuiltinAppDefinition {
    pub runs: Runs,
    pub dock: DockPlace,
    /// Home goes to it, it is in front when nothing else is, and the pick of a file is made in it.
    pub home: bool,
    /// What is read from the system while it runs.
    pub watches: &'static [Watch],
    /// Which of its views shows, a number of its own. When it changes, the focus starts where the new
    /// view says, as when another app comes in front (see [`crate::Top::App`]).
    pub view: fn(&State) -> u8,
    /// What a key typed on the second screen's keyboard does while it is in front.
    pub(crate) typed: fn(&mut State, Stroke, &mut Out),
    /// The entry the settings panel edits while it is in front, if any.
    pub subject: fn(&State) -> Option<Arc<str>>,
    /// For an app that shows by itself rather than being opened: whether it is needed now. While it is,
    /// it runs and is in front, over anything; when it no longer is, it closes.
    pub needed: Option<fn(&State) -> bool>,
}

/// Nothing, for a hook an app doesn't need.
fn no_view(_: &State) -> u8 {
    0
}

fn not_typed(_: &mut State, _: Stroke, _: &mut Out) {}

fn no_subject(_: &State) -> Option<Arc<str>> {
    None
}

/// When an app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    Always,
    /// From when it is shown until it is closed (Close, Quit).
    WhileOpen,
}

/// Where an app is on the dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPlace {
    /// Pinned, first unless moved; it can't be unpinned.
    PinnedFirst,
    /// While it runs, where a running entry goes.
    WhileRunning,
    /// Never on the dock.
    Never,
}

/// The Library (see [`crate::library_view`]): its list, its edit mode, or a pick.
const LIBRARY: BuiltinAppDefinition = BuiltinAppDefinition {
    runs: Runs::Always,
    dock: DockPlace::PinnedFirst,
    home: true,
    watches: &[],
    view: |s| match (s.library_view.pick(), s.library_view.edit()) {
        (Some(_), _) => 2,
        (None, Some(_)) => 1,
        (None, None) => 0,
    },
    // Finds an entry in the list by what is typed; the edit mode's list isn't found in.
    typed: |s, stroke, out| {
        if s.library_view.edit().is_none() || s.library_view.pick().is_some() {
            s.library_view.find(stroke, out);
        }
    },
    // The selection, where a browsed file that belongs to a library entry stands for that entry.
    subject: |s| s.library_view.selected().cloned().map(|id| s.library_view.entry_of_file(&id).unwrap_or(id)),
    needed: None,
};

/// The Device view (see [`crate::setup`]).
const DEVICE: BuiltinAppDefinition = BuiltinAppDefinition {
    runs: Runs::WhileOpen,
    dock: DockPlace::WhileRunning,
    home: false,
    watches: &[Watch::Account],
    view: no_view,
    typed: not_typed,
    subject: no_subject,
    needed: None,
};

/// Setup, the first start's steps (see [`crate::setup::wizard`]): shown while the account or the first
/// start needs them; its view is the step. Keys typed go to its editor alone.
const SETUP: BuiltinAppDefinition = BuiltinAppDefinition {
    runs: Runs::WhileOpen,
    dock: DockPlace::Never,
    home: false,
    watches: &[Watch::Account],
    view: |s| s.setup.wizard().map_or(0, |step| step as u8),
    typed: not_typed,
    subject: no_subject,
    needed: Some(|s| s.setup.wizard().is_some()),
};

/// `app`'s definition.
pub fn definition(app: BuiltinApp) -> &'static BuiltinAppDefinition {
    match app {
        BuiltinApp::Library => &LIBRARY,
        BuiltinApp::Device => &DEVICE,
        BuiltinApp::Setup => &SETUP,
    }
}

/// Whether entry `id` is an app always pinned to the dock, which can't be unpinned.
pub fn always_pinned(id: &str) -> bool {
    BuiltinApp::of(id).is_some_and(|app| definition(app).dock == DockPlace::PinnedFirst)
}

/// The app needed now, if one is (see [`BuiltinAppDefinition::needed`]).
pub(crate) fn needed(s: &State) -> Option<BuiltinApp> {
    BuiltinApp::ALL.into_iter().find(|app| definition(*app).needed.is_some_and(|needed| needed(s)))
}

/// The home app (see [`BuiltinAppDefinition::home`]).
pub fn home() -> BuiltinApp {
    BuiltinApp::ALL.into_iter().find(|app| definition(*app).home).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_one_home_and_it_always_runs() {
        let homes: Vec<BuiltinApp> = BuiltinApp::ALL.into_iter().filter(|app| definition(*app).home).collect();
        assert_eq!(homes, [home()]);
        assert_eq!(definition(home()).runs, Runs::Always);
    }
}
