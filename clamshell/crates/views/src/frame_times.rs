//! How long a frame takes to draw at the Thor's size: a whole frame recorded and played straight into
//! a buffer, as a live view draws a frame that changed everywhere; only the playing; and recording a
//! frame and finding it unchanged, which is what every event costs a visible screen. Run it by hand:
//! `cargo test --release -p clamshell-views frame_times -- --ignored --nocapture`.

use clamshell_state::State;
use clamshell_state::{Event, Id, Init, LibraryViewEvent, PointerEvent, Tab};
use clamshell_ui::Bounds;
use clamshell_ui::Screen;
use clamshell_ui::gfx::DrawCache;
use clamshell_ui::gfx::images::Images;
use clamshell_ui::gfx::paint::play;
use clamshell_ui::render::{Placement, Retained, ScreenFrames, record, render};
use std::time::{Duration, Instant};

fn state(events: Vec<Event>) -> State {
    let mut s = State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() });
    s.apply(Event::tick(1_700_000_000), &mut Vec::new());
    for e in events {
        s.apply(e, &mut Vec::new());
    }
    s
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// Two buffers drawing one screen's frames: one draws nearly each frame, the other now and then; both
/// hold what a whole frame draws, also after a frame no buffer drew.
#[test]
fn a_buffer_that_draws_now_and_then_catches_up() {
    let (w, h) = (480u32, 270u32);
    let at = Placement { size_px: (w, h), scale: 1, drift: (0, 0) };
    let (mut fonts, mut images) = (DrawCache::load(), Images::synchronous());
    let mut frames = ScreenFrames::default();
    let (mut every, mut sometimes) = (tiny_skia::Pixmap::new(w, h).unwrap(), tiny_skia::Pixmap::new(w, h).unwrap());
    let (mut held_every, mut held_sometimes) = (0, 0);
    for step in 0..7 {
        // The Library's selection a row further down each frame.
        let s = state(vec![Event::LibraryView(LibraryViewEvent::ListStep(step + 1))]);
        frames.record(at, Screen::Top.into(), &s, &mut fonts, &mut images, &crate::VIEWS);
        // At step 4 no buffer draws: what it changed is kept for the next draw.
        if step != 4 {
            held_every = frames.draw(&mut every, held_every, &mut fonts, &images).0;
        }
        if step % 3 == 2 {
            held_sometimes = frames.draw(&mut sometimes, held_sometimes, &mut fonts, &images).0;
            let mut whole = tiny_skia::Pixmap::new(w, h).unwrap();
            render(&mut whole, &mut Retained::default(), at, Screen::Top.into(), &s, &mut fonts, &mut images, &crate::VIEWS).unwrap();
            assert!(every.data() == whole.data() && sometimes.data() == whole.data(), "step {step}");
        }
    }
}

#[test]
#[ignore = "a measurement, by hand in release"]
fn frame_times() {
    let tab = |t| vec![Event::Pointer(PointerEvent::Activate(Id::Tab(t)))];
    let cases = [
        ("top: the Library", Screen::Top, (1920u32, 1080u32), vec![]),
        ("bottom: dock", Screen::Bottom, (1240, 1080), tab(Tab::Dock)),
        ("bottom: settings", Screen::Bottom, (1240, 1080), tab(Tab::Settings)),
        ("bottom: keyboard", Screen::Bottom, (1240, 1080), tab(Tab::Keyboard)),
    ];
    let (mut fonts, mut images) = (DrawCache::load(), Images::synchronous());
    for (name, screen, size, events) in cases {
        let s = state(events);
        let at = Placement { size_px: size, scale: 1, drift: (1, -2) };
        let mut retained = Retained::default();
        let mut pixmap = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
        let rounds = 200;
        let (mut full, mut checked, mut played) = (Vec::new(), Vec::new(), Vec::new());
        for round in 0..rounds + 20 {
            // A whole frame: recorded and played into the buffer.
            let t = Instant::now();
            render(&mut pixmap, &mut retained, at, screen.into(), &s, &mut fonts, &mut images, &crate::VIEWS).unwrap();
            let f = t.elapsed();
            // Recorded again and found unchanged, which is all an event that changes nothing costs.
            let t = Instant::now();
            record(&mut retained, at, screen.into(), &s, &mut fonts, &mut images, &crate::VIEWS);
            assert_eq!(retained.damage(), None);
            let c = t.elapsed();
            // Only played.
            let t = Instant::now();
            play(&mut pixmap, &retained.next, Bounds::of_size(size), &mut fonts, &images);
            let p = t.elapsed();
            // The first rounds warm the caches.
            if round >= 20 {
                full.push(f);
                checked.push(c);
                played.push(p);
            }
        }
        let ms = |v: Vec<Duration>| median(v).as_secs_f64() * 1e3;
        println!("{name:18} whole frame {:>7.3} ms   played {:>7.3} ms   recorded and compared {:>7.3} ms", ms(full), ms(played), ms(checked),);
    }
}
