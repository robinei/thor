//! What every host shares: the Wayland display and its socket, the state's event channel, Xwayland, and
//! the loop that runs them. A host adds its outputs and input to the loop between [`Base::new`] and
//! [`Base::run`].

use std::os::fd::AsFd;
use std::time::Instant;

use clamshell_state::{Effect, Event, State};
use clamshell_ui::{Config, Error, EventSender};
use smithay::{
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, channel, generic::Generic, ping},
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::server::{Process, Server, insert_client};

pub(crate) struct Base {
    pub event_loop: EventLoop<'static, Server>,
    pub server: Server,
    sender: EventSender,
}

impl Base {
    /// The display, its socket and the state's channel, with the server over `state`. `relative_motion`:
    /// whether the host gives relative pointer motion, which the relative-pointer protocols need.
    pub fn new(
        state: State,
        config: Config,
        relative_motion: bool,
        identify: impl Fn(u32) -> Process + 'static,
        on_effect: impl FnMut(&Effect) + 'static,
    ) -> Result<Base, Error> {
        let event_loop: EventLoop<Server> = EventLoop::try_new()?;
        let mut display = Display::<Server>::new()?;
        let (wake, woken) = ping::make_ping()?;
        event_loop.handle().insert_source(woken, |_, _, _| {}).map_err(|e| e.error)?;
        let mut server = Server::new(state, config, display.handle(), relative_motion, move || wake.ping(), identify, on_effect)?;
        let socket = ListeningSocketSource::new_auto()?;
        server.socket = socket.socket_name().to_string_lossy().into_owned();
        log::info!("compositor: WAYLAND_DISPLAY={}", server.socket);
        event_loop.handle().insert_source(socket, |stream, _, server| insert_client(&mut server.dh, stream)).map_err(|e| e.error)?;
        // The duplicated descriptor is only a readiness source. The closure owns the display, so dispatch
        // does not need an unsafe mutable borrow of calloop's Generic storage.
        let fd = display.as_fd().try_clone_to_owned()?;
        event_loop
            .handle()
            .insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, server| {
                display.dispatch_clients(server)?;
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.error)?;
        let (sender, receiver) = EventSender::channel();
        event_loop
            .handle()
            .insert_source(receiver, |event, _, server| {
                if let channel::Event::Msg(event) = event {
                    server.core.handle(event);
                }
            })
            .map_err(|e| e.error)?;
        Ok(Base { event_loop, server, sender })
    }

    /// Starts Xwayland's socket, the system threads (`start`, with the socket's path) and the state, then
    /// runs the loop until the server exits. After each turn, `host` does its part (drawing what changed)
    /// and says when it next needs a turn.
    pub fn run(self, start: impl FnOnce(EventSender, std::path::PathBuf), mut host: impl FnMut(&mut Server, Instant) -> Option<Instant>) -> Result<(), Error> {
        let Base { mut event_loop, mut server, sender } = self;
        let mut x11 = crate::x11::Supervisor::new(event_loop.handle())?;
        server.x11_display = Some(x11.display());
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("compositor: XDG_RUNTIME_DIR is missing")?;
        start(sender, std::path::PathBuf::from(runtime).join(&server.socket));
        server.core.handle(Event::Start);
        let mut host_deadline = None;
        while !server.exit {
            let now = Instant::now();
            x11.turn(&mut server, now);
            let wait = server
                .core
                .deadline()
                .into_iter()
                .chain(server.cursor_activity.deadline(now))
                .chain(x11.deadline())
                .chain(host_deadline)
                .min()
                .map(|at| at.saturating_duration_since(now));
            event_loop.dispatch(wait, &mut server)?;
            server.turn();
            host_deadline = host(&mut server, Instant::now());
            server.dh.flush_clients()?;
        }
        Ok(())
    }
}
