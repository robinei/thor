//! The on-screen keyboard's output: a virtual keyboard (uinput), like the touchpad and the controller.
//! Each key press from the keyboard mode becomes its modifiers pressed, the key pressed and released,
//! and the modifiers released. Key positions are US; the compositor must use the US layout for this
//! device (sway: `input "<vendor>:<product>:clamshell_keyboard" xkb_layout us`), or the characters drawn
//! on the keys won't match what is typed.

use std::io;

use clamshell_core::bottom::keyboard::{Key, Named, Stroke};
use evdev::uinput::VirtualDevice;
use evdev::{InputEvent, KeyCode};

use crate::input::{Press, Raw, keyset};

pub const NAME: &str = "clamshell keyboard";

/// The US key for an (unshifted) printable character.
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

/// The keys of one stroke in order: modifiers down, the key down, the key up, modifiers up.
pub fn events(stroke: &Stroke) -> Vec<(KeyCode, Press)> {
    let Some(key) = key_of(stroke) else {
        return Vec::new();
    };
    let mods: Vec<KeyCode> = [
        (stroke.ctrl, KeyCode::KEY_LEFTCTRL),
        (stroke.alt, KeyCode::KEY_LEFTALT),
        (stroke.logo, KeyCode::KEY_LEFTMETA),
        (stroke.shift, KeyCode::KEY_LEFTSHIFT),
    ]
    .into_iter()
    .filter_map(|(on, code)| on.then_some(code))
    .collect();
    let mut out: Vec<(KeyCode, Press)> = mods.iter().map(|&m| (m, Press::Down)).collect();
    out.push((key, Press::Down));
    out.push((key, Press::Up));
    out.extend(mods.iter().rev().map(|&m| (m, Press::Up)));
    out
}

/// The modifier keys, in the order they go down (and come up reversed).
pub const MODIFIERS: [KeyCode; 4] = [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTALT, KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT];

/// A keymap's chord: its modifiers' keys and its key's, if it has one.
pub fn chord_keys(chord: &clamshell_core::keymap::Chord) -> (Vec<KeyCode>, Option<KeyCode>) {
    let mods = [chord.ctrl, chord.alt, chord.logo, chord.shift].into_iter().zip(MODIFIERS).filter_map(|(on, k)| on.then_some(k)).collect();
    let key = chord.key.and_then(|key| key_of(&Stroke { key, shift: false, ctrl: false, alt: false, logo: false }));
    (mods, key)
}

/// Every key the on-screen keyboard (or a keymap's chord) can press.
pub fn all_keys() -> Vec<KeyCode> {
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

pub struct VirtualKeyboard {
    device: VirtualDevice,
}

impl VirtualKeyboard {
    /// Creates the device; it has every key the on-screen keyboard can press.
    pub fn new() -> io::Result<Self> {
        let keys = keyset(&all_keys());
        let device = VirtualDevice::builder()?.name(NAME).with_keys(&keys)?.build()?;
        Ok(VirtualKeyboard { device })
    }

    /// Types one stroke. The key goes down and up in separate reports, as from a real keyboard.
    pub fn stroke(&mut self, stroke: &Stroke) -> io::Result<()> {
        let events = events(stroke);
        let Some(release) = events.iter().position(|&(_, press)| press == Press::Up) else {
            return Ok(());
        };
        let report = |keys: &[(KeyCode, Press)]| -> Vec<InputEvent> {
            keys.iter().map(|&(code, press)| Raw::key(code, press).into()).chain([Raw::REPORT.into()]).collect()
        };
        let (down, up) = events.split_at(release);
        self.device.emit(&report(down))?;
        self.device.emit(&report(up))
    }
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
    fn a_shifted_ctrl_stroke_wraps_the_key_in_its_modifiers() {
        let s = Stroke { key: Key::Char('t'), shift: true, ctrl: true, alt: false, logo: false };
        assert_eq!(
            events(&s),
            vec![
                (KeyCode::KEY_LEFTCTRL, Press::Down),
                (KeyCode::KEY_LEFTSHIFT, Press::Down),
                (KeyCode::KEY_T, Press::Down),
                (KeyCode::KEY_T, Press::Up),
                (KeyCode::KEY_LEFTSHIFT, Press::Up),
                (KeyCode::KEY_LEFTCTRL, Press::Up),
            ]
        );
    }

    #[test]
    fn named_keys_and_f_keys() {
        assert_eq!(events(&stroke(Key::Named(Named::F(12))))[0], (KeyCode::KEY_F12, Press::Down));
        assert_eq!(events(&stroke(Key::Named(Named::Delete)))[0], (KeyCode::KEY_DELETE, Press::Down));
        assert!(events(&stroke(Key::Named(Named::F(13)))).is_empty());
    }

    /// Creates the real device and reads it back. Needs /dev/uinput; the reader grabs the device first,
    /// so the compositor never sees these keys. `cargo test -p clamshell-system -- --ignored`
    #[test]
    #[ignore = "needs /dev/uinput"]
    fn the_virtual_keyboard_sends_its_keys() {
        let mut kb = VirtualKeyboard::new().expect("uinput");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut reader = evdev::raw_stream::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(NAME)).expect("device node");
        reader.grab().expect("grab");
        kb.stroke(&Stroke { key: Key::Char('a'), shift: true, ctrl: false, alt: false, logo: false }).expect("stroke");
        let mut keys = Vec::new();
        while keys.len() < 4 {
            for e in reader.fetch_events().expect("read") {
                if e.event_type() == evdev::EventType::KEY {
                    keys.push((e.code(), e.value()));
                }
            }
        }
        assert_eq!(keys, vec![(KeyCode::KEY_LEFTSHIFT.0, 1), (KeyCode::KEY_A.0, 1), (KeyCode::KEY_A.0, 0), (KeyCode::KEY_LEFTSHIFT.0, 0)]);
    }
}
