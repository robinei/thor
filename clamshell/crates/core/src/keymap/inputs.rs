//! The controller's inputs: its buttons, the directions, the sides, an input and its kind, and the
//! game's pad's own outputs (PadOut).

use super::*;

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
    /// The Thor's Back button, under the screen: not on the Xbox pad games see.
    Back,
    /// The back buttons (paddles), where a controller has them.
    L4,
    R4,
    L5,
    R5,
}

impl Button {
    pub const ALL: [Button; 15] = [
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
        Button::Back,
        Button::L4,
        Button::R4,
        Button::L5,
        Button::R5,
    ];

    /// Whether it is a back button (a paddle; not on the Xbox pad games see).
    pub fn is_back(self) -> bool {
        matches!(self, Button::L4 | Button::R4 | Button::L5 | Button::R5)
    }

    /// Whether the Xbox pad games see has it (a pad output can be it): not the paddles, not Back.
    pub fn on_pad(self) -> bool {
        !self.is_back() && self != Button::Back
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
            Button::Back => "back",
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
            Button::Back => "Back",
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
        let buttons = Button::ALL.into_iter().filter(|b| b.on_pad()).map(PadOut::Button);
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
            Input::Button(b) if b.on_pad() => Some(PadOut::Button(b)),
            Input::Dpad(d) => Some(PadOut::Dpad(d)),
            Input::Trigger(s) => Some(PadOut::Trigger(s)),
            _ => None,
        }
    }
}
