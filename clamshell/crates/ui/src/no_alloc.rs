//! Drawing a frame must not touch the heap once caches are warm (glyphs rasterized, buffers sized):
//! counted with an allocator that tallies this thread's allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_core::Control;
use clamshell_core::DeviceId;
use clamshell_core::bottom::keyboard::{self, Key, Modifier};
use clamshell_core::device::SticksEvent;
use clamshell_core::device::account::Account;
use clamshell_core::device::sticks::Calibrator;
use clamshell_core::keymap::{Button, Input, Keymaps, Layer, Param, Side};
use clamshell_core::radios::{bluetooth, wifi};
use clamshell_core::session::tray;
use clamshell_core::session::windows::Window;
use clamshell_core::settings::Setting;
use clamshell_core::status::battery;
use clamshell_core::{
    DockId, EditorId, Event, Id, Init, Instance, KeymapId, ListEvent, ListId, PointerEvent, Reading, RowPart, Scope, SettingsId, ShelfEvent, ShelfId, Slide,
    State, Status, Tab, Target, TimeZones, levels,
};

use crate::Screen;
use crate::draw::DrawCache;
use crate::render::{Placement, Retained, render};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: forwards to the system allocator unchanged; only counts calls.
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

fn window(id: i64, app_id: &str, unit: Option<&str>, focused: bool) -> Window {
    let workspace = unit.map_or("1", |u| u.trim_end_matches(".scope")).to_owned();
    Window { id, app_id: app_id.into(), focused, unit: unit.map(Into::into), workspace, ..Default::default() }
}

fn key(k: Key) -> Event {
    Event::Pointer(PointerEvent::Activate(Id::Key(keyboard::find(k).expect("key on the layout"))))
}

/// Shelf's Tools collection opened and its Device entry run: the Device view.
fn device() -> Vec<Event> {
    let library = clamshell_library::demo::library();
    let tools = library.collections.iter().position(|c| c.name == "Tools").unwrap_or(0);
    let entry = || tap(Id::Shelf(ShelfId::Entry("tool:device".into())));
    vec![tap(Id::Shelf(ShelfId::Collection(u16::try_from(tools).unwrap_or(0)))), entry(), entry()]
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

fn account() -> Account {
    Account { available: true, password: true, ssh: true }
}

/// Each case from the same base state (the demo library, a fixed time), so they don't depend on each
/// other.
#[test]
fn warm_frames_do_not_allocate() {
    let mut cache = DrawCache::load();
    let mut images = crate::images::Images::synchronous();
    let (mut top, mut bottom) = (Retained::default(), Retained::default());
    let (mut top_px, mut bottom_px) = (tiny_skia::Pixmap::new(1920, 1080).expect("pixmap"), tiny_skia::Pixmap::new(1240, 1080).expect("pixmap"));
    let top_at = Placement { size_px: (1920, 1080), scale: 1, drift: (1, -2) };
    let bottom_at = Placement { size_px: (1240, 1080), scale: 1, drift: (-3, 0) };

    let cases: Vec<(&str, Vec<Event>)> = vec![
        ("sysinfo", vec![]),
        ("settings", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Settings)))]),
        ("settings, a changed tile", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::FpsCap)))]),
        ("settings, all games", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Scope(Scope::Global)))]),
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
                Event::Units(vec![Instance::Run { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 }]),
                Event::Units(["clamshell-run-zelda_1-1-0", "clamshell-run-foot_1-1-0"].into_iter().filter_map(Instance::parse).collect()),
                Event::Windows(vec![
                    window(5, "retroarch", Some("clamshell-run-zelda_1-1-0.scope"), false),
                    window(7, "foot", Some("clamshell-run-foot_1-1-0.scope"), true),
                    window(8, "foot", None, false),
                ]),
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
        ("shelf, dragged part way", vec![Event::Shelf(ShelfEvent::Rows(3)), Event::List(ListEvent::Scroll(ListId::Shelf, 1.6))]),
        ("keymap picker", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::Keymap)))]),
        (
            "keymap edit screen",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
            ],
        ),
        (
            "keymap mouse",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Trigger(Side::Left)))),
                tap(Id::Keymap(KeymapId::Target(Target::MouseParts))),
            ],
        ),
        (
            "keymap picking a control",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Button(Button::A)))),
                tap(Id::Keymap(KeymapId::Target(Target::Pick))),
            ],
        ),
        (
            "keymap panel, a slider up, another layer",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
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
                tap(Id::Settings(SettingsId::Tile(Setting::Keymap))),
                tap(Id::Row { list: ListId::Keymaps, i: 1, part: RowPart::Edit }),
                tap(Id::Keymap(KeymapId::Input(Input::Stick(Side::Left)))),
                tap(Id::Keymap(KeymapId::Target(Target::Keys))),
                key(Key::Char('w')),
                key(Key::Mod(Modifier::Shift)),
            ],
        ),
        ("wifi password", vec![tap(Id::Tab(Tab::Wifi)), wifi_reading(), tap(ListId::Wifi.row(1)), tap(ListId::Wifi.row(0))]),
        (
            "sliding",
            vec![
                Event::Reading(Reading::Status(Status { volume: Some(40), ..Default::default() })),
                Event::Pointer(PointerEvent::Down(Id::Control(Control::Volume))),
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
                })),
                Event::Pointer(PointerEvent::Down(levels::Level::Stream(0).id())),
            ],
        ),
        ("brightness", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Brightness))), Event::Reading(Reading::Backlights(vec!["panel".into()]))]),
        (
            "dark while a game runs",
            vec![
                Event::Units(vec![Instance::Run { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 }]),
                tap(Id::Settings(SettingsId::Tile(Setting::BottomScreen))),
            ],
        ),
        ("keyboard", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Keyboard)))]),
        ("device", [device(), vec![Event::Reading(Reading::Account(account()))]].concat()),
        (
            "device, the password",
            [device(), vec![Event::Reading(Reading::Account(account())), tap(Id::Device(DeviceId::Password)), key(Key::Char('a'))]].concat(),
        ),
        (
            "device, the sticks calibrating",
            [device(), vec![tap(Id::Device(DeviceId::Sticks)), Event::Sticks(SticksEvent::Progress(Calibrator::new(-1024, 1024).progress()))]].concat(),
        ),
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
            "keymap name editor",
            vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(Setting::Keymap))), tap(Id::Keymap(KeymapId::New)), key(Key::Char('a'))],
        ),
        // (The demo's TV tab: its series, open.)
        ("shelf, an open group", vec![Event::Shelf(ShelfEvent::Rows(8)), tap(Id::Shelf(ShelfId::Collection(2))), tap(Id::Shelf(ShelfId::Launch))]),
        (
            "shelf, a video",
            vec![
                Event::Shelf(ShelfEvent::Rows(8)),
                tap(Id::Shelf(ShelfId::Collection(2))),
                tap(Id::Shelf(ShelfId::Launch)),
                tap(Id::Shelf(ShelfId::Launch)),
                Event::Shelf(ShelfEvent::ListStep(1)),
            ],
        ),
        (
            "dock, an entry's menu",
            vec![
                tap(Id::Tab(Tab::Dock)),
                Event::Units(vec![Instance::Run { key: clamshell_library::key_of("desktop:foot"), secs: 1, n: 0 }]),
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
                tap(Id::Battery(clamshell_core::BatteryId::Spender(0))),
            ],
        ),
        (
            "keyboard, Shift and a pressed key",
            vec![
                tap(Id::Tab(Tab::Keyboard)),
                key(Key::Mod(Modifier::Shift)),
                Event::Pointer(PointerEvent::Down(Id::Key(keyboard::find(Key::Char('h')).expect("h")))),
            ],
        ),
    ];
    // What the cases that are easy to get wrong must have reached, to test the screen they are named for.
    type Reached = fn(&State) -> bool;
    let reached: [(&str, Reached); 7] = [
        ("keymap name editor", |s| s.bottom().editor().is_some()),
        ("shelf, an open group", |s| s.shelf().group().is_some()),
        ("shelf, a video", |s| s.shelf().selected().is_some_and(|id| id.ends_with(".mkv"))),
        ("dock, an entry's menu", |s| s.dock().menu().is_some() && !s.dock().tray_open()),
        ("dock, a tray menu", |s| s.dock().tray_open()),
        ("battery, a program's menu", |s| s.spender_menu().is_some()),
        ("time", |s| s.bottom().tab() == Tab::Time),
    ];
    for (name, events) in cases {
        // (The Nintendo layout with a layer added.)
        let mut keymaps = Keymaps::initial();
        keymaps.get_mut(1).and_then(|k| k.add_layer("aim"));
        // (As the device's own session: tray icons are followed only there.)
        let mut state = State::new(Init { library: clamshell_library::demo::library(), keymaps, device_session: true, ..Init::default() });
        state.apply(Event::tick(1_700_000_000));
        for event in events {
            state.apply(event);
        }
        if let Some((_, check)) = reached.iter().find(|(n, _)| *n == name) {
            assert!(check(&state), "{name}: not reached");
        }
        for round in 0..3 {
            let before = allocs();
            render(top_px.as_mut(), &mut top, top_at, Screen::Top, &state, &mut cache, &mut images).expect("top");
            render(bottom_px.as_mut(), &mut bottom, bottom_at, Screen::Bottom, &state, &mut cache, &mut images).expect("bottom");
            let n = allocs() - before;
            // The first rounds fill the glyph cache and size the vectors.
            if round == 2 {
                assert_eq!(n, 0, "{name}: {n} allocations in a warm frame");
            }
        }
    }
}
