//! The game's route allocates nothing per controller report once warm: a player's router in the game,
//! through a keymap with layers, chords, a hold, a double, turbo, a full pull, a stick on the pointer and
//! the gyro on a stick. The test counts allocations with an allocator that tallies this thread's calls
//! (the test binary's allocator; other tests' threads aren't counted).
#![allow(unsafe_code, clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use clamshell_state::PadRoute;
use clamshell_state::bottom::keyboard::{Key, Named};
use clamshell_state::keymap::{Actuation, Binding, Button, Chord, Input, Keymap, Layer, MouseButton, Output, PadOut, Side};
use evdev::{AbsoluteAxisCode, KeyCode};

use super::motion::{Rate, Sample};
use super::route::{Axis, Out, PadRouter, PadSnapshot, Ranges};
use crate::os::evdev::{Press, Range, Raw};

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

/// A keymap that uses every part of the remap.
fn keymap() -> Keymap {
    let key = |c: Chord| Binding::to(Output::Key(c));
    let b = Input::Button;
    let mut k = Keymap::new(1, "all");
    let aim = k.add_layer("aim").unwrap();
    let menu = k.add_layer("menu").unwrap();
    k.set_layer_once(menu, true);
    assert!(k.set(b(Button::A), Some(key(Chord { ctrl: true, ..Chord::of(Key::Char('c')) }))));
    assert!(k.set(b(Button::LB), Some(Binding::to(Output::Layer(menu)))));
    assert!(k.set(b(Button::RB), Some(Binding { turbo: 10, ..key(Chord::of(Key::Char('r'))) })));
    assert!(k.set_of(Layer::BASE, b(Button::RB), Actuation::Hold, Some(Binding::to(Output::Layer(aim)))));
    assert!(k.set(b(Button::Y), Some(key(Chord::of(Key::Char('t'))))));
    assert!(k.set_of(Layer::BASE, b(Button::Y), Actuation::Double, Some(Binding::to(Output::Layer(menu)))));
    assert!(k.set(b(Button::X), Some(Binding::to(Output::Mouse(MouseButton::Left)))));
    assert!(k.set(b(Button::B), Some(Binding::to(Output::Pad(PadOut::Button(Button::A))))));
    assert!(k.set_in(menu, b(Button::A), Some(key(Chord::of(Key::Named(Named::Enter))))));
    assert!(k.set_in(aim, b(Button::A), Some(key(Chord { shift: true, ..Chord::of(Key::Char('y')) }))));
    assert!(k.set(Input::Stick(Side::Left), Some(Binding::to(Output::Pointer))));
    assert!(k.set(Input::Gyro, Some(Binding::to(Output::Stick(Side::Right)))));
    let full = Binding { threshold: 95, ..Binding::to(Output::Mouse(MouseButton::Right)) };
    assert!(k.set_of(Layer::BASE, Input::Trigger(Side::Right), Actuation::FullPull, Some(full)));
    k
}

fn router() -> (PadRouter, Out) {
    let ranges = Ranges { stick: Range::STICK, trigger: Range { min: 0, max: 1023 } };
    let axes = [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
        AbsoluteAxisCode::ABS_Z,
        AbsoluteAxisCode::ABS_RZ,
        AbsoluteAxisCode::ABS_HAT0X,
        AbsoluteAxisCode::ABS_HAT0Y,
    ]
    .map(|code| Axis { code, value: 0, rest: 0 })
    .to_vec();
    let mut out = Out::default();
    let r = PadRouter::new(ranges, PadSnapshot::new(axes), PadRoute::Game, keymap(), &mut out);
    (r, out)
}

/// A round of play: each kind of input pressed, held through ticks and let go, sticks moved together in
/// one report, the gyro turning, a trigger pulled through its full pull.
fn play(r: &mut PadRouter, out: &mut Out, t0: Instant) {
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let key = |code, down: bool| Raw::key(code, if down { Press::Down } else { Press::Up });
    let mut at = 0;
    let mut report = |r: &mut PadRouter, out: &mut Out, raws: &[Raw], after: u64| {
        at += after;
        for &raw in raws {
            r.event(raw, ms(at), out);
        }
        r.event(Raw::REPORT, ms(at), out);
        r.tick(ms(at), out);
        out.pad.clear();
        out.mouse.clear();
        out.events.clear();
        out.buzz = None;
    };
    for code in [KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_WEST, KeyCode::BTN_NORTH, KeyCode::BTN_TL, KeyCode::BTN_TR] {
        report(r, out, &[key(code, true)], 10);
        report(r, out, &[key(KeyCode::BTN_SOUTH, true)], 10);
        report(r, out, &[key(KeyCode::BTN_SOUTH, false)], 400);
        report(r, out, &[key(code, false)], 10);
    }
    // A double, then the Once layer's input.
    report(r, out, &[key(KeyCode::BTN_NORTH, true)], 400);
    report(r, out, &[key(KeyCode::BTN_NORTH, false)], 30);
    report(r, out, &[key(KeyCode::BTN_NORTH, true)], 30);
    report(r, out, &[key(KeyCode::BTN_NORTH, false)], 30);
    report(r, out, &[key(KeyCode::BTN_SOUTH, true)], 10);
    report(r, out, &[key(KeyCode::BTN_SOUTH, false)], 10);
    // Both sticks and a trigger moving, each report with several axes.
    for i in 0..20 {
        let v = (i * 3000) % 32000 - 16000;
        report(r, out, &[Raw::abs(AbsoluteAxisCode::ABS_X, v), Raw::abs(AbsoluteAxisCode::ABS_Y, -v), Raw::abs(AbsoluteAxisCode::ABS_RZ, (i * 60) % 1024)], 8);
    }
    report(r, out, &[Raw::abs(AbsoluteAxisCode::ABS_X, 0), Raw::abs(AbsoluteAxisCode::ABS_Y, 0), Raw::abs(AbsoluteAxisCode::ABS_RZ, 0)], 8);
    // The gyro on the right stick, a few samples between ticks as they come.
    for i in 0..20u64 {
        let yaw = if i % 2 == 0 { 40.0 } else { -40.0 };
        r.motion(Sample { rate: Rate { pitch: 5.0, yaw, roll: 0.0 }, at: Duration::from_millis(i * 4) }, out);
        if i % 4 == 3 {
            report(r, out, &[], 4);
        }
    }
}

#[test]
fn the_games_route_allocates_nothing_once_warm() {
    let (mut r, mut out) = router();
    let t0 = Instant::now();
    // Warm: every buffer grows to what play needs.
    for round in 0..3 {
        play(&mut r, &mut out, t0 + Duration::from_secs(round * 60));
    }
    let before = allocs();
    for round in 3..6 {
        play(&mut r, &mut out, t0 + Duration::from_secs(round * 60));
    }
    assert_eq!(allocs() - before, 0, "allocations in three rounds of play once warm");
}
