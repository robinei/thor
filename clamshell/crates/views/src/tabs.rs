//! The second screen's tabs as the views have them, by their `Tab`: each one's view below the bar and its
//! button in the bar. What each tab is (what it watches, where its button is) is its definition in the
//! state (`Tab::definition`); a tab missing here doesn't compile.

use clamshell_state::levels::slider::Control;
use clamshell_state::{Id, State, Tab};
use clamshell_ui::gfx::icons::Icon;
use clamshell_ui::toolkit::Frame;
use clamshell_ui::toolkit::style::Tone;

use super::{battery, bluetooth, dock, levels, settings, sysinfo, time, touchpad, wifi};

/// A tab's view and button.
pub(super) struct TabView {
    /// Draws the tab in the frame's body, below the bar.
    pub draw: fn(&mut Frame),
    /// Its button in the bar.
    pub button: fn(&State) -> Button,
    /// Whether what it shows fades while the area below the bar is a touchpad.
    pub fades: bool,
}

/// A tab's button in the bar, as it looks while the tab isn't open (open, it is lit).
pub(super) struct Button {
    /// Its target. Volume and brightness target their controls, where a tap opens the tab and a slide
    /// sets the level.
    pub id: Id,
    /// Its icon with its level or status, or the time for the clock's.
    pub face: Face,
    /// Dim without a reading, or as the tab's state says.
    pub tone: Tone,
}

pub(super) enum Face {
    Icon(Icon),
    Clock,
}

/// Normal with a reading, dim without one.
fn reading(on: bool) -> Tone {
    if on { Tone::Normal } else { Tone::Dim }
}

/// A button showing `icon`, dim, for a tab with no reading to show.
fn plain(tab: Tab, icon: Icon) -> Button {
    Button { id: Id::Tab(tab), face: Face::Icon(icon), tone: Tone::Dim }
}

/// The battery's level below which its button stands out, unless charging.
const LOW_BATTERY: u8 = 15;

/// `tab`'s view and button.
pub(super) fn view(tab: Tab) -> TabView {
    let tab_view = |draw: fn(&mut Frame), button: fn(&State) -> Button| TabView { draw, button, fades: true };
    match tab {
        Tab::Settings => tab_view(settings::draw, |_| plain(Tab::Settings, Icon::Sliders)),
        // While a browsed file's name or the account password is being typed, the editor replaces the
        // keyboard.
        Tab::Keyboard => tab_view(
            |f| {
                let body = f.body();
                super::typing(f, body);
            },
            |_| plain(Tab::Keyboard, Icon::Keyboard),
        ),
        Tab::Dock => tab_view(dock::draw, |_| plain(Tab::Dock, Icon::Apps)),
        // It is the touchpad: nothing to fade.
        Tab::Touchpad => TabView { fades: false, ..tab_view(touchpad::draw, |_| plain(Tab::Touchpad, Icon::Touchpad)) },
        Tab::Sysinfo => tab_view(sysinfo::draw, |_| plain(Tab::Sysinfo, Icon::Sysinfo)),
        Tab::Wifi => tab_view(wifi::draw, |s| {
            let quality = s.sys().status().wifi;
            Button { id: Id::Tab(Tab::Wifi), face: Face::Icon(Icon::wifi(quality)), tone: reading(quality.is_some()) }
        }),
        // An empty, dim battery when there is none, and the accent colour when low.
        Tab::Battery => tab_view(battery::draw, |s| {
            let status = &s.sys().status();
            let (icon, tone) = match status.battery {
                Some(level) => {
                    let low = level <= LOW_BATTERY && !status.charging;
                    (Icon::battery(level, status.charging), if low { Tone::Accent } else { Tone::Normal })
                }
                None => (Icon::Battery { tenths: 0, charging: false }, Tone::Dim),
            };
            Button { id: Id::Tab(Tab::Battery), face: Face::Icon(icon), tone }
        }),
        Tab::Bluetooth => tab_view(bluetooth::draw, |s| {
            let bt = s.radios().bluetooth();
            let connected = bt.connected() > 0;
            Button { id: Id::Tab(Tab::Bluetooth), face: Face::Icon(Icon::Bluetooth { connected }), tone: reading(bt.on && connected) }
        }),
        Tab::Volume => tab_view(levels::draw_volume, |s| {
            let volume = s.control(Control::Volume);
            let icon = Icon::volume(volume.unwrap_or(0), s.levels().muted());
            Button { id: Id::Control(Control::Volume), face: Face::Icon(icon), tone: reading(volume.is_some()) }
        }),
        Tab::Brightness => tab_view(levels::draw_brightness, |s| {
            let lit = s.control(Control::Brightness).is_some();
            Button { id: Id::Control(Control::Brightness), face: Face::Icon(Icon::Brightness), tone: reading(lit) }
        }),
        // Its button is the clock.
        Tab::Time => tab_view(time::draw, |_| Button { id: Id::Tab(Tab::Time), face: Face::Clock, tone: Tone::Normal }),
        // Its button stands out while a window of the program in front is below behind another tab, and is
        // dim with nothing below.
        Tab::Windows => tab_view(
            |f| {
                let body = f.body();
                super::windows(f, body);
            },
            |s| {
                let tone = if s.bottom().hidden_down(s.session()).is_some() {
                    Tone::Fn
                } else if s.session().bottom_window().is_some() {
                    Tone::Normal
                } else {
                    Tone::Dim
                };
                Button { id: Id::Tab(Tab::Windows), face: Face::Icon(Icon::Windows), tone }
            },
        ),
    }
}
