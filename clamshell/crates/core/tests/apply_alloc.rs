//! How much `State::apply` allocates for the events that come all the time, once the state is warm.
//! Exploration for now: it prints the counts (`cargo test -p clamshell-core --test apply_alloc -- --nocapture`).
#![allow(clippy::unwrap_used, unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use clamshell_core::{Effect, Event, Id, Init, PointerEvent, Reading, State, Status, Tab};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static TRACE: Cell<bool> = const { Cell::new(false) };
}

/// With CLAMSHELL_ALLOC_TRACE set, where each counted allocation came from (core's frames only).
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
    let frames: Vec<&str> = bt.lines().filter(|l| l.contains("clamshell_core::") && !l.contains("apply_alloc")).map(str::trim).take(4).collect();
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

#[test]
fn allocations_per_event() {
    let mut s = State::new(Init { library: clamshell_library::demo::library(), ..Init::default() });
    let t0 = 1_760_000_000;
    let mut rows = Vec::new();
    let mut effects = Vec::new();
    let mut run = |name: &str, s: &mut State, make: &dyn Fn(u64) -> Event| {
        // Warm up with the same kind of event, then measure the third.
        for i in 0..2 {
            effects.clear();
            s.apply(make(i), &mut effects);
        }
        rows.push((name.to_owned(), count(s, make(2), &mut effects)));
    };
    s.apply(Event::Start, &mut Vec::new());
    run("tick, same minute", &mut s, &|i| Event::Tick { unix_secs: t0 + i, mono_secs: 100 + i });
    run("tick, a new minute each", &mut s, &|i| Event::Tick { unix_secs: t0 + 60 * (i + 1), mono_secs: 200 + 60 * i });
    run("status reading, unchanged", &mut s, &|_| Event::Reading(Reading::Status(Status::default())));
    run("windows, none, unchanged", &mut s, &|_| Event::Windows(Vec::new()));
    run("units, none, unchanged", &mut s, &|_| Event::Units(Vec::new()));
    run("pointer down and up on a tab", &mut s, &|i| {
        Event::Pointer(if i % 2 == 0 { PointerEvent::Down(Id::Tab(Tab::Volume)) } else { PointerEvent::Up(Id::Tab(Tab::Volume)) })
    });
    run("focus moved by the pad", &mut s, &|i| Event::Pointer(PointerEvent::Focus(Id::Tab(if i % 2 == 0 { Tab::Volume } else { Tab::Wifi }))));
    for (name, n) in rows {
        println!("{n:4}  {name}");
    }
}
