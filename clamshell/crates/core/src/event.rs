//! What comes in: everything that changes the state is an [`Event`], from the UI (input, layout) or a
//! policy thread (readings of the system, the compositor's windows, systemd's scopes, games starting and
//! ending), grouped by what they are about.

use std::sync::Arc;

use crate::battery::Battery;
use crate::bluetooth::Bluetooth;
use crate::hardware::DeviceKey;
use crate::id::Id;
use crate::idle::IdleEvent;
use crate::instance::Instance;
use crate::levels::{Audio, Level};
use crate::metrics::Metrics;
use crate::pad::PadInput;
use crate::scroll::ListEvent;
use crate::slider::Control;
use crate::sys::{Status, TouchpadProblem};
use crate::wifi::Wifi;
use crate::windows::Window;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Wall clock, once a second (policy thread).
    Tick {
        unix_secs: u64,
    },
    /// The system as a policy thread read it.
    Reading(Reading),
    /// Touch, the mouse or the pad on a target.
    Pointer(PointerEvent),
    /// A finger on volume or brightness in the bar, sliding.
    Slide(Slide),
    /// A level row set by the finger, or stepped by the pad.
    Level(LevelEvent),
    /// A scrolled list moved or was laid out.
    List(ListEvent),
    /// The pad in shelf, and shelf's layout.
    Shelf(ShelfEvent),
    /// A game shelf started began, ended, or couldn't start (the launcher thread).
    Game(GameEvent),
    /// The compositor's windows, after any change.
    Windows(Vec<Window>),
    /// Our running scopes (dock app instances and games shelf started), after any change.
    Units(Vec<Instance>),
    /// The UI is up: what the state in force needs at start (the tab's watch, the screens' brightness,
    /// where the pad goes).
    Start,
    /// The pad's home button.
    Home,
    /// View, away from shelf: the pad between the second screen and what else wants it (the pad mouse,
    /// typing).
    PadSwitch,
    /// The pad, while it drives ClamShell (the UI handles these; the state ignores them).
    Pad(PadInput),
    /// An input pressed or moved on the pad while it listens (the keymap's edit screen).
    Listened(crate::keymap::Input),
    /// A text field of the app in front wants typing (true), or no longer does (the Wayland side).
    TextInput(bool),
    /// The system woke from sleep.
    Resumed,
    /// What the input sources say about input (for idle).
    Idle(IdleEvent),
    /// A hardware key (the device's buttons, the pad's Home with RB), or the same asked for by
    /// `clamshell ctl`.
    Device(DeviceKey),
    /// The screenshot asked for is taken (its file), or failed (why).
    Screenshot(Result<String, String>),
    /// A calibration of the gyro asked for ended: whether it took (the device was held still enough).
    GyroCalibrated(bool),
    Quit,
}

/// What a policy thread read from the system.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Battery, Wi-Fi signal, volume, brightness (when they change).
    Status(Status),
    /// CPU, GPU, memory, temperatures, power (once a second).
    Metrics(Metrics),
    /// A running game's frame rate and frame time in ms (MangoHud), or none.
    Frames { fps: Option<f32>, frame_ms: Option<f32> },
    /// Wi-Fi as NetworkManager has it, after any change (while the Wi-Fi tab is open, or a join runs).
    Wifi(Wifi),
    /// Bluetooth as BlueZ has it, after any change.
    Bluetooth(Bluetooth),
    /// The charge history and what is using the machine (every few seconds while the battery tab is
    /// open).
    Battery(Battery),
    /// The outputs and the apps playing (while the volume tab is open).
    Audio(Audio),
    /// The backlights' names (while the brightness tab is open).
    Backlights(Vec<String>),
    /// Touchpad mode works, or why it doesn't.
    TouchpadProblem(Option<TouchpadProblem>),
}

/// A press on a target, from touch, the mouse or the pad's A; and the pad's focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerEvent {
    /// A finger, button or the pad's A went down on / up from a target: shown as pressed meanwhile.
    Down(Id),
    Up(Id),
    /// Tap, click, or the pad's A on a focused item.
    Activate(Id),
    /// A press held longer than a tap (locks a keyboard modifier; otherwise as `Activate`).
    LongPress(Id),
    /// The pad moved focus (the UI finds the target, since only it knows the layout). Shows the focus.
    Focus(Id),
    /// Touch or mouse input began: the focus ring is hidden until the pad moves focus again.
    HideFocus,
}

/// A finger on a bar control, sliding (`x` in the second screen's layout units, which is `width` wide).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slide {
    /// The finger came down (or was held long enough): the slider comes up under it.
    Start {
        control: Control,
        x: f32,
        width: f32,
    },
    To {
        x: f32,
    },
    /// The finger that slides lifted.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelEvent {
    /// A level row set by the finger.
    Set(Level, u8),
    /// A level row stepped by the pad (-1 or 1).
    Step(Level, i16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfEvent {
    /// The pad moved through the list: by this many games (a page is the rows on screen).
    ListStep(i32),
    /// The pad switched collections: by this many.
    CollectionStep(i32),
    /// How many list rows shelf shows (from the layout), so the selection stays on screen.
    Rows(usize),
    /// The pad's Y (desktop: F): the selected game in or out of the favourites.
    ToggleFavourite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// The launcher started a game.
    Started(Arc<str>),
    /// It ended (start in epoch seconds, duration in seconds).
    Played { game: Arc<str>, start: u64, duration: u64 },
    /// It couldn't be started.
    LaunchFailed(Arc<str>),
}

macro_rules! into_event {
    ($($group:ident => $variant:ident),*) => {
        $(impl From<$group> for Event {
            fn from(e: $group) -> Event {
                Event::$variant(e)
            }
        })*
    };
}

into_event!(ListEvent => List, Reading => Reading, PointerEvent => Pointer, Slide => Slide, LevelEvent => Level, ShelfEvent => Shelf, GameEvent => Game);
