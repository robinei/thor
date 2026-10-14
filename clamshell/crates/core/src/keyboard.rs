//! The on-screen keyboard: a PC-style layout (US) with an Fn layer, modifiers that a tap arms for the
//! next key or that act while held (as on a real keyboard, with another finger typing), and Caps Lock.
//! Keys are identified by their position, so the same key keeps its id on both layers.

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable ASCII character, unshifted (`a`, `1`, `[`); Shift picks the US shifted one.
    Char(char),
    Named(Named),
    Mod(Modifier),
    /// Caps Lock: on until tapped again; shifts letters only. Kept here (it isn't sent).
    Caps,
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

/// A modifier's state: armed by a tap for the next key, and held while a finger is on it. Holding it
/// while typing makes it act for those keys and end with the finger; a tap without typing arms it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Latch {
    pub armed: bool,
    pub held: bool,
    /// A key was typed while held (so lifting ends it, rather than arming it).
    used: bool,
}

impl Latch {
    pub fn on(self) -> bool {
        self.armed || self.held
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
/// : < > ?); the rest has its own row on top with Esc, Tab and Caps Lock, - and = at its right as on a PC.
/// Fn: F1-F10
/// on the digits, F11 and F12 at the top right (on - and =), arrows on w/a/s/d under the left hand, and
/// under the right a PC's six-key block with its outer columns swapped: PgUp, Home, Insert on i/o/p over
/// PgDn, End, Delete on k/l/Backspace.
pub const LAYOUT: &[&[Slot]] = &[
    &[
        named(Named::Esc, 1.0),
        named(Named::Tab, 1.0),
        Slot { base: Key::Caps, fn_key: None, width: 1.0 },
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
        // One Shift is enough: a tap arms it for one key, or hold it while typing.
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
        // Super (sway's shortcuts; rarely needed here) is Fn+Alt, for a wider space bar.
        Slot { base: Key::Mod(Modifier::Alt), fn_key: Some(Key::Mod(Modifier::Super)), width: 1.0 },
        Slot { base: Key::Char(' '), fn_key: None, width: 3.0 },
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
    pub caps: bool,
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

    /// Whether `key` is typed shifted now: Shift shifts anything; Caps Lock letters, which Shift then
    /// turns back to lower case.
    pub fn shifts(&self, key: Key) -> bool {
        match key {
            Key::Char(c) if c.is_ascii_alphabetic() => self.shift.on() != self.caps,
            _ => self.shift.on(),
        }
    }

    /// A finger came down on the key at `pos`: a modifier is held.
    pub fn down(&mut self, pos: KeyPos) {
        if let Some(Key::Mod(m)) = self.effective(pos) {
            let latch = self.latch_mut(m);
            latch.held = true;
            latch.used = false;
        }
    }

    /// The finger lifted from the key at `pos` (after its tap, if it was one): a modifier that was used
    /// while held is done. (Either of the key's modifiers: Fn may have changed since it came down.)
    pub fn up(&mut self, pos: KeyPos) {
        let Some(slot) = slot(pos) else {
            return;
        };
        for key in [Some(slot.base), slot.fn_key].into_iter().flatten() {
            if let Key::Mod(m) = key {
                let latch = self.latch_mut(m);
                if !latch.held {
                    continue;
                }
                if latch.used {
                    latch.armed = false;
                }
                latch.held = false;
                latch.used = false;
            }
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

    /// A tap (`long` = false) or long press on the key at `pos`; returns the stroke to send, if any. A tap
    /// on a modifier arms or disarms it (a long press on one does nothing: holding is `down` and `up`).
    pub fn press(&mut self, pos: KeyPos, long: bool) -> Option<Stroke> {
        match self.effective(pos)? {
            Key::Mod(m) => {
                if !long {
                    let latch = self.latch_mut(m);
                    latch.armed = !latch.armed;
                    // A modifier reached through Fn (Super on Alt) uses Fn up, so the next key is plain.
                    if slot(pos).is_some_and(|s| s.base != Key::Mod(m)) && !self.fn_layer.held {
                        self.fn_layer.armed = false;
                    }
                }
                None
            }
            Key::Caps => {
                self.caps = !self.caps;
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
                // Armed modifiers are used up; held ones stay until lifted.
                for m in Modifier::ALL {
                    let latch = self.latch_mut(m);
                    if latch.held {
                        latch.used = true;
                    } else {
                        latch.armed = false;
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

    /// A finger on a key and off again, as the input side reports it: down, the tap, up.
    fn touch(kb: &mut Keyboard, key: Key) -> Option<Stroke> {
        let p = pos(key);
        kb.down(p);
        let stroke = kb.press(p, false);
        kb.up(p);
        stroke
    }

    #[test]
    fn a_tapped_shift_arms_one_key() {
        let mut kb = Keyboard::default();
        touch(&mut kb, Key::Mod(Modifier::Shift));
        assert!(touch(&mut kb, Key::Char('1')).unwrap().shift);
        assert!(!touch(&mut kb, Key::Char('1')).unwrap().shift);
        touch(&mut kb, Key::Mod(Modifier::Shift));
        touch(&mut kb, Key::Mod(Modifier::Shift));
        assert!(!touch(&mut kb, Key::Char('1')).unwrap().shift, "a second tap disarms");
    }

    #[test]
    fn a_held_shift_acts_while_held() {
        let mut kb = Keyboard::default();
        let shift = pos(Key::Mod(Modifier::Shift));
        kb.down(shift);
        assert!(touch(&mut kb, Key::Char('h')).unwrap().shift);
        assert!(touch(&mut kb, Key::Char('e')).unwrap().shift, "still held");
        // Held past a long press: the long press does nothing, lifting ends it.
        kb.press(shift, true);
        kb.up(shift);
        assert!(!touch(&mut kb, Key::Char('l')).unwrap().shift);
        // Lifted quickly after typing: the tap comes before the lift, and still nothing stays armed.
        kb.down(shift);
        touch(&mut kb, Key::Char('o'));
        kb.press(shift, false);
        kb.up(shift);
        assert!(!kb.shift.on());
    }

    #[test]
    fn caps_lock_shifts_letters_until_tapped_again() {
        let mut kb = Keyboard::default();
        touch(&mut kb, Key::Caps);
        assert!(kb.caps);
        assert!(touch(&mut kb, Key::Char('a')).unwrap().shift);
        assert!(!touch(&mut kb, Key::Char('1')).unwrap().shift, "letters only");
        touch(&mut kb, Key::Mod(Modifier::Shift));
        assert!(!touch(&mut kb, Key::Char('a')).unwrap().shift, "Shift with Caps Lock: lower case");
        assert!(touch(&mut kb, Key::Char('b')).unwrap().shift);
        touch(&mut kb, Key::Caps);
        assert!(!touch(&mut kb, Key::Char('a')).unwrap().shift);
    }

    #[test]
    fn fn_turns_wasd_into_arrows_and_iop_kl_into_the_six_key_block() {
        let mut kb = Keyboard::default();
        tap(&mut kb, Key::Mod(Modifier::Fn));
        assert_eq!(tap(&mut kb, Key::Char('w')).unwrap().key, Key::Named(Named::Up));
        assert_eq!(tap(&mut kb, Key::Char('w')).unwrap().key, Key::Char('w'), "Fn once: one key");
        kb.down(pos(Key::Mod(Modifier::Fn)));
        let block: Vec<Key> = ['a', 's', 'd', 'i', 'o', 'p', 'k', 'l'].map(|c| tap(&mut kb, Key::Char(c)).unwrap().key).to_vec();
        let named = [Named::Left, Named::Down, Named::Right, Named::PageUp, Named::Home, Named::Insert, Named::PageDown, Named::End];
        assert_eq!(block, named.map(Key::Named).to_vec());
        assert_eq!(tap(&mut kb, Key::Named(Named::Backspace)).unwrap().key, Key::Named(Named::Delete));
        assert_eq!(tap(&mut kb, Key::Char('1')).unwrap().key, Key::Named(Named::F(1)));
        kb.up(pos(Key::Mod(Modifier::Fn)));
        assert_eq!(tap(&mut kb, Key::Char('w')).unwrap().key, Key::Char('w'), "lifted");
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
    fn super_is_fn_alt() {
        let mut kb = Keyboard::default();
        touch(&mut kb, Key::Mod(Modifier::Fn));
        touch(&mut kb, Key::Mod(Modifier::Alt));
        assert!(kb.logo.armed && !kb.alt.armed && !kb.fn_layer.on());
        let s = touch(&mut kb, Key::Char('d')).unwrap();
        assert!(s.logo && s.key == Key::Char('d'), "plain d with Super, not Fn's arrow");
        // Held Fn, then held Alt: Super while held.
        let (fn_key, alt) = (pos(Key::Mod(Modifier::Fn)), pos(Key::Mod(Modifier::Alt)));
        kb.down(fn_key);
        kb.down(alt);
        kb.up(fn_key);
        assert!(touch(&mut kb, Key::Char('1')).unwrap().logo);
        kb.up(alt);
        assert!(!kb.logo.on());
    }

    #[test]
    fn a_long_press_alone_neither_arms_nor_locks() {
        let mut kb = Keyboard::default();
        let ctrl = pos(Key::Mod(Modifier::Ctrl));
        kb.down(ctrl);
        assert!(kb.ctrl.on(), "on while held (and shown so)");
        kb.press(ctrl, true);
        kb.up(ctrl);
        assert!(!kb.ctrl.on());
    }
}
