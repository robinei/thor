//! `--snapshot DIR` draws both screens at the Thor's size into PNG files, without Wayland. The shots cover
//! shelf, the second screen and every tab. Each is drawn after the events that lead to it, in groups that
//! each start from a fresh state. They are taken at a fixed time unless CLAMSHELL_SNAPSHOT_NOW is set, so
//! they come out the same on every run. The test below keeps three references in tests/: hashes of the
//! shots' pixels (snapshots.txt), where the pad's focus goes on each shot (focus.txt, see
//! focus_map.rs), and the effects of the events on the way (trace.txt, see trace.rs).

use std::fmt;
use std::path::Path;

use clamshell_state::Control;
use clamshell_state::DeviceId;
use clamshell_state::bottom::keyboard::{Key, Modifier, Named};
use clamshell_state::device::SticksEvent;
use clamshell_state::keymap::{Actuation, Button, Dir, Input, Layer, MouseButton, Output, PadOut, Param, Side};
use clamshell_state::levels::Level;
use clamshell_state::settings::Setting;
use clamshell_state::{
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

/// The fixed Thor reference canvas; live outputs are configured independently.
fn size(screen: Screen) -> (u32, u32) {
    let preview = clamshell_state::Profile::thor().preview;
    match screen {
        Screen::Top => preview.top,
        Screen::Bottom => preview.bottom,
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
    let mut display_modes = clamshell_state::device::display::DisplayModes::default();
    for refresh in [60000, 120000] {
        display_modes.add(refresh);
    }
    State::new(Init {
        display_modes,
        profile: clamshell_state::Profile::thor(),
        library,
        dock,
        keymaps: clamshell_state::keymap::Keymaps::initial(),
        ..Init::default()
    })
}

/// A play of `entry` from `start` for `duration` seconds, as the launcher reports one. The clock is set
/// back to `now` afterwards.
fn a_play(entry: &str, start: u64, duration: u64, now: u64) -> Vec<Event> {
    // The game has a window on its workspace while it plays, as a real game would.
    let workspace = clamshell_state::Instance::workspace_of(&clamshell_library::key_of(entry));
    let window = clamshell_state::session::windows::Window {
        id: 900,
        app_id: "game".into(),
        workspace,
        visible: true,
        ..clamshell_state::session::windows::Window::default()
    };
    vec![
        Event::tick(start),
        Event::Units(vec![clamshell_state::Instance::Run { key: clamshell_library::key_of(entry), secs: start, n: 0 }]),
        clamshell_state::WindowEvent::Opened(window).into(),
        Event::tick(start + duration),
        Event::Units(vec![]),
        clamshell_state::WindowEvent::Closed(900).into(),
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
    let control = clamshell_state::Control::Param(param);
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
            ],
        ),
        // A held on the selected entry with the pad opens its actions, with A and B shown at the bottom.
        // Shelf is in front, so the pad is on them. The start sets shelf's rows as a frame would have
        // reported them before the taps.
        group(
            vec![ShelfEvent::Rows(8).into()],
            vec![shot(
                "top-actions.png",
                Top,
                vec![
                    ff6(),
                    Event::Pointer(PointerEvent::Focus(Id::Shelf(ShelfId::Entry("ff6".into())))),
                    Event::Pointer(PointerEvent::LongPress(Id::Shelf(ShelfId::Entry("ff6".into())))),
                ],
            )],
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
                shot("keymap-edit-shared.png", Bottom, vec![tap(Id::Row { list: ListId::Keymaps, i: 3, part: clamshell_state::RowPart::Edit })]),
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
                shot("dock-menu.png", Bottom, vec![demo::running().remove(0), PointerEvent::LongPress(Id::Dock(clamshell_state::DockId::Entry(1))).into()]),
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
                    vec![Reading::Account(clamshell_state::device::account::Account { available: true, password: false, ssh: false }).into()],
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
                    Reading::Time(clamshell_state::TimeZones { ntp: Some(false), ..demo::time_zones() }).into(),
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
    walk_group(state, group, now, &mut |state: &State, _: &[Event], name: &str, screen| {
        let path = dir.join(name);
        let events = clamshell_ui::snapshot(state, screen, size(screen), 1, &path).map_err(|e| Error::Render(path.display().to_string(), e.to_string()))?;
        log::info!("wrote {}", path.display());
        Ok(events)
    })
}

/// What [`walk_group`] shows along the way.
pub trait Visit {
    /// `event` was applied and asked for `effects`.
    fn applied(&mut self, _event: &Event, _effects: &[clamshell_state::Effect]) {}

    /// The state is at the shot `name` of `screen`, after the events in `log` applied to a fresh state.
    /// Returns what the frame reports, which is applied next, as a live frame's report is: shelf's rows
    /// and the lists' rows keep the selections on screen.
    fn shot(&mut self, state: &State, log: &[Event], name: &str, screen: Screen) -> Result<Vec<Event>, Error>;
}

impl<F: FnMut(&State, &[Event], &str, Screen) -> Result<Vec<Event>, Error>> Visit for F {
    fn shot(&mut self, state: &State, log: &[Event], name: &str, screen: Screen) -> Result<Vec<Event>, Error> {
        self(state, log, name, screen)
    }
}

/// The events that set a group up before its own: the clock, the UI's start (as when it comes up), the
/// bar's status and the demo's time zone, so the clock reads the same whatever zone the machine is in.
fn setup(now: u64) -> [Event; 4] {
    [Event::tick(now), Event::Start, Event::Reading(Reading::Status(demo::STATUS)), Event::Reading(Reading::Time(demo::time_zones()))]
}

/// Brings `state` to each of the group's shots in turn and shows `visit` each event applied and each
/// shot.
fn walk_group(state: State, Group { start, shots }: Group, now: u64, visit: &mut impl Visit) -> Result<(), Error> {
    let mut walk = Walk { state, log: Vec::new(), effects: Vec::new() };
    for event in setup(now).into_iter().chain(start) {
        walk.apply(event, visit);
    }
    // Clear the notice the setup left ("Starting …" for its launches) by moving the clock past it and back.
    walk.apply(Event::tick(now + 5), visit);
    walk.apply(Event::tick(now), visit);
    for Shot { name, screen, before } in shots {
        for event in before {
            walk.apply(event, visit);
        }
        for event in visit.shot(&walk.state, &walk.log, name, screen)? {
            walk.apply(event, visit);
        }
    }
    Ok(())
}

/// A group's state on the way, with the events applied to it so far.
struct Walk {
    state: State,
    log: Vec<Event>,
    effects: Vec<clamshell_state::Effect>,
}

impl Walk {
    fn apply(&mut self, event: Event, visit: &mut impl Visit) {
        self.effects.clear();
        self.state.apply(event.clone(), &mut self.effects);
        visit.applied(&event, &self.effects);
        self.log.push(event);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::dev::focus_map::{self, Map, Mapper};
    use crate::dev::trace;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;
    use std::path::PathBuf;

    /// The references: the shots' pixel hashes, one `name hash` line per file; the pad's moves on each
    /// shot; and the effects of the events on the way.
    const PIXELS: &str = "tests/snapshots.txt";
    const FOCUS: &str = "tests/focus.txt";
    const TRACE: &str = "tests/trace.txt";
    const PIXELS_HEADER: &str = "# FNV-1a hashes of the snapshots' sizes and pixels (src/dev/snapshot.rs). After a change meant to change\n\
                                 # what is drawn: CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots";
    const FOCUS_HEADER: &str = "# Where the pad's focus goes on each snapshot (src/dev/focus_map.rs): where the pad starts, what B does,\n\
                                # and for each target the pad may focus on the shot's screen, where ^ up, v down, < left and > right\n\
                                # lead, in sections for the bar, shelf, the keyboard and the rest of the screen. \"-\" stays; [event] is what a move\n\
                                # does that leaves the focus where it is, such as a level's step. Targets written the same next to\n\
                                # each other share a line. \"-- bar as in x.png\": the same as in that shot; \"... except\" then, for\n\
                                # the targets whose moves differ, only the moves that differ. Shelf entries are their quoted ids,\n\
                                # list rows List[i], dock tiles Dock[i], keys Key[row,col].";
    const TRACE_HEADER: &str = "# The effects of the snapshot scenario's events (src/dev/trace.rs), group by group: each event that\n\
                                # had effects, then its effects. \"-- name\" is where a shot is taken.";

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
    /// drawn, and checks each frame against the shot drawn whole with no command cut to its bounds. A
    /// difference means a damaged area or buffer age was wrong, or a command's bounds left out pixels it
    /// draws, such as a glyph's anti-aliased edge.
    #[test]
    fn damaged_frames_match_whole_ones() {
        let groups = shots(FIXED_NOW).unwrap();
        std::thread::scope(|scope| {
            for group in groups {
                scope.spawn(move || {
                    let frames = |screen| (clamshell_ui::Frames::new(screen, size(screen)).unwrap(), clamshell_ui::Frames::new(screen, size(screen)).unwrap());
                    let (mut top, mut bottom) = (frames(Screen::Top), frames(Screen::Bottom));
                    walk_group(state(clamshell_library::demo::library()), group, FIXED_NOW, &mut |state: &State, _: &[Event], name: &str, screen| {
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

    /// One group walked: its shots drawn into the directory, their focus maps, and its trace.
    struct Checked<'a> {
        dir: &'a std::path::Path,
        mapper: Mapper<'a>,
        maps: Vec<Map>,
        trace: String,
    }

    impl Visit for Checked<'_> {
        fn applied(&mut self, event: &Event, effects: &[clamshell_state::Effect]) {
            if effects.is_empty() {
                return;
            }
            let _ = writeln!(self.trace, "{}", trace::event(event));
            for effect in effects {
                let _ = writeln!(self.trace, "  {}", trace::effect(effect));
            }
        }

        fn shot(&mut self, state: &State, log: &[Event], name: &str, screen: Screen) -> Result<Vec<Event>, Error> {
            let _ = writeln!(self.trace, "-- {name}");
            let path = self.dir.join(name);
            let events = clamshell_ui::snapshot(state, screen, size(screen), 1, &path).map_err(|e| Error::Render(name.into(), e.to_string()))?;
            // The pad moves after the frame reported back, as on a live screen.
            self.maps.push(self.mapper.map(state, &[log, &events].concat(), name, screen));
            Ok(events)
        }
    }

    /// Lines of `old` and `new` that differ, `-` for removed and `+` for added, in order.
    fn diff(old: &str, new: &str) -> String {
        let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        // The longest common subsequence's lengths, from the ends.
        let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
            }
        }
        let (mut i, mut j, mut out) = (0, 0, String::new());
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                (i, j) = (i + 1, j + 1);
            } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
                let _ = writeln!(out, "- {}", a[i]);
                i += 1;
            } else {
                let _ = writeln!(out, "+ {}", b[j]);
                j += 1;
            }
        }
        out
    }

    /// The blocks of a reference, by the line each starts with (`== ...`).
    fn blocks(text: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut current: Option<(String, String)> = None;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            if line.starts_with("== ") {
                out.extend(current.take());
                current = Some((line.to_owned(), String::new()));
            }
            if let Some((_, block)) = &mut current {
                block.push_str(line);
                block.push('\n');
            }
        }
        out.extend(current);
        out
    }

    /// The `== ` line of a block.
    fn head(block: &str) -> String {
        block.lines().next().unwrap_or_default().to_owned()
    }

    /// Compares `text`'s blocks with the reference's, by their `== ` lines. Returns the blocks that
    /// differ with their text diffs.
    fn differences(reference: &str, blocks_now: &[String]) -> Vec<(String, String)> {
        let before = blocks(reference);
        let now: BTreeMap<String, &String> = blocks_now.iter().map(|b| (head(b), b)).collect();
        let mut out = Vec::new();
        for b in blocks_now {
            let old = before.get(&head(b)).map_or("", String::as_str);
            if old != b.as_str() {
                out.push((head(b), diff(old, b)));
            }
        }
        for (h, old) in &before {
            if !now.contains_key(h) {
                out.push((h.clone(), diff(old, "")));
            }
        }
        out
    }

    /// Draws the snapshots and compares them with the references: their pixels' hashes, where the pad's
    /// focus goes on each, and the effects of the events on the way. The focus maps are also checked
    /// against rules every screen keeps. On a difference the drawn files are left to look at: the shots,
    /// and for a focus difference the shot with the pad's moves drawn over it. After a change meant to
    /// make a difference, look at these and at the printed diff, then write the new references with
    /// `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
    #[test]
    fn snapshots_match_their_references() {
        let base = std::env::temp_dir().join(format!("clamshell-snapshots-{}", std::process::id()));
        let dir = base.join("shots");
        std::fs::create_dir_all(&dir).unwrap();
        let groups = shots(FIXED_NOW).unwrap();
        let library = clamshell_library::demo::library();
        let fresh = || state(library.clone());
        let walked: Vec<(Vec<Map>, String)> = std::thread::scope(|scope| {
            let threads: Vec<_> = groups
                .into_iter()
                .map(|group| {
                    let (dir, fresh) = (&dir, &fresh);
                    scope.spawn(move || {
                        let first = group.shots.first().map_or("", |s| s.name);
                        let mut checked = Checked { dir, mapper: Mapper::new(fresh, size), maps: Vec::new(), trace: format!("== group {first}\n") };
                        walk_group(fresh(), group, FIXED_NOW, &mut checked).unwrap();
                        (checked.maps, checked.trace)
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        let maps: Vec<&Map> = walked.iter().flat_map(|(maps, _)| maps).collect();
        let traces: Vec<String> = walked.iter().map(|(_, trace)| trace.clone()).collect();

        let mut hashes = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            hashes.insert(entry.file_name().to_string_lossy().into_owned(), pixels_hash(&entry.path()));
        }
        let pixels: String = std::iter::once(format!("{PIXELS_HEADER}\n")).chain(hashes.iter().map(|(name, hash)| format!("{name} {hash}\n"))).collect();
        let focus_blocks = focus_map::blocks(&maps);
        let focus = format!("{FOCUS_HEADER}\n{}", focus_blocks.concat());
        let trace = format!("{TRACE_HEADER}\n{}", traces.concat());

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let failures: Vec<(&Map, &String)> = maps.iter().flat_map(|m| m.failures.iter().map(move |f| (*m, f))).collect();
        let overlay = |m: &Map| {
            let out = dir.join(m.name.replace(".png", ".focus.png"));
            focus_map::overlay(m, &dir.join(&m.name), &out).unwrap();
            out
        };
        let mut report = String::new();
        if !failures.is_empty() {
            let _ = writeln!(report, "the pad's focus breaks a rule:");
            for (m, f) in &failures {
                let _ = writeln!(report, "  {}: {f}", m.name);
            }
            let names: BTreeSet<&str> = failures.iter().map(|(m, _)| m.name.as_str()).collect();
            for m in maps.iter().filter(|m| names.contains(m.name.as_str())) {
                let _ = writeln!(report, "  {}", overlay(m).display());
            }
        }
        if std::env::var_os("CLAMSHELL_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(root.join(PIXELS), pixels).unwrap();
            std::fs::write(root.join(FOCUS), focus).unwrap();
            std::fs::write(root.join(TRACE), trace).unwrap();
        } else {
            let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap_or_default();
            let expected: BTreeMap<String, String> =
                read(PIXELS).lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(' ').map(|(n, h)| (n.to_owned(), h.to_owned()))).collect();
            let changed: BTreeSet<&String> = hashes.keys().chain(expected.keys()).filter(|n| hashes.get(*n) != expected.get(*n)).collect();
            if !changed.is_empty() {
                let _ = writeln!(report, "the pixels changed ({PIXELS}):");
                for name in changed {
                    let _ = writeln!(report, "  {}", dir.join(name).display());
                }
            }
            let focus_diff = differences(&read(FOCUS), &focus_blocks);
            if !focus_diff.is_empty() {
                let _ = writeln!(report, "the pad's focus changed ({FOCUS}):");
                for (h, d) in &focus_diff {
                    let image = maps.iter().find(|m| h.starts_with(&format!("== {} ", m.name))).map(|m| overlay(m).display().to_string());
                    let _ = writeln!(report, "{h}  {}\n{d}", image.unwrap_or_default());
                }
            }
            let trace_diff = differences(&read(TRACE), &traces);
            if !trace_diff.is_empty() {
                let _ = writeln!(report, "the effects changed ({TRACE}):");
                for (h, d) in &trace_diff {
                    let _ = writeln!(report, "{h}\n{d}");
                }
            }
            if !report.is_empty() && failures.is_empty() {
                let _ = writeln!(
                    report,
                    "Look at the images and the diffs. If every difference follows from the intended change, update the references with \
                     CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots; otherwise it is a bug."
                );
            }
        }
        if report.is_empty() {
            let _ = std::fs::remove_dir_all(&base);
        }
        assert!(report.is_empty(), "drawn into {}\n{report}", dir.display());
    }
}
