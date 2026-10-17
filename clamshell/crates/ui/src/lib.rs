//! The Wayland side: two views, the main (top) screen and the second (bottom) screen, drawn on the CPU
//! into shm buffers. A view is redrawn only when the state changed for it, at most once per frame
//! callback; between events the program sleeps.
//!
//! The top view is an xdg toplevel (sway rules make it fullscreen in its own workspace). The bottom view
//! is a layer-shell surface on the second screen's output when that output exists (the Thor: `DSI-1`),
//! else a panel in a desktop's corner, or a second ordinary window, so it runs on any desktop.
//!
//! Input is turned into events by pure parts (`input` for presses, `focus` for the pad); `App` only
//! connects them to Wayland and the event loop.

mod display;
mod draw;
mod fling;
mod focus;
mod icons;
pub mod images;
mod input;
pub mod nav;
#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod no_alloc;
mod render;
mod screens;
mod text_input;
mod toolkit;
mod view;
mod views;
mod wayland;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clamshell_core::touch::TouchLayout;
use clamshell_core::{Dirty, Effect, Event, Id, ListEvent, ListId, PadInput, PointerEvent, ShelfEvent, State};
use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken, channel,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::{GlobalList, registry_queue_init},
            protocol::{wl_keyboard, wl_pointer, wl_surface::WlSurface, wl_touch},
        },
    },
    registry::RegistryState,
    seat::{SeatState, keyboard::Keysym},
    shell::{wlr_layer::LayerShell, xdg::XdgShell},
    shm::{Shm, slot::SlotPool},
};

use draw::DrawCache;
use focus::PadPress;
use input::{Action, End, Input, Source};
use nav::Dir;
use render::{Picture, Placement};
use screens::Screens;
use view::View;

pub use screens::Screen;

pub type Error = Box<dyn std::error::Error>;

pub struct Config {
    /// Output that gets the bottom view as a layer-shell surface; without it, the bottom view is a window.
    pub bottom_output: String,
    /// Logical sizes when shown as windows (on a desktop).
    pub top_size: (u32, u32),
    pub bottom_size: (u32, u32),
    /// Without the second screen's output (a desktop): show the bottom view as a panel in a corner
    /// instead of a window, so tapping its keyboard leaves the keyboard focus on the app being typed into.
    pub bottom_panel: bool,
    /// Where the touchpad area and the bar's targets are, for the touchpad tab's input side; updated
    /// with every frame of the second screen.
    pub touch_layout: Arc<Mutex<TouchLayout>>,
}

/// Sends events into the UI's event loop from other threads.
#[derive(Clone)]
pub struct EventSender(channel::Sender<Event>);

impl EventSender {
    /// False once the UI has exited.
    pub fn send(&self, event: Event) -> bool {
        self.0.send(event).is_ok()
    }
}

/// Runs the UI until quit. `start` gets a sender for the system threads; `on_effect` carries out what the
/// state asks for (launching games, ...).
pub fn run(state: State, config: Config, start: impl FnOnce(EventSender), on_effect: impl FnMut(&Effect) + 'static) -> Result<(), Error> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let shells = Shells::bind(&globals, &qh)?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let images = install_images(&event_loop.handle())?;
    let mut app = App::new(&globals, &qh, state, &config, images, event_loop.handle(), Box::new(on_effect))?;

    // Learn the outputs and their names before placing the bottom view. (These roundtrips dispatch into
    // the app, so it exists before its views.)
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;
    if let Some(seat) = app.seat_state.seats().next() {
        app.text_input.seat(&seat, &qh);
        app.display.seat(&seat, &qh);
    }
    app.place_views(&shells, &qh, &config);

    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).map_err(|e| e.error)?;
    let (sender, receiver) = channel::channel();
    event_loop
        .handle()
        .insert_source(receiver, |event, _, app: &mut App| {
            if let channel::Event::Msg(event) = event {
                app.handle(event);
            }
        })
        .map_err(|e| e.error)?;
    start(EventSender(sender));
    // What the state in force needs now that everything runs (the tab's watch, brightness, the pad).
    app.handle(Event::Start);

    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
        app.redraw_dirty();
    }
    Ok(())
}

/// The shells the views are made with.
struct Shells {
    compositor: CompositorState,
    xdg: XdgShell,
    layer: Option<LayerShell>,
}

impl Shells {
    fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Result<Shells, Error> {
        Ok(Shells { compositor: CompositorState::bind(globals, qh)?, xdg: XdgShell::bind(globals, qh)?, layer: LayerShell::bind(globals, qh).ok() })
    }
}

/// Images: requests to a decoding thread, results back through the event loop.
fn install_images(handle: &LoopHandle<'static, App>) -> Result<images::Images, Error> {
    let (requests, worker) = std::sync::mpsc::channel();
    let (done, results) = channel::channel();
    images::spawn_worker(worker, move |key, pixmap| done.send((key, pixmap)).is_ok())?;
    handle
        .insert_source(results, |event, _, app: &mut App| {
            if let channel::Event::Msg((key, pixmap)) = event {
                app.images.arrived(key, pixmap);
                app.mark(Dirty::BOTH);
            }
        })
        .map_err(|e| e.error)?;
    Ok(images::Images::new(requests))
}

/// How the bottom view is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomKind {
    /// A layer surface over the second screen's output (the device).
    Output,
    /// A layer surface in a desktop's corner, which never takes the keyboard focus.
    Panel,
    /// An ordinary window.
    Window,
}

impl BottomKind {
    /// From whether the second screen's output exists, whether the compositor has layer shell, and
    /// whether a panel was asked for.
    fn choose(output: bool, layer_shell: bool, panel: bool) -> BottomKind {
        match (output, layer_shell) {
            (true, true) => BottomKind::Output,
            (false, true) if panel => BottomKind::Panel,
            _ => BottomKind::Window,
        }
    }

    /// Keys stand in for the pad everywhere but on the device (there, keys reaching shelf are typing).
    fn desktop_keys(self) -> bool {
        self != BottomKind::Output
    }
}

/// Renders one screen without Wayland, at `size` logical pixels and integer `scale`, to a PNG file.
/// For checking layouts and for automated tests. Returns what a live frame would tell the state (shelf's
/// rows, the lists' rows).
pub fn snapshot(state: &State, screen: Screen, size: (u32, u32), scale: u32, path: &std::path::Path) -> Result<Vec<Event>, Error> {
    let mut picture = Picture::default();
    let at = Placement { size_px: (size.0 * scale, size.1 * scale), scale, drift: (0, 0) };
    let rendered = render::render(&mut picture, at, screen, state, &mut DrawCache::load(), &mut images::Images::synchronous())?;
    picture.pixmap.ok_or("no picture")?.save_png(path)?;
    Ok(frame_events(state, &rendered.out))
}

/// What a frame says about the lists it drew that the state doesn't know yet: how many rows each
/// shows (as the state has it, give or take rounding).
fn list_changes(state: &State, out: &toolkit::FrameOut) -> impl Iterator<Item = Event> {
    ListId::ALL.into_iter().filter_map(move |list| {
        let rows = out.lists.get(list.index()).copied().flatten()?;
        let known = state.scroll(list).visible().is_some_and(|v| (v - rows).abs() < 1e-3);
        (!known).then(|| ListEvent::Visible(list, rows).into())
    })
}

/// What a frame drawn without Wayland reports back to the state, as a live one would: shelf's rows, the
/// lists' visible rows.
fn frame_events(state: &State, out: &toolkit::FrameOut) -> Vec<Event> {
    let mut events: Vec<Event> = out.list_rows.filter(|&r| r != state.shelf().rows()).map(|r| ShelfEvent::Rows(r).into()).into_iter().collect();
    events.extend(list_changes(state, out));
    events
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<App>,
    /// Made once the outputs are known (see `run`).
    views: Screens<Option<View>>,
    state: State,
    fonts: DrawCache,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    touch_layout: Arc<Mutex<TouchLayout>>,
    images: images::Images,
    input: Input,
    /// The presses' running timers.
    timers: HashMap<(Source, input::Timer), RegistrationToken>,
    /// Where the pad's focus was on each screen, for View switching back to it.
    focus_memory: Screens<Option<Id>>,
    /// Keys stand in for the pad (a desktop; not on the device, where keys reaching shelf are typing).
    desktop_keys: bool,
    text_input: text_input::TextInput,
    /// The compositor's idle report, the screens' power and the main screen's refresh rate.
    display: display::Display,
    loop_handle: LoopHandle<'static, App>,
    on_effect: Box<dyn FnMut(&Effect)>,
    exit: bool,
}

impl App {
    fn new(
        globals: &GlobalList,
        qh: &QueueHandle<App>,
        state: State,
        config: &Config,
        images: images::Images,
        loop_handle: LoopHandle<'static, App>,
        on_effect: Box<dyn FnMut(&Effect)>,
    ) -> Result<App, Error> {
        let shm = Shm::bind(globals, qh)?;
        let pool = SlotPool::new(4 << 20, &shm)?;
        Ok(App {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            shm,
            pool,
            qh: qh.clone(),
            views: Screens::default(),
            state,
            fonts: DrawCache::load(),
            keyboard: None,
            pointer: None,
            touch: None,
            touch_layout: config.touch_layout.clone(),
            images,
            input: Input::default(),
            timers: HashMap::new(),
            focus_memory: Screens::default(),
            desktop_keys: true,
            text_input: text_input::TextInput::bind(globals, qh),
            display: display::Display::bind(globals, qh, &config.bottom_output),
            loop_handle,
            on_effect,
            exit: false,
        })
    }

    /// The top view as a window; the bottom one as `BottomKind::choose` says.
    fn place_views(&mut self, shells: &Shells, qh: &QueueHandle<App>, config: &Config) {
        self.views.top = Some(View::window(&shells.compositor, &shells.xdg, qh, Screen::Top, config.top_size));
        self.display.all_on(self.output_state.outputs(), qh);
        let output = self.output_state.outputs().find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(config.bottom_output.as_str()));
        let kind = BottomKind::choose(output.is_some(), shells.layer.is_some(), config.bottom_panel);
        self.desktop_keys = kind.desktop_keys();
        self.views.bottom = Some(match (kind, &shells.layer) {
            (BottomKind::Output, Some(layer_shell)) => {
                log::info!("bottom view: layer surface on {}", config.bottom_output);
                View::layer(&shells.compositor, layer_shell, qh, output.as_ref(), None)
            }
            (BottomKind::Panel, Some(layer_shell)) => {
                log::info!("bottom view: a panel in the corner (it never takes keyboard focus)");
                View::layer(&shells.compositor, layer_shell, qh, None, Some(config.bottom_size))
            }
            _ => {
                log::info!("bottom view: a window (no output {})", config.bottom_output);
                View::window(&shells.compositor, &shells.xdg, qh, Screen::Bottom, config.bottom_size)
            }
        });
    }

    fn view_mut(&mut self, surface: &WlSurface) -> Option<&mut View> {
        self.views.iter_mut().filter_map(|(_, v)| v.as_mut()).find(|v| v.shell.surface() == surface)
    }

    fn handle(&mut self, event: Event) {
        // The pad driving the screens is input, handled here like keys and touches.
        if let Event::Pad(input) = event {
            return self.pad(input);
        }
        let (dirty, effects) = self.state.apply(event);
        self.mark(dirty);
        let released = self.input.pad_route(self.state.pad().route(), &self.state);
        self.run_actions(released);
        for effect in &effects {
            match effect {
                Effect::Quit => self.exit = true,
                // The screens' power is the compositor's, asked through Wayland.
                Effect::Screens(on) => {
                    let outputs: Vec<_> = self.output_state.outputs().collect();
                    self.display.power(*on, outputs.into_iter(), &self.qh);
                }
                other => {
                    // The main screen's refresh rate follows the settings in force (Wayland too).
                    if let Effect::Apply { settings, .. } | Effect::Launch { settings, .. } = other {
                        self.display.refresh(settings.get(clamshell_core::settings::Setting::Refresh), &self.qh);
                    }
                    (self.on_effect)(other)
                }
            }
        }
    }

    fn handle_all(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.handle(event);
        }
    }

    fn mark(&mut self, dirty: Dirty) {
        let dirty = Screens::from(dirty);
        for (screen, view) in self.views.iter_mut() {
            if let Some(view) = view {
                view.dirty |= dirty[screen];
            }
        }
    }

    fn redraw_dirty(&mut self) {
        let mut rows_changed = None;
        let mut lists = Vec::new();
        // ClamShell's bottom view behind a window moved to the bottom screen while that is shown.
        let behind = self.state.bottom_shows_windows();
        if let Some(view) = self.views.iter_mut().find(|(s, _)| *s == Screen::Bottom).and_then(|(_, v)| v.as_mut()) {
            view.set_behind(behind);
        }
        for (screen, view) in self.views.iter_mut() {
            let Some(view) = view.as_mut().filter(|v| v.due()) else {
                continue;
            };
            match view.draw(&mut self.pool, &mut self.fonts, &self.state, &mut self.images, &self.qh) {
                Ok(rendered) => {
                    if let Some(area) = rendered.out.touchpad_area {
                        let scale = view.scale.max(1) as f32;
                        let screen_px = (view.size.0 as f32 * scale, view.size.1 as f32 * scale);
                        view::publish(&self.touch_layout, &view.picture.targets, area, rendered.origin.0, scale, screen_px);
                    }
                    rows_changed = rows_changed.or(rendered.out.list_rows.filter(|&rows| rows != self.state.shelf().rows()));
                    lists.extend(list_changes(&self.state, &rendered.out));
                }
                Err(e) => {
                    log::warn!("drawing {screen:?}: {e}");
                    view.dirty = false;
                }
            }
        }
        // The list's row count follows the layout; the state keeps the selection within it.
        if let Some(rows) = rows_changed {
            self.handle(Event::Shelf(ShelfEvent::Rows(rows)));
        }
        self.handle_all(lists);
    }

    /// Carries out what the input side asks: events, and the presses' timers on the event loop.
    fn run_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Emit(event) => self.handle(event),
                Action::Arm(source, timer, after) => self.arm(source, timer, after),
                Action::Disarm(source, timer) => {
                    if let Some(token) = self.timers.remove(&(source, timer)) {
                        self.loop_handle.remove(token);
                    }
                }
            }
        }
    }

    fn arm(&mut self, source: Source, timer: input::Timer, after: Duration) {
        if let Some(token) = self.timers.remove(&(source, timer)) {
            self.loop_handle.remove(token);
        }
        let fire = move |_, _: &mut (), app: &mut App| {
            // This timer is done (dropped by returning Drop), not to be removed from inside itself.
            app.timers.remove(&(source, timer));
            let actions = app.input.timer(source, timer, &app.state, std::time::Instant::now());
            app.run_actions(actions);
            TimeoutAction::Drop
        };
        match self.loop_handle.insert_source(Timer::from_duration(after), fire) {
            Ok(token) => {
                self.timers.insert((source, timer), token);
            }
            Err(e) => log::warn!("{timer:?} timer: {}", e.error),
        }
    }

    /// A finger or button comes down at `position` on `surface`: a press on the topmost (last drawn)
    /// target there, so a button on a tile (the dock's "+") wins over the tile.
    fn press_at(&mut self, source: Source, surface: &WlSurface, (x, y): (f64, f64)) {
        let Some(view) = self.view_mut(surface) else {
            return;
        };
        let Some(target) = view.picture.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).cloned() else {
            return;
        };
        let actions = self.input.touch(source, &target, (x as f32, y as f32), std::time::Instant::now());
        self.run_actions(actions);
    }

    /// The wheel over a scrolled list on `surface` at `position`: it scrolls by `notches` rows.
    fn wheel(&mut self, surface: &WlSurface, (x, y): (f64, f64), notches: i32) {
        let Some(view) = self.view_mut(surface) else {
            return;
        };
        let list = view.picture.targets.iter().rev().find(|t| t.rect.contains(x as f32, y as f32)).and_then(|t| t.list);
        if let Some(list) = list {
            self.handle(ListEvent::Scroll(list.list, notches as f32).into());
        }
    }

    fn moved(&mut self, source: Source, (x, y): (f64, f64)) {
        let actions = self.input.moved(source, (x as f32, y as f32), std::time::Instant::now());
        self.run_actions(actions);
    }

    fn up(&mut self, source: Source, end: End) {
        let actions = self.input.up(source, end, &self.state, std::time::Instant::now());
        self.run_actions(actions);
    }

    fn cancel_touches(&mut self) {
        let actions = self.input.cancel_touches(&self.state);
        self.run_actions(actions);
    }

    /// The pad on ClamShell's screens (see `focus`): directions move focus, A acts on the focused
    /// target, B goes back (see `focus::back`), Y toggles the favourite, X the pin on the dock, the shoulders
    /// page through shelf (or step the second screen's tabs), the triggers go to shelf's first or last entry.
    /// View moves focus to the other screen with shelf in front; else it moves the pad between the second
    /// screen and what else wants it (the pad mouse, typing; the pad thread sends View from there too).
    fn pad(&mut self, input: PadInput) {
        // Moving a dock entry: the D-pad moves it, A or B ends it.
        if self.state.dock().moving().is_some() && self.state.bottom().tab() == clamshell_core::Tab::Dock {
            let step = |dir| Some(clamshell_core::DockId::Step(dir));
            use clamshell_core::keymap::Dir as Step;
            let id = match input {
                PadInput::Up => step(Step::Up),
                PadInput::Down => step(Step::Down),
                PadInput::Left => step(Step::Left),
                PadInput::Right => step(Step::Right),
                PadInput::A(true) | PadInput::B => Some(clamshell_core::DockId::Done),
                _ => None,
            };
            if let Some(id) = id {
                self.handle(PointerEvent::Activate(Id::Dock(id)).into());
            }
            return;
        }
        let dir = match input {
            PadInput::Up => Some(Dir::Up),
            PadInput::Down => Some(Dir::Down),
            PadInput::Left => Some(Dir::Left),
            PadInput::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            let event = focus::navigate(&self.state, &self.views, &self.focus_memory, dir);
            return self.handle_all(event);
        }
        match input {
            PadInput::A(true) => self.press_focus(Source::PadA),
            PadInput::A(false) => self.up(Source::PadA, End::Release),
            PadInput::B => self.handle_all(focus::back(&self.state)),
            PadInput::Y => self.handle(ShelfEvent::ToggleFavourite.into()),
            PadInput::X => self.handle(ShelfEvent::TogglePin.into()),
            // The Back button: what the screen has for back (the edit screen's other layer).
            PadInput::Back => {
                if let Some(id) = self.state.pad_back() {
                    self.handle(PointerEvent::Activate(id).into());
                }
            }
            PadInput::Shoulder(step) => self.handle_all(focus::shoulder(&self.state, &self.views, step)),
            PadInput::Trigger(step) => self.handle_all(focus::trigger(&self.state, &self.views, step)),
            PadInput::Switch if self.state.pad_on_top() => {
                let event = focus::switch_screen(&self.state, &self.views, &mut self.focus_memory);
                self.handle_all(event);
            }
            // The pad between the second screen and what else wants it; back on the screen, the focus
            // where it was.
            PadInput::Switch => {
                let on_ui = |s: &State| s.pad().route() == clamshell_core::PadRoute::Ui;
                if on_ui(&self.state) {
                    self.focus_memory.bottom = self.state.pointer().focus().cloned();
                }
                self.handle(Event::PadSwitch);
                if on_ui(&self.state) {
                    let event = focus::focus_bottom(&self.state, &self.views, &self.focus_memory);
                    self.handle(event);
                }
            }
            PadInput::Up | PadInput::Down | PadInput::Left | PadInput::Right => {}
        }
    }

    /// A (or Enter) on the focused target.
    fn press_focus(&mut self, source: Source) {
        match focus::press(&self.state, &self.views, &self.focus_memory) {
            Some(PadPress::Press(id)) => {
                let actions = self.input.press(source, id, std::time::Instant::now());
                self.run_actions(actions);
            }
            Some(PadPress::Focus(event)) => self.handle(event),
            None => {}
        }
    }

    /// A key on a desktop (see `focus::desktop_key`).
    fn key(&mut self, sym: Keysym, down: bool) {
        if !self.desktop_keys {
            return;
        }
        match focus::desktop_key(sym) {
            Some(PadInput::A(_)) if down => self.press_focus(Source::Enter),
            Some(PadInput::A(_)) => self.up(Source::Enter, End::Release),
            Some(input) if down => self.pad(input),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bottom_view_goes_on_the_second_screen_when_there_is_one() {
        assert_eq!(BottomKind::choose(true, true, false), BottomKind::Output);
        assert_eq!(BottomKind::choose(true, true, true), BottomKind::Output);
        assert_eq!(BottomKind::choose(false, true, true), BottomKind::Panel);
        assert_eq!(BottomKind::choose(false, true, false), BottomKind::Window);
        assert_eq!(BottomKind::choose(true, false, true), BottomKind::Window);
        assert!(!BottomKind::Output.desktop_keys() && BottomKind::Panel.desktop_keys() && BottomKind::Window.desktop_keys());
    }
}
