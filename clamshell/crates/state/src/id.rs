//! Stable ids for everything that can be focused or tapped on either screen. [`Id`] groups them by what
//! they belong to, with an enum of targets per screen (`ShelfId`, `DockId`, `KeymapId`, ...). These ids
//! name targets, not the things the targets act on; a keymap's own id, for example, is its number, a
//! `u16`.
//!
//! Focus is kept by id, so it survives redraws and layout changes. List items are named by position, so
//! drawing needs no allocation, and are resolved to what they are when pressed (see
//! [`crate::nav::items`]).

use std::sync::Arc;

use crate::bottom::Tab;
use crate::bottom::keyboard::KeyPos;
use crate::levels::Level;
use crate::levels::slider::Control;
use crate::settings::Setting;
use crate::settings::panel::Scope;
use crate::status::time::ClockField;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    /// Shelf, on the main screen.
    Shelf(ShelfId),
    /// The dock tab.
    Dock(DockId),
    /// The settings tab.
    Settings(SettingsId),
    /// The editor's buttons, for the environment or a Wi-Fi password.
    Editor(EditorId),
    Wifi(WifiId),
    Bluetooth(BtId),
    /// The volume tab's buttons.
    Audio(AudioId),
    /// A level row in the volume or brightness tab, set by a finger.
    Level(Level),
    /// An on-screen keyboard key, by position.
    Key(KeyPos),
    /// A tab's icon in the bar.
    Tab(Tab),
    /// Volume or brightness in the bar. A tap opens its tab, and a hold or a slide brings up its slider.
    Control(Control),
    /// The pad mouse toggle.
    PadMouse,
    /// A row of a scrolled list by position, or one of the row's controls.
    Row {
        list: crate::nav::lists::ListId,
        i: u16,
        part: crate::nav::lists::RowPart,
    },
    /// Buttons on the keymap screens (see [`crate::keymap::screens`]).
    Keymap(KeymapId),
    /// Buttons on a kind's picker (see [`crate::profiles::picker`]).
    Profile(crate::profiles::picker::Kind, crate::profiles::picker::ProfileId),
    /// A scrolled list's area, under its rows. A finger on it between rows drags the list, and a tap does
    /// nothing.
    List(crate::nav::lists::ListId),
    /// The darkened second screen while a game runs. A tap anywhere lights it.
    Wake,
    /// Empty space on the second screen below the bar. A tap does nothing, and a moving finger points
    /// (see [`crate::bottom::touch`]).
    Blank,
    /// The time tab's targets (see [`crate::status::time`]).
    Time(TimeId),
    /// The Device view's targets (see [`crate::device`]).
    Device(DeviceId),
    /// The battery tab's list of what is using the machine (see [`crate::status::battery`]).
    Battery(BatteryId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShelfId {
    Entry(Arc<str>),
    /// A collection, by position.
    Collection(u16),
    Launch,
    /// Leaves the open group, back to the list it is in.
    Up,
    /// Actions in the details: make the selected entry a favourite or not, and pin it to the dock or not.
    Favourite,
    Pin,
    /// Plays a folder's next video (see [`crate::Library::to_continue`]).
    Continue,
    /// Adds the selected entry, or the selected entries, to what its runner plays (see
    /// [`crate::session::enqueue`]).
    Enqueue,
    /// A file action on a browsed entry or on the open folder (see [`crate::shelf::files`]).
    File(crate::shelf::files::FileAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockId {
    /// An entry on the dock, by position.
    Entry(u16),
    /// The menu a long press opens (see [`crate::session::dock`]). Its items: open another, close the
    /// window in view, pin or unpin, move, quit, force kill, and back to the dock.
    OpenAnother,
    CloseWindow,
    Pin,
    Move,
    Quit,
    ForceKill,
    Back,
    /// The corner mark of the tile at this position, which opens its program's tray menu (see
    /// [`crate::session::tray`]).
    Tray(u16),
    /// Menu in the long-press menu, which opens the same tray menu.
    TrayMenu,
    /// Start with ClamShell, or Don't, in the long-press menu (see [`crate::session::autostart`]).
    Autostart,
    /// Suspend or Resume in the long-press menu (see [`crate::session::background`]).
    Suspend,
    /// An entry of the tray menu, by its id there.
    TrayEntry(i32),
    /// In move mode, moves the entry one place or row along. `Done` ends move mode.
    Step(crate::keymap::Dir),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsId {
    /// A tile. A tap steps to the next value, and a long press goes back to the default.
    Tile(Setting),
    /// The environment variables tile.
    Env,
    /// The panel's scope tabs.
    Scope(Scope),
    /// Drops the overrides of the game in context.
    Reset,
}

/// Buttons on the keymap screens, but the picker's and Back, which are [`Id::Profile`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeymapId {
    /// Makes a game its own keymap from the one in force for it. This is a row in the picker, and an
    /// action on the edit screen while a shared keymap is being edited.
    MakeOwn,
    /// An input on the edit screen. A tap selects it and shows its panel, and a tap on the selected one
    /// opens the way it is mapped now. A long press resets it in the layer shown. While a control is being
    /// picked, this is the control picked.
    Input(crate::keymap::Input),
    /// Makes the edit screen show and edit this layer. A long press opens the layer's menu.
    Layer(crate::keymap::Layer),
    /// Undo on the edit screen, which puts the keymap back as it was before its last change.
    Undo,
    /// A layer's Edit in the list of layers, which opens its menu.
    EditLayer(crate::keymap::Layer),
    /// The Once switch in a layer's menu.
    LayerOnce,
    /// The edit screen's layer button. It opens the list of the keymap's layers, to show one or add one,
    /// or with a long press to open a layer's menu.
    LayerPicker,
    /// Rename and delete in a layer's menu.
    RenameLayer,
    DeleteLayer,
    /// In the list of layers chosen as a target: this layer, or a new one.
    ToLayer(crate::keymap::Layer),
    NewLayer,
    /// Makes the panel show and edit the selected input's binding for this actuation: press, hold or
    /// double.
    Actuation(crate::keymap::Actuation),
    /// "No changes", which clears the layer shown once confirmed.
    Clear,
    ConfirmClear,
    /// A way to remap the selected input, from the panel's icons, or what it maps to.
    Target(Target),
    /// Flips a switch among the selected input's parameters, such as an inversion.
    Toggle(crate::keymap::Param),
    /// The gyro panel's "Active" row, which opens the choice of what holds the gyro on.
    Holds,
    /// Calibrates the gyro from its next second of readings, with the device held still.
    Calibrate,
    /// What holds the gyro on. None means it is always on.
    Hold(Option<crate::keymap::Input>),
    /// Takes the modifiers alone as the chord, for example Shift as a key.
    Done,
}

/// What the selected input is remapped to. It is either a way to choose the output, from the panel's
/// icons, or the output itself. A chooser closes as soon as something is chosen, and Back gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// No binding. In Base, the input works as on the pad, and the gyro is off. In another layer, it works
    /// as in the layers below.
    Default,
    /// A control picked on the pad's drawing by tapping it or pressing it on the pad: a button, a trigger,
    /// a stick or the D-pad.
    Pick,
    /// A key chord typed on the keyboard.
    Key,
    /// Four keys typed in turn, for a stick.
    Keys,
    /// A part of the mouse picked on its drawing: a button or the wheel.
    MouseParts,
    /// A layer of the keymap, picked from the list. The layer is on while the binding is held.
    Layers,
    /// An output chosen directly.
    To(crate::keymap::Output),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorId {
    Save,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WifiId {
    /// The radio's switch.
    Power,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BtId {
    /// The radio's switch.
    Power,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioId {
    /// The output volume's mute button.
    MuteOutput,
    /// An output, by position.
    Output(u16),
}

impl Id {
    /// Whether this target is on the main screen. Shelf's are; everything else is on the second screen.
    pub(crate) fn on_top(&self) -> bool {
        matches!(self, Id::Shelf(_))
    }
}

/// The time tab's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeId {
    /// The header's back button. It goes from a region's zones to the regions, and from those to the main
    /// page.
    Back,
    /// The time zone row, which opens the regions.
    Zone,
    /// Whether the network sets the clock.
    Ntp,
    /// 12-hour or 24-hour clocks.
    TwelveHour,
    /// Steps a field up (`true`) or down, while the clock is set by hand.
    Step(ClockField, bool),
}

/// The Device view's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceId {
    /// The Wi-Fi row, which opens the Wi-Fi tab.
    Wifi,
    /// The time zone row, which opens the time tab.
    Zone,
    Password,
    Ssh,
    /// Calibrates the sticks.
    Sticks,
    /// Calibrates the gyro.
    Gyro,
    /// Calibrates the top screen's refresh rate.
    Refresh,
    /// Finishes the first-start setup.
    Done,
    /// Back to shelf.
    Close,
}

/// The battery tab's list of what is using the machine. A tap on a row opens its menu, which offers
/// Quit, Force kill, or back to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BatteryId {
    /// The list's row, by position.
    Spender(u16),
    Quit,
    ForceKill,
    Back,
}

/// What holding a target does once the press lasts beyond a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Activate now and again at the repeat rate until released, as keyboard keys do.
    Repeat,
    /// One long press, then nothing on release. Keyboard modifiers get this too; they act while held
    /// through `Down` and `Up`, not through the long press.
    Long,
}
