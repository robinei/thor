//! The second screen. Its tabs are chosen from the bar, and the last one chosen stays, also across
//! restarts. This module also holds what belongs to the screen rather than to a tab's subject: the
//! editor over the keyboard, the settings panel's level, the keyboard a text field brought up, and the
//! screen going dark while a game runs.
//!
//! Volume and brightness open from their controls, and Bluetooth, Wi-Fi and Battery from their status
//! icons. All of them are ordinary tabs.

pub mod editor;
pub mod keyboard;
pub mod notice;
pub mod screen;
pub mod touch;

use crate::Effect;
use crate::effect::Watch;
use crate::kit::names::names;
use crate::out::Out;
use editor::{Editor, Editors};
use notice::{Notice, NoticeKind, NoticeSlot};

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
    /// The windows moved down to the second screen, shown below the bar, or how to move one there.
    Windows,
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
    Windows: "windows";
});

impl Tab {
    /// Whether the tab has a button among the bar's spread ones. The time tab's button is the clock, and
    /// the windows' tab's is at the bar's right end.
    pub fn in_bar(self) -> bool {
        !matches!(self, Tab::Time | Tab::Windows)
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
            Tab::Settings | Tab::Dock | Tab::Keyboard | Tab::Touchpad | Tab::Windows => None,
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
    /// What a key just did, such as change a level or take a screenshot, shown under the bar for a
    /// moment.
    notice: NoticeSlot,
    /// Whether the area below the bar is pointing (see [`crate::bottom::touch`]).
    pointing: bool,
    /// The text on the system clipboard, for Ctrl+V in the editor. Files copied in the Files tab use their
    /// own clipboard (see [`crate::shelf::files`]).
    clipboard: Option<std::sync::Arc<str>>,
    /// The tab chosen changed and isn't saved yet (see [`crate::Resume`]).
    unsaved: bool,
    /// The tab the windows' tab was opened from, which Home + right stick press goes back to.
    before_windows: Tab,
    /// The tab left as a window moved down, not yet settled (see [`Bottom::take_left`]).
    left: Option<Tab>,
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

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.get()
    }

    /// The tab to come back to after a restart. This is the tab that was chosen, not the keyboard a text
    /// field brought up.
    pub(crate) fn chosen_tab(&self) -> Tab {
        self.raised_from.unwrap_or(self.tab)
    }
}

impl Bottom {
    /// Changes the tab. Whatever the old tab had open, such as an editor or the keymap screens, stays
    /// there for coming back to. Returns the tab left if it changed, so State can follow (see
    /// [`crate::State::left_tab`]).
    pub(crate) fn set_tab(&mut self, tab: Tab) -> Option<Tab> {
        if tab == self.tab {
            return None;
        }
        self.unsaved = true;
        if tab == Tab::Windows {
            self.before_windows = self.tab;
        }
        Some(std::mem::replace(&mut self.tab, tab))
    }

    /// The user chose a tab, by its icon, by tapping a control, or with the shoulder buttons. The tab
    /// stays, even if a text field had brought up the keyboard. Returns the tab left if it changed.
    pub(crate) fn choose_tab(&mut self, tab: Tab) -> Option<Tab> {
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
    /// taking focus brings typing back from pointing. Returns the tab left if it changed.
    pub(crate) fn text_input(&mut self, active: bool) -> Option<Tab> {
        self.pointing &= !active;
        if active {
            // Not over an open editor, since what is typed there would be lost.
            if self.tab == Tab::Keyboard || self.editor().is_some() {
                None
            } else {
                self.raised_from = Some(self.tab);
                self.set_tab(Tab::Keyboard)
            }
        } else {
            // An editor open on the keyboard keeps it, and goes back when it closes.
            if self.editor().is_some() {
                return None;
            }
            match self.raised_from.take() {
                Some(tab) if self.tab == Tab::Keyboard => self.set_tab(tab),
                _ => None,
            }
        }
    }

    /// Opens `editor` on the keyboard tab, which goes back to the tab it came up from when the editing is
    /// done (see [`Bottom::back_from_keyboard`]). While another editor is open there, it is refused with a
    /// notice at `now`, so what was typed there isn't lost. Returns the tab left if it changed.
    pub(crate) fn open_on_keyboard(&mut self, editor: Editor, now: u64) -> Option<Tab> {
        if self.editors.on(Tab::Keyboard).is_some() {
            self.notice.show(NoticeKind::KeyboardBusy, now);
            return None;
        }
        self.editors.open(editor);
        if self.tab != Tab::Keyboard {
            self.raised_from = Some(self.tab);
        }
        self.set_tab(Tab::Keyboard)
    }

    /// Goes back to the tab the keyboard came up from, if it did. Returns the tab left if it changed.
    pub(crate) fn back_from_keyboard(&mut self) -> Option<Tab> {
        self.raised_from.take().and_then(|tab| self.set_tab(tab))
    }

    /// Contacts on the area below the bar, passed on to the virtual touchpad, or none (see [`touch`]).
    pub(crate) fn point(&mut self, pointing: touch::Pointing, out: &mut Out) {
        match pointing {
            touch::Pointing::Contacts(contacts) => {
                self.pointing = true;
                out.effects.push(Effect::Touchpad(contacts));
            }
            touch::Pointing::Idle => self.pointing = false,
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
        s.applied(crate::bottom::touch::Pointing::Idle.into());
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
        s.observed(vec![foot()]);
        let fx = s.applied(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(fx, vec![Effect::PadRoute(PadRoute::Keys)]);
        // The app in front has the pad again, as a game would.
        let back = vec![Effect::PadRoute(PadRoute::Game)];
        assert_eq!(s.applied(Event::TextInput(false)), back);
        assert_eq!(s.bottom().tab(), Tab::Touchpad, "back where it came from");
    }

    #[test]
    fn a_tab_chosen_while_the_keyboard_is_up_stays() {
        let mut s = state();
        s.observed(vec![foot()]);
        // Even the keyboard's own.
        s.applied(Event::TextInput(true));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        s.applied(Event::TextInput(false));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
        assert_eq!(s.resume().tab, "keyboard");
        // Already on the keyboard, so there is nothing to go back to.
        tap(&mut s, Id::Tab(Tab::Dock));
        tap(&mut s, Id::Tab(Tab::Keyboard));
        s.applied(Event::TextInput(true));
        s.applied(Event::TextInput(false));
        assert_eq!(s.bottom().tab(), Tab::Keyboard);
    }

    fn rename(text: &str) -> Editor {
        let purpose = crate::bottom::editor::Purpose::FileName(crate::shelf::files::FileName::Rename("/a/b".into()));
        Editor { purpose, text: text.to_owned() }
    }

    fn password() -> Editor {
        let step = crate::setup::account::PasswordStep::New { current: None };
        Editor { purpose: crate::bottom::editor::Purpose::AccountPassword(step), text: String::new() }
    }

    #[test]
    fn a_text_field_done_leaves_an_editor_on_the_keyboard_open() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Dock));
        s.observed(vec![foot()]);
        s.applied(Event::TextInput(true));
        s.applied(Event::EditOnKeyboard(rename("b")));
        s.applied(Event::TextInput(false));
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "the editor stays in view");
        assert_eq!(s.bottom().editor().map(|e| e.text.as_str()), Some("b"));
        s.applied(Event::Pointer(crate::PointerEvent::Activate(Id::Editor(EditorId::Cancel))));
        assert_eq!(s.bottom().tab(), Tab::Dock, "the editor closing goes back where the keyboard came from");
    }

    #[test]
    fn a_second_editor_for_the_keyboard_is_refused_and_what_is_typed_stays() {
        let mut s = state();
        tap(&mut s, Id::Tab(Tab::Dock));
        s.applied(Event::EditOnKeyboard(rename("new name")));
        s.applied(Event::EditOnKeyboard(password()));
        assert_eq!(notice(&s), Some(crate::bottom::notice::NoticeKind::KeyboardBusy));
        assert_eq!(s.bottom().editor(), Some(&rename("new name")), "the name typed is kept");
        assert_eq!(s.bottom().chosen_tab(), Tab::Dock, "still raised from where it came");
        // Not even while another tab is shown.
        tap(&mut s, Id::Tab(Tab::Wifi));
        s.applied(Event::EditOnKeyboard(password()));
        assert_eq!(s.bottom().tab(), Tab::Wifi);
        tap(&mut s, Id::Tab(Tab::Keyboard));
        assert_eq!(s.bottom().editor(), Some(&rename("new name")));
    }

    #[test]
    fn any_program_asking_for_text_brings_up_the_keyboard() {
        let mut s = state();
        crate::testkit::start(&mut s, "zelda-lttp");
        s.applied(Event::TextInput(true));
        assert_eq!(s.bottom().tab(), Tab::Keyboard, "a game too");
    }
}
