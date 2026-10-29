//! The views of ClamShell's own apps on the main screen, by their `BuiltinApp`: what each draws, and so
//! what it means to the pad (see `toolkit::PadOut`). What each app does is its definition in the state's
//! `builtin_apps`; an app missing here doesn't compile.

use clamshell_state::BuiltinApp;

use super::{device, library_view, setup};
use clamshell_ui::gfx::Rect;
use clamshell_ui::toolkit::Frame;

/// `app`'s view, which draws it over all of the screen's rect.
pub(super) fn view(app: BuiltinApp) -> fn(&mut Frame, Rect) {
    match app {
        BuiltinApp::Library => library_view::app,
        BuiltinApp::Device => device::app,
        BuiltinApp::Setup => setup::app,
    }
}
