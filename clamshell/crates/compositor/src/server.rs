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
    pub screens: crate::screens::Screens,
    pub pointer: crate::pointer::Pointer,
    pub pacing: crate::pacing::Pacing,
    pub keyboard_home: bool,
    /// A VT asked for with Ctrl+Alt+F1..F12, for the host whose seat can switch.
    pub vt_switch: Option<i32>,
    pub keyboard_source: smithay::input::keyboard::KeyboardSource,
    pub touches: [Option<crate::touch::Contact>; 32],
    pub touch_pending: bool,
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
        let mut core = Core::new(state, config.touch_layout, move || wake())?;
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
            touches: std::array::from_fn(|_| None),
            touch_pending: false,
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
