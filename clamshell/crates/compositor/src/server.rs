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
    pub window: crate::surface::Surface,
    pub opened: bool,
    pub mapped: bool,
    pub title_dirty: bool,
    pub presentation: Option<clamshell_state::session::presentation::WindowPresentation>,
}

pub(crate) struct Server {
    pub core: Core,
    pub colour: crate::colour::State,
    pub dh: DisplayHandle,
    pub compositor: CompositorState,
    pub shell: XdgShellState,
    pub activation: smithay::wayland::xdg_activation::XdgActivationState,
    pub shm: ShmState,
    pub clock: smithay::utils::Clock<smithay::utils::Monotonic>,
    pub dmabuf: smithay::wayland::dmabuf::DmabufState,
    pub pending_imports: Vec<(smithay::backend::allocator::dmabuf::Dmabuf, smithay::wayland::dmabuf::ImportNotifier)>,
    pub seats: SeatState<Self>,
    pub data_device: DataDeviceState,
    pub primary_selection: smithay::wayland::selection::primary_selection::PrimarySelectionState,
    pub data_control: smithay::wayland::selection::ext_data_control::DataControlState,
    pub seat: Seat<Self>,
    pub popups: PopupManager,
    pub menu: Option<crate::popup::Menu>,
    pub windows: Vec<ClientWindow>,
    pub outputs: [Output; 2],
    pub pointer: Point<f64, Logical>,
    pub relative_pointer: bool,
    pub capture: Option<crate::pointer::Capture>,
    pub host_capture: crate::pointer::CaptureMode,
    pub host_focused: bool,
    pub cursor: smithay::input::pointer::CursorImageStatus,
    pub cursor_activity: crate::cursor::Activity,
    pub pointer_window: Option<i64>,
    pub pointer_buttons: Vec<u32>,
    pub arrow_pressed: bool,
    pub keyboard_home: bool,
    pub keyboard_source: smithay::input::keyboard::KeyboardSource,
    pub touches: [Option<crate::touch::Contact>; 32],
    pub touch_pending: bool,
    pub size: (i32, i32),
    pub top_width: i32,
    preview_widths: (u32, u32),
    /// The outputs' refresh in mHz: the host monitor's when nested, so clients pacing to it (mpv's
    /// display-resample) match the frame callbacks they get.
    pub refresh: i32,
    pub exit: bool,
    pub screenshot_requested: bool,
    pub started: Instant,
    pub idle: crate::idle::Idle,
    pub presentation_revision: Option<u64>,
    pub trace: std::rc::Rc<std::cell::RefCell<crate::trace::Ring>>,
    pub socket: String,
    pub on_effect: Box<dyn FnMut(&Effect)>,
    pub(crate) identify: Box<dyn Fn(u32) -> Process>,
    pub(crate) next_id: i64,
    pub x11_selections: crate::x11::selection::SeatSelections,
    pub x11_close: Vec<i64>,
    pub x11_popup_dismiss: Vec<i64>,
    pub x11_changes: Vec<Window>,
    pub x11_display: Option<String>,
}

impl Server {
    pub fn new(
        state: State,
        config: Config,
        dh: DisplayHandle,
        relative_motion: bool,
        wake: impl Fn() + Send + Sync + 'static,
        identify: impl Fn(u32) -> Process + 'static,
        on_effect: impl FnMut(&Effect) + 'static,
    ) -> Result<Self, Error> {
        let preview_widths = (config.top_size.0.max(1), config.bottom_size.0.max(1));
        let preview_size = (config.top_size.0.saturating_add(config.bottom_size.0).min(i32::MAX as u32) as i32, config.top_size.1.min(i32::MAX as u32) as i32);
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "clamshell");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        seat.add_touch();
        seat.input_method();
        seat.text_input().set_compositor_input_method(true);
        TextInputManagerState::new::<Self>(&dh);
        smithay::wayland::cursor_shape::CursorShapeManagerState::new::<Self>(&dh);
        smithay::wayland::viewporter::ViewporterState::new::<Self>(&dh);
        smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(&dh);
        let primary_selection = smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<Self>(&dh);
        let data_control = smithay::wayland::selection::ext_data_control::DataControlState::new::<Self, _>(&dh, Some(&primary_selection), |_| true);
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
        let trace = std::rc::Rc::new(std::cell::RefCell::new(crate::trace::Ring::new()));
        let observed = trace.clone();
        let mut core = Core::new(state, config.touch_layout, wake)?;
        core.observe_events(move |event, state| observed.borrow_mut().record(event, state));
        let clock = smithay::utils::Clock::<smithay::utils::Monotonic>::new();
        smithay::wayland::presentation::PresentationState::new::<Self>(&dh, clock.id() as u32);
        let mut server = Self {
            clock,
            core,
            colour: Default::default(),
            compositor: CompositorState::new::<Self>(&dh),
            shell: XdgShellState::new::<Self>(&dh),
            activation: smithay::wayland::xdg_activation::XdgActivationState::new::<Self>(&dh),
            shm: ShmState::new::<Self>(&dh, vec![smithay::reexports::wayland_server::protocol::wl_shm::Format::Abgr8888]),
            dmabuf: smithay::wayland::dmabuf::DmabufState::new(),
            pending_imports: Vec::new(),
            data_device: DataDeviceState::new::<Self>(&dh),
            primary_selection,
            data_control,
            seats,
            seat,
            dh,
            outputs,
            popups: PopupManager::default(),
            menu: None,
            windows: Vec::new(),
            pointer: (0.0, 0.0).into(),
            relative_pointer: false,
            capture: None,
            host_capture: crate::pointer::CaptureMode::Free,
            host_focused: true,
            cursor: smithay::input::pointer::CursorImageStatus::default_named(),
            cursor_activity: crate::cursor::Activity::default(),
            pointer_window: None,
            pointer_buttons: Vec::new(),
            arrow_pressed: false,
            keyboard_home: false,
            keyboard_source: smithay::input::keyboard::KeyboardSource::new_auxiliary(),
            touches: std::array::from_fn(|_| None),
            touch_pending: false,
            size: preview_size,
            top_width: config.top_size.0.min(i32::MAX as u32) as i32,
            preview_widths,
            refresh: 60_000,
            exit: false,
            screenshot_requested: false,
            started: Instant::now(),
            idle: crate::idle::Idle::new(),
            presentation_revision: None,
            trace,
            socket: String::new(),
            on_effect: Box::new(on_effect),
            identify: Box::new(identify),
            next_id: 1,
            x11_selections: Default::default(),
            x11_close: Vec::new(),
            x11_popup_dismiss: Vec::new(),
            x11_changes: Vec::new(),
            x11_display: None,
        };
        if relative_motion {
            server.enable_relative_pointer();
        }
        server.resize(preview_size);
        Ok(server)
    }

    pub fn resize(&mut self, (width, height): (i32, i32)) {
        self.touch_cancel();
        self.size = (width.max(4), height.max(4));
        let (top, bottom) = self.preview_widths;
        self.top_width = ((u64::from(self.size.0 as u32) * u64::from(top) / (u64::from(top) + u64::from(bottom))) as i32).clamp(2, self.size.0 - 2);
        for (screen, output, x, w) in
            [(Screen::Top, &self.outputs[0], 0, self.top_width), (Screen::Bottom, &self.outputs[1], self.top_width, self.size.0 - self.top_width)]
        {
            let mode = Mode { size: (w, self.size.1).into(), refresh: self.refresh };
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
        self.windows.iter().find(|w| w.window.root().as_ref() == Some(surface)).map(|w| w.id)
    }

    pub fn add_toplevel(&mut self, surface: ToplevelSurface) {
        if self.surface_id(surface.wl_surface()).is_some() {
            self.reject_client(surface.wl_surface(), "wl_surface already has a toplevel");
            return;
        }
        let Some(next) = self.next_id.checked_add(1) else {
            log::error!("compositor: window ids exhausted");
            surface.send_close();
            return;
        };
        self.windows.push(ClientWindow {
            id: self.next_id,
            window: crate::surface::Surface::Wayland(SurfaceWindow::new_wayland_window(surface)),
            opened: false,
            mapped: false,
            title_dirty: false,
            presentation: None,
        });
        self.next_id = next;
    }

    pub(crate) fn reject_client(&self, surface: &WlSurface, message: &str) {
        log::warn!("compositor: rejecting client surface {:?}: {message}", surface.id());
        let Some(client) = surface.client() else { return };
        let backend = self.dh.backend_handle();
        let mut display = None;
        // Posting the error also flushes it. Merely marking a client dead outside a
        // protocol dispatch can leave its socket waiting for another incoming request.
        if backend
            .with_all_objects_for(client.id(), |object| {
                if object.protocol_id() == 1 {
                    display = Some(object);
                }
            })
            .is_ok()
            && let Some(display) = display
        {
            let message = std::ffi::CString::new(message).unwrap_or_else(|_| c"client request rejected".to_owned());
            backend.post_error(display, 3, message); // wl_display.implementation
        }
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
        Window {
            id,
            app_id,
            title,
            colour: crate::colour::protocol::current(surface.wl_surface()),
            parent: parent.and_then(|p| self.surface_id(&p)),
            unit: process.unit,
            pids: process.pids,
            limits,
            ..Window::default()
        }
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
        let Some(index) = self.windows.iter().position(|w| w.window.root().as_ref() == Some(surface)) else { return };
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
            if self.core.state().session().window(id).is_some_and(|old| old.limits != limits || old.colour != crate::colour::protocol::current(surface)) {
                self.core.handle(WindowEvent::Changed(self.facts(&toplevel, id)).into());
            }
        }
        self.sync_windows();
        if !toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
        }
    }

    pub fn sync_windows(&mut self) {
        self.sync_x11_popups();
        self.validate_capture();
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
            if let Some(surface) = client.window.toplevel() {
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
            }
            let Some(root) = client.window.root() else { continue };
            let output = if window.down { &self.outputs[1] } else { &self.outputs[0] };
            for candidate in &self.outputs {
                if candidate == output {
                    candidate.enter(&root);
                } else {
                    candidate.leave(&root);
                }
            }
        }
        let surface = focus.and_then(|id| self.windows.iter().find(|w| w.id == id && w.mapped)).and_then(|w| w.window.root());
        let surface = self.menu_focus(surface);
        if let Some(keyboard) = self.seat.get_keyboard()
            && keyboard.current_focus() != surface
        {
            self.core.handle(clamshell_state::Event::TextInput(false));
            if surface.is_none() {
                self.activation.retain_tokens(|_, _| false);
                smithay::wayland::selection::data_device::set_data_device_focus(&self.dh, &self.seat, None);
                smithay::wayland::selection::primary_selection::set_primary_focus(&self.dh, &self.seat, None);
            }
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// Flush client titles once for the frame, after all requests in the dispatch batch.
    pub fn frame(&mut self) {
        for mut facts in self.x11_changes.drain(..) {
            let Some(old) = self.core.state().session().window(facts.id) else { continue };
            let title_changed = old.title != facts.title;
            let changed =
                old.app_id != facts.app_id || old.parent != facts.parent || old.limits != facts.limits || old.role != facts.role || old.colour != facts.colour;
            if changed {
                // Scope adoption can arrive while the properties wait for this frame.
                facts.unit.clone_from(&old.unit);
                facts.pids.clone_from(&old.pids);
            }
            let title = title_changed.then(|| WindowEvent::Title { window: facts.id, title: std::mem::take(&mut facts.title) });
            match (changed, title) {
                (true, Some(title)) => self.core.handle(clamshell_state::Event::WindowBatch(vec![WindowEvent::Changed(facts), title])),
                (true, None) => self.core.handle(WindowEvent::Changed(facts).into()),
                (false, Some(title)) => self.core.handle(title.into()),
                (false, None) => {}
            }
        }
        for index in 0..self.windows.len() {
            let pending = self.windows.get_mut(index).and_then(|client| {
                std::mem::take(&mut client.title_dirty).then(|| client.window.toplevel().cloned().map(|surface| (client.id, surface))).flatten()
            });
            if let Some((id, surface)) = pending {
                let title = with_states(surface.wl_surface(), |states| {
                    states.data_map.get::<XdgToplevelSurfaceData>().and_then(|data| data.lock().ok()).and_then(|data| {
                        let title = data.title.as_deref().unwrap_or_default();
                        self.core.state().session().window(id).filter(|old| old.title != title).map(|_| title.to_owned())
                    })
                });
                if let Some(title) = title {
                    self.core.handle(WindowEvent::Title { window: id, title }.into());
                }
            }
        }
        self.turn();
    }

    pub fn turn(&mut self) {
        // A destructor event must follow the dispatch that installs the new object's data.
        // Sending it from get_information itself removes the object before wayland-backend installs it.
        for information in self.colour.information.drain(..) {
            information.done();
        }
        self.expire_activation_tokens(Instant::now());
        self.update_idle(Instant::now());
        self.core.run(Instant::now());
        while let Some(effect) = self.core.next_effect() {
            match effect {
                Effect::DumpWindowEvents(ref path) => crate::trace::save(self.trace.borrow().snapshot(self.core.state()), path.clone()),
                Effect::Screenshot => self.screenshot_requested = true,
                Effect::Quit => self.exit = true,
                Effect::Window(WindowCommand::Close(ref ids)) => {
                    self.x11_close.extend(ids.iter().copied().filter(|id| {
                        self.core.state().session().window(*id).is_some()
                            && self.windows.iter().all(|window| window.id != *id || window.window.toplevel().is_none())
                    }));
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
                    if let (Some(display), Effect::Launch { env, .. }) = (&self.x11_display, &mut launch) {
                        env.retain(|(name, _)| name != "DISPLAY");
                        env.push(("DISPLAY".into(), display.clone()));
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
    fn disconnected(&self, id: ClientId, reason: DisconnectReason) {
        match reason {
            DisconnectReason::ProtocolError(error) => log::warn!("compositor: client {id:?} disconnected for protocol error: {error:?}"),
            reason => log::debug!("compositor: client {id:?} disconnected: {reason:?}"),
        }
    }
}

pub(crate) fn insert_client(dh: &mut DisplayHandle, stream: std::os::unix::net::UnixStream) {
    if let Err(error) = dh.insert_client(stream, Arc::new(ClientState::default())) {
        log::warn!("compositor: accept client: {error}");
    }
}
