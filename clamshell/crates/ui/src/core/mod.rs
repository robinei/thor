//! The UI without a backend: the state and the event queue, the input machines, the timers as deadlines,
//! and each screen recorded and drawn into a pixmap. The compositor hosts it:
//! - gives it input by screen ([`Core::press`], [`Core::pad`], ...) and the screens' sizes;
//! - runs it when its deadline comes and after each turn of its loop ([`Core::run`]);
//! - draws a screen that is due when a frame can be shown: [`Core::record`], then [`Core::draw`] into a
//!   pixmap of the screen's size, then [`Core::frames_done`]; and the same for each screen's overlay
//!   ([`Layer::Overlay`]), shown where it draws ([`Core::overlay_area`]);
//! - carries out the effects, in order ([`Core::next_effect`]): the screens' power, the refresh rate and
//!   quitting itself, the rest through the system crate.

mod input;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use clamshell_state::bottom::touch::{Contact, Pointing};
use clamshell_state::{Effect, Event, Id, ShelfEvent, State};

use crate::Error;
use crate::gfx::fonts::Found;
use crate::gfx::images::{Images, Key};
use crate::gfx::record::Bounds;
use crate::gfx::{self, DrawCache, Rect};
use crate::input::focus::Drawn;
use crate::input::{Action, EventClock, Input, Source, Timer};
use crate::render::{self, Placement, ScreenFrames};
use crate::screens::{Layer, Screen, Screens};
use crate::toolkit::Target;

/// What came back from the font finder's and the image decoder's threads.
enum Arrival {
    Font(Found),
    Image(Key, Option<tiny_skia::Pixmap>),
}

/// One screen: its frames, its size and what it reported.
struct ScreenCore {
    frames: ScreenFrames,
    /// Logical size, as the host has it.
    size: (u32, u32),
    scale: u32,
    /// What it shows may have changed since it was last recorded, so it is recorded again when a frame
    /// is due.
    stale: bool,
    /// The second screen's area below the bar in the last frame, in logical pixels.
    body: Option<Rect>,
}

impl Default for ScreenCore {
    fn default() -> Self {
        ScreenCore { frames: ScreenFrames::default(), size: (1, 1), scale: 1, stale: true, body: None }
    }
}

impl Drawn for Screens<ScreenCore> {
    fn targets(&self, screen: Screen) -> &[Target] {
        &self[screen].frames.retained.targets
    }

    fn size(&self, screen: Screen) -> (f32, f32) {
        let (w, h) = self[screen].size;
        (w as f32, h as f32)
    }
}

type EventObserver = Box<dyn FnMut(&Event, &State)>;

pub struct Core {
    state: State,
    state_revision: u64,
    event_observer: Option<EventObserver>,
    fonts: DrawCache,
    images: Images,
    arrivals: Receiver<Arrival>,
    screens: Screens<ScreenCore>,
    /// What is drawn over each screen, at its size.
    overlays: Screens<ScreenCore>,
    input: Input,
    /// The presses' running timers, each with when it fires.
    timers: Vec<(Source, Timer, Instant)>,
    /// Turns the host's event times into instants, so they say when a finger moved rather than when the
    /// event was handled.
    clock: EventClock,
    /// Fingers pointing in the touch frame being handled. They are sent on together once the whole frame
    /// has been handled, at the next run.
    contacts: Vec<Contact>,
    /// Where the pad's focus was on each screen, so View can switch back to it.
    focus_memory: Screens<Option<Id>>,
    /// The effects of the event being handled, kept to be reused for the next one.
    effects: Vec<Effect>,
    /// The effects not yet taken by the host, in order.
    outbox: VecDeque<Effect>,
    /// The events waiting while another is handled.
    queue: EventQueue,
    /// What the frames recorded since the last [`Core::frames_done`] report: a changed row count of
    /// shelf's list, and the lists' rows.
    rows: Option<usize>,
    lists: Vec<Event>,
}

impl Core {
    /// The core for `state`. Fonts for characters the built-in one lacks are found, and images decoded,
    /// on threads of their own; `wake` is called from those threads when something came back, so that
    /// the host runs the core.
    pub fn new(state: State, wake: impl Fn() + Send + Sync + 'static) -> Result<Core, Error> {
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let fonts = {
            let (requests, finder) = std::sync::mpsc::channel();
            let (arrived, wake) = (arrived.clone(), wake.clone());
            gfx::spawn_font_finder(finder, move |found| tell(&arrived, &*wake, Arrival::Font(found)))?;
            DrawCache::with_finder(requests)
        };
        let images = {
            let (requests, worker) = std::sync::mpsc::channel();
            gfx::images::spawn_worker(worker, move |key, pixmap| tell(&arrived, &*wake, Arrival::Image(key, pixmap)))?;
            Images::new(requests)
        };
        Ok(Core {
            state,
            state_revision: 0,
            event_observer: None,
            fonts,
            images,
            arrivals,
            screens: Screens::default(),
            overlays: Screens::default(),
            input: Input::default(),
            timers: Vec::new(),
            clock: EventClock::default(),
            contacts: Vec::new(),
            focus_memory: Screens::default(),
            effects: Vec::new(),
            outbox: VecDeque::new(),
            queue: EventQueue::default(),
            rows: None,
            lists: Vec::new(),
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Observe inputs before they are applied, for host diagnostics. The observer cannot change state.
    pub fn observe_events(&mut self, observer: impl FnMut(&Event, &State) + 'static) {
        self.event_observer = Some(Box::new(observer));
    }

    /// Changes whenever an event is applied, including events generated by the UI itself.
    pub fn state_revision(&self) -> u64 {
        self.state_revision
    }

    /// Handles `event`. One that comes while another is being handled, such as one the input side emits,
    /// waits until that one is done, effects and all (see [`EventQueue`]).
    pub fn handle(&mut self, event: Event) {
        if !self.queue.push(event) {
            return;
        }
        while let Some(event) = self.queue.next() {
            self.handle_one(event);
        }
    }

    fn handle_one(&mut self, event: Event) {
        // The pad driving the screens is input, handled here like keys and touches.
        if let Event::Pad(input) = event {
            return self.pad(input);
        }
        if let Some(observer) = &mut self.event_observer {
            observer(&event, &self.state);
        }
        self.effects.clear();
        self.state.apply(event, &mut self.effects);
        self.state_revision = self.state_revision.wrapping_add(1);
        self.changed();
        let released = self.input.pad_route(self.state.pad().route(), &self.state);
        self.run_actions(released);
        self.outbox.extend(self.effects.drain(..));
    }

    pub(crate) fn handle_all(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.handle(event);
        }
    }

    /// The next effect for the host to carry out, in the order the events gave them.
    pub fn next_effect(&mut self) -> Option<Effect> {
        self.outbox.pop_front()
    }

    /// When the core is next to run ([`Core::run`]) for a timer, if one is running.
    pub fn deadline(&self) -> Option<Instant> {
        self.timers.iter().map(|&(_, _, at)| at).min()
    }

    /// Runs what is due at `now`, which the host does after each turn of its loop: takes in the fonts
    /// and images that came back, fires the timers that are due, earliest first, and sends on the
    /// fingers pointing in the touch frame just handled.
    pub fn run(&mut self, now: Instant) {
        while let Ok(arrival) = self.arrivals.try_recv() {
            match arrival {
                Arrival::Font(found) => {
                    self.fonts.found(found);
                }
                Arrival::Image(key, pixmap) => self.images.arrived(key, pixmap),
            }
            self.changed();
        }
        while let Some(i) = self.timers.iter().enumerate().filter(|(_, t)| t.2 <= now).min_by_key(|(_, t)| t.2).map(|(i, _)| i) {
            let (source, timer, _) = self.timers.swap_remove(i);
            let actions = self.input.timer(source, timer, &self.state, now);
            self.run_actions(actions);
        }
        if !self.contacts.is_empty() {
            let contacts = std::mem::take(&mut self.contacts);
            self.handle(Pointing::Contacts(contacts).into());
        }
    }

    /// Carries out what the input side asks: events, and the presses' timers.
    pub(crate) fn run_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Emit(event) => self.handle(event),
                Action::Arm(source, timer, after) => {
                    self.timers.retain(|t| (t.0, t.1) != (source, timer));
                    self.timers.push((source, timer, Instant::now() + after));
                }
                Action::Disarm(source, timer) => self.timers.retain(|t| (t.0, t.1) != (source, timer)),
                // Positions are in the second screen's pixels, and a finger's slot comes from its touch id.
                Action::Point(Source::Touch(id), phase, (x, y)) => {
                    let scale = self.screens.bottom.scale as f32;
                    let slot = id.rem_euclid(clamshell_state::bottom::touch::SLOTS) as u8;
                    self.contacts.push(Contact { slot, phase, x: (x * scale).round() as i32, y: (y * scale).round() as i32 });
                }
                Action::Point(..) => {}
            }
        }
    }

    /// What the screens show may have changed, so each is recorded again before its next frame and drawn
    /// if it changed.
    pub(crate) fn changed(&mut self) {
        for (_, screen) in self.screens.iter_mut().chain(self.overlays.iter_mut()) {
            screen.stale = true;
        }
    }

    /// `screen`'s logical size and integer scale, as the host shows it. It is recorded again, and its
    /// overlay too.
    pub fn resize(&mut self, screen: Screen, size: (u32, u32), scale: u32) {
        for s in [&mut self.screens[screen], &mut self.overlays[screen]] {
            (s.size, s.scale, s.stale) = (size, scale.max(1), true);
        }
    }

    fn layer(&self, layer: Layer) -> &ScreenCore {
        match layer {
            Layer::Screen(screen) => &self.screens[screen],
            Layer::Overlay(screen) => &self.overlays[screen],
        }
    }
    /// `layer`'s logical size and its scale.
    pub fn size(&self, layer: impl Into<Layer>) -> ((u32, u32), u32) {
        let s = self.layer(layer.into());
        (s.size, s.scale)
    }

    /// The area below the bottom bar in the last recording, in logical pixels. A compositor uses the
    /// same layout for client windows as the view and its input targets do.
    pub fn content_bounds(&self, screen: Screen) -> Option<Bounds> {
        self.screens[screen].body.map(|r| Bounds::new(r.x.ceil() as i32, r.y.ceil() as i32, (r.x + r.w).floor() as i32, (r.y + r.h).floor() as i32))
    }

    /// Whether `layer` may have changed since it was last recorded.
    pub fn stale(&self, layer: impl Into<Layer>) -> bool {
        self.layer(layer.into()).stale
    }

    /// Records `layer`'s next frame and compares it with the last one. Returns what changed in buffer
    /// pixels, the whole frame if `whole` (each buffer then draws all of it), or `None` if nothing did.
    /// What the frame reports is kept for [`Core::frames_done`].
    pub fn record(&mut self, layer: impl Into<Layer>, whole: bool) -> Option<Bounds> {
        let layer = layer.into();
        let s = match layer {
            Layer::Screen(screen) => &mut self.screens[screen],
            Layer::Overlay(screen) => &mut self.overlays[screen],
        };
        let size_px = (s.size.0 * s.scale, s.size.1 * s.scale);
        let at = Placement { size_px, scale: s.scale, drift: render::orbit(self.state.clock().unix()) };
        let out = s.frames.record(at, layer, &self.state, &mut self.fonts, &mut self.images);
        s.stale = false;
        s.body = out.touchpad_area;
        self.rows = self.rows.or(out.list_rows.filter(|&rows| rows != self.state.shelf().rows()));
        self.lists.extend(render::list_changes(&self.state, &out));
        if whole {
            s.frames.redraw_all(Bounds::of_size(size_px));
            return Some(Bounds::of_size(size_px));
        }
        s.frames.damage()
    }

    /// Draws `layer`'s latest frame into `target`, which is its size in buffer pixels and holds frame `held`
    /// (0 for none): what changed since. Any number of pixmaps draw the same frames, each catching up from
    /// the frame it holds.
    /// Returns the frame it now holds and the area drawn.
    pub fn draw(&mut self, layer: impl Into<Layer>, target: &mut tiny_skia::Pixmap, held: u64) -> (u64, Bounds) {
        let s = match layer.into() {
            Layer::Screen(screen) => &mut self.screens[screen],
            Layer::Overlay(screen) => &mut self.overlays[screen],
        };
        s.frames.draw(target, held, &mut self.fonts, &self.images)
    }

    /// Whether a buffer of `layer` that holds frame `held` has something to draw (see [`Core::draw`]).
    pub fn behind(&self, layer: impl Into<Layer>, held: u64) -> bool {
        self.layer(layer.into()).frames.behind(held)
    }

    /// What a buffer of `layer` holding frame `held` lacks of the latest frame, in buffer pixels: what
    /// changed since, or all of it from too far back (see [`Core::draw`]). For a buffer brought up to date by
    /// copying from one drawn, so after that one has drawn the latest frame.
    pub fn since(&self, layer: impl Into<Layer>, held: u64) -> Bounds {
        self.layer(layer.into()).frames.since(held)
    }

    /// Where `screen`'s overlay's latest frame draws, in buffer pixels: the part of it to show over
    /// what the screen shows. None when it draws nothing.
    pub fn overlay_area(&self, screen: Screen) -> Option<Bounds> {
        let latest = &self.overlays[screen].frames.retained.latest;
        let ink = latest.ink().intersect(Bounds::of_size(latest.size()));
        (!ink.is_empty()).then_some(ink)
    }

    /// The frames recorded since the last call are done: the state learns what they report. The list's
    /// row count follows the layout, and the state keeps the selection within it.
    pub fn frames_done(&mut self) {
        if let Some(rows) = self.rows.take() {
            self.handle(Event::Shelf(ShelfEvent::Rows(rows)));
        }
        let lists = std::mem::take(&mut self.lists);
        self.handle_all(lists);
    }
}

/// Sends `arrival` to the core and wakes its host. False once the core is gone.
fn tell(to: &Sender<Arrival>, wake: &dyn Fn(), arrival: Arrival) -> bool {
    let sent = to.send(arrival).is_ok();
    wake();
    sent
}

/// The events waiting to be handled. Events are handled one at a time: one that comes while another is
/// being handled, from the input side or a frame drawn, waits until that one is done, effects and all, so
/// that the effects come in the order of the events. Waiting events are handled in the order they came.
#[derive(Default)]
struct EventQueue {
    events: VecDeque<Event>,
    /// An event is being handled.
    busy: bool,
}

impl EventQueue {
    /// Queues `event`. Returns whether the caller is to handle the queue, which it is unless an event is
    /// being handled already.
    fn push(&mut self, event: Event) -> bool {
        self.events.push_back(event);
        !std::mem::replace(&mut self.busy, true)
    }

    /// The next event to handle, or None once the queue is empty, which ends handling.
    fn next(&mut self) -> Option<Event> {
        let next = self.events.pop_front();
        self.busy = next.is_some();
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Handles events as `Core::handle` does, where handling event `n` below 3 emits event `n + 1`, and
    /// the emitted one is handled after the effects are given.
    struct Fake {
        queue: EventQueue,
        log: Vec<String>,
    }

    impl Fake {
        fn handle(&mut self, event: Event) {
            if !self.queue.push(event) {
                return;
            }
            while let Some(event) = self.queue.next() {
                let Event::Shelf(ShelfEvent::Rows(n)) = event else {
                    continue;
                };
                self.log.push(format!("apply {n}"));
                if n < 3 {
                    self.handle(Event::Shelf(ShelfEvent::Rows(n + 1)));
                    self.handle(Event::Shelf(ShelfEvent::Rows(n + 10)));
                }
                self.log.push(format!("effects {n}"));
            }
        }
    }

    #[test]
    fn an_event_emitted_while_handling_waits_for_the_effects() {
        let mut app = Fake { queue: EventQueue::default(), log: Vec::new() };
        app.handle(Event::Shelf(ShelfEvent::Rows(1)));
        let expected = ["apply 1", "effects 1", "apply 2", "effects 2", "apply 11", "effects 11", "apply 3", "effects 3", "apply 12", "effects 12"];
        assert_eq!(app.log, expected);
        // Done, so the next event is handled at once.
        app.handle(Event::Shelf(ShelfEvent::Rows(20)));
        assert_eq!(app.log.get(10..), Some(&["apply 20".to_owned(), "effects 20".to_owned()][..]));
    }
}
