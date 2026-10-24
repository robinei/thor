//! Protocol objects and the bridge from client lifecycles into ClamShell's state.

use std::sync::Arc;
use std::time::Instant;

use clamshell_state::{Effect, State, WindowCommand, WindowEvent, session::windows::Window};
use clamshell_ui::{Config, Core, Error, Screen};
use smithay::{
    desktop::{PopupManager, Window as SurfaceWindow},
    input::{Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{
        DisplayHandle, Resource,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Transform},
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        input_method::InputMethodSeat,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgShellState, XdgToplevelSurfaceData},
        shm::ShmState,
        text_input::{TextInputManagerState, TextInputSeat},
    },
};

/// Identity read once from the client's credentials by the system-side resolver.
#[derive(Default)]
pub struct Process {
    pub unit: Option<String>,
    pub pids: Vec<u32>,
}

pub(crate) struct ClientWindow {
    pub id: i64,
    pub window: SurfaceWindow,
    pub opened: bool,
    pub mapped: bool,
    pub presentation: Option<clamshell_state::session::presentation::WindowPresentation>,
}

pub(crate) struct Server {
    pub core: Core,
    pub dh: DisplayHandle,
    pub compositor: CompositorState,
    pub shell: XdgShellState,
    pub shm: ShmState,
    pub seats: SeatState<Self>,
    pub data_device: DataDeviceState,
    pub seat: Seat<Self>,
    pub popups: PopupManager,
    pub windows: Vec<ClientWindow>,
    pub outputs: [Output; 2],
    pub pointer: Point<f64, Logical>,
    pub pointer_window: Option<i64>,
    pub pointer_buttons: Vec<u32>,
    pub arrow_pressed: bool,
    pub keyboard_home: bool,
    pub keyboard_source: smithay::input::keyboard::KeyboardSource,
    pub touches: [Option<crate::touch::Contact>; 32],
    pub touch_pending: bool,
    pub size: (i32, i32),
    pub top_width: i32,
    pub exit: bool,
    pub started: Instant,
    pub idle: crate::idle::Idle,
    pub presentation_revision: Option<u64>,
    pub socket: String,
    pub on_effect: Box<dyn FnMut(&Effect)>,
    identify: Box<dyn Fn(u32) -> Process>,
    next_id: i64,
}

impl Server {
    pub fn new(
        state: State,
        config: Config,
        dh: DisplayHandle,
        wake: impl Fn() + Send + Sync + 'static,
        identify: impl Fn(u32) -> Process + 'static,
        on_effect: impl FnMut(&Effect) + 'static,
    ) -> Result<Self, Error> {
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "clamshell");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        seat.add_touch();
        seat.input_method();
        seat.text_input().set_compositor_input_method(true);
        TextInputManagerState::new::<Self>(&dh);
        smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(&dh);
        let outputs = ["ClamShell-Top", "ClamShell-Bottom"].map(|name| {
            let output = Output::new(
                name.into(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "ClamShell".into(),
                    model: "Nested".into(),
                    serial_number: name.into(),
                },
            );
            output.create_global::<Self>(&dh);
            output
        });
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut server = Self {
            core: Core::new(state, config.touch_layout, wake)?,
            compositor: CompositorState::new::<Self>(&dh),
            shell: XdgShellState::new::<Self>(&dh),
            shm: ShmState::new::<Self>(&dh, vec![smithay::reexports::wayland_server::protocol::wl_shm::Format::Abgr8888]),
            data_device: DataDeviceState::new::<Self>(&dh),
            seats,
            seat,
            dh,
            outputs,
            popups: PopupManager::default(),
            windows: Vec::new(),
            pointer: (0.0, 0.0).into(),
            pointer_window: None,
            pointer_buttons: Vec::new(),
            arrow_pressed: false,
            keyboard_home: false,
            keyboard_source: smithay::input::keyboard::KeyboardSource::new_auxiliary(),
            touches: std::array::from_fn(|_| None),
            touch_pending: false,
            size: (1580, 540),
            top_width: 960,
            exit: false,
            started: Instant::now(),
            idle: crate::idle::Idle::new(),
            presentation_revision: None,
            socket: String::new(),
            on_effect: Box::new(on_effect),
            identify: Box::new(identify),
            next_id: 1,
        };
        server.resize((1580, 540));
        Ok(server)
    }

    pub fn resize(&mut self, (width, height): (i32, i32)) {
        self.touch_cancel();
        self.size = (width.max(4), height.max(4));
        self.top_width = (self.size.0 * 960 / 1580).max(2);
        for (screen, output, x, w) in
            [(Screen::Top, &self.outputs[0], 0, self.top_width), (Screen::Bottom, &self.outputs[1], self.top_width, self.size.0 - self.top_width)]
        {
            let mode = Mode { size: (w, self.size.1).into(), refresh: 60_000 };
            output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((x, 0).into()));
            output.set_preferred(mode);
            self.core.resize(screen, (w as u32, self.size.1 as u32), 1);
            self.core.record(screen, true);
        }
        self.sync_windows();
    }

    pub fn area(&self, down: bool) -> Rectangle<i32, Logical> {
        if down {
            let b = self.core.content_bounds(Screen::Bottom).unwrap_or_default();
            Rectangle::new((self.top_width + b.x0, b.y0).into(), ((b.x1 - b.x0).max(1), (b.y1 - b.y0).max(1)).into())
        } else {
            Rectangle::from_size((self.top_width, self.size.1).into())
        }
    }

    pub fn surface_id(&self, surface: &WlSurface) -> Option<i64> {
        self.windows.iter().find(|w| w.window.toplevel().is_some_and(|t| t.wl_surface() == surface)).map(|w| w.id)
    }

    pub fn add_toplevel(&mut self, surface: ToplevelSurface) {
        let Some(next) = self.next_id.checked_add(1) else {
            log::error!("compositor: window ids exhausted");
            surface.send_close();
            return;
        };
        self.windows.push(ClientWindow {
            id: self.next_id,
            window: SurfaceWindow::new_wayland_window(surface),
            opened: false,
            mapped: false,
            presentation: None,
        });
        self.next_id = next;
    }

    pub fn facts(&self, surface: &ToplevelSurface, id: i64) -> Window {
        let (app_id, title, parent) = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| (data.app_id.clone().unwrap_or_default(), data.title.clone().unwrap_or_default(), data.parent.clone()))
                .unwrap_or_default()
        });
        let process =
            self.core.state().session().window(id).map(|window| Process { unit: window.unit.clone(), pids: window.pids.clone() }).unwrap_or_else(|| {
                surface
                    .wl_surface()
                    .client()
                    .and_then(|c| c.get_credentials(&self.dh).ok())
                    .and_then(|c| u32::try_from(c.pid).ok())
                    .map(|pid| (self.identify)(pid))
                    .unwrap_or_default()
            });
        let limits = Self::size_limits(surface);
        Window { id, app_id, title, parent: parent.and_then(|p| self.surface_id(&p)), unit: process.unit, pids: process.pids, limits, ..Window::default() }
    }

    fn size_limits(surface: &ToplevelSurface) -> clamshell_state::session::presentation::WindowLimits {
        with_states(surface.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            clamshell_state::session::presentation::WindowLimits {
                min: (current.min_size.w.max(0) as u32, current.min_size.h.max(0) as u32),
                max: (current.max_size.w.max(0) as u32, current.max_size.h.max(0) as u32),
            }
        })
    }

    pub fn open_on_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.windows.iter().position(|w| w.window.toplevel().is_some_and(|t| t.wl_surface() == surface)) else { return };
        let Some(client) = self.windows.get(index) else { return };
        let Some(toplevel) = client.window.toplevel().cloned() else { return };
        let id = client.id;
        if !client.opened {
            let mut facts = self.facts(&toplevel, id);
            facts.focused = self.core.state().session().opening_focus(&facts);
            self.core.handle(WindowEvent::Opened(facts).into());
            if let Some(client) = self.windows.get_mut(index) {
                client.opened = true;
            }
        } else {
            let limits = Self::size_limits(&toplevel);
            if self.core.state().session().window(id).is_some_and(|old| old.limits != limits) {
                self.core.handle(WindowEvent::Changed(self.facts(&toplevel, id)).into());
            }
        }
        self.sync_windows();
        if !toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    }

    pub fn sync_windows(&mut self) {
        self.cancel_hidden_touches();
        let revision = self.core.state_revision();
        if self.presentation_revision != Some(revision) {
            for client in &mut self.windows {
                client.presentation = self.core.state().window_presentation(client.id);
            }
            self.presentation_revision = Some(revision);
        }
        let focus = self.core.state().session().keyboard_focus();
        for client in &self.windows {
            let Some(window) = self.core.state().session().window(client.id) else { continue };
            let Some(surface) = client.window.toplevel() else { continue };
            let area = self.area(window.down);
            let Some(policy) = client.presentation else { continue };
            let size = crate::geometry::configure_size(window, policy, area.size, client.window.geometry().size);
            surface.with_pending_state(|pending| {
                pending.size = size;
                pending.bounds = Some(area.size);
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                if policy.layout == clamshell_state::session::presentation::WindowLayout::Fullscreen {
                    pending.states.set(State::Fullscreen);
                } else {
                    pending.states.unset(State::Fullscreen);
                }
                if focus == Some(client.id) {
                    pending.states.set(State::Activated);
                } else {
                    pending.states.unset(State::Activated);
                }
            });
            if surface.is_initial_configure_sent() {
                surface.send_pending_configure();
            }
            let output = if window.down { &self.outputs[1] } else { &self.outputs[0] };
            for candidate in &self.outputs {
                if candidate == output {
                    candidate.enter(surface.wl_surface());
                } else {
                    candidate.leave(surface.wl_surface());
                }
            }
        }
        let surface =
            focus.and_then(|id| self.windows.iter().find(|w| w.id == id && w.mapped)).and_then(|w| w.window.toplevel()).map(|t| t.wl_surface().clone());
        if let Some(keyboard) = self.seat.get_keyboard()
            && keyboard.current_focus() != surface
        {
            self.core.handle(clamshell_state::Event::TextInput(false));
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    pub fn turn(&mut self) {
        self.update_idle(Instant::now());
        self.core.run(Instant::now());
        while let Some(effect) = self.core.next_effect() {
            match effect {
                Effect::Quit => self.exit = true,
                Effect::Window(WindowCommand::Close(ref ids)) => {
                    for client in self.windows.iter().filter(|w| ids.contains(&w.id)) {
                        if let Some(surface) = client.window.toplevel() {
                            surface.send_close();
                        }
                    }
                }
                Effect::Window(_) | Effect::Outputs { .. } | Effect::Refresh(_) => {}
                Effect::Key(ref stroke) => self.stroke(stroke),
                Effect::Launch { ref env, .. } => {
                    let mut launch = effect.clone();
                    if let Effect::Launch { env: target, .. } = &mut launch {
                        *target =
                            env.iter().filter(|(k, _)| k != "WAYLAND_DISPLAY").cloned().chain([("WAYLAND_DISPLAY".into(), self.socket.clone())]).collect();
                    }
                    (self.on_effect)(&launch);
                }
                _ => (self.on_effect)(&effect),
            }
        }
        self.sync_windows();
        self.popups.cleanup();
    }

    pub fn visible(&self, id: i64) -> bool {
        let session = self.core.state().session();
        session.top_window().is_some_and(|w| w.id == id)
            || (self.core.state().bottom().shows_windows(session) && session.bottom_window().is_some_and(|w| w.id == id))
    }

    pub fn geometry(&self, id: i64) -> Option<crate::geometry::Geometry> {
        let window = self.core.state().session().window(id)?;
        let client = self.windows.iter().find(|w| w.id == id && w.mapped)?;
        crate::geometry::Geometry::new(self.area(window.down), client.window.geometry(), client.presentation?)
    }
}

#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

pub(crate) fn insert_client(dh: &mut DisplayHandle, stream: std::os::unix::net::UnixStream) {
    if let Err(error) = dh.insert_client(stream, Arc::new(ClientState::default())) {
        log::warn!("compositor: accept client: {error}");
    }
}
