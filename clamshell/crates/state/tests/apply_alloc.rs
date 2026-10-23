//! `State::apply` allocates nothing for the events that come all the time, once the state is warm: ticks,
//! readings, the windows and scopes listed again, presses and the pad's focus. The counts are printed
//! (`cargo test -p clamshell-state --test apply_alloc -- --nocapture`), and with CLAMSHELL_ALLOC_TRACE set,
//! where each allocation came from.
#![allow(clippy::unwrap_used, unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_state::radios::wifi::{Network, Wifi};
use clamshell_state::session::windows::Window;
use clamshell_state::status::metrics::Metrics;
use clamshell_state::{Effect, EntryAct, Event, Id, Init, Instance, ListEvent, ListId, PointerEvent, Reading, State, Status, Tab, WindowEvent};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static TRACE: Cell<bool> = const { Cell::new(false) };
}

/// With CLAMSHELL_ALLOC_TRACE set, where each counted allocation came from (this crate's frames only).
fn trace() {
    if TRACE.with(Cell::get) {
        return;
    }
    TRACE.with(|t| t.set(true));
    if std::env::var_os("CLAMSHELL_ALLOC_TRACE").is_none() {
        TRACE.with(|t| t.set(false));
        return;
    }
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = bt.lines().filter(|l| l.contains("clamshell_state::") && !l.contains("apply_alloc")).map(str::trim).take(8).collect();
    eprintln!("  alloc at: {}", frames.join(" <- "));
    TRACE.with(|t| t.set(false));
}

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

struct Counting;

// SAFETY: forwards to the system allocator unchanged and only counts the calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        if MEASURING.try_with(Cell::get).unwrap_or(false) {
            trace();
        }
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

/// Allocations made while applying `event` (built beforehand, so building it doesn't count).
fn count(s: &mut State, event: Event, effects: &mut Vec<Effect>) -> usize {
    effects.clear();
    let before = ALLOCS.with(Cell::get);
    MEASURING.with(|m| m.set(true));
    s.apply(event, effects);
    MEASURING.with(|m| m.set(false));
    ALLOCS.with(Cell::get) - before
}

/// A warm state: up, with two programs of ours running, each with a window, one of them in front, and a
/// terminal of the user's on another workspace.
fn warm() -> (State, Vec<Window>, Vec<Instance>) {
    let window_rules = clamshell_state::bottom::screen::WindowRules::from_list(vec![clamshell_state::bottom::screen::WindowRule {
        owner: "desktop:foot".into(),
        app_id: "foot".into(),
        title: Some("lower".into()),
    }]);
    let mut s = State::new(Init { library: clamshell_library::demo::library(), window_rules, ..Init::default() });
    let mut effects = Vec::new();
    s.apply(Event::Start, &mut effects);
    s.apply(Event::Tick { unix_secs: T0, mono_secs: 10 }, &mut effects);
    let mut units = Vec::new();
    let mut windows = vec![Window { id: 1, app_id: "foot".into(), workspace: "1".into(), ..Window::default() }];
    for (n, id) in ["desktop:firefox", "desktop:foot"].into_iter().enumerate() {
        effects.clear();
        s.apply(Event::Entry(id.into(), EntryAct::Play), &mut effects);
        let instance = effects
            .iter()
            .find_map(|e| match e {
                Effect::Launch { instance, .. } => Some(instance.clone()),
                _ => None,
            })
            .unwrap();
        units.push(instance.clone());
        s.apply(Event::Units(units.clone()), &mut effects);
        for w in &mut windows {
            w.focused = false;
        }
        windows.push(Window {
            id: 10 + n as i64,
            app_id: id.trim_start_matches("desktop:").into(),
            unit: Some(format!("{}.scope", instance.name())),
            workspace: instance.workspace(),
            focused: true,
            visible: true,
            ..Window::default()
        });
        s.apply(Event::window_changes(s.session().windows(), &windows), &mut effects);
    }
    assert_eq!(s.session().running_entries().len(), 2, "both programs run");
    (s, windows, units)
}

const T0: u64 = 1_760_000_000;

#[test]
fn allocations_per_event() {
    let mut rows = Vec::new();
    let mut effects = Vec::new();
    let mut run = |name: &str, s: &mut State, make: &dyn Fn(u64) -> Event| {
        // Warm up with the same kind of event, then measure the next few.
        for i in 0..3 {
            effects.clear();
            s.apply(make(i), &mut effects);
        }
        let n = (3..6).map(|i| count(s, make(i), &mut effects)).max().unwrap_or(0);
        rows.push((name.to_owned(), n));
    };
    let mut s = State::new(Init { library: clamshell_library::demo::library(), ..Init::default() });
    s.apply(Event::Start, &mut Vec::new());
    let (mut w, windows, units) = warm();
    for (state, s) in [("idle", &mut s), ("running", &mut w)] {
        // A list of networks, longer than the 3 rows shown.
        let networks: Vec<Network> = (0..8).map(|i| Network { ssid: format!("network {i}"), strength: 50, ..Network::default() }).collect();
        let wifi = Wifi { present: true, on: true, networks, ..Wifi::default() };
        s.apply(Event::Reading(Reading::Wifi(wifi.clone())), &mut Vec::new());
        s.apply(ListEvent::Visible(ListId::Wifi, 3.0).into(), &mut Vec::new());
        let windows = if state == "idle" { Vec::new() } else { windows.clone() };
        let units = if state == "idle" { Vec::new() } else { units.clone() };
        let name = |what: &str| format!("{state}: {what}");
        run(&name("tick, same minute"), s, &|i| Event::Tick { unix_secs: T0 + 1000 + i, mono_secs: 1000 + i });
        run(&name("tick, a new minute each"), s, &|i| Event::Tick { unix_secs: T0 + 2000 + 60 * i, mono_secs: 2000 + 60 * i });
        run(&name("status reading, unchanged"), s, &|_| Event::Reading(Reading::Status(Status::default())));
        run(&name("status reading, changing"), s, &|i| Event::Reading(Reading::Status(Status { battery: Some(50 + i as u8 % 2), ..Status::default() })));
        run(&name("metrics reading"), s, &|i| Event::Reading(Reading::Metrics(Metrics { cpu: Some(i as f32 / 10.0), ..Metrics::default() })));
        run(&name("frames reading"), s, &|i| Event::Reading(Reading::Frames { fps: Some(60.0 + i as f32), frame_ms: None }));
        run(&name("wifi reading, unchanged"), s, &|_| Event::Reading(Reading::Wifi(wifi.clone())));
        run(&name("windows, unchanged"), s, &|_| Event::window_changes(&windows, &windows));
        if let Some(window) = windows.first() {
            run(&name("window open, repeated"), s, &|_| WindowEvent::Opened(window.clone()).into());
            run(&name("window title"), s, &|i| WindowEvent::Title { window: window.id, title: format!("title {i}") }.into());
            run(&name("window facts"), s, &|_| WindowEvent::Changed(window.clone()).into());
            run(&name("window activation, already shown"), s, &|_| WindowEvent::Activate(window.id).into());
            run(&name("keyboard focus, UI and client"), s, &|i| WindowEvent::KeyboardFocus((i % 2 == 0).then_some(window.id)).into());
        }
        run(&name("window batch, reordered"), s, &|i| {
            let ids = if i % 2 == 0 { windows.iter().rev().map(|w| w.id).collect() } else { windows.iter().map(|w| w.id).collect() };
            Event::WindowBatch(vec![WindowEvent::Order(ids)])
        });
        run(&name("window close, already gone"), s, &|_| WindowEvent::Closed(i64::MAX).into());
        run(&name("units, unchanged"), s, &|_| Event::Units(units.clone()));
        run(&name("pointer down and up on a tab"), s, &|i| {
            Event::Pointer(if i % 2 == 0 { PointerEvent::Down(Id::Tab(Tab::Volume), None) } else { PointerEvent::Up(Id::Tab(Tab::Volume)) })
        });
        run(&name("pointer down and up on a row"), s, &|i| {
            Event::Pointer(if i % 2 == 0 { PointerEvent::Down(ListId::Wifi.row(0), None) } else { PointerEvent::Up(ListId::Wifi.row(0)) })
        });
        run(&name("focus moved by the pad"), s, &|i| Event::Pointer(PointerEvent::Focus(Id::Tab(if i % 2 == 0 { Tab::Volume } else { Tab::Wifi }))));
        run(&name("focus moved by the pad down a list"), s, &|i| Event::Pointer(PointerEvent::Focus(ListId::Wifi.row(if i % 2 == 0 { 0 } else { 7 }))));
    }
    for (name, n) in &rows {
        println!("{n:4}  {name}");
    }
    let allocating: Vec<_> = rows.iter().filter(|(_, n)| *n > 0).collect();
    assert!(allocating.is_empty(), "allocations per event: {allocating:?}");
}
