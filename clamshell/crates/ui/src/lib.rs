//! The Wayland side: two views, the main (top) screen and the second (bottom) screen, drawn on the CPU
//! into shm buffers. A view is redrawn only when the state changed for it, at most once per frame
//! callback; between events the program sleeps.
//!
//! The top view is an xdg toplevel (sway rules make it fullscreen in its own workspace). The bottom view
//! is a layer-shell surface on the second screen's output when that output exists (the Thor: `DSI-1`),
//! else a second ordinary window, so it runs on any desktop.


mod draw;
mod icons;
pub mod images;
mod input;
pub mod nav;
#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod no_alloc;
mod view;
mod views;
mod wayland;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay_client_toolkit::{
    compositor::CompositorState,
    output::OutputState,
    reexports::{
        calloop::{EventLoop, LoopHandle, RegistrationToken, channel},
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_keyboard, wl_pointer, wl_surface::WlSurface, wl_touch},
        },
    },
    registry::RegistryState,
    seat::SeatState,
    shell::{wlr_layer::LayerShell, xdg::XdgShell},
    shm::{Shm, slot::SlotPool},
};
use clamshell_core::touch::TouchLayout;
use clamshell_core::{Dirty, Effect, Event, State};
use tiny_skia::Pixmap;

use draw::DrawCache;
use input::{Press, Source};
use view::{View, inset};
use views::Scratch;

pub type Error = Box<dyn std::error::Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Top,
    Bottom,
}

pub struct Config {
    /// Output that gets the bottom view as a layer-shell surface; without it, the bottom view is a window.
    pub bottom_output: String,
    /// Logical sizes when shown as windows (on a desktop).
    pub top_size: (u32, u32),
    pub bottom_size: (u32, u32),
    /// Without the second screen's output (a desktop): show the bottom view as a panel in a corner
    /// instead of a window, so tapping its keyboard leaves the keyboard focus on the app being typed into.
    pub bottom_panel: bool,
    /// Where the touchpad area and the bar's targets are, for the touchpad mode's input side; updated
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

/// Runs the UI until quit. `start` gets a sender for the policy threads; `on_effect` carries out what the
/// state asks for (launching games, ...).
pub fn run(
    state: State,
    config: Config,
    start: impl FnOnce(EventSender),
    on_effect: impl FnMut(&Effect) + 'static,
) -> Result<(), Error> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let xdg = XdgShell::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh).ok();
    let shm = Shm::bind(&globals, &qh)?;
    let pool = SlotPool::new(4 << 20, &shm)?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    // Images: requests to a decoding thread, results back through the event loop.
    let (image_requests, image_worker) = std::sync::mpsc::channel();
    let (image_done, image_results) = channel::channel();
    images::spawn_worker(image_worker, move |key, pixmap| image_done.send((key, pixmap)).is_ok())?;
    event_loop
        .handle()
        .insert_source(image_results, |event, _, app: &mut App| {
            if let channel::Event::Msg((key, pixmap)) = event {
                app.images.arrived(key, pixmap);
                app.mark(Dirty::BOTH);
            }
        })
        .map_err(|e| e.error)?;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        qh: qh.clone(),
        top: None,
        bottom: None,
        state,
        fonts: DrawCache::load(),
        keyboard: None,
        pointer: None,
        touch: None,
        touch_layout: config.touch_layout.clone(),
        images: images::Images::new(image_requests),
        presses: HashMap::new(),
        slider_source: None,
        hold_timer: None,
        loop_handle: event_loop.handle(),
        on_effect: Box::new(on_effect),
        exit: false,
    };

    // Learn the outputs and their names before placing the bottom view.
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    app.top = Some(View::window(&compositor, &xdg, &qh, Screen::Top, config.top_size));
    let bottom_output = app.output_state.outputs().find(|o| {
        app.output_state.info(o).and_then(|i| i.name).as_deref() == Some(config.bottom_output.as_str())
    });
    app.bottom = Some(match (bottom_output, &layer_shell) {
        (Some(output), Some(layer_shell)) => {
            log::info!("bottom view: layer surface on {}", config.bottom_output);
            View::layer(&compositor, layer_shell, &qh, Some(&output), None)
        }
        (None, Some(layer_shell)) if config.bottom_panel => {
            log::info!("bottom view: a panel in the corner (it never takes keyboard focus)");
            View::layer(&compositor, layer_shell, &qh, None, Some(config.bottom_size))
        }
        _ => {
            log::info!("bottom view: a window (no output {})", config.bottom_output);
            View::window(&compositor, &xdg, &qh, Screen::Bottom, config.bottom_size)
        }
    });

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

    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
        app.redraw_dirty();
    }
    Ok(())
}

/// Renders one screen without Wayland, at `size` logical pixels and integer `scale`, to a PNG file.
/// For checking layouts and for automated tests. Returns the top screen's list rows, as a live frame
/// reports them.
pub fn snapshot(state: &State, screen: Screen, size: (u32, u32), scale: u32, path: &std::path::Path) -> Result<Option<usize>, Error> {
    let mut pixmap = Pixmap::new(size.0 * scale, size.1 * scale).ok_or("can't allocate the pixmap")?;
    let mut fonts = DrawCache::load();
    let ((w, h), offset) = inset((size.0 * scale, size.1 * scale), scale, (0, 0));
    let mut canvas = draw::Canvas::new(&mut pixmap, scale as f32, &mut fonts).with_offset(offset);
    let mut targets = Vec::new();
    let mut rows = None;
    match screen {
        Screen::Top => {
            rows = Some(views::top(&mut canvas, state, w, h, &mut images::Images::synchronous(), &mut Scratch::default(), &mut targets));
        }
        Screen::Bottom => {
            views::bottom(&mut canvas, state, w, h, &mut images::Images::synchronous(), &mut Scratch::default(), &mut targets);
        }
    }
    pixmap.save_png(path)?;
    Ok(rows)
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<App>,
    top: Option<View>,
    bottom: Option<View>,
    state: State,
    fonts: DrawCache,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    touch_layout: Arc<Mutex<TouchLayout>>,
    images: images::Images,
    presses: HashMap<Source, Press>,
    /// The press that is moving a slider, if any.
    slider_source: Option<Source>,
    /// Fires when the latest press becomes a hold, then at the repeat rate for repeating keys.
    hold_timer: Option<(Source, RegistrationToken)>,
    loop_handle: LoopHandle<'static, App>,
    on_effect: Box<dyn FnMut(&Effect)>,
    exit: bool,
}

impl App {
    fn views(&self) -> impl Iterator<Item = &View> {
        [self.top.as_ref(), self.bottom.as_ref()].into_iter().flatten()
    }

    fn view_mut(&mut self, surface: &WlSurface) -> Option<&mut View> {
        [self.top.as_mut(), self.bottom.as_mut()].into_iter().flatten().find(|v| v.shell.surface() == surface)
    }

    fn handle(&mut self, event: Event) {
        let (dirty, effects) = self.state.apply(event);
        self.mark(dirty);
        for effect in &effects {
            match effect {
                Effect::Quit => self.exit = true,
                other => (self.on_effect)(other),
            }
        }
    }

    fn mark(&mut self, dirty: Dirty) {
        for view in [self.top.as_mut(), self.bottom.as_mut()].into_iter().flatten() {
            view.dirty |= match view.screen {
                Screen::Top => dirty.top,
                Screen::Bottom => dirty.bottom,
            };
        }
    }

    fn redraw_dirty(&mut self) {
        let mut rows_changed = None;
        for view in [self.top.as_mut(), self.bottom.as_mut()].into_iter().flatten() {
            if !(view.configured && view.dirty && !view.frame_pending) {
                continue;
            }
            match view.draw(&mut self.pool, &mut self.fonts, &self.state, &self.touch_layout, &mut self.images, &self.qh) {
                Ok(Some(rows)) if rows != self.state.rows => rows_changed = Some(rows),
                Ok(_) => {}
                Err(e) => {
                    log::warn!("drawing {:?}: {e}", view.screen);
                    view.dirty = false;
                }
            }
        }
        // The list's row count follows the layout; the state keeps the selection within it.
        if let Some(rows) = rows_changed {
            self.handle(Event::Rows(rows));
        }
    }
}
