//! The UI without a backend: the state and the event queue, the input machines, the timers as deadlines,
//! and each screen recorded and drawn into a pixmap. The compositor hosts it:
//! - gives it input by screen ([`Core::press`], [`Core::key`], ...) and the screens' sizes;
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
use clamshell_state::{Effect, Event, Id, PadInput, PointerEvent, State};

use crate::Error;
use crate::gfx::fonts::Found;
use crate::gfx::images::{Images, Key};
use crate::gfx::record::Bounds;
use crate::gfx::{self, DrawCache, Rect};
use crate::input::focus::Drawn;
use crate::input::{Action, EventClock, Input, Source, Timer};
use crate::render::{self, Placement, ScreenFrames};
use crate::screens::{Layer, Screen, Screens};
use crate::toolkit::{PadOut, Target};

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
    /// Where the content went in the last frame, in logical pixels (see [`crate::toolkit::FrameOut`]).
    content: Option<Rect>,
    /// Where a finger may point in the last frame, in logical pixels, and whether it points at once
    /// (see [`crate::toolkit::FrameOut`]).
    point_area: Option<Rect>,
    pointing: Option<fn(&State) -> bool>,
    /// What a press on the screen tells the state, as the last frame declared.
    touched: Option<Event>,
    /// What the pad's buttons do, as the last frame declared.
    pad: PadOut,
}

impl Default for ScreenCore {
    fn default() -> Self {
        ScreenCore {
            frames: ScreenFrames::default(),
            size: (1, 1),
            scale: 1,
            stale: true,
            content: None,
            point_area: None,
            pointing: None,
            touched: None,
            pad: PadOut::default(),
        }
    }
}

/// What the pad's focus goes over: the screens as last recorded, and the overlays over them.
struct Layers<'a> {
    screens: &'a Screens<ScreenCore>,
    overlays: &'a Screens<ScreenCore>,
}

impl Drawn for Layers<'_> {
    fn targets(&self, screen: Screen) -> &[Target] {
        &self.screens[screen].frames.retained.targets
    }

    fn overlay(&self, screen: Screen) -> &[Target] {
        &self.overlays[screen].frames.retained.targets
    }

    fn size(&self, screen: Screen) -> (f32, f32) {
        let (w, h) = self.screens[screen].size;
        (w as f32, h as f32)
    }

    fn pad(&self, screen: Screen) -> &PadOut {
        &self.screens[screen].pad
    }
}

type EventObserver = Box<dyn FnMut(&Event, &State)>;

pub struct Core {
    state: State,
    /// What is drawn (see [`crate::Views`]).
    views: crate::Views,
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
    /// What the frames recorded since the last [`Core::frames_done`] report, in order: what views reported
    /// (see [`crate::toolkit::FrameOut::report`]), and the lists' rows.
    reports: Vec<Event>,
}

impl Core {
    /// The core for `state`. Fonts for characters the built-in one lacks are found, and images decoded,
    /// on threads of their own; `wake` is called from those threads when something came back, so that
    /// the host runs the core.
    pub fn new(state: State, views: crate::Views, wake: impl Fn() + Send + Sync + 'static) -> Result<Core, Error> {
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
            views,
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
            reports: Vec::new(),
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
        self.step(Step::Event(event));
    }

    /// Takes `step` in turn (see [`Step`]).
    fn step(&mut self, step: Step) {
        if !self.queue.push(step) {
            return;
        }
        while let Some(step) = self.queue.next() {
            match step {
                // The pad driving the screens is input, handled here like keys and touches.
                Step::Event(Event::Pad(input)) => self.pad_from(input, Source::PadA),
                Step::Event(event) => self.handle_one(event),
                Step::Pad(input, source) => self.pad_from(input, source),
                Step::After(after) => after(self),
            }
        }
    }

    /// Runs `after` once the events queued before it are handled, for input that depends on what they
    /// change (see [`Step::After`]).
    fn after(&mut self, after: fn(&mut Core)) {
        self.step(Step::After(after));
    }

    fn handle_one(&mut self, event: Event) {
        if let Some(observer) = &mut self.event_observer {
            observer(&event, &self.state);
        }
        self.effects.clear();
        let (top, focus) = (self.state.top(), self.state.pointer().focus().cloned());
        // A press puts the focus on what it pressed, which isn't putting it somewhere (see `follow_top`).
        let pressed = match &event {
            Event::Pointer(PointerEvent::Activate(id) | PointerEvent::LongPress(id)) => Some(id.clone()),
            _ => None,
        };
        self.state.apply(event, &mut self.effects);
        for (level, line) in self.state.take_log() {
            match level {
                clamshell_state::LogLevel::Debug => log::debug!("{line}"),
                clamshell_state::LogLevel::Info => log::info!("{line}"),
                clamshell_state::LogLevel::Warn => log::warn!("{line}"),
            }
        }
        self.state_revision = self.state_revision.wrapping_add(1);
        self.changed();
        self.follow_top(top, focus.as_ref(), pressed.as_ref());
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

    /// When the main screen shows something else of ClamShell's than it did before an event (`top`), the
    /// focus starts where its view says, unless the event put the focus somewhere itself, as a pick ended
    /// puts it back on the field it was for: it was `focus`, and is now neither that nor the target the
    /// event pressed (`pressed`).
    fn follow_top(&mut self, top: clamshell_state::Top, focus: Option<&Id>, pressed: Option<&Id>) {
        let now = self.state.top();
        let put = self.state.pointer().focus().filter(|id| Some(*id) != focus && Some(*id) != pressed).is_some();
        if now == top || matches!(now, clamshell_state::Top::Window { .. }) || put {
            return;
        }
        self.record(Layer::Screen(Screen::Top), false);
        self.screens.top.stale = true;
        if let Some(start) = self.screens.top.pad.start.clone() {
            self.handle(PointerEvent::Start(start).into());
        }
    }

    /// Records again the layers the pad may act on that changed since they were last recorded, so that
    /// what the pad does follows what they show now (see `toolkit::PadOut`). They stay stale for the host,
    /// which records and draws them as usual. The main screen's view isn't recorded while a window covers
    /// it: the pad doesn't use it then.
    pub(crate) fn fresh(&mut self) {
        let top = self.state.pad().on_top();
        let layers = [Layer::Screen(Screen::Bottom), Layer::Overlay(Screen::Top), Layer::Overlay(Screen::Bottom)];
        for layer in layers.into_iter().chain(top.then_some(Layer::Screen(Screen::Top))) {
            if self.stale(layer) {
                self.record(layer, false);
                match layer {
                    Layer::Screen(screen) => self.screens[screen].stale = true,
                    Layer::Overlay(screen) => self.overlays[screen].stale = true,
                }
            }
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

    /// Where the screen's content went in the last recording, in logical pixels, such as the area below
    /// the second screen's bar. A compositor uses the same layout for client windows as the view and its
    /// input targets do.
    pub fn content_bounds(&self, screen: Screen) -> Option<Bounds> {
        self.screens[screen].content.map(|r| Bounds::new(r.x.ceil() as i32, r.y.ceil() as i32, (r.x + r.w).floor() as i32, (r.y + r.h).floor() as i32))
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
        let mut out = s.frames.record(at, layer, &self.state, &mut self.fonts, &mut self.images, &self.views);
        s.stale = false;
        (s.content, s.point_area, s.pointing) = (out.content, out.point_area, out.pointing);
        s.touched = out.touched.take();
        s.pad = std::mem::take(&mut out.pad);
        self.reports.extend(out.report.take());
        self.reports.extend(render::list_changes(&self.state, &out));
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
        let reports = std::mem::take(&mut self.reports);
        self.handle_all(reports);
    }
}

/// Sends `arrival` to the core and wakes its host. False once the core is gone.
fn tell(to: &Sender<Arrival>, wake: &dyn Fn(), arrival: Arrival) -> bool {
    let sent = to.send(arrival).is_ok();
    wake();
    sent
}

/// What the queue holds: events, and the pad's input, which turns into events.
///
/// Pad input is always taken from the queue, by the pad's own events ([`Event::Pad`]) and by the keys
/// that stand in for the pad alike. So every event it gives is only queued, and what it decides it
/// decides from the state before any of them: it never reads a state that one of its own events was
/// meant to change and didn't yet. Input that depends on the outcome of its events queues an
/// [`Step::After`] behind them.
enum Step {
    Event(Event),
    Pad(PadInput, Source),
    /// Runs once the steps queued before it are done.
    After(fn(&mut Core)),
}

/// The steps waiting to be taken. Events are handled one at a time: one that comes while another is
/// being handled, from the input side or a frame drawn, waits until that one is done, effects and all, so
/// that the effects come in the order of the events. Waiting steps are taken in the order they came.
#[derive(Default)]
struct EventQueue {
    events: VecDeque<Step>,
    /// A step is being taken.
    busy: bool,
}

impl EventQueue {
    /// Queues `step`. Returns whether the caller is to take the queue's steps, which it is unless one is
    /// being taken already.
    fn push(&mut self, step: Step) -> bool {
        self.events.push_back(step);
        !std::mem::replace(&mut self.busy, true)
    }

    /// The next step to take, or None once the queue is empty, which ends handling.
    fn next(&mut self) -> Option<Step> {
        let next = self.events.pop_front();
        self.busy = next.is_some();
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clamshell_state::LibraryViewEvent;

    /// Handles events as `Core::handle` does, where handling event `n` below 3 emits event `n + 1`, and
    /// the emitted one is handled after the effects are given.
    struct Fake {
        queue: EventQueue,
        log: Vec<String>,
    }

    impl Fake {
        fn handle(&mut self, event: Event) {
            if !self.queue.push(Step::Event(event)) {
                return;
            }
            while let Some(step) = self.queue.next() {
                let Step::Event(Event::LibraryView(LibraryViewEvent::Rows(n))) = step else {
                    continue;
                };
                self.log.push(format!("apply {n}"));
                if n < 3 {
                    self.handle(Event::LibraryView(LibraryViewEvent::Rows(n + 1)));
                    self.handle(Event::LibraryView(LibraryViewEvent::Rows(n + 10)));
                }
                self.log.push(format!("effects {n}"));
            }
        }
    }

    #[test]
    fn an_event_emitted_while_handling_waits_for_the_effects() {
        let mut app = Fake { queue: EventQueue::default(), log: Vec::new() };
        app.handle(Event::LibraryView(LibraryViewEvent::Rows(1)));
        let expected = ["apply 1", "effects 1", "apply 2", "effects 2", "apply 11", "effects 11", "apply 3", "effects 3", "apply 12", "effects 12"];
        assert_eq!(app.log, expected);
        // Done, so the next event is handled at once.
        app.handle(Event::LibraryView(LibraryViewEvent::Rows(20)));
        assert_eq!(app.log.get(10..), Some(&["apply 20".to_owned(), "effects 20".to_owned()][..]));
    }
}
