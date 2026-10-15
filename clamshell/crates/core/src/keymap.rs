//! Keymaps: named remappings of the controller, shared between games (a game's or all games' setting,
//! [`crate::settings::Setting::Keymap`], is a keymap's id; 0 is Default, no remapping). Each maps some of
//! the pad's inputs ([`Input`]) to something else ([`Output`]); an input without a binding does what it
//! does on the pad. What an input may map to depends on its kind ([`Kind`]): a button to a pad button, a
//! key chord, a mouse button or a wheel step; a stick to a stick, the mouse pointer, four keys or the
//! D-pad; a trigger to a trigger or, past a threshold, anything a button maps to; the gyro to the mouse
//! pointer or a stick, always or while a chosen button is held.
//!
//! Saved in `keymaps.txt` (next to `game-settings.txt`): a `keymap ID NAME` line starts a keymap, then a
//! line per binding, `INPUT = OUTPUT [option=value ...]`. Lines it doesn't understand are skipped, so
//! later features (turbo, toggles, layers) can add their own.

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

/// The speed steps of motion mapped to a stick or the pointer, 1 to 5.
pub const SPEEDS: [u8; 5] = [1, 2, 3, 4, 5];
pub const DEFAULT_SPEED: u8 = 3;
/// A trigger mapped to a button acts past this (percent of its travel).
pub const THRESHOLDS: [u8; 3] = [25, 50, 75];
pub const DEFAULT_THRESHOLD: u8 = 50;

/// An input's binding: what it maps to, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding {
    pub output: Output,
    /// Motion to the pointer or a stick: one of [`SPEEDS`].
    pub speed: u8,
    /// A trigger to a button: percent of its travel.
    pub threshold: u8,
    /// The gyro acts only while this is held (a button, or a trigger past half); always without one.
    pub hold: Option<Input>,
}

impl Binding {
    pub fn to(output: Output) -> Binding {
        Binding { output, speed: DEFAULT_SPEED, threshold: DEFAULT_THRESHOLD, hold: None }
    }
}

/// What may hold the gyro on: the buttons and the triggers.
pub fn gyro_holds() -> impl Iterator<Item = Input> {
    Button::ALL.into_iter().map(Input::Button).chain(Side::ALL.into_iter().map(Input::Trigger))
}

/// A named set of bindings (the inputs without one do what they do on the pad).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    /// Its number, kept across renames (0 is Default, which no keymap has).
    pub id: u16,
    pub name: String,
    /// In [`Input::all`]'s order, one per input at most.
    bindings: Vec<(Input, Binding)>,
}

impl Keymap {
    pub fn new(id: u16, name: &str) -> Keymap {
        Keymap { id, name: name.to_owned(), bindings: Vec::new() }
    }

    pub fn bindings(&self) -> &[(Input, Binding)] {
        &self.bindings
    }

    pub fn get(&self, input: Input) -> Option<&Binding> {
        self.bindings.iter().find(|(i, _)| *i == input).map(|(_, b)| b)
    }

    /// Binds `input` (none: back to what it does on the pad). A binding that doesn't fit the input's
    /// kind is refused; whether it changed.
    pub fn set(&mut self, input: Input, binding: Option<Binding>) -> bool {
        if binding.is_some_and(|b| !b.output.fits(input.kind())) {
            return false;
        }
        let before = self.bindings.clone();
        self.bindings.retain(|(i, _)| *i != input);
        if let Some(b) = binding {
            self.bindings.push((input, b));
            self.bindings.sort_by_key(|(i, _)| *i);
        }
        self.bindings != before
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
        let bindings = self.get(id)?.bindings.clone();
        let copy = self.add(name);
        if let Some(k) = self.get_mut(copy) {
            k.bindings = bindings;
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
            for (input, b) in &k.bindings {
                let _ = input.name(&mut text);
                text.push_str(" = ");
                let _ = write_output(&b.output, &mut text);
                if matches!(b.output, Output::Stick(_) | Output::Pointer) && b.speed != DEFAULT_SPEED {
                    let _ = write!(text, " speed={}", b.speed);
                }
                if input.kind() == Kind::Trigger && b.output.digital() && b.threshold != DEFAULT_THRESHOLD {
                    let _ = write!(text, " threshold={}", b.threshold);
                }
                if let Some(hold) = b.hold {
                    text.push_str(" hold=");
                    let _ = hold.name(&mut text);
                }
                text.push('\n');
            }
        }
        text
    }

    /// Reads [`Keymaps::text`] back; lines it doesn't understand are skipped.
    pub fn load(text: &str) -> Keymaps {
        let mut keymaps = Keymaps::default();
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("keymap ") {
                let (id, name) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Ok(id) = id.parse::<u16>()
                    && id > 0
                    && keymaps.get(id).is_none()
                {
                    keymaps.list.push(Keymap::new(id, name.trim()));
                }
                continue;
            }
            let (Some(keymap), Some((input, binding))) = (keymaps.list.last_mut(), parse_binding(line)) else {
                continue;
            };
            keymap.set(input, Some(binding));
        }
        keymaps
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

/// `INPUT = OUTPUT [option=value ...]`.
fn parse_binding(line: &str) -> Option<(Input, Binding)> {
    let (input, rest) = line.split_once('=')?;
    let input = Input::parse(input.trim())?;
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
    let mut binding = Binding::to(output);
    for word in words {
        match word.split_once('=') {
            Some(("speed", v)) => binding.speed = v.parse().ok().filter(|v| SPEEDS.contains(v)).unwrap_or(DEFAULT_SPEED),
            Some(("threshold", v)) => binding.threshold = v.parse().ok().filter(|v| THRESHOLDS.contains(v)).unwrap_or(DEFAULT_THRESHOLD),
            Some(("hold", v)) => binding.hold = Input::parse(v).filter(|h| gyro_holds().any(|g| g == *h)),
            // (Options of later features.)
            _ => {}
        }
    }
    (input.kind() == Kind::Gyro || binding.hold.is_none()).then_some((input, binding))
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
        k.set(Input::Stick(Side::Right), Some(Binding { speed: 4, ..Binding::to(Output::Pointer) }));
        k.set(Input::Stick(Side::Left), Some(Binding::to(Output::Keys([chord("w"), chord("a"), chord("s"), chord("d")]))));
        k.set(Input::Trigger(Side::Left), Some(Binding { threshold: 25, ..Binding::to(Output::Mouse(MouseButton::Right)) }));
        k.set(Input::Trigger(Side::Right), Some(Binding::to(Output::Trigger(Side::Left))));
        k.set(Input::StickDir(Side::Right, Dir::Up), Some(Binding::to(Output::Wheel(Dir::Up))));
        k.set(Input::Dpad(Dir::Down), Some(Binding::to(Output::Nothing)));
        k.set(Input::Button(Button::L4), Some(Binding::to(Output::Pad(PadOut::Dpad(Dir::Left)))));
        k.set(Input::Gyro, Some(Binding { speed: 2, hold: Some(Input::Trigger(Side::Left)), ..Binding::to(Output::Pointer) }));
        let text = maps.text();
        assert!(text.starts_with("keymap 1 Nintendo layout\na = pad b\nb = pad a\nx = pad y\ny = pad x\nkeymap 2 FPS\na = key ctrl+x\nl4 = pad left\n"), "{text}");
        assert!(text.contains("\nlstick = keys w a s d\n") && text.contains("\nrstick = pointer speed=4\n") && text.contains("\ngyro = pointer speed=2 hold=lt\n"), "{text}");
        assert!(text.contains("\nlt = mouse right threshold=25\n") && text.contains("\nrt = trigger left\n") && text.contains("\nrstick-up = wheel up\n"), "{text}");
        assert_eq!(Keymaps::load(&text), maps);
    }

    #[test]
    fn lines_not_understood_are_skipped() {
        let text = "# a comment\nkeymap 3 Turbo\na = key x turbo=10\nb = warp 9\nlayer 2\nzz = pad a\nlstick = key x\ngyro = pointer hold=up\nx = pad x hold=a\nkeymap 0 bad\nkeymap x bad\nkeymap 3 again\n";
        let maps = Keymaps::load(text);
        assert_eq!(maps.list().len(), 1);
        let k = maps.get(3).unwrap();
        assert_eq!(k.name, "Turbo");
        assert_eq!(k.get(Input::Button(Button::A)).map(|b| b.output), Some(Output::Key(chord("x"))), "an unknown option is skipped");
        assert_eq!(k.get(Input::Gyro).map(|b| b.hold), Some(None), "a D-pad direction doesn't hold the gyro");
        assert_eq!(k.bindings().len(), 2, "{k:?}");
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
