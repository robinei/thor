//! The on-screen keyboard: a PC-style layout (US) with an Fn layer, and one-shot or locked modifiers.
//! Keys are identified by their position, so the same key keeps its id on both layers.

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable ASCII character, unshifted (`a`, `1`, `[`); Shift picks the US shifted one.
    Char(char),
    Named(Named),
    Mod(Modifier),
}

/// Keys that type no character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Named {
    Backspace,
    Tab,
    Enter,
    Esc,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    /// F1-F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    /// The logo key; sent to the app or compositor like any other modifier.
    Super,
    Fn,
}

impl Modifier {
    pub const ALL: [Modifier; 5] = [Modifier::Shift, Modifier::Ctrl, Modifier::Alt, Modifier::Super, Modifier::Fn];
}

/// A modifier's state: a tap arms it for the next key, a long press locks it. A locked Shift is Caps Lock:
/// it shifts letters only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Latch {
    #[default]
    Off,
    Once,
    Locked,
}

impl Latch {
    pub fn on(self) -> bool {
        self != Latch::Off
    }
}

/// One key on the layout: what it does normally and with Fn, and its width in key units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub base: Key,
    pub fn_key: Option<Key>,
    pub width: f32,
}

const fn c(ch: char) -> Slot {
    Slot { base: Key::Char(ch), fn_key: None, width: 1.0 }
}

const fn cf(ch: char, f: Key) -> Slot {
    Slot { base: Key::Char(ch), fn_key: Some(f), width: 1.0 }
}

const fn named(n: Named, width: f32) -> Slot {
    Slot { base: Key::Named(n), fn_key: None, width }
}

const fn modifier(m: Modifier, width: f32) -> Slot {
    Slot { base: Key::Mod(m), fn_key: None, width }
}

const fn f(n: u8) -> Key {
    Key::Named(Named::F(n))
}

const fn n(k: Named) -> Key {
    Key::Named(k)
}

/// The layout: US letters and digits in their usual places, every row 10 units so keys use the full
/// width. The most typed punctuation sits after the space bar, nearest the thumbs (; , . / and with Shift
/// : < > ?); the rest has its own row on top with Esc and Tab, - and = at its right as on a PC. Fn: F1-F10
/// on the digits, F11 and F12 at the top right (on - and =), arrows on w/a/s/d under the left hand, and
/// under the right a PC's six-key block with its outer columns swapped: PgUp, Home, Insert on i/o/p over
/// PgDn, End, Delete on k/l/Backspace.
pub const LAYOUT: &[&[Slot]] = &[
    &[
        named(Named::Esc, 1.5),
        named(Named::Tab, 1.5),
        c('`'),
        c('\''),
        c('['),
        c(']'),
        c('\\'),
        cf('-', f(11)),
        cf('=', f(12)),
    ],
    &[
        cf('1', f(1)),
        cf('2', f(2)),
        cf('3', f(3)),
        cf('4', f(4)),
        cf('5', f(5)),
        cf('6', f(6)),
        cf('7', f(7)),
        cf('8', f(8)),
        cf('9', f(9)),
        cf('0', f(10)),
    ],
    &[
        c('q'),
        cf('w', n(Named::Up)),
        c('e'),
        c('r'),
        c('t'),
        c('y'),
        c('u'),
        cf('i', n(Named::PageUp)),
        cf('o', n(Named::Home)),
        cf('p', n(Named::Insert)),
    ],
    &[
        cf('a', n(Named::Left)),
        cf('s', n(Named::Down)),
        cf('d', n(Named::Right)),
        c('f'),
        c('g'),
        c('h'),
        c('j'),
        cf('k', n(Named::PageDown)),
        cf('l', n(Named::End)),
        Slot { base: n(Named::Backspace), fn_key: Some(n(Named::Delete)), width: 1.0 },
    ],
    &[
        // One Shift is enough: it is sticky (tap for one key, hold for Caps Lock).
        modifier(Modifier::Shift, 1.5),
        c('z'),
        c('x'),
        c('c'),
        c('v'),
        c('b'),
        c('n'),
        c('m'),
        named(Named::Enter, 1.5),
    ],
    &[
        modifier(Modifier::Fn, 1.0),
        modifier(Modifier::Ctrl, 1.0),
        modifier(Modifier::Super, 1.0),
        modifier(Modifier::Alt, 1.0),
        Slot { base: Key::Char(' '), fn_key: None, width: 2.0 },
        c(';'),
        c(','),
        c('.'),
        c('/'),
    ],
];

/// A key's place on the layout: its stable id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPos {
    pub row: u8,
    pub col: u8,
}

pub fn slot(pos: KeyPos) -> Option<&'static Slot> {
    LAYOUT.get(usize::from(pos.row))?.get(usize::from(pos.col))
}

/// The first position of `key` on the base layer (for tests and the pad's default focus).
pub fn find(key: Key) -> Option<KeyPos> {
    LAYOUT.iter().enumerate().find_map(|(row, keys)| {
        let col = keys.iter().position(|s| s.base == key)?;
        Some(KeyPos { row: u8::try_from(row).ok()?, col: u8::try_from(col).ok()? })
    })
}

/// The US-layout character Shift gives for `c`.
pub fn shifted(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        other => other,
    }
}

/// A key press to send to the focused app, with the modifiers held for it. `Char` stays unshifted; the
/// receiver sends Shift as a modifier, as a real keyboard does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub key: Key,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

#[derive(Debug, Default)]
pub struct Keyboard {
    pub shift: Latch,
    pub ctrl: Latch,
    pub alt: Latch,
    pub logo: Latch,
    pub fn_layer: Latch,
}

impl Keyboard {
    pub fn latch(&self, m: Modifier) -> Latch {
        match m {
            Modifier::Shift => self.shift,
            Modifier::Ctrl => self.ctrl,
            Modifier::Alt => self.alt,
            Modifier::Super => self.logo,
            Modifier::Fn => self.fn_layer,
        }
    }

    fn latch_mut(&mut self, m: Modifier) -> &mut Latch {
        match m {
            Modifier::Shift => &mut self.shift,
            Modifier::Ctrl => &mut self.ctrl,
            Modifier::Alt => &mut self.alt,
            Modifier::Super => &mut self.logo,
            Modifier::Fn => &mut self.fn_layer,
        }
    }

    /// Whether `key` is typed shifted now: Shift once shifts anything, Caps Lock only letters.
    pub fn shifts(&self, key: Key) -> bool {
        match (self.shift, key) {
            (Latch::Once, _) => true,
            (Latch::Locked, Key::Char(c)) => c.is_ascii_alphabetic(),
            _ => false,
        }
    }

    /// What the key at `pos` does now (its Fn function while Fn is on).
    pub fn effective(&self, pos: KeyPos) -> Option<Key> {
        let slot = slot(pos)?;
        Some(match (self.fn_layer.on(), slot.fn_key) {
            (true, Some(fn_key)) => fn_key,
            _ => slot.base,
        })
    }

    /// A tap (`long` = false) or long press on the key at `pos`; returns the stroke to send, if any.
    pub fn press(&mut self, pos: KeyPos, long: bool) -> Option<Stroke> {
        match self.effective(pos)? {
            Key::Mod(m) => {
                let latch = self.latch_mut(m);
                *latch = match (*latch, long) {
                    (Latch::Locked, _) => Latch::Off,
                    (_, true) => Latch::Locked,
                    (Latch::Off, false) => Latch::Once,
                    (Latch::Once, false) => Latch::Off,
                };
                None
            }
            key => {
                let stroke = Stroke {
                    key,
                    shift: self.shifts(key),
                    ctrl: self.ctrl.on(),
                    alt: self.alt.on(),
                    logo: self.logo.on(),
                };
                for m in Modifier::ALL {
                    let latch = self.latch_mut(m);
                    if *latch == Latch::Once {
                        *latch = Latch::Off;
                    }
                }
                Some(stroke)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn pos(key: Key) -> KeyPos {
        find(key).unwrap()
    }

    fn tap(kb: &mut Keyboard, key: Key) -> Option<Stroke> {
        kb.press(pos(key), false)
    }

    #[test]
    fn every_row_is_ten_units() {
        for row in LAYOUT {
            assert_eq!(row.iter().map(|s| s.width).sum::<f32>(), 10.0);
        }
    }

    #[test]
    fn all_printable_ascii_is_reachable() {
        let mut typed = std::collections::HashSet::new();
        for row in LAYOUT {
            for slot in *row {
                if let Key::Char(c) = slot.base {
                    typed.insert(c);
                    typed.insert(shifted(c));
                }
            }
        }
        for c in ' '..='~' {
            assert!(typed.contains(&c), "{c:?} missing");
        }
    }

    #[test]
    fn shift_once_then_off_and_locked_stays() {
        let mut kb = Keyboard::default();
        tap(&mut kb, Key::Mod(Modifier::Shift));
        assert!(tap(&mut kb, Key::Char('1')).unwrap().shift);
        assert!(!tap(&mut kb, Key::Char('1')).unwrap().shift);
        kb.press(pos(Key::Mod(Modifier::Shift)), true);
        assert!(tap(&mut kb, Key::Char('a')).unwrap().shift);
        assert_eq!(kb.shift, Latch::Locked);
    }

    #[test]
    fn locked_shift_is_caps_lock() {
        let mut kb = Keyboard::default();
        kb.press(pos(Key::Mod(Modifier::Shift)), true);
        assert!(tap(&mut kb, Key::Char('a')).unwrap().shift);
        assert!(!tap(&mut kb, Key::Char('1')).unwrap().shift);
    }

    #[test]
    fn fn_turns_wasd_into_arrows_and_iop_kl_into_the_six_key_block() {
        let mut kb = Keyboard::default();
        tap(&mut kb, Key::Mod(Modifier::Fn));
        assert_eq!(tap(&mut kb, Key::Char('w')).unwrap().key, Key::Named(Named::Up));
        assert_eq!(tap(&mut kb, Key::Char('w')).unwrap().key, Key::Char('w'), "Fn once: one key");
        kb.press(pos(Key::Mod(Modifier::Fn)), true);
        let block: Vec<Key> = ['a', 's', 'd', 'i', 'o', 'p', 'k', 'l'].map(|c| tap(&mut kb, Key::Char(c)).unwrap().key).to_vec();
        let named = [Named::Left, Named::Down, Named::Right, Named::PageUp, Named::Home, Named::Insert, Named::PageDown, Named::End];
        assert_eq!(block, named.map(Key::Named).to_vec());
        kb.press(pos(Key::Mod(Modifier::Fn)), false);
        kb.press(pos(Key::Mod(Modifier::Fn)), true);
        assert_eq!(tap(&mut kb, Key::Named(Named::Backspace)).unwrap().key, Key::Named(Named::Delete));
        assert_eq!(tap(&mut kb, Key::Char('1')).unwrap().key, Key::Named(Named::F(1)));
    }

    #[test]
    fn modifiers_combine_for_one_stroke() {
        let mut kb = Keyboard::default();
        tap(&mut kb, Key::Mod(Modifier::Ctrl));
        tap(&mut kb, Key::Mod(Modifier::Alt));
        let s = tap(&mut kb, Key::Char('t')).unwrap();
        assert!(s.ctrl && s.alt && !s.shift);
        assert!(!tap(&mut kb, Key::Char('t')).unwrap().ctrl, "one-shot modifiers are used up");
    }

    #[test]
    fn super_applies_to_the_next_key() {
        let mut kb = Keyboard::default();
        tap(&mut kb, Key::Mod(Modifier::Super));
        assert!(tap(&mut kb, Key::Char('d')).unwrap().logo);
        assert!(!tap(&mut kb, Key::Char('d')).unwrap().logo);
    }

    #[test]
    fn a_locked_modifier_is_released_by_a_tap() {
        let mut kb = Keyboard::default();
        kb.press(pos(Key::Mod(Modifier::Ctrl)), true);
        tap(&mut kb, Key::Mod(Modifier::Ctrl));
        assert_eq!(kb.ctrl, Latch::Off);
    }
}
