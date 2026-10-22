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
use crate::out::Out;
use crate::{Dirty, Effect, State};
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
    tab: Tab,
    /// When the second screen was last touched or had the pad's focus, in monotonic seconds as in
    /// [`crate::Event::Tick`]. None before the clock's first reading. Also whether the screen is drawn
    /// dimmed (see [`Bottom::dimmed`]).
    used_at: Option<u64>,
    dimmed: bool,
    /// The tab that was open when an app's text field brought up the keyboard. The screen goes back to it
    /// when the field is done, unless the user chose a tab in the meantime.
    raised_from: Option<Tab>,
    editors: Editors,
    /// The darkened second screen was tapped awake. This lasts until the next game.
    woken: bool,
    /// What the bottom screen shows, ClamShell's view or a window moved down, and whether it is off.
    screen: screen::BottomScreen,
    /// Which programs' windows go to the bottom screen.
    window_rules: screen::WindowRules,
    /// Windows a rule sent to the bottom screen. Each is sent once and dropped from here when it closes.
    sent_down: Vec<i64>,
    /// What a key just did, such as change a level or take a screenshot, shown under the bar for a
    /// moment.
    notice: Option<Notice>,
    /// Whether the area below the bar is pointing (see [`crate::bottom::touch`]).
    pointing: bool,
    /// The text on the system clipboard, for Ctrl+V in the editor. Files copied in the Files tab use their
    /// own clipboard (see [`crate::shelf::files`]).
    clipboard: Option<std::sync::Arc<str>>,
    /// The tab chosen changed and isn't saved yet (see [`crate::Resume`]).
    unsaved: bool,
}

impl Bottom {
    /// The second screen at start, with the saved window rules.
    pub(crate) fn new(window_rules: screen::WindowRules) -> Bottom {
        Bottom { window_rules, ..Bottom::default() }
    }

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

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn window_rules(&self) -> &screen::WindowRules {
        &self.window_rules
    }

    /// The tab to come back to after a restart. This is the tab that was chosen, not the keyboard a text
    /// field brought up.
    pub fn chosen_tab(&self) -> Tab {
        self.raised_from.unwrap_or(self.tab)
    }
}

impl Bottom {
    /// Changes the tab. Whatever the old tab had open, such as an editor or the keymap screens, stays
    /// there for coming back to. Returns whether it changed, so State can follow (see
    /// [`State::left_tab`]).
    pub(crate) fn set_tab(&mut self, tab: Tab) -> bool {
        if tab == self.tab {
            return false;
        }
        self.unsaved = true;
        self.tab = tab;
        true
    }

    /// The user chose a tab, by its icon, by tapping a control, or with the shoulder buttons. The tab
    /// stays, even if a text field had brought up the keyboard. Returns whether the tab changed.
    pub(crate) fn choose_tab(&mut self, tab: Tab) -> bool {
        if self.raised_from.take().is_some() {
            self.unsaved = true;
        }
        self.set_tab(tab)
    }

    /// The tab as it was left, at start.
    pub(crate) fn restore_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Whether the tab chosen changed since this was last asked, so the place is saved.
    pub(crate) fn take_unsaved(&mut self) -> bool {
        std::mem::take(&mut self.unsaved)
    }

    /// A text field wants typing, or is done. The keyboard comes up, and when the field is done the
    /// screen goes back to the tab it came from, unless the user chose a tab in the meantime. A field
    /// taking focus brings typing back from pointing. Returns the screens to redraw and whether the tab
    /// changed.
    pub(crate) fn text_input(&mut self, active: bool) -> (Dirty, bool) {
        let pointing = self.pointing;
        self.pointing &= !active;
        let changed = if active {
            // Not over an open editor, since what is typed there would be lost.
            if self.tab == Tab::Keyboard || self.editor().is_some() {
                false
            } else {
                self.raised_from = Some(self.tab);
                self.set_tab(Tab::Keyboard)
            }
        } else {
            match self.raised_from.take() {
                Some(tab) if self.tab == Tab::Keyboard => self.set_tab(tab),
                _ => false,
            }
        };
        let dirty = if changed {
            Dirty::BOTH
        } else if self.pointing != pointing {
            Dirty::BOTTOM
        } else {
            Dirty::NONE
        };
        (dirty, changed)
    }

    /// Opens `editor` on the keyboard tab, which goes back to the tab it came up from when the editing is
    /// done (see [`Bottom::back_from_keyboard`]). Returns whether the tab changed.
    pub(crate) fn open_on_keyboard(&mut self, editor: Editor) -> bool {
        self.editors.open(editor);
        if self.tab != Tab::Keyboard {
            self.raised_from = Some(self.tab);
        }
        self.set_tab(Tab::Keyboard)
    }

    /// Goes back to the tab the keyboard came up from, if it did. Returns whether the tab changed.
    pub(crate) fn back_from_keyboard(&mut self) -> bool {
        match self.raised_from.take() {
            Some(tab) => self.set_tab(tab),
            None => false,
        }
    }

    /// Contacts on the area below the bar, passed on to the virtual touchpad, or none (see [`touch`]).
    pub(crate) fn point(&mut self, pointing: touch::Pointing, out: &mut Out) {
        let before = self.pointing;
        match pointing {
            touch::Pointing::Contacts(contacts) => {
                self.pointing = true;
                out.effects.push(Effect::Touchpad(contacts));
            }
            touch::Pointing::Idle => self.pointing = false,
        }
        if self.pointing != before {
            out.mark(Dirty::BOTTOM);
        }
    }
}

impl State {
    /// The user chose a tab (see [`Bottom::choose_tab`]).
    pub(crate) fn choose_tab(&mut self, tab: Tab) {
        if self.bottom.choose_tab(tab) {
            self.left_tab();
        }
    }

    /// Goes back to the tab the keyboard came up from, if it did.
    pub(crate) fn back_from_keyboard(&mut self) {
        if self.bottom.back_from_keyboard() {
            self.left_tab();
        }
    }

    /// The second screen's tab changed, and its watch follows (see [`crate::sync::Sent::watches`]). What the
    /// tab left had going ends: the dock's menu or move, and any level row being set. The time tab opens
    /// on its main page again.
    pub(crate) fn left_tab(&mut self) {
        self.dock.close();
        self.levels.release();
        self.sys.reset_time_page();
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
