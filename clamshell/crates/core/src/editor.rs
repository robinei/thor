//! The editor: a line of text typed on the on-screen keyboard (Enter saves, Esc cancels), for a game's
//! environment, a Wi-Fi password or a keymap's name; and environment variables as text, `KEY=value` words.

use std::sync::Arc;

use crate::keyboard::{self, Key, Named, Stroke};

/// What the editor edits, and the text so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub purpose: Purpose,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A game's environment variables, or all games' (`None`).
    Env { game: Option<Arc<str>> },
    /// The password for joining this Wi-Fi network.
    Password { ssid: String },
    /// A keymap's name.
    KeymapName(KeymapName),
}

/// What a keymap's name is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapName {
    New,
    Rename(u16),
    /// A copy of this one.
    Duplicate(u16),
}

/// How a key ended the editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Save,
    Cancel,
}

impl Editor {
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

/// `KEY=value` words (quotes as in launch commands) as variables.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    clamshell_library::launch::tokenize(text)
        .into_iter()
        .filter_map(|word| word.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .filter(|(k, _)| !k.is_empty())
        .collect()
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
            if d == '=' && eq.is_none() {
                eq = Some(len);
            }
            len += 1;
            chars.next();
        }
        if quote.is_some() {
            chars.next();
        }
        // Like parse_env: a key before the '=' (trimmed tokens: a quoted word's leading spaces don't count).
        if eq.is_some_and(|i| i > 0) {
            count += 1;
        }
    }
    count
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
        for text in ["", "A=1", "A=1 B=2", "  A=1   'B=x y' C", "=1 D=", "\"E=a b\" F=c", "'unclosed=x y", "x 'y' z=1=2"] {
            assert_eq!(count_env(text), parse_env(text).len(), "{text:?}");
        }
    }
}
