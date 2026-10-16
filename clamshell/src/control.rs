//! The control pipe ($XDG_RUNTIME_DIR/clamshell-control, or CLAMSHELL_CONTROL): `clamshell ctl` and
//! tests send commands through it, one per line, as the events the device's keys or a user's taps and
//! presses would make. The commands and the targets a tap or a long press can be on are the rows of
//! [`COMMANDS`] and [`TARGETS`]: each its words (placeholders in capitals), what it does, and what it
//! makes; `clamshell ctl help` lists them.

use std::fmt;
use std::io::BufRead;
use std::path::PathBuf;

use clamshell_core::Scope;
use clamshell_core::hardware::DeviceKey;
use clamshell_core::keyboard::{self, Key, Modifier, Named};
use clamshell_core::keymap::{Actuation, Dir, Input, Layer, MouseButton, PadOut, Param, Side};
use clamshell_core::levels::Level;
use clamshell_core::settings::Setting;
use clamshell_core::slider::Control;
use clamshell_core::{
    BtId, DockId, EditorId, Event, Id, KeymapId, LevelEvent, ListId, PadInput, PointerEvent, RowPart, SettingsId, ShelfEvent, ShelfId, Slide, Tab, Target,
    WifiId,
};
use clamshell_system::{Sink, worker};

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
        "speed" => Ok(Control::Param(Param::Speed)),
        "threshold" => Ok(Control::Param(Param::Threshold)),
        "deadzone" => Ok(Control::Param(Param::Deadzone)),
        "full" => Ok(Control::Param(Param::Full)),
        "curve" => Ok(Control::Param(Param::Curve)),
        "turbo" => Ok(Control::Param(Param::Turbo)),
        "holdtime" => Ok(Control::Param(Param::HoldTime)),
        "accel" => Ok(Control::Param(Param::Accel)),
        "accelat" => Ok(Control::Param(Param::AccelAt)),
        "tighten" => Ok(Control::Param(Param::Tighten)),
        "stickmin" => Ok(Control::Param(Param::StickMin)),
        other => Err(ParseError::Name(other.to_owned())),
    }
}

/// A command or a target, as a row of the tables: its words (literal, or a placeholder in capitals that
/// any word fills: `N`, `INPUT`, ...), what it does (for `clamshell ctl help`), and what it makes from the
/// placeholders' words.
pub struct Form<T> {
    pub words: &'static str,
    pub help: &'static str,
    make: fn(&[&str]) -> Result<T, ParseError>,
}

const fn form<T>(words: &'static str, help: &'static str, make: fn(&[&str]) -> Result<T, ParseError>) -> Form<T> {
    Form { words, help, make }
}

/// The placeholders' words, if `words` fit `pattern` (the same count, each literal the same).
fn fits<'a>(pattern: &str, words: &[&'a str]) -> Option<Vec<&'a str>> {
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

/// The first row `words` fit, made; not a command if none.
fn by_table<T>(table: &[Form<T>], words: &[&str]) -> Result<T, ParseError> {
    for f in table {
        if let Some(args) = fits(f.words, words) {
            return (f.make)(&args);
        }
    }
    Err(ParseError::Unknown)
}

/// The `i`th placeholder's word.
fn arg<'a>(args: &[&'a str], i: usize) -> &'a str {
    args.get(i).copied().unwrap_or("")
}

fn name_err(word: &str) -> ParseError {
    ParseError::Name(word.to_owned())
}

fn device(key: DeviceKey) -> Result<Vec<Event>, ParseError> {
    Ok(vec![Event::Device(key)])
}

/// The level row set to a value, and the finger lifted.
fn level(level: Level, value: &str) -> Result<Vec<Event>, ParseError> {
    Ok(vec![LevelEvent::Set(level, number(value)?).into(), PointerEvent::Up(level.id()).into()])
}

/// The commands (a tap's and a long press's targets are [`TARGETS`]; `type TEXT` types the rest of the
/// line as it is).
pub const COMMANDS: &[Form<Vec<Event>>] = &[
    form("volume up", "the volume key", |_| device(DeviceKey::VolumeUp)),
    form("volume down", "the volume key", |_| device(DeviceKey::VolumeDown)),
    form("volume mute", "mute", |_| device(DeviceKey::Mute)),
    form("brightness up", "the brightness key", |_| device(DeviceKey::BrightnessUp)),
    form("brightness down", "the brightness key", |_| device(DeviceKey::BrightnessDown)),
    form("screenshot", "the screenshot key", |_| device(DeviceKey::Screenshot)),
    form("bottom next", "Home+Select: the bottom screen shows the next of ClamShell's view and the windows moved there", |_| device(DeviceKey::BottomNext)),
    form("bottom power", "Home+Start: the bottom screen off, or on", |_| device(DeviceKey::BottomPower)),
    form("window down", "Home+Down: the window in front to the bottom screen (remembered for its program)", |_| device(DeviceKey::WindowDown)),
    form("window up", "Home+Up: the bottom screen's window back up (forgotten)", |_| device(DeviceKey::WindowUp)),
    form("home", "the Home button", |_| Ok(vec![Event::Home])),
    form("list N", "shelf's selection moved by N", |a| Ok(vec![ShelfEvent::ListStep(number(arg(a, 0))?).into()])),
    form("tab NAME", "a tab opened (dock, wifi, keyboard, ...)", |a| {
        let name = arg(a, 0);
        Ok(vec![PointerEvent::Activate(Id::Tab(Tab::from_name(name).ok_or_else(|| name_err(name))?)).into()])
    }),
    form("pad BUTTON", "the pad on ClamShell's screens: up, down, left, right, a (pressed and let go), b, y, lb, rb, switch, back", |a| pad(arg(a, 0))),
    form("listen INPUT", "an input as the pad sends it while it listens (a keymap's edit screen)", |a| Ok(vec![Event::Listened(input_named(arg(a, 0))?)])),
    form("lift SLIDER", "a held slider let go", |a| Ok(vec![Slide::End.into(), PointerEvent::Up(Id::Control(control(arg(a, 0))?)).into()])),
    form(
        "slide SLIDER FROM TO",
        "a finger down on a slider at FROM (on a 620-wide layout), moved to TO, lifted: volume, brightness, or the selected input's speed, threshold, deadzone, full, curve, turbo, holdtime, accel, accelat, tighten, stickmin",
        |a| Ok(slide(control(arg(a, 0))?, number(arg(a, 1))?, number(arg(a, 2))?, false)),
    ),
    form("slide SLIDER FROM TO hold", "the same, held", |a| Ok(slide(control(arg(a, 0))?, number(arg(a, 1))?, number(arg(a, 2))?, true))),
    form("level volume V", "the volume tab's level row set to V", |a| level(Level::Volume, arg(a, 0))),
    form("level brightness V", "the brightness tab's level row set to V", |a| level(Level::Brightness, arg(a, 0))),
    form("level backlight N V", "a screen's backlight row set to V", |a| level(Level::Backlight(number(arg(a, 0))?), arg(a, 1))),
    form("level stream N V", "an app's volume row set to V", |a| level(Level::Stream(number(arg(a, 0))?), arg(a, 1))),
];

fn km(id: KeymapId) -> Result<Id, ParseError> {
    Ok(Id::Keymap(id))
}

/// What a tap or a long press (`tap TARGET`, `long TARGET`) can be on.
pub const TARGETS: &[Form<Id>] = &[
    form("app N", "the dock's app N (by position in launcher.json)", |a| Ok(Id::Dock(DockId::App(number(arg(a, 0))?)))),
    form("plus N", "another instance of app N", |a| Ok(Id::Dock(DockId::NewInstance(number(arg(a, 0))?)))),
    form("game", "the dock's first game tile", |_| Ok(Id::Dock(DockId::GameTile(0)))),
    form("game N", "the dock's game tile N", |a| Ok(Id::Dock(DockId::GameTile(number(arg(a, 0))?)))),
    form("start-game", "the dock's start tile", |_| Ok(Id::Dock(DockId::StartGame))),
    form("launch", "shelf's launch", |_| Ok(Id::Shelf(ShelfId::Launch))),
    form("keep", "shelf's keep (another game running)", |_| Ok(Id::Shelf(ShelfId::StartKeep))),
    form("close", "shelf's close (another game running)", |_| Ok(Id::Shelf(ShelfId::StartClose))),
    form("network N", "Wi-Fi's network row N", |a| Ok(ListId::Wifi.row(number(arg(a, 0))?))),
    form("wifi-power", "Wi-Fi's switch", |_| Ok(Id::Wifi(WifiId::Power))),
    form("device N", "Bluetooth's device row N", |a| Ok(ListId::Bluetooth.row(number(arg(a, 0))?))),
    form("bluetooth-power", "Bluetooth's switch", |_| Ok(Id::Bluetooth(BtId::Power))),
    form("pad-mouse", "the bar's pad mouse switch", |_| Ok(Id::PadMouse)),
    form("scope game", "the settings' game tab", |_| Ok(Id::Settings(SettingsId::Scope(Scope::Game)))),
    form("scope all", "the settings' all games tab", |_| Ok(Id::Settings(SettingsId::Scope(Scope::Global)))),
    form("setting KEY", "a settings tile by its key in game-settings.txt (pad, fps, keymap, ...)", |a| {
        let key = arg(a, 0);
        Ok(Id::Settings(SettingsId::Tile(Setting::from_name(key).ok_or_else(|| name_err(key))?)))
    }),
    form("save", "the editor's save", |_| Ok(Id::Editor(EditorId::Save))),
    form("key NAME", "an on-screen key: a character, space, ctrl, alt, shift, fn, enter, esc, backspace, tab", |a| key(key_named(arg(a, 0))?)),
    // The keymaps' screens.
    form("keymap N", "the keymap picker's row N (0 Default, then the keymaps; a long press: its menu)", |a| Ok(ListId::Keymaps.row(number(arg(a, 0))?))),
    form("keymap-edit N", "row N's Edit", |a| Ok(Id::Row { list: ListId::Keymaps, i: number(arg(a, 0))?, part: RowPart::Edit })),
    form("keymap-new", "the picker's New", |_| km(KeymapId::New)),
    form("keymap-back", "back one keymap screen (picking: Cancel)", |_| km(KeymapId::Back)),
    form("keymap-menu ACTION", "a keymap's menu: edit, rename, duplicate, delete", |a| {
        km(match arg(a, 0) {
            "edit" => KeymapId::Edit,
            "rename" => KeymapId::Rename,
            "duplicate" => KeymapId::Duplicate,
            "delete" => KeymapId::Delete,
            other => return Err(name_err(other)),
        })
    }),
    form("keymap-delete", "deleting confirmed", |_| km(KeymapId::ConfirmDelete)),
    form("keymap-undo", "the edit screen's Undo", |_| km(KeymapId::Undo)),
    form("map-from INPUT", "an input selected (as in keymaps.txt: a, lb, up, lstick, rstick-up, lt, gyro; a long press sets it back)", |a| {
        km(KeymapId::Input(input_named(arg(a, 0))?))
    }),
    form("keymap-act ACT", "the panel's tab: press, hold, double, full", |a| {
        let act = arg(a, 0);
        km(KeymapId::Actuation(
            Actuation::ALL
                .into_iter()
                .find(|x| x.label().eq_ignore_ascii_case(act) || (act == "full" && *x == Actuation::FullPull))
                .ok_or_else(|| name_err(act))?,
        ))
    }),
    form(
        "map-to TARGET",
        "the panel's ways (pick, key, keys, mouse, layers, pointer, default, nothing) or an output (pad-INPUT, mouse-left|right|middle|back|forward, wheel-up|down|left|right, stick-left|right, dpad, trigger-left|right)",
        |a| km(KeymapId::Target(target_named(arg(a, 0))?)),
    ),
    form("keymap-layers", "the layer button: the layers' list", |_| km(KeymapId::LayerPicker)),
    form("keymap-layer base", "Base in the layers' list", |_| km(KeymapId::Layer(Layer::BASE))),
    form("keymap-layer N", "layer N in the layers' list (a long press: its menu)", |a| km(KeymapId::Layer(Layer(number(arg(a, 0))?)))),
    form("layer-edit N", "layer N's Edit: its menu", |a| km(KeymapId::EditLayer(Layer(number(arg(a, 0))?)))),
    form("layer-once", "the layer menu's Once", |_| km(KeymapId::LayerOnce)),
    form("layer-rename", "the layer menu's Rename", |_| km(KeymapId::RenameLayer)),
    form("layer-delete", "the layer menu's Delete", |_| km(KeymapId::DeleteLayer)),
    form("keymap-clear", "the layer menu's Clear", |_| km(KeymapId::Clear)),
    form("keymap-clear-confirm", "clearing confirmed", |_| km(KeymapId::ConfirmClear)),
    form("new-layer", "New layer, in either layers' list", |_| km(KeymapId::NewLayer)),
    form("to-layer N", "layer N chosen as the target", |a| km(KeymapId::ToLayer(Layer(number(arg(a, 0))?)))),
    form("toggle", "the binding's Toggle", |_| km(KeymapId::Toggle(Param::Toggle))),
    form("invert x", "Invert X", |_| km(KeymapId::Toggle(Param::InvertX))),
    form("invert y", "Invert Y", |_| km(KeymapId::Toggle(Param::InvertY))),
    form("twist", "the gyro's Turn by tilting", |_| km(KeymapId::Toggle(Param::Twist))),
    form("ratchet", "the gyro's Off while held", |_| km(KeymapId::Toggle(Param::Ratchet))),
    form("calibrate", "the gyro's Calibrate", |_| km(KeymapId::Calibrate)),
    form("keymap-active", "the gyro's Active row: what holds it on", |_| km(KeymapId::Holds)),
    form("gyro-hold always", "the gyro always on", |_| km(KeymapId::Hold(None))),
    form("gyro-hold INPUT", "the gyro on while INPUT is held", |a| km(KeymapId::Hold(Some(input_named(arg(a, 0))?)))),
    form("key-done", "the keyboard's Done (the modifiers alone)", |_| km(KeymapId::Done)),
];

/// The commands and targets, a line each.
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

/// A line as the events it stands for.
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
        "back" => input(PadInput::Back),
        other => return Err(ParseError::Name(other.to_owned())),
    })
}

fn input_named(word: &str) -> Result<Input, ParseError> {
    Input::parse(word).ok_or_else(|| name_err(word))
}

/// A target of the "map to" screen by name.
fn target_named(word: &str) -> Result<Target, ParseError> {
    let name = || ParseError::Name(word.to_owned());
    let side = |w: &str| match w {
        "left" => Ok(Side::Left),
        "right" => Ok(Side::Right),
        _ => Err(name()),
    };
    Ok(match word.split_once('-') {
        None => match word {
            "default" => Target::Default,
            "nothing" => Target::Nothing,
            "pick" => Target::Pick,
            "mouse" => Target::MouseParts,
            "layers" => Target::Layers,
            "key" => Target::Key,
            "keys" => Target::Keys,
            "pointer" => Target::Pointer,
            "dpad" => Target::Dpad,
            _ => return Err(name()),
        },
        Some(("pad", input)) => Target::Pad(PadOut::of(input_named(input)?).ok_or_else(name)?),
        Some(("mouse", m)) => Target::Mouse(MouseButton::from_name(m).ok_or_else(name)?),
        Some(("wheel", d)) => Target::Wheel(Dir::from_name(d).ok_or_else(name)?),
        Some(("stick", s)) => Target::Stick(side(s)?),
        Some(("trigger", s)) => Target::Trigger(side(s)?),
        _ => return Err(name()),
    })
}

/// An on-screen keyboard key by name.
fn key_named(word: &str) -> Result<Key, ParseError> {
    let mut chars = word.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(Key::Char(c));
    }
    Ok(match word {
        "space" => Key::Char(' '),
        "ctrl" => Key::Mod(Modifier::Ctrl),
        "alt" => Key::Mod(Modifier::Alt),
        "shift" => Key::Mod(Modifier::Shift),
        "fn" => Key::Mod(Modifier::Fn),
        "enter" => Key::Named(Named::Enter),
        "esc" => Key::Named(Named::Esc),
        "backspace" => Key::Named(Named::Backspace),
        "tab" => Key::Named(Named::Tab),
        other => return Err(ParseError::Name(other.to_owned())),
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
/// Sends `line` to the running ClamShell's control pipe (`clamshell ctl`). Without ClamShell reading it,
/// an error rather than waiting.
pub fn send(path: Option<&PathBuf>, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = path.ok_or_else(|| std::io::Error::other("no control pipe (no XDG_RUNTIME_DIR)"))?;
    // Non-blocking: opening a pipe nobody reads fails at once (ENXIO) instead of waiting for a reader.
    let mut pipe = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e} (is ClamShell running?)", path.display())))?;
    writeln!(pipe, "{line}")
}

/// Reads commands from the pipe at `path` (made if it isn't there) and sends their events.
pub fn spawn(path: PathBuf, sink: impl Sink) {
    if !path.exists()
        && let Err(e) = rustix::fs::mknodat(rustix::fs::CWD, &path, rustix::fs::FileType::Fifo, rustix::fs::Mode::from_raw_mode(0o600), 0)
    {
        log::warn!("control: {}: {e}", path.display());
    }
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
        assert_eq!(parse("tap network 2"), Ok(activate(ListId::Wifi.row(2))));
        assert_eq!(parse("long network 0"), Ok(vec![PointerEvent::LongPress(ListId::Wifi.row(0)).into()]));
        assert_eq!(parse("tap wifi-power"), Ok(activate(Id::Wifi(WifiId::Power))));
        assert_eq!(parse("tap device 3"), Ok(activate(ListId::Bluetooth.row(3))));
        assert_eq!(parse("tap bluetooth-power"), Ok(activate(Id::Bluetooth(BtId::Power))));
        assert_eq!(parse("tap pad-mouse"), Ok(activate(Id::PadMouse)));
        assert_eq!(parse("tap save"), Ok(activate(Id::Editor(EditorId::Save))));
        assert_eq!(parse("poke app 1"), Err(ParseError::Unknown));
        assert_eq!(parse("tap app x"), Err(ParseError::Number("x".into())));
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
        assert_eq!(parse("tap map-to pad-b"), Ok(km(KeymapId::Target(Target::Pad(PadOut::Button(Button::B))))));
        assert_eq!(parse("tap map-to pad-l4"), Err(ParseError::Name("pad-l4".into())), "not on the game's pad");
        assert_eq!(parse("tap map-to mouse-right"), Ok(km(KeymapId::Target(Target::Mouse(MouseButton::Right)))));
        assert_eq!(parse("tap map-to stick-left"), Ok(km(KeymapId::Target(Target::Stick(Side::Left)))));
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
        // (A row whose words an earlier row's pattern takes would never be reached.)
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
