//! Drawing a frame must not touch the heap once caches are warm (glyphs rasterized, buffers sized):
//! counted with an allocator that tallies this thread's allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_core::keyboard::{self, Key, Modifier};
use clamshell_core::{DockId, EditorId, Event, GameEvent, Id, Instance, PointerEvent, Reading, SettingsId, Slide, Init, State, Tab, WifiId};
use tiny_skia::Pixmap;

use crate::draw::{Canvas, DrawCache};
use crate::views::{self, Scratch};

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
    clamshell_core::windows::Window { id, app_id: app_id.into(), title: String::new(), focused, unit: unit.map(Into::into), workspace }
}

fn key(k: Key) -> Event {
    Event::Pointer(PointerEvent::Activate(Id::Key(keyboard::find(k).expect("key on the layout"))))
}

#[test]
fn warm_frames_do_not_allocate() {
    let mut state = State::new(Init { library: clamshell_library::demo_library(), ..Init::default() });
    state.apply(Event::Tick { unix_secs: 1_700_000_000 });
    let mut cache = DrawCache::load();
    let mut scratch = Scratch::default();
    let mut images = crate::images::Images::synchronous();
    let mut targets = Vec::new();
    let mut top = Pixmap::new(1920, 1080).expect("pixmap");
    let mut bottom = Pixmap::new(1240, 1080).expect("pixmap");

    let cases: Vec<(&str, Vec<Event>)> = vec![
        ("sysinfo", vec![]),
        ("settings", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Settings)))]),
        ("settings, a changed tile", vec![Event::Pointer(PointerEvent::Activate(Id::Settings(SettingsId::Tile(clamshell_core::settings::Setting::FpsCap))))]),
        ("settings, all games", vec![Event::Pointer(PointerEvent::Activate(Id::Settings(SettingsId::Scope(clamshell_core::Scope::Global))))]),
        ("environment editor", vec![Event::Pointer(PointerEvent::Activate(Id::Settings(SettingsId::Env))), key(Key::Char('a'))]),
        ("environment saved", vec![key(Key::Char('=')), key(Key::Char('1')), Event::Pointer(PointerEvent::Activate(Id::Editor(EditorId::Save)))]),
        ("dock, an app running", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Dock))), Event::Pointer(PointerEvent::Activate(Id::Dock(DockId::App(1))))]),
        (
            "dock, a game and windows",
            vec![
                Event::Game(GameEvent::Started("zelda-lttp".into())),
                Event::Units(["clamshell-game-1-0", "clamshell-app-terminal-1-0"].into_iter().filter_map(Instance::parse).collect()),
                Event::Windows(vec![
                    window(5, "retroarch", Some("clamshell-game-1-0.scope"), false),
                    window(7, "foot", Some("clamshell-app-terminal-1-0.scope"), true),
                    window(8, "foot", None, false),
                ]),
            ],
        ),
        (
            "wifi",
            vec![
                Event::Reading(Reading::Status(clamshell_core::Status { volume: Some(40), brightness: Some(70), wifi: Some(60), ..Default::default() })),
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Wifi))),
                Event::Reading(Reading::Wifi(clamshell_core::wifi::Wifi {
                    device: true,
                    enabled: true,
                    networks: vec![
                        clamshell_core::wifi::Network { ssid: "home".into(), strength: 80, secure: true, known: true, active: true, enterprise: false },
                        clamshell_core::wifi::Network { ssid: "cafe".into(), strength: 40, secure: true, ..Default::default() },
                    ],
                    connecting: Some("cafe".into()),
                    error: None,
                })),
            ],
        ),
        ("wifi password", vec![Event::Pointer(PointerEvent::Activate(Id::Wifi(WifiId::Network(1)))), Event::Pointer(PointerEvent::Activate(Id::Wifi(WifiId::Network(0))))]),
        (
            "sliding",
            vec![
                Event::Pointer(PointerEvent::Activate(Id::Editor(EditorId::Cancel))),
                Event::Pointer(PointerEvent::Down(Id::Control(clamshell_core::slider::Control::Volume))),
                Event::Slide(Slide::Start { control: clamshell_core::slider::Control::Volume, x: 285.0, width: 620.0 }),
                Event::Slide(Slide::To { x: 320.0 }),
            ],
        ),
        (
            "battery",
            vec![
                Event::Pointer(PointerEvent::Up(Id::Control(clamshell_core::slider::Control::Volume))),
                Event::Reading(Reading::Status(clamshell_core::Status { battery: Some(41), ..Default::default() })),
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Battery))),
                Event::Reading(Reading::Battery(clamshell_core::battery::Battery {
                    history: (0..50u64)
                        .map(|i| clamshell_core::battery::Charge { at: 1_699_950_000 + i * 900, level: (90 - i) as u8, charging: i > 40 })
                        .collect(),
                    spenders: vec![
                        clamshell_core::battery::Spender { group: clamshell_core::battery::Group::Process("firefox".into()), cpu: 0.2, gpu: 0.1 },
                        clamshell_core::battery::Spender { group: clamshell_core::battery::Group::System, cpu: 0.05, gpu: 0.0 },
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
                Event::Pointer(PointerEvent::Down(Id::Level(clamshell_core::levels::Level::Stream(0)))),
            ],
        ),
        (
            "brightness",
            vec![
                Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Brightness))),
                Event::Reading(Reading::Backlights(vec!["panel".into()])),
            ],
        ),
        ("keyboard", vec![Event::Pointer(PointerEvent::Activate(Id::Tab(Tab::Keyboard)))]),
        ("keyboard, Fn", vec![key(Key::Mod(Modifier::Fn))]),
        ("keyboard, Shift and a pressed key", vec![key(Key::Mod(Modifier::Shift)), key(Key::Char('h'))]),
    ];
    for (name, events) in cases {
        for event in events {
            state.apply(event);
        }
        for round in 0..3 {
            let before = allocs();
            views::top(
                &mut Canvas::new(&mut top, 1.0, &mut cache),
                &state,
                1920.0,
                1080.0,
                &mut images,
                &mut scratch,
                &mut targets,
            );
            views::bottom(
                &mut Canvas::new(&mut bottom, 1.0, &mut cache),
                &state,
                1240.0,
                1080.0,
                &mut images,
                &mut scratch,
                &mut targets,
            );
            let n = allocs() - before;
            // The first rounds fill the glyph cache and size the vectors.
            if round == 2 {
                assert_eq!(n, 0, "{name}: {n} allocations in a warm frame");
            }
        }
    }
}
