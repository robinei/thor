//! What ClamShell's own apps on the main screen do, each declared in one place: whether it always runs or
//! only while open, where it goes on the dock, whether it is the home, and what is read from the system
//! while it runs. Shared code (the session, the dock, sync) reads these instead of asking which app it is.
//! An app's rules are its own area's (the Device view's in [`crate::setup`]), its targets its own `Id`
//! variant, and how it looks its view in clamshell-ui, found by its [`BuiltinApp`]. Who each is, as a
//! library entry (its id, title and summary), is [`BuiltinApp`]'s.

use clamshell_library::BuiltinApp;

use crate::effect::Watch;

/// What one of ClamShell's own apps does.
#[derive(Debug)]
pub struct BuiltinAppDefinition {
    pub runs: Runs,
    pub dock: DockPlace,
    /// Home goes to it, it is in front when nothing else is, and the pick of a file is made in it.
    pub home: bool,
    /// What is read from the system while it runs.
    pub watches: &'static [Watch],
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
}

const LIBRARY: BuiltinAppDefinition = BuiltinAppDefinition { runs: Runs::Always, dock: DockPlace::PinnedFirst, home: true, watches: &[] };

const DEVICE: BuiltinAppDefinition = BuiltinAppDefinition { runs: Runs::WhileOpen, dock: DockPlace::WhileRunning, home: false, watches: &[Watch::Account] };

/// `app`'s definition.
pub fn definition(app: BuiltinApp) -> &'static BuiltinAppDefinition {
    match app {
        BuiltinApp::Library => &LIBRARY,
        BuiltinApp::Device => &DEVICE,
    }
}

/// Whether entry `id` is an app always pinned to the dock, which can't be unpinned.
pub fn always_pinned(id: &str) -> bool {
    BuiltinApp::of(id).is_some_and(|app| definition(app).dock == DockPlace::PinnedFirst)
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
