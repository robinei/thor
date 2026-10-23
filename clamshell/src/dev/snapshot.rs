//! `--snapshot DIR` draws both screens at the Thor's size into PNG files, without Wayland. The shots cover
//! shelf, the second screen and every tab. Each is drawn after the events that lead to it, in groups that
//! each start from a fresh state. They are taken at a fixed time unless CLAMSHELL_SNAPSHOT_NOW is set, so
//! they come out the same on every run. The test below keeps hashes of their pixels in tests/snapshots.txt
//! to catch changes in what is drawn.

use std::fmt;
use std::path::Path;

use clamshell_core::Control;
use clamshell_core::DeviceId;
use clamshell_core::bottom::keyboard::{Key, Modifier, Named};
use clamshell_core::device::SticksEvent;
use clamshell_core::keymap::{Actuation, Button, Dir, Input, Layer, MouseButton, Output, PadOut, Param, Side};
use clamshell_core::levels::Level;
use clamshell_core::settings::Setting;
use clamshell_core::{
    Dock, EditorId, Event, Id, Init, KeymapId, LevelEvent, Library, ListEvent, ListId, PointerEvent, Reading, Scope, SettingsId, ShelfEvent, ShelfId, Slide,
    State, Status, Tab, Target, TimeId,
};
use clamshell_ui::Screen;

use crate::control::{self, ParseError};
use crate::dev::demo;

/// The time the snapshots are taken at, unless the current time is asked for.
pub const FIXED_NOW: u64 = 1_760_000_000;

/// `--snapshot DIR`: draws the snapshots of the demo library, or of the real library if
/// CLAMSHELL_SNAPSHOT_LIBRARY=real. They are taken at [`FIXED_NOW`], or at the current time if
/// CLAMSHELL_SNAPSHOT_NOW is set.
pub fn command(sources: &clamshell_system::library::Sources, dir: &Path) -> Result<(), String> {
    let real = std::env::var("CLAMSHELL_SNAPSHOT_LIBRARY").is_ok_and(|v| v == "real");
    let library = if real { clamshell_system::library::load(sources).library } else { clamshell_library::demo::library() };
    let now = if std::env::var_os("CLAMSHELL_SNAPSHOT_NOW").is_some() { clamshell_system::time::clock::unix_secs() } else { FIXED_NOW };
    render(&|| state(library.clone()), now, dir).map_err(|e| e.to_string())
}

/// One file: the screen drawn after the events in `before`.
pub struct Shot {
    pub name: &'static str,
    pub screen: Screen,
    pub before: Vec<Event>,
}

/// A screen's size in pixels: the Thor's panels.
fn size(screen: Screen) -> (u32, u32) {
    match screen {
        Screen::Top => (1920, 1080),
        Screen::Bottom => (1240, 1080),
    }
}

#[derive(Debug)]
pub enum Error {
    /// A key the scenario presses isn't on the keyboard.
    Keyboard(ParseError),
    /// A file couldn't be drawn: the file and the reason.
    Render(String, String),
    /// A group's thread panicked.
    Panicked,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Keyboard(e) => write!(f, "the scenario's keys: {e}"),
            Error::Render(file, e) => write!(f, "{file}: {e}"),
            Error::Panicked => write!(f, "a group's drawing panicked"),
        }
    }
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Keyboard(e)
    }
}

/// Opens shelf's Tools collection and runs its Device entry by selecting and then activating it. This
/// shows the Device view.
fn open_device() -> Vec<Event> {
    let library = clamshell_library::demo::library();
    let tools = state(library).shelf().library().collections.iter().position(|c| c.name == "Tools").unwrap_or(0);
    let device = || tap(Id::Shelf(ShelfId::Entry("tool:device".into())));
    vec![tap(Id::Shelf(ShelfId::Collection(u16::try_from(tools).unwrap_or(0)))), device(), device()]
}

/// The state the snapshots start from: a library with its apps pinned to the dock, and nothing else
/// remembered.
pub fn state(library: Library) -> State {
    let dock = Dock::pinned(vec!["desktop:steam".into(), "desktop:foot".into(), "desktop:firefox".into()]);
    State::new(Init { library, dock, keymaps: clamshell_core::keymap::Keymaps::initial(), ..Init::default() })
}

/// A play of `entry` from `start` for `duration` seconds, as the launcher reports one. The clock is set
/// back to `now` afterwards.
fn a_play(entry: &str, start: u64, duration: u64, now: u64) -> Vec<Event> {
    // The game has a window on its workspace while it plays, as a real game would.
    let workspace = clamshell_core::Instance::workspace_of(&clamshell_library::key_of(entry));
    let window = clamshell_core::session::windows::Window {
        id: 900,
        app_id: "game".into(),
        workspace,
        visible: true,
        ..clamshell_core::session::windows::Window::default()
    };
    vec![
        Event::tick(start),
        Event::Units(vec![clamshell_core::Instance::Run { key: clamshell_library::key_of(entry), secs: start, n: 0 }]),
        Event::Windows(vec![window]),
        Event::tick(start + duration),
        Event::Units(vec![]),
        Event::Windows(vec![]),
        Event::tick(now),
    ]
}

fn tap(id: Id) -> Event {
    PointerEvent::Activate(id).into()
}

fn key(k: Key) -> Result<Event, ParseError> {
    control::key(k).map(tap)
}

/// A tap on a button of the keymaps' screens.
fn km(id: KeymapId) -> Event {
    tap(Id::Keymap(id))
}

/// Selects an input on the keymap edit screen. The panel's ways to remap then apply to it.
fn select(input: Input) -> Vec<Event> {
    vec![km(KeymapId::Input(input))]
}

/// Slides the selected input's parameter by `by` times half the width of the Thor's second screen. The
/// finger is lifted if `lift` is set.
fn slide(param: Param, by: f32, lift: bool) -> Vec<Event> {
    let control = clamshell_core::Control::Param(param);
    // The slide runs along the screen-wide track, as touch reports it, starting mid-screen in the panel.
    let (x, width) = (620.0, 1240.0);
    let mut events = vec![Slide::Start { control, x, width }.into(), Slide::To { x: x + by * width * 0.5 }.into()];
    if lift {
        events.push(Slide::End.into());
    }
    events
}

/// A tap on the settings' keymap tile, which opens the picker.
fn keymap_tile() -> Event {
    tap(Id::Settings(SettingsId::Tile(Setting::Keymap)))
}

fn tab(t: Tab) -> Event {
    tap(Id::Tab(t))
}

/// `n` taps on a settings tile.
fn taps(setting: Setting, n: usize) -> impl Iterator<Item = Event> {
    (0..n).map(move |_| tap(Id::Settings(SettingsId::Tile(setting))))
}

/// A group of shots. It starts from a fresh state with `start`'s events applied, then takes each shot
/// after the events that lead to it, building on the group's earlier shots. Groups don't share state, so
/// a change shows in the shots of the groups it touches.
pub struct Group {
    pub start: Vec<Event>,
    pub shots: Vec<Shot>,
}

/// A keymap named FPS, made on the settings' keymap screens and left on its edit screen with A selected.
/// It maps A to Ctrl+X; L1 to right click, Esc when held and Tab when pressed twice; the left stick to
/// WASD; the right stick's up to the wheel; the gyro to the pointer while L2 is held; Select to M; the
/// D-pad's left to 1; and B to the pad's Y. R1 has turbo and the right stick's deadzone is tuned.
fn fps_keymap() -> Result<Vec<Event>, ParseError> {
    Ok([
        vec![tab(Tab::Settings), keymap_tile(), km(KeymapId::New)],
        control::type_text("FPS")?,
        vec![tap(Id::Editor(EditorId::Save))],
        select(Input::Button(Button::A)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Mod(Modifier::Ctrl))?, key(Key::Char('x'))?],
        select(Input::Button(Button::LB)),
        vec![km(KeymapId::Target(Target::To(Output::Mouse(MouseButton::Right))))],
        select(Input::Stick(Side::Left)),
        vec![km(KeymapId::Target(Target::Keys)), key(Key::Char('w'))?, key(Key::Char('a'))?, key(Key::Char('s'))?, key(Key::Char('d'))?],
        select(Input::StickDir(Side::Right, Dir::Up)),
        vec![km(KeymapId::Target(Target::To(Output::Wheel(Dir::Up))))],
        select(Input::Gyro),
        vec![km(KeymapId::Target(Target::To(Output::Pointer))), km(KeymapId::Holds), km(KeymapId::Hold(Some(Input::Trigger(Side::Left))))],
        select(Input::Button(Button::View)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Char('m'))?],
        select(Input::Dpad(Dir::Left)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Char('1'))?],
        select(Input::Button(Button::B)),
        vec![km(KeymapId::Target(Target::To(Output::Pad(PadOut::Button(Button::Y)))))],
        select(Input::Button(Button::LB)),
        vec![km(KeymapId::Actuation(Actuation::Hold)), km(KeymapId::Target(Target::Key)), key(Key::Named(Named::Esc))?],
        vec![km(KeymapId::Actuation(Actuation::Double)), km(KeymapId::Target(Target::Key)), key(Key::Named(Named::Tab))?],
        select(Input::Button(Button::RB)),
        slide(Param::Turbo, 0.2, true),
        vec![km(KeymapId::Input(Input::Stick(Side::Right)))],
        slide(Param::Deadzone, 0.12, true),
        vec![km(KeymapId::Input(Input::Button(Button::A)))],
    ]
    .concat())
}

/// [`fps_keymap`] with a layer named aim that maps A to Y. The layer is shown with L1 selected, which is
/// as in Base there.
fn fps_with_layer() -> Result<Vec<Event>, ParseError> {
    Ok([
        fps_keymap()?,
        vec![km(KeymapId::LayerPicker), km(KeymapId::NewLayer)],
        control::type_text("aim")?,
        vec![tap(Id::Editor(EditorId::Save))],
        select(Input::Button(Button::A)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Char('y'))?, km(KeymapId::Input(Input::Button(Button::LB)))],
    ]
    .concat())
}

/// The scenario: groups of shots (see [`Group`]).
pub fn shots(now: u64) -> Result<Vec<Group>, ParseError> {
    use Screen::{Bottom, Top};
    let shot = |name, screen, before| Shot { name, screen, before };
    let group = |start, shots| Group { start, shots };
    let ff6 = || tap(Id::Shelf(ShelfId::Entry("ff6".into())));
    let ctrl = || control::key(Key::Mod(Modifier::Ctrl));
    let volume = Id::Control(Control::Volume);
    Ok(vec![
        group(
            vec![],
            vec![
                shot("top.png", Top, vec![]),
                shot("top-tab2.png", Top, vec![ShelfEvent::CollectionStep(1).into()]),
                shot("top-tab2-down.png", Top, vec![ShelfEvent::ListStep(5).into()]),
                shot("top-back.png", Top, vec![ShelfEvent::CollectionStep(-1).into()]),
                shot(
                    "top-played.png",
                    Top,
                    a_play("zelda-lttp", 1_759_900_000, 4 * 3600 + 1260, now)
                        .into_iter()
                        .chain([ShelfEvent::ToggleFavourite.into(), ShelfEvent::TogglePin.into(), Event::Units(vec![demo::run("zelda-lttp", 0)])])
                        .collect(),
                ),
                shot("top-switch.png", Top, vec![ff6(), ff6()]),
                // A held on the selected entry with the pad opens its actions, with A and B shown at the
                // bottom.
                shot(
                    "top-actions.png",
                    Top,
                    vec![
                        ff6(),
                        Event::Pointer(PointerEvent::Focus(Id::Shelf(ShelfId::Entry("ff6".into())))),
                        Event::Pointer(PointerEvent::LongPress(Id::Shelf(ShelfId::Entry("ff6".into())))),
                    ],
                ),
            ],
        ),
        // A series selected in shelf and then opened, showing its first season's episodes with the first one
        // played. The start sets shelf's rows as a frame would have reported them before the taps.
        group(
            vec![ShelfEvent::Rows(8).into()],
            vec![
                shot("top-folder.png", Top, vec![tap(Id::Shelf(ShelfId::Collection(2)))]),
                shot(
                    "top-group.png",
                    Top,
                    [tap(Id::Shelf(ShelfId::Launch)), tap(Id::Shelf(ShelfId::Launch))]
                        .into_iter()
                        .chain(a_play("tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv", 1_759_900_000, 3600, now))
                        .chain([ShelfEvent::ListStep(2).into()])
                        .collect(),
                ),
                // Recent shows the played episode with its series and season before its title.
                shot("top-recent.png", Top, vec![tap(Id::Shelf(ShelfId::Collection(0)))]),
            ],
        ),
        group(vec![], vec![shot("bottom.png", Bottom, vec![])]),
        // The settings while a game runs.
        group(
            vec![ff6(), ff6()],
            vec![
                shot(
                    "settings-running.png",
                    Bottom,
                    [tab(Tab::Settings)]
                        .into_iter()
                        .chain(taps(Setting::FpsCap, 4))
                        .chain([keymap_tile(), tap(ListId::Keymaps.row(2))])
                        .chain(taps(Setting::Hud, 1))
                        .collect(),
                ),
                shot(
                    "settings-global.png",
                    Bottom,
                    [tap(Id::Settings(SettingsId::Scope(Scope::Global)))]
                        .into_iter()
                        .chain(taps(Setting::Power, 1))
                        .chain(taps(Setting::Rumble, 2))
                        .chain(taps(Setting::ChargeLimit, 2))
                        .collect(),
                ),
                shot(
                    "settings.png",
                    Bottom,
                    a_play("zelda-lttp", now - 60, 60, now).into_iter().chain([tap(Id::Settings(SettingsId::Scope(Scope::Content)))]).collect(),
                ),
                shot(
                    "settings-env.png",
                    Bottom,
                    [tap(Id::Settings(SettingsId::Env))].into_iter().chain(control::type_text("dxvk_hud=fps mesa_vk_wsi_present_mode=fifo")?).collect(),
                ),
                shot("status-low.png", Bottom, vec![tap(Id::Editor(EditorId::Save)), Reading::Status(demo::STATUS_LOW).into()]),
            ],
        ),
        // Naming a new keymap.
        group(
            vec![ff6(), ff6(), tab(Tab::Settings)],
            vec![
                shot("keymap-name.png", Bottom, [keymap_tile(), km(KeymapId::New)].into_iter().chain(control::type_text("FPS")?).collect()),
                shot("keymap-edit-new.png", Bottom, vec![tap(Id::Editor(EditorId::Save))]),
            ],
        ),
        // The FPS keymap's edit screen.
        group(
            fps_keymap()?,
            vec![
                shot("keymap-edit.png", Bottom, vec![]),
                shot("keymap-edit-hold.png", Bottom, vec![km(KeymapId::Input(Input::Button(Button::LB))), km(KeymapId::Actuation(Actuation::Hold))]),
                shot(
                    "keymap-edit-trigger.png",
                    Bottom,
                    vec![
                        km(KeymapId::Input(Input::Trigger(Side::Right))),
                        km(KeymapId::Actuation(Actuation::FullPull)),
                        km(KeymapId::Target(Target::MouseParts)),
                        km(KeymapId::Target(Target::To(Output::Mouse(MouseButton::Left)))),
                    ],
                ),
                shot("keymap-edit-stick.png", Bottom, vec![km(KeymapId::Input(Input::Stick(Side::Left)))]),
                shot("keymap-edit-slider.png", Bottom, slide(Param::Curve, 0.3, false)),
            ],
        ),
        // Its layers.
        group(
            fps_with_layer()?,
            vec![
                shot("keymap-edit-back.png", Bottom, vec![]),
                shot("keymap-layer-picker.png", Bottom, vec![km(KeymapId::LayerPicker)]),
                shot("keymap-layer-menu.png", Bottom, vec![PointerEvent::LongPress(Id::Keymap(KeymapId::Layer(Layer(1)))).into(), km(KeymapId::LayerOnce)]),
                shot("keymap-layer-clear.png", Bottom, vec![km(KeymapId::Clear)]),
                shot(
                    "keymap-layers.png",
                    Bottom,
                    vec![
                        km(KeymapId::Back),
                        km(KeymapId::Back),
                        km(KeymapId::Layer(Layer::BASE)),
                        km(KeymapId::Input(Input::Button(Button::Back))),
                        km(KeymapId::Target(Target::Layers)),
                    ],
                ),
                shot("keymap-edit-layer-key.png", Bottom, vec![km(KeymapId::ToLayer(Layer(1)))]),
                shot("keymap-layer-list.png", Bottom, vec![km(KeymapId::LayerPicker)]),
            ],
        ),
        // Its ways to remap and the gyro, with Back mapped to the layer as a layer key.
        group(
            [
                fps_with_layer()?,
                vec![km(KeymapId::LayerPicker), km(KeymapId::Layer(Layer::BASE))],
                select(Input::Button(Button::Back)),
                vec![km(KeymapId::Target(Target::Layers)), km(KeymapId::ToLayer(Layer(1)))],
            ]
            .concat(),
            vec![
                shot("keymap-mouse.png", Bottom, vec![km(KeymapId::Input(Input::Button(Button::A))), km(KeymapId::Target(Target::MouseParts))]),
                shot("keymap-edit-gyro.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Input(Input::Gyro))]),
                shot("keymap-edit-gyro-accel.png", Bottom, [slide(Param::Accel, 0.25, true), vec![km(KeymapId::Toggle(Param::Twist))]].concat()),
                shot("keymap-holds.png", Bottom, vec![km(KeymapId::Holds)]),
                shot("keymap-edit-rstick.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Input(Input::Stick(Side::Right)))]),
                shot(
                    "keymap-capture.png",
                    Bottom,
                    vec![km(KeymapId::Input(Input::Button(Button::Y))), km(KeymapId::Target(Target::Key)), key(Key::Mod(Modifier::Alt))?],
                ),
                shot("keymap-pick.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Input(Input::Button(Button::B))), km(KeymapId::Target(Target::Pick))]),
            ],
        ),
        // The keymaps' list.
        group(
            [vec![ff6(), ff6()], fps_keymap()?, vec![km(KeymapId::Back)]].concat(),
            vec![
                shot("keymaps.png", Bottom, vec![]),
                shot("keymap-menu.png", Bottom, vec![PointerEvent::LongPress(ListId::Keymaps.row(3)).into()]),
                shot(
                    "keymap-delete.png",
                    Bottom,
                    vec![
                        km(KeymapId::Back),
                        tap(ListId::Keymaps.row(3)),
                        keymap_tile(),
                        PointerEvent::LongPress(ListId::Keymaps.row(3)).into(),
                        km(KeymapId::Delete),
                    ],
                ),
            ],
        ),
        // A game's own keymap, made from FPS after FPS is chosen for the game. Then its menu, the own keymap
        // kept while FPS is chosen again, and FPS's edit screen while games use it.
        group(
            [vec![ff6(), ff6()], fps_keymap()?, vec![km(KeymapId::Back), tap(ListId::Keymaps.row(3)), keymap_tile()]].concat(),
            vec![
                shot("keymaps-chosen.png", Bottom, vec![]),
                shot("keymap-edit-shared.png", Bottom, vec![tap(Id::Row { list: ListId::Keymaps, i: 3, part: clamshell_core::RowPart::Edit })]),
                shot("keymap-own.png", Bottom, vec![km(KeymapId::MakeOwn), km(KeymapId::Back)]),
                shot("keymap-own-menu.png", Bottom, vec![PointerEvent::LongPress(ListId::Keymaps.row(0)).into()]),
                shot("keymap-own-kept.png", Bottom, vec![km(KeymapId::Back), tap(ListId::Keymaps.row(3)), keymap_tile()]),
                shot("keymap-load.png", Bottom, vec![PointerEvent::LongPress(ListId::Keymaps.row(0)).into(), km(KeymapId::LoadFrom)]),
                shot("keymap-save.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::SaveAs)]),
                shot("keymap-replace.png", Bottom, vec![tap(ListId::Keymaps.row(2))]),
                shot("keymap-edit-own.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Back), km(KeymapId::Edit)]),
            ],
        ),
        group(
            vec![],
            vec![
                shot("launcher.png", Bottom, vec![tab(Tab::Dock), PointerEvent::HideFocus.into()]),
                shot("launcher-running.png", Bottom, demo::running()),
                shot("bar-window-below.png", Bottom, demo::window_below()),
                shot("dock-menu.png", Bottom, vec![demo::running().remove(0), PointerEvent::LongPress(Id::Dock(clamshell_core::DockId::Entry(1))).into()]),
            ],
        ),
        group(
            vec![],
            vec![
                shot("wifi.png", Bottom, vec![tab(Tab::Wifi), Reading::Wifi(demo::wifi()).into()]),
                shot("wifi-crowded.png", Bottom, vec![Reading::Wifi(demo::wifi_crowded()).into()]),
                shot("wifi-scrolled.png", Bottom, vec![ListEvent::Scroll(ListId::Wifi, 3.4).into()]),
            ],
        ),
        group(vec![], vec![shot("battery.png", Bottom, demo::battery(now))]),
        group(
            vec![],
            vec![
                shot("time.png", Bottom, vec![tab(Tab::Time), Reading::Time(demo::time_zones()).into()]),
                shot("time-zones.png", Bottom, vec![PointerEvent::Activate(Id::Time(TimeId::Zone)).into()]),
                shot("time-europe.png", Bottom, vec![PointerEvent::Activate(ListId::Time.row(7)).into()]),
            ],
        ),
        // The Device view, opened from the Tools' Device entry in shelf. First off the device, where there is
        // no helper. Then with an account without a password, a stick calibration in progress, and the
        // password typed on the bottom screen, shown as dots.
        group(
            vec![ShelfEvent::Rows(8).into()],
            vec![
                shot("device-unavailable.png", Top, open_device().into_iter().chain([Reading::Time(demo::time_zones()).into()]).collect()),
                shot(
                    "device.png",
                    Top,
                    vec![Reading::Account(clamshell_core::device::account::Account { available: true, password: false, ssh: false }).into()],
                ),
                shot("device-sticks.png", Top, vec![tap(Id::Device(DeviceId::Sticks)), Event::Sticks(SticksEvent::Progress(demo::sticks_progress()))]),
                shot("device-password.png", Bottom, [tap(Id::Device(DeviceId::Password))].into_iter().chain(control::type_text("secret")?).collect()),
            ],
        ),
        // The clock set by hand, on a 12-hour dial.
        group(
            vec![],
            vec![shot(
                "time-by-hand.png",
                Bottom,
                vec![
                    tab(Tab::Time),
                    Reading::Time(clamshell_core::TimeZones { ntp: Some(false), ..demo::time_zones() }).into(),
                    PointerEvent::Activate(Id::Time(TimeId::TwelveHour)).into(),
                ],
            )],
        ),
        group(vec![], vec![shot("bluetooth.png", Bottom, vec![tab(Tab::Bluetooth), Reading::Bluetooth(demo::bluetooth()).into()])]),
        group(
            vec![],
            vec![shot(
                "volume.png",
                Bottom,
                vec![
                    tab(Tab::Volume),
                    Reading::Status(Status { volume: Some(64), brightness: Some(70), battery: Some(76), wifi: Some(80), ..Status::default() }).into(),
                    Reading::Audio(demo::audio()).into(),
                ],
            )],
        ),
        group(
            vec![],
            vec![shot(
                "brightness.png",
                Bottom,
                vec![
                    tab(Tab::Brightness),
                    Reading::Backlights(vec!["dsi0-backlight".into(), "dsi1-backlight".into()]).into(),
                    LevelEvent::Set(Level::Backlight(1), 65).into(),
                    PointerEvent::Up(Level::Backlight(1).id()).into(),
                ],
            )],
        ),
        // The volume's slider over the dock, then the keyboard.
        group(
            vec![],
            vec![
                shot(
                    "slider.png",
                    Bottom,
                    vec![
                        tab(Tab::Dock),
                        PointerEvent::Down(volume.clone(), None).into(),
                        Slide::Start { control: Control::Volume, x: 560.0, width: 1232.0 }.into(),
                        Slide::To { x: 700.0 }.into(),
                    ],
                ),
                shot("keyboard.png", Bottom, vec![Slide::End.into(), PointerEvent::Up(volume).into(), tab(Tab::Keyboard)]),
                shot("keyboard-fn.png", Bottom, vec![key(Key::Mod(Modifier::Fn))?]),
                shot(
                    "keyboard-mods.png",
                    Bottom,
                    vec![key(Key::Mod(Modifier::Fn))?, key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(ctrl()?, None).into()],
                ),
                shot("keyboard-caps.png", Bottom, vec![PointerEvent::Up(ctrl()?).into(), key(Key::Mod(Modifier::Shift))?, key(Key::Caps)?]),
                shot("touchpad.png", Bottom, vec![tab(Tab::Touchpad), tap(Id::PadMouse)]),
                shot("sysinfo.png", Bottom, demo::metrics()),
                shot("keyboard-back.png", Bottom, vec![tab(Tab::Keyboard)]),
                shot("keyboard-pressed.png", Bottom, vec![key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(control::key(Key::Char('g'))?, None).into()]),
            ],
        ),
    ])
}

/// Draws the scenario into `dir`. Each group starts from a fresh state made by `fresh`, and each file is
/// written as soon as its shot is drawn.
pub fn render(fresh: &(dyn Fn() -> State + Sync), now: u64, dir: &Path) -> Result<(), Error> {
    // The groups are independent, so each runs on a thread of its own. Within a group the shots are drawn
    // in order.
    let groups = shots(now)?;
    let results: Vec<Result<(), Error>> = std::thread::scope(|scope| {
        let threads: Vec<_> = groups.into_iter().map(|group| scope.spawn(move || render_group(fresh(), group, now, dir))).collect();
        threads.into_iter().map(|t| t.join().unwrap_or(Err(Error::Panicked))).collect()
    });
    results.into_iter().collect()
}

/// One group's shots, in order, from `state`.
fn render_group(state: State, group: Group, now: u64, dir: &Path) -> Result<(), Error> {
    walk_group(state, group, now, |state, name, screen| {
        let path = dir.join(name);
        let events = clamshell_ui::snapshot(state, screen, size(screen), 1, &path).map_err(|e| Error::Render(path.display().to_string(), e.to_string()))?;
        log::info!("wrote {}", path.display());
        Ok(events)
    })
}

/// Brings `state` to each of the group's shots in turn and calls `shot` with it, the shot's name and its
/// screen. `shot` returns what the frame reports, which is applied, as a live frame's report is: shelf's
/// rows and the lists' rows keep the selections on screen.
fn walk_group(
    mut state: State,
    Group { start, shots }: Group,
    now: u64,
    mut shot: impl FnMut(&State, &str, Screen) -> Result<Vec<Event>, Error>,
) -> Result<(), Error> {
    state.apply(Event::tick(now), &mut Vec::new());
    state.apply(Event::Reading(Reading::Status(demo::STATUS)), &mut Vec::new());
    // The demo's time zone, so the clock reads the same whatever zone the machine is in.
    state.apply(Event::Reading(Reading::Time(demo::time_zones())), &mut Vec::new());
    for event in start {
        state.apply(event, &mut Vec::new());
    }
    // Clear the notice the setup left ("Starting …" for its launches) by moving the clock past it and back.
    state.apply(Event::tick(now + 5), &mut Vec::new());
    state.apply(Event::tick(now), &mut Vec::new());
    for Shot { name, screen, before } in shots {
        for event in before {
            state.apply(event, &mut Vec::new());
        }
        for event in shot(&state, name, screen)? {
            state.apply(event, &mut Vec::new());
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The reference hashes, one `name hash` line per file.
    const REFERENCE: &str = "tests/snapshots.txt";
    const HEADER: &str = "# FNV-1a hashes of the snapshots' sizes and pixels (src/dev/snapshot.rs). After a change meant to change\n\
                          # what is drawn: CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots\n";

    /// Hashes a PNG's size and pixels. The file's bytes are not hashed because they change with the
    /// encoder, while the pixels don't.
    fn pixels_hash(path: &std::path::Path) -> String {
        let image = image::open(path).unwrap().to_rgba8();
        let mut bytes = Vec::with_capacity(8 + image.as_raw().len());
        bytes.extend(image.width().to_le_bytes());
        bytes.extend(image.height().to_le_bytes());
        bytes.extend(image.as_raw());
        format!("{:016x}", clamshell_library::hash::fnv1a64(&bytes))
    }

    /// Draws every shot of the scenario as a live view would, frame after frame with only what changed
    /// drawn, and checks each frame against the shot drawn whole.
    #[test]
    fn damaged_frames_match_whole_ones() {
        let groups = shots(FIXED_NOW).unwrap();
        std::thread::scope(|scope| {
            for group in groups {
                scope.spawn(move || {
                    let frames = |screen| (clamshell_ui::Frames::new(screen, size(screen)).unwrap(), clamshell_ui::Frames::new(screen, size(screen)).unwrap());
                    let (mut top, mut bottom) = (frames(Screen::Top), frames(Screen::Bottom));
                    walk_group(state(clamshell_library::demo::library()), group, FIXED_NOW, |state, name, screen| {
                        let (damaged, whole) = if screen == Screen::Top { (&mut top.0, &mut top.1) } else { (&mut bottom.0, &mut bottom.1) };
                        let (pixels, events) = damaged.draw(state);
                        assert!(pixels == whole.whole(state).unwrap(), "{name}: drawn frame after frame, it differs from the whole frame");
                        Ok(events)
                    })
                    .unwrap();
                });
            }
        });
    }

    /// Draws and hashes the snapshots and compares them with the reference hashes. If any differ, the
    /// drawn files are left to look at. After a change that is meant to change what is drawn, check the
    /// files (`cargo run --features dev -- --snapshot DIR`) and then write the new hashes with
    /// `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
    #[test]
    fn snapshots_match_their_reference() {
        let base = std::env::temp_dir().join(format!("clamshell-snapshots-{}", std::process::id()));
        let dir = base.join("shots");
        std::fs::create_dir_all(&dir).unwrap();
        render(&|| state(clamshell_library::demo::library()), FIXED_NOW, &dir).unwrap();
        let mut hashes = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            hashes.insert(entry.file_name().to_string_lossy().into_owned(), pixels_hash(&entry.path()));
        }
        let text: String = std::iter::once(HEADER.to_owned()).chain(hashes.iter().map(|(name, hash)| format!("{name} {hash}\n"))).collect();
        let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(REFERENCE);
        if std::env::var_os("CLAMSHELL_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&reference, text).unwrap();
            let _ = std::fs::remove_dir_all(&base);
            return;
        }
        let expected: BTreeMap<String, String> = std::fs::read_to_string(&reference)
            .expect("tests/snapshots.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once(' ').map(|(n, h)| (n.to_owned(), h.to_owned())))
            .collect();
        let changed: std::collections::BTreeSet<&String> = hashes.keys().chain(expected.keys()).filter(|n| hashes.get(*n) != expected.get(*n)).collect();
        if changed.is_empty() {
            let _ = std::fs::remove_dir_all(&base);
        }
        assert!(
            changed.is_empty(),
            "snapshots changed: {changed:?} (drawn into {}); if meant, update with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`",
            dir.display()
        );
    }
}
