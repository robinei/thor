//! The Wayland side: two views, the main (top) screen and the second (bottom) screen, drawn on the CPU
//! into shm buffers. A view is redrawn only when the state changed for it, at most once per frame
//! callback; between events the program sleeps.
//!
//! The top view is an xdg toplevel (sway rules make it fullscreen in its own workspace). The bottom view
//! is a layer-shell surface on the second screen's output when that output exists (the Thor: `DSI-1`),
//! else a second ordinary window, so it runs on any desktop.

mod draw;
mod icons;
pub mod nav;
mod views;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken, channel,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface::WlSurface, wl_touch},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        touch::TouchHandler,
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use sysprog_core::touch::{Region, TouchLayout};
use sysprog_core::{Dirty, Effect, Event, Hold, Id, State};
use tiny_skia::Pixmap;

use draw::{DrawCache, Rect};
use nav::{Dir, NavTarget};
use views::{Scratch, Target};

pub type Error = Box<dyn std::error::Error>;

/// A press held this long is a hold: keys start repeating, modifiers lock.
const LONG_PRESS: Duration = Duration::from_millis(450);
/// Key repeat interval while held (25 per second).
const REPEAT: Duration = Duration::from_millis(40);

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
        presses: HashMap::new(),
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
            View::layer(&compositor, layer_shell, &qh, &output)
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

/// OLED burn-in protection: every screen is laid out inside a black margin of MARGIN buffer pixels on
/// each side, and drawn moved within it by an offset that drifts slowly (about a pixel a minute, along an
/// irregular path within ±MARGIN), so static elements like the bar's icons and the clock don't sit on the
/// same pixels for hours, and nothing ever reaches the edge or is cut off. Tap areas move with the
/// picture.
const MARGIN: u32 = 4;

fn orbit(unix_secs: u64) -> (i32, i32) {
    let minutes = (unix_secs / 60) as f32;
    let tau = std::f32::consts::TAU;
    let reach = MARGIN as f32;
    let dx = (reach * (minutes * tau / 17.0).sin()).round() as i32;
    let dy = (reach * (minutes * tau / 23.0).sin()).round() as i32;
    (dx, dy)
}

/// The logical size to lay a screen out in (inside the margin), and the buffer-pixel offset to draw it
/// at for `orbit`.
fn inset(size_px: (u32, u32), scale: u32, (dx, dy): (i32, i32)) -> ((f32, f32), (i32, i32)) {
    let scale = scale.max(1) as f32;
    let w = size_px.0.saturating_sub(2 * MARGIN) as f32 / scale;
    let h = size_px.1.saturating_sub(2 * MARGIN) as f32 / scale;
    ((w, h), (MARGIN as i32 + dx, MARGIN as i32 + dy))
}

/// Renders one screen without Wayland, at `size` logical pixels and integer `scale`, to a PNG file.
/// For checking layouts and for automated tests.
pub fn snapshot(state: &State, screen: Screen, size: (u32, u32), scale: u32, path: &std::path::Path) -> Result<(), Error> {
    let mut pixmap = Pixmap::new(size.0 * scale, size.1 * scale).ok_or("can't allocate the pixmap")?;
    let mut fonts = DrawCache::load();
    let ((w, h), offset) = inset((size.0 * scale, size.1 * scale), scale, (0, 0));
    let mut canvas = draw::Canvas::new(&mut pixmap, scale as f32, &mut fonts).with_offset(offset);
    let mut targets = Vec::new();
    match screen {
        Screen::Top => views::top(&mut canvas, state, w, h, &mut targets),
        Screen::Bottom => {
            views::bottom(&mut canvas, state, w, h, &mut Scratch::default(), &mut targets);
        }
    }
    pixmap.save_png(path)?;
    Ok(())
}

enum Shell {
    Window(Window),
    Layer(LayerSurface),
}

impl Shell {
    fn surface(&self) -> &WlSurface {
        match self {
            Shell::Window(w) => w.wl_surface(),
            Shell::Layer(l) => l.wl_surface(),
        }
    }
}

struct View {
    screen: Screen,
    shell: Shell,
    /// Logical size, from the compositor's configure (or `default_size` if it leaves it to us).
    size: (u32, u32),
    default_size: (u32, u32),
    scale: i32,
    configured: bool,
    dirty: bool,
    /// A frame was committed and its callback hasn't come yet: wait, then draw the latest state once.
    frame_pending: bool,
    /// What the last frame drew that can be tapped or focused.
    targets: Vec<Target>,
    pixmap: Option<Pixmap>,
    /// shm buffers, reused once the compositor releases them (two in turn, a third if it holds both).
    buffers: Vec<Buffer>,
    scratch: Scratch,
}

impl View {
    fn new(screen: Screen, shell: Shell, default_size: (u32, u32)) -> Self {
        View {
            screen,
            shell,
            size: default_size,
            default_size,
            scale: 1,
            configured: false,
            dirty: true,
            frame_pending: false,
            targets: Vec::new(),
            pixmap: None,
            buffers: Vec::new(),
            scratch: Scratch::default(),
        }
    }

    fn window(
        compositor: &CompositorState,
        xdg: &XdgShell,
        qh: &QueueHandle<App>,
        screen: Screen,
        size: (u32, u32),
    ) -> Self {
        let surface = compositor.create_surface(qh);
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, qh);
        let (title, app_id) = match screen {
            Screen::Top => ("sysprog: top", "sysprog.top"),
            Screen::Bottom => ("sysprog: bottom", "sysprog.bottom"),
        };
        window.set_title(title);
        window.set_app_id(app_id);
        window.set_min_size(Some((200, 150)));
        window.commit();
        View::new(screen, Shell::Window(window), size)
    }

    fn layer(
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<App>,
        output: &wl_output::WlOutput,
    ) -> Self {
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(qh, surface, Layer::Top, Some("sysprog-bottom"), Some(output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        View::new(Screen::Bottom, Shell::Layer(layer), (1, 1))
    }

    fn draw(
        &mut self,
        pool: &mut SlotPool,
        fonts: &mut DrawCache,
        state: &State,
        touch: &Mutex<TouchLayout>,
        qh: &QueueHandle<App>,
    ) -> Result<(), Error> {
        let scale = self.scale.max(1);
        let (lw, lh) = self.size;
        let scale_u = scale.unsigned_abs();
        let (w, h) = (lw * scale_u, lh * scale_u);
        if self.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
            self.pixmap = Pixmap::new(w, h);
        }
        let Some(pixmap) = self.pixmap.as_mut() else {
            return Err(format!("can't allocate a {w}x{h} pixmap").into());
        };
        let ((cw, ch), offset) = inset((w, h), scale_u, orbit(state.unix_secs));
        let mut body = None;
        {
            let mut canvas = draw::Canvas::new(pixmap, scale as f32, fonts).with_offset(offset);
            let (w, h) = (cw, ch);
            match self.screen {
                Screen::Top => views::top(&mut canvas, state, w, h, &mut self.targets),
                Screen::Bottom => {
                    body = Some(views::bottom(&mut canvas, state, w, h, &mut self.scratch, &mut self.targets));
                }
            }
        }

        let (sx, sy) = (offset.0 as f32 / scale as f32, offset.1 as f32 / scale as f32);
        for target in &mut self.targets {
            target.rect.x += sx;
            target.rect.y += sy;
        }
        if let Some(body) = body {
            publish(touch, &self.targets, body, (sx, sy), scale as f32, (w as f32, h as f32));
        }

        let (wi, hi) = (i32::try_from(w)?, i32::try_from(h)?);
        // A buffer of this size the compositor isn't reading; a new one only if all are in use.
        self.buffers.retain(|b| b.height() == hi && b.stride() == wi * 4);
        let free = self.buffers.iter().position(|b| b.canvas(pool).is_some());
        let index = match free {
            Some(i) => i,
            None if self.buffers.len() < 3 => {
                let (buffer, _) = pool.create_buffer(wi, hi, wi * 4, wl_shm::Format::Argb8888)?;
                self.buffers.push(buffer);
                self.buffers.len() - 1
            }
            // All busy: keep the view dirty; a buffer release wakes the loop and it draws then.
            None => return Ok(()),
        };
        let Some(buffer) = self.buffers.get(index) else {
            return Err("buffer index".into());
        };
        let Some(bytes) = buffer.canvas(pool) else {
            return Err("buffer busy".into());
        };
        // tiny-skia is RGBA in memory, ARGB8888 is BGRA (little-endian); both premultiplied. sway also
        // takes ABGR8888 (RGBA in memory), which would save this copy; KWin doesn't.
        for (dst, src) in bytes.chunks_exact_mut(4).zip(pixmap.data().chunks_exact(4)) {
            if let ([b, g, r, a], [sr, sg, sb, sa]) = (dst, src) {
                (*b, *g, *r, *a) = (*sb, *sg, *sr, *sa);
            }
        }

        let surface = self.shell.surface();
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, wi, hi);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface)?;
        surface.commit();
        self.dirty = false;
        self.frame_pending = true;
        Ok(())
    }
}

/// Where a press comes from: each touch point, the mouse, or the pad's A (Enter on the desktop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    Touch(i32),
    Mouse,
    PadA,
}

/// A press in progress, on the target it started on.
struct Press {
    id: Id,
    /// The hold timer already acted (repeated or long-pressed): the release adds nothing.
    held: bool,
}

/// Tells the touchpad mode's input side where things are, in screen pixels: the area below the bar is
/// the touchpad; the bar's targets stay taps. No allocation once the region list has grown.
fn publish(touch: &Mutex<TouchLayout>, targets: &[Target], body: Rect, (sx, sy): (f32, f32), scale: f32, screen: (f32, f32)) {
    let Ok(mut layout) = touch.lock() else {
        return;
    };
    layout.screen = screen;
    layout.pad = ((body.x + sx) * scale, (body.y + sy) * scale, (body.x + sx + body.w) * scale, (body.y + sy + body.h) * scale);
    layout.regions.clear();
    for t in targets.iter().filter(|t| matches!(t.id, Id::Mode(_) | Id::PadMouse)) {
        let r = t.rect;
        layout.regions.push(Region { x0: r.x * scale, y0: r.y * scale, x1: (r.x + r.w) * scale, y1: (r.y + r.h) * scale, id: t.id.clone() });
    }
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
    presses: HashMap<Source, Press>,
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
        for view in [self.top.as_mut(), self.bottom.as_mut()].into_iter().flatten() {
            if !(view.configured && view.dirty && !view.frame_pending) {
                continue;
            }
            if let Err(e) = view.draw(&mut self.pool, &mut self.fonts, &self.state, &self.touch_layout, &self.qh) {
                log::warn!("drawing {:?}: {e}", view.screen);
                view.dirty = false;
            }
        }
    }

    /// What the last frame drew at `position` on `surface`.
    fn target_at(&mut self, surface: &WlSurface, (x, y): (f64, f64)) -> Option<Id> {
        let view = self.view_mut(surface)?;
        view.targets.iter().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| t.id.clone())
    }

    /// A press starts: light the target, and arm the hold timer. A tap acts on release; a hold acts
    /// from the timer (repeat, or one long press).
    fn down(&mut self, source: Source, id: Id) {
        if self.presses.contains_key(&source) {
            return;
        }
        if matches!(source, Source::Touch(_) | Source::Mouse) {
            self.handle(Event::HideFocus);
        }
        self.presses.insert(source, Press { id: id.clone(), held: false });
        self.handle(Event::Down(id));
        self.cancel_hold_timer();
        let timer = Timer::from_duration(LONG_PRESS);
        match self.loop_handle.insert_source(timer, move |_, _, app: &mut App| app.on_hold(source)) {
            Ok(token) => self.hold_timer = Some((source, token)),
            Err(e) => log::warn!("hold timer: {}", e.error),
        }
    }

    fn on_hold(&mut self, source: Source) -> TimeoutAction {
        let Some(press) = self.presses.get_mut(&source) else {
            self.hold_timer = None;
            return TimeoutAction::Drop;
        };
        press.held = true;
        let id = press.id.clone();
        match self.state.hold(&id) {
            Hold::Repeat => {
                self.handle(Event::Activate(id));
                TimeoutAction::ToDuration(REPEAT)
            }
            Hold::Long => {
                self.hold_timer = None;
                self.handle(Event::LongPress(id));
                TimeoutAction::Drop
            }
        }
    }

    /// A press ends: a tap activates; a hold already acted.
    fn up(&mut self, source: Source, activate: bool) {
        if let Some((timer_source, token)) = self.hold_timer
            && timer_source == source
        {
            self.loop_handle.remove(token);
            self.hold_timer = None;
        }
        let Some(press) = self.presses.remove(&source) else {
            return;
        };
        if activate && !press.held {
            self.handle(Event::Activate(press.id.clone()));
        }
        self.handle(Event::Up(press.id));
    }

    fn cancel_hold_timer(&mut self) {
        if let Some((_, token)) = self.hold_timer.take() {
            self.loop_handle.remove(token);
        }
    }

    /// All touch points gone without a tap (cancelled, or the touch device went away).
    fn cancel_touches(&mut self) {
        let touches: Vec<Source> = self.presses.keys().copied().filter(|s| matches!(s, Source::Touch(_))).collect();
        for source in touches {
            self.up(source, false);
        }
    }

    /// Pad-style focus movement over both screens' targets.
    fn navigate(&mut self, dir: Dir) {
        let mut all = Vec::new();
        for view in self.views() {
            let (w, h) = (view.size.0.max(1) as f32, view.size.1.max(1) as f32);
            let y0 = match view.screen {
                Screen::Top => 0.0,
                Screen::Bottom => 1.0,
            };
            all.extend(view.targets.iter().map(|t| NavTarget {
                screen: view.screen,
                rect: Rect::new(t.rect.x / w, y0 + t.rect.y / h, t.rect.w / w, t.rect.h / h),
                id: t.id.clone(),
            }));
        }
        if let Some(id) = nav::step(&all, self.state.focus.as_ref(), dir) {
            self.handle(Event::Focus(id));
        }
    }

    /// The keyboard stands in for the pad on the desktop: arrows move focus, Enter/Space is A (press,
    /// hold), Escape quits.
    fn key(&mut self, sym: Keysym, down: bool) {
        match (sym, down) {
            (Keysym::Left, true) => self.navigate(Dir::Left),
            (Keysym::Right, true) => self.navigate(Dir::Right),
            (Keysym::Up, true) => self.navigate(Dir::Up),
            (Keysym::Down, true) => self.navigate(Dir::Down),
            (Keysym::Return | Keysym::KP_Enter | Keysym::space, true) => {
                if let Some(id) = self.state.focus.clone() {
                    self.down(Source::PadA, id);
                }
            }
            (Keysym::Return | Keysym::KP_Enter | Keysym::space, false) => self.up(Source::PadA, true),
            (Keysym::Escape, true) => self.handle(Event::Quit),
            _ => {}
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, factor: i32) {
        if let Some(view) = self.view_mut(surface) {
            view.scale = factor.max(1);
            view.dirty = true;
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(view) = self.view_mut(surface) {
            view.frame_pending = false;
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window, configure: WindowConfigure, _: u32) {
        if let Some(view) = self.view_mut(window.wl_surface()) {
            let (w, h) = configure.new_size;
            view.size = (w.map_or(view.default_size.0, |w| w.get()), h.map_or(view.default_size.1, |h| h.get()));
            view.configured = true;
            view.dirty = true;
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        log::warn!("the compositor closed the bottom layer surface");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(view) = self.view_mut(layer.wl_surface()) {
            let (w, h) = configure.new_size;
            view.size = (if w == 0 { view.default_size.0 } else { w }, if h == 0 { view.default_size.1 } else { h });
            view.configured = true;
            view.dirty = true;
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).map_err(|e| log::warn!("keyboard: {e}")).ok();
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = self.seat_state.get_pointer(qh, &seat).map_err(|e| log::warn!("pointer: {e}")).ok();
            }
            Capability::Touch if self.touch.is_none() => {
                self.touch = self.seat_state.get_touch(qh, &seat).map_err(|e| log::warn!("touch: {e}")).ok();
            }
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            Capability::Pointer => {
                if let Some(p) = self.pointer.take() {
                    p.release();
                }
            }
            Capability::Touch => {
                if let Some(t) = self.touch.take() {
                    t.release();
                }
                self.cancel_touches();
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32) {}

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event.keysym, true);
    }

    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if matches!(event.keysym, Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down) {
            self.key(event.keysym, true);
        }
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event.keysym, false);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for event in events {
            match event.kind {
                PointerEventKind::Press { .. } => {
                    if let Some(id) = self.target_at(&event.surface, event.position) {
                        self.down(Source::Mouse, id);
                    }
                }
                PointerEventKind::Release { .. } => self.up(Source::Mouse, true),
                _ => {}
            }
        }
    }
}

impl TouchHandler for App {
    fn down(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _: u32,
        _: u32,
        surface: WlSurface,
        id: i32,
        position: (f64, f64),
    ) {
        if let Some(target) = self.target_at(&surface, position) {
            self.down(Source::Touch(id), target);
        }
    }

    fn up(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, _: u32, id: i32) {
        self.up(Source::Touch(id), true);
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, _: i32, _: (f64, f64)) {}

    fn shape(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64, _: f64) {}

    fn orientation(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64) {}

    fn cancel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch) {
        self.cancel_touches();
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);

/// Drawing a frame must not touch the heap once caches are warm (glyphs rasterized, buffers sized):
/// counted with an allocator that tallies this thread's allocations.
#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod no_alloc {
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
    fn orbit_stays_in_the_margin_and_moves_slowly() {
        let mut last = super::orbit(0);
        for minute in 1..2000u64 {
            let (dx, dy) = super::orbit(minute * 60);
            assert!(dx.unsigned_abs() <= super::MARGIN && dy.unsigned_abs() <= super::MARGIN);
            assert!((dx - last.0).abs() <= 2 && (dy - last.1).abs() <= 2);
            last = (dx, dy);
        }
    }

    #[test]
    fn warm_frames_do_not_allocate() {
        let mut state = State::new(sysprog_library::demo_games());
        state.apply(Event::Tick { unix_secs: 1_700_000_000 });
        let mut cache = DrawCache::load();
        let mut scratch = Scratch::default();
        let mut targets = Vec::new();
        let mut top = Pixmap::new(1920, 1080).expect("pixmap");
        let mut bottom = Pixmap::new(1240, 1080).expect("pixmap");

        let cases: Vec<(&str, Vec<Event>)> = vec![
            ("game settings", vec![]),
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
                views::top(&mut Canvas::new(&mut top, 1.0, &mut cache), &state, 1920.0, 1080.0, &mut targets);
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
}
