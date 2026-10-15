//! `--snapshot DIR`: both screens drawn at the Thor's size into PNG files, without Wayland: shelf, the
//! second screen and every tab, each after the events that lead to it. At a fixed time (unless
//! CLAMSHELL_SNAPSHOT_NOW is set), so the files compare byte for byte across runs; the test below keeps
//! their hashes (tests/snapshots.txt) to catch changes in what is drawn.

use std::fmt;
use std::path::Path;

use clamshell_core::keyboard::{Key, Modifier};
use clamshell_core::keymap::{Button, Dir, Input, MouseButton, Side};
use clamshell_core::levels::Level;
use clamshell_core::settings::Setting;
use clamshell_core::slider::Control;
use clamshell_core::{
    Apps, EditorId, Event, GameEvent, Id, Init, KeymapId, Library, LevelEvent, ListEvent, ListId, PointerEvent, Reading, Scope, SettingsId, ShelfEvent, ShelfId, Slide, State, Status, Tab, Target,
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

/// The state snapshots start from: a library, the example launcher.json's apps (their desktop files
/// from this machine's data dirs), nothing remembered.
pub fn state(library: Library) -> State {
    state_with(library, clamshell_policy::desktop::from_text(crate::LAUNCHER_EXAMPLE))
}

fn state_with(library: Library, apps: Apps) -> State {
    State::new(Init { library, apps, keymaps: clamshell_core::keymap::Keymaps::initial(), ..Init::default() })
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

/// The scenario, in order: each shot after the events that lead to it (they build on each other).
pub fn shots(now: u64) -> Result<Vec<Shot>, ParseError> {
    use Screen::{Bottom, Top};
    let shot = |name, screen, before| Shot { name, screen, before };
    let ff6 = || tap(Id::Shelf(ShelfId::Game("ff6".into())));
    let ctrl = || control::key(Key::Mod(Modifier::Ctrl));
    let volume = Id::Control(Control::Volume);
    Ok(vec![
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
                GameEvent::Started("zelda-lttp".into()).into(),
            ],
        ),
        shot("top-switch.png", Top, vec![ff6(), ff6()]),
        shot("bottom.png", Bottom, vec![]),
        shot(
            "settings-running.png",
            Bottom,
            [tab(Tab::Settings)]
                .into_iter()
                .chain(taps(Setting::FpsCap, 4))
                .chain([tap(Id::Settings(SettingsId::Tile(Setting::Keymap))), tap(ListId::Keymaps.row(1))])
                .chain(taps(Setting::Hud, 1))
                .collect(),
        ),
        shot(
            "settings-global.png",
            Bottom,
            [tap(Id::Settings(SettingsId::Scope(Scope::Global)))]
                .into_iter()
                .chain(taps(Setting::Power, 1))
                .chain(taps(Setting::Deadzone, 2))
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
        shot("keymap-name.png", Bottom, [keymap_tile(), km(KeymapId::New)].into_iter().chain(control::type_text("FPS")?).collect()),
        shot(
            "keymap-edit.png",
            Bottom,
            vec![
                tap(Id::Editor(EditorId::Save)),
                km(KeymapId::Input(Input::Button(Button::A))),
                km(KeymapId::Target(Target::Key)),
                key(Key::Mod(Modifier::Ctrl))?,
                key(Key::Char('x'))?,
                km(KeymapId::Input(Input::Button(Button::LB))),
                km(KeymapId::Target(Target::Mouse(MouseButton::Right))),
                km(KeymapId::Input(Input::Stick(Side::Left))),
                km(KeymapId::Target(Target::Keys)),
                key(Key::Char('w'))?,
                key(Key::Char('a'))?,
                key(Key::Char('s'))?,
                key(Key::Char('d'))?,
                km(KeymapId::Input(Input::StickDir(Side::Right, Dir::Up))),
                km(KeymapId::Target(Target::Wheel(Dir::Up))),
                km(KeymapId::Input(Input::Gyro)),
                km(KeymapId::Target(Target::Pointer)),
                km(KeymapId::Hold(Some(Input::Trigger(Side::Left)))),
                km(KeymapId::Back),
            ],
        ),
        shot("keymap-map-to.png", Bottom, vec![km(KeymapId::Input(Input::Button(Button::A)))]),
        shot("keymap-map-to-gyro.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Input(Input::Gyro))]),
        shot(
            "keymap-map-to-stick.png",
            Bottom,
            vec![km(KeymapId::Back), km(KeymapId::Input(Input::Stick(Side::Right))), km(KeymapId::Target(Target::Pointer)), km(KeymapId::Speed(4))],
        ),
        shot("keymap-capture.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Input(Input::Button(Button::Y))), km(KeymapId::Target(Target::Key)), key(Key::Mod(Modifier::Alt))?]),
        shot("keymap-press.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Back), km(KeymapId::Input(Input::Button(Button::B))), km(KeymapId::Target(Target::PressPad))]),
        shot("keymaps.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Back), km(KeymapId::Back)]),
        shot("keymap-menu.png", Bottom, vec![PointerEvent::LongPress(ListId::Keymaps.row(2)).into()]),
        shot("keymap-delete.png", Bottom, vec![km(KeymapId::Back), tap(ListId::Keymaps.row(2)), keymap_tile(), PointerEvent::LongPress(ListId::Keymaps.row(2)).into(), km(KeymapId::Delete)]),
        shot("bottom.png", Bottom, vec![km(KeymapId::Back), km(KeymapId::Back), Reading::Status(demo::STATUS).into()]),
        shot("launcher.png", Bottom, vec![tab(Tab::Dock), PointerEvent::HideFocus.into()]),
        shot("launcher-running.png", Bottom, demo::running()),
        shot("wifi.png", Bottom, vec![tab(Tab::Wifi), Reading::Wifi(demo::wifi()).into()]),
        shot("wifi-crowded.png", Bottom, vec![Reading::Wifi(demo::wifi_crowded()).into()]),
        shot("wifi-scrolled.png", Bottom, vec![ListEvent::Scroll(ListId::Wifi, 3.4).into()]),
        shot("battery.png", Bottom, demo::battery(now)),
        shot("bluetooth.png", Bottom, vec![tab(Tab::Bluetooth), Reading::Bluetooth(demo::bluetooth()).into()]),
        shot(
            "volume.png",
            Bottom,
            vec![
                tab(Tab::Volume),
                Reading::Status(Status { volume: Some(64), brightness: Some(70), battery: Some(76), wifi: Some(80), ..Status::default() }).into(),
                Reading::Audio(demo::audio()).into(),
            ],
        ),
        shot(
            "brightness.png",
            Bottom,
            vec![
                tab(Tab::Brightness),
                Reading::Backlights(vec!["dsi0-backlight".into(), "dsi1-backlight".into()]).into(),
                LevelEvent::Set(Level::Backlight(1), 65).into(),
                PointerEvent::Up(Level::Backlight(1).id()).into(),
            ],
        ),
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
        shot(
            "keyboard-mods.png",
            Bottom,
            vec![key(Key::Mod(Modifier::Fn))?, key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(ctrl()?).into()],
        ),
        shot("keyboard-caps.png", Bottom, vec![PointerEvent::Up(ctrl()?).into(), key(Key::Mod(Modifier::Shift))?, key(Key::Caps)?]),
        shot("touchpad.png", Bottom, vec![tab(Tab::Touchpad), tap(Id::PadMouse)]),
        shot("sysinfo.png", Bottom, demo::metrics()),
        shot("keyboard-back.png", Bottom, vec![tab(Tab::Keyboard)]),
        shot("keyboard-pressed.png", Bottom, vec![key(Key::Mod(Modifier::Shift))?, PointerEvent::Down(control::key(Key::Char('g'))?).into()]),
    ])
}

/// Draws the scenario into `dir` (each file written as its shot comes; a later shot of the same name
/// replaces it).
pub fn render(state: &mut State, now: u64, dir: &Path) -> Result<(), Error> {
    state.apply(Event::Tick { unix_secs: now });
    state.apply(Event::Reading(Reading::Status(demo::STATUS)));
    for Shot { name, screen, before } in shots(now)? {
        for event in before {
            state.apply(event);
        }
        let path = dir.join(name);
        // What a live frame reports (shelf's rows, the lists' rows) keeps the selections on screen.
        let events = clamshell_ui::snapshot(state, screen, size(screen), 1, &path).map_err(|e| Error::Render(path.display().to_string(), e))?;
        for event in events {
            state.apply(event);
        }
        log::info!("wrote {}", path.display());
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

    /// The example apps with desktop files of their own (no icons), so what is drawn doesn't depend on
    /// what this machine has installed.
    fn apps(dir: &Path) -> Apps {
        let apps = dir.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        for (id, name) in [("steam", "Steam"), ("firefox", "Firefox")] {
            std::fs::write(apps.join(format!("{id}.desktop")), format!("[Desktop Entry]\nName={name}\nExec={id}\n")).unwrap();
        }
        clamshell_policy::desktop::from_text_in(crate::LAUNCHER_EXAMPLE, &[dir.to_path_buf()])
    }

    /// The snapshots, drawn and hashed, against the reference hashes. After a change meant to change what
    /// is drawn (look at the files: `clamshell --snapshot DIR`), write the new hashes with
    /// `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`.
    #[test]
    fn snapshots_match_their_reference() {
        let base = std::env::temp_dir().join(format!("clamshell-snapshots-{}", std::process::id()));
        let dir = base.join("shots");
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = state_with(clamshell_library::demo_library(), apps(&base.join("data")));
        render(&mut state, FIXED_NOW, &dir).unwrap();
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
        assert!(
            changed.is_empty(),
            "snapshots changed: {changed:?}; if meant, update with `CLAMSHELL_UPDATE_SNAPSHOTS=1 cargo test -p clamshell snapshots`"
        );
    }
}
