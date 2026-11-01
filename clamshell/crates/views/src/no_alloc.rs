//! Drawing a frame must not touch the heap once the caches are warm, that is, once glyphs are rasterized
//! and buffers sized: recording it, comparing it with the last, and playing what changed. The test
//! counts allocations with an allocator that tallies this thread's calls.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_state::Control;
use clamshell_state::DeviceId;
use clamshell_state::bottom::keyboard::{self, Key, Modifier};
use clamshell_state::device_view::SticksEvent;
use clamshell_state::device_view::sticks::{Phase, Progress, StickAxis};
use clamshell_state::keymap::{Button, Input, Keymaps, Layer, Param, Side};
use clamshell_state::library_view::edit::EditId;
use clamshell_state::lights::screens::LightsId;
use clamshell_state::lights::{LightParam, Stick};
use clamshell_state::radios::{bluetooth, wifi};
use clamshell_state::session::tray;
use clamshell_state::session::windows::WindowFacts;
use clamshell_state::settings::Setting;
use clamshell_state::setup::account::Account;
use clamshell_state::status::battery;
use clamshell_state::{
    DockId, EditorId, Event, Id, Init, KeymapId, LaunchUnit, LibraryViewEvent, LibraryViewId, ListEvent, ListId, PointerEvent, Reading, RowPart, SettingsId,
    SettingsLevel, Slide, State, Status, Tab, Target, TimeZones, levels,
};

use clamshell_ui::Screen;
use clamshell_ui::gfx::DrawCache;
use clamshell_ui::render::{Placement, ScreenFrames};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: forwards to the system allocator unchanged and only counts the calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(Cell::get)
}

fn window(id: i64, app_id: &str, unit: Option<&str>) -> WindowFacts {
    WindowFacts { id, app_id: app_id.into(), unit: unit.map(Into::into), ..Default::default() }
}

fn key(k: Key) -> Event {
    Event::Pointer(PointerEvent::Activate(Id::Key(keyboard::find(k).expect("key on the layout"))))
}

/// Opens the Library's Tools collection and runs its Device entry, which shows the Device view.
fn device() -> Vec<Event> {
    let library = clamshell_library::demo::library();
    let tools = library.collections.iter().position(|c| c.name == "Tools").unwrap_or(0);
    let entry = || tap(Id::LibraryView(LibraryViewId::Entry("builtin:device".into())));
    vec![tap(Id::LibraryView(LibraryViewId::Collection(u16::try_from(tools).unwrap_or(0)))), entry(), entry()]
}

/// Two players, and two controllers, one on each, the second gone.
fn controllers() -> Vec<Event> {
    use clamshell_state::device_view::controllers::ControllerEvent;
    let connected = |key: &str| Event::Controller(ControllerEvent::Connected { key: key.into(), name: format!("{key} pad") });
    vec![
        connected("a"),
        connected("b"),
        Event::Controller(ControllerEvent::StepCount(1)),
        Event::Controller(ControllerEvent::Step(1, 1)),
        Event::Controller(ControllerEvent::Disconnected("b".into())),
    ]
}

/// A sticks calibration as it starts, waiting for the sticks to be let go.
fn calibrating() -> Progress {
    let rest = StickAxis { low: 0, centre: 0, high: 0 };
    Progress { phase: Phase::LetGo, now: [0; 4], seen: [rest; 4], min: -1024, max: 1024 }
}

/// A program's notification, which stays until dismissed; the third is critical.
fn notification(id: u32) -> Event {
    let n = clamshell_state::Notification {
        id: clamshell_state::NotificationId::Bus(id),
        app: "mail".into(),
        summary: format!("New mail {id}"),
        body: "From Ann: about tomorrow\nSecond line".into(),
        urgency: if id == 3 { clamshell_state::Urgency::Critical } else { clamshell_state::Urgency::Normal },
        action: Some("default".into()),
        timeout_ms: 0,
    };
    clamshell_state::NotificationEvent::Notified(Box::new(n)).into()
}

/// A tap on one of the Library's edit mode's targets.
fn edit(id: EditId) -> Event {
    tap(Id::LibraryView(LibraryViewId::Edit(id)))
}

fn tap(id: Id) -> Event {
    Event::Pointer(PointerEvent::Activate(id))
}

fn wifi_reading() -> Event {
    Event::Reading(Reading::Wifi(wifi::Wifi {
        present: true,
        on: true,
        networks: vec![
            wifi::Network { ssid: "home".into(), strength: 80, secure: true, saved: true, connected: true, enterprise: false },
            wifi::Network { ssid: "cafe".into(), strength: 40, secure: true, ..Default::default() },
        ],
        busy: Some("cafe".into()),
        error: None,
    }))
}

/// A screen drawn frame after frame into one pixmap, as a live view does.
struct Screen1 {
    frames: ScreenFrames,
    pixmap: tiny_skia::Pixmap,
    held: u64,
    at: Placement,
    layer: clamshell_ui::screens::Layer,
}

impl Screen1 {
    fn new(layer: impl Into<clamshell_ui::screens::Layer>, at: Placement) -> Self {
        let pixmap = tiny_skia::Pixmap::new(at.size_px.0, at.size_px.1).expect("pixmap");
        Screen1 { frames: ScreenFrames::default(), pixmap, held: 0, at, layer: layer.into() }
    }

    /// Records `state`, and plays what changed since the last frame.
    fn draw(&mut self, state: &State, cache: &mut DrawCache, images: &mut clamshell_ui::gfx::images::Images) {
        self.frames.record(self.at, self.layer, state, cache, images, &crate::VIEWS);
        if self.frames.damage().is_some() {
            self.held = self.frames.draw(&mut self.pixmap, self.held, cache, images).0;
        }
    }
}

fn account() -> Account {
    Account::Read { password: true, ssh: true }
}

/// Each case starts from the same base state, the demo library at a fixed time, so that the cases don't
/// depend on each other.
#[test]
fn warm_frames_do_not_allocate() {
    let mut cache = DrawCache::load();
    let mut images = clamshell_ui::gfx::images::Images::synchronous();
    let mut top = Screen1::new(Screen::Top, Placement { size_px: (1920, 1080), scale: 1, drift: (1, -2) });
    let mut bottom = Screen1::new(Screen::Bottom, Placement { size_px: (1240, 1080), scale: 1, drift: (-3, 0) });
    let mut overlay = Screen1::new(clamshell_ui::screens::Layer::Overlay(Screen::Top), Placement { size_px: (1920, 1080), scale: 1, drift: (1, -2) });
    let mut below = Screen1::new(clamshell_ui::screens::Layer::Overlay(Screen::Bottom), Placement { size_px: (1240, 1080), scale: 1, drift: (-3, 0) });

    let cases: Vec<(&str, Vec<Event>)> = vec![
        ("sysinfo", vec![]),
        ("settings", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Settings)))]),
        ("settings, a changed tile", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::FpsCap)))]),
        ("settings, all games", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Scope(SettingsLevel::Global)))]),
        ("environment editor", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Env)), key(Key::Char('a'))]),
        (
            "environment saved",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Env)),
                key(Key::Char('a')),
                key(Key::Char('=')),
                key(Key::Char('1')),
                tap(Id::Editor(EditorId::Save)),
            ],
        ),
        (
            "dock, an app running",
            vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Dock))), Event::Pointer(PointerEvent::Activate(Id::Dock(DockId::Entry(1))))],
        ),
        (
            "dock, a game and windows",
            vec![
                tap(Id::Tab(Tab::Dock)),
                Event::Units(vec![LaunchUnit { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 }]),
                Event::Units(["clamshell-run-zelda_1-1-0", "clamshell-run-foot_1-1-0"].into_iter().filter_map(LaunchUnit::parse).collect()),
                Event::WindowBatch(
                    [
                        window(5, "retroarch", Some("clamshell-run-zelda_1-1-0.scope")),
                        window(7, "foot", Some("clamshell-run-foot_1-1-0.scope")),
                        window(8, "foot", None),
                    ]
                    .into_iter()
                    .map(clamshell_state::WindowEvent::Opened)
                    .collect(),
                ),
                clamshell_state::WindowEvent::Activate(7).into(),
            ],
        ),
        ("notifications", (1..=5).map(notification).collect()),
        (
            "window arrows, Home held",
            vec![
                Event::WindowBatch(
                    [window(1, "test", Some("clamshell-run-test-1-0.service")), window(2, "test", Some("clamshell-run-test-1-0.service"))]
                        .into_iter()
                        .map(clamshell_state::WindowEvent::Opened)
                        .collect(),
                ),
                clamshell_state::WindowEvent::Activate(1).into(),
                Event::Reading(Reading::Frames { fps: Some(59.9), frame_ms: Some(16.7) }),
                Event::HomeHeld(true),
            ],
        ),
        (
            "wifi",
            vec![
                Event::Reading(Reading::Status(Status { volume: Some(40), brightness: Some(70), wifi: Some(60), ..Default::default() })),
                tap(Id::Tab(Tab::Wifi)),
                wifi_reading(),
            ],
        ),
        (
            "wifi, scrolled part way",
            vec![
                tap(Id::Tab(Tab::Wifi)),
                Event::Reading(Reading::Wifi(wifi::Wifi {
                    present: true,
                    on: true,
                    networks: (0..20).map(|i| wifi::Network { ssid: format!("n{i}"), strength: 50, ..Default::default() }).collect(),
                    ..Default::default()
                })),
                Event::List(ListEvent::Visible(ListId::Wifi, 6.5)),
                Event::List(ListEvent::Scroll(ListId::Wifi, 3.3)),
            ],
        ),
        ("the Library, dragged part way", vec![Event::LibraryView(LibraryViewEvent::Rows(3)), Event::List(ListEvent::Scroll(ListId::LibraryView, 1.6))]),
        ("edit mode, a tab's folders", [device(), vec![tap(Id::Device(DeviceId::EditLibrary)), edit(EditId::Tab(2))]].concat()),
        ("edit mode, the sources", [device(), vec![tap(Id::Device(DeviceId::EditLibrary)), edit(EditId::Sources)]].concat()),
        (
            "edit mode, a source's place chosen",
            [device(), vec![tap(Id::Device(DeviceId::EditLibrary)), edit(EditId::Sources), edit(EditId::Open), edit(EditId::Field(3))]].concat(),
        ),
        ("edit mode, the runners", [device(), vec![tap(Id::Device(DeviceId::EditLibrary)), edit(EditId::Runners)]].concat()),
        (
            "a save picked in Home",
            vec![
                Event::Portal(clamshell_state::PortalEvent::Asked(clamshell_state::FileRequest {
                    id: 1,
                    title: String::new(),
                    asks: clamshell_state::Asks::Save { name: "notes.txt".into() },
                    patterns: vec!["*.txt".into()],
                    asker: Vec::new(),
                    folder: None,
                })),
                tap(Id::LibraryView(LibraryViewId::Launch)),
            ],
        ),
        ("setup, the password", vec![Event::Reading(Reading::Account(Account::Read { password: false, ssh: false }))]),
        ("keymap picker", vec![tap(Id::Tab(Tab::Settings)), PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into()]),
        ("the Device view's controllers", [device(), controllers()].concat()),
        (
            "two players' keymap tiles, and player 2's picker",
            [controllers(), vec![tap(Id::Tab(Tab::Settings)), PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap2))).into()]].concat(),
        ),
        (
            "tile menu",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(Setting::FpsCap))),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::FpsCap))).into(),
            ],
        ),
        (
            "keymap edit screen",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
            ],
        ),
        (
            "keymap mouse",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Trigger(Side::Left)))),
                tap(Id::Keymap(KeymapId::Target(Target::MouseParts))),
            ],
        ),
        (
            "keymap picking a control",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Button(Button::A)))),
                tap(Id::Keymap(KeymapId::Target(Target::Pick))),
            ],
        ),
        (
            "keymap panel, a slider up, another layer",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Layer(Layer(1)))),
                tap(Id::Keymap(KeymapId::Input(Input::Stick(Side::Left)))),
                Event::Slide(Slide::Start { control: Control::Param(Param::Deadzone), x: 285.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 320.0 }),
            ],
        ),
        (
            "keymap key capture",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Stick(Side::Left)))),
                tap(Id::Keymap(KeymapId::Target(Target::Keys))),
                key(Key::Char('w')),
                key(Key::Mod(Modifier::Shift)),
            ],
        ),
        ("lights tile", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::Lights)))]),
        ("lights picker", vec![tap(Id::Tab(Tab::Settings)), PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Lights))).into()]),
        (
            "lights edit screen, unlinked, a slider held",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Lights))).into(),
                tap(Id::Row { list: ListId::Lights, i: 2, part: RowPart::Edit }),
                tap(Id::Lights(LightsId::Linked)),
                tap(Id::Lights(LightsId::Stick(Stick::Right))),
                Event::Slide(Slide::Start { control: Control::Light(LightParam::Hue), x: 300.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 340.0 }),
            ],
        ),
        ("wifi password", vec![tap(Id::Tab(Tab::Wifi)), wifi_reading(), tap(ListId::Wifi.row(1)), tap(ListId::Wifi.row(0))]),
        (
            "sliding",
            vec![
                Event::Reading(Reading::Status(Status { volume: Some(40), ..Default::default() })),
                Event::Pointer(PointerEvent::Down(Id::Control(Control::Volume), None)),
                Event::Slide(Slide::Start { control: Control::Volume, x: 285.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 320.0 }),
            ],
        ),
        (
            "battery",
            vec![
                Event::Reading(Reading::Status(Status { battery: Some(41), ..Default::default() })),
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Battery))),
                Event::Reading(Reading::Battery(battery::Battery {
                    history: (0..50u64).map(|i| battery::Charge { at: 1_699_950_000 + i * 900, level: (90 - i) as u8, charging: i > 40 }).collect(),
                    spenders: vec![
                        battery::Spender {
                            group: battery::Group::Process("firefox".into()),
                            cpu: 0.2,
                            gpu: 0.1,
                            memory: 300 << 20,
                            pids: vec![],
                            endable: true,
                        },
                        battery::Spender { group: battery::Group::System, cpu: 0.05, gpu: 0.0, memory: 600 << 20, pids: vec![], endable: false },
                    ],
                    has_history: true,
                })),
            ],
        ),
        (
            "bluetooth",
            vec![
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Bluetooth))),
                Event::Reading(Reading::Bluetooth(bluetooth::Bluetooth {
                    present: true,
                    on: true,
                    devices: vec![bluetooth::Device {
                        address: "00:11".into(),
                        name: "buds".into(),
                        kind: bluetooth::Kind::Audio,
                        paired: true,
                        connected: true,
                    }],
                    busy: Some("00:11".into()),
                    error: None,
                    // (A keyboard's code shown in the header, drawn without allocating too.)
                    code: Some(clamshell_state::radios::bluetooth::PairingCode::Type("123456".into())),
                })),
            ],
        ),
        (
            "volume",
            vec![
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Volume))),
                Event::Reading(Reading::Audio(levels::Audio {
                    outputs: vec![levels::AudioOutput { id: 1, name: "Speakers".into(), default: true }],
                    streams: vec![levels::Stream { id: 2, app: "Firefox".into(), title: String::new(), volume: 40, muted: false }],
                    seen: 0,
                })),
                Event::Pointer(PointerEvent::Down(levels::Level::Stream(0).id(), None)),
            ],
        ),
        ("brightness", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Brightness))), Event::Reading(Reading::Backlights(vec!["panel".into()]))]),
        (
            "dark while a game runs",
            vec![
                Event::Units(vec![LaunchUnit { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 }]),
                // The second screen's setting, from Off when idle past On to Dark in game.
                tap(Id::Settings(SettingsId::Tile(Setting::BottomScreen))),
                tap(Id::Settings(SettingsId::Tile(Setting::BottomScreen))),
            ],
        ),
        ("keyboard", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Keyboard)))]),
        ("device", [device(), vec![Event::Reading(Reading::Account(account()))]].concat()),
        (
            "device, the password",
            [device(), vec![Event::Reading(Reading::Account(account())), tap(Id::Device(DeviceId::Password)), key(Key::Char('a'))]].concat(),
        ),
        ("device, the sticks calibrating", [device(), vec![tap(Id::Device(DeviceId::Sticks)), Event::Sticks(SticksEvent::Progress(calibrating()))]].concat()),
        ("keyboard, Fn", vec![tap(Id::Tab(Tab::Keyboard)), key(Key::Mod(Modifier::Fn))]),
        ("touchpad", vec![tap(Id::Tab(Tab::Touchpad))]),
        (
            "time",
            vec![
                tap(Id::Tab(Tab::Time)),
                Event::Reading(Reading::Time(TimeZones {
                    current: Some("Europe/Oslo".into()),
                    zones: ["Europe/Oslo", "Europe/Paris", "America/New_York"].map(String::from).to_vec(),
                    ntp: Some(true),
                    synced: true,
                    error: None,
                })),
            ],
        ),
        (
            "time, a region's zones",
            vec![
                tap(Id::Tab(Tab::Time)),
                Event::Reading(Reading::Time(TimeZones {
                    current: Some("Europe/Oslo".into()),
                    zones: ["Europe/Oslo", "Europe/Paris", "America/New_York"].map(String::from).to_vec(),
                    ntp: Some(true),
                    synced: true,
                    error: None,
                })),
                tap(Id::Time(clamshell_state::TimeId::Zone)),
                tap(clamshell_state::ListId::Time.row(7)),
            ],
        ),
        (
            "a tone curve's editor",
            vec![
                // (A new curve, named, opens on its sliders.)
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Curve))).into(),
                tap(Id::Profile(clamshell_state::profiles::picker::Kind::Curve, clamshell_state::profiles::picker::ProfileId::New)),
                key(Key::Char('a')),
                tap(Id::Editor(clamshell_state::EditorId::Save)),
            ],
        ),
        (
            "keymap name editor",
            vec![
                tap(Id::Tab(Tab::Settings)),
                PointerEvent::LongPress(Id::Settings(SettingsId::Tile(Setting::Keymap))).into(),
                tap(Id::Profile(clamshell_state::profiles::picker::Kind::Keymap, clamshell_state::profiles::picker::ProfileId::New)),
                key(Key::Char('a')),
            ],
        ),
        // The demo's TV tab with one of its series open.
        (
            "the Library, an open group",
            vec![
                Event::LibraryView(LibraryViewEvent::Rows(8)),
                tap(Id::LibraryView(LibraryViewId::Collection(2))),
                tap(Id::LibraryView(LibraryViewId::Launch)),
            ],
        ),
        (
            "the Library, a video",
            vec![
                Event::LibraryView(LibraryViewEvent::Rows(8)),
                tap(Id::LibraryView(LibraryViewId::Collection(2))),
                tap(Id::LibraryView(LibraryViewId::Launch)),
                tap(Id::LibraryView(LibraryViewId::Launch)),
                Event::LibraryView(LibraryViewEvent::ListStep(1)),
            ],
        ),
        (
            "dock, an entry's menu",
            vec![
                tap(Id::Tab(Tab::Dock)),
                Event::Units(vec![LaunchUnit { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 0 }]),
                Event::Pointer(PointerEvent::LongPress(Id::Dock(DockId::Entry(0)))),
            ],
        ),
        (
            "dock, a tray menu",
            vec![
                tap(Id::Tab(Tab::Dock)),
                Event::Tray(vec![tray::TrayItem {
                    key: "org.kde.StatusNotifierItem-1/StatusNotifierItem".into(),
                    id: "steam".into(),
                    title: "Steam".into(),
                    pid: Some(4242),
                    unit: None,
                    icon: None,
                    menu: ["Library", "Friends", "Exit Steam"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, label)| tray::TrayEntry { id: i as i32, label: label.into(), enabled: true, checked: None, children: vec![] })
                        .collect(),
                }]),
                Event::Pointer(PointerEvent::LongPress(Id::Dock(DockId::Entry(0)))),
                tap(Id::Dock(DockId::TrayMenu)),
            ],
        ),
        (
            "battery, a program's menu",
            vec![
                tap(Id::Tab(Tab::Battery)),
                Event::Reading(Reading::Battery(battery::Battery {
                    history: vec![],
                    spenders: vec![battery::Spender {
                        group: battery::Group::Process("firefox".into()),
                        cpu: 0.2,
                        gpu: 0.1,
                        memory: 300 << 20,
                        pids: vec![77],
                        endable: true,
                    }],
                    has_history: false,
                })),
                tap(Id::Battery(clamshell_state::BatteryId::Spender(0))),
            ],
        ),
        (
            "keyboard, Shift and a pressed key",
            vec![
                tap(Id::Tab(Tab::Keyboard)),
                key(Key::Mod(Modifier::Shift)),
                Event::Pointer(PointerEvent::Down(Id::Key(keyboard::find(Key::Char('h')).expect("h")), None)),
            ],
        ),
    ];
    // For the cases that are easy to get wrong: the state each must reach, so that it tests the screen it
    // is named for.
    type Reached = fn(&State) -> bool;
    let reached: [(&str, Reached); 14] = [
        ("lights picker", |s| s.lighting().screen().is_some()),
        ("lights edit screen, unlinked, a slider held", |s| {
            s.lighting().editing().is_some() && s.held_slider().is_some() && s.lighting().stick() == Stick::Right
        }),
        ("notifications", |s| s.notifications().len() == 5 && s.bottom_lit()),
        ("window arrows, Home held", |s| s.session().home_held() && s.session().has_other_top_window()),
        ("keymap name editor", |s| s.bottom().editor().is_some()),
        ("the Library, an open group", |s| s.library_view().group().is_some()),
        ("the Library, a video", |s| s.library_view().selected().is_some_and(|id| id.ends_with(".mkv"))),
        ("dock, an entry's menu", |s| s.dock().menu().is_some() && !s.dock().tray_open()),
        ("dock, a tray menu", |s| s.dock().tray_open()),
        ("battery, a program's menu", |s| s.sys().spender_menu().is_some()),
        ("time", |s| s.bottom().tab() == Tab::Time),
        ("time, a region's zones", |s| matches!(s.sys().time_page(), clamshell_state::status::time::TimePage::Region(_))),
        ("a tone curve's editor", |s| matches!(s.curving().screen(), Some(clamshell_state::curve::screens::CurveScreen::Edit(_)))),
        ("dark while a game runs", |s| s.bottom_dark()),
    ];
    let base = || {
        // The Nintendo layout, with a layer added.
        let mut keymaps = Keymaps::initial();
        keymaps.get_mut(1).and_then(|k| k.add_layer("aim"));
        // Run as the device's own session, because tray icons are followed only there.
        let lights = clamshell_state::lights::LightSets::initial();
        let mut state = State::new(Init { library: Some(clamshell_library::demo::library()), keymaps, lights, device_session: true, ..Init::default() });
        state.apply(Event::tick(1_700_000_000), &mut Vec::new());
        // The time tab's zone in every case, since a change of zone from one case to the next is read
        // anew.
        state.apply(Event::Reading(Reading::Time(TimeZones { current: Some("Europe/Oslo".into()), ..TimeZones::default() })), &mut Vec::new());
        state
    };
    let mut last = base();
    for (name, events) in cases {
        let mut state = base();
        for event in events {
            state.apply(event, &mut Vec::new());
        }
        if let Some((_, check)) = reached.iter().find(|(n, _)| *n == name) {
            assert!(check(&state), "{name}: not reached");
        }
        // Each round goes back to the last case's state and then to this one's, so that both frames
        // change part of the screen.
        for round in 0..3 {
            let before = allocs();
            for s in [&last, &state] {
                top.draw(s, &mut cache, &mut images);
                bottom.draw(s, &mut cache, &mut images);
                overlay.draw(s, &mut cache, &mut images);
                below.draw(s, &mut cache, &mut images);
            }
            let n = allocs() - before;
            // The first rounds fill the glyph cache and size the vectors.
            if round == 2 {
                assert_eq!(n, 0, "{name}: {n} allocations in a warm frame");
            }
        }
        last = state;
    }
}

/// Input on a warm core allocates nothing either: a finger tapping a tab, sliding the volume in the bar,
/// and swiping the Wi-Fi list; the pad moving the focus; and a held key repeating on its timer.
#[test]
fn warm_input_does_not_allocate() {
    use clamshell_state::PadInput;
    use clamshell_ui::input::{End, Source};
    let state = State::new(Init { library: Some(clamshell_library::demo::library()), account: account(), ..Init::default() });
    let mut core = clamshell_ui::Core::new(state, crate::VIEWS, || {}).expect("a core");
    let (top_size, bottom_size) = ((1920, 1080), (1240, 1080));
    core.resize(Screen::Top, top_size, 1);
    core.resize(Screen::Bottom, bottom_size, 1);
    core.handle(Event::tick(1_760_000_000));
    core.handle(wifi_reading());
    core.handle(Event::Reading(Reading::Status(Status { volume: Some(40), brightness: Some(70), wifi: Some(60), ..Default::default() })));
    core.handle(tap(Id::Tab(Tab::Wifi)));
    let mut frames = clamshell_ui::PadFrames::new(crate::VIEWS, top_size, bottom_size);
    let layers = [
        clamshell_ui::Layer::Screen(Screen::Top),
        clamshell_ui::Layer::Screen(Screen::Bottom),
        clamshell_ui::Layer::Overlay(Screen::Top),
        clamshell_ui::Layer::Overlay(Screen::Bottom),
    ];
    let draw = |core: &mut clamshell_ui::Core| {
        for layer in layers {
            core.record(layer, false);
        }
        core.frames_done();
        while core.next_effect().is_some() {}
    };
    draw(&mut core);
    let centre = |id: &Id, core: &clamshell_ui::Core, frames: &mut clamshell_ui::PadFrames| {
        let drawn = frames.record(core.state()).drawn(Screen::Bottom);
        let r = drawn.iter().find(|t| t.id == *id).map(|t| t.rect).expect("drawn");
        (f64::from(r[0] + r[2] / 2.0), f64::from(r[1] + r[3] / 2.0))
    };
    let wifi_tab = centre(&Id::Tab(Tab::Wifi), &core, &mut frames);
    let volume = centre(&Id::Control(Control::Volume), &core, &mut frames);
    let row = centre(&ListId::Wifi.row(1), &core, &mut frames);
    let finger = Source::Touch(1);
    let mut t = 1000u32;
    type Case = Box<dyn FnMut(&mut clamshell_ui::Core)>;
    let mut cases: Vec<(&str, Case)> = vec![
        (
            "a tap on a tab",
            Box::new(move |core| {
                core.press(finger, Screen::Bottom, wifi_tab, t);
                let now = core.at(t);
                core.up(finger, End::Release, now);
            }),
        ),
        (
            "the volume slid",
            Box::new(move |core| {
                t += 100;
                core.press(finger, Screen::Bottom, volume, t);
                for i in 1..=6 {
                    core.moved(finger, (volume.0 + 20.0 * f64::from(i), volume.1), t + 8 * i);
                }
                let now = core.at(t + 60);
                core.up(finger, End::Release, now);
            }),
        ),
        (
            "a list swiped",
            Box::new(move |core| {
                t += 100;
                core.press(finger, Screen::Bottom, row, t);
                for i in 1..=6 {
                    core.moved(finger, (row.0, row.1 + 10.0 * f64::from(i)), t + 8 * i);
                }
                let now = core.at(t + 300);
                core.up(finger, End::Cancel, now);
            }),
        ),
        (
            "the pad's focus moved",
            Box::new(|core| {
                core.handle(Event::Pad(PadInput::Down));
                core.handle(Event::Pad(PadInput::Up));
            }),
        ),
        (
            "the pad's A on a tab",
            Box::new(|core| {
                core.handle(PointerEvent::Focus(Id::Tab(Tab::Wifi)).into());
                core.handle(Event::Pad(PadInput::A(true)));
                core.handle(Event::Pad(PadInput::A(false)));
            }),
        ),
        (
            "the shoulders",
            Box::new(|core| {
                core.handle(Event::Pad(PadInput::Shoulder(1)));
                core.handle(Event::Pad(PadInput::Shoulder(-1)));
            }),
        ),
        (
            "a held press on its timer",
            Box::new(move |core| {
                t += 1000;
                core.press(finger, Screen::Bottom, wifi_tab, t);
                if let Some(at) = core.deadline() {
                    core.run(at);
                }
                let now = core.at(t + 600);
                core.up(finger, End::Release, now);
            }),
        ),
    ];
    let mut failed = Vec::new();
    for (name, case) in &mut cases {
        for _ in 0..3 {
            case(&mut core);
            draw(&mut core);
        }
        let before = allocs();
        case(&mut core);
        let n = allocs() - before;
        draw(&mut core);
        eprintln!("{name}: {n}");
        if n > 0 {
            failed.push(*name);
        }
    }
    assert!(failed.is_empty(), "allocated: {failed:?}");
}
