//! The views of the second screen's tabs, below the bar, by their `Tab`. What each tab is (what it watches,
//! where its button is) is its definition in the state (`Tab::definition`), and its button is the bar's;
//! a tab missing here doesn't compile.

use clamshell_state::Tab;
use clamshell_ui::toolkit::Frame;

use super::{battery, bluetooth, dock, levels, settings, sysinfo, time, touchpad, wifi};

/// `tab`'s view, which draws it in the frame's body, the area below the bar.
pub(super) fn view(tab: Tab) -> fn(&mut Frame) {
    match tab {
        Tab::Settings => settings::draw,
        // While a browsed file's name or the account password is being typed, the editor replaces the
        // keyboard.
        Tab::Keyboard => |f| {
            let body = f.body();
            super::typing(f, body);
        },
        Tab::Dock => dock::draw,
        Tab::Touchpad => touchpad::draw,
        Tab::Sysinfo => sysinfo::draw,
        Tab::Wifi => wifi::draw,
        Tab::Battery => battery::draw,
        Tab::Bluetooth => bluetooth::draw,
        Tab::Volume => levels::draw_volume,
        Tab::Brightness => levels::draw_brightness,
        Tab::Time => time::draw,
        Tab::Windows => |f| {
            let body = f.body();
            super::windows(f, body);
        },
    }
}
