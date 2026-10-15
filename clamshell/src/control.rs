//! The control pipe (CLAMSHELL_CONTROL): tests drive the program through it, one command per line, as
//! the events a user's taps and presses would make. The grammar:
//!
//! - `tap TARGET`, `long TARGET` (a tap, a long press) on: `app N` (the dock's apps, by position in
//!   launcher.json), `plus N` (another instance of app N), `game` / `game N` (the dock's game tiles),
//!   `start-game`, `launch`, `keep`, `close` (shelf), `network N`, `wifi-power`, `device N`,
//!   `bluetooth-power`, `pad-mouse`, `save` (the editor)
//! - `tab NAME`: open a tab (dock, wifi, keyboard, ...)
//! - `home`; `list N`: move shelf's selection by N
//! - `type TEXT`: type on the on-screen keyboard
//! - `slide volume|brightness FROM TO [hold]`: a finger down at FROM on a 620-wide layout, moved to TO,
//!   lifted unless held; `lift volume|brightness` lifts it
//! - `level volume|brightness V`, `level backlight|stream N V`: a level row in the volume or brightness
//!   tab set to V
//! - `pad up|down|left|right|a|b|y|lb|rb|switch`: the pad while shelf is in front (`a` presses and
//!   releases)

use std::fmt;
use std::io::BufRead;
use std::path::PathBuf;

use clamshell_core::keyboard::{self, Key, Modifier};
use clamshell_core::levels::Level;
use clamshell_core::slider::Control;
use clamshell_core::{
    BtId, DockId, EditorId, Event, Id, LevelEvent, PadInput, PointerEvent, ShelfEvent, ShelfId, Slide, Tab, WifiId,
};
use clamshell_policy::{Sink, worker};

/// The layout width a slide's positions are on (the second screen at half size).
const SLIDE_WIDTH: f32 = 620.0;
/// The steps a slide moves in.
const SLIDE_STEPS: u16 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not a command of the grammar.
    Unknown,
    /// A number that doesn't parse.
    Number(String),
    /// A name it doesn't know (a slider, a tab, a pad button).
    Name(String),
    /// A character the on-screen keyboard has no key for.
    Untypeable(char),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Unknown => write!(f, "not a command"),
            ParseError::Number(w) => write!(f, "not a number: {w:?}"),
            ParseError::Name(w) => write!(f, "unknown name: {w:?}"),
            ParseError::Untypeable(c) => write!(f, "no key types {c:?}"),
        }
    }
}

fn number<T: std::str::FromStr>(word: &str) -> Result<T, ParseError> {
    word.parse().map_err(|_| ParseError::Number(word.to_owned()))
}

fn control(name: &str) -> Result<Control, ParseError> {
    match name {
        "volume" => Ok(Control::Volume),
        "brightness" => Ok(Control::Brightness),
        other => Err(ParseError::Name(other.to_owned())),
    }
}

/// A line as the events it stands for.
pub fn parse(line: &str) -> Result<Vec<Event>, ParseError> {
    // The text to type is the rest of the line, as it is.
    if let Some(text) = line.strip_prefix("type ") {
        return type_text(text);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["home"] => vec![Event::Home],
        ["list", step] => vec![ShelfEvent::ListStep(number(step)?).into()],
        ["tab", name] => vec![PointerEvent::Activate(Id::Tab(Tab::from_name(name).ok_or_else(|| ParseError::Name((*name).to_owned()))?)).into()],
        ["pad", button] => pad(button)?,
        // (A slide never ends in a tap: that would open the tab.)
        ["lift", name] => vec![Slide::End.into(), PointerEvent::Up(Id::Control(control(name)?)).into()],
        ["slide", name, from, to, rest @ ..] => {
            let hold = match rest {
                [] => false,
                ["hold"] => true,
                _ => return Err(ParseError::Unknown),
            };
            slide(control(name)?, number(from)?, number(to)?, hold)
        }
        ["level", rest @ ..] => {
            let (level, value) = match rest {
                ["volume", v] => (Level::Volume, v),
                ["brightness", v] => (Level::Brightness, v),
                ["backlight", n, v] => (Level::Backlight(number(n)?), v),
                ["stream", n, v] => (Level::Stream(number(n)?), v),
                _ => return Err(ParseError::Unknown),
            };
            vec![LevelEvent::Set(level, number(value)?).into(), PointerEvent::Up(Id::Level(level)).into()]
        }
        ["tap", target @ ..] => vec![PointerEvent::Activate(target_id(target)?).into()],
        ["long", target @ ..] => vec![PointerEvent::LongPress(target_id(target)?).into()],
        _ => return Err(ParseError::Unknown),
    })
}

/// A finger down on a slider at `from`, moved to `to` in steps, lifted unless held.
fn slide(control: Control, from: f32, to: f32, hold: bool) -> Vec<Event> {
    let id = Id::Control(control);
    let mut events = vec![PointerEvent::Down(id.clone()).into(), Slide::Start { control, x: from, width: SLIDE_WIDTH }.into()];
    events.extend((1..=SLIDE_STEPS).map(|i| Slide::To { x: from + (to - from) * f32::from(i) / f32::from(SLIDE_STEPS) }.into()));
    if !hold {
        events.extend([Slide::End.into(), PointerEvent::Up(id).into()]);
    }
    events
}

fn pad(button: &str) -> Result<Vec<Event>, ParseError> {
    let input = |i: PadInput| vec![Event::Pad(i)];
    Ok(match button {
        "up" => input(PadInput::Up),
        "down" => input(PadInput::Down),
        "left" => input(PadInput::Left),
        "right" => input(PadInput::Right),
        "a" => vec![Event::Pad(PadInput::A(true)), Event::Pad(PadInput::A(false))],
        "b" => input(PadInput::B),
        "y" => input(PadInput::Y),
        "lb" => input(PadInput::Shoulder(-1)),
        "rb" => input(PadInput::Shoulder(1)),
        "switch" => input(PadInput::Switch),
        other => return Err(ParseError::Name(other.to_owned())),
    })
}

/// The target a tap or a long press is on.
fn target_id(words: &[&str]) -> Result<Id, ParseError> {
    Ok(match words {
        ["app", n] => Id::Dock(DockId::App(number(n)?)),
        ["plus", n] => Id::Dock(DockId::NewInstance(number(n)?)),
        ["game"] => Id::Dock(DockId::GameTile(0)),
        ["game", n] => Id::Dock(DockId::GameTile(number(n)?)),
        ["start-game"] => Id::Dock(DockId::StartGame),
        ["network", n] => Id::Wifi(WifiId::Network(number(n)?)),
        ["wifi-power"] => Id::Wifi(WifiId::Power),
        ["device", n] => Id::Bluetooth(BtId::Device(number(n)?)),
        ["bluetooth-power"] => Id::Bluetooth(BtId::Power),
        ["pad-mouse"] => Id::PadMouse,
        ["save"] => Id::Editor(EditorId::Save),
        ["keep"] => Id::Shelf(ShelfId::StartKeep),
        ["close"] => Id::Shelf(ShelfId::StartClose),
        ["launch"] => Id::Shelf(ShelfId::Launch),
        _ => return Err(ParseError::Unknown),
    })
}

/// A key of the on-screen keyboard.
pub fn key(key: Key) -> Result<Id, ParseError> {
    let untypeable = || ParseError::Untypeable(if let Key::Char(c) = key { c } else { '?' });
    keyboard::find(key).map(Id::Key).ok_or_else(untypeable)
}

/// Text typed on the on-screen keyboard: each character's key, after Shift (once) if it needs it.
pub fn type_text(text: &str) -> Result<Vec<Event>, ParseError> {
    let tap = |k: Key| key(k).map(|id| Event::Pointer(PointerEvent::Activate(id)));
    let mut events = Vec::new();
    for c in text.chars() {
        let char_key = |k: Key| tap(k).map_err(|_| ParseError::Untypeable(c));
        match (' '..='~').find(|&b| b != c && keyboard::shifted(b) == c) {
            Some(base) => events.extend([char_key(Key::Mod(Modifier::Shift))?, char_key(Key::Char(base))?]),
            None => events.push(char_key(Key::Char(c))?),
        }
    }
    Ok(events)
}

/// Reads commands from the pipe, one per line, for as long as the program runs.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    let spawned = worker::detached("control", move || {
        // Each writer closes the pipe after its line: open it again for the next.
        while let Ok(file) = std::fs::File::open(&path) {
            for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
                match parse(&line) {
                    Ok(events) => {
                        if events.into_iter().try_for_each(|e| sink.send(e)).is_err() {
                            return;
                        }
                    }
                    Err(e) => log::warn!("control: {line:?}: {e}"),
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("control: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn activate(id: Id) -> Vec<Event> {
        vec![PointerEvent::Activate(id).into()]
    }

    #[test]
    fn taps_and_long_presses() {
        assert_eq!(parse("tap app 1"), Ok(activate(Id::Dock(DockId::App(1)))));
        assert_eq!(parse("long app 0"), Ok(vec![PointerEvent::LongPress(Id::Dock(DockId::App(0))).into()]));
        assert_eq!(parse("tap plus 2"), Ok(activate(Id::Dock(DockId::NewInstance(2)))));
        assert_eq!(parse("tap game"), Ok(activate(Id::Dock(DockId::GameTile(0)))));
        assert_eq!(parse("long game 1"), Ok(vec![PointerEvent::LongPress(Id::Dock(DockId::GameTile(1))).into()]));
        assert_eq!(parse("tap start-game"), Ok(activate(Id::Dock(DockId::StartGame))));
        assert_eq!(parse("tap launch"), Ok(activate(Id::Shelf(ShelfId::Launch))));
        assert_eq!(parse("tap keep"), Ok(activate(Id::Shelf(ShelfId::StartKeep))));
        assert_eq!(parse("tap close"), Ok(activate(Id::Shelf(ShelfId::StartClose))));
        assert_eq!(parse("tap network 2"), Ok(activate(Id::Wifi(WifiId::Network(2)))));
        assert_eq!(parse("long network 0"), Ok(vec![PointerEvent::LongPress(Id::Wifi(WifiId::Network(0))).into()]));
        assert_eq!(parse("tap wifi-power"), Ok(activate(Id::Wifi(WifiId::Power))));
        assert_eq!(parse("tap device 3"), Ok(activate(Id::Bluetooth(BtId::Device(3)))));
        assert_eq!(parse("tap bluetooth-power"), Ok(activate(Id::Bluetooth(BtId::Power))));
        assert_eq!(parse("tap pad-mouse"), Ok(activate(Id::PadMouse)));
        assert_eq!(parse("tap save"), Ok(activate(Id::Editor(EditorId::Save))));
        assert_eq!(parse("poke app 1"), Err(ParseError::Unknown));
        assert_eq!(parse("tap app x"), Err(ParseError::Number("x".into())));
        assert_eq!(parse("tap nothing"), Err(ParseError::Unknown));
    }

    #[test]
    fn tabs_home_and_the_list() {
        assert_eq!(parse("tab wifi"), Ok(activate(Id::Tab(Tab::Wifi))));
        assert_eq!(parse("tab nowhere"), Err(ParseError::Name("nowhere".into())));
        assert_eq!(parse("home"), Ok(vec![Event::Home]));
        assert_eq!(parse("list -2"), Ok(vec![ShelfEvent::ListStep(-2).into()]));
    }

    #[test]
    fn the_pad() {
        assert_eq!(parse("pad a"), Ok(vec![Event::Pad(PadInput::A(true)), Event::Pad(PadInput::A(false))]));
        assert_eq!(parse("pad lb"), Ok(vec![Event::Pad(PadInput::Shoulder(-1))]));
        assert_eq!(parse("pad switch"), Ok(vec![Event::Pad(PadInput::Switch)]));
        assert_eq!(parse("pad x"), Err(ParseError::Name("x".into())));
    }

    #[test]
    fn slides_and_lifts() {
        let events = parse("slide volume 280 330").unwrap();
        let id = Id::Control(Control::Volume);
        assert_eq!(events[..2], [PointerEvent::Down(id.clone()).into(), Slide::Start { control: Control::Volume, x: 280.0, width: 620.0 }.into()]);
        assert_eq!(events[2], Slide::To { x: 285.0 }.into());
        assert_eq!(events[11], Slide::To { x: 330.0 }.into());
        assert_eq!(events[12..], [Slide::End.into(), PointerEvent::Up(id.clone()).into()]);
        assert_eq!(parse("slide brightness 100 200 hold").unwrap().len(), 12, "held: not lifted");
        assert_eq!(parse("slide volume 1 2 later"), Err(ParseError::Unknown));
        assert_eq!(parse("lift volume"), Ok(vec![Slide::End.into(), PointerEvent::Up(id).into()]));
        assert_eq!(parse("lift treble"), Err(ParseError::Name("treble".into())));
    }

    #[test]
    fn levels() {
        assert_eq!(parse("level volume 40"), Ok(vec![LevelEvent::Set(Level::Volume, 40).into(), PointerEvent::Up(Id::Level(Level::Volume)).into()]));
        assert_eq!(parse("level backlight 1 65"), Ok(vec![LevelEvent::Set(Level::Backlight(1), 65).into(), PointerEvent::Up(Id::Level(Level::Backlight(1))).into()]));
        assert_eq!(parse("level stream 0 5").map(|e| e.len()), Ok(2));
        assert_eq!(parse("level volume loud"), Err(ParseError::Number("loud".into())));
        assert_eq!(parse("level treble 3"), Err(ParseError::Unknown));
    }

    #[test]
    fn typing() {
        let shift = Event::Pointer(PointerEvent::Activate(key(Key::Mod(Modifier::Shift)).unwrap()));
        let char_key = |c| Event::Pointer(PointerEvent::Activate(key(Key::Char(c)).unwrap()));
        assert_eq!(parse("type a!"), Ok(vec![char_key('a'), shift.clone(), char_key('1')]));
        assert_eq!(parse("type  x"), Ok(vec![char_key(' '), char_key('x')]), "the text as it is");
        assert_eq!(type_text("é"), Err(ParseError::Untypeable('é')));
    }
}
