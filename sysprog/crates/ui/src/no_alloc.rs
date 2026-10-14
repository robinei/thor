//! Drawing a frame must not touch the heap once caches are warm (glyphs rasterized, buffers sized):
//! counted with an allocator that tallies this thread's allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use sysprog_core::keyboard::{self, Key, Modifier};
use sysprog_core::{Event, Id, Mode, State};
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

fn key(k: Key) -> Event {
    Event::Activate(Id::Key(keyboard::find(k).expect("key on the layout")))
}

#[test]
fn warm_frames_do_not_allocate() {
    let mut state = State::new(sysprog_library::demo_library());
    state.apply(Event::Tick { unix_secs: 1_700_000_000 });
    let mut cache = DrawCache::load();
    let mut scratch = Scratch::default();
    let mut images = crate::images::Images::synchronous();
    let mut targets = Vec::new();
    let mut top = Pixmap::new(1920, 1080).expect("pixmap");
    let mut bottom = Pixmap::new(1240, 1080).expect("pixmap");

    let cases: Vec<(&str, Vec<Event>)> = vec![
        ("sysinfo", vec![]),
        ("settings", vec![Event::Activate(Id::Mode(Mode::Settings))]),
        ("settings, a changed tile", vec![Event::Activate(Id::Tile(sysprog_core::settings::Setting::FpsCap))]),
        ("settings, all games", vec![Event::Activate(Id::Scope(sysprog_core::Scope::Global))]),
        ("environment editor", vec![Event::Activate(Id::Env), key(Key::Char('a'))]),
        ("environment saved", vec![key(Key::Char('=')), key(Key::Char('1')), Event::Activate(Id::EnvSave)]),
        ("launcher, an app running", vec![Event::Activate(Id::Mode(Mode::Launcher)), Event::Activate(Id::App(2))]),
        ("keyboard", vec![Event::Activate(Id::Mode(Mode::Keyboard))]),
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
