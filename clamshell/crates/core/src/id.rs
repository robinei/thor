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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorId {
    Save,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WifiId {
    /// The radio's switch.
    Power,
    /// A network, by position.
    Network(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BtId {
    /// The radio's switch.
    Power,
    /// A device, by position.
    Device(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioId {
    /// The output volume's mute button.
    MuteOutput,
    /// An app's mute button, by position.
    StreamMute(u16),
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
