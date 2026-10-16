//! What an input may be mapped to (Output): a key chord, a mouse button, the pad's own, a layer, ...

use super::*;
use crate::names::names;

/// A key with the modifiers held for it; or the modifiers alone (no key: Shift as a key of its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Chord {
    pub key: Option<Key>,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Chord {
    pub fn of(key: Key) -> Chord {
        Chord { key: Some(key), ..Chord::default() }
    }

    /// Whether it presses anything.
    pub fn is_empty(&self) -> bool {
        self.key.is_none() && !(self.shift || self.ctrl || self.alt || self.logo)
    }

    /// As text: `ctrl+shift+x`, `space`, `f5`, `shift` (modifiers first, `+` between).
    pub fn write(&self, out: &mut impl Write) -> fmt::Result {
        let mut first = true;
        let mut part = |out: &mut dyn Write, text: &str| -> fmt::Result {
            if !first {
                out.write_char('+')?;
            }
            first = false;
            out.write_str(text)
        };
        for (on, name) in [(self.ctrl, "ctrl"), (self.alt, "alt"), (self.logo, "super"), (self.shift, "shift")] {
            if on {
                part(out, name)?;
            }
        }
        match self.key {
            Some(Key::Char(' ')) => part(out, "space"),
            Some(Key::Char(c)) => part(out, c.encode_utf8(&mut [0; 4])),
            Some(Key::Named(Named::F(n))) => {
                let mut name = Small::default();
                let _ = write!(name, "f{n}");
                part(out, name.as_str())
            }
            Some(Key::Named(n)) => part(out, named_name(n)),
            Some(Key::Mod(_) | Key::Caps) | None => Ok(()),
        }
    }

    /// How it reads on screen: `Ctrl+X`, `Space`, `F5`, `Shift`.
    pub fn label(&self, out: &mut impl Write) -> fmt::Result {
        let mut first = true;
        let mut part = |out: &mut dyn Write, args: fmt::Arguments| -> fmt::Result {
            if !first {
                out.write_char('+')?;
            }
            first = false;
            out.write_fmt(args)
        };
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.logo, "Super"), (self.shift, "Shift")] {
            if on {
                part(out, format_args!("{name}"))?;
            }
        }
        match self.key {
            Some(Key::Char(' ')) => part(out, format_args!("Space")),
            Some(Key::Char(c)) => part(out, format_args!("{}", c.to_ascii_uppercase())),
            Some(Key::Named(Named::F(n))) => part(out, format_args!("F{n}")),
            Some(Key::Named(n)) => part(out, format_args!("{}", named_label(n))),
            Some(Key::Mod(_) | Key::Caps) | None => Ok(()),
        }
    }

    pub fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord::default();
        for part in text.split('+') {
            match part {
                "ctrl" => chord.ctrl = true,
                "alt" => chord.alt = true,
                "super" => chord.logo = true,
                "shift" => chord.shift = true,
                "space" => chord.key = Some(Key::Char(' ')),
                _ if chord.key.is_some() => return None,
                p => {
                    let mut chars = p.chars();
                    chord.key = Some(match (chars.next(), chars.next()) {
                        (Some(c), None) if (' '..='~').contains(&c) && crate::keyboard::find(Key::Char(c)).is_some() => Key::Char(c),
                        _ => Key::Named(parse_named(p)?),
                    });
                }
            }
        }
        (!chord.is_empty()).then_some(chord)
    }
}

/// A few bytes of text without allocation (a function key's name).
#[derive(Default)]
struct Small {
    bytes: [u8; 8],
    len: usize,
}

impl Small {
    fn as_str(&self) -> &str {
        self.bytes.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl Write for Small {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        let slot = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn named_name(n: Named) -> &'static str {
    match n {
        Named::Backspace => "backspace",
        Named::Tab => "tab",
        Named::Enter => "enter",
        Named::Esc => "esc",
        Named::Delete => "delete",
        Named::Insert => "insert",
        Named::Home => "home",
        Named::End => "end",
        Named::PageUp => "pageup",
        Named::PageDown => "pagedown",
        Named::Left => "left",
        Named::Right => "right",
        Named::Up => "up",
        Named::Down => "down",
        Named::F(_) => "f",
    }
}

fn named_label(n: Named) -> &'static str {
    match n {
        Named::Backspace => "Backspace",
        Named::Tab => "Tab",
        Named::Enter => "Enter",
        Named::Esc => "Esc",
        Named::Delete => "Delete",
        Named::Insert => "Insert",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "PgUp",
        Named::PageDown => "PgDn",
        Named::Left => "Left",
        Named::Right => "Right",
        Named::Up => "Up",
        Named::Down => "Down",
        Named::F(_) => "F",
    }
}

fn parse_named(word: &str) -> Option<Named> {
    if let Some(n) = word.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n).then_some(Named::F(n));
    }
    [
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
    .find(|n| named_name(*n) == word)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

names!(MouseButton {
    Left: "left", "Left click";
    Right: "right", "Right click";
    Middle: "middle", "Middle click";
    Back: "back", "Back";
    Forward: "forward", "Forward";
});

/// What an input does instead of what it does on the pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Output {
    /// Nothing at all.
    Nothing,
    /// A button of the pad games see.
    Pad(PadOut),
    Key(Chord),
    Mouse(MouseButton),
    /// One step of the wheel each press (up, down, left, right).
    Wheel(Dir),
    /// A stick (a stick's or the gyro's motion).
    Stick(Side),
    /// The mouse pointer (a stick's or the gyro's motion).
    Pointer,
    /// Four keys, up, left, down, right (a stick: WASD).
    Keys([Chord; 4]),
    /// The D-pad (a stick).
    Dpad,
    /// A trigger (a trigger).
    Trigger(Side),
    /// A layer of the keymap on (while the binding is on: held, or toggled).
    Layer(Layer),
}

impl Output {
    /// Whether an input of `kind` may map to it.
    pub fn fits(&self, kind: Kind) -> bool {
        match self {
            Output::Nothing => true,
            Output::Pad(_) | Output::Key(_) | Output::Mouse(_) | Output::Wheel(_) | Output::Layer(_) => matches!(kind, Kind::Digital | Kind::Trigger),
            Output::Stick(_) | Output::Pointer => matches!(kind, Kind::Stick | Kind::Gyro),
            Output::Keys(_) | Output::Dpad => kind == Kind::Stick,
            Output::Trigger(_) => kind == Kind::Trigger,
        }
    }

    /// How it reads on screen: `B`, `Ctrl+X`, `Left click`, `Wheel up`, `Right stick`, `Mouse`,
    /// `W A S D`, `D-pad`, `LT`, `Nothing`.
    pub fn label(&self, out: &mut impl Write) -> fmt::Result {
        let side = |s: &Side| if *s == Side::Left { "Left" } else { "Right" };
        match self {
            Output::Nothing => out.write_str("Nothing"),
            Output::Pad(PadOut::Dpad(d)) => write!(out, "D-pad {}", d.name()),
            Output::Pad(p) => p.input().label(out),
            Output::Key(c) => c.label(out),
            Output::Mouse(m) => out.write_str(m.label()),
            Output::Wheel(d) => write!(out, "Wheel {}", d.name()),
            Output::Stick(s) => write!(out, "{} stick", side(s)),
            Output::Pointer => out.write_str("Mouse"),
            Output::Keys(keys) => {
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.write_char(' ')?;
                    }
                    k.label(out)?;
                }
                Ok(())
            }
            Output::Dpad => out.write_str("D-pad"),
            Output::Trigger(s) => out.write_str(if *s == Side::Left { "LT" } else { "RT" }),
            // (By its number here; the screens give it its keymap's name.)
            Output::Layer(l) => write!(out, "Layer {}", l.0),
        }
    }

    /// Whether it is on or off (a trigger mapped to it goes by its threshold).
    pub fn digital(&self) -> bool {
        matches!(self, Output::Pad(_) | Output::Key(_) | Output::Mouse(_) | Output::Wheel(_) | Output::Layer(_))
    }
}
