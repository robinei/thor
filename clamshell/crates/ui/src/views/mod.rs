//! The screens, drawn in immediate mode. Each is a function of the state and the screen's size that
//! draws everything and lists what can be tapped or focused. The main screen shows the shelf. The second
//! screen shows the bar with the open tab below it, or nothing while it is dark. There is one module per
//! tab. They are drawn with the toolkit (`crate::toolkit`): the frame, style, widgets, text formatting and
//! scrolled lists.
//!
//! Everything here is in layout units: logical pixels from the top left of the area inside the burn-in
//! margin (see `render`). Sizes are fractions of the screen or of the element they sit in. Sizes that
//! define a layout, such as a row's height, a column's width or a gap, are named constants in their
//! module, or in `style` when shared. Small adjustments, such as half a padding or slightly larger text,
//! are written where they are used.

pub(crate) mod bar;
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
mod notifications;
mod overlay;
mod profiles;
mod radio;
mod settings;
mod shelf;
mod sysinfo;
mod time;
mod touchpad;
mod wifi;

use clamshell_state::Tab;
use clamshell_state::bottom::editor::Purpose;
use clamshell_state::shelf::files::FileName;

use crate::gfx::Rect;
use crate::toolkit::Frame;

/// How much the black layer over the area below the bar hides it while that area is a touchpad.
const POINTING_FADE: f32 = 0.7;
/// How much the black layer over the second screen hides it while it is dimmed. A third is left, as with
/// the screens' brightness when idle dims them.
const BOTTOM_DIM: f32 = 2.0 / 3.0;

/// The main screen, over all of `screen`: the shelf, or the Device view shown in its place. The frame
/// starts black (see `render`).
pub fn top(f: &mut Frame, screen: Rect) {
    f.body = screen;
    match f.s.device().top_view() {
        clamshell_state::device::TopView::Shelf => f.out.list_rows = Some(shelf::draw(f, screen)),
        clamshell_state::device::TopView::Device => device::draw(f, screen),
    }
}

/// Over what `which` shows, over all of `screen`: on the top screen the HUD and the window arrows, over
/// the window; on the bottom screen the notifications. The frame starts clear (see `render`).
pub fn overlay(f: &mut Frame, screen: Rect, which: crate::Screen) {
    f.body = screen;
    match which {
        crate::Screen::Top => overlay::draw(f, screen),
        crate::Screen::Bottom => notifications::draw(f, screen),
    }
}

/// The windows' tab: the window moved down, which the compositor shows below the bar, or with none
/// there, how to move one there.
fn windows(f: &mut Frame, body: Rect) {
    if f.s.bottom().shows_windows(f.s.session()) {
        return;
    }
    let size = f.text(crate::toolkit::style::Text::Body);
    let line = size * 1.6;
    let mid = body.y + body.h / 2.0;
    let at = |y: f32| Rect::new(body.x, y, body.w, line);
    f.c.text_in(at(mid - line), size, crate::toolkit::style::Tone::Normal, "Nothing here", crate::gfx::Align::Center);
    f.c.text_in(at(mid), size * 0.85, crate::toolkit::style::Tone::Dim, "Home with the right stick down moves a window here", crate::gfx::Align::Center);
}

/// The second screen: the bar, the open tab below it, and the slider over both while one is moving.
/// While a game wants the screen dark, nothing is drawn. The frame starts black (see `render`).
pub fn bottom(f: &mut Frame, screen: Rect) {
    f.body = screen;
    if f.s.bottom_dark() {
        dark::draw(f, screen);
        f.out.touchpad_area = Some(screen);
        return;
    }
    let body = bar::draw(f, screen);
    f.body = body;
    match f.s.bottom().tab() {
        Tab::Settings => settings::draw(f),
        // While a browsed file's name or the account password is being typed, the editor replaces the
        // keyboard.
        Tab::Keyboard => match f.s.bottom().editor().map(|e| &e.purpose) {
            Some(Purpose::FileName(FileName::Rename(_))) => editor::draw(f, "New name", "Rename"),
            Some(Purpose::FileName(FileName::NewFolder(_))) => editor::draw(f, "New folder's name", "Make"),
            Some(Purpose::FileName(FileName::Extract(_))) => editor::draw(f, "Extract to", "Extract"),
            Some(Purpose::AccountPassword(_)) => device::draw_password(f),
            _ => keyboard::draw(f, body),
        },
        Tab::Dock => dock::draw(f),
        Tab::Touchpad => touchpad::draw(f),
        Tab::Sysinfo => sysinfo::draw(f),
        Tab::Wifi => wifi::draw(f),
        Tab::Battery => battery::draw(f),
        Tab::Bluetooth => bluetooth::draw(f),
        Tab::Volume => levels::draw_volume(f),
        Tab::Brightness => levels::draw_brightness(f),
        Tab::Time => time::draw(f),
        Tab::Windows => windows(f, body),
    }
    // While pointing, the area below the bar is a touchpad and its contents are faded. The touchpad tab
    // has nothing to fade.
    if f.s.bottom().pointing() && f.s.bottom().tab() != Tab::Touchpad {
        f.c.fill(body, crate::gfx::Rgb(0).alpha(POINTING_FADE));
    }
    bar::draw_slider_over(f, screen, body.y);
    f.out.touchpad_area = Some(body);
    // Dimmed when untouched for a while and the pad is in use elsewhere, as the screens are before they
    // go off.
    if f.s.bottom().dimmed() {
        f.c.fill(screen, crate::gfx::Rgb(0).alpha(BOTTOM_DIM));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests;
