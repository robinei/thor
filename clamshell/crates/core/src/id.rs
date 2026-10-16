//! Stable ids of everything that can be focused or tapped, on either screen, grouped by what they belong
//! to. Focus is kept by id, so it survives redraws and layout changes; list items are named by position
//! (so drawing needs no allocation) and resolved by what they are when pressed (see [`crate::items`]).

use std::sync::Arc;

use crate::Dirty;
use crate::keyboard::KeyPos;
use crate::levels::Level;
use crate::panel::Scope;
use crate::settings::Setting;
use crate::slider::Control;
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
        list: crate::scroll::ListId,
        i: u16,
        part: crate::scroll::RowPart,
    },
    /// The keymaps' screens' buttons (see [`crate::keymap_screens`]).
    Keymap(KeymapId),
    /// A scrolled list's area, under its rows: a finger on it (between rows) drags the list; a tap does
    /// nothing.
    List(crate::scroll::ListId),
    /// The dark second screen (while a game runs): a tap anywhere lights it.
    Wake,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShelfId {
    Game(Arc<str>),
    /// A collection, by position.
    Collection(u16),
    Launch,
    /// The choice when starting a game while another runs: keep that one running, or close it.
    StartKeep,
    StartClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockId {
    /// An app, by position.
    App(u16),
    /// A running game's tile, by position among the running games.
    GameTile(u16),
    /// The first tile while no game runs: to shelf, to start one.
    StartGame,
    /// The "+" on a running app that allows several instances: another one.
    NewInstance(u16),
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
    /// No binding: as on the pad in the main layer (the gyro: off), as the main layer in the Back layer.
    Default,
    Nothing,
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
    Pointer,
    Pad(crate::keymap::PadOut),
    Mouse(crate::keymap::MouseButton),
    Wheel(crate::keymap::Dir),
    Stick(crate::keymap::Side),
    Dpad,
    Trigger(crate::keymap::Side),
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
