//! The second screen's tabs and the editor: a tab chosen or left, the keyboard coming up and going back,
//! and the editor's Save or Cancel.

use crate::bottom::editor::{Done, Purpose};
use crate::out::Out;
use crate::profiles::picker::Kind as ProfileKind;
use crate::{State, Tab};

impl State {
    /// The user chose a tab (see [`crate::Bottom::choose_tab`]).
    pub(crate) fn choose_tab(&mut self, tab: Tab) {
        if let Some(left) = self.bottom.choose_tab(tab) {
            self.left_tab(left);
        }
    }

    /// A text field wants typing, or is done (see [`crate::Bottom::text_input`]). While the second screen shows
    /// a window moved there, the keyboard doesn't come up over it: its tab is a tap away.
    pub(crate) fn text_input(&mut self, active: bool) {
        if active && self.bottom.shows_windows(&self.session) {
            return;
        }
        if let Some(left) = self.bottom.text_input(active) {
            self.left_tab(left);
        }
    }

    /// Opens `editor` on the keyboard tab (see [`crate::Bottom::open_on_keyboard`]).
    pub(crate) fn edit_on_keyboard(&mut self, editor: crate::Editor) {
        if let Some(left) = self.bottom.open_on_keyboard(editor, self.clock.mono()) {
            self.left_tab(left);
        }
    }

    /// Goes back to the tab the keyboard came up from, if it did.
    fn back_from_keyboard(&mut self) {
        if let Some(left) = self.bottom.back_from_keyboard() {
            self.left_tab(left);
        }
    }

    /// The second screen's tab changed from `left`, and its watch follows (see
    /// [`crate::sync::Sent::watches`]). What the tab left had going ends: the dock's menu or move, any
    /// level row being set, and the modifiers armed on the keymaps' capture screen. The time tab opens on
    /// its main page again.
    pub(crate) fn left_tab(&mut self, left: Tab) {
        if self.keymapping.capturing(left, false) {
            self.keyboard.take_modifiers();
        }
        self.dock.close();
        self.levels.release();
        self.sys.left_tab();
    }

    /// The editor's Save or Cancel, by its button or a key. Either closes it. Save hands the text to its
    /// purpose: the environment is saved, the network joined with the password, the keymap or file named,
    /// or the account password used. An editor on the keyboard tab then goes back to the tab the keyboard
    /// came up from, unless another editor took its place there (the new password after the current one).
    pub(crate) fn editor_done(&mut self, done: Done, out: &mut Out) {
        let Some(editor) = self.bottom.close_editor() else {
            return;
        };
        let on_keyboard = editor.tab() == Tab::Keyboard;
        match (done, editor.purpose) {
            (Done::Cancel, _) => {}
            (Done::Save, Purpose::Env { entry }) => {
                self.prefs.set_env(entry, &editor.text);
                out.effects.push(self.prefs.save_effect());
            }
            (Done::Save, Purpose::Password { ssid }) => self.radios.join(ssid, Some(editor.text), out),
            (Done::Save, Purpose::KeymapName(action)) => self.keymapping.named(action, &editor.text, out),
            (Done::Save, Purpose::ProfileName(ProfileKind::Keymap, action)) => self.keymapping.named_profile(action, &editor.text, out),
            (Done::Save, Purpose::ProfileName(ProfileKind::Curve, action)) => self.curving.named_profile(action, &editor.text, out),
            (Done::Save, Purpose::FileName(purpose)) => {
                if let Some(op) = crate::shelf::files::named(purpose, &editor.text) {
                    self.shelf.run_file_op(op, out);
                }
            }
            (Done::Save, Purpose::AccountPassword(step)) => crate::device::account::password_typed(step, editor.text, self.bottom.editors_mut(), out),
        }
        if on_keyboard && self.bottom.editor().is_none() {
            self.back_from_keyboard();
        }
    }
}
