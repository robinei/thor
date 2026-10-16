//! The control pipe ($XDG_RUNTIME_DIR/clamshell-control, or CLAMSHELL_CONTROL): `clamshell ctl` and
//! tests send commands through it, one per line, as the events the device's keys or a user's taps and
//! presses would make. The grammar:
//!
//! - `volume up|down|mute`, `brightness up|down`, `screenshot`: as the hardware keys
//! - `tap TARGET`, `long TARGET` (a tap, a long press) on: `app N` (the dock's apps, by position in
//!   launcher.json), `plus N` (another instance of app N), `game` / `game N` (the dock's game tiles),
//!   `start-game`, `launch`, `keep`, `close` (shelf), `network N`, `wifi-power`, `device N`,
//!   `bluetooth-power`, `pad-mouse`, `save` (the editor), `setting KEY` (a settings tile, by its key in
//!   game-settings.txt: `pad`, `fps`, `keymap`, ...), `scope game|all` (the settings' game or all games);
//!   the keymaps' screens: `keymap N` (the picker's row N: 0 Default, then the keymaps; a long press
//!   opens its menu), `keymap-edit N`, `keymap-new`, `keymap-back`, `keymap-menu
//!   edit|rename|duplicate|delete`, `keymap-delete` (confirmed), `map-from INPUT` (as in keymaps.txt:
//!   `a`, `lb`, `up`, `lstick`, `rstick-up`, `lt`, `gyro`: selects it, again reopens the way it is
//!   mapped; while a control is picked, the one picked; a long press sets it back),
//!   `keymap-act press|hold|double|full` (the panel's tab), `keymap-layers` (the layer button: the
//!   list), `keymap-layer base|N` (in the list; a long press: its menu, `layer-rename`, `layer-once`, `layer-delete`,
//!   `keymap-clear`), `new-layer` (in either list), `to-layer N` (the layers' list as a target), `keymap-clear`,
//!   `keymap-clear-confirm`, `toggle` (the binding's toggle), `twist`, `ratchet` and `calibrate` (the
//!   gyro's), `keymap-active` (the gyro's hold), `invert x|y`, `map-to TARGET` (the panel's ways: `pick`,
//!   `key`, `keys`, `mouse`, `pointer`, `default`, `nothing`; or an output: `pad-INPUT`,
//!   `mouse-left|right|middle|back|forward`, `wheel-up|down|left|right`, `stick-left|right`, `dpad`,
//!   `trigger-left|right`), `gyro-hold INPUT|always`, `key-done`; `key NAME` (an on-screen keyboard key: a
//!   character, `space`, `ctrl`, `alt`, `shift`, `fn`, `enter`, `esc`, `backspace`, `tab`)
//! - `tab NAME`: open a tab (dock, wifi, keyboard, ...)
//! - `home`; `list N`: move shelf's selection by N
//! - `type TEXT`: type on the on-screen keyboard
//! - `slide volume|brightness FROM TO [hold]`: a finger down at FROM on a 620-wide layout, moved to TO,
//!   lifted unless held; `lift volume|brightness` lifts it; the same for the selected input's parameter
//!   on the keymap edit screen: `slide speed|threshold|deadzone|full|curve|turbo|holdtime|accel|accelat|
//!   tighten|stickmin FROM TO [hold]`
//! - `level volume|brightness V`, `level backlight|stream N V`: a level row in the volume or brightness
//!   tab set to V
//! - `pad up|down|left|right|a|b|y|lb|rb|switch|back`: the pad on ClamShell's screens (`a` presses and
//!   releases)
//! - `listen INPUT`: an input as the pad sends it while it listens (a keymap's edit screen)

use std::fmt;
use std::io::BufRead;
use std::path::PathBuf;

use clamshell_core::hardware::DeviceKey;
use clamshell_core::keyboard::{self, Key, Modifier, Named};
use clamshell_core::keymap::{Actuation, Dir, Input, Layer, MouseButton, PadOut, Param, Side};
use clamshell_core::levels::Level;
use clamshell_core::slider::Control;
use clamshell_core::{
    BtId, DockId, ListId, EditorId, KeymapId, RowPart, Target, Event, Id, LevelEvent, PadInput, PointerEvent, SettingsId, ShelfEvent, ShelfId, Slide, Tab,
    WifiId,
};
use clamshell_core::Scope;
use clamshell_core::settings::Setting;
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

/// A line as the events it stands for.
pub fn parse(line: &str) -> Result<Vec<Event>, ParseError> {
    // The text to type is the rest of the line, as it is.
    if let Some(text) = line.strip_prefix("type ") {
        return type_text(text);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    Ok(match words.as_slice() {
        ["volume", "up"] => vec![Event::Device(DeviceKey::VolumeUp)],
        ["volume", "down"] => vec![Event::Device(DeviceKey::VolumeDown)],
        ["volume", "mute"] => vec![Event::Device(DeviceKey::Mute)],
        ["brightness", "up"] => vec![Event::Device(DeviceKey::BrightnessUp)],
        ["brightness", "down"] => vec![Event::Device(DeviceKey::BrightnessDown)],
        ["screenshot"] => vec![Event::Device(DeviceKey::Screenshot)],
        ["home"] => vec![Event::Home],
        ["list", step] => vec![ShelfEvent::ListStep(number(step)?).into()],
        ["tab", name] => vec![PointerEvent::Activate(Id::Tab(Tab::from_name(name).ok_or_else(|| ParseError::Name((*name).to_owned()))?)).into()],
        ["pad", button] => pad(button)?,
        ["listen", input] => vec![Event::Listened(Input::parse(input).ok_or_else(|| ParseError::Name((*input).to_owned()))?)],
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
            vec![LevelEvent::Set(level, number(value)?).into(), PointerEvent::Up(level.id()).into()]
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
        "back" => input(PadInput::Back),
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
        ["network", n] => ListId::Wifi.row(number(n)?),
        ["wifi-power"] => Id::Wifi(WifiId::Power),
        ["device", n] => ListId::Bluetooth.row(number(n)?),
        ["bluetooth-power"] => Id::Bluetooth(BtId::Power),
        ["pad-mouse"] => Id::PadMouse,
        ["scope", "game"] => Id::Settings(SettingsId::Scope(Scope::Game)),
        ["scope", "all"] => Id::Settings(SettingsId::Scope(Scope::Global)),
        ["setting", key] => Id::Settings(SettingsId::Tile(
            Setting::ALL.into_iter().find(|s| s.key() == *key).ok_or_else(|| ParseError::Name((*key).to_owned()))?,
        )),
        ["save"] => Id::Editor(EditorId::Save),
        ["keymap", n] => ListId::Keymaps.row(number(n)?),
        ["keymap-edit", n] => Id::Row { list: ListId::Keymaps, i: number(n)?, part: RowPart::Edit },
        ["keymap-new"] => Id::Keymap(KeymapId::New),
        ["keymap-back"] => Id::Keymap(KeymapId::Back),
        ["keymap-delete"] => Id::Keymap(KeymapId::ConfirmDelete),
        ["keymap-menu", action] => Id::Keymap(match *action {
            "edit" => KeymapId::Edit,
            "rename" => KeymapId::Rename,
            "duplicate" => KeymapId::Duplicate,
            "delete" => KeymapId::Delete,
            other => return Err(ParseError::Name(other.to_owned())),
        }),
        ["map-from", input] => Id::Keymap(KeymapId::Input(input_named(input)?)),
        ["map-to", target] => Id::Keymap(KeymapId::Target(target_named(target)?)),
        ["keymap-act", act] => Id::Keymap(KeymapId::Actuation(
            Actuation::ALL
                .into_iter()
                .find(|a| a.label().eq_ignore_ascii_case(act) || (*act == "full" && *a == Actuation::FullPull))
                .ok_or_else(|| ParseError::Name((*act).to_owned()))?,
        )),
        ["keymap-layer", "base"] => Id::Keymap(KeymapId::Layer(Layer::BASE)),
        ["keymap-layer", n] => Id::Keymap(KeymapId::Layer(Layer(number(n)?))),
        ["keymap-layers"] => Id::Keymap(KeymapId::LayerPicker),
        ["layer-rename"] => Id::Keymap(KeymapId::RenameLayer),
        ["layer-delete"] => Id::Keymap(KeymapId::DeleteLayer),
        ["layer-once"] => Id::Keymap(KeymapId::LayerOnce),
        ["to-layer", n] => Id::Keymap(KeymapId::ToLayer(Layer(number(n)?))),
        ["new-layer"] => Id::Keymap(KeymapId::NewLayer),
        ["keymap-clear"] => Id::Keymap(KeymapId::Clear),
        ["keymap-clear-confirm"] => Id::Keymap(KeymapId::ConfirmClear),
        ["keymap-active"] => Id::Keymap(KeymapId::Holds),
        ["invert", "x"] => Id::Keymap(KeymapId::Toggle(Param::InvertX)),
        ["invert", "y"] => Id::Keymap(KeymapId::Toggle(Param::InvertY)),
        ["toggle"] => Id::Keymap(KeymapId::Toggle(Param::Toggle)),
        ["twist"] => Id::Keymap(KeymapId::Toggle(Param::Twist)),
        ["ratchet"] => Id::Keymap(KeymapId::Toggle(Param::Ratchet)),
        ["calibrate"] => Id::Keymap(KeymapId::Calibrate),
        ["gyro-hold", "always"] => Id::Keymap(KeymapId::Hold(None)),
        ["gyro-hold", input] => Id::Keymap(KeymapId::Hold(Some(input_named(input)?))),
        ["key-done"] => Id::Keymap(KeymapId::Done),
        ["key", name] => key(key_named(name)?)?,
        ["keep"] => Id::Shelf(ShelfId::StartKeep),
        ["close"] => Id::Shelf(ShelfId::StartClose),
        ["launch"] => Id::Shelf(ShelfId::Launch),
        _ => return Err(ParseError::Unknown),
    })
}

fn input_named(word: &str) -> Result<Input, ParseError> {
    Input::parse(word).ok_or_else(|| ParseError::Name(word.to_owned()))
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
        Some(("mouse", m)) => Target::Mouse(MouseButton::ALL.into_iter().find(|b| b.name() == m).ok_or_else(name)?),
        Some(("wheel", d)) => Target::Wheel(Dir::ALL.into_iter().find(|x| x.name() == d).ok_or_else(name)?),
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
