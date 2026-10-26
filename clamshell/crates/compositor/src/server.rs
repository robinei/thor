//! The server: what it holds, and its turn after each dispatch, which carries out the compositor's own
//! effects. The windows are in `windows`, frame pacing in `pacing`, the clients in `clients`, the
//! protocols' states in `protocols`, input in `input` and the modules it leads to, and the X11 bridge in
//! `x11`.

use std::time::Instant;

use clamshell_state::{Compositor, Effect, State};
use clamshell_ui::{Config, Core, Error, Screen};
use smithay::{
    desktop::PopupManager,
    input::Seat,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{DisplayHandle, Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, Transform},
    wayland::{input_method::InputMethodSeat, output::OutputManagerState, text_input::TextInputSeat},
};

use crate::clients::{ClientState, Process};
use crate::windows::ClientWindow;

pub(crate) struct Server {
    pub core: Core,
    pub colour: crate::colour::State,
    pub dh: DisplayHandle,
    /// The event loop's handle, for a commit's buffer to wake it when ready (see `CompositorHandler::new_surface`).
    pub loop_handle: Option<smithay::reexports::calloop::LoopHandle<'static, Server>>,
    pub protocols: crate::protocols::Protocols,
    pub clock: smithay::utils::Clock<smithay::utils::Monotonic>,
    pub pending_imports: Vec<(smithay::backend::allocator::dmabuf::Dmabuf, smithay::wayland::dmabuf::ImportNotifier)>,
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
    pub pacing: crate::pacing::Pacing,
    /// The top output's instances clients bound, each told its game's mode (see [`Server::tell_game_mode`]).
    pub(crate) top_bound: Vec<smithay::reexports::wayland_server::Weak<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>>,
    /// The screens last told to the state (see [`Server::report_screens`]).
    screens_sent: Option<clamshell_state::session::presentation::Screens>,
    pub pointer_window: Option<i64>,
    pub pointer_buttons: Vec<u32>,
    /// The left button came down on an overlay's target, which acted at once; its release is not passed on.
    pub overlay_pressed: bool,
    pub keyboard_home: bool,
    /// A VT asked for with Ctrl+Alt+F1..F12, for the host whose seat can switch.
    pub vt_switch: Option<i32>,
    /// Whether the state wants the top screen and the second on (`Compositor::Outputs`), and the top
    /// screen's refresh in Hz, 0 for the rate it had at start (`Compositor::Refresh`), for a host with
    /// screens.
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
    pub(crate) identities: crate::clients::Identities,
    pub(crate) next_id: i64,
    pub x11: crate::x11::Bridge,
}

impl Server {
    pub fn new(
        state: State,
        config: Config,
        dh: DisplayHandle,
        relative_motion: bool,
        wake: impl Fn() + Send + Sync + 'static,
        identify: impl Fn(u32) -> Process + Send + Sync + 'static,
        on_effect: impl FnMut(&Effect) + 'static,
    ) -> Result<Self, Error> {
        let preview_widths = (config.top_size.0.max(1), config.bottom_size.0.max(1));
        let preview_size = (config.top_size.0.saturating_add(config.bottom_size.0).min(i32::MAX as u32) as i32, config.top_size.1.min(i32::MAX as u32) as i32);
        let clock = smithay::utils::Clock::<smithay::utils::Monotonic>::new();
        let mut protocols = crate::protocols::Protocols::new(&dh, clock.id() as u32);
        let mut seat = protocols.seats.new_wl_seat(&dh, "clamshell");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        seat.add_touch();
        seat.input_method();
        seat.text_input().set_compositor_input_method(true);
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
        let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
        let identities = crate::clients::Identities::new(identify, wake.clone());
        let mut core = Core::new(state, config.touch_layout, move || wake())?;
        core.observe_events(move |event, state| observed.borrow_mut().record(event, state));
        let mut server = Self {
            clock,
            core,
            colour: Default::default(),
            protocols,
            pending_imports: Vec::new(),
            seat,
            dh,
            loop_handle: None,
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
            pacing: Default::default(),
            screens_sent: None,
            top_bound: Vec::new(),
            pointer_window: None,
            pointer_buttons: Vec::new(),
            overlay_pressed: false,
            keyboard_home: false,
            vt_switch: None,
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
            identities,
            next_id: 1,
            x11: Default::default(),
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
        self.top_width = ((u64::from(self.size.0 as u32) * u64::from(top) / (u64::from(top) + u64::from(bottom)).max(1)) as i32).clamp(2, self.size.0 - 2);
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

    /// Flush client titles once for the frame, after all requests in the dispatch batch.
    pub fn frame(&mut self) {
        self.flush_x11_changes();
        self.flush_titles();
        self.turn();
    }

    pub fn turn(&mut self) {
        self.take_identified();
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
            let Effect::Compositor(own) = effect else {
                (self.on_effect)(&effect);
                continue;
            };
            match own {
                Compositor::DumpWindowEvents(path) => crate::trace::save(self.trace.borrow().snapshot(self.core.state()), path),
                Compositor::Screenshot => self.screenshot_requested = true,
                Compositor::Quit => self.exit = true,
                Compositor::Close(ids) => {
                    self.x11.close.extend(ids.iter().copied().filter(|id| {
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
                Compositor::Outputs { top, bottom } => self.screens_on = [top, top && bottom],
                Compositor::Refresh(hz) => self.refresh_wanted = hz,
                Compositor::Key(stroke) => self.stroke(&stroke),
            }
        }
        self.sync_windows();
        self.popups.cleanup();
    }

    /// Each screen's colour (the top's, the second's), described to clients (and told to the state, see
    /// [`Server::report_screens`]).
    pub fn describe_screens(&mut self, descriptions: [clamshell_state::colour::ColourDescription; 2]) {
        self.colour.set_outputs(descriptions);
    }

    /// Gives the state the histogram the colour pipeline measured (see [`crate::colour::histogram`]).
    pub(crate) fn report_histogram(&mut self, histogram: clamshell_state::colour::Histogram) {
        self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Histogram(Box::new(histogram))));
    }

    /// Tells the state the screens' colours and the top screen's size when they change: a curve is shown
    /// within the colour, and a game's resolution is a fraction of the size.
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
}
