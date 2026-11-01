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

/// What one of the second screen's tabs is, declared in one place as the built-in apps are (see
/// [`crate::builtin_apps`]): what is read from the system while it is seen, and where its button is in the
/// bar. Its rules are its own area's, and how it looks its view in clamshell-views, found by its [`Tab`].
#[derive(Debug)]
pub struct TabDefinition {
    /// What is read from the system while it is open and seen.
    pub watch: fn(&crate::State) -> Option<Watch>,
    /// Whether the pad steps through something there rather than moving the focus, such as a dock entry
    /// being moved (see [`crate::nav::pad`]).
    pub steps_pad: fn(&crate::State) -> bool,
    pub button: BarPlace,
    /// With a window in front, the pad types with it rather than playing (see [`crate::nav::pad`]).
    pub types: bool,
    /// Whether it is open again at the next start when it was open at the last (see [`crate::Resume`]).
    pub restored: bool,
}

/// Where a tab's button is in the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarPlace {
    /// Among the buttons spread along it, in [`Tab::ALL`]'s order.
    Spread,
    /// The clock.
    Clock,
    /// At its right end.
    End,
}

impl Tab {
    /// Its definition.
    pub fn definition(self) -> &'static TabDefinition {
        const fn tab(watch: fn(&crate::State) -> Option<Watch>, button: BarPlace) -> TabDefinition {
            TabDefinition { watch, button, steps_pad: |_| false, types: false, restored: true }
        }
        const SPREAD: TabDefinition = tab(|_| None, BarPlace::Spread);
        const VOLUME: TabDefinition = tab(|_| Some(Watch::Audio), BarPlace::Spread);
        const BRIGHTNESS: TabDefinition = tab(|_| Some(Watch::Backlights), BarPlace::Spread);
        const WIFI: TabDefinition = tab(|_| Some(Watch::Wifi), BarPlace::Spread);
        const BLUETOOTH: TabDefinition = tab(|_| Some(Watch::Bluetooth), BarPlace::Spread);
        const BATTERY: TabDefinition = tab(|_| Some(Watch::Battery), BarPlace::Spread);
        const SYSINFO: TabDefinition = tab(|_| Some(Watch::Metrics), BarPlace::Spread);
        const TIME: TabDefinition = tab(|_| Some(Watch::Time), BarPlace::Clock);
        const WINDOWS: TabDefinition = tab(|_| None, BarPlace::End);
        // An entry's menu open watches the battery's readings, to show what the entry costs; a tile being
        // moved takes the pad's steps.
        const DOCK: TabDefinition =
            TabDefinition { watch: |s| s.dock().menu().is_some().then_some(Watch::Battery), steps_pad: |s| s.dock().moving().is_some(), ..SPREAD };
        const KEYBOARD: TabDefinition = TabDefinition { types: true, ..SPREAD };
        // Every touch there moves the pointer, so it opens only on request.
        const TOUCHPAD: TabDefinition = TabDefinition { restored: false, ..SPREAD };
        match self {
            Tab::Volume => &VOLUME,
            Tab::Brightness => &BRIGHTNESS,
            Tab::Wifi => &WIFI,
            Tab::Bluetooth => &BLUETOOTH,
            Tab::Battery => &BATTERY,
            Tab::Sysinfo => &SYSINFO,
            Tab::Time => &TIME,
            Tab::Windows => &WINDOWS,
            Tab::Keyboard => &KEYBOARD,
            Tab::Touchpad => &TOUCHPAD,
            Tab::Dock => &DOCK,
            Tab::Settings => &SPREAD,
        }
    }
}

/// The second screen's own state.
#[derive(Debug, Clone, Default)]
pub struct Bottom {
    tab: Tab,
    /// When the second screen was last touched or had the pad's focus, in monotonic seconds as in
    /// [`crate::Event::Tick`]. None before the clock's first reading. Also whether the screen is drawn
    /// dimmed (see [`Bottom::dimmed_at`]).
    used_at: Option<u64>,
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
    /// own clipboard (see [`crate::library_view::files`]).
    clipboard: Option<std::sync::Arc<str>>,
    /// The tab the windows' tab was opened from, which Home + right stick press goes back to.
    before_windows: Tab,
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
    /// there for coming back to; what the tab left had going ends when the state settles (see
    /// [`crate::State::follow_tab`]).
    pub(crate) fn set_tab(&mut self, tab: Tab) {
        if tab == self.tab {
            return;
        }
        if tab == Tab::Windows {
            self.before_windows = self.tab;
        }
        self.tab = tab;
    }

    /// The user chose a tab, by its icon, by tapping a control, or with the shoulder buttons. The tab
    /// stays, even if a text field had brought up the keyboard.
    pub(crate) fn choose_tab(&mut self, tab: Tab) {
        self.raised_from = None;
        self.set_tab(tab);
    }

    /// The tab as it was left, at start.
    pub(crate) fn restore_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// A text field wants typing, or is done. The keyboard comes up, and when the field is done the
    /// screen goes back to the tab it came from, unless the user chose a tab in the meantime. A field
    /// taking focus brings typing back from pointing.
    pub(crate) fn text_input(&mut self, active: bool) {
        self.pointing &= !active;
        if active {
            // Not over an open editor, since what is typed there would be lost.
            if self.tab != Tab::Keyboard && self.editor().is_none() {
                self.raised_from = Some(self.tab);
                self.set_tab(Tab::Keyboard);
            }
        } else {
            // An editor open on the keyboard keeps it, and goes back when it closes.
            if self.editor().is_some() {
                return;
            }
            if let Some(tab) = self.raised_from.take().filter(|_| self.tab == Tab::Keyboard) {
                self.set_tab(tab);
            }
        }
    }

    /// Opens `editor` on the keyboard tab, which goes back to the tab it came up from when the editing is
    /// done (see [`Bottom::back_from_keyboard`]). While another editor is open there, it is refused with a
    /// notice at `now`, so what was typed there isn't lost.
    pub(crate) fn open_on_keyboard(&mut self, editor: Editor, now: u64) {
        if self.editors.on(Tab::Keyboard).is_some() {
            self.notice.show(NoticeKind::KeyboardBusy, now);
            return;
        }
        self.editors.open(editor);
        if self.tab != Tab::Keyboard {
            self.raised_from = Some(self.tab);
        }
        self.set_tab(Tab::Keyboard);
    }

    /// Types the open editor's snippet `i` (see [`editor::Editor::snippets`]).
    pub(crate) fn insert_snippet(&mut self, i: usize) {
        let tab = self.tab;
        if let Some(e) = self.editors.on_mut(tab)
            && let Some(snippet) = e.snippets().get(i)
        {
            e.text.push_str(snippet);
        }
    }

    /// Goes back to the tab the keyboard came up from, if it did.
    pub(crate) fn back_from_keyboard(&mut self) {
        if let Some(tab) = self.raised_from.take() {
            self.set_tab(tab);
        }
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
        let purpose = crate::bottom::editor::Purpose::FileName(crate::library_view::files::FileName::Rename("/a/b".into()));
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

    /// A file of ClamShell's that couldn't be written is said, not only logged: an edit of library.json
    /// that didn't land vanishes at its read-back, a setting is lost at the next start.
    #[test]
    fn a_file_not_saved_is_said() {
        let mut s = state();
        s.applied(Event::NotSaved("library.json"));
        assert_eq!(notice(&s), Some(crate::bottom::notice::NoticeKind::NotSaved("library.json")));
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
