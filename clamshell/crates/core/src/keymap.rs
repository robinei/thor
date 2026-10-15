//! Keymaps: named remappings of the controller, shared between games (a game's or all games' setting,
//! [`crate::settings::Setting::Keymap`], is a keymap's id; 0 is Default, no remapping). Each maps some of
//! the pad's inputs ([`Input`]) to something else ([`Output`]); an input without a binding does what it
//! does on the pad. What an input may map to depends on its kind ([`Kind`]): a button to a pad button, a
//! key chord, a mouse button or a wheel step; a stick to a stick, the mouse pointer, four keys or the
//! D-pad; a trigger to a trigger or, past a threshold, anything a button maps to; the gyro to the mouse
//! pointer or a stick, always or while a chosen button is held.
//!
//! Each keymap has two layers ([`Layer`]): the main one, and one in force while the pad's Back button is
//! held. An input without a binding in the Back layer does what the main layer says.
//!
//! A button (or a D-pad or stick direction) may also act differently held long or pressed twice
//! ([`Actuation`]): each is a binding of its own. A binding to something on or off may also repeat while
//! on (turbo) or stay on from one actuation to the next (toggle).
//!
//! Saved in `keymaps.txt` (next to `game-settings.txt`): a `keymap ID NAME` line starts a keymap, then a
//! line per binding, `INPUT [hold|double] = OUTPUT [option=value ...] [toggle]`; a `layer back` line
//! starts the Back layer's bindings (`layer main` the main layer's again). Lines it doesn't understand are
//! skipped, and so are the bindings of a layer it doesn't know, so later features can add their own.

use std::fmt::{self, Write};

use crate::keyboard::{Key, Named};

/// A face, shoulder, menu, stick-click or back button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    A,
    B,
    X,
    Y,
    LB,
    RB,
    View,
    Menu,
    LS,
    RS,
    /// The back buttons (paddles), where a controller has them.
    L4,
    R4,
    L5,
    R5,
}

impl Button {
    pub const ALL: [Button; 14] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::LB,
        Button::RB,
        Button::View,
        Button::Menu,
        Button::LS,
        Button::RS,
        Button::L4,
        Button::R4,
        Button::L5,
        Button::R5,
    ];

    /// Whether it is a back button (not on the Xbox pad games see: it can't be a pad output).
    pub fn is_back(self) -> bool {
        matches!(self, Button::L4 | Button::R4 | Button::L5 | Button::R5)
    }

    pub fn name(self) -> &'static str {
        match self {
            Button::A => "a",
            Button::B => "b",
            Button::X => "x",
            Button::Y => "y",
            Button::LB => "lb",
            Button::RB => "rb",
            Button::View => "view",
            Button::Menu => "menu",
            Button::LS => "ls",
            Button::RS => "rs",
            Button::L4 => "l4",
            Button::R4 => "r4",
            Button::L5 => "l5",
            Button::R5 => "r5",
        }
    }

    /// How it reads on screen.
    pub fn label(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::LB => "LB",
            Button::RB => "RB",
            Button::View => "View",
            Button::Menu => "Menu",
            Button::LS => "LS",
            Button::RS => "RS",
            Button::L4 => "L4",
            Button::R4 => "R4",
            Button::L5 => "L5",
            Button::R5 => "R5",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];

    pub fn name(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dir::Up => "Up",
            Dir::Down => "Down",
            Dir::Left => "Left",
            Dir::Right => "Right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub const ALL: [Side; 2] = [Side::Left, Side::Right];

    fn letter(self) -> &'static str {
        match self {
            Side::Left => "l",
            Side::Right => "r",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// One of the pad's inputs that a keymap can map. (Home is ClamShell's: never mapped.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Input {
    Button(Button),
    Dpad(Dir),
    /// A stick as a whole.
    Stick(Side),
    /// A stick pushed one way (past half its travel).
    StickDir(Side, Dir),
    Trigger(Side),
    Gyro,
}

/// What kind of input it is, which decides what it may map to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// On or off: buttons, D-pad directions, a stick's directions.
    Digital,
    Stick,
    Trigger,
    Gyro,
}

impl Input {
    /// Every input, in the order keymaps are written.
    pub fn all() -> impl Iterator<Item = Input> {
        let buttons = Button::ALL.into_iter().map(Input::Button);
        let dpad = Dir::ALL.into_iter().map(Input::Dpad);
        let sticks = Side::ALL.into_iter().flat_map(|s| std::iter::once(Input::Stick(s)).chain(Dir::ALL.into_iter().map(move |d| Input::StickDir(s, d))));
        let triggers = Side::ALL.into_iter().map(Input::Trigger);
        buttons.chain(dpad).chain(sticks).chain(triggers).chain([Input::Gyro])
    }

    pub fn kind(self) -> Kind {
        match self {
            Input::Button(_) | Input::Dpad(_) | Input::StickDir(..) => Kind::Digital,
            Input::Stick(_) => Kind::Stick,
            Input::Trigger(_) => Kind::Trigger,
            Input::Gyro => Kind::Gyro,
        }
    }

    /// Its name in the keymap file and the control pipe: `a`, `up`, `lstick`, `lstick-up`, `lt`, `gyro`.
    pub fn name(self, out: &mut impl Write) -> fmt::Result {
        match self {
            Input::Button(b) => out.write_str(b.name()),
            Input::Dpad(d) => out.write_str(d.name()),
            Input::Stick(s) => write!(out, "{}stick", s.letter()),
            Input::StickDir(s, d) => write!(out, "{}stick-{}", s.letter(), d.name()),
            Input::Trigger(s) => write!(out, "{}t", s.letter()),
            Input::Gyro => out.write_str("gyro"),
        }
    }

    pub fn parse(word: &str) -> Option<Input> {
        Input::all().find(|i| {
            let mut name = String::new();
            i.name(&mut name).is_ok() && name == word
        })
    }

    /// How it reads on screen ("A", "Left stick up", "LT").
    pub fn label(self, out: &mut impl Write) -> fmt::Result {
        let side = |s: Side| if s == Side::Left { "Left" } else { "Right" };
        match self {
            Input::Button(b) => out.write_str(b.label()),
            Input::Dpad(d) => write!(out, "D-pad {}", d.name()),
            Input::Stick(s) => write!(out, "{} stick", side(s)),
            Input::StickDir(s, d) => write!(out, "{} stick {}", side(s), d.name()),
            Input::Trigger(s) => out.write_str(if s == Side::Left { "LT" } else { "RT" }),
            Input::Gyro => out.write_str("Gyro"),
        }
    }
}

/// A button on the pad games see (the Xbox 360 pad): a button (not a back button), a D-pad direction,
/// or a trigger pressed fully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadOut {
    Button(Button),
    Dpad(Dir),
    Trigger(Side),
}

impl PadOut {
    /// What the pad games see has, in the order offered.
    pub fn all() -> impl Iterator<Item = PadOut> {
        let buttons = Button::ALL.into_iter().filter(|b| !b.is_back()).map(PadOut::Button);
        buttons.chain(Side::ALL.into_iter().map(PadOut::Trigger)).chain(Dir::ALL.into_iter().map(PadOut::Dpad))
    }

    /// The input of the same name (to name it, and to take it from a press on the pad).
    pub fn input(self) -> Input {
        match self {
            PadOut::Button(b) => Input::Button(b),
            PadOut::Dpad(d) => Input::Dpad(d),
            PadOut::Trigger(s) => Input::Trigger(s),
        }
    }

    /// The pad output a pressed input stands for, if there is one (a back button or a stick isn't one).
    pub fn of(input: Input) -> Option<PadOut> {
        match input {
            Input::Button(b) if !b.is_back() => Some(PadOut::Button(b)),
            Input::Dpad(d) => Some(PadOut::Dpad(d)),
            Input::Trigger(s) => Some(PadOut::Trigger(s)),
            _ => None,
        }
    }
}

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
                let mut name = crate::keymap::Small::default();
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

impl MouseButton {
    pub const ALL: [MouseButton; 5] = [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward];

    pub fn label(self) -> &'static str {
        match self {
            MouseButton::Left => "Left click",
            MouseButton::Right => "Right click",
            MouseButton::Middle => "Middle click",
            MouseButton::Back => "Back",
            MouseButton::Forward => "Forward",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
            MouseButton::Back => "back",
            MouseButton::Forward => "forward",
        }
    }
}

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
}

impl Output {
    /// Whether an input of `kind` may map to it.
    pub fn fits(&self, kind: Kind) -> bool {
        match self {
            Output::Nothing => true,
            Output::Pad(_) | Output::Key(_) | Output::Mouse(_) | Output::Wheel(_) => matches!(kind, Kind::Digital | Kind::Trigger),
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
        }
    }

    /// Whether it is on or off (a trigger mapped to it goes by its threshold).
    pub fn digital(&self) -> bool {
        matches!(self, Output::Pad(_) | Output::Key(_) | Output::Mouse(_) | Output::Wheel(_))
    }
}

/// Motion mapped to a stick or the pointer: its speed, 0 to 100 (50 is the stock speed). Files from
/// before kept it as a step of 1 to 5, which load as 0, 25, 50, 75, 100.
pub const DEFAULT_SPEED: u8 = 50;
/// A trigger mapped to a button acts past this (percent of its travel, 1 to 100).
pub const DEFAULT_THRESHOLD: u8 = 50;
/// A press held this long is a hold (hundredths of a second, 20 to 100).
pub const DEFAULT_HOLD_TIME: u8 = 40;
/// The gyro: turning faster than this (degrees per second) gets the whole of its acceleration.
pub const DEFAULT_ACCEL_AT: u8 = 60;
/// The gyro: turning slower than this (tenths of a degree per second) is scaled down toward nothing
/// (tightening, against a hand's tremor).
pub const DEFAULT_TIGHTEN: u8 = 10;

/// A stick on the pointer: its deadzone and curve unless set otherwise (as the pointer moved before the
/// parameters were there: nothing within 15 % of the middle, then the distance from there squared).
pub const POINTER_DEADZONE: u8 = 15;
pub const POINTER_CURVE: u8 = 50;

/// An input's binding: what it maps to, and how. A stick's or trigger's tuning (deadzone, where it is
/// full, curve, inversion) applies to its values before they are mapped, whatever they map to; a binding
/// to the input itself ([`as_on_the_pad`]) with tuning is the input as on the pad, tuned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding {
    pub output: Output,
    /// Motion to the pointer or a stick: 0 to 100 (see [`DEFAULT_SPEED`]).
    pub speed: u8,
    /// A trigger to a button: percent of its travel.
    pub threshold: u8,
    /// A stick or trigger: percent of its travel that counts as rest, and where it counts as full
    /// (the range between stretched to the whole).
    pub deadzone: u8,
    pub full: u8,
    /// A stick: 0 is linear, more is finer near the middle (up to its distance cubed).
    pub curve: u8,
    /// A stick or the gyro's motion: across and up/down turned round.
    pub invert_x: bool,
    pub invert_y: bool,
    /// The gyro acts only while this is held (a button, or a trigger past half); always without one.
    pub hold: Option<Input>,
    /// Something on or off: pressed again and again while on, this many times a second (0: not).
    pub turbo: u8,
    /// Something on or off: one actuation turns it on, the next off.
    pub toggle: bool,
    /// A hold's binding: how long a press must last (hundredths of a second).
    pub hold_time: u8,
    /// The gyro: how much faster turns go further (0: as slow ones; 100: three times at full).
    pub accel: u8,
    /// The gyro: the turning speed (degrees per second) that gets all of the acceleration.
    pub accel_at: u8,
    /// The gyro: tightening, tenths of a degree per second (see [`DEFAULT_TIGHTEN`]).
    pub tighten: u8,
    /// The gyro to a stick: where its output starts (percent; past a game's own stick deadzone).
    pub stick_min: u8,
    /// The gyro: tilting the device like a steering wheel (roll) turns as well as rotating it (yaw).
    pub twist: bool,
    /// The gyro: off while its hold is held, instead of on (to lift and re-centre, a ratchet).
    pub ratchet: bool,
}

impl Binding {
    /// A binding to `output` with the parameters' general defaults (no deadzone, linear); see
    /// [`Binding::of`] for an input's.
    pub fn to(output: Output) -> Binding {
        Binding {
            output,
            speed: DEFAULT_SPEED,
            threshold: DEFAULT_THRESHOLD,
            deadzone: 0,
            full: 100,
            curve: 0,
            invert_x: false,
            invert_y: false,
            hold: None,
            turbo: 0,
            toggle: false,
            hold_time: DEFAULT_HOLD_TIME,
            accel: 0,
            accel_at: DEFAULT_ACCEL_AT,
            tighten: DEFAULT_TIGHTEN,
            stick_min: 0,
            twist: false,
            ratchet: false,
        }
    }

    /// `input`'s binding to `output` with the defaults for that pair ([`Param::default_for`]: a stick on
    /// the pointer has a deadzone and a curve).
    pub fn of(input: Input, output: Output) -> Binding {
        let mut b = Binding::to(output);
        for p in Param::ALL {
            p.set(&mut b, p.default_for(input, output));
        }
        b
    }

    /// Whether any parameter of `input`'s differs from its default for this output.
    pub fn tuned(&self, input: Input) -> bool {
        params(input, self.output).any(|p| p.get(self) != p.default_for(input, self.output)) || self.hold.is_some()
    }

    /// Whether it changes a stick's or trigger's values (a deadzone, a full point short of the end, a
    /// curve, an inversion).
    pub fn shapes(&self) -> bool {
        self.deadzone > 0 || self.full < 100 || self.curve > 0 || self.invert_x || self.invert_y
    }

    /// The same binding to another output: each parameter at its default for the old output goes to its
    /// default for the new one; the ones set stay.
    pub fn retarget(&self, input: Input, output: Output) -> Binding {
        let mut b = Binding { output, ..*self };
        for p in Param::ALL {
            if p.get(self) == p.default_for(input, self.output) {
                p.set(&mut b, p.default_for(input, output));
            }
        }
        b
    }
}

/// A binding's parameters beside its output: values set with a slider (0 to 100 on its track), and
/// switches ([`Param::switch`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Param {
    Speed,
    Threshold,
    Deadzone,
    Full,
    Curve,
    InvertX,
    InvertY,
    /// A hold's: how long the press must last.
    HoldTime,
    /// Presses a second while on (0: off).
    Turbo,
    Toggle,
    /// The gyro's.
    Accel,
    AccelAt,
    Tighten,
    StickMin,
    Twist,
    Ratchet,
}

impl Param {
    pub const ALL: [Param; 16] = [
        Param::Speed,
        Param::Accel,
        Param::AccelAt,
        Param::Tighten,
        Param::StickMin,
        Param::Deadzone,
        Param::Full,
        Param::Curve,
        Param::Threshold,
        Param::HoldTime,
        Param::Turbo,
        Param::Toggle,
        Param::Twist,
        Param::InvertX,
        Param::InvertY,
        Param::Ratchet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Param::Speed => "Speed",
            Param::Threshold => "Acts past",
            Param::Deadzone => "Deadzone",
            Param::Full => "Full at",
            Param::Curve => "Curve",
            Param::InvertX => "Invert X",
            Param::InvertY => "Invert Y",
            Param::HoldTime => "Hold for",
            Param::Turbo => "Turbo",
            Param::Toggle => "Toggle",
            Param::Accel => "Acceleration",
            Param::AccelAt => "Fastest at",
            Param::Tighten => "Tightening",
            Param::StickMin => "Stick from",
            Param::Twist => "Turn by tilting",
            Param::Ratchet => "Off while held",
        }
    }

    /// On or off (a tap switches it), rather than a value on a slider.
    pub fn switch(self) -> bool {
        matches!(self, Param::InvertX | Param::InvertY | Param::Toggle | Param::Twist | Param::Ratchet)
    }

    /// A value of it as shown: `40 %`, `Off` or `12/s` (turbo), `400 ms` (a hold's time).
    pub fn show(self, v: u8, out: &mut impl Write) -> fmt::Result {
        match self {
            Param::Turbo if v == 0 => out.write_str("Off"),
            Param::Turbo => write!(out, "{v}/s"),
            Param::HoldTime => write!(out, "{} ms", u16::from(v) * 10),
            Param::Accel if v == 0 => out.write_str("Off"),
            Param::AccelAt => write!(out, "{v} °/s"),
            Param::Tighten => write!(out, "{}.{} °/s", v / 10, v % 10),
            _ => write!(out, "{v} %"),
        }
    }

    /// Whether `input` mapped to `output` has it: a stick its deadzone, full point, curve and inversion
    /// (and speed on the pointer); a trigger its deadzone and full point (and threshold on a button);
    /// the gyro on motion its speed and inversion.
    pub fn applies(self, input: Input, output: Output) -> bool {
        let motion = matches!(output, Output::Pointer | Output::Stick(_));
        match (input.kind(), self) {
            (Kind::Stick, Param::Deadzone | Param::Full | Param::Curve | Param::InvertX | Param::InvertY) => true,
            (Kind::Stick, Param::Speed) => output == Output::Pointer,
            (Kind::Trigger, Param::Deadzone | Param::Full) => true,
            (Kind::Trigger, Param::Threshold) => output.digital(),
            (Kind::Gyro, Param::Speed | Param::InvertX | Param::InvertY | Param::Accel | Param::AccelAt | Param::Tighten | Param::Twist | Param::Ratchet) => motion,
            (Kind::Gyro, Param::StickMin) => matches!(output, Output::Stick(_)),
            (Kind::Digital | Kind::Trigger, Param::Turbo | Param::Toggle) => output.digital(),
            _ => false,
        }
    }

    /// [`Param::applies`] for `input`'s binding of `act`: a hold's also has its time.
    pub fn applies_to(self, input: Input, output: Output, act: Actuation) -> bool {
        match self {
            Param::HoldTime => act == Actuation::Hold,
            p => p.applies(input, output),
        }
    }

    /// The values it may take.
    pub fn range(self) -> std::ops::RangeInclusive<u8> {
        match self {
            Param::Speed | Param::Curve => 0..=100,
            Param::Threshold => 1..=100,
            Param::Deadzone => 0..=50,
            Param::Full => 50..=100,
            Param::InvertX | Param::InvertY | Param::Toggle => 0..=1,
            Param::HoldTime => 20..=100,
            Param::Turbo => 0..=30,
            Param::Accel => 0..=100,
            Param::AccelAt => 10..=200,
            Param::Tighten => 0..=50,
            Param::StickMin => 0..=50,
            Param::Twist | Param::Ratchet => 0..=1,
        }
    }

    /// Its value unless set, for `input` mapped to `output`: the same for every pair but a stick on
    /// the pointer, whose deadzone and curve are [`POINTER_DEADZONE`] and [`POINTER_CURVE`].
    pub fn default_for(self, input: Input, output: Output) -> u8 {
        let pointer = input.kind() == Kind::Stick && output == Output::Pointer;
        match self {
            Param::Speed => DEFAULT_SPEED,
            Param::Threshold => DEFAULT_THRESHOLD,
            Param::Full => 100,
            Param::Deadzone if pointer => POINTER_DEADZONE,
            Param::Curve if pointer => POINTER_CURVE,
            Param::Deadzone | Param::Curve | Param::InvertX | Param::InvertY | Param::Turbo | Param::Toggle => 0,
            Param::Accel | Param::StickMin | Param::Twist | Param::Ratchet => 0,
            Param::HoldTime => DEFAULT_HOLD_TIME,
            Param::AccelAt => DEFAULT_ACCEL_AT,
            Param::Tighten => DEFAULT_TIGHTEN,
        }
    }

    pub fn get(self, b: &Binding) -> u8 {
        match self {
            Param::Speed => b.speed,
            Param::Threshold => b.threshold,
            Param::Deadzone => b.deadzone,
            Param::Full => b.full,
            Param::Curve => b.curve,
            Param::InvertX => u8::from(b.invert_x),
            Param::InvertY => u8::from(b.invert_y),
            Param::HoldTime => b.hold_time,
            Param::Turbo => b.turbo,
            Param::Toggle => u8::from(b.toggle),
            Param::Accel => b.accel,
            Param::AccelAt => b.accel_at,
            Param::Tighten => b.tighten,
            Param::StickMin => b.stick_min,
            Param::Twist => u8::from(b.twist),
            Param::Ratchet => u8::from(b.ratchet),
        }
    }

    /// Sets it, kept within [`Param::range`].
    pub fn set(self, b: &mut Binding, value: u8) {
        let value = value.clamp(*self.range().start(), *self.range().end());
        match self {
            Param::Speed => b.speed = value,
            Param::Threshold => b.threshold = value,
            Param::Deadzone => b.deadzone = value,
            Param::Full => b.full = value,
            Param::Curve => b.curve = value,
            Param::InvertX => b.invert_x = value > 0,
            Param::InvertY => b.invert_y = value > 0,
            Param::HoldTime => b.hold_time = value,
            Param::Turbo => b.turbo = value,
            Param::Toggle => b.toggle = value > 0,
            Param::Accel => b.accel = value,
            Param::AccelAt => b.accel_at = value,
            Param::Tighten => b.tighten = value,
            Param::StickMin => b.stick_min = value,
            Param::Twist => b.twist = value > 0,
            Param::Ratchet => b.ratchet = value > 0,
        }
    }

    /// Its name in the keymap file (the switches go together as `invert=x`, `invert=xy`).
    fn name(self) -> &'static str {
        match self {
            Param::Speed => "speed",
            Param::Threshold => "threshold",
            Param::Deadzone => "deadzone",
            Param::Full => "full",
            Param::Curve => "curve",
            Param::InvertX | Param::InvertY => "invert",
            Param::HoldTime => "holdtime",
            Param::Turbo => "turbo",
            Param::Toggle => "toggle",
            Param::Accel => "accel",
            Param::AccelAt => "accelat",
            Param::Tighten => "tighten",
            Param::StickMin => "stickmin",
            Param::Twist => "twist",
            Param::Ratchet => "ratchet",
        }
    }
}

/// The parameters `input` mapped to `output` has, in the order shown.
pub fn params(input: Input, output: Output) -> impl Iterator<Item = Param> {
    Param::ALL.into_iter().filter(move |p| p.applies(input, output))
}

/// The parameters of `input`'s binding of `act` mapped to `output` (a hold's has its time too).
pub fn params_of(input: Input, output: Output, act: Actuation) -> impl Iterator<Item = Param> {
    Param::ALL.into_iter().filter(move |p| p.applies_to(input, output, act))
}

/// How a button (a D-pad or stick direction too) is used: pressed (as usual), held long, or pressed
/// twice quickly. Each may have a binding of its own; with a hold's or a double's, a short press acts
/// only once it is known not to be one of those (on letting go, or after the window for a second
/// press).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Actuation {
    #[default]
    Press,
    Hold,
    Double,
}

impl Actuation {
    pub const ALL: [Actuation; 3] = [Actuation::Press, Actuation::Hold, Actuation::Double];

    pub fn label(self) -> &'static str {
        match self {
            Actuation::Press => "Press",
            Actuation::Hold => "Hold",
            Actuation::Double => "Double",
        }
    }

    /// Its word after the input in the keymap file (a press has none).
    fn name(self) -> &'static str {
        match self {
            Actuation::Press => "",
            Actuation::Hold => "hold",
            Actuation::Double => "double",
        }
    }

    /// Whether an input of `kind` has it: every input is pressed; only buttons and directions are held
    /// or pressed twice.
    pub fn fits(self, kind: Kind) -> bool {
        self == Actuation::Press || kind == Kind::Digital
    }
}

/// How the panel and the labels tell an input's binding: none, its own output only tuned, or mapped to
/// something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Tuned,
    Mapped,
}

/// What `binding` (of `input`) changes.
pub fn change(input: Input, binding: Option<&Binding>) -> Change {
    match binding {
        None => Change::None,
        Some(b) if Some(b.output) != as_on_the_pad(input) => Change::Mapped,
        Some(b) if b.tuned(input) => Change::Tuned,
        Some(_) => Change::None,
    }
}

/// What may hold the gyro on: the buttons and the triggers.
pub fn gyro_holds() -> impl Iterator<Item = Input> {
    Button::ALL.into_iter().map(Input::Button).chain(Side::ALL.into_iter().map(Input::Trigger))
}

/// A keymap's layers: the main one, and the one in force while the pad's Back button is held (its
/// inputs without a binding do what the main layer says).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Layer {
    #[default]
    Main,
    Back,
}

impl Layer {
    pub const ALL: [Layer; 2] = [Layer::Main, Layer::Back];

    /// Its name in the keymap file (`layer back`).
    pub fn name(self) -> &'static str {
        match self {
            Layer::Main => "main",
            Layer::Back => "back",
        }
    }

    /// How it reads on screen.
    pub fn label(self) -> &'static str {
        match self {
            Layer::Main => "Main",
            Layer::Back => "Back held",
        }
    }

    pub fn other(self) -> Layer {
        match self {
            Layer::Main => Layer::Back,
            Layer::Back => Layer::Main,
        }
    }
}

/// One layer's bindings, an actuation's each in [`Input::all`]'s order, one per input at most.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Bindings {
    press: Vec<(Input, Binding)>,
    hold: Vec<(Input, Binding)>,
    double: Vec<(Input, Binding)>,
}

impl Bindings {
    fn of(&self, act: Actuation) -> &Vec<(Input, Binding)> {
        match act {
            Actuation::Press => &self.press,
            Actuation::Hold => &self.hold,
            Actuation::Double => &self.double,
        }
    }

    fn of_mut(&mut self, act: Actuation) -> &mut Vec<(Input, Binding)> {
        match act {
            Actuation::Press => &mut self.press,
            Actuation::Hold => &mut self.hold,
            Actuation::Double => &mut self.double,
        }
    }

    fn is_empty(&self) -> bool {
        Actuation::ALL.into_iter().all(|a| self.of(a).is_empty())
    }
}

/// A named set of bindings in two layers (the inputs without one in the main layer do what they do on
/// the pad; without one in the Back layer, what the main layer says), a press's, a hold's and a
/// double's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    /// Its number, kept across renames (0 is Default, which no keymap has).
    pub id: u16,
    pub name: String,
    main: Bindings,
    back: Bindings,
}

impl Keymap {
    pub fn new(id: u16, name: &str) -> Keymap {
        Keymap { id, name: name.to_owned(), main: Bindings::default(), back: Bindings::default() }
    }

    /// The main layer's bindings (a press's).
    pub fn bindings(&self) -> &[(Input, Binding)] {
        &self.main.press
    }

    /// A layer's bindings (a press's).
    pub fn layer(&self, layer: Layer) -> &[(Input, Binding)] {
        self.layer_of(layer, Actuation::Press)
    }

    /// A layer's bindings of `act`.
    pub fn layer_of(&self, layer: Layer, act: Actuation) -> &[(Input, Binding)] {
        self.bindings_in(layer).of(act)
    }

    fn bindings_in(&self, layer: Layer) -> &Bindings {
        match layer {
            Layer::Main => &self.main,
            Layer::Back => &self.back,
        }
    }

    fn bindings_in_mut(&mut self, layer: Layer) -> &mut Bindings {
        match layer {
            Layer::Main => &mut self.main,
            Layer::Back => &mut self.back,
        }
    }

    /// Whether `layer` has no bindings at all.
    pub fn is_empty(&self, layer: Layer) -> bool {
        self.bindings_in(layer).is_empty()
    }

    /// `input`'s binding in the main layer.
    pub fn get(&self, input: Input) -> Option<&Binding> {
        self.get_in(Layer::Main, input)
    }

    /// `input`'s binding in `layer` itself (none in the Back layer: the main layer's applies).
    pub fn get_in(&self, layer: Layer, input: Input) -> Option<&Binding> {
        self.get_of(layer, input, Actuation::Press)
    }

    /// `input`'s binding of `act` in `layer` itself.
    pub fn get_of(&self, layer: Layer, input: Input, act: Actuation) -> Option<&Binding> {
        self.layer_of(layer, act).iter().find(|(i, _)| *i == input).map(|(_, b)| b)
    }

    /// The binding in force for `input`, with Back held (`back`) or not: the Back layer's, else the main
    /// layer's.
    pub fn lookup(&self, input: Input, back: bool) -> Option<&Binding> {
        self.lookup_of(input, Actuation::Press, back)
    }

    /// [`Keymap::lookup`] for `act`.
    pub fn lookup_of(&self, input: Input, act: Actuation, back: bool) -> Option<&Binding> {
        back.then(|| self.get_of(Layer::Back, input, act)).flatten().or_else(|| self.get_of(Layer::Main, input, act))
    }

    /// Binds `input` in the main layer (none: back to what it does on the pad); see [`Keymap::set_in`].
    pub fn set(&mut self, input: Input, binding: Option<Binding>) -> bool {
        self.set_in(Layer::Main, input, binding)
    }

    /// Binds `input` in `layer` (none: as on the pad in the main layer, as the main layer in the Back
    /// layer). A binding that doesn't fit the input's kind is refused; whether it changed.
    pub fn set_in(&mut self, layer: Layer, input: Input, binding: Option<Binding>) -> bool {
        self.set_of(layer, input, Actuation::Press, binding)
    }

    /// Binds `input`'s `act` in `layer` (none: no hold or double; for a press, as [`Keymap::set_in`]).
    /// Refused where it doesn't fit (a binding to the input's kind, a hold or double to a stick); whether
    /// it changed.
    pub fn set_of(&mut self, layer: Layer, input: Input, act: Actuation, binding: Option<Binding>) -> bool {
        if binding.is_some_and(|b| !b.output.fits(input.kind())) || (binding.is_some() && !act.fits(input.kind())) {
            return false;
        }
        let list = self.bindings_in_mut(layer).of_mut(act);
        let before = list.clone();
        list.retain(|(i, _)| *i != input);
        if let Some(b) = binding {
            list.push((input, b));
            list.sort_by_key(|(i, _)| *i);
        }
        *list != before
    }

    /// Drops every binding of `layer`; whether it had any.
    pub fn clear(&mut self, layer: Layer) -> bool {
        let had = !self.is_empty(layer);
        *self.bindings_in_mut(layer) = Bindings::default();
        had
    }
}

/// What does what `input` does on the pad, as a binding's output (to say so in the Back layer while the
/// main layer maps it): a button, D-pad direction, stick or trigger as itself; the gyro off. A stick's
/// direction has none (it is part of its stick).
pub fn as_on_the_pad(input: Input) -> Option<Output> {
    match input {
        Input::Button(b) => Some(Output::Pad(PadOut::Button(b))),
        Input::Dpad(d) => Some(Output::Pad(PadOut::Dpad(d))),
        Input::Stick(s) => Some(Output::Stick(s)),
        Input::Trigger(s) => Some(Output::Trigger(s)),
        Input::Gyro => Some(Output::Nothing),
        Input::StickDir(..) => None,
    }
}

/// The ready-made keymap made when there is no keymap file yet: A and B, X and Y swapped.
pub fn nintendo(id: u16) -> Keymap {
    let mut k = Keymap::new(id, "Nintendo layout");
    for (from, to) in [(Button::A, Button::B), (Button::B, Button::A), (Button::X, Button::Y), (Button::Y, Button::X)] {
        k.set(Input::Button(from), Some(Binding::to(Output::Pad(PadOut::Button(to)))));
    }
    k
}

/// Every keymap, by id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymaps {
    list: Vec<Keymap>,
}

impl Keymaps {
    /// What there is without a keymap file: the Nintendo layout.
    pub fn initial() -> Keymaps {
        Keymaps { list: vec![nintendo(1)] }
    }

    pub fn list(&self) -> &[Keymap] {
        &self.list
    }

    pub fn get(&self, id: u16) -> Option<&Keymap> {
        self.list.iter().find(|k| k.id == id)
    }

    pub fn get_mut(&mut self, id: u16) -> Option<&mut Keymap> {
        self.list.iter_mut().find(|k| k.id == id)
    }

    /// A name no keymap has: `name`, else with a number after it.
    pub fn unique_name(&self, name: &str) -> String {
        let name = name.trim();
        let taken = |n: &str| self.list.iter().any(|k| k.name == n);
        if !taken(name) {
            return name.to_owned();
        }
        (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
    }

    /// A new, empty keymap named `name` (made unique); its id.
    pub fn add(&mut self, name: &str) -> u16 {
        let id = self.list.iter().map(|k| k.id).max().unwrap_or(0).saturating_add(1);
        let name = self.unique_name(name);
        self.list.push(Keymap::new(id, &name));
        id
    }

    /// A copy of keymap `id` named `name` (made unique); the copy's id.
    pub fn duplicate(&mut self, id: u16, name: &str) -> Option<u16> {
        let (main, back) = self.get(id).map(|k| (k.main.clone(), k.back.clone()))?;
        let copy = self.add(name);
        if let Some(k) = self.get_mut(copy) {
            k.main = main;
            k.back = back;
        }
        Some(copy)
    }

    /// Renames keymap `id` (another's name is made unique); whether it was there.
    pub fn rename(&mut self, id: u16, name: &str) -> bool {
        if self.get(id).is_some_and(|k| k.name == name.trim()) {
            return true;
        }
        let name = self.unique_name(name);
        self.get_mut(id).map(|k| k.name = name).is_some()
    }

    pub fn remove(&mut self, id: u16) -> bool {
        let before = self.list.len();
        self.list.retain(|k| k.id != id);
        self.list.len() != before
    }

    /// As text (see the module's doc).
    pub fn text(&self) -> String {
        let mut text = String::new();
        for k in &self.list {
            let _ = writeln!(text, "keymap {} {}", k.id, k.name);
            write_bindings(&k.main, &mut text);
            if !k.back.is_empty() {
                let _ = writeln!(text, "layer {}", Layer::Back.name());
                write_bindings(&k.back, &mut text);
            }
        }
        text
    }

    /// Reads [`Keymaps::text`] back; lines it doesn't understand are skipped, and the bindings of a
    /// layer it doesn't know.
    pub fn load(text: &str) -> Keymaps {
        let mut keymaps = Keymaps::default();
        // The layer the lines are in (none: one not known).
        let mut layer = Some(Layer::Main);
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("keymap ") {
                let (id, name) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Ok(id) = id.parse::<u16>()
                    && id > 0
                    && keymaps.get(id).is_none()
                {
                    keymaps.list.push(Keymap::new(id, name.trim()));
                }
                layer = Some(Layer::Main);
                continue;
            }
            if let Some(name) = line.strip_prefix("layer ") {
                layer = Layer::ALL.into_iter().find(|l| l.name() == name.trim());
                continue;
            }
            let (Some(keymap), Some(layer), Some((input, act, binding))) = (keymaps.list.last_mut(), layer, parse_binding(line)) else {
                continue;
            };
            keymap.set_of(layer, input, act, Some(binding));
        }
        keymaps
    }
}

/// A layer's bindings, a line each, a press's, then a hold's and a double's (see the module's doc).
fn write_bindings(bindings: &Bindings, text: &mut String) {
    for act in Actuation::ALL {
        for (input, b) in bindings.of(act) {
            let _ = input.name(text);
            if act != Actuation::Press {
                text.push(' ');
                text.push_str(act.name());
            }
            text.push_str(" = ");
            let _ = write_output(&b.output, text);
            for p in params_of(*input, b.output, act).filter(|p| !p.switch()) {
                let v = p.get(b);
                if v != p.default_for(*input, b.output) {
                    // (Speed with a percent sign: files from before have a step there.)
                    let _ = write!(text, " {}={v}{}", p.name(), if p == Param::Speed { "%" } else { "" });
                }
            }
            if b.invert_x || b.invert_y {
                let _ = write!(text, " invert={}{}", if b.invert_x { "x" } else { "" }, if b.invert_y { "y" } else { "" });
            }
            if let Some(hold) = b.hold {
                text.push_str(" hold=");
                let _ = hold.name(text);
            }
            for (on, word) in [(b.toggle, "toggle"), (b.twist, "twist"), (b.ratchet, "ratchet")] {
                if on {
                    text.push(' ');
                    text.push_str(word);
                }
            }
            text.push('\n');
        }
    }
}

fn write_output(o: &Output, out: &mut String) -> fmt::Result {
    match o {
        Output::Nothing => out.write_str("nothing"),
        Output::Pad(p) => {
            out.write_str("pad ")?;
            p.input().name(out)
        }
        Output::Key(c) => {
            out.write_str("key ")?;
            c.write(out)
        }
        Output::Mouse(m) => write!(out, "mouse {}", m.name()),
        Output::Wheel(d) => write!(out, "wheel {}", d.name()),
        Output::Stick(s) => write!(out, "stick {}", s.name()),
        Output::Pointer => out.write_str("pointer"),
        Output::Keys(keys) => {
            out.write_str("keys")?;
            for k in keys {
                out.write_char(' ')?;
                k.write(out)?;
            }
            Ok(())
        }
        Output::Dpad => out.write_str("dpad"),
        Output::Trigger(s) => write!(out, "trigger {}", s.name()),
    }
}

fn side(word: &str) -> Option<Side> {
    Side::ALL.into_iter().find(|s| s.name() == word)
}

/// A speed as written: `N%` (0 to 100), or a step of 1 to 5 (files from before).
fn speed(v: &str) -> Option<u8> {
    match v.strip_suffix('%') {
        Some(percent) => percent.parse().ok().filter(|p| Param::Speed.range().contains(p)),
        None => v.parse::<u8>().ok().filter(|s| (1..=5).contains(s)).map(|s| (s - 1) * 25),
    }
}

/// `INPUT = OUTPUT [option=value ...]`.
fn parse_binding(line: &str) -> Option<(Input, Actuation, Binding)> {
    let (input, rest) = line.split_once('=')?;
    let mut left = input.split_whitespace();
    let input = Input::parse(left.next()?)?;
    let act = match left.next() {
        None => Actuation::Press,
        Some(w) => Actuation::ALL.into_iter().find(|a| a.name() == w && *a != Actuation::Press)?,
    };
    let mut words = rest.split_whitespace().peekable();
    let output = match (words.next()?, words.peek().copied()) {
        ("nothing", _) => Output::Nothing,
        ("pointer", _) => Output::Pointer,
        ("dpad", _) => Output::Dpad,
        ("pad", Some(w)) => Output::Pad(PadOut::of(Input::parse(w)?)?),
        ("key", Some(w)) => Output::Key(Chord::parse(w)?),
        ("mouse", Some(w)) => Output::Mouse(MouseButton::ALL.into_iter().find(|m| m.name() == w)?),
        ("wheel", Some(w)) => Output::Wheel(Dir::ALL.into_iter().find(|d| d.name() == w)?),
        ("stick", Some(w)) => Output::Stick(side(w)?),
        ("trigger", Some(w)) => Output::Trigger(side(w)?),
        ("keys", Some(_)) => {
            let mut keys = [Chord::default(); 4];
            for k in &mut keys {
                *k = Chord::parse(words.next()?)?;
            }
            Output::Keys(keys)
        }
        _ => return None,
    };
    if !matches!(output, Output::Nothing | Output::Pointer | Output::Dpad | Output::Keys(_)) {
        words.next();
    }
    let mut binding = Binding::of(input, output);
    for word in words {
        match word.split_once('=') {
            Some(("speed", v)) => binding.speed = speed(v).unwrap_or(DEFAULT_SPEED),
            Some(("threshold", v)) => binding.threshold = v.parse().ok().filter(|v| Param::Threshold.range().contains(v)).unwrap_or(DEFAULT_THRESHOLD),
            Some((name @ ("deadzone" | "full" | "curve" | "turbo" | "holdtime" | "accel" | "accelat" | "tighten" | "stickmin"), v)) => {
                if let Some(p) = Param::ALL.into_iter().find(|p| p.name() == name)
                    && let Some(v) = v.parse().ok().filter(|v| p.range().contains(v))
                {
                    p.set(&mut binding, v);
                }
            }
            Some(("invert", v)) => {
                binding.invert_x = v.contains('x');
                binding.invert_y = v.contains('y');
            }
            Some(("hold", v)) => binding.hold = Input::parse(v).filter(|h| gyro_holds().any(|g| g == *h)),
            None if word == "toggle" => binding.toggle = true,
            None if word == "twist" => binding.twist = true,
            None if word == "ratchet" => binding.ratchet = true,
            // (Options of later features.)
            _ => {}
        }
    }
    (input.kind() == Kind::Gyro || binding.hold.is_none()).then_some((input, act, binding))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap()
    }

    #[test]
    fn chords_as_text() {
        let mut text = String::new();
        chord("x+ctrl").write(&mut text).unwrap();
        assert_eq!(text, "ctrl+x", "modifiers first");
        for t in ["shift", "space", "f5", "alt+tab", "ctrl+shift+=", "pagedown", "super+d", "/"] {
            let mut back = String::new();
            chord(t).write(&mut back).unwrap();
            assert_eq!(back, t);
        }
        assert_eq!(Chord::parse("ctrl+x+y"), None, "two keys");
        assert_eq!(Chord::parse("f13"), None);
        assert_eq!(Chord::parse("é"), None, "no key types it");
        assert_eq!(Chord::parse(""), None);
    }

    #[test]
    fn only_fitting_targets_bind() {
        let mut k = Keymap::new(1, "k");
        assert!(k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+x"))))));
        assert!(!k.set(Input::Button(Button::A), Some(Binding::to(Output::Pointer))), "a button can't move the pointer");
        assert!(!k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Mouse(MouseButton::Left)))));
        assert!(k.set(Input::Stick(Side::Right), Some(Binding::to(Output::Pointer))));
        assert!(k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Key(chord("shift"))))), "a trigger past its threshold");
        assert!(!k.set(Input::Trigger(Side::Left), Some(Binding::to(Output::Dpad))));
        assert!(!k.set(Input::Gyro, Some(Binding::to(Output::Keys([chord("w"); 4])))));
        assert!(k.set(Input::Gyro, Some(Binding::to(Output::Stick(Side::Right)))));
        for kind in [Kind::Digital, Kind::Stick, Kind::Trigger, Kind::Gyro] {
            assert!(Output::Nothing.fits(kind));
        }
        assert!(k.set(Input::Button(Button::A), None));
        assert!(!k.set(Input::Button(Button::A), None), "already as on the pad");
        assert_eq!(k.get(Input::Button(Button::A)), None);
    }

    #[test]
    fn keymaps_round_trip_as_text() {
        let mut maps = Keymaps::initial();
        let fps = maps.add("FPS");
        let k = maps.get_mut(fps).unwrap();
        k.set(Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+x")))));
        k.set(Input::Stick(Side::Right), Some(Binding { speed: 75, deadzone: 20, full: 90, curve: 40, invert_y: true, ..Binding::to(Output::Pointer) }));
        k.set(Input::Trigger(Side::Right), Some(Binding { deadzone: 5, ..Binding::to(Output::Trigger(Side::Right)) }));
        k.set(Input::Stick(Side::Left), Some(Binding::to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")]))));
        k.set(Input::Trigger(Side::Left), Some(Binding { threshold: 25, ..Binding::to(Output::Mouse(MouseButton::Right)) }));
        k.set(Input::StickDir(Side::Right, Dir::Up), Some(Binding::to(Output::Wheel(Dir::Up))));
        k.set(Input::Dpad(Dir::Down), Some(Binding::to(Output::Nothing)));
        k.set(Input::Button(Button::L4), Some(Binding::to(Output::Pad(PadOut::Dpad(Dir::Left)))));
        k.set(Input::Gyro, Some(Binding { speed: 37, hold: Some(Input::Trigger(Side::Left)), ..Binding::to(Output::Pointer) }));
        let text = maps.text();
        assert!(text.starts_with("keymap 1 Nintendo layout\na = pad b\nb = pad a\nx = pad y\ny = pad x\nkeymap 2 FPS\na = key ctrl+x\nl4 = pad left\n"), "{text}");
        assert!(text.contains("\nlstick = keys w a s d\n") && text.contains("\nrstick = pointer speed=75% deadzone=20 full=90 curve=40 invert=y\n") && text.contains("\ngyro = pointer speed=37% hold=lt\n"), "{text}");
        assert!(text.contains("\nlt = mouse right threshold=25\n") && text.contains("\nrt = trigger right deadzone=5\n") && text.contains("\nrstick-up = wheel up\n"), "{text}");
        assert_eq!(Keymaps::load(&text), maps);
    }

    #[test]
    fn lines_not_understood_are_skipped() {
        let text = "# a comment\nkeymap 3 Turbo\na = key x turbo=10\nb = warp 9\nzz = pad a\nlstick = key x\ngyro = pointer hold=up\nx = pad x hold=a\nlayer 2\ny = pad a\nlayer main\nlt = trigger right\nkeymap 0 bad\nkeymap x bad\nkeymap 3 again\n";
        let maps = Keymaps::load(text);
        assert_eq!(maps.list().len(), 1);
        let k = maps.get(3).unwrap();
        assert_eq!(k.name, "Turbo");
        assert_eq!(k.get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Key(chord("x"))), "an unknown option is skipped");
        assert_eq!(k.get(Input::Gyro).map(|b| b.hold), Some(None), "a D-pad direction doesn't hold the gyro");
        assert_eq!(k.get(Input::Button(Button::Y)), None, "a layer not known is skipped");
        assert_eq!(k.get(Input::Trigger(Side::Left)).map(|b| b.output), Some(Output::Trigger(Side::Right)), "main again");
        assert_eq!(k.bindings().len(), 3, "{k:?}");
        assert!(k.layer(Layer::Back).is_empty());
    }

    #[test]
    fn speeds_from_before_load_as_their_equivalents() {
        let text = "keymap 1 Old\nrstick = pointer speed=4\nlstick = pointer speed=1\ngyro = stick right speed=5\nlt = key x threshold=75\nrt = key y threshold=0\nkeymap 2 New\nrstick = pointer speed=4% deadzone=60\n";
        let maps = Keymaps::load(text);
        let get = |k: u16, i: Input| *maps.get(k).unwrap().get(i).unwrap();
        assert_eq!(get(1, Input::Stick(Side::Right)).speed, 75);
        assert_eq!(get(1, Input::Stick(Side::Left)).speed, 0);
        assert_eq!(get(1, Input::Gyro).speed, 100);
        assert_eq!(get(1, Input::Trigger(Side::Left)).threshold, 75, "thresholds were percent already");
        assert_eq!(get(1, Input::Trigger(Side::Right)).threshold, DEFAULT_THRESHOLD, "out of range");
        assert_eq!(get(2, Input::Stick(Side::Right)).speed, 4, "a percent");
        assert_eq!(get(2, Input::Stick(Side::Right)).deadzone, POINTER_DEADZONE, "out of range: its default");
        assert!(maps.text().contains("rstick = pointer speed=75%\n"), "{}", maps.text());
        let mut b = Binding::to(Output::Pointer);
        Param::Full.set(&mut b, 10);
        assert_eq!(b.full, 50);
        Param::Deadzone.set(&mut b, 90);
        Param::Threshold.set(&mut b, 0);
        assert_eq!((b.deadzone, b.threshold), (50, 1), "kept in range");
        let ps = |i, o| params(i, o).collect::<Vec<_>>();
        let stick = vec![Param::Deadzone, Param::Full, Param::Curve, Param::InvertX, Param::InvertY];
        assert_eq!(ps(Input::Stick(Side::Left), Output::Pointer), [vec![Param::Speed], stick.clone()].concat());
        assert_eq!(ps(Input::Stick(Side::Left), Output::Stick(Side::Left)), stick, "as on the pad: tuned all the same");
        let gyro = |stick: bool| {
            let mut v = vec![Param::Speed, Param::Accel, Param::AccelAt, Param::Tighten];
            if stick {
                v.push(Param::StickMin);
            }
            v.extend([Param::Twist, Param::InvertX, Param::InvertY, Param::Ratchet]);
            v
        };
        assert_eq!(ps(Input::Gyro, Output::Stick(Side::Right)), gyro(true));
        assert_eq!(ps(Input::Gyro, Output::Pointer), gyro(false));
        // Written and read back.
        let b = Binding { accel: 40, accel_at: 90, tighten: 25, stick_min: 15, twist: true, ratchet: true, hold: Some(Input::Button(Button::LB)), ..Binding::to(Output::Stick(Side::Right)) };
        let mut k = Keymap::new(1, "g");
        k.set(Input::Gyro, Some(b));
        let maps = Keymaps { list: vec![k] };
        assert!(maps.text().contains("gyro = stick right accel=40 accelat=90 tighten=25 stickmin=15 hold=lb twist ratchet\n"), "{}", maps.text());
        assert_eq!(Keymaps::load(&maps.text()), maps);
        assert_eq!(ps(Input::Gyro, Output::Nothing), vec![], "off");
        assert_eq!(ps(Input::Trigger(Side::Left), Output::Key(chord("x"))), vec![Param::Deadzone, Param::Full, Param::Threshold, Param::Turbo, Param::Toggle]);
        assert_eq!(ps(Input::Trigger(Side::Left), Output::Trigger(Side::Right)), vec![Param::Deadzone, Param::Full]);
        assert_eq!(ps(Input::Button(Button::A), Output::Key(chord("x"))), vec![Param::Turbo, Param::Toggle]);
        assert_eq!(ps(Input::Button(Button::A), Output::Nothing), vec![], "nothing has no turbo");
        let a = Input::Button(Button::A);
        assert_eq!(params_of(a, Output::Key(chord("x")), Actuation::Hold).collect::<Vec<_>>(), vec![Param::HoldTime, Param::Turbo, Param::Toggle]);
    }

    #[test]
    fn holds_doubles_turbo_and_toggle_are_kept_and_written() {
        let a = Input::Button(Button::A);
        let mut k = Keymap::new(4, "Combo");
        assert!(k.set(a, Some(Binding { turbo: 12, ..Binding::to(Output::Key(chord("x"))) })));
        assert!(k.set_of(Layer::Main, a, Actuation::Hold, Some(Binding { hold_time: 60, ..Binding::to(Output::Key(chord("esc"))) })));
        assert!(k.set_of(Layer::Main, a, Actuation::Double, Some(Binding { toggle: true, ..Binding::to(Output::Mouse(MouseButton::Left)) })));
        assert!(!k.set_of(Layer::Main, Input::Stick(Side::Left), Actuation::Hold, Some(Binding::to(Output::Pointer))), "a stick has no hold");
        assert!(k.set_of(Layer::Back, a, Actuation::Hold, Some(Binding::to(Output::Key(chord("tab"))))));
        assert_eq!(k.lookup_of(a, Actuation::Hold, true).map(|b| b.output), Some(Output::Key(chord("tab"))));
        assert_eq!(k.lookup_of(a, Actuation::Double, true).map(|b| b.toggle), Some(true), "falls through");
        let maps = Keymaps { list: vec![k] };
        let text = maps.text();
        assert_eq!(text, "keymap 4 Combo\na = key x turbo=12\na hold = key esc holdtime=60\na double = mouse left toggle\nlayer back\na hold = key tab\n");
        assert_eq!(Keymaps::load(&text), maps);
        assert!(!maps.list.first().unwrap().is_empty(Layer::Back));
        // Shown: turbo off or a rate, a hold's time.
        let show = |p: Param, v| {
            let mut s = String::new();
            p.show(v, &mut s).unwrap();
            s
        };
        assert_eq!((show(Param::Turbo, 0), show(Param::Turbo, 12), show(Param::HoldTime, 40), show(Param::Curve, 30)), ("Off".into(), "12/s".into(), "400 ms".into(), "30 %".into()));
    }

    #[test]
    fn a_stick_on_the_pointer_has_defaults_of_its_own() {
        let rs = Input::Stick(Side::Right);
        let b = Binding::of(rs, Output::Pointer);
        assert_eq!((b.deadzone, b.curve), (POINTER_DEADZONE, POINTER_CURVE));
        assert!(!b.tuned(rs));
        // Back to the stick: those defaults go with it; a value set stays.
        let own = b.retarget(rs, Output::Stick(Side::Right));
        assert_eq!((own.deadzone, own.curve), (0, 0));
        let set = Binding { deadzone: 20, ..b }.retarget(rs, Output::Stick(Side::Right));
        assert_eq!((set.deadzone, set.curve), (20, 0));
        // Written only where it differs from them; read back with them.
        let mut maps = Keymaps::initial();
        maps.get_mut(1).unwrap().set(rs, Some(Binding { deadzone: 0, ..b }));
        assert!(maps.text().contains("rstick = pointer deadzone=0\n"), "{}", maps.text());
        assert_eq!(Keymaps::load(&maps.text()), maps);
        assert_eq!(Keymaps::load("keymap 1 K\nrstick = pointer\n").get(1).unwrap().get(rs), Some(&b));
    }

    #[test]
    fn an_input_only_tuned_counts_as_changed_but_not_remapped() {
        let ls = Input::Stick(Side::Left);
        let own = Binding::to(Output::Stick(Side::Left));
        assert_eq!(change(ls, None), Change::None);
        assert_eq!(change(ls, Some(&own)), Change::None, "as on the pad, said");
        assert_eq!(change(ls, Some(&Binding { deadzone: 12, ..own })), Change::Tuned);
        assert_eq!(change(ls, Some(&Binding { deadzone: 12, ..Binding::to(Output::Pointer) })), Change::Mapped);
        assert_eq!(change(Input::Button(Button::A), Some(&Binding::to(Output::Pad(PadOut::Button(Button::B))))), Change::Mapped);
        let text = "keymap 1 T\nlstick = stick left deadzone=12 invert=xy\n";
        let maps = Keymaps::load(text);
        assert_eq!(maps.get(1).unwrap().get(ls), Some(&Binding { deadzone: 12, invert_x: true, invert_y: true, ..own }));
        assert_eq!(maps.text(), text);
    }

    #[test]
    fn the_back_layer_round_trips_and_old_files_load_unchanged() {
        let old = "keymap 1 Nintendo layout\na = pad b\nb = pad a\nx = pad y\ny = pad x\n";
        let maps = Keymaps::load(old);
        assert_eq!(maps, Keymaps::initial(), "a file from before layers");
        assert_eq!(maps.text(), old, "and written the same");
        let mut maps = Keymaps::initial();
        let k = maps.get_mut(1).unwrap();
        assert!(k.set_in(Layer::Back, Input::Button(Button::A), Some(Binding::to(Output::Key(chord("ctrl+y"))))));
        assert!(k.set_in(Layer::Back, Input::Gyro, Some(Binding { hold: Some(Input::Button(Button::RB)), ..Binding::to(Output::Pointer) })));
        assert!(k.set_in(Layer::Back, Input::Button(Button::B), as_on_the_pad(Input::Button(Button::B)).map(Binding::to)));
        let text = maps.text();
        assert_eq!(text, format!("{old}layer back\na = key ctrl+y\nb = pad b\ngyro = pointer hold=rb\n"));
        assert_eq!(Keymaps::load(&text), maps);
        // A copy has both layers; clearing one leaves the other.
        let copy = maps.duplicate(1, "Copy").unwrap();
        let k = maps.get_mut(copy).unwrap();
        assert_eq!(k.layer(Layer::Back).len(), 3);
        assert!(k.clear(Layer::Back) && !k.clear(Layer::Back));
        assert_eq!(k.bindings().len(), 4);
    }

    #[test]
    fn the_back_layer_falls_through_to_the_main_one() {
        let mut k = nintendo(1);
        let a = Input::Button(Button::A);
        let x = Input::Button(Button::X);
        k.set_in(Layer::Back, a, Some(Binding::to(Output::Mouse(MouseButton::Left))));
        k.set_in(Layer::Back, x, as_on_the_pad(x).map(Binding::to));
        let out = |input, back| k.lookup(input, back).map(|b| b.output);
        assert_eq!(out(a, false), Some(Output::Pad(PadOut::Button(Button::B))), "main");
        assert_eq!(out(a, true), Some(Output::Mouse(MouseButton::Left)), "the Back layer's own");
        assert_eq!(out(Input::Button(Button::B), true), Some(Output::Pad(PadOut::Button(Button::A))), "not in it: main's");
        assert_eq!(out(x, true), Some(Output::Pad(PadOut::Button(Button::X))), "as on the pad, over main's Y");
        assert_eq!(out(Input::Button(Button::LB), true), None, "in neither: as on the pad");
        assert_eq!(as_on_the_pad(Input::StickDir(Side::Left, Dir::Up)), None);
        assert!(Input::all().filter_map(|i| as_on_the_pad(i).map(|o| (i, o))).all(|(i, o)| o.fits(i.kind())));
    }

    #[test]
    fn adding_renaming_duplicating_removing() {
        let mut maps = Keymaps::initial();
        let a = maps.add("Racing");
        assert_eq!(maps.add("Racing"), 3);
        assert_eq!(maps.get(3).map(|k| k.name.as_str()), Some("Racing 2"), "names stay apart");
        maps.get_mut(a).unwrap().set(Input::Button(Button::A), Some(Binding::to(Output::Pad(PadOut::Trigger(Side::Right)))));
        let copy = maps.duplicate(a, "Racing").unwrap();
        assert_eq!(maps.get(copy).map(|k| (k.name.as_str(), k.bindings().len())), Some(("Racing 3", 1)));
        assert!(maps.rename(a, "Rally"));
        assert!(maps.rename(a, "Rally"), "its own name again");
        assert_eq!(maps.get(a).map(|k| k.name.as_str()), Some("Rally"));
        assert!(maps.remove(copy) && !maps.remove(copy));
        assert_eq!(maps.add("New"), 4, "ids are never taken twice while the highest stays");
        assert_eq!(nintendo(1).get(Input::Button(Button::X)).map(|b| b.output), Some(Output::Pad(PadOut::Button(Button::Y))));
    }

    #[test]
    fn inputs_by_name() {
        for input in Input::all() {
            let mut name = String::new();
            input.name(&mut name).unwrap();
            assert_eq!(Input::parse(&name), Some(input), "{name}");
        }
        assert_eq!(Input::all().count(), 14 + 4 + 10 + 2 + 1);
        assert_eq!(PadOut::all().count(), 16);
    }
}
