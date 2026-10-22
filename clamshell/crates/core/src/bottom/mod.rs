//! The second screen. Its tabs are chosen from the bar, and the last one chosen stays, also across
//! restarts. This module also holds what belongs to the screen rather than to a tab's subject: the
//! editor over the keyboard, the settings panel's scope, the keyboard a text field brought up, and the
//! screen going dark while a game runs.
//!
//! Volume and brightness open from their controls, and Bluetooth, Wi-Fi and Battery from their status
//! icons. All of them are ordinary tabs.

pub mod editor;
pub mod keyboard;
pub mod notice;
pub mod screen;
pub mod touch;

use crate::effect::Watch;
use crate::event::Reading;
use crate::kit::names::names;
use crate::settings::panel::Scope;
use crate::{Effect, State};
use editor::{Editor, Editors};
use notice::Notice;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    Settings,
    /// The apps at hand and what is running, with the game first.
    Dock,
    Keyboard,
    Touchpad,
    #[default]
    Sysinfo,
    Volume,
    Brightness,
    Wifi,
    Bluetooth,
    Battery,
    /// The clock's tab, for the time zone.
    Time,
}

// In the bar's order.
names!(Tab {
    Settings: "settings";
    Dock: "dock";
    Keyboard: "keyboard";
    Touchpad: "touchpad";
    Sysinfo: "sysinfo";
    Volume: "volume";
    Brightness: "brightness";
    Bluetooth: "bluetooth";
    Wifi: "wifi";
    Battery: "battery";
    Time: "time";
});

impl Tab {
    /// Whether the tab has its own button in the bar. The time tab's button is the clock.
    pub fn in_bar(self) -> bool {
        self != Tab::Time
    }

    /// What the tab watches while open, meaning what is read from the system for it.
    pub fn watch(self) -> Option<Watch> {
        match self {
            Tab::Volume => Some(Watch::Audio),
            Tab::Brightness => Some(Watch::Backlights),
            Tab::Wifi => Some(Watch::Wifi),
            Tab::Bluetooth => Some(Watch::Bluetooth),
            Tab::Battery => Some(Watch::Battery),
            Tab::Time => Some(Watch::Time),
            Tab::Sysinfo => Some(Watch::Metrics),
            Tab::Settings | Tab::Dock | Tab::Keyboard | Tab::Touchpad => None,
        }
    }

    /// Whether the second screen shows `reading` with this tab open. The bar always shows the status
    /// and Bluetooth, and each tab shows its own readings.
    pub fn shows(self, reading: &Reading) -> bool {
        match reading {
            Reading::Status(_) | Reading::Bluetooth(_) => true,
            // For an editor's Paste, in any tab that has one.
            Reading::Clipboard(_) => true,
            // Shown in the dock.
            Reading::Icon { .. } => true,
            Reading::Metrics(_) => matches!(self, Tab::Sysinfo | Tab::Battery),
            Reading::Frames { .. } => self == Tab::Sysinfo,
            Reading::Wifi(_) => self == Tab::Wifi,
            Reading::Battery(_) => self == Tab::Battery,
            Reading::Audio(_) => self == Tab::Volume,
            Reading::Backlights(_) => self == Tab::Brightness,
            // The clock shows it too.
            Reading::Time(_) => true,
            // Shown in the Device view, on the main screen.
            Reading::Account(_) => false,
        }
    }
}

/// The second screen's own state.
#[derive(Debug, Clone, Default)]
pub struct Bottom {
    pub(crate) tab: Tab,
    /// When the second screen was last touched or had the pad's focus, in monotonic seconds as in
    /// [`crate::Event::Tick`]. None before the clock's first reading. Also whether the screen is drawn
    /// dimmed (see [`crate::State::bottom_dimmed`]).
    pub(crate) used_at: Option<u64>,
    pub(crate) dimmed: bool,
    /// The tab that was open when an app's text field brought up the keyboard. The screen goes back to it
    /// when the field is done, unless the user chose a tab in the meantime.
    pub(crate) raised_from: Option<Tab>,
    pub(crate) editors: Editors,
    /// What the settings panel edits. It is global whenever there is no game in context.
    pub(crate) scope: Scope,
    /// The darkened second screen was tapped awake. This lasts until the next game.
    pub(crate) woken: bool,
    /// What the bottom screen shows, ClamShell's view or a window moved down, and whether it is off.
    pub(crate) screen: crate::bottom::screen::BottomScreen,
    /// What a key just did, such as change a level or take a screenshot, shown under the bar for a
    /// moment.
    pub(crate) notice: Option<Notice>,
    /// Whether the area below the bar is pointing (see [`crate::bottom::touch`]).
    pub(crate) pointing: bool,
    /// The text on the system clipboard, for Ctrl+V in the editor. Files copied in the Files tab use their
    /// own clipboard (see [`crate::shelf::files`]).
    pub(crate) clipboard: Option<std::sync::Arc<str>>,
}

impl Bottom {
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Whether the area below the bar is pointing (see [`crate::bottom::touch`]). It always is in the
    /// touchpad tab.
    pub fn pointing(&self) -> bool {
        self.pointing || self.tab == Tab::Touchpad
    }

    /// The editor open in the tab shown, if any.
    pub fn editor(&self) -> Option<&Editor> {
        self.editors.on(self.tab)
    }

    pub(crate) fn editor_mut(&mut self) -> Option<&mut Editor> {
        self.editors.on_mut(self.tab)
    }

    /// Closes the editor of the tab shown and returns it.
    pub(crate) fn close_editor(&mut self) -> Option<Editor> {
        self.editors.close(self.tab)
    }

    /// The editors, for a rule that opens one.
    pub(crate) fn editors_mut(&mut self) -> &mut Editors {
        &mut self.editors
    }

    /// The editors and the notice, for a rule that may open an editor or show a notice.
    pub(crate) fn editors_and_notice(&mut self) -> (&mut Editors, &mut Option<Notice>) {
        (&mut self.editors, &mut self.notice)
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The tab to come back to after a restart. This is the tab that was chosen, not the keyboard a text
    /// field brought up.
    pub fn chosen_tab(&self) -> Tab {
        self.raised_from.unwrap_or(self.tab)
    }
}

impl State {
    /// The user chose a tab, by its icon, by tapping a control, or with the shoulder buttons. The tab
    /// stays, even if a text field had brought up the keyboard.
    pub(crate) fn choose_tab(&mut self, tab: Tab) -> Vec<Effect> {
        if self.bottom.raised_from.take().is_some() {
            self.saves.resume.touch(self.clock.mono());
        }
        self.set_tab(tab)
    }

    /// Changes the second screen's tab, and its watch follows (see [`State::follow_watch`]). Any level
    /// being set is released. Whatever the old tab had open, such as an editor or the keymap screens,
    /// stays there for coming back to.
    pub(crate) fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        if tab == self.bottom.tab {
            return vec![];
        }
        self.saves.resume.touch(self.clock.mono());
        self.bottom.tab = tab;
        self.dock.menu = None;
        self.dock.moving = None;
        // No level row of the tab left is being set any more.
        self.levels.release();
        // The time tab opens on its main page again.
        self.sys.reset_time_page();
        vec![]
    }

    /// A text field wants typing, or is done. The keyboard comes up, and when the field is done the
    /// screen goes back to the tab it came from, unless the user chose a tab in the meantime.
    pub(crate) fn text_input(&mut self, active: bool) -> Vec<Effect> {
        if active {
            // Not over an open editor, since what is typed there would be lost.
            if self.bottom.tab == Tab::Keyboard || self.bottom.editor().is_some() {
                return vec![];
            }
            self.bottom.raised_from = Some(self.bottom.tab);
            return self.set_tab(Tab::Keyboard);
        }
        match self.bottom.raised_from.take() {
            Some(tab) if self.bottom.tab == Tab::Keyboard => self.set_tab(tab),
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::*;
    use crate::*;

    #[test]
    fn the_touchpad_tab_always_points() {
        let mut s = state();
        assert_eq!(tap(&mut s, Id::Tab(Tab::Touchpad)), vec![Effect::Watch(Watch::Metrics, false)], "nothing to watch");
        assert!(s.bottom().pointing());
        s.apply(crate::bottom::touch::Pointing::Idle.into());
        assert!(s.bottom().pointing(), "idle or not");
        let wifi = |on| Effect::Watch(Watch::Wifi, on);
        assert_eq!(tap(&mut s, Id::Tab(Tab::Wifi)), vec![wifi(true)]);
        assert!(!s.bottom().pointing());
        assert_eq!(tap(&mut s, Id::Tab(Tab::Touchpad)), vec![wifi(false)], "the old tab's off");
    }

    #[test]
    fn a_text_field_brings_up_the_keyboard_and_back() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Touchpad));
        s.apply(Event::Windows(vec![foot()]));
        let (dirty, fx) = s.apply(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!((dirty, fx), (Dirty::BOTH, vec![Effect::PadRoute(PadRoute::Keys)]));
        // The app in front has the pad again, as a game would.
        let back = vec![Effect::PadRoute(PadRoute::Game)];
        assert_eq!(s.apply(Event::TextInput(false)).1, back);
        assert_eq!(s.bottom().tab(), Tab::Touchpad, "back where it came from");
    }

    #[test]
    fn a_tab_chosen_while_the_keyboard_is_up_stays() {
        let mut s = state();
        s.apply(Event::Windows(vec![foot()]));
        // Even the keyboard's own.
        s.apply(Event::TextInput(true));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        s.apply(Event::TextInput(false));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(s.resume().tab, "keyboard");
        // Already on the keyboard, so there is nothing to go back to.
        tap(&mut s, Id::Tab(Tab::Dock));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        s.apply(Event::TextInput(true));
        s.apply(Event::TextInput(false));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
    }

    #[test]
    fn any_program_asking_for_text_brings_up_the_keyboard() {
        let mut s = state();
        crate::testkit::start(&mut s, "zelda-lttp");
        s.apply(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "a game too");
    }
}
