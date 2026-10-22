//! The editor: a line of text typed on the on-screen keyboard (Enter saves, Esc cancels), for a game's
//! environment, a Wi-Fi password, a keymap's name or a file's; and environment variables as text, `KEY=value`
//! words. Its keys, Save and Cancel, and the clipboard pasted into it, are the state's here; what a saved
//! text does is its purpose's (the settings, Wi-Fi, keymaps, files).

use std::sync::Arc;

use crate::effect::{Effect, Save};
use crate::id::EditorId;
use crate::keyboard::{self, Key, Named, Stroke};

/// What the editor edits, and the text so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub purpose: Purpose,
    pub text: String,
}

impl Editor {
    /// The tab it shows on: Wi-Fi for a network's password, Settings for the environment and a keymap's
    /// name, and Keyboard for the account password and a file's name.
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
    Env { game: Option<Arc<str>> },
    /// The password for joining this Wi-Fi network.
    Password { ssid: String },
    /// A keymap's name.
    KeymapName(KeymapName),
    /// A browsed file's or folder's name (see [`crate::files`]).
    FileName(crate::files::FileName),
    /// The account's password, current or new (see [`crate::account`]).
    AccountPassword(crate::account::PasswordStep),
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
    /// A new layer of this keymap (shown once made); for an input, mapped to it.
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
    /// Adds `text` (a line: its line breaks as spaces, its ends trimmed of them).
    pub fn paste(&mut self, text: &str) {
        let line: String = text.trim_matches(['\n', '\r']).chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        self.text.push_str(&line);
    }

    /// Types `stroke`: a character or Backspace; Enter saves, Esc cancels.
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

/// `KEY=value` words (see [`words`]) as variables.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    words(text).into_iter().filter_map(|word| word.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).filter(|(k, _)| !k.is_empty()).collect()
}

/// The words of `text`: whitespace separates; a word starting with `'` or `"` runs to the next same quote
/// (quotes removed); quotes inside an unquoted word are literal; words are trimmed.
fn words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c == '\'' || c == '"' {
            i += 1;
            while chars.get(i).is_some_and(|&d| d != c) {
                i += 1;
            }
            // Past the closing quote, if there is one.
            let end = (i + 1).min(chars.len());
            let word: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
            let closed = word.len() > 1 && word.ends_with(c);
            let inner = if closed { word.get(1..word.len() - 1).unwrap_or("") } else { word.get(1..).unwrap_or("") };
            words.push(inner.trim().to_owned());
            i = end;
        } else {
            while chars.get(i).is_some_and(|d| !d.is_whitespace()) {
                i += 1;
            }
            let word: String = chars.get(start..i).unwrap_or(&[]).iter().collect();
            words.push(word.trim().to_owned());
        }
    }
    words
}

/// How many variables `parse_env` finds in `text`, without allocating (for drawing).
pub fn count_env(text: &str) -> usize {
    let mut count = 0;
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        // One word: to the closing quote if it starts with one (the quotes don't count), else to whitespace.
        let quote = (c == '\'' || c == '"').then_some(c);
        if quote.is_some() {
            chars.next();
        }
        let (mut len, mut eq) = (0, None);
        while let Some(&d) = chars.peek() {
            if quote.map_or(d.is_whitespace(), |q| d == q) {
                break;
            }
            // (A quoted word's leading spaces: trimmed, as parse_env trims its words.)
            if len == 0 && d.is_whitespace() {
                chars.next();
                continue;
            }
            if d == '=' && eq.is_none() {
                eq = Some(len);
            }
            len += 1;
            chars.next();
        }
        if quote.is_some() {
            chars.next();
        }
        // Like parse_env: a key before the '='.
        if eq.is_some_and(|i| i > 0) {
            count += 1;
        }
    }
    count
}

impl crate::State {
    /// The editor's buttons.
    pub(crate) fn on_editor(&mut self, id: EditorId) -> Vec<Effect> {
        match id {
            EditorId::Save => self.save_editor(),
            EditorId::Cancel => self.cancel_editor(),
        }
    }

    /// The clipboard's text into the open editor (Ctrl+V, or a right click), if there is one.
    pub(crate) fn paste(&mut self) {
        if let (Some(text), Some(editor)) = (self.clipboard.clone(), self.bottom.editor_mut()) {
            editor.paste(&text);
        }
    }

    /// A key from the on-screen keyboard while the editor is open: Ctrl+V pastes the clipboard's text.
    pub(crate) fn edit(&mut self, stroke: Stroke) -> Vec<Effect> {
        if stroke.ctrl && !stroke.alt && stroke.key == crate::keyboard::Key::Char('v') {
            self.paste();
            return vec![];
        }
        match self.bottom.editor_mut().and_then(|e| e.key(stroke)) {
            Some(Done::Save) => self.save_editor(),
            Some(Done::Cancel) => self.cancel_editor(),
            None => vec![],
        }
    }

    /// The editor's Cancel: closed (a file's name: back to the tab the keyboard came up from).
    fn cancel_editor(&mut self) -> Vec<Effect> {
        match self.bottom.close_editor().map(|e| e.purpose) {
            Some(Purpose::FileName(_)) => self.file_name_cancelled(),
            _ => vec![],
        }
    }

    /// The editor's Save: the environment saved, or the network joined with the password.
    fn save_editor(&mut self) -> Vec<Effect> {
        let Some(editor) = self.bottom.close_editor() else {
            return vec![];
        };
        match editor.purpose {
            Purpose::Env { game } => {
                self.settings.set_env(game, &editor.text);
                vec![Effect::Save(Save::Settings(self.settings.clone()))]
            }
            Purpose::Password { ssid } => self.join(ssid, Some(editor.text)),
            Purpose::KeymapName(action) => self.named_keymap(action, &editor.text),
            Purpose::FileName(purpose) => self.file_named(purpose, &editor.text),
            Purpose::AccountPassword(step) => self.password_typed(step, editor.text),
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
    fn typing_editing_and_finishing() {
        let mut e = Editor { purpose: Purpose::Env { game: None }, text: String::new() };
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

    #[test]
    fn counting_variables_agrees_with_parsing_them() {
        for text in ["", "A=1", "A=1 B=2", "  A=1   'B=x y' C", "=1 D=", "\"E=a b\" F=c", "'unclosed=x y", "x 'y' z=1=2", "' =x'", "'  A =x'"] {
            assert_eq!(count_env(text), parse_env(text).len(), "{text:?}");
        }
    }
}
