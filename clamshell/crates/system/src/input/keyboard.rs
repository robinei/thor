//! The keys a keymap's chords press, through the pad's virtual device (see [`crate::input::pad`]): US
//! key positions, as is the compositor's layout, so a chord's character is what is typed.

use clamshell_state::bottom::keyboard::{Key, Named, Stroke};
use evdev::KeyCode;

/// The US key for an unshifted printable character.
fn char_key(c: char) -> Option<KeyCode> {
    Some(match c {
        'a' => KeyCode::KEY_A,
        'b' => KeyCode::KEY_B,
        'c' => KeyCode::KEY_C,
        'd' => KeyCode::KEY_D,
        'e' => KeyCode::KEY_E,
        'f' => KeyCode::KEY_F,
        'g' => KeyCode::KEY_G,
        'h' => KeyCode::KEY_H,
        'i' => KeyCode::KEY_I,
        'j' => KeyCode::KEY_J,
        'k' => KeyCode::KEY_K,
        'l' => KeyCode::KEY_L,
        'm' => KeyCode::KEY_M,
        'n' => KeyCode::KEY_N,
        'o' => KeyCode::KEY_O,
        'p' => KeyCode::KEY_P,
        'q' => KeyCode::KEY_Q,
        'r' => KeyCode::KEY_R,
        's' => KeyCode::KEY_S,
        't' => KeyCode::KEY_T,
        'u' => KeyCode::KEY_U,
        'v' => KeyCode::KEY_V,
        'w' => KeyCode::KEY_W,
        'x' => KeyCode::KEY_X,
        'y' => KeyCode::KEY_Y,
        'z' => KeyCode::KEY_Z,
        '1' => KeyCode::KEY_1,
        '2' => KeyCode::KEY_2,
        '3' => KeyCode::KEY_3,
        '4' => KeyCode::KEY_4,
        '5' => KeyCode::KEY_5,
        '6' => KeyCode::KEY_6,
        '7' => KeyCode::KEY_7,
        '8' => KeyCode::KEY_8,
        '9' => KeyCode::KEY_9,
        '0' => KeyCode::KEY_0,
        ' ' => KeyCode::KEY_SPACE,
        '-' => KeyCode::KEY_MINUS,
        '=' => KeyCode::KEY_EQUAL,
        '[' => KeyCode::KEY_LEFTBRACE,
        ']' => KeyCode::KEY_RIGHTBRACE,
        '\\' => KeyCode::KEY_BACKSLASH,
        ';' => KeyCode::KEY_SEMICOLON,
        '\'' => KeyCode::KEY_APOSTROPHE,
        '`' => KeyCode::KEY_GRAVE,
        ',' => KeyCode::KEY_COMMA,
        '.' => KeyCode::KEY_DOT,
        '/' => KeyCode::KEY_SLASH,
        _ => return None,
    })
}

fn named_key(n: Named) -> KeyCode {
    match n {
        Named::Backspace => KeyCode::KEY_BACKSPACE,
        Named::Tab => KeyCode::KEY_TAB,
        Named::Enter => KeyCode::KEY_ENTER,
        Named::Esc => KeyCode::KEY_ESC,
        Named::Delete => KeyCode::KEY_DELETE,
        Named::Insert => KeyCode::KEY_INSERT,
        Named::Home => KeyCode::KEY_HOME,
        Named::End => KeyCode::KEY_END,
        Named::PageUp => KeyCode::KEY_PAGEUP,
        Named::PageDown => KeyCode::KEY_PAGEDOWN,
        Named::Left => KeyCode::KEY_LEFT,
        Named::Right => KeyCode::KEY_RIGHT,
        Named::Up => KeyCode::KEY_UP,
        Named::Down => KeyCode::KEY_DOWN,
        Named::F(1) => KeyCode::KEY_F1,
        Named::F(2) => KeyCode::KEY_F2,
        Named::F(3) => KeyCode::KEY_F3,
        Named::F(4) => KeyCode::KEY_F4,
        Named::F(5) => KeyCode::KEY_F5,
        Named::F(6) => KeyCode::KEY_F6,
        Named::F(7) => KeyCode::KEY_F7,
        Named::F(8) => KeyCode::KEY_F8,
        Named::F(9) => KeyCode::KEY_F9,
        Named::F(10) => KeyCode::KEY_F10,
        Named::F(11) => KeyCode::KEY_F11,
        Named::F(12) => KeyCode::KEY_F12,
        Named::F(_) => KeyCode::KEY_UNKNOWN,
    }
}

/// The key a stroke presses, if it has one.
fn key_of(stroke: &Stroke) -> Option<KeyCode> {
    match stroke.key {
        Key::Char(c) => char_key(c),
        Key::Named(n) => Some(named_key(n)).filter(|k| *k != KeyCode::KEY_UNKNOWN),
        Key::Mod(_) | Key::Caps => None,
    }
}

/// The modifier keys, in the order they go down. They come up in reverse order.
pub(crate) const MODIFIERS: [KeyCode; 4] = [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTALT, KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT];

/// The keys of a keymap's chord: its modifiers, and its key if it has one.
pub(crate) fn chord_keys(chord: &clamshell_state::keymap::Chord) -> (Vec<KeyCode>, Option<KeyCode>) {
    let mods = [chord.ctrl, chord.alt, chord.logo, chord.shift].into_iter().zip(MODIFIERS).filter_map(|(on, k)| on.then_some(k)).collect();
    let key = chord.key.and_then(|key| key_of(&Stroke { key, shift: false, ctrl: false, alt: false, logo: false }));
    (mods, key)
}

/// Every key a keymap's chord can press.
pub(crate) fn all_keys() -> Vec<KeyCode> {
    let mut keys = MODIFIERS.to_vec();
    keys.extend((' '..='~').filter_map(char_key));
    let named = [
        Named::Backspace,
        Named::Tab,
        Named::Enter,
        Named::Esc,
        Named::Delete,
        Named::Insert,
        Named::Home,
        Named::End,
        Named::PageUp,
        Named::PageDown,
        Named::Left,
        Named::Right,
        Named::Up,
        Named::Down,
    ];
    keys.extend(named.into_iter().chain((1..=12).map(Named::F)).map(named_key));
    keys.sort_by_key(|k| k.0);
    keys.dedup();
    keys
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn stroke(key: Key) -> Stroke {
        Stroke { key, shift: false, ctrl: false, alt: false, logo: false }
    }

    #[test]
    fn every_printable_character_has_a_key() {
        for c in ' '..='~' {
            let base = if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c };
            let unshifted = match base {
                '!' => '1',
                '@' => '2',
                '#' => '3',
                '$' => '4',
                '%' => '5',
                '^' => '6',
                '&' => '7',
                '*' => '8',
                '(' => '9',
                ')' => '0',
                '_' => '-',
                '+' => '=',
                '{' => '[',
                '}' => ']',
                '|' => '\\',
                ':' => ';',
                '"' => '\'',
                '<' => ',',
                '>' => '.',
                '?' => '/',
                '~' => '`',
                other => other,
            };
            assert!(char_key(unshifted).is_some(), "{c:?}");
        }
    }

    #[test]
    fn named_keys_and_f_keys() {
        assert_eq!(key_of(&stroke(Key::Named(Named::F(12)))), Some(KeyCode::KEY_F12));
        assert_eq!(key_of(&stroke(Key::Named(Named::Delete))), Some(KeyCode::KEY_DELETE));
        assert_eq!(key_of(&stroke(Key::Named(Named::F(13)))), None);
    }
}
