//! `--snapshot DIR`: both screens drawn at the Thor's size into PNG files, without Wayland: shelf, the
//! second screen and every tab, each after the events that lead to it, in groups that each start fresh. At a fixed time (unless
//! CLAMSHELL_SNAPSHOT_NOW is set), so the files compare byte for byte across runs; the test below keeps
//! their hashes (tests/snapshots.txt) to catch changes in what is drawn.

use std::fmt;
use std::path::Path;

use clamshell_core::keyboard::{Key, Modifier, Named};
use clamshell_core::keymap::{Actuation, Button, Dir, Input, Layer, MouseButton, PadOut, Param, Side};
use clamshell_core::levels::Level;
use clamshell_core::settings::Setting;
use clamshell_core::slider::Control;
use clamshell_core::{
    Dock, EditorId, Event, GameEvent, Id, Init, KeymapId, LevelEvent, Library, ListEvent, ListId, PointerEvent, Reading, Scope, SettingsId, ShelfEvent,
    ShelfId, Slide, State, Status, Tab, Target,
};
use clamshell_ui::Screen;

use crate::control::{self, ParseError};
use crate::demo;

/// The time snapshots are taken at, unless asked for the current one.
pub const FIXED_NOW: u64 = 1_760_000_000;

/// One file: the screen drawn, after these events.
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
    Render(String, clamshell_ui::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Keyboard(e) => write!(f, "the scenario's keys: {e}"),
            Error::Render(file, e) => write!(f, "{file}: {e}"),
        }
    }
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Keyboard(e)
    }
}

/// The state snapshots start from: a library, its apps pinned to the dock, nothing else remembered.
pub fn state(library: Library) -> State {
    let dock = Dock::pinned(vec!["desktop:steam".into(), "desktop:foot".into(), "desktop:firefox".into()]);
    State::new(Init { library, dock, keymaps: clamshell_core::keymap::Keymaps::initial(), ..Init::default() })
}

fn tap(id: Id) -> Event {
    PointerEvent::Activate(id).into()
}

fn key(k: Key) -> Result<Event, ParseError> {
    control::key(k).map(tap)
}

/// A button of the keymaps' screens.
fn km(id: KeymapId) -> Event {
    tap(Id::Keymap(id))
}

/// An input on the keymap edit screen selected (its panel's ways then remap it).
fn select(input: Input) -> Vec<Event> {
    vec![km(KeymapId::Input(input))]
}

/// The selected input's parameter slid by `by` of half the Thor's second screen, the finger lifted if
/// `lift`.
fn slide(param: Param, by: f32, lift: bool) -> Vec<Event> {
    let control = clamshell_core::slider::Control::Param(param);
    // (Along the screen-wide track, as touch gives it: a finger down in the panel, mid-screen.)
    let (x, width) = (620.0, 1240.0);
    let mut events = vec![Slide::Start { control, x, width }.into(), Slide::To { x: x + by * width * 0.5 }.into()];
    if lift {
        events.push(Slide::End.into());
    }
    events
}

/// The settings' keymap tile: the picker.
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

/// A group of shots: from a fresh state, `start`'s events (a named starting state, see below), then each
/// shot after the events that lead to it, building on the group's earlier ones. Groups don't share
/// state, so a change shows in the shots of the groups it touches.
pub struct Group {
    pub start: Vec<Event>,
    pub shots: Vec<Shot>,
}

/// A keymap named FPS made on the settings' keymap screens, on its edit screen with A selected: A to
/// Ctrl+X, L1 to right click (held Esc, twice Tab), the left stick to WASD, the right stick's up to the
/// wheel, the gyro to the pointer while L2 is held, Select to M, the D-pad's left to 1, B to the pad's
/// Y, R1 with turbo, the right stick's deadzone tuned.
fn fps_keymap() -> Result<Vec<Event>, ParseError> {
    Ok([
        vec![tab(Tab::Settings), keymap_tile(), km(KeymapId::New)],
        control::type_text("FPS")?,
        vec![tap(Id::Editor(EditorId::Save))],
        select(Input::Button(Button::A)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Mod(Modifier::Ctrl))?, key(Key::Char('x'))?],
        select(Input::Button(Button::LB)),
        vec![km(KeymapId::Target(Target::Mouse(MouseButton::Right)))],
        select(Input::Stick(Side::Left)),
        vec![km(KeymapId::Target(Target::Keys)), key(Key::Char('w'))?, key(Key::Char('a'))?, key(Key::Char('s'))?, key(Key::Char('d'))?],
        select(Input::StickDir(Side::Right, Dir::Up)),
        vec![km(KeymapId::Target(Target::Wheel(Dir::Up)))],
        select(Input::Gyro),
        vec![km(KeymapId::Target(Target::Pointer)), km(KeymapId::Holds), km(KeymapId::Hold(Some(Input::Trigger(Side::Left))))],
        select(Input::Button(Button::View)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Char('m'))?],
        select(Input::Dpad(Dir::Left)),
        vec![km(KeymapId::Target(Target::Key)), key(Key::Char('1'))?],
        select(Input::Button(Button::B)),
        vec![km(KeymapId::Target(Target::Pad(PadOut::Button(Button::Y))))],
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

/// [`fps_keymap`] with a layer named aim (A in it to Y), shown, L1 selected (as in Base there).
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
    let ff6 = || tap(Id::Shelf(ShelfId::Game("ff6".into())));
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
                    vec![
                        GameEvent::Played { game: "zelda-lttp".into(), start: 1_759_900_000, duration: 4 * 3600 + 1260 }.into(),
                        ShelfEvent::ToggleFavourite.into(),
                        ShelfEvent::TogglePin.into(),
                        GameEvent::Started("zelda-lttp".into()).into(),
                    ],
                ),
                shot("top-switch.png", Top, vec![ff6(), ff6()]),
            ],
        ),
        // A series selected in shelf, then open: its first season's episodes, the first played. (Shelf's rows as a frame
        // would have reported them before the taps.)
        group(
            vec![ShelfEvent::Rows(8).into()],
            vec![
                shot("top-folder.png", Top, vec![tap(Id::Shelf(ShelfId::Collection(2)))]),
                shot(
                    "top-group.png",
                    Top,
                    vec![
                        tap(Id::Shelf(ShelfId::Launch)),
                        tap(Id::Shelf(ShelfId::Launch)),
                        GameEvent::Played { game: "tv:Cosmos/Season 1/1. The Shores of the Cosmic Ocean.mkv".into(), start: 1_759_900_000, duration: 3600 }
                            .into(),
                        ShelfEvent::ListStep(2).into(),
                    ],
                ),
            ],
        ),
        group(vec![], vec![shot("bottom.png", Bottom, vec![])]),
        // The settings, a game running.
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
                    vec![GameEvent::Played { game: "zelda-lttp".into(), start: now, duration: 60 }.into(), tap(Id::Settings(SettingsId::Scope(Scope::Game)))],
                ),
                shot(
                    "settings-env.png",
                    Bottom,
                    [tap(Id::Settings(SettingsId::Env))].into_iter().chain(control::type_text("dxvk_hud=fps mesa_vk_wsi_present_mode=fifo")?).collect(),
                ),
                shot("status-low.png", Bottom, vec![tap(Id::Editor(EditorId::Save)), Reading::Status(demo::STATUS_LOW).into()]),
            ],
        ),
        // A new keymap named.
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
                        km(KeymapId::Target(Target::Mouse(MouseButton::Left))),
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
        // Its ways to remap and the gyro (Back to the layer, as a layer key).
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
        // A game's own keymap: made from FPS (chosen for it), its menu, kept while FPS is chosen again; FPS's
        // edit screen, used by games.
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
                shot("dock-menu.png", Bottom, vec![demo::running().remove(2), PointerEvent::LongPress(Id::Dock(clamshell_core::DockId::Entry(1))).into()]),
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
                shot("time-europe.png", Bottom, vec![PointerEvent::Activate(ListId::Time.row(7)).into()]),
            ],
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
                        PointerEvent::Down(volume.clone()).into(),
                        Slide::Start { control: Control::Volume, x: 560.0, width: 1232.0 }.into(),
                        Slide::To { x: 700.0 }.into(),
                    ],
                ),
                shot("keyboard.png", Bottom, vec![Slide::End.into(), PointerEvent::Up(volume).into(), tab(Tab::Keyboard)]),
                shot("keyboard-fn.png", Bottom, vec![key(Key::Mod(Modifier::Fn))?]),
                shot("keyboard-mods.png", Bottom, vec![key(Key::Mod(Modifier::Fn))?, key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(ctrl()?).into()]),
                shot("keyboard-caps.png", Bottom, vec![PointerEvent::Up(ctrl()?).into(), key(Key::Mod(Modifier::Shift))?, key(Key::Caps)?]),
                shot("touchpad.png", Bottom, vec![tab(Tab::Touchpad), tap(Id::PadMouse)]),
                shot("sysinfo.png", Bottom, demo::metrics()),
                shot("keyboard-back.png", Bottom, vec![tab(Tab::Keyboard)]),
                shot("keyboard-pressed.png", Bottom, vec![key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(control::key(Key::Char('g'))?).into()]),
            ],
        ),
    ])
}

/// Draws the scenario into `dir`, each group from a fresh state (`fresh`), each file written as its
/// shot comes.
pub fn render(fresh: &dyn Fn() -> State, now: u64, dir: &Path) -> Result<(), Error> {
    for Group { start, shots } in shots(now)? {
        let mut state = fresh();
        state.apply(Event::Tick { unix_secs: now });
        state.apply(Event::Reading(Reading::Status(demo::STATUS)));
        for event in start {
            state.apply(event);
        }
        for Shot { name, screen, before } in shots {
            for event in before {
                state.apply(event);
            }
            let path = dir.join(name);
            // What a live frame reports (shelf's rows, the lists' rows) keeps the selections on screen.
            let events = clamshell_ui::snapshot(&state, screen, size(screen), 1, &path).map_err(|e| Error::Render(path.display().to_string(), e))?;
            for event in events {
                state.apply(event);
            }
            log::info!("wrote {}", path.display());
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
    const HEADER: &str = "# FNV-1a hashes of the `clamshell --snapshot` files (src/snapshot.rs). After a change meant to change\n\
                          # what is drawn: CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots\n";

    /// FNV-1a, 64 bits: enough to tell files apart, and no dependency.
    fn hash(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
    }

    /// The snapshots, drawn and hashed, against the reference hashes. After a change meant to change what
    /// is drawn (look at the files: `clamshell --snapshot DIR`), write the new hashes with
    /// `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
    #[test]
    fn snapshots_match_their_reference() {
        let base = std::env::temp_dir().join(format!("clamshell-snapshots-{}", std::process::id()));
        let dir = base.join("shots");
        std::fs::create_dir_all(&dir).unwrap();
        render(&|| state(clamshell_library::demo_library()), FIXED_NOW, &dir).unwrap();
        let mut hashes = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            hashes.insert(entry.file_name().to_string_lossy().into_owned(), format!("{:016x}", hash(&std::fs::read(entry.path()).unwrap())));
        }
        let _ = std::fs::remove_dir_all(&base);
        let text: String = std::iter::once(HEADER.to_owned()).chain(hashes.iter().map(|(name, hash)| format!("{name} {hash}\n"))).collect();
        let reference = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(REFERENCE);
        if std::env::var_os("CLAMSHELL_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&reference, text).unwrap();
            return;
        }
        let expected: BTreeMap<String, String> = std::fs::read_to_string(&reference)
            .expect("tests/snapshots.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once(' ').map(|(n, h)| (n.to_owned(), h.to_owned())))
            .collect();
        let changed: std::collections::BTreeSet<&String> = hashes.keys().chain(expected.keys()).filter(|n| hashes.get(*n) != expected.get(*n)).collect();
        assert!(changed.is_empty(), "snapshots changed: {changed:?}; if meant, update with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`");
    }
}
