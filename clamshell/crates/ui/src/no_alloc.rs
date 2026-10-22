//! Drawing a frame must not touch the heap once caches are warm (glyphs rasterized, buffers sized):
//! counted with an allocator that tallies this thread's allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_core::device::{DeviceId, SticksEvent};
use clamshell_core::keyboard::{self, Key, Modifier};
use clamshell_core::{DockId, EditorId, Event, Id, Init, Instance, PointerEvent, Reading, SettingsId, Slide, State, Tab};

use crate::Screen;
use crate::draw::DrawCache;
use crate::render::{Picture, Placement, render};

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

fn window(id: i64, app_id: &str, unit: Option<&str>, focused: bool) -> clamshell_core::windows::Window {
    let workspace = unit.map_or("1", |u| u.trim_end_matches(".scope")).to_owned();
    clamshell_core::windows::Window { id, app_id: app_id.into(), focused, unit: unit.map(Into::into), workspace, ..Default::default() }
}

fn key(k: Key) -> Event {
    Event::Pointer(PointerEvent::Activate(Id::Key(keyboard::find(k).expect("key on the layout"))))
}

/// Shelf's Tools collection opened and its Device entry run: the Device view.
fn device() -> Vec<Event> {
    let library = clamshell_library::demo::library();
    let tools = library.collections.iter().position(|c| c.name == "Tools").unwrap_or(0);
    let entry = || tap(Id::Shelf(clamshell_core::ShelfId::Game("tool:device".into())));
    vec![tap(Id::Shelf(clamshell_core::ShelfId::Collection(u16::try_from(tools).unwrap_or(0)))), entry(), entry()]
}

fn tap(id: Id) -> Event {
    Event::Pointer(PointerEvent::Activate(id))
}

fn wifi_reading() -> Event {
    Event::Reading(Reading::Wifi(clamshell_core::wifi::Wifi {
        device: true,
        enabled: true,
        networks: vec![
            clamshell_core::wifi::Network { ssid: "home".into(), strength: 80, secure: true, known: true, active: true, enterprise: false },
            clamshell_core::wifi::Network { ssid: "cafe".into(), strength: 40, secure: true, ..Default::default() },
        ],
        connecting: Some("cafe".into()),
        error: None,
    }))
}

fn account() -> clamshell_core::account::Account {
    clamshell_core::account::Account { available: true, password: true, ssh: true }
}

/// Each case from the same base state (the demo library, a fixed time), so they don't depend on each
/// other.
#[test]
fn warm_frames_do_not_allocate() {
    let mut cache = DrawCache::load();
    let mut images = crate::images::Images::synchronous();
    let (mut top, mut bottom) = (Picture::default(), Picture::default());
    let top_at = Placement { size_px: (1920, 1080), scale: 1, drift: (1, -2) };
    let bottom_at = Placement { size_px: (1240, 1080), scale: 1, drift: (-3, 0) };

    let cases: Vec<(&str, Vec<Event>)> = vec![
        ("sysinfo", vec![]),
        ("settings", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Settings)))]),
        ("settings, a changed tile", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::FpsCap)))]),
        ("settings, all games", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Scope(clamshell_core::Scope::Global)))]),
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
                Event::Reading(Reading::Status(clamshell_core::Status { volume: Some(40), brightness: Some(70), wifi: Some(60), ..Default::default() })),
                tap(Id::Tab(Tab::Wifi)),
                wifi_reading(),
            ],
        ),
        (
            "wifi, scrolled part way",
            vec![
                tap(Id::Tab(Tab::Wifi)),
                Event::Reading(Reading::Wifi(clamshell_core::wifi::Wifi {
                    device: true,
                    enabled: true,
                    networks: (0..20).map(|i| clamshell_core::wifi::Network { ssid: format!("n{i}"), strength: 50, ..Default::default() }).collect(),
                    ..Default::default()
                })),
                Event::List(clamshell_core::ListEvent::Visible(clamshell_core::ListId::Wifi, 6.5)),
                Event::List(clamshell_core::ListEvent::Scroll(clamshell_core::ListId::Wifi, 3.3)),
            ],
        ),
        (
            "shelf, dragged part way",
            vec![Event::Shelf(clamshell_core::ShelfEvent::Rows(3)), Event::List(clamshell_core::ListEvent::Scroll(clamshell_core::ListId::Shelf, 1.6))],
        ),
        ("keymap picker", vec![tap(Id::Tab(Tab::Settings)), tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap)))]),
        (
            "keymap edit screen",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap))),
                tap(Id::Row { list: clamshell_core::ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit }),
            ],
        ),
        (
            "keymap mouse",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap))),
                tap(Id::Row { list: clamshell_core::ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit }),
                tap(Id::Keymap(clamshell_core::KeymapId::Input(clamshell_core::keymap::Input::Trigger(clamshell_core::keymap::Side::Left)))),
                tap(Id::Keymap(clamshell_core::KeymapId::Target(clamshell_core::Target::MouseParts))),
            ],
        ),
        (
            "keymap picking a control",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap))),
                tap(Id::Row { list: clamshell_core::ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit }),
                tap(Id::Keymap(clamshell_core::KeymapId::Input(clamshell_core::keymap::Input::Button(clamshell_core::keymap::Button::A)))),
                tap(Id::Keymap(clamshell_core::KeymapId::Target(clamshell_core::Target::Pick))),
            ],
        ),
        (
            "keymap panel, a slider up, another layer",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap))),
                tap(Id::Row { list: clamshell_core::ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit }),
                tap(Id::Keymap(clamshell_core::KeymapId::Layer(clamshell_core::keymap::Layer(1)))),
                tap(Id::Keymap(clamshell_core::KeymapId::Input(clamshell_core::keymap::Input::Stick(clamshell_core::keymap::Side::Left)))),
                Event::Slide(Slide::Start { control: clamshell_core::slider::Control::Param(clamshell_core::keymap::Param::Deadzone), x: 285.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 320.0 }),
            ],
        ),
        (
            "keymap key capture",
            vec![
                tap(Id::Tab(Tab::Settings)),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::Keymap))),
                tap(Id::Row { list: clamshell_core::ListId::Keymaps, i: 1, part: clamshell_core::RowPart::Edit }),
                tap(Id::Keymap(clamshell_core::KeymapId::Input(clamshell_core::keymap::Input::Stick(clamshell_core::keymap::Side::Left)))),
                tap(Id::Keymap(clamshell_core::KeymapId::Target(clamshell_core::Target::Keys))),
                key(Key::Char('w')),
                key(Key::Mod(Modifier::Shift)),
            ],
        ),
        ("wifi password", vec![tap(Id::Tab(Tab::Wifi)), wifi_reading(), tap(clamshell_core::ListId::Wifi.row(1)), tap(clamshell_core::ListId::Wifi.row(0))]),
        (
            "sliding",
            vec![
                Event::Reading(Reading::Status(clamshell_core::Status { volume: Some(40), ..Default::default() })),
                Event::Pointer(PointerEvent::Down(Id::Control(clamshell_core::slider::Control::Volume))),
                Event::Slide(Slide::Start { control: clamshell_core::slider::Control::Volume, x: 285.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 320.0 }),
            ],
        ),
        (
            "battery",
            vec![
                Event::Reading(Reading::Status(clamshell_core::Status { battery: Some(41), ..Default::default() })),
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Battery))),
                Event::Reading(Reading::Battery(clamshell_core::battery::Battery {
                    history: (0..50u64)
                        .map(|i| clamshell_core::battery::Charge { at: 1_699_950_000 + i * 900, level: (90 - i) as u8, charging: i > 40 })
                        .collect(),
                    spenders: vec![
                        clamshell_core::battery::Spender {
                            group: clamshell_core::battery::Group::Process("firefox".into()),
                            cpu: 0.2,
                            gpu: 0.1,
                            memory: 300 << 20,
                            pids: vec![],
                            endable: true,
                        },
                        clamshell_core::battery::Spender {
                            group: clamshell_core::battery::Group::System,
                            cpu: 0.05,
                            gpu: 0.0,
                            memory: 600 << 20,
                            pids: vec![],
                            endable: false,
                        },
                    ],
                    has_history: true,
                })),
            ],
        ),
        (
            "bluetooth",
            vec![
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Bluetooth))),
                Event::Reading(Reading::Bluetooth(clamshell_core::bluetooth::Bluetooth {
                    adapter: true,
                    powered: true,
                    devices: vec![clamshell_core::bluetooth::Device {
                        address: "00:11".into(),
                        name: "buds".into(),
                        kind: clamshell_core::bluetooth::Kind::Audio,
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
                Event::Reading(Reading::Audio(clamshell_core::levels::Audio {
                    outputs: vec![clamshell_core::levels::Output { id: 1, name: "Speakers".into(), default: true }],
                    streams: vec![clamshell_core::levels::Stream { id: 2, app: "Firefox".into(), title: String::new(), volume: 40, muted: false }],
                })),
                Event::Pointer(PointerEvent::Down(clamshell_core::levels::Level::Stream(0).id())),
            ],
        ),
        ("brightness", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Brightness))), Event::Reading(Reading::Backlights(vec!["panel".into()]))]),
        (
            "dark while a game runs",
            vec![
                Event::Units(vec![Instance::Run { key: clamshell_library::key_of("zelda-lttp"), secs: 1, n: 0 }]),
                tap(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::BottomScreen))),
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
            [
                device(),
                vec![tap(Id::Device(DeviceId::Sticks)), Event::Sticks(SticksEvent::Progress(clamshell_core::sticks::Calibrator::new(-1024, 1024).progress()))],
            ]
            .concat(),
        ),
        ("keyboard, Fn", vec![tap(Id::Tab(Tab::Keyboard)), key(Key::Mod(Modifier::Fn))]),
        (
            "keyboard, Shift and a pressed key",
            vec![
                tap(Id::Tab(Tab::Keyboard)),
                key(Key::Mod(Modifier::Shift)),
                Event::Pointer(PointerEvent::Down(Id::Key(keyboard::find(Key::Char('h')).expect("h")))),
            ],
        ),
    ];
    for (name, events) in cases {
        // (The Nintendo layout with a layer added.)
        let mut keymaps = clamshell_core::keymap::Keymaps::initial();
        keymaps.get_mut(1).and_then(|k| k.add_layer("aim"));
        let mut state = State::new(Init { library: clamshell_library::demo::library(), keymaps, ..Init::default() });
        state.apply(Event::tick(1_700_000_000));
        for event in events {
            state.apply(event);
        }
        for round in 0..3 {
            let before = allocs();
            render(&mut top, top_at, Screen::Top, &state, &mut cache, &mut images).expect("top");
            render(&mut bottom, bottom_at, Screen::Bottom, &state, &mut cache, &mut images).expect("bottom");
            let n = allocs() - before;
            // The first rounds fill the glyph cache and size the vectors.
            if round == 2 {
                assert_eq!(n, 0, "{name}: {n} allocations in a warm frame");
            }
        }
    }
}
