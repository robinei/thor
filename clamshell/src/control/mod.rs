//! The control pipe, at $XDG_RUNTIME_DIR/clamshell-control or CLAMSHELL_CONTROL. `clamshell ctl` and the
//! tests send commands through it, one per line. Each command becomes the events that the device's keys
//! or a user's taps and presses would make.
//!
//! The commands are the rows of [`COMMANDS`], and the targets a tap or a long press can be on are the rows
//! of [`TARGETS`]. Each row has its words (placeholders in capitals), what it does, and the events or
//! target it makes. `clamshell ctl help` lists them.
//!
//! Any of the user's processes can write to the pipe (mode 0600); see docs/security.md.

mod pipe;
mod words;

use std::fmt;

use clamshell_core::Control;
use clamshell_core::keymap::Param;
use clamshell_core::{Event, PointerEvent};

pub use pipe::{send, spawn};
pub use words::type_text;
use words::{COMMANDS, TARGETS};
// The snapshots tap keys by name.
#[cfg(any(test, feature = "dev"))]
pub use words::key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not a command of the grammar.
    Unknown,
    /// A number that doesn't parse.
    Number(String),
    /// An unknown name, such as a slider, a tab or a pad button.
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

pub(super) fn number<T: std::str::FromStr>(word: &str) -> Result<T, ParseError> {
    word.parse().map_err(|_| ParseError::Number(word.to_owned()))
}

pub(super) fn control(name: &str) -> Result<Control, ParseError> {
    match name {
        "volume" => Ok(Control::Volume),
        "brightness" => Ok(Control::Brightness),
        other => Param::from_name(other).filter(|p| !p.switch()).map(Control::Param).ok_or_else(|| name_err(other)),
    }
}

/// A command or a target, as a row of the tables. It has its words, what it does (for `clamshell ctl
/// help`), and a function that makes the result from the placeholders' words. Each word is either literal
/// or a placeholder in capitals that any word fills, such as `N` or `INPUT`.
pub struct Form<T> {
    pub words: &'static str,
    pub help: &'static str,
    make: fn(&[&str]) -> Result<T, ParseError>,
}

pub(super) const fn form<T>(words: &'static str, help: &'static str, make: fn(&[&str]) -> Result<T, ParseError>) -> Form<T> {
    Form { words, help, make }
}

/// Returns the placeholders' words if `words` fit `pattern`, which means the same number of words and
/// every literal matching.
pub(super) fn fits<'a>(pattern: &str, words: &[&'a str]) -> Option<Vec<&'a str>> {
    let tokens: Vec<&str> = pattern.split(' ').collect();
    if tokens.len() != words.len() {
        return None;
    }
    let mut args = Vec::new();
    for (t, w) in tokens.iter().zip(words) {
        if t.chars().all(|c| c.is_ascii_uppercase()) {
            args.push(*w);
        } else if t != w {
            return None;
        }
    }
    Some(args)
}

/// Makes the result of the first row that `words` fit. If no row fits, the line is not a command.
pub(super) fn by_table<T>(table: &[Form<T>], words: &[&str]) -> Result<T, ParseError> {
    for f in table {
        if let Some(args) = fits(f.words, words) {
            return (f.make)(&args);
        }
    }
    Err(ParseError::Unknown)
}

/// The `i`th placeholder's word, or an empty string.
pub(super) fn arg<'a>(args: &[&'a str], i: usize) -> &'a str {
    args.get(i).copied().unwrap_or("")
}

pub(super) fn name_err(word: &str) -> ParseError {
    ParseError::Name(word.to_owned())
}

/// The help text: the commands and targets, a line each.
pub fn help() -> String {
    let mut text = String::from("Commands (`type TEXT` types the rest of the line):\n");
    let line = |t: &mut String, words: &str, help: &str| t.push_str(&format!("  {words:<26} {help}\n"));
    for f in COMMANDS {
        line(&mut text, f.words, f.help);
    }
    text.push_str("\nTargets of `tap TARGET` and `long TARGET`:\n");
    for f in TARGETS {
        line(&mut text, f.words, f.help);
    }
    text
}

/// Parses a line into the events it stands for.
pub fn parse(line: &str) -> Result<Vec<Event>, ParseError> {
    // The text to type is the rest of the line, as it is.
    if let Some(text) = line.strip_prefix("type ") {
        return type_text(text);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["tap", target @ ..] => Ok(vec![PointerEvent::Activate(by_table(TARGETS, target)?).into()]),
        ["long", target @ ..] => Ok(vec![PointerEvent::LongPress(by_table(TARGETS, target)?).into()]),
        words => by_table(COMMANDS, words),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_core::bottom::keyboard::{Key, Modifier};
    use clamshell_core::keymap::{Actuation, Dir, Input, Layer, MouseButton, Output, PadOut, Side};
    use clamshell_core::levels::Level;
    use clamshell_core::{BtId, DockId, EditorId, Id, KeymapId, LevelEvent, ListId, PadInput, RowPart, ShelfEvent, ShelfId, Slide, Tab, Target, WifiId};

    fn activate(id: Id) -> Vec<Event> {
        vec![PointerEvent::Activate(id).into()]
    }

    #[test]
    fn taps_and_long_presses() {
        assert_eq!(parse("tap dock 1"), Ok(activate(Id::Dock(DockId::Entry(1)))));
        assert_eq!(parse("long dock 0"), Ok(vec![PointerEvent::LongPress(Id::Dock(DockId::Entry(0))).into()]));
        assert_eq!(parse("tap dock-menu open-another"), Ok(activate(Id::Dock(DockId::OpenAnother))));
        assert_eq!(parse("tap dock-tray 2"), Ok(activate(Id::Dock(DockId::Tray(2)))));
        assert_eq!(parse("tap tray-entry 6"), Ok(activate(Id::Dock(DockId::TrayEntry(6)))));
        assert_eq!(parse("tap dock-step right"), Ok(activate(Id::Dock(DockId::Step(clamshell_core::keymap::Dir::Right)))));
        assert_eq!(parse("tap launch"), Ok(activate(Id::Shelf(ShelfId::Launch))));
        assert_eq!(parse("tap collection 1"), Ok(activate(Id::Shelf(ShelfId::Collection(1)))));
        assert_eq!(parse("tap up"), Ok(activate(Id::Shelf(ShelfId::Up))));
        assert_eq!(parse("tap file new-folder"), Ok(activate(Id::Shelf(ShelfId::File(clamshell_core::shelf::files::FileAction::NewFolder)))));
        assert_eq!(parse("tap network 2"), Ok(activate(ListId::Wifi.row(2))));
        assert_eq!(parse("long network 0"), Ok(vec![PointerEvent::LongPress(ListId::Wifi.row(0)).into()]));
        assert_eq!(parse("tap wifi-power"), Ok(activate(Id::Wifi(WifiId::Power))));
        assert_eq!(parse("tap device 3"), Ok(activate(ListId::Bluetooth.row(3))));
        assert_eq!(parse("tap bluetooth-power"), Ok(activate(Id::Bluetooth(BtId::Power))));
        assert_eq!(parse("tap pad-mouse"), Ok(activate(Id::PadMouse)));
        assert_eq!(parse("tap save"), Ok(activate(Id::Editor(EditorId::Save))));
        assert_eq!(parse("poke dock 1"), Err(ParseError::Unknown));
        assert_eq!(parse("tap dock x"), Err(ParseError::Number("x".into())));
        assert_eq!(parse("tap nothing"), Err(ParseError::Unknown));
    }

    #[test]
    fn the_keymaps_screens() {
        use clamshell_core::keymap::Button;
        let km = |id| activate(Id::Keymap(id));
        assert_eq!(parse("tap keymap 1"), Ok(activate(ListId::Keymaps.row(1))));
        assert_eq!(parse("long keymap 2"), Ok(vec![PointerEvent::LongPress(ListId::Keymaps.row(2)).into()]));
        assert_eq!(parse("tap keymap-edit 1"), Ok(activate(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit })));
        assert_eq!(parse("tap keymap-new"), Ok(km(KeymapId::New)));
        assert_eq!(parse("tap keymap-menu rename"), Ok(km(KeymapId::Rename)));
        assert_eq!(parse("tap map-from rstick"), Ok(km(KeymapId::Input(Input::Stick(Side::Right)))));
        assert_eq!(parse("tap map-from lstick-up"), Ok(km(KeymapId::Input(Input::StickDir(Side::Left, Dir::Up)))));
        assert_eq!(parse("tap map-to key"), Ok(km(KeymapId::Target(Target::Key))));
        assert_eq!(parse("tap map-to pad-b"), Ok(km(KeymapId::Target(Target::To(Output::Pad(PadOut::Button(Button::B)))))));
        assert_eq!(parse("tap map-to pad-l4"), Err(ParseError::Name("pad-l4".into())), "not on the game's pad");
        assert_eq!(parse("tap map-to mouse-right"), Ok(km(KeymapId::Target(Target::To(Output::Mouse(MouseButton::Right))))));
        assert_eq!(parse("tap map-to stick-left"), Ok(km(KeymapId::Target(Target::To(Output::Stick(Side::Left))))));
        assert_eq!(parse("tap gyro-hold lt"), Ok(km(KeymapId::Hold(Some(Input::Trigger(Side::Left))))));
        assert_eq!(parse("tap gyro-hold always"), Ok(km(KeymapId::Hold(None))));
        assert_eq!(parse("tap keymap-layer 1"), Ok(km(KeymapId::Layer(Layer(1)))));
        assert_eq!(parse("tap keymap-layer base"), Ok(km(KeymapId::Layer(Layer::BASE))));
        assert_eq!(parse("tap invert y"), Ok(km(KeymapId::Toggle(Param::InvertY))));
        assert_eq!(parse("tap map-to pick"), Ok(km(KeymapId::Target(Target::Pick))));
        assert_eq!(parse("tap keymap-act hold"), Ok(km(KeymapId::Actuation(Actuation::Hold))));
        assert_eq!(parse("tap map-to mouse"), Ok(km(KeymapId::Target(Target::MouseParts))));
        assert_eq!(parse("tap key ctrl"), Ok(activate(key(Key::Mod(Modifier::Ctrl)).unwrap())));
        assert_eq!(parse("tap key x"), Ok(activate(key(Key::Char('x')).unwrap())));
        assert_eq!(parse("listen a"), Ok(vec![Event::Listened(Input::Button(Button::A))]));
        assert_eq!(parse("listen home"), Err(ParseError::Name("home".into())));
    }

    #[test]
    fn every_row_is_in_the_help_and_none_is_shadowed() {
        let help = help();
        for f in COMMANDS.iter().map(|f| (f.words, f.help)).chain(TARGETS.iter().map(|f| (f.words, f.help))) {
            assert!(help.contains(f.0) && help.contains(f.1), "{f:?}");
        }
        // A row whose words an earlier row's pattern also fits would never be reached.
        let shadowed = |table: &[(&str, usize)]| {
            for (i, (later, _)) in table.iter().enumerate() {
                let words: Vec<&str> = later.split(' ').collect();
                for (earlier, _) in table.iter().take(i) {
                    assert!(fits(earlier, &words).is_none(), "{later:?} is taken by {earlier:?}");
                }
            }
        };
        shadowed(&COMMANDS.iter().map(|f| (f.words, 0)).collect::<Vec<_>>());
        shadowed(&TARGETS.iter().map(|f| (f.words, 0)).collect::<Vec<_>>());
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
        assert_eq!(parse("pad start"), Ok(vec![Event::Pad(PadInput::Start)]));
        assert_eq!(parse("pad z"), Err(ParseError::Name("z".into())));
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
        let speed = Control::Param(Param::Speed);
        assert_eq!(parse("slide speed 300 400").unwrap()[1], Slide::Start { control: speed, x: 300.0, width: 620.0 }.into());
    }

    #[test]
    fn levels() {
        assert_eq!(parse("level volume 40"), Ok(vec![LevelEvent::Set(Level::Volume, 40).into(), PointerEvent::Up(Id::Level(Level::Volume)).into()]));
        assert_eq!(parse("level backlight 1 65"), Ok(vec![LevelEvent::Set(Level::Backlight(1), 65).into(), PointerEvent::Up(Level::Backlight(1).id()).into()]));
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
