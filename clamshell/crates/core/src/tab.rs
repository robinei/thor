//! The second screen: its tabs, chosen from the bar (the last one chosen stays, also across restarts),
//! and what belongs to the screen rather than to a tab's subject: the editor over the keyboard, the
//! settings panel's scope, the keyboard a text field brought up, and the screen being dark while a game
//! runs. Volume and brightness open from their controls, Bluetooth, Wi-Fi and Battery from their status
//! icons; all are tabs alike.

use crate::editor::Editor;
use crate::effect::Watch;
use crate::event::Reading;
use crate::names::names;
use crate::panel::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    Settings,
    /// The apps at hand and what runs (the game first).
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
    /// The clock's: the time zone.
    Time,
}

// (In the bar's order.)
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
    /// Whether it has a button of its own in the bar (the time tab's is the clock).
    pub fn in_bar(self) -> bool {
        self != Tab::Time
    }

    /// What the tab watches while open (the system read for it).
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

    /// Whether the second screen shows `reading` with this tab open: the bar shows the status and
    /// Bluetooth always, the tabs their own readings.
    pub fn shows(self, reading: &Reading) -> bool {
        match reading {
            Reading::Status(_) | Reading::Bluetooth(_) => true,
            // (An editor's Paste, in any tab that has one.)
            Reading::Clipboard(_) => true,
            // (The dock's.)
            Reading::Icon { .. } => true,
            Reading::Metrics(_) => matches!(self, Tab::Sysinfo | Tab::Battery),
            Reading::Frames { .. } => self == Tab::Sysinfo,
            Reading::Wifi(_) => self == Tab::Wifi,
            Reading::Battery(_) => self == Tab::Battery,
            Reading::Audio(_) => self == Tab::Volume,
            Reading::Backlights(_) => self == Tab::Brightness,
            // (The clock shows it too.)
            Reading::Time(_) => true,
            // (The Device view's, on the main screen.)
            Reading::Account(_) => false,
        }
    }
}

/// The second screen's own state.
#[derive(Debug, Clone, Default)]
pub struct Bottom {
    pub(crate) tab: Tab,
    /// When the second screen was last touched, or the pad's focus was on it ([`crate::mono_secs`]; none
    /// before the clock's first reading), and whether it is drawn dimmed (see
    /// [`crate::State::bottom_dimmed`]).
    pub(crate) used_at: Option<u64>,
    pub(crate) dimmed: bool,
    /// The tab the keyboard was brought up from by an app's text field: back there when the field is
    /// done, unless the user chose a tab meanwhile.
    pub(crate) raised_from: Option<Tab>,
    /// The texts being edited (a game's environment, a Wi-Fi password), each in its own tab, kept while
    /// another tab is shown (at most one a tab).
    pub(crate) editors: Vec<Editor>,
    /// What the settings panel edits (global whenever there is no game in context).
    pub(crate) scope: Scope,
    /// The keymaps' screen open over the settings tab (see [`crate::keymap_screens`]).
    pub(crate) keymaps: Option<crate::keymap_screens::Screen>,
    /// The edit screen's layer, selection and confirmation.
    pub(crate) keymap_edit: crate::keymap_screens::EditView,
    /// The keymap being edited as it was before each change since its edit screen opened (Undo).
    pub(crate) keymap_undo: crate::keymap_screens::Undo,
    /// The dark second screen was tapped awake (until the next game).
    pub(crate) woken: bool,
    /// What the bottom screen shows (ClamShell's view or a window moved down), and whether it is off.
    pub(crate) screen: crate::bottom_screen::BottomScreen,
    /// What the time tab shows: its main page, the regions, or a region's zones.
    pub(crate) time_page: crate::time::TimePage,
    /// What a key just did (a level, a screenshot), shown under the bar for a moment.
    pub(crate) notice: Option<crate::hardware::Notice>,
    /// The area below the bar points (see [`crate::touch`]).
    pub(crate) pointing: bool,
}

impl Bottom {
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Whether the area below the bar points (see [`crate::touch`]): always in the touchpad tab.
    pub fn pointing(&self) -> bool {
        self.pointing || self.tab == Tab::Touchpad
    }

    /// The editor open in the tab shown, if any.
    pub fn editor(&self) -> Option<&Editor> {
        self.editors.iter().find(|e| e.tab() == self.tab)
    }

    pub(crate) fn editor_mut(&mut self) -> Option<&mut Editor> {
        let tab = self.tab;
        self.editors.iter_mut().find(|e| e.tab() == tab)
    }

    /// Opens an editor in its tab (one there before is replaced).
    pub(crate) fn open_editor(&mut self, editor: Editor) {
        self.editors.retain(|e| e.tab() != editor.tab());
        self.editors.push(editor);
    }

    /// Closes the editor of the tab shown, giving it back.
    pub(crate) fn close_editor(&mut self) -> Option<Editor> {
        let tab = self.tab;
        let i = self.editors.iter().position(|e| e.tab() == tab)?;
        Some(self.editors.remove(i))
    }

    pub fn notice(&self) -> Option<&crate::hardware::Notice> {
        self.notice.as_ref()
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The tab to come back to after a restart: the one chosen, not the keyboard a text field brought
    /// up.
    pub fn chosen_tab(&self) -> Tab {
        self.raised_from.unwrap_or(self.tab)
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
        s.apply(crate::touch::Pointing::Idle.into());
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
        // (The app in front has the pad again, as a game would.)
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
        // Already on the keyboard: nothing to go back to.
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
