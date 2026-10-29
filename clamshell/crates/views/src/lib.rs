//! ClamShell's screens, drawn in immediate mode with clamshell-ui's toolkit, and given to its framework as
//! [`VIEWS`]. Each is a function of the state and the screen's size that
//! draws everything and lists what can be tapped or focused. The main screen shows the Library. The second
//! screen shows the bar with the open tab below it, or nothing while it is dark. There is one module per
//! tab. They are drawn with the toolkit (`clamshell_ui::toolkit`): the frame, style, widgets, text formatting and
//! scrolled lists.
//!
//! Everything here is in layout units: logical pixels from the top left of the area inside the burn-in
//! margin (see `render`). Sizes are fractions of the screen or of the element they sit in. Sizes that
//! define a layout, such as a row's height, a column's width or a gap, are named constants in their
//! module, or in `style` when shared. Small adjustments, such as half a padding or slightly larger text,
//! are written where they are used.

mod apps;
mod bar;
mod battery;
mod bluetooth;
mod curves;
mod dark;
mod device;
mod dock;
mod editor;
mod keyboard;
mod keymaps;
mod levels;
mod library_edit;
mod library_view;
mod notifications;
mod overlay;
mod profiles;
mod radio;
mod settings;
mod setup;
mod sysinfo;
mod tabs;
mod time;
mod touchpad;
mod wifi;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod frame_times;
#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod no_alloc;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod pad_tests;

use clamshell_state::bottom::editor::Purpose;
use clamshell_state::library_view::files::FileName;
use clamshell_state::{Tab, Top};

use clamshell_ui::gfx::Rect;
use clamshell_ui::toolkit::Frame;

/// ClamShell's views, for the framework to draw (see [`clamshell_ui::Views`]).
pub const VIEWS: clamshell_ui::Views = clamshell_ui::Views { top, bottom, overlay };

/// How much the black layer over the area below the bar hides it while that area is a touchpad.
const POINTING_FADE: f32 = 0.7;

/// The main screen, over all of `screen`: the app of ClamShell's in front, or Setup in the home app's place.
/// The frame starts black (see `render`).
pub fn top(f: &mut Frame, screen: Rect) {
    f.body = screen;
    match f.s.top() {
        Top::App { app, .. } => apps::view(app)(f, screen),
        // The window is shown over the view, which isn't drawn.
        Top::Window { .. } => {}
    }
}

/// Over what `which` shows, over all of `screen`: on the top screen the HUD and the window arrows, over
/// the window; on the bottom screen the notifications. The frame starts clear (see `render`).
pub fn overlay(f: &mut Frame, screen: Rect, which: clamshell_ui::Screen) {
    f.body = screen;
    match which {
        clamshell_ui::Screen::Top => overlay::draw(f, screen),
        clamshell_ui::Screen::Bottom => notifications::draw(f, screen),
    }
}

/// The keyboard, or over it the editor open: what it edits says how it is titled.
fn typing(f: &mut Frame, body: Rect) {
    match f.s.bottom().editor().map(|e| &e.purpose) {
        Some(Purpose::FileName(FileName::Rename(_))) => editor::draw(f, "New name", "Rename"),
        Some(Purpose::FileName(FileName::NewFolder(_))) => editor::draw(f, "New folder's name", "Make"),
        Some(Purpose::FileName(FileName::Extract(_))) => editor::draw(f, "Extract to", "Extract"),
        Some(Purpose::AccountPassword(_)) => device::draw_password(f),
        Some(Purpose::PlaceName(clamshell_state::library_view::edit::PlaceName::Rename(_))) => editor::draw(f, "New name", "Rename"),
        Some(Purpose::PlaceName(clamshell_state::library_view::edit::PlaceName::New(None))) => editor::draw(f, "New tab's name", "Make"),
        Some(Purpose::PlaceName(clamshell_state::library_view::edit::PlaceName::New(Some(_)))) => editor::draw(f, "New folder's name", "Make"),
        Some(Purpose::FormField(field, _)) => editor::draw(f, field.label(), "Set"),
        Some(Purpose::SaveName) => editor::draw(f, "Save as", "Set"),
        Some(Purpose::Password { ssid }) => {
            let title = clamshell_ui::toolkit::format::Fmt::<128>::of(format_args!("Password for {ssid}"));
            editor::draw(f, &title, "Join");
        }
        _ => keyboard::draw(f, body),
    }
}

/// The windows' tab: the window moved down, which the compositor shows below the bar, or with none
/// there, how to move one there.
fn windows(f: &mut Frame, body: Rect) {
    if f.s.bottom().shows_windows(f.s.session()) {
        return;
    }
    let size = f.text(clamshell_ui::toolkit::style::Text::Body);
    let line = size * 1.6;
    let mid = body.y + body.h / 2.0;
    let at = |y: f32| Rect::new(body.x, y, body.w, line);
    f.c.text_in(at(mid - line), size, clamshell_ui::toolkit::style::Tone::Normal, "Nothing here", clamshell_ui::gfx::Align::Center);
    f.c.text_in(
        at(mid),
        size * 0.85,
        clamshell_ui::toolkit::style::Tone::Dim,
        "Home with the right stick down moves a window here",
        clamshell_ui::gfx::Align::Center,
    );
}

/// The second screen: the bar, the open tab below it, and the slider over both while one is moving.
/// While a game wants the screen dark, nothing is drawn; while Setup shows, only the keyboard and its
/// editor, over the whole screen. The frame starts black (see `render`).
pub fn bottom(f: &mut Frame, screen: Rect) {
    f.body = screen;
    if f.s.setup().wizard().is_some() {
        typing(f, screen);
        return;
    }
    if f.s.bottom_dark() {
        dark::draw(f, screen);
        f.out.touchpad_area = Some(screen);
        return;
    }
    let body = bar::draw(f, screen);
    f.body = body;
    tabs::view(f.s.bottom().tab())(f);
    // While pointing, the area below the bar is a touchpad and its contents are faded. The touchpad tab
    // has nothing to fade.
    if f.s.bottom().pointing() && f.s.bottom().tab() != Tab::Touchpad {
        f.c.fill(body, clamshell_ui::gfx::Rgb(0).alpha(POINTING_FADE));
    }
    bar::draw_slider_over(f, screen, body.y);
    f.out.touchpad_area = Some(body);
    // Dimmed when untouched for a while and the pad is in use elsewhere, as the screens are before they
    // go off.
    if f.s.bottom_dimmed() {
        f.c.fill(screen, clamshell_ui::gfx::Rgb(0).alpha(clamshell_ui::DIM));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;
