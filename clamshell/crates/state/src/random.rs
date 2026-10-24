//! The randomized event-sequence test. Seeded runs of a few thousand events each, drawn to be valid for
//! the state as it is (ids that exist, taps on what is shown, results only after their requests), with a
//! small model of the outside world (the compositor's windows, systemd's scopes, the system threads'
//! answers) that follows the effects. After every event the invariants of `docs/state-machines.md`
//! ("Invariants") are checked; a failure prints the seed, the event's number and the events before it.
//!
//! The fixed seeds run with the other tests. More seeds, locally:
//!
//! ```text
//! RANDOM_SEEDS=2000 cargo test -p clamshell-state --release random
//! RANDOM_SEED=1234 RANDOM_EVENTS=20000 cargo test -p clamshell-state random
//! ```
//!
//! `RANDOM_SEEDS=n` runs seeds 1 to n, `RANDOM_SEED=x` that seed alone, and `RANDOM_EVENTS` sets the events
//! per seed.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bottom::keyboard::{KeyPos, slot};
use crate::device::buttons::DeviceKey;
use crate::device::idle::{IdleEvent, Stage};
use crate::device::{SticksEvent, TopView};
use crate::keymap::{Button, Input};
use crate::levels::{Audio, AudioOutput, Stream};
use crate::out::Out;
use crate::radios::bluetooth::{Bluetooth, Device as BtDevice, Kind};
use crate::radios::wifi::{Network, Wifi};
use crate::session::windows::{SHELF_APP_ID, Window};
use crate::settings::Setting;
use crate::settings::panel::Scope;
use crate::*;

/// The seeds that run with the other tests, and the events in each.
const SEEDS: [u64; 8] = [1, 2, 3, 4, 5, 0x5eed, 0xc1a5, 0xdead_beef];
const EVENTS: usize = 4000;

/// SplitMix64: small, seeded, good enough to draw events.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n` (0 for 0).
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    /// True one time in `n`.
    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }
}

/// The library: the demo's, with a Steam game (ff6), an app handed over to another program
/// (desktop:firefox), an enqueue runner's album, a browsed file, the series (a group) and the Device tool.
/// Any running program may open more than one window.
fn library() -> Library {
    let mut library = clamshell_library::demo::library();
    for g in &mut library.entries {
        match &*g.id {
            "ff6" => g.launch = Launch::Steam { appid: 620 },
            "desktop:firefox" => g.runner = Some(RunnerOf { handover: true, ..RunnerOf::new("android", "Android", true) }),
            _ => {}
        }
    }
    let music = RunnerOf {
        command: vec!["mpv".into(), "--input-ipc-server={socket}".into(), "{paths}".into()].into(),
        enqueue: Some(r#"{"command": ["loadfile", "{path}", "append-play"]}"#.into()),
        ..RunnerOf::new("music", "Music", true)
    };
    let n = library.entries.len();
    let song = |id: &str, file: &str, parent| Entry { file: Some(file.into()), runner: Some(music.clone()), parent, ..Entry::new(id, id) };
    library.entries.push(Entry { items: vec![n + 1, n + 2], runner: Some(music.clone()), ..Entry::new("album", "Album") });
    library.entries.push(song("song-a", "/m/Album/a.mp3", Some(n)));
    library.entries.push(song("song-b", "/m/Album/b.mp3", Some(n)));
    library.entries.push(song("single", "/m/single.mp3", None));
    library.entries.push(Entry {
        file: Some("/h/notes.md".into()),
        launch: Launch::program(vec!["xdg-open".into(), "/h/notes.md".into()]),
        browsed: Some(Browsed::default()),
        ..Entry::new("browse:Home/notes.md", "/h/notes.md")
    });
    library.collections.push(Collection::new("Music", vec![n, n + 3]));
    library.collections.push(Collection::new("Files", vec![n + 4]));
    library
}

/// The outside world as far as the state can tell: what the compositor, systemd and the system threads
/// would report, following the effects.
#[derive(Default)]
struct World {
    windows: Vec<Window>,
    reported_windows: Vec<Window>,
    next_window: i64,
    units: Vec<Instance>,
    /// Launches asked for and not answered.
    launches: Vec<(Arc<str>, Instance)>,
    /// Keys asked to end.
    ending: Vec<String>,
    /// Windows asked to close.
    closing: Vec<i64>,
    sticks: bool,
    gyro: bool,
    refresh: bool,
    account: bool,
    screenshot: bool,
    asleep: bool,
    /// An event drawn to follow the one drawn now, such as a touch's press after HideFocus.
    next: Option<Event>,
}

impl World {
    fn new() -> World {
        let shelf = Window { id: 1, app_id: SHELF_APP_ID.into(), focused: true, visible: true, workspace: Instance::Shelf.name(), ..Window::default() };
        World { windows: vec![shelf], next_window: 2, ..World::default() }
    }

    fn focus(&mut self, pick: impl Fn(&Window) -> bool) {
        let Some(i) = self.windows.iter().position(|w| pick(w) && !w.down) else {
            return;
        };
        let ws = self.windows.get(i).map(|w| w.workspace.clone()).unwrap_or_default();
        for (j, w) in self.windows.iter_mut().enumerate() {
            w.focused = i == j;
            if !w.down {
                w.visible = w.workspace == ws;
            }
        }
    }

    /// What the effects ask of the outside.
    fn follow(&mut self, effects: &[Effect]) {
        for e in effects {
            match e {
                Effect::Launch { entry, instance, .. } => self.launches.push((entry.clone(), instance.clone())),
                Effect::EndRun { key, .. } => self.ending.push(key.clone()),
                Effect::Window(WindowCommand::Close(ids)) => self.closing.extend(ids),
                Effect::Window(WindowCommand::FocusWindow(id)) => self.focus(|w| w.id == *id),
                Effect::Window(WindowCommand::ShowShelf) => self.focus(|w| w.app_id == SHELF_APP_ID),
                Effect::Window(WindowCommand::Move { windows, to }) => {
                    for w in self.windows.iter_mut().filter(|w| windows.contains(&w.id)) {
                        w.workspace = to.clone();
                    }
                }
                Effect::Window(WindowCommand::Down(id)) => {
                    for w in self.windows.iter_mut().filter(|w| w.id == *id) {
                        (w.down, w.focused) = (true, false);
                    }
                }
                Effect::Window(WindowCommand::Up { window, .. }) => {
                    for w in self.windows.iter_mut().filter(|w| w.id == *window) {
                        w.down = false;
                    }
                }
                Effect::Window(WindowCommand::Return { window, to }) => {
                    for w in self.windows.iter_mut().filter(|w| w.id == *window) {
                        w.down = false;
                        w.visible = false;
                        w.focused = false;
                        w.workspace.clone_from(to);
                    }
                }
                Effect::CalibrateSticks => self.sticks = true,
                Effect::CalibrateGyro => self.gyro = true,
                Effect::CalibrateRefresh => self.refresh = true,
                Effect::Account(_) => self.account = true,
                Effect::Screenshot => self.screenshot = true,
                Effect::Sleep => self.asleep = true,
                _ => {}
            }
        }
    }

    fn report(&mut self) -> Event {
        let event = crate::session::windows::changes(&self.reported_windows, &self.windows);
        self.reported_windows.clone_from(&self.windows);
        event
    }

    fn window(&mut self, app_id: &str, workspace: String, unit: Option<String>, focused: bool) -> Event {
        self.open(app_id, workspace, unit, focused);
        self.report()
    }

    fn open(&mut self, app_id: &str, workspace: String, unit: Option<String>, focused: bool) -> Window {
        let id = self.next_window;
        self.next_window += 1;
        let window = Window { id, app_id: app_id.into(), workspace, unit, pids: vec![1000 + id as u32], ..Window::default() };
        self.windows.push(window.clone());
        if focused {
            self.focus(|w| w.id == id);
        }
        self.windows.iter().find(|w| w.id == id).cloned().unwrap_or(window)
    }
}

/// What the effects so far say the outside has, for invariants 2 and 11.
#[derive(Default)]
struct Sent {
    route: Option<PadRoute>,
    pad_activity: bool,
    keymap: Option<crate::keymap::Keymap>,
    apply: Option<(Option<Arc<str>>, crate::settings::Values)>,
    status: bool,
    account: bool,
    /// The last Outputs.
    outputs: (bool, bool),
    /// The last Refresh, and the refresh rate of the last Launch or Apply, which it follows.
    refresh: u16,
    refresh_wanted: u16,
    /// Each slice's last Freeze.
    frozen: HashMap<String, bool>,
    /// Keys ended (not forced) since their last quit or launch.
    ended: Vec<String>,
    /// The focus ring was shown by a hold on shelf's entry, with the pad elsewhere.
    ring_by_hold: bool,
}

/// The event drawn next, valid for `s` as it is, and the world changed to match.
fn draw(rng: &mut Rng, s: &State, w: &mut World, clock: &mut u64) -> Event {
    if let Some(e) = w.next.take() {
        return e;
    }
    loop {
        if let Some(e) = draw_once(rng, s, w, clock) {
            return e;
        }
    }
}

const TABS: [Tab; 11] =
    [Tab::Settings, Tab::Dock, Tab::Keyboard, Tab::Touchpad, Tab::Sysinfo, Tab::Volume, Tab::Brightness, Tab::Wifi, Tab::Bluetooth, Tab::Battery, Tab::Time];

/// The bar's targets, always shown.
fn bar() -> Vec<Id> {
    let mut ids = vec![Id::PadMouse, Id::Blank, Id::Control(Control::Volume), Id::Control(Control::Brightness), Id::BottomWindows];
    ids.extend(TABS.map(Id::Tab));
    ids
}

/// The targets shown now on the main screen and in the tab, roughly as the UI would draw them.
fn targets(rng: &mut Rng, s: &State) -> Vec<Id> {
    let mut ids = Vec::new();
    if s.bottom_dark() {
        ids.push(Id::Wake);
    }
    match s.device().top_view() {
        TopView::Device => ids.extend(
            [
                DeviceId::Wifi,
                DeviceId::Zone,
                DeviceId::Password,
                DeviceId::Ssh,
                DeviceId::Sticks,
                DeviceId::Gyro,
                DeviceId::Refresh,
                DeviceId::Done,
                DeviceId::Close,
            ]
            .map(Id::Device),
        ),
        TopView::Shelf if s.session().shelf_in_front() => {
            let sh = s.shelf();
            ids.extend(sh.list().iter().filter_map(|&i| sh.library().entries.get(i)).map(|g| Id::Shelf(ShelfId::Entry(g.id.clone()))));
            ids.extend((0..sh.library().collections.len() as u16).map(|i| Id::Shelf(ShelfId::Collection(i))));
            ids.extend([ShelfId::Launch, ShelfId::Up, ShelfId::Favourite, ShelfId::Pin, ShelfId::Continue, ShelfId::Enqueue].map(Id::Shelf));
        }
        TopView::Shelf => {}
    }
    if s.bottom().editor().is_some() {
        ids.extend([EditorId::Save, EditorId::Cancel].map(Id::Editor));
    }
    let row = |list: ListId, n: usize| (0..n.min(8) as u16).map(move |i| Id::Row { list, i, part: RowPart::Main });
    match s.bottom().tab() {
        Tab::Dock => {
            ids.extend((0..s.dock().items().len() as u16).flat_map(|i| [Id::Dock(DockId::Entry(i)), Id::Dock(DockId::Tray(i))]));
            if s.dock().menu().is_some() {
                ids.extend(
                    [
                        DockId::OpenAnother,
                        DockId::CloseWindow,
                        DockId::Pin,
                        DockId::Move,
                        DockId::Quit,
                        DockId::ForceKill,
                        DockId::Back,
                        DockId::Autostart,
                        DockId::Suspend,
                        DockId::TrayMenu,
                        DockId::TrayEntry(1),
                        DockId::TrayEntry(2),
                    ]
                    .map(Id::Dock),
                );
            }
            if s.dock().moving().is_some() {
                ids.extend([DockId::Step(crate::keymap::Dir::Left), DockId::Step(crate::keymap::Dir::Right), DockId::Done].map(Id::Dock));
            }
        }
        Tab::Keyboard => {
            for _ in 0..6 {
                let pos = KeyPos { row: rng.below(6) as u8, col: rng.below(14) as u8 };
                if slot(pos).is_some() {
                    ids.push(Id::Key(pos));
                }
            }
        }
        Tab::Settings => match s.keymapping().screen() {
            None => {
                ids.extend(Setting::ALL.map(|t| Id::Settings(SettingsId::Tile(t))));
                ids.extend([Scope::Global, Scope::Runner, Scope::Content].map(|c| Id::Settings(SettingsId::Scope(c))));
                ids.extend([SettingsId::Reset, SettingsId::Env].map(Id::Settings));
            }
            Some(_) => {
                ids.extend(
                    [
                        KeymapId::Back,
                        KeymapId::New,
                        KeymapId::Edit,
                        KeymapId::Rename,
                        KeymapId::Duplicate,
                        KeymapId::Delete,
                        KeymapId::ConfirmDelete,
                        KeymapId::MakeOwn,
                        KeymapId::Undo,
                        KeymapId::LayerPicker,
                        KeymapId::NewLayer,
                        KeymapId::Done,
                        KeymapId::Input(Input::Button(Button::A)),
                        KeymapId::Input(Input::Gyro),
                        KeymapId::Target(Target::Pick),
                        KeymapId::Target(Target::Key),
                        KeymapId::Target(Target::Default),
                        KeymapId::Calibrate,
                    ]
                    .map(Id::Keymap),
                );
                ids.extend(row(ListId::Keymaps, s.lists().len(ListId::Keymaps)));
                ids.extend(row(ListId::Layers, s.lists().len(ListId::Layers)));
            }
        },
        Tab::Wifi => {
            ids.push(Id::Wifi(WifiId::Power));
            ids.extend(row(ListId::Wifi, s.lists().len(ListId::Wifi)));
        }
        Tab::Bluetooth => {
            ids.push(Id::Bluetooth(BtId::Power));
            ids.extend(row(ListId::Bluetooth, s.lists().len(ListId::Bluetooth)));
        }
        Tab::Volume => {
            ids.push(Id::Audio(AudioId::MuteOutput));
            ids.extend((0..s.levels().audio().outputs.len() as u16).map(|i| Id::Audio(AudioId::Output(i))));
            ids.extend((0..s.lists().len(ListId::Streams).min(4) as u16).map(|i| Id::Row { list: ListId::Streams, i, part: RowPart::Mute }));
        }
        Tab::Time => {
            ids.extend([TimeId::Back, TimeId::Zone, TimeId::Ntp, TimeId::TwelveHour].map(Id::Time));
            ids.extend(row(ListId::Time, s.lists().len(ListId::Time)));
        }
        Tab::Battery => ids.push(Id::Battery(BatteryId::Back)),
        Tab::Touchpad | Tab::Sysinfo | Tab::Brightness => {}
    }
    ids
}

fn draw_once(rng: &mut Rng, s: &State, w: &mut World, clock: &mut u64) -> Option<Event> {
    let library = s.shelf().library();
    Some(match rng.below(100) {
        // Time passes, mostly a little.
        0..=14 => {
            *clock += match rng.below(20) {
                // Long enough to sleep, now and then.
                0 if rng.one_in(8) => 900 + rng.below(600) as u64,
                0 => 30 + rng.below(370) as u64,
                1..=3 => 5 + rng.below(20) as u64,
                _ => rng.below(3) as u64,
            };
            Event::Tick { unix_secs: 1_800_000_000 + *clock, mono_secs: *clock }
        }
        // Taps and long presses on what is shown, sometimes as a press down and up.
        15..=39 => {
            let ids = if rng.one_in(4) { bar() } else { targets(rng, s) };
            let id = rng.pick(&ids)?.clone();
            let press = match rng.below(10) {
                0 => PointerEvent::LongPress(id),
                1 => PointerEvent::Down(id, None),
                2 => PointerEvent::Up(s.pointer().pressed()?.clone()),
                3 if s.pad().route() == PadRoute::Ui && (s.pad().on_top() || !id.on_top()) => return Some(PointerEvent::Focus(id).into()),
                4 => PointerEvent::HideFocus,
                _ => PointerEvent::Activate(id),
            };
            // A press by touch, rather than the pad's A, hides the ring first, as the UI does. Without
            // the pad on ClamShell's screens, it is touch.
            let touch = s.pad().route() != PadRoute::Ui || rng.one_in(2);
            if touch && s.pointer().show_focus() && press != PointerEvent::HideFocus {
                w.next = Some(press.into());
                PointerEvent::HideFocus.into()
            } else {
                press.into()
            }
        }
        // The pad in shelf.
        40..=43 if !(s.pad().route() == PadRoute::Ui && s.pad().on_top()) => ShelfEvent::Rows(3 + rng.below(6)).into(),
        40..=43 => match rng.below(7) {
            0 => ShelfEvent::ListStep(rng.below(5) as i32 - 2).into(),
            1 => ShelfEvent::CollectionStep(rng.below(3) as i32 - 1).into(),
            2 => ShelfEvent::Rows(3 + rng.below(6)).into(),
            3 => ShelfEvent::ToggleFavourite.into(),
            4 => ShelfEvent::ClearFilter.into(),
            5 => ShelfEvent::TogglePin.into(),
            _ => ShelfEvent::Continue.into(),
        },
        // Lists scrolled and laid out.
        44..=45 => {
            let list = *rng.pick(&ListId::ALL)?;
            if rng.one_in(2) { ListEvent::Scroll(list, rng.below(9) as f32 - 4.0).into() } else { ListEvent::Visible(list, 2.0 + rng.below(6) as f32).into() }
        }
        // A pending launch answered: its scope appears, or the launcher says it failed.
        46..=52 => {
            let i = rng.below(w.launches.len());
            if w.launches.is_empty() {
                return None;
            }
            let (entry, instance) = w.launches.remove(i);
            if rng.one_in(5) {
                Event::Launch(LaunchEvent::LaunchFailed(entry))
            } else {
                w.units.push(instance);
                Event::Units(w.units.clone())
            }
        }
        // A window opens: a launch's (a program may open several), a Steam game's or a handed-over app's
        // on the landing workspace, or a stray one.
        53..=60 if w.windows.len() > 8 => return None,
        53..=60 => {
            let focused = rng.one_in(2);
            match rng.below(6) {
                0 => w.window("steam_app_620", String::new(), Some("app-steam.scope".into()), focused),
                1 => w.window("firefox", String::new(), None, focused),
                2 => w.window("foot", "1".into(), None, focused),
                _ => {
                    let unit = rng.pick(&w.units)?.clone();
                    let app = format!("app-{}", unit.key().unwrap_or("x"));
                    w.window(&app, unit.workspace(), unit.unit(), focused)
                }
            }
        }
        // A window closes, by itself or as asked.
        61..=64 => {
            let id = match w.closing.pop() {
                Some(id) if !rng.one_in(4) => id,
                _ => rng.pick(&w.windows).filter(|x| x.app_id != SHELF_APP_ID)?.id,
            };
            w.windows.retain(|x| x.id != id);
            if !w.windows.iter().any(|x| x.focused) {
                w.focus(|x| x.app_id == SHELF_APP_ID);
            }
            w.report()
        }
        // The focus moves to another window.
        65..=66 => {
            let id = rng.pick(&w.windows)?.id;
            w.focus(|x| x.id == id);
            w.report()
        }
        // A scope ends, by itself or as asked; its windows go with it most times.
        67..=69 => {
            let key = match w.ending.pop() {
                Some(k) if !rng.one_in(4) => k,
                _ => rng.pick(&w.units)?.key()?.to_owned(),
            };
            w.units.retain(|u| u.key() != Some(key.as_str()));
            if !rng.one_in(4) {
                let gone = |x: &Window| x.unit.as_deref().and_then(Instance::from_unit).is_some_and(|i| i.key() == Some(key.as_str()));
                w.windows.retain(|x| !gone(x));
                if !w.windows.iter().any(|x| x.focused) {
                    w.focus(|x| x.app_id == SHELF_APP_ID);
                }
                w.ending.retain(|k| *k != key);
            }
            Event::Units(w.units.clone())
        }
        // Something done to an entry: a running one, one on the dock, or any.
        70..=73 => {
            let id = match rng.below(3) {
                0 => rng.pick(s.session().running_entries())?.id.clone(),
                1 => rng.pick(s.dock().items())?.id.clone(),
                _ => rng.pick(&library.entries)?.id.clone(),
            };
            let act = [
                EntryAct::Play,
                EntryAct::Show,
                EntryAct::OpenAnother,
                EntryAct::Suspend,
                EntryAct::Suspend,
                EntryAct::ToggleAutostart,
                EntryAct::TogglePin,
                EntryAct::Quit,
                EntryAct::ForceKill,
            ];
            Event::Entry(id, *rng.pick(&act)?)
        }
        // The system threads' answers to what was asked.
        74..=76 => match rng.below(6) {
            0 if w.sticks => {
                if rng.one_in(3) {
                    w.sticks = false;
                    Event::Sticks(SticksEvent::Done("pad".into(), None))
                } else {
                    let rest = crate::device::sticks::StickAxis { low: 0, centre: 0, high: 0 };
                    let p = crate::device::sticks::Progress { phase: crate::device::sticks::Phase::Turn, now: [0; 4], seen: [rest; 4], min: -1024, max: 1024 };
                    Event::Sticks(SticksEvent::Progress(p))
                }
            }
            1 if w.gyro => {
                w.gyro = false;
                Event::GyroCalibrated(rng.one_in(2))
            }
            2 if w.refresh => {
                w.refresh = false;
                Event::RefreshCalibrated(Ok("done".into()))
            }
            3 if w.account => {
                w.account = false;
                Event::AccountChanged(if rng.one_in(2) { Ok("done".into()) } else { Err("failed".into()) })
            }
            4 if w.screenshot => {
                w.screenshot = false;
                Event::Screenshot(Ok("/tmp/shot.png".into()))
            }
            5 if w.asleep => {
                w.asleep = false;
                if rng.one_in(3) { Event::Idle(IdleEvent::Busy) } else { Event::Resumed }
            }
            _ => return None,
        },
        // Readings.
        77..=82 => Event::Reading(match rng.below(6) {
            0 => Reading::Status(Status {
                battery: Some(rng.below(101) as u8),
                discharging: rng.one_in(2),
                volume: Some(rng.below(101) as u8),
                muted: rng.one_in(4),
                brightness: Some(rng.below(101) as u8),
                ..Status::default()
            }),
            1 => {
                let net = |ssid: &str, secure, saved, connected| Network { ssid: ssid.into(), strength: 50, secure, enterprise: false, saved, connected };
                let mut networks = vec![net("home", true, true, rng.one_in(2)), net("cafe", false, false, false), net("work", true, false, false)];
                networks.truncate(rng.below(4));
                Reading::Wifi(Wifi { present: true, on: !rng.one_in(5), networks, ..Wifi::default() })
            }
            2 => {
                let dev = |name: &str, paired, connected| BtDevice { address: format!("00:{name}"), name: name.into(), kind: Kind::Other, paired, connected };
                let mut devices = vec![dev("buds", true, rng.one_in(2)), dev("pad", false, false)];
                devices.truncate(rng.below(3));
                Reading::Bluetooth(Bluetooth { present: true, on: true, devices, ..Bluetooth::default() })
            }
            3 => {
                let out = |id, name: &str, default| AudioOutput { id, name: name.into(), default };
                let stream = |id, app: &str| Stream { id, app: app.into(), title: String::new(), volume: 50, muted: false };
                let mut outputs = vec![out(1, "speakers", rng.one_in(2)), out(2, "buds", false)];
                outputs.truncate(rng.below(3));
                let mut streams = vec![stream(5, "mpv"), stream(6, "game")];
                streams.truncate(rng.below(3));
                Reading::Audio(Audio { outputs, streams })
            }
            4 => Reading::Account(crate::device::account::Account { available: true, password: !rng.one_in(3), ssh: rng.one_in(2) }),
            _ => Reading::Clipboard(rng.one_in(2).then(|| "pasted".into())),
        }),
        // The device's keys and buttons.
        83..=86 => {
            let keys = [
                DeviceKey::VolumeUp,
                DeviceKey::VolumeDown,
                DeviceKey::BrightnessUp,
                DeviceKey::BrightnessDown,
                DeviceKey::Mute,
                DeviceKey::Screenshot,
                DeviceKey::BottomView,
                DeviceKey::CloseWindow,
                DeviceKey::Quit,
                DeviceKey::BottomPower,
                DeviceKey::WindowDown,
                DeviceKey::WindowUp,
                DeviceKey::WindowPrevious,
                DeviceKey::WindowNext,
            ];
            Event::Device(*rng.pick(&keys)?)
        }
        87..=89 => match rng.below(5) {
            0 => Event::Home,
            1 => Event::PadSwitch,
            2 => Event::TextInput(rng.one_in(2)),
            3 => Event::BottomTouched,
            _ => Event::Paste,
        },
        // Idle's sources.
        90..=92 => Event::Idle(match rng.below(5) {
            0 => IdleEvent::Watching(true),
            1 => IdleEvent::CompositorQuiet,
            2 => IdleEvent::CompositorInput,
            3 => IdleEvent::KeptAwake,
            _ => IdleEvent::DeviceInput,
        }),
        // Levels and the slider.
        93..=94 => match rng.below(5) {
            0 => LevelEvent::Step(Level::Volume, if rng.one_in(2) { 1 } else { -1 }).into(),
            1 => LevelEvent::Set(Level::Brightness, rng.below(101) as u8).into(),
            2 => Slide::Start { control: if rng.one_in(2) { Control::Volume } else { Control::Brightness }, x: rng.below(100) as f32, width: 100.0 }.into(),
            3 => Slide::To { x: rng.below(100) as f32 }.into(),
            _ => Slide::End.into(),
        },
        // The pad's inputs while it listens on the keymap edit screen.
        95 if s.pad().route() == PadRoute::Listen => {
            let inputs: Vec<Input> = Input::all().collect();
            Event::Listened(*rng.pick(&inputs)?)
        }
        // A stray program's tray icon comes or goes.
        96 if rng.one_in(2) => {
            let pid = w.windows.iter().filter(|x| x.app_id == "foot").find_map(|x| x.pids.first().copied());
            let entry = |id: i32| crate::session::tray::TrayEntry { id, label: format!("item {id}"), enabled: true, checked: None, children: Vec::new() };
            let item = crate::session::tray::TrayItem {
                key: ":1.5/StatusNotifierItem".into(),
                id: "foot".into(),
                title: "Foot".into(),
                pid: if rng.one_in(3) { Some(4242) } else { pid },
                unit: None,
                icon: None,
                menu: vec![entry(1), crate::session::tray::TrayEntry { children: vec![entry(3)], ..entry(2) }],
            };
            Event::Tray(if rng.one_in(3) { Vec::new() } else { vec![item] })
        }
        96 => Event::Fetching(rng.one_in(2)),
        97 if rng.one_in(4) => Event::Library(Box::new(library.clone())),
        97 => Event::DumpWindowEvents("window-events.json".into()),
        98 => Event::Enqueue,
        99 => {
            let window = rng.pick(&w.windows)?.clone();
            match rng.below(10) {
                0 => {
                    let window = if rng.one_in(2) && w.windows.len() < 9 {
                        let mut window = w.open("foot", "1".into(), None, false);
                        window.kind = if rng.one_in(2) { WindowKind::X11 } else { WindowKind::Wayland };
                        if rng.one_in(3) {
                            window.parent = rng.pick(&w.windows).filter(|p| p.id < window.id).map(|p| p.id);
                        }
                        if let Some(actual) = w.windows.iter_mut().find(|actual| actual.id == window.id) {
                            actual.kind = window.kind;
                            actual.parent = window.parent;
                        }
                        window
                    } else {
                        window
                    };
                    WindowEvent::Opened(window).into()
                }
                1 if window.app_id != SHELF_APP_ID => {
                    w.windows.retain(|w| w.id != window.id);
                    WindowEvent::Closed(window.id).into()
                }
                2 => {
                    let title = format!("title {}", rng.below(5));
                    if let Some(w) = w.windows.iter_mut().find(|w| w.id == window.id) {
                        w.title.clone_from(&title);
                    }
                    WindowEvent::Title { window: window.id, title }.into()
                }
                3 => WindowEvent::KeyboardFocus(Some(window.id)).into(),
                4 => WindowEvent::KeyboardFocus(None).into(),
                5 => Event::HomeHeld(rng.one_in(2)),
                6 => Event::PadActive,
                7 => {
                    use crate::session::presentation::{WindowLimits, WindowRole};
                    let mut window = window;
                    window.limits = if rng.one_in(2) { WindowLimits { min: (640, 480), max: (640, 480) } } else { WindowLimits::default() };
                    window.role = if rng.one_in(2) { WindowRole::Utility } else { WindowRole::Normal };
                    WindowEvent::Changed(window).into()
                }
                8 => Event::HomeTap,
                _ => WindowEvent::Activate(window.id).into(),
            }
        }
        _ => return None,
    })
}

/// Sync run again: what it sends now (invariant 1 wants nothing).
fn sync_again(s: &mut State) -> Vec<Effect> {
    let mut out = Out::default();
    s.sync(&mut out, 0);
    out.effects
}

/// The entries running before an event, and those launching with when they were asked for.
type Before = (Vec<Arc<str>>, Vec<(Arc<str>, u64)>);

/// Checks the invariants after `event` gave `effects`. `before` is the state's running and launching
/// entries before the event.
fn check(s: &mut State, sent: &mut Sent, effects: &[Effect], before: &Before, started: bool) -> Result<(), String> {
    macro_rules! ensure {
        ($cond:expr, $($arg:tt)*) => {
            if !$cond {
                return Err(format!($($arg)*));
            }
        };
    }
    // 4. Live window ids are unique.
    for (i, window) in s.session().windows().iter().enumerate() {
        ensure!(!s.session().windows().iter().skip(i + 1).any(|other| other.id == window.id), "duplicate window id {}", window.id);
    }
    ensure!(s.session().windows().iter().filter(|w| w.focused && !w.down).count() <= 1, "more than one top window selected");
    ensure!(s.session().windows().iter().filter(|w| w.down && w.visible).count() <= 1, "more than one bottom tab selected");
    ensure!(s.session().selections_valid(), "a selected window is not in its top group");
    ensure!(s.session().windows_down().count() <= 1, "more than one window below");
    ensure!(s.session().keyboard_focus().is_none_or(|id| s.session().window(id).is_some()), "keyboard focus names a closed window");
    // 1. Sync converges.
    let again = sync_again(s);
    ensure!(again.is_empty(), "1: sync again sends {again:?}");

    // 2. What was sent mirrors the state.
    for e in effects {
        match e {
            Effect::PadRoute(r) => sent.route = Some(*r),
            Effect::PadActivity(watch) => sent.pad_activity = *watch,
            Effect::Keymap(k) => sent.keymap = Some(k.clone()),
            Effect::Apply { entry, settings } => sent.apply = Some((entry.clone(), *settings)),
            Effect::Watch(Watch::Status, on) => sent.status = *on,
            Effect::Watch(Watch::Account, on) => sent.account = *on,
            Effect::Outputs { top, bottom } => sent.outputs = (*top, *bottom),
            Effect::Refresh(hz) => sent.refresh = *hz,
            _ => {}
        }
        if let Effect::Launch { settings, .. } | Effect::Apply { settings, .. } = e {
            sent.refresh_wanted = settings.get(Setting::Refresh);
        }
    }
    if started {
        ensure!(sent.route == Some(s.pad().route()), "2: route sent {:?}, state {:?}", sent.route, s.pad().route());
        let in_force = s.keymapping().in_force(s.in_force().get(Setting::Keymap));
        ensure!(sent.keymap.as_ref() == Some(&in_force), "2: keymap sent {:?}, in force {}", sent.keymap.as_ref().map(|k| k.id), in_force.id);
        ensure!(sent.account == s.device().is_open(), "2: account watched {}, view open {}", sent.account, s.device().is_open());
    }
    let screens_on = s.idle().stage() < Stage::Off;
    ensure!(sent.status == screens_on, "2: status watched {}, screens on {screens_on}", sent.status);
    let outputs = (screens_on, !s.bottom().off());
    ensure!(sent.outputs == outputs, "2: outputs sent {:?}, state {outputs:?}", sent.outputs);
    ensure!(sent.refresh == sent.refresh_wanted, "2: refresh sent {}, last launched or applied {}", sent.refresh, sent.refresh_wanted);
    let apply = (s.session().program().cloned(), s.in_force());
    ensure!(sent.apply.as_ref() == Some(&apply), "2: applied {:?}, in force {:?}", sent.apply, apply);

    // 3. The pad's route.
    ensure!(
        sent.pad_activity == s.session().top_window().is_some_and(|w| Some(w.id) != s.session().keyboard_focus()),
        "pad activity watch differs from keyboard focus"
    );
    let route = s.pad().route();
    let listening = s.listening();
    let moving = s.dock().moving().is_some() && s.bottom().tab() == Tab::Dock;
    let claimant = if s.pad().mouse_on() {
        PadRoute::Mouse
    } else {
        match (s.session().front(), s.bottom().tab()) {
            (Front::Shelf, _) => PadRoute::Ui,
            (_, Tab::Keyboard) => PadRoute::Keys,
            _ => PadRoute::Game,
        }
    };
    ensure!(
        (route == PadRoute::Listen) == (listening && !moving && (claimant == PadRoute::Ui || route == PadRoute::Listen)),
        "3: route {route:?}, listening {listening}"
    );
    if listening {
        ensure!(route != PadRoute::Ui || moving, "3: listening but route Ui");
    }
    if moving {
        ensure!(route == PadRoute::Ui, "3: a dock entry moves but the route is {route:?}");
    }
    if claimant == PadRoute::Ui && !moving {
        ensure!(route == if listening { PadRoute::Listen } else { PadRoute::Ui }, "3: shelf claims, route {route:?}");
    }
    ensure!(route == claimant || route == PadRoute::Ui || route == PadRoute::Listen, "3: route {route:?}, claimant's {claimant:?}");
    let again = s.reconcile_pad();
    ensure!(again.is_none(), "3: reconciling again changes the route to {again:?}");
    // A hold on shelf's selected entry shows the ring on its actions whatever has the pad, since the ring
    // is what shows the actions are open. It stays until the next touch hides it, though the focus may
    // move meanwhile (the Device view opening puts it on its first row).
    if !s.pointer().show_focus() {
        sent.ring_by_hold = false;
    } else if route != PadRoute::Ui && s.shelf().in_actions(s.pointer()) {
        sent.ring_by_hold = true;
    }
    ensure!(!s.pointer().show_focus() || route == PadRoute::Ui || sent.ring_by_hold, "3: the focus ring shows with route {route:?}");

    // 4. The session.
    let session = s.session();
    let running = session.running_entries();
    for (i, r) in running.iter().enumerate() {
        ensure!(!running.iter().skip(i + 1).any(|o| o.id == r.id), "4: {} runs twice", r.id);
        match r.origin {
            Origin::Ours => ensure!(session.has_units(&r.key), "4: {} runs as ours without a scope", r.id),
            Origin::Found { .. } => ensure!(!session.has_units(&r.key), "4: {} found with its scope listed", r.id),
            Origin::Windows { .. } => {}
        }
        if session.paused(&r.id) {
            ensure!(r.origin == Origin::Ours, "4: {} paused as {:?}", r.id, r.origin);
        }
        if session.suspended(&r.id) {
            ensure!(session.paused(&r.id), "4: {} suspended, not paused", r.id);
        }
    }
    let launching = session.launching();
    for (i, l) in launching.iter().enumerate() {
        ensure!(!launching.iter().skip(i + 1).any(|o| o.entry == l.entry), "4: {} launching twice", l.entry);
        ensure!(!session.units().contains(&l.instance), "4: {} launching with its scope listed", l.entry);
    }
    if let Some(p) = session.program() {
        ensure!(session.running(p).is_some(), "4: program {p} doesn't run");
    }
    if let Some(owner) = session.windows().iter().find(|w| w.focused && !w.down).and_then(|w| session.running_of(w)) {
        ensure!(session.program() == Some(&owner.id), "4: program {:?}, the focused window's {}", session.program(), owner.id);
    }
    for q in session.quitting_entries() {
        ensure!(session.running(q).is_some(), "4: {q} quitting, not running");
        ensure!(!session.paused(q), "4: {q} quitting and paused");
    }

    // 5. The dock.
    let items = s.dock().items();
    for (i, item) in items.iter().enumerate() {
        ensure!(!items.iter().skip(i + 1).any(|o| o.id == item.id), "5: {} on the dock twice", item.id);
        ensure!(item.pinned || session.running(&item.id).is_some() || s.dock().moving() == Some(&item.id), "5: {} on the dock for nothing", item.id);
    }
    for r in running {
        ensure!(items.iter().any(|i| i.id == r.id), "5: {} runs off the dock", r.id);
    }

    // 6. Shelf.
    let shelf = s.shelf();
    let list = shelf.list();
    if !shelf.library().collections.is_empty() {
        ensure!(shelf.current() < shelf.library().collections.len(), "6: collection {} out of range", shelf.current());
    }
    if list.is_empty() {
        ensure!(shelf.selected().is_none(), "6: {:?} selected in an empty list", shelf.selected());
    } else {
        let cursor = shelf.cursor().index;
        ensure!(cursor < list.len(), "6: cursor {cursor} past {}", list.len());
        ensure!(
            shelf.selected() == shelf.entry_at(cursor).map(|g| &g.id),
            "6: {:?} selected, {:?} at the cursor",
            shelf.selected(),
            shelf.entry_at(cursor).map(|g| &g.id)
        );
    }
    let filter = shelf.filter().to_lowercase();
    for g in list.iter().filter_map(|&i| shelf.library().entries.get(i)) {
        ensure!(g.title.to_lowercase().starts_with(&filter), "6: {} listed for {filter:?}", g.id);
    }

    // 7. Lists.
    for list in ListId::ALL {
        let lists = s.lists();
        let (scroll, len) = (lists.scroll(list), lists.len(list));
        ensure!(scroll.offset() >= 0.0 && scroll.offset() <= scroll.max_offset(len), "7: {list:?} at {} of {len}", scroll.offset());
    }

    // 8. The bottom screen.
    let bottom = s.bottom();
    ensure!(!bottom.dimmed() || !bottom.shows_windows(session), "8: dimmed while showing windows");
    ensure!(bottom.chosen_tab() == bottom.tab() || bottom.tab() == Tab::Keyboard, "8: chosen {:?}, shown {:?}", bottom.chosen_tab(), bottom.tab());
    ensure!(bottom.editor().is_none_or(|e| e.tab() == bottom.tab()), "8: an editor for another tab shown");
    ensure!(bottom.tab() != Tab::Touchpad || bottom.pointing(), "8: the touchpad tab doesn't point");

    // 9. Keymaps.
    let keymaps = s.keymapping().keymaps();
    if let Some(id) = s.keymapping().screen().and_then(|sc| sc.keymap()) {
        let keymap = keymaps.get(id);
        ensure!(keymap.is_some(), "9: the screen's keymap {id} is gone");
        let layer = s.keymapping().edit().layer;
        ensure!(keymap.is_some_and(|k| k.layer_name(layer).is_some()), "9: layer {layer:?} of keymap {id} is gone");
    }
    let settings = s.prefs().settings();
    let chosen = std::iter::once(settings.global.get(Setting::Keymap)).chain(settings.overrides.values().filter_map(|o| o.get(Setting::Keymap)));
    for id in chosen {
        ensure!(id == 0 || keymaps.get(id).is_some(), "9: keymap {id} chosen and gone");
    }

    // 10. Settings.
    for (entry, o) in &settings.overrides {
        ensure!(!o.is_empty(), "10: {entry} has empty overrides");
        for t in Setting::ALL.into_iter().filter(|t| t.global_only()) {
            ensure!(o.get(t).is_none(), "10: {entry} overrides global-only {t:?}");
        }
    }

    // 11. The effect stream.
    let (was_running, was_launching) = before;
    let mut launched = Vec::new();
    let mut recorded = Vec::new();
    for e in effects {
        match e {
            Effect::Freeze { slice, frozen } => {
                let last = sent.frozen.insert(slice.clone(), *frozen);
                ensure!(last != Some(*frozen), "11: slice {slice} frozen {frozen} twice");
                ensure!(last.is_some() || *frozen, "11: slice {slice} thawed, never frozen");
            }
            Effect::EndRun { key, force: false } => {
                ensure!(!sent.ended.contains(key), "11: {key} ended twice for one quit");
                sent.ended.push(key.clone());
            }
            Effect::RecordPlay { entry, .. } => {
                ensure!(!recorded.contains(entry), "11: two plays of {entry} at once");
                ensure!(was_running.contains(entry), "11: a play of {entry}, which didn't run");
                recorded.push(entry.clone());
            }
            Effect::Launch { entry, instance, .. } => {
                // One given up in this event (see LAUNCH_WAIT) may be launched again in it.
                let now = s.clock().mono();
                let waiting = was_launching.iter().any(|(e, at)| e == entry && now < at + crate::session::running::LAUNCH_WAIT);
                ensure!(!waiting && !launched.contains(entry), "11: {entry} launched while launching");
                launched.push(entry.clone());
                if let Some(key) = instance.key() {
                    sent.ended.retain(|k| k != key);
                }
            }
            _ => {}
        }
    }
    // A slice with no scope listed is gone; one started again starts thawed.
    let keys: Vec<String> = s.session().units().iter().filter_map(Instance::key).map(Instance::slice_of).collect();
    sent.frozen.retain(|slice, _| keys.contains(slice));
    Ok(())
}

/// Runs `events` events from `seed`. Returns a description of the first invariant broken.
fn run(seed: u64, events: usize) -> Result<(), String> {
    let mut rng = Rng(seed);
    let init = Init { library: library(), keymaps: crate::keymap::Keymaps::initial(), device_session: true, first_start: rng.one_in(3), ..Init::default() };
    let mut s = State::new(init);
    let refresh = s.in_force().get(Setting::Refresh);
    let mut sent = Sent {
        apply: Some((s.session().program().cloned(), s.in_force())),
        status: true,
        outputs: (true, true),
        refresh,
        refresh_wanted: refresh,
        ..Sent::default()
    };
    let mut world = World::new();
    let mut clock = 1000;
    let mut history: Vec<String> = Vec::new();
    let mut effects = Vec::new();
    for i in 0..events {
        let event = match i {
            0 => Event::Tick { unix_secs: 1_800_000_000 + clock, mono_secs: clock },
            1 => Event::Start,
            2 => Event::Units(Vec::new()),
            3 => world.report(),
            _ => draw(&mut rng, &s, &mut world, &mut clock),
        };
        // A quit asked for may end its entry again.
        match &event {
            Event::Entry(id, EntryAct::Quit) => {
                if let Some(r) = s.session().running(id) {
                    sent.ended.retain(|k| *k != r.key);
                }
            }
            Event::Device(DeviceKey::Quit) | Event::Pointer(PointerEvent::Activate(Id::Dock(DockId::Quit))) => sent.ended.clear(),
            _ => {}
        }
        let before =
            (s.session().running_entries().iter().map(|r| r.id.clone()).collect(), s.session().launching().iter().map(|l| (l.entry.clone(), l.at)).collect());
        history.push(match &event {
            Event::Library(_) => format!("{i}: Library (the same)"),
            e => format!("{i}: {e:?}"),
        });
        if history.len() > 20 {
            history.remove(0);
        }
        effects.clear();
        s.apply(event, &mut effects);
        world.follow(&effects);
        if let Err(e) = check(&mut s, &mut sent, &effects, &before, i >= 1) {
            return Err(format!("seed {seed}, event {i}: {e}\neffects: {effects:?}\nlast events:\n{}", history.join("\n")));
        }
    }
    Ok(())
}

fn var(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

/// The invariants hold after every event of seeded random sequences (see the module's doc).
#[test]
fn random_event_sequences_keep_the_invariants() {
    let events = var("RANDOM_EVENTS").map_or(EVENTS, |n| n as usize);
    let seeds: Vec<u64> = match (var("RANDOM_SEED"), var("RANDOM_SEEDS")) {
        (Some(seed), _) => vec![seed],
        (None, Some(n)) => (1..=n).collect(),
        (None, None) => SEEDS.to_vec(),
    };
    for seed in seeds {
        let result = run(seed, events);
        assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
    }
}
