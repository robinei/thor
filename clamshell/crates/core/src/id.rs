//! Stable ids of everything that can be focused or tapped, on either screen, grouped by what they belong
//! to: [`Id`], and an enum per screen of its targets (`ShelfId`, `DockId`, `KeymapId`, ...). These name
//! targets, not the things they act on (a keymap's own id is its number, a `u16`). Focus is kept by id, so
//! it survives redraws and layout changes; list items are named by position (so drawing needs no
//! allocation) and resolved by what they are when pressed (see [`crate::nav::items`]).

use std::sync::Arc;

use crate::Dirty;
use crate::keyboard::KeyPos;
use crate::levels::Level;
use crate::levels::slider::Control;
use crate::panel::Scope;
use crate::settings::Setting;
use crate::tab::Tab;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    /// Shelf, on the main screen.
    Shelf(ShelfId),
    /// The dock tab.
    Dock(DockId),
    /// The settings tab.
    Settings(SettingsId),
    /// The editor's buttons (the environment, a Wi-Fi password).
    Editor(EditorId),
    Wifi(WifiId),
    Bluetooth(BtId),
    /// The volume tab's buttons.
    Audio(AudioId),
    /// A level row in the volume or brightness tab: the finger sets it.
    Level(Level),
    /// An on-screen keyboard key, by position.
    Key(KeyPos),
    /// A tab's icon in the bar.
    Tab(Tab),
    /// Volume or brightness in the bar: a tap opens its tab, a hold or a slide brings up its slider.
    Control(Control),
    /// The pad mouse toggle.
    PadMouse,
    /// A row of a scrolled list, by position: the row, or one of its controls.
    Row {
        list: crate::nav::lists::ListId,
        i: u16,
        part: crate::nav::lists::RowPart,
    },
    /// The keymaps' screens' buttons (see [`crate::keymap::screens`]).
    Keymap(KeymapId),
    /// A scrolled list's area, under its rows: a finger on it (between rows) drags the list; a tap does
    /// nothing.
    List(crate::nav::lists::ListId),
    /// The dark second screen (while a game runs): a tap anywhere lights it.
    Wake,
    /// The second screen below the bar where there is nothing: a tap does nothing, a finger moving
    /// points (see [`crate::touch`]).
    Blank,
    /// The time tab's (see [`crate::time`]).
    Time(crate::time::TimeId),
    /// The Device view's (see [`crate::device`]).
    Device(crate::device::DeviceId),
    /// The battery tab's list of what is using the machine (see [`crate::battery`]).
    Battery(crate::battery::BatteryId),
    /// The bar's button for the front program's windows on the bottom screen, hidden by ClamShell's view:
    /// they are shown.
    BottomWindows,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShelfId {
    Entry(Arc<str>),
    /// A collection, by position.
    Collection(u16),
    Launch,
    /// Out of the open group (back to the list it is in).
    Up,
    /// The details' actions: the selected entry a favourite or not, pinned to the dock or not.
    Favourite,
    Pin,
    /// A folder's next video played (see [`crate::Library::to_continue`]).
    Continue,
    /// The selected entry (or the entries selected) added to what its runner plays (see [`crate::enqueue`]).
    Enqueue,
    /// A browsed entry's file action, or the folder open's (see [`crate::files`]).
    File(crate::files::FileAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockId {
    /// An entry on the dock, by position.
    Entry(u16),
    /// The menu a long press opens (see [`crate::dock`]): another launch, the window in view closed,
    /// pinned or unpinned, moved, quit, force killed, back to the dock.
    OpenAnother,
    CloseWindow,
    Pin,
    Move,
    Quit,
    ForceKill,
    Back,
    /// The corner mark of the tile at this position: its program's tray menu (see [`crate::tray`]).
    Tray(u16),
    /// Its long-press menu's Menu: the same.
    TrayMenu,
    /// Its long-press menu's Start with ClamShell, or Don't (see [`crate::autostart`]).
    Autostart,
    /// Its long-press menu's Suspend, or Resume (see [`crate::background`]).
    Suspend,
    /// An entry of that menu, by its id there.
    TrayEntry(i32),
    /// Move mode: the entry one place (or row) along; done.
    Step(crate::keymap::Dir),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsId {
    /// A tile: a tap steps to the next value, a long press goes back to the default.
    Tile(Setting),
    /// The environment variables tile.
    Env,
    /// The panel's scope tabs.
    Scope(Scope),
    /// Drops the game in context's overrides.
    Reset,
}

/// The keymaps' screens' buttons (the picker's rows are [`Id::Row`]s of [`crate::ListId::Keymaps`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeymapId {
    /// Back one screen.
    Back,
    /// The picker's New.
    New,
    /// A keymap's menu.
    Edit,
    Rename,
    Duplicate,
    Delete,
    /// Deleting a keymap games use, after all.
    ConfirmDelete,
    /// A game's own keymap made from the one in force there (the picker's row, the edit screen's action
    /// while a shared keymap is edited).
    MakeOwn,
    /// A game's own keymap's menu: the named keymaps to save it over, or as a new one.
    SaveAs,
    /// Saving over a named keymap, after all.
    ConfirmSave,
    /// A game's own keymap's menu: the keymaps to load into it.
    LoadFrom,
    /// An input on the edit screen: a tap selects it (its panel), a tap on the selected one opens the way
    /// it is mapped now; a long press sets it back (in the layer shown). While a control is being picked,
    /// the one picked.
    Input(crate::keymap::Input),
    /// The edit screen shows and edits this layer (a long press: its menu).
    Layer(crate::keymap::Layer),
    /// The edit screen's Undo: the keymap as it was before its last change.
    Undo,
    /// A layer's Edit, in the list of layers: its menu.
    EditLayer(crate::keymap::Layer),
    /// A layer's menu: its Once switch.
    LayerOnce,
    /// The edit screen's layer button: the list of the keymap's layers (to show one, add one, or a
    /// long press: its menu).
    LayerPicker,
    /// A layer's menu: rename it, delete it.
    RenameLayer,
    DeleteLayer,
    /// The layers' list (a layer as a target): this layer, or a new one.
    ToLayer(crate::keymap::Layer),
    NewLayer,
    /// The panel shows and edits the selected input's binding of this actuation (press, hold, double).
    Actuation(crate::keymap::Actuation),
    /// "No changes": the layer shown is cleared, once confirmed.
    Clear,
    ConfirmClear,
    /// A way to remap the selected input (the panel's icons), or what it maps to.
    Target(Target),
    /// A switch among the selected input's parameters (an inversion), flipped.
    Toggle(crate::keymap::Param),
    /// The panel's "Active" row of the gyro: the choice of what holds it on opens.
    Holds,
    /// The gyro calibrated from its next second (the device held still).
    Calibrate,
    /// What holds the gyro on (none: always on).
    Hold(Option<crate::keymap::Input>),
    /// The modifiers alone as the chord (Shift as a key).
    Done,
}

/// What the selected input is remapped to: a way to choose it (the panel's icons: one closes as soon as
/// something is chosen, Back gives up), or the output itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// No binding: in Base, as on the pad (the gyro: off); in another layer, as the layers below it.
    Default,
    /// A control picked on the pad's drawing, tapped or pressed on the pad: a button, a trigger, a stick,
    /// the D-pad.
    Pick,
    /// A key chord typed on the keyboard.
    Key,
    /// Four keys typed in turn (a stick).
    Keys,
    /// A part of the mouse picked on its drawing: a button, the wheel.
    MouseParts,
    /// A layer of the keymap, from the list of them (on while the binding is).
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
    /// Which screens a press on it redraws: shelf's targets are on the main screen, and the second
    /// screen's settings tab follows shelf's selection; everything else is on the second screen.
    pub(crate) fn screen(&self) -> Dirty {
        match self {
            Id::Shelf(_) => Dirty::BOTH,
            _ => Dirty::BOTTOM,
        }
    }
}

/// What holding a target does once the press lasts beyond a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Activate now and again at the repeat rate until released (keyboard keys).
    Repeat,
    /// One long press, then nothing on release (locks a modifier).
    Long,
}
