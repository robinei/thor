//! The tables of commands and targets, a row each with its words, what it does and what it makes. Also
//! the names of the pad buttons, keys, and keymap inputs and outputs that the commands take.

use clamshell_state::Control;
use clamshell_state::bottom::keyboard::{self, Key, Modifier, Named};
use clamshell_state::device::buttons::DeviceKey;
use clamshell_state::keymap::{Actuation, Dir, Input, Layer, MouseButton, Output, PadOut, Param, Side};
use clamshell_state::levels::Level;
use clamshell_state::settings::Setting;
use clamshell_state::{
    BtId, DockId, EditorId, Event, Id, KeymapId, LevelEvent, ListId, PadInput, PointerEvent, RowPart, SettingsId, ShelfEvent, ShelfId, Slide, Tab, Target,
    WifiId,
};
use clamshell_state::{ClockField, Scope, TimeId};

use super::{Form, ParseError, arg, control, form, name_err, number};

/// The layout width that a slide's positions are measured on: the second screen at half size.
pub(super) const SLIDE_WIDTH: f32 = 620.0;

/// The number of steps a slide moves in.
pub(super) const SLIDE_STEPS: u16 = 10;

pub(super) fn device(key: DeviceKey) -> Result<Vec<Event>, ParseError> {
    Ok(vec![Event::Device(key)])
}

/// Sets the level row to a value and lifts the finger.
pub(super) fn level(level: Level, value: &str) -> Result<Vec<Event>, ParseError> {
    Ok(vec![LevelEvent::Set(level, number(value)?).into(), PointerEvent::Up(level.id()).into()])
}

/// The commands. The targets of a tap or a long press are in [`TARGETS`], and `type TEXT` types the rest
/// of the line as it is.
pub const COMMANDS: &[Form<Vec<Event>>] = &[
    form("volume up", "the volume key", |_| device(DeviceKey::VolumeUp)),
    form("volume down", "the volume key", |_| device(DeviceKey::VolumeDown)),
    form("volume mute", "mute", |_| device(DeviceKey::Mute)),
    form("brightness up", "the brightness key", |_| device(DeviceKey::BrightnessUp)),
    form("brightness down", "the brightness key", |_| device(DeviceKey::BrightnessDown)),
    form("screenshot", "the screenshot key", |_| device(DeviceKey::Screenshot)),
    form("bottom view", "Home + right stick press: the bottom screen shows ClamShell's view or the windows moved there", |_| device(DeviceKey::BottomView)),
    form("bottom power", "Home+Start: the bottom screen off, or on", |_| device(DeviceKey::BottomPower)),
    form("window events FILE", "dump the in-process compositor's last 256 window inputs", |a| Ok(vec![Event::DumpWindowEvents(arg(a, 0).into())])),
    form("window down", "Home + right stick down: the window in front to the bottom screen (remembered for its program)", |_| device(DeviceKey::WindowDown)),
    form("window up", "Home + right stick up: the bottom screen's window back up (forgotten)", |_| device(DeviceKey::WindowUp)),
    form("window previous", "the program's previous window on top", |_| device(DeviceKey::WindowPrevious)),
    form("window next", "the program's next window on top", |_| device(DeviceKey::WindowNext)),
    form("window close", "Home+B: the window in front closes", |_| device(DeviceKey::CloseWindow)),
    form("window quit", "Home+B held: the program in front quits (as its dock menu's Quit)", |_| device(DeviceKey::Quit)),
    form("home", "the Home button", |_| Ok(vec![Event::Home])),
    form("list N", "shelf's selection moved by N", |a| Ok(vec![ShelfEvent::ListStep(number(arg(a, 0))?).into()])),
    form("pin", "shelf's selected entry pinned to the dock, or unpinned (the pad's X)", |_| Ok(vec![ShelfEvent::TogglePin.into()])),
    form("favourite", "shelf's selected entry a favourite, or not (the pad's Y)", |_| Ok(vec![ShelfEvent::ToggleFavourite.into()])),
    form("tab NAME", "a tab opened (dock, wifi, keyboard, ...)", |a| {
        let name = arg(a, 0);
        Ok(vec![PointerEvent::Activate(Id::Tab(Tab::from_name(name).ok_or_else(|| name_err(name))?)).into()])
    }),
    form("pad BUTTON", "the pad on ClamShell's screens: up, down, left, right, a (pressed and let go), b, x, y, start, lb, rb, switch, back", |a| {
        pad(arg(a, 0))
    }),
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

pub(super) fn km(id: KeymapId) -> Result<Id, ParseError> {
    Ok(Id::Keymap(id))
}

/// What a tap or a long press (`tap TARGET`, `long TARGET`) can be on.
pub const TARGETS: &[Form<Id>] = &[
    form("dock N", "the dock's entry N (by position)", |a| Ok(Id::Dock(DockId::Entry(number(arg(a, 0))?)))),
    form("dock-tray N", "the tray mark of the dock's entry N: its program's menu", |a| Ok(Id::Dock(DockId::Tray(number(arg(a, 0))?)))),
    form("tray-entry ID", "an entry of a program's tray menu, by its id there (clamshell --tray lists them)", |a| {
        Ok(Id::Dock(DockId::TrayEntry(number(arg(a, 0))?)))
    }),
    form(
        "dock-menu ACTION",
        "a dock entry's menu (after a long press): open-another, close-window, menu, pin, autostart, move, suspend (or resume), quit, force-kill, back",
        |a| {
            Ok(Id::Dock(match arg(a, 0) {
                "open-another" => DockId::OpenAnother,
                "menu" => DockId::TrayMenu,
                "autostart" => DockId::Autostart,
                "suspend" | "resume" => DockId::Suspend,
                "close-window" => DockId::CloseWindow,
                "pin" => DockId::Pin,
                "move" => DockId::Move,
                "quit" => DockId::Quit,
                "force-kill" => DockId::ForceKill,
                "back" => DockId::Back,
                other => return Err(name_err(other)),
            }))
        },
    ),
    form("spender N", "the battery tab's row N of what is using the machine: its menu", |a| {
        Ok(Id::Battery(clamshell_state::BatteryId::Spender(number(arg(a, 0))?)))
    }),
    form("spender-menu ACTION", "a battery tab row's menu: quit, force-kill, back", |a| {
        use clamshell_state::BatteryId;
        Ok(Id::Battery(match arg(a, 0) {
            "quit" => BatteryId::Quit,
            "force-kill" => BatteryId::ForceKill,
            "back" => BatteryId::Back,
            other => return Err(name_err(other)),
        }))
    }),
    form("dock-step DIR", "moving a dock entry: one place (left, right) or row (up, down) along", |a| {
        Ok(Id::Dock(DockId::Step(match arg(a, 0) {
            "left" => clamshell_state::keymap::Dir::Left,
            "right" => clamshell_state::keymap::Dir::Right,
            "up" => clamshell_state::keymap::Dir::Up,
            "down" => clamshell_state::keymap::Dir::Down,
            other => return Err(name_err(other)),
        })))
    }),
    form("dock-done", "moving a dock entry: done", |_| Ok(Id::Dock(DockId::Done))),
    form("launch", "shelf's launch", |_| Ok(Id::Shelf(ShelfId::Launch))),
    form("enqueue", "shelf's Enqueue (added to what its runner plays, else played)", |_| Ok(Id::Shelf(ShelfId::Enqueue))),
    form("up", "out of shelf's open group", |_| Ok(Id::Shelf(ShelfId::Up))),
    form("collection N", "shelf's collection tab N", |a| Ok(Id::Shelf(ShelfId::Collection(number(arg(a, 0))?)))),
    form(
        "file ACTION",
        "a browsed entry's file action: copy, cut, rename, trash, restore, delete, empty-trash, eject, select, paste, new-folder, show-hidden, extract, open-in",
        |a| {
            use clamshell_state::shelf::files::FileAction;
            Ok(Id::Shelf(ShelfId::File(match arg(a, 0) {
                "copy" => FileAction::Copy,
                "cut" => FileAction::Cut,
                "rename" => FileAction::Rename,
                "trash" => FileAction::Trash,
                "restore" => FileAction::Restore,
                "delete" => FileAction::Delete,
                "empty-trash" => FileAction::EmptyTrash,
                "eject" => FileAction::Eject,
                "paste" => FileAction::Paste,
                "new-folder" => FileAction::NewFolder,
                "select" => FileAction::Select,
                "show-hidden" => FileAction::ShowHidden,
                "extract" => FileAction::Extract,
                "open-in" => FileAction::OpenIn,
                other => return Err(name_err(other)),
            })))
        },
    ),
    form("network N", "Wi-Fi's network row N", |a| Ok(ListId::Wifi.row(number(arg(a, 0))?))),
    form("wifi-power", "Wi-Fi's switch", |_| Ok(Id::Wifi(WifiId::Power))),
    form("time-zone", "the time tab's time zone row (the regions)", |_| Ok(Id::Time(TimeId::Zone))),
    form("time-back", "the time tab's way back (a region's zones to the regions, those to its main page)", |_| Ok(Id::Time(TimeId::Back))),
    form("time-ntp", "the time tab's Set automatically switch", |_| Ok(Id::Time(TimeId::Ntp))),
    form("time-12h", "the time tab's 24-hour clock switch", |_| Ok(Id::Time(TimeId::TwelveHour))),
    form("time-step FIELD DIR", "the time tab's arrow of a field (day, month, year, hour, minute), up or down", |a| {
        let field = match arg(a, 0) {
            "day" => ClockField::Day,
            "month" => ClockField::Month,
            "year" => ClockField::Year,
            "hour" => ClockField::Hour,
            "minute" => ClockField::Minute,
            other => return Err(name_err(other)),
        };
        let up = match arg(a, 1) {
            "up" => true,
            "down" => false,
            other => return Err(name_err(other)),
        };
        Ok(Id::Time(TimeId::Step(field, up)))
    }),
    form("device N", "Bluetooth's device row N", |a| Ok(ListId::Bluetooth.row(number(arg(a, 0))?))),
    form("bluetooth-power", "Bluetooth's switch", |_| Ok(Id::Bluetooth(BtId::Power))),
    form("pad-mouse", "the bar's pad mouse switch", |_| Ok(Id::PadMouse)),
    form("scope game", "the settings' item's own tab (a game's, a film's)", |_| Ok(Id::Settings(SettingsId::Scope(Scope::Content)))),
    form("scope runner", "the settings' runner's tab (an app's own)", |_| Ok(Id::Settings(SettingsId::Scope(Scope::Runner)))),
    form("scope all", "the settings' all games tab", |_| Ok(Id::Settings(SettingsId::Scope(Scope::Global)))),
    form("setting KEY", "a settings tile by its key (power, fps, keymap, ...)", |a| {
        let key = arg(a, 0);
        Ok(Id::Settings(SettingsId::Tile(Setting::from_name(key).ok_or_else(|| name_err(key))?)))
    }),
    form("env", "the settings' variables tile: its editor", |_| Ok(Id::Settings(SettingsId::Env))),
    form("save", "the editor's save", |_| Ok(Id::Editor(EditorId::Save))),
    form("key NAME", "an on-screen key: a character, space, ctrl, alt, shift, fn, enter, esc, backspace, tab", |a| key(key_named(arg(a, 0))?)),
    // The keymaps' screens.
    form(
        "keymap N",
        "the keymap picker's row N (for a game 0 its own keymap or Make own, then Default, then the named keymaps; else 0 Default; a long press: its menu)",
        |a| Ok(ListId::Keymaps.row(number(arg(a, 0))?)),
    ),
    form("keymap-edit N", "row N's Edit", |a| Ok(Id::Row { list: ListId::Keymaps, i: number(arg(a, 0))?, part: RowPart::Edit })),
    form("keymap-new", "the picker's New", |_| km(KeymapId::New)),
    form("keymap-back", "back one keymap screen (picking: Cancel)", |_| km(KeymapId::Back)),
    form("keymap-menu ACTION", "a keymap's menu: edit, rename, duplicate, delete; a game's own: load, save-as", |a| {
        km(match arg(a, 0) {
            "edit" => KeymapId::Edit,
            "rename" => KeymapId::Rename,
            "duplicate" => KeymapId::Duplicate,
            "delete" => KeymapId::Delete,
            "load" => KeymapId::LoadFrom,
            "save-as" => KeymapId::SaveAs,
            other => return Err(name_err(other)),
        })
    }),
    form("keymap-delete", "deleting confirmed", |_| km(KeymapId::ConfirmDelete)),
    form("keymap-replace", "saving over a named keymap confirmed", |_| km(KeymapId::ConfirmSave)),
    form("keymap-make-own", "the edit screen's Make own (a game's own keymap, a copy)", |_| km(KeymapId::MakeOwn)),
    form("keymap-undo", "the edit screen's Undo", |_| km(KeymapId::Undo)),
    form("map-from INPUT", "an input selected (a, lb, up, lstick, rstick-up, lt, gyro; a long press sets it back)", |a| {
        km(KeymapId::Input(input_named(arg(a, 0))?))
    }),
    form("keymap-act ACT", "the panel's tab: press, hold, double, full", |a| {
        let act = arg(a, 0);
        km(KeymapId::Actuation(Actuation::from_name(act).ok_or_else(|| name_err(act))?))
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

/// Puts a finger down on a slider at `from` and moves it to `to` in steps. The finger is lifted at the end
/// unless `hold` is set.
pub(super) fn slide(control: Control, from: f32, to: f32, hold: bool) -> Vec<Event> {
    let id = Id::Control(control);
    let mut events = vec![PointerEvent::Down(id.clone(), None).into(), Slide::Start { control, x: from, width: SLIDE_WIDTH }.into()];
    events.extend((1..=SLIDE_STEPS).map(|i| Slide::To { x: from + (to - from) * f32::from(i) / f32::from(SLIDE_STEPS) }.into()));
    if !hold {
        events.extend([Slide::End.into(), PointerEvent::Up(id).into()]);
    }
    events
}

pub(super) fn pad(button: &str) -> Result<Vec<Event>, ParseError> {
    let input = |i: PadInput| vec![Event::Pad(i)];
    Ok(match button {
        "up" => input(PadInput::Up),
        "down" => input(PadInput::Down),
        "left" => input(PadInput::Left),
        "right" => input(PadInput::Right),
        "a" => vec![Event::Pad(PadInput::A(true)), Event::Pad(PadInput::A(false))],
        "b" => input(PadInput::B),
        "y" => input(PadInput::Y),
        "x" => input(PadInput::X),
        "start" => input(PadInput::Start),
        "lb" => input(PadInput::Shoulder(-1)),
        "rb" => input(PadInput::Shoulder(1)),
        "switch" => input(PadInput::Switch),
        "back" => input(PadInput::Back),
        other => return Err(ParseError::Name(other.to_owned())),
    })
}

pub(super) fn input_named(word: &str) -> Result<Input, ParseError> {
    Input::parse(word).ok_or_else(|| name_err(word))
}

/// A target of the "map to" screen by name.
pub(super) fn target_named(word: &str) -> Result<Target, ParseError> {
    let name = || ParseError::Name(word.to_owned());
    let side = |w: &str| match w {
        "left" => Ok(Side::Left),
        "right" => Ok(Side::Right),
        _ => Err(name()),
    };
    Ok(match word.split_once('-') {
        None => match word {
            "default" => Target::Default,
            "nothing" => Target::To(Output::Nothing),
            "pick" => Target::Pick,
            "mouse" => Target::MouseParts,
            "layers" => Target::Layers,
            "key" => Target::Key,
            "keys" => Target::Keys,
            "pointer" => Target::To(Output::Pointer),
            "dpad" => Target::To(Output::Dpad),
            _ => return Err(name()),
        },
        Some(("pad", input)) => Target::To(Output::Pad(PadOut::of(input_named(input)?).ok_or_else(name)?)),
        Some(("mouse", m)) => Target::To(Output::Mouse(MouseButton::from_name(m).ok_or_else(name)?)),
        Some(("wheel", d)) => Target::To(Output::Wheel(Dir::from_name(d).ok_or_else(name)?)),
        Some(("stick", s)) => Target::To(Output::Stick(side(s)?)),
        Some(("trigger", s)) => Target::To(Output::Trigger(side(s)?)),
        _ => return Err(name()),
    })
}

/// An on-screen keyboard key by name.
pub(super) fn key_named(word: &str) -> Result<Key, ParseError> {
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

/// Types text on the on-screen keyboard: each character's key, preceded by one Shift tap if it needs it.
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
