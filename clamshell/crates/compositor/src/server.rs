//! Protocol objects and the bridge from client lifecycles into ClamShell's state.

use std::sync::Arc;
use std::time::Instant;

use clamshell_state::{Effect, State, WindowEvent, session::windows::Window};
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
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size, Transform},
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
    /// The Steam game it belongs to, if Steam started it (see [`Server::tell_game_mode`]).
    pub steam_app: Option<u32>,
}

pub(crate) struct ClientWindow {
    pub id: i64,
    pub window: crate::surface::Surface,
    pub opened: bool,
    pub mapped: bool,
    pub title_dirty: bool,
    pub presentation: Option<clamshell_state::session::presentation::WindowPresentation>,
    pub frames: crate::frames::Frames,
}

pub(crate) struct Server {
    pub core: Core,
    pub colour: crate::colour::State,
    pub dh: DisplayHandle,
    /// The event loop's handle, for a commit's buffer to wake it when ready (see `CompositorHandler::new_surface`).
    pub loop_handle: Option<smithay::reexports::calloop::LoopHandle<'static, Server>>,
    /// Explicit sync (`linux-drm-syncobj-v1`), on a GPU that can wait on a timeline point by an eventfd.
    pub syncobj: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
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
    /// When the frame rate of the window in front is next sent, while there is one to send, and what was
    /// last sent.
    pub frames_due: Option<Instant>,
    frames_sent: (Option<f32>, Option<f32>),
    /// The top output's instances clients bound, each told its game's mode (see [`Server::tell_game_mode`]).
    pub(crate) top_bound: Vec<smithay::reexports::wayland_server::Weak<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>>,
    /// The screens last told to the state (see [`Server::report_screens`]).
    screens_sent: Option<clamshell_state::session::presentation::Screens>,
    pub pointer_window: Option<i64>,
    pub pointer_buttons: Vec<u32>,
    pub arrow_pressed: bool,
    pub keyboard_home: bool,
    /// A VT asked for with Ctrl+Alt+F1..F12, for the host whose seat can switch.
    pub vt_switch: Option<i32>,
    /// Commits so far, for a host to tell when clients want frame callbacks without a visible change.
    pub commits: u64,
    /// Whether the state wants the top screen and the second on (`Effect::Outputs`), and the top
    /// screen's refresh in Hz, 0 for the rate it had at start (`Effect::Refresh`), for a host with screens.
    pub screens_on: [bool; 2],
    pub refresh_wanted: u16,
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
    /// Xwayland's surfaces by the serial it gave them, until their window is paired (see `x11::shell`).
    pub xwayland_serials: Vec<(u64, WlSurface)>,
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
        // The clipboard's contents only for ClamShell's own program: its clipboard thread, and
        // `clamshell --clipboard` for Waydroid's sharing; any other client could read what is copied.
        let data_control = smithay::wayland::selection::ext_data_control::DataControlState::new::<Self, _>(&dh, Some(&primary_selection), |client| {
            client.get_data::<ClientState>().is_some_and(|state| state.ours)
        });
        let outputs = ["ClamShell-Top", "ClamShell-Bottom"].map(|name| {
            Output::new(
                name.into(),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "ClamShell".into(),
                    model: "Nested".into(),
                    serial_number: name.into(),
                },
            )
        });
        // A game sees only the top screen: Wine lays out every output it is shown as a monitor, and a game
        // may open on the second. A dual-screen emulator sees both (see `ClientState::top_only`).
        outputs[0].create_global::<Self>(&dh);
        outputs[1].create_global_with_filter::<Self, _>(&dh, |client| !ClientState::top_only(client));
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let trace = std::rc::Rc::new(std::cell::RefCell::new(crate::trace::Ring::new()));
        let observed = trace.clone();
        let mut core = Core::new(state, config.touch_layout, wake)?;
        core.observe_events(move |event, state| observed.borrow_mut().record(event, state));
        let clock = smithay::utils::Clock::<smithay::utils::Monotonic>::new();
        smithay::wayland::presentation::PresentationState::new::<Self>(&dh, clock.id() as u32);
        // FIFO, by which Mesa's Vulkan paces a game in FIFO mode instead of with a frame callback a
        // swapchain's teardown can destroy a second time from another thread as it is answered (which
        // under Wine takes a game's window when it switches mode). Its barriers are signalled with the
        // frame callbacks (see `Server::frame_callbacks`).
        smithay::wayland::fifo::FifoManagerState::new::<Self>(&dh);
        // Commit timing: a commit to be shown no sooner than a time, held until the frame shown then is
        // composed (see `Server::release_timed_commits`). Mesa's Vulkan times FIFO frames by it.
        smithay::wayland::commit_timing::CommitTimingManagerState::new::<Self>(&dh);
        crate::x11::shell::install(&dh);
        // The screens' scale, 1, said as the fraction clients may size their buffers by (mpv's software
        // output sets a zero viewport size, a protocol error, when not told it).
        smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Self>(&dh);
        // Dialogs marked as such (GTK 4, Qt 6, SDL 3), whatever their parent: their role (see `Server::facts`).
        smithay::wayland::shell::xdg::dialog::XdgDialogState::new::<Self>(&dh);
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
            loop_handle: None,
            syncobj: None,
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
            frames_due: None,
            frames_sent: (None, None),
            screens_sent: None,
            top_bound: Vec::new(),
            pointer_window: None,
            pointer_buttons: Vec::new(),
            arrow_pressed: false,
            keyboard_home: false,
            vt_switch: None,
            commits: 0,
            screens_on: [true; 2],
            refresh_wanted: 0,
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
            xwayland_serials: Vec::new(),
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
            // One mode, the screen's: Smithay keeps every mode an output had, and Wine would offer them.
            for old in output.modes().into_iter().filter(|old| *old != mode) {
                output.delete_mode(old);
            }
            self.core.resize(screen, (w as u32, self.size.1 as u32), 1);
            self.core.record(screen, true);
        }
        // Games are told their own modes again, the screen's having been sent to everyone.
        self.top_bound.retain(|output| output.upgrade().is_ok());
        for output in self.top_bound.clone() {
            if let Ok(output) = output.upgrade() {
                self.tell_game_mode(&output);
            }
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
            frames: Default::default(),
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
        let (app_id, title, parent, dialog) = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| {
                    let dialog = data.dialog_hint != smithay::wayland::shell::xdg::dialog::ToplevelDialogHint::Unknown;
                    (data.app_id.clone().unwrap_or_default(), data.title.clone().unwrap_or_default(), data.parent.clone(), dialog)
                })
                .unwrap_or_default()
        });
        let process = self
            .core
            .state()
            .session()
            .window(id)
            .map(|window| Process { unit: window.unit.clone(), pids: window.pids.clone(), steam_app: None })
            .unwrap_or_else(|| {
                surface
                    .wl_surface()
                    .client()
                    .and_then(|c| c.get_credentials(&self.dh).inspect_err(|e| log::warn!("compositor: window {id}: its client's process unknown: {e}")).ok())
                    .and_then(|c| u32::try_from(c.pid).ok())
                    .map(|pid| (self.identify)(pid))
                    .unwrap_or_default()
            });
        let limits = Self::size_limits(surface);
        Window {
            id,
            app_id,
            title,
            colour: self.windows.iter().find(|w| w.id == id).and_then(|client| self.window_colour(&client.window)),
            parent: parent.and_then(|p| self.surface_id(&p)),
            unit: process.unit,
            pids: process.pids,
            limits,
            role: if dialog { clamshell_state::session::presentation::WindowRole::Dialog } else { clamshell_state::session::presentation::WindowRole::Normal },
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

    /// A window's colour, as the state has it: the description of the surface holding its picture (see
    /// `Surface::picture`: Wine declares a game's on the subsurface it draws in), else its toplevel's.
    pub(crate) fn window_colour(&self, window: &crate::surface::Surface) -> Option<std::sync::Arc<clamshell_state::session::presentation::ColourDescription>> {
        use smithay::backend::renderer::element::Id;
        let picture = window.picture_surface().map(|surface| Id::from_wayland_resource(&surface));
        let root = window.root().map(|surface| Id::from_wayland_resource(&surface));
        picture.and_then(|id| self.colour.surfaces.get(&id)).or_else(|| root.and_then(|id| self.colour.surfaces.get(&id))).cloned()
    }

    pub fn open_on_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.windows.iter().position(|w| w.window.root().as_ref() == Some(surface)) else { return };
        let Some(client) = self.windows.get(index) else { return };
        let Some(toplevel) = client.window.toplevel().cloned() else { return };
        let id = client.id;
        if !client.opened {
            let mut facts = self.facts(&toplevel, id);
            let colour = facts.colour.as_ref().map_or("unspecified".to_string(), |c| format!("{:?} {:?}", c.primaries, c.transfer));
            facts.focused = self.core.state().session().opening_focus(&facts);
            log::info!("compositor: window {id} opens: {:?} {:?}, focused {}, colour {colour}", facts.app_id, facts.title, facts.focused);
            self.core.handle(WindowEvent::Opened(facts).into());
            if let Some(client) = self.windows.get_mut(index) {
                client.opened = true;
            }
        } else {
            let limits = Self::size_limits(&toplevel);
            let colour = self.windows.get(index).and_then(|client| self.window_colour(&client.window));
            let old = self.core.state().session().window(id).map(|old| (old.limits, old.colour.clone()));
            if let Some((_, old_colour)) = &old
                && *old_colour != colour
            {
                let shown = colour.as_ref().map(|c| format!("{:?} {:?}", c.primaries, c.transfer));
                log::info!("compositor: window {id}'s colour is now {}", shown.as_deref().unwrap_or("unspecified"));
            }
            if old.is_some_and(|(old_limits, old_colour)| old_limits != limits || old_colour != colour) {
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
                let fill = self.fill_size(client.id, area.size);
                let size = crate::geometry::configure_size(window, policy, area.size, fill, client.window.geometry().size);
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
        self.report_frames(Instant::now());
        self.report_screens();
        self.core.run(Instant::now());
        while let Some(effect) = self.core.next_effect() {
            match effect {
                Effect::DumpWindowEvents(ref path) => crate::trace::save(self.trace.borrow().snapshot(self.core.state()), path.clone()),
                Effect::Screenshot => self.screenshot_requested = true,
                Effect::Quit => self.exit = true,
                Effect::Close(ref ids) => {
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
                // The second screen is on only while the screens are.
                Effect::Outputs { top, bottom } => self.screens_on = [top, top && bottom],
                Effect::Refresh(hz) => self.refresh_wanted = hz,
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

    /// What a fullscreen window `id` fills in an area of `area`: the area, or the resolution its game is set
    /// to (a fraction of it, `docs/picture.md`), which the window is then scaled up from.
    pub fn fill_size(&self, id: i64, area: Size<i32, Logical>) -> Size<i32, Logical> {
        let resolution = self.core.state().window_clarity(id).resolution;
        let (w, h) = resolution.size_on((area.w.max(0) as u32, area.h.max(0) as u32));
        (w as i32, h as i32).into()
    }

    /// Takes in a client that connected, knowing it by its process (see `running_of_process`): a game's is
    /// shown only the top screen, at its resolution as it is now, so a change applies the next time the
    /// game starts.
    pub(crate) fn admit(&mut self, stream: std::os::unix::net::UnixStream) {
        let Some(client) = insert_client(&mut self.dh, stream) else { return };
        let process = client
            .get_credentials(&self.dh)
            .inspect_err(|e| log::warn!("compositor: a client's process unknown, so it is taken as no game's: {e}"))
            .ok()
            .and_then(|c| u32::try_from(c.pid).ok())
            .map(|pid| (self.identify)(pid));
        let game = process.as_ref().and_then(|process| self.core.state().process_game(process.unit.as_deref(), process.steam_app));
        if let Some(state) = client.get_data::<ClientState>() {
            let _ = state.game.set(game);
        }
        if let (Some(process), Some((resolution, _))) = (process, game) {
            let told = clamshell_state::Reading::Told { unit: process.unit, steam_app: process.steam_app, resolution };
            self.core.handle(clamshell_state::Event::Reading(told));
        }
    }

    /// Tells a game's client that bound the top output the mode its resolution gives the screen, after the
    /// screen's own that Smithay sent, so a game choosing from the display modes (Wine's) finds it as the
    /// largest. The screen's
    /// mode itself never changes for a game: Wine lays its monitors out by their modes, and the second
    /// screen's monitor would move under the game's window.
    pub(crate) fn tell_game_mode(&mut self, output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput) {
        use smithay::reexports::wayland_server::{Resource, protocol::wl_output::Mode as WMode};
        let Some((resolution, _)) = output.client().and_then(|client| client.get_data::<ClientState>()?.game.get().copied().flatten()) else { return };
        let screen = (self.top_width.max(0) as u32, self.size.1.max(0) as u32);
        let (w, h) = resolution.size_on(screen);
        if (w, h) != screen && w > 0 && h > 0 {
            output.mode(WMode::Current, w as i32, h as i32, self.refresh);
            if output.version() >= 2 {
                output.done();
            }
        }
    }

    /// Each screen's colour (the top's, the second's), described to clients (and told to the state, see
    /// [`Server::report_screens`]).
    pub fn describe_screens(&mut self, descriptions: [clamshell_state::session::presentation::ColourDescription; 2]) {
        self.colour.set_outputs(descriptions);
    }

    /// Tells the state the screens' colours and the top screen's size when they change: a curve is shown
    /// within the colour, and a game's resolution is a fraction of the size.
    /// Gives the state the histogram the colour pipeline measured (see [`crate::colour::histogram`]).
    pub(crate) fn report_histogram(&mut self, histogram: clamshell_state::session::presentation::Histogram) {
        self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Histogram(Box::new(histogram))));
    }

    fn report_screens(&mut self) {
        let screens = clamshell_state::session::presentation::Screens {
            colour: self.colour.descriptions,
            top_size: (self.top_width.max(0) as u32, self.size.1.max(0) as u32),
        };
        if self.screens_sent != Some(screens) {
            self.screens_sent = Some(screens);
            self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Screens(Box::new(screens))));
        }
    }

    /// Frame callbacks for the windows shown on the top screen or the second (`down`; None: both, as
    /// nested), a capped window's at its cap's pace (see [`crate::frames::Frames::pace`]) on a screen
    /// showing a frame every `refresh`, and with them their surfaces' FIFO barriers, whose held commits
    /// then go ahead. Returns when a callback held back is due.
    pub fn frame_callbacks(&mut self, down: Option<bool>, refresh: std::time::Duration, now: Instant) -> Option<Instant> {
        let shown: Vec<_> = self.windows.iter().map(|client| client.mapped && self.visible(client.id)).collect();
        let state = self.core.state();
        let [top, bottom] = &self.outputs;
        let time = self.started.elapsed();
        let mut held: Option<Instant> = None;
        let mut barriers: Vec<smithay::reexports::wayland_server::Client> = Vec::new();
        for (client, shown) in self.windows.iter_mut().zip(shown) {
            let on_down = state.session().window(client.id).is_some_and(|w| w.down);
            if !shown || down.is_some_and(|down| down != on_down) {
                continue;
            }
            // A capped window uses its turn only while it waits for a callback or on a FIFO barrier, so
            // that one drawing slower than its cap isn't held back further.
            let cap = state.window_fps_cap(client.id);
            let waiting = client.window.root().is_some_and(|root| {
                with_states(&root, |states| !states.cached_state.get::<smithay::wayland::compositor::SurfaceAttributes>().current().frame_callbacks.is_empty())
            });
            let mut barrier = false;
            client.window.with_surfaces(|_, states| {
                barrier |= states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.is_some();
            });
            let waiting = waiting || barrier;
            let paced = if cap.is_some() && !waiting { Ok(()) } else { client.frames.pace(cap, refresh, now) };
            match paced {
                Ok(()) => {
                    let output = if on_down { bottom } else { top };
                    client.window.send_frame(output, time, Some(std::time::Duration::ZERO), |_, _| Some(output.clone()));
                    client.window.with_surfaces(|surface, states| {
                        // Only once the content that set it has been shown: let go earlier, the next
                        // would replace it unseen.
                        if !crate::presentation::content_shown(states) {
                            return;
                        }
                        let barrier = states.cached_state.get::<smithay::wayland::fifo::FifoBarrierCachedState>().current().barrier.take();
                        if let Some(barrier) = barrier {
                            barrier.signal();
                            if let Some(owner) = surface.client().filter(|owner| !barriers.iter().any(|c| c.id() == owner.id())) {
                                barriers.push(owner);
                            }
                        }
                    });
                }
                Err(due) => held = Some(held.map_or(due, |held| held.min(due))),
            }
        }
        let dh = self.dh.clone();
        for owner in barriers {
            use smithay::wayland::compositor::CompositorHandler;
            self.client_compositor_state(&owner).blocker_cleared(self, &dh);
        }
        held
    }

    /// Lets the windows' commits timed (`wp_commit_timing`) for no later than `shown`, when the frame about
    /// to be composed is shown, go ahead, and a capped window's whatever their time: its cap paces it, and
    /// Mesa's timing, which takes a frame shown later than it asked as one missed, would move its times on
    /// further each frame the cap holds back. Returns the earliest time a commit is still held for.
    pub fn release_timed_commits(&mut self, shown: smithay::utils::Time<smithay::utils::Monotonic>) -> Option<smithay::utils::Time<smithay::utils::Monotonic>> {
        use smithay::wayland::commit_timing::{CommitTimerBarrierStateUserData, Timestamp};
        let mut released: Vec<smithay::reexports::wayland_server::Client> = Vec::new();
        let mut held: Option<Timestamp> = None;
        let state = self.core.state();
        for client in &self.windows {
            let until = if state.window_fps_cap(client.id).is_some() {
                Timestamp::from(smithay::utils::Time::<smithay::utils::Monotonic>::from(std::time::Duration::from_secs(1 << 40)))
            } else {
                shown.into()
            };
            client.window.with_surfaces(|surface, states| {
                let Some(mut timers) = states.data_map.get::<CommitTimerBarrierStateUserData>().and_then(|timers| timers.lock().ok()) else { return };
                if timers.signal_until(until)
                    && let Some(owner) = surface.client().filter(|owner| !released.iter().any(|c| c.id() == owner.id()))
                {
                    released.push(owner);
                }
                if let Some(next) = timers.next_deadline() {
                    held = Some(held.map_or(next, |held| held.min(next)));
                }
            });
        }
        let dh = self.dh.clone();
        for owner in released {
            use smithay::wayland::compositor::CompositorHandler;
            self.client_compositor_state(&owner).blocker_cleared(self, &dh);
        }
        held.map(Into::into)
    }

    /// Sends the frame rate of the window on the top screen every [`crate::frames::REPORT`], while it
    /// draws: none for a window that drew nothing in the last second, so that the FPS tile measures no
    /// game rather than 0.
    fn report_frames(&mut self, now: Instant) {
        if self.frames_due.is_some_and(|due| due > now) {
            return;
        }
        let at = self.clock.now().into();
        let top = self.core.state().session().top_window().map(|w| w.id);
        let rate = self.windows.iter().find(|w| w.mapped && Some(w.id) == top).map(|w| w.frames.rate(at));
        let reading = rate.filter(|(fps, _)| *fps > 0.0).map_or((None, None), |(fps, ms)| (Some(fps), ms));
        if reading != self.frames_sent {
            self.frames_sent = reading;
            self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Frames { fps: reading.0, frame_ms: reading.1 }));
        }
        self.frames_due = reading.0.is_some().then(|| now + crate::frames::REPORT);
    }

    pub fn visible(&self, id: i64) -> bool {
        let session = self.core.state().session();
        session.top_window().is_some_and(|w| w.id == id)
            || (self.core.state().bottom().shows_windows(session) && session.bottom_window().is_some_and(|w| w.id == id))
    }

    pub fn geometry(&self, id: i64) -> Option<crate::geometry::Geometry> {
        let window = self.core.state().session().window(id)?;
        let client = self.windows.iter().find(|w| w.id == id && w.mapped)?;
        let (size, place) = client.window.picture().unzip();
        let picture = crate::geometry::Picture { size, place };
        crate::geometry::Geometry::new(self.area(window.down), client.window.geometry(), client.presentation?, picture)
    }
}

#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
    /// For a game's process, its resolution when it connected and whether it shows a window on the second
    /// screen (see [`Server::admit`]).
    pub game: std::sync::OnceLock<Option<(clamshell_state::settings::Resolution, bool)>>,
    /// Xwayland, the only client that sees `xwayland_shell_v1`.
    pub xwayland: bool,
    /// The client runs ClamShell's own program (see [`insert_client`]): only it sees `ext_data_control`.
    pub ours: bool,
}

impl ClientState {
    /// Whether the client sees only the top screen: a game's, unless it shows a window on the second screen
    /// as a dual-screen emulator does.
    fn top_only(client: &smithay::reexports::wayland_server::Client) -> bool {
        client.get_data::<ClientState>().and_then(|state| state.game.get().copied().flatten()).is_some_and(|(_, dual)| !dual)
    }
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

pub(crate) fn insert_client(dh: &mut DisplayHandle, stream: std::os::unix::net::UnixStream) -> Option<smithay::reexports::wayland_server::Client> {
    let state = ClientState { ours: runs_clamshell(&stream), ..ClientState::default() };
    dh.insert_client(stream, Arc::new(state)).inspect_err(|error| log::warn!("compositor: accept client: {error}")).ok()
}

/// Whether the process at the other end of `stream` runs the same executable as this one. Known as it
/// connects: a protocol's filter, asked with the display locked, can't ask the client's credentials.
fn runs_clamshell(stream: &std::os::unix::net::UnixStream) -> bool {
    let exe = |pid: &str| std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    match rustix::net::sockopt::socket_peercred(stream) {
        Ok(peer) => exe(&peer.pid.as_raw_nonzero().to_string()).is_some_and(|theirs| Some(theirs) == exe("self")),
        Err(e) => {
            log::warn!("compositor: a client's process unknown, so it isn't taken as ClamShell's: {e}");
            false
        }
    }
}
