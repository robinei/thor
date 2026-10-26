//! The server: what it holds, and its turn after each dispatch, which carries out the compositor's own
//! effects. The windows are in `windows`, the screens in `screens`, frame pacing in `pacing`, the clients
//! in `clients`, the protocols' states in `protocols`, input in `input` and the modules it leads to (the
//! pointer in `pointer`), and the X11 bridge in `x11`.

use std::time::Instant;

use clamshell_state::{Compositor, Effect, State};
use clamshell_ui::{Config, Core, Error};
use smithay::{
    desktop::PopupManager,
    input::Seat,
    reexports::wayland_server::{DisplayHandle, Resource, protocol::wl_surface::WlSurface},
    wayland::{input_method::InputMethodSeat, output::OutputManagerState, text_input::TextInputSeat},
};

use crate::clients::Process;
use crate::windows::ClientWindow;

/// The compositor: the UI's core it hosts, the clients' windows and the protocols' states, the screens,
/// input, and what a host reads after each turn.
pub(crate) struct Server {
    /// The UI and the state it owns.
    pub core: Core,
    /// The screens' colour and the colour protocols' objects.
    pub colour: crate::colour::State,
    pub dh: DisplayHandle,
    /// The event loop's handle, for a commit's buffer to wake it when ready (see `CompositorHandler::new_surface`).
    pub loop_handle: Option<smithay::reexports::calloop::LoopHandle<'static, Server>>,
    /// Smithay's states of the protocols served.
    pub protocols: crate::protocols::Protocols,
    /// The clock presentation feedback and commit timing are told in.
    pub clock: smithay::utils::Clock<smithay::utils::Monotonic>,
    /// Clients' dmabufs to import with the renderer before the next drawing (see `dmabuf`).
    pub pending_imports: Vec<(smithay::backend::allocator::dmabuf::Dmabuf, smithay::wayland::dmabuf::ImportNotifier)>,
    /// The one seat, with a keyboard, a pointer and touch.
    pub seat: Seat<Self>,
    pub popups: PopupManager,
    /// The xdg menu open, with its grab (see `popup`).
    pub menu: Option<crate::popup::Menu>,
    /// The clients' windows, in stacking order, the frontmost last.
    pub windows: Vec<ClientWindow>,
    pub screens: crate::screens::Screens,
    pub pointer: crate::pointer::Pointer,
    pub touch: crate::touch::Touches,
    /// Whether the desktop's Home key (F12) is down.
    pub keyboard_home: bool,
    /// A VT asked for with Ctrl+Alt+F1..F12, for the host whose seat can switch.
    pub vt_switch: Option<i32>,
    /// The source the on-screen keyboard's keys are sent from, apart from a host's keyboard.
    pub keyboard_source: smithay::input::keyboard::KeyboardSource,
    pub pacing: crate::pacing::Pacing,
    /// Whether the event loop ends: the state quit, or a failure left no way on.
    pub exit: bool,
    /// Whether the state asked for a screenshot, which the host takes with its next drawing.
    pub screenshot_requested: bool,
    /// When the server began, the time frame callbacks and cursor animations count from.
    pub started: Instant,
    pub idle: crate::idle::Idle,
    /// The state's revision the windows' `shown` was last worked out at (see [`Server::sync_windows`]).
    pub presentation_revision: Option<u64>,
    /// The last window events, dumped by `Compositor::DumpWindowEvents` (see `trace`).
    pub trace: std::rc::Rc<std::cell::RefCell<crate::trace::Ring>>,
    /// The Wayland socket's name (WAYLAND_DISPLAY).
    pub socket: String,
    /// Where the effects that aren't the compositor's own go.
    on_effect: Box<dyn FnMut(&Effect)>,
    /// The clients' processes, identified off this thread (see `clients`).
    pub identities: crate::clients::Identities,
    /// The id the next window gets, Wayland's and X11's alike.
    pub next_id: i64,
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
        let clock = smithay::utils::Clock::<smithay::utils::Monotonic>::new();
        let mut protocols = crate::protocols::Protocols::new(&dh, clock.id() as u32);
        let mut seat = protocols.seats.new_wl_seat(&dh, "clamshell");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        seat.add_touch();
        seat.input_method();
        seat.text_input().set_compositor_input_method(true);
        let screens = crate::screens::Screens::new(&dh, &config);
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let trace = std::rc::Rc::new(std::cell::RefCell::new(crate::trace::Ring::new()));
        let observed = trace.clone();
        let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
        let identities = crate::clients::Identities::new(identify, wake.clone());
        let mut core = Core::new(state, move || wake())?;
        core.observe_events(move |event, state| observed.borrow_mut().record(event, state));
        let mut server = Self {
            clock,
            core,
            screens,
            colour: Default::default(),
            protocols,
            pending_imports: Vec::new(),
            seat,
            dh,
            loop_handle: None,
            popups: PopupManager::default(),
            menu: None,
            windows: Vec::new(),
            pointer: crate::pointer::Pointer::new(),
            pacing: Default::default(),
            keyboard_home: false,
            vt_switch: None,
            keyboard_source: smithay::input::keyboard::KeyboardSource::new_auxiliary(),
            touch: crate::touch::Touches::new(),
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
        server.resize(server.screens.size);
        Ok(server)
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
                Compositor::Outputs { top, bottom } => self.screens.on = [top, top && bottom],
                Compositor::Refresh(hz) => self.screens.refresh_wanted = hz,
                Compositor::Key(stroke) => self.stroke(&stroke),
            }
        }
        self.sync_windows();
        self.popups.cleanup();
    }

    /// Gives the state the histogram the colour pipeline measured (see [`crate::colour::histogram`]).
    pub(crate) fn report_histogram(&mut self, histogram: clamshell_state::colour::Histogram) {
        self.core.handle(clamshell_state::Event::Reading(clamshell_state::Reading::Histogram(Box::new(histogram))));
    }
}
