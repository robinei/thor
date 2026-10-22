//! The editor: a line of text typed on the on-screen keyboard, where Enter saves and Esc cancels. It is
//! used for a game's environment, a Wi-Fi password, a keymap's name or a file's name. Its keys, its Save
//! and Cancel buttons, and pasting the clipboard into it are handled here. What a saved text does is up
//! to the module it is for: the settings, Wi-Fi, keymaps or files.

use std::fmt;
use std::sync::Arc;

use crate::bottom::keyboard::{self, Key, Named, Stroke};
use crate::effect::{Effect, Save};

/// What the editor edits, and the text so far.
#[derive(Clone, PartialEq, Eq)]
pub struct Editor {
    pub purpose: Purpose,
    pub text: String,
}

impl fmt::Debug for Editor {
    /// Keeps a typed password out of logs, as [`crate::kit::secret::Secret`] does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let password = matches!(self.purpose, Purpose::Password { .. } | Purpose::AccountPassword(_));
        let text: &dyn fmt::Debug = if password { &"…" } else { &self.text };
        f.debug_struct("Editor").field("purpose", &self.purpose).field("text", text).finish()
    }
}

impl Editor {
    /// The tab the editor shows on: Wi-Fi for a network's password, Settings for the environment and a
    /// keymap's name, and Keyboard for the account password and a file's name.
    pub fn tab(&self) -> crate::Tab {
        match self.purpose {
            Purpose::Password { .. } => crate::Tab::Wifi,
            Purpose::AccountPassword(_) | Purpose::FileName(_) => crate::Tab::Keyboard,
            Purpose::Env { .. } | Purpose::KeymapName(_) => crate::Tab::Settings,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A game's environment variables, or all games' (`None`).
    Env { entry: Option<Arc<str>> },
    /// The password for joining this Wi-Fi network.
    Password { ssid: String },
    /// A keymap's name.
    KeymapName(KeymapName),
    /// A browsed file's or folder's name (see [`crate::shelf::files`]).
    FileName(crate::shelf::files::FileName),
    /// The account's password, current or new (see [`crate::device::account`]).
    AccountPassword(crate::device::account::PasswordStep),
}

/// What a keymap's name is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapName {
    New,
    Rename(u16),
    /// A copy of this one.
    Duplicate(u16),
    /// A copy of a game's own keymap, as a new named one.
    SaveAs(u16),
    /// A new layer of this keymap, shown once it is made. If an input is given, it is mapped to the
    /// layer.
    NewLayer(u16, Option<(crate::keymap::Input, crate::keymap::Actuation)>),
    RenameLayer(u16, crate::keymap::Layer),
}

/// How a key ended the editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Save,
    Cancel,
}

impl Editor {
    /// Adds `text` as one line. Line breaks at the ends are trimmed and the rest become spaces.
    pub fn paste(&mut self, text: &str) {
        let line: String = text.trim_matches(['\n', '\r']).chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        self.text.push_str(&line);
    }

    /// Types `stroke`, a character or Backspace. Enter saves and Esc cancels.
    pub fn key(&mut self, stroke: Stroke) -> Option<Done> {
        match stroke.key {
            Key::Char(c) if !stroke.ctrl && !stroke.alt && !stroke.logo => {
                self.text.push(if stroke.shift { keyboard::shifted(c) } else { c });
            }
            Key::Named(Named::Backspace) => {
                self.text.pop();
            }
            Key::Named(Named::Enter) => return Some(Done::Save),
            Key::Named(Named::Esc) => return Some(Done::Cancel),
            _ => {}
        }
        None
    }
}

/// The texts being edited, such as a game's environment or a Wi-Fi password. Each is in its own tab, at
/// most one per tab, and is kept while another tab is shown.
#[derive(Debug, Clone, Default)]
pub struct Editors {
    list: Vec<Editor>,
}

impl Editors {
    /// The editor open in `tab`, if any.
    pub fn on(&self, tab: crate::Tab) -> Option<&Editor> {
        self.list.iter().find(|e| e.tab() == tab)
    }

    pub(crate) fn on_mut(&mut self, tab: crate::Tab) -> Option<&mut Editor> {
        self.list.iter_mut().find(|e| e.tab() == tab)
    }

    /// Opens an editor in its tab, replacing any editor already there.
    pub(crate) fn open(&mut self, editor: Editor) {
        self.list.retain(|e| e.tab() != editor.tab());
        self.list.push(editor);
    }

    /// Closes the editor in `tab` and returns it.
    pub(crate) fn close(&mut self, tab: crate::Tab) -> Option<Editor> {
        let i = self.list.iter().position(|e| e.tab() == tab)?;
        Some(self.list.remove(i))
    }
}

impl crate::Bottom {
    /// A key from the on-screen keyboard while the editor is open. Ctrl+V pastes the clipboard's text.
    /// Returns how the key ended the editing, if it did.
    pub(crate) fn edit(&mut self, stroke: Stroke) -> Option<Done> {
        if stroke.ctrl && !stroke.alt && stroke.key == Key::Char('v') {
            self.paste();
            return None;
        }
        self.editor_mut().and_then(|e| e.key(stroke))
    }

    /// Pastes the clipboard's text into the open editor, if there is one. Used by Ctrl+V and a right
    /// click.
    pub(crate) fn paste(&mut self) {
        if let (Some(text), Some(editor)) = (self.clipboard.clone(), self.editor_mut()) {
            editor.paste(&text);
        }
    }

    /// A reading of the system clipboard's text. Returns whether it changed.
    pub(crate) fn read_clipboard(&mut self, text: Option<Arc<str>>) -> bool {
        crate::kit::replace(&mut self.clipboard, text)
    }
}

impl crate::State {
    /// The editor's Save or Cancel, by its button or a key. Either closes it. Save hands the text to its
    /// purpose: the environment is saved, the network joined with the password, the keymap or file named,
    /// or the account password used. Cancelling a file's name goes back to the tab the keyboard came up
    /// from.
    pub(crate) fn editor_done(&mut self, done: Done) -> Vec<Effect> {
        let Some(editor) = self.bottom.close_editor() else {
            return vec![];
        };
        match (done, editor.purpose) {
            (Done::Cancel, Purpose::FileName(_)) => self.file_name_cancelled(),
            (Done::Cancel, _) => vec![],
            (Done::Save, Purpose::Env { entry: game }) => {
                self.settings.set_env(game, &editor.text);
                vec![Effect::Save(Save::Settings(self.settings.clone()))]
            }
            (Done::Save, Purpose::Password { ssid }) => vec![self.radios.join(ssid, Some(editor.text))],
            (Done::Save, Purpose::KeymapName(action)) => self.named_keymap(action, &editor.text),
            (Done::Save, Purpose::FileName(purpose)) => self.file_named(purpose, &editor.text),
            (Done::Save, Purpose::AccountPassword(step)) => crate::device::account::password_typed(step, editor.text, self.bottom.editors_mut()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(key: Key, shift: bool) -> Stroke {
        Stroke { key, shift, ctrl: false, alt: false, logo: false }
    }

    #[test]
    fn a_password_being_typed_stays_out_of_logs() {
        let e = Editor { purpose: Purpose::Password { ssid: "home".into() }, text: "hunter22".into() };
        assert!(!format!("{e:?}").contains("hunter22"));
        let e = Editor { purpose: Purpose::Env { entry: None }, text: "A=1".into() };
        assert!(format!("{e:?}").contains("A=1"), "the rest is shown");
        let connect = crate::radios::wifi::Command::Connect { ssid: "home".into(), password: Some(crate::kit::secret::Secret("hunter22".into())) };
        assert!(!format!("{connect:?}").contains("hunter22"));
    }

    #[test]
    fn typing_editing_and_finishing() {
        let mut e = Editor { purpose: Purpose::Env { entry: None }, text: String::new() };
        for c in ['a', '-', 'b'] {
            assert_eq!(e.key(stroke(Key::Char(c), c == '-')), None);
        }
        assert_eq!(e.text, "a_b");
        e.key(stroke(Key::Named(Named::Backspace), false));
        assert_eq!(e.text, "a_");
        assert_eq!(e.key(Stroke { ctrl: true, ..stroke(Key::Char('c'), false) }), None, "shortcuts don't type");
        assert_eq!(e.text, "a_");
        assert_eq!(e.key(stroke(Key::Named(Named::Enter), false)), Some(Done::Save));
        assert_eq!(e.key(stroke(Key::Named(Named::Esc), false)), Some(Done::Cancel));
    }
}
