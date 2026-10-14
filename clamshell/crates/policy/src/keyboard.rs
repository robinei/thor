//! The on-screen keyboard's output: a virtual keyboard (uinput), like the touchpad and the controller.
//! Each key press from the keyboard mode becomes its modifiers pressed, the key pressed and released,
//! and the modifiers released. Key positions are US; the compositor must use the US layout for this
//! device (sway: `input "<vendor>:<product>:clamshell_keyboard" xkb_layout us`), or the characters drawn
//! on the keys won't match what is typed.

use std::io;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use clamshell_core::keyboard::{Key, Named, Stroke};

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
        Key::Mod(_) => None,
    }
}

/// The events for one stroke: modifiers down, the key down, (report), the key up, modifiers up, (report).
pub fn events(stroke: &Stroke) -> Vec<(KeyCode, i32)> {
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
    let mut out: Vec<(KeyCode, i32)> = mods.iter().map(|&m| (m, 1)).collect();
    out.push((key, 1));
    out.push((key, 0));
    out.extend(mods.iter().rev().map(|&m| (m, 0)));
    out
}

pub struct VirtualKeyboard {
    device: VirtualDevice,
}

impl VirtualKeyboard {
    /// Creates the device; it has every key the on-screen keyboard can press.
    pub fn new() -> io::Result<Self> {
        let mut keys: AttributeSet<KeyCode> = [
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTALT,
            KeyCode::KEY_LEFTMETA,
            KeyCode::KEY_LEFTSHIFT,
        ]
        .into_iter()
        .collect();
        for c in ' '..='~' {
            if let Some(k) = char_key(c) {
                keys.insert(k);
            }
        }
        for n in [
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
        ]
        .into_iter()
        .chain((1..=12).map(Named::F))
        {
            keys.insert(named_key(n));
        }
        let device = VirtualDevice::builder()?.name(NAME).with_keys(&keys)?.build()?;
        Ok(VirtualKeyboard { device })
    }

    /// Types one stroke. The key goes down and up in separate reports, as from a real keyboard.
    pub fn stroke(&mut self, stroke: &Stroke) -> io::Result<()> {
        let events = events(stroke);
        let Some(release) = events.iter().position(|&(_, v)| v == 0) else {
            return Ok(());
        };
        let key = |&(code, value): &(KeyCode, i32)| InputEvent::new(EventType::KEY.0, code.0, value);
        let syn = InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0);
        let (down, up) = events.split_at(release);
        let mut batch: Vec<InputEvent> = down.iter().map(key).collect();
        batch.push(syn);
        self.device.emit(&batch)?;
        batch.clear();
        batch.extend(up.iter().map(key));
        batch.push(syn);
        self.device.emit(&batch)
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
                (KeyCode::KEY_LEFTCTRL, 1),
                (KeyCode::KEY_LEFTSHIFT, 1),
                (KeyCode::KEY_T, 1),
                (KeyCode::KEY_T, 0),
                (KeyCode::KEY_LEFTSHIFT, 0),
                (KeyCode::KEY_LEFTCTRL, 0),
            ]
        );
    }

    #[test]
    fn named_keys_and_f_keys() {
        assert_eq!(events(&stroke(Key::Named(Named::F(12))))[0], (KeyCode::KEY_F12, 1));
        assert_eq!(events(&stroke(Key::Named(Named::Delete)))[0], (KeyCode::KEY_DELETE, 1));
        assert!(events(&stroke(Key::Named(Named::F(13)))).is_empty());
    }

    /// Creates the real device and reads it back. Needs /dev/uinput; the reader grabs the device first,
    /// so the compositor never sees these keys. `cargo test -p clamshell-policy -- --ignored`
    #[test]
    #[ignore]
    fn the_virtual_keyboard_sends_its_keys() {
        let mut kb = VirtualKeyboard::new().expect("uinput");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut reader = evdev::raw_stream::enumerate().map(|(_, d)| d).find(|d| d.name() == Some(NAME)).expect("device node");
        reader.grab().expect("grab");
        kb.stroke(&Stroke { key: Key::Char('a'), shift: true, ctrl: false, alt: false, logo: false }).expect("stroke");
        let mut keys = Vec::new();
        while keys.len() < 4 {
            for e in reader.fetch_events().expect("read") {
                if e.event_type() == EventType::KEY {
                    keys.push((e.code(), e.value()));
                }
            }
        }
        assert_eq!(
            keys,
            vec![
                (KeyCode::KEY_LEFTSHIFT.0, 1),
                (KeyCode::KEY_A.0, 1),
                (KeyCode::KEY_A.0, 0),
                (KeyCode::KEY_LEFTSHIFT.0, 0)
            ]
        );
    }
}
