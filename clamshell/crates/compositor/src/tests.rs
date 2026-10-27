//! Real xdg-shell traffic through a socket pair, without a GPU or a host compositor.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use clamshell_state::Compositor;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use clamshell_state::{Event, HardwareKey, Init, State, session::windows::WindowPlace};
use clamshell_ui::Config;
use smithay::reexports::{
    wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
    wayland_server::Display,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_touch},
};

use crate::clients::{ClientState, Process};
use crate::server::Server;

#[derive(Default)]
pub(crate) struct Client {
    sizes: Vec<(i32, i32)>,
    presentation_clock: Option<u32>,
    presented: Vec<(u32, Duration)>,
    discarded: Vec<u32>,
    presentation_outputs: usize,
    ignore_configures: bool,
    touches: Vec<TouchRecord>,
    keys: Vec<(u32, bool)>,
    keyboard_enter: u32,
    activation_tokens: Vec<String>,
    text: Vec<keyboard_tests::TextRecord>,
    dmabuf_formats: Vec<(u32, u64)>,
    feedback_device: Vec<u8>,
    feedback_table: Vec<u8>,
    feedback_done: bool,
    imported_buffers: usize,
    pointer_serial: u32,
    cursor_enter: u32,
    popup_done: usize,
    relative: Vec<(f64, f64, f64, f64)>,
    constraints: Vec<&'static str>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(_: &mut Self, proxy: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(state: &mut Self, proxy: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event
            && !state.ignore_configures
        {
            proxy.ack_configure(serial);
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(state: &mut Self, _: &xdg_toplevel::XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.sizes.push((width, height));
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore wl_buffer::WlBuffer);

/// A test server, without a GPU or a host compositor: the preview's screens (960×540 and 620×540), a state
/// from nothing, every process of no unit, and effects dropped, unless set otherwise.
pub(crate) struct ServerBuilder {
    state: State,
    config: Config,
    relative_motion: bool,
    identify: Box<dyn Fn(u32) -> Process + Send + Sync>,
    on_effect: Box<dyn FnMut(&clamshell_state::Effect)>,
}

impl ServerBuilder {
    pub(crate) fn new() -> ServerBuilder {
        ServerBuilder {
            state: State::new(Init::default()),
            config: Config { top_size: (960, 540), bottom_size: (620, 540) },
            relative_motion: false,
            identify: Box::new(|_| Process::default()),
            on_effect: Box::new(|_| {}),
        }
    }

    pub(crate) fn state(self, state: State) -> ServerBuilder {
        ServerBuilder { state, ..self }
    }

    pub(crate) fn sizes(self, top_size: (u32, u32), bottom_size: (u32, u32)) -> ServerBuilder {
        ServerBuilder { config: Config { top_size, bottom_size }, ..self }
    }

    /// With relative pointer motion, as on a host that gives it.
    pub(crate) fn relative_motion(self) -> ServerBuilder {
        ServerBuilder { relative_motion: true, ..self }
    }

    /// With each process as `identify` says, by its pid (see [`crate::clients::Identities`]).
    pub(crate) fn identify(self, identify: impl Fn(u32) -> Process + Send + Sync + 'static) -> ServerBuilder {
        ServerBuilder { identify: Box::new(identify), ..self }
    }

    /// With the effects the compositor passes on given to `on_effect`.
    pub(crate) fn on_effect(self, on_effect: impl FnMut(&clamshell_state::Effect) + 'static) -> ServerBuilder {
        ServerBuilder { on_effect: Box::new(on_effect), ..self }
    }

    /// The server and its display, before the UI's Start.
    pub(crate) fn build(self) -> (Display<Server>, Server) {
        let display = Display::<Server>::new().unwrap();
        let server = Server::new(self.state, self.config, display.handle(), self.relative_motion, || {}, self.identify, self.on_effect).unwrap();
        (display, server)
    }

    /// The server after the UI's Start and a first frame, as most tests want it.
    pub(crate) fn started(self) -> (Display<Server>, Server) {
        let (display, mut server) = self.build();
        server.core.handle(Event::Start);
        server.frame();
        (display, server)
    }
}

/// A client's side of a test: it reports each step it has taken and waits until the server has checked it.
pub(crate) struct Steps<R> {
    report: mpsc::Sender<R>,
    next: mpsc::Receiver<()>,
}

impl<R> Steps<R> {
    /// Reports `step`, and waits until the server has checked it.
    pub(crate) fn step(&self, step: R) {
        self.report.send(step).map_err(|_| "the server is gone").unwrap();
        self.next.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}

/// A test client on a thread of its own (see [`spawn_client`]).
pub(crate) struct TestClient<R> {
    /// The server's handle of the client, to look up its objects.
    pub(crate) peer: smithay::reexports::wayland_server::Client,
    thread: std::thread::JoinHandle<()>,
    reports: mpsc::Receiver<R>,
    next: mpsc::Sender<()>,
}

/// Connects a client to `display`, with `state` (Xwayland's or an ordinary client's), and runs `run` on a
/// thread of its own with the globals, its event queue and its [`Steps`].
pub(crate) fn spawn_client<R: Send + 'static>(
    display: &mut Display<Server>,
    state: ClientState,
    run: impl FnOnce(wayland_client::globals::GlobalList, wayland_client::EventQueue<Client>, Steps<R>) + Send + 'static,
) -> TestClient<R> {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let peer = display.handle().insert_client(server_socket, Arc::new(state)).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, queue) = registry_queue_init::<Client>(&connection).unwrap();
        run(globals, queue, Steps { report, next: steps });
    });
    TestClient { peer, thread, reports, next }
}

impl<R> TestClient<R> {
    /// Serves the client until its thread ends, for at most 15 s. Each turn dispatches its requests, lets
    /// `turn` do the server's work between (a frame, for most tests), flushes the replies, and hands a step
    /// the client reported to `check`, after which the client goes on. Asserts that the client ended, and
    /// returns the steps checked.
    pub(crate) fn serve(
        self,
        display: &mut Display<Server>,
        server: &mut Server,
        mut turn: impl FnMut(&mut Server),
        mut check: impl FnMut(&mut Display<Server>, &mut Server, R),
    ) -> usize {
        let until = Instant::now() + Duration::from_secs(15);
        let mut checked = 0;
        while !self.thread.is_finished() && Instant::now() < until {
            display.dispatch_clients(server).unwrap();
            turn(server);
            display.flush_clients().unwrap();
            if let Ok(step) = self.reports.try_recv() {
                check(display, server, step);
                display.flush_clients().unwrap();
                checked += 1;
                // (A client that ends without waiting for the last check has gone.)
                let _ = self.next.send(());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(self.thread.is_finished(), "the client timed out after {checked} steps");
        self.thread.join().unwrap();
        checked
    }
}

/// A buffer of `size` in shared memory, all white.
pub(crate) fn shm_buffer(globals: &GlobalList, qh: &QueueHandle<Client>, (width, height): (i32, i32)) -> wl_buffer::WlBuffer {
    let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&vec![0xff; (width * height * 4) as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), width * height * 4, qh, ());
    pool.create_buffer(0, width, height, width * 4, wl_shm::Format::Abgr8888, qh, ())
}

/// A test client's toplevel, shown (see [`open_toplevel`]).
pub(crate) struct Toplevel {
    pub(crate) surface: wl_surface::WlSurface,
    pub(crate) xdg: xdg_surface::XdgSurface,
    pub(crate) top: xdg_toplevel::XdgToplevel,
    pub(crate) buffer: wl_buffer::WlBuffer,
    app_id: String,
}

/// Opens a toplevel with `app_id` and, once it is configured, shows a buffer of `size`, or of the size it
/// was configured to.
pub(crate) fn open_toplevel(globals: &GlobalList, queue: &mut EventQueue<Client>, client: &mut Client, app_id: &str, size: Option<(i32, i32)>) -> Toplevel {
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = shell.get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    top.set_app_id(app_id.into());
    surface.commit();
    queue.roundtrip(client).unwrap();
    let size = size.unwrap_or_else(|| *client.sizes.last().unwrap());
    let buffer = shm_buffer(globals, &qh, size);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, size.0, size.1);
    surface.commit();
    queue.roundtrip(client).unwrap();
    Toplevel { surface, xdg, top, buffer, app_id: app_id.into() }
}

impl Toplevel {
    /// Takes its buffer away, which unmaps it.
    pub(crate) fn unmap(&self, queue: &mut EventQueue<Client>, client: &mut Client) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        queue.roundtrip(client).unwrap();
    }

    /// Shows it again. Unmapping reset its xdg state, so its app id is set again and it is configured
    /// again before its buffer.
    pub(crate) fn remap(&self, queue: &mut EventQueue<Client>, client: &mut Client) {
        self.top.set_app_id(self.app_id.clone());
        self.surface.commit();
        queue.roundtrip(client).unwrap();
        self.surface.attach(Some(&self.buffer), 0, 0);
        self.surface.commit();
        queue.roundtrip(client).unwrap();
    }
}

/// A client of a launch's unit, `test.service`.
fn launched() -> ClientState {
    ClientState { process: Some(Process { unit: Some("test.service".into()), pids: vec![100], game: None }), ..ClientState::default() }
}

#[test]
fn an_xdg_window_is_placed_and_configured_before_its_first_buffer() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("test".into());
        top.set_title("before configure".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        steps.step(*client.sizes.last().unwrap());
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, size| {
        let windows = server.core.state().session().windows();
        assert_eq!(size, (960, 540));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].title, "before configure");
        assert_eq!(windows[0].unit.as_deref(), Some("test.service"));
        assert!(matches!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Top(_)));
        assert!(!server.windows[0].mapped, "initial configure precedes the first buffer");
        assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(windows[0].id));
    });
    assert_eq!(checked, 1);
}

#[test]
fn a_windows_presentation_is_worked_out_again_only_when_the_state_changes() {
    use clamshell_state::presentation::{WindowLayout, WindowRole};
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let _window = open_toplevel(&globals, &mut queue, &mut Client::default(), "test", None);
        steps.step(());
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ()| {
        assert!(server.windows[0].mapped);
        server.sync_windows();
        let revision = server.presentation_revision;
        for _ in 0..10 {
            server.sync_windows();
        }
        assert_eq!(server.presentation_revision, revision);
        let mut facts = server.core.state().session().windows()[0].facts.clone();
        facts.role = WindowRole::Utility;
        server.core.handle(clamshell_state::WindowEvent::Changed(facts.clone()).into());
        server.sync_windows();
        assert_ne!(server.presentation_revision, revision);
        assert_eq!(server.windows[0].shown.presentation.unwrap().layout, WindowLayout::Centred);
        facts.role = WindowRole::Normal;
        server.core.handle(clamshell_state::WindowEvent::Changed(facts).into());
        server.sync_windows();
        assert_eq!(server.windows[0].shown.presentation.unwrap().layout, WindowLayout::Fullscreen);
    });
    assert_eq!(checked, 1);
}

#[test]
fn a_window_moved_down_that_ignores_its_configure_is_scaled_at_once() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let mut client = Client::default();
        let _window = open_toplevel(&globals, &mut queue, &mut client, "test", None);
        client.ignore_configures = true;
        steps.step(None);
        queue.roundtrip(&mut client).unwrap();
        steps.step(Some(*client.sizes.last().unwrap()));
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |display, server, configured| {
        let Some(size) = configured else {
            server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
            server.frame();
            display.flush_clients().unwrap();
            return;
        };
        // The client keeps its original buffer and stops acknowledging configure. It
        // cannot hold up placement; rendering and input use a fitted transform at once.
        let id = server.core.state().session().windows()[0].id;
        let geometry = server.geometry(id).unwrap();
        assert!(geometry.scale.x < 1.0);
        assert_eq!(geometry.native.size, (960, 540).into());
        assert_eq!(server.core.state().session().place(id).unwrap(), WindowPlace::Bottom);
        assert_eq!(size, (server.area(true).size.w, server.area(true).size.h));
    });
    assert_eq!(checked, 2);
}

#[test]
fn a_burst_of_titles_reaches_the_state_once_per_frame() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let mut client = Client::default();
        let window = open_toplevel(&globals, &mut queue, &mut client, "test", None);
        steps.step(false);
        for number in 0..100 {
            window.top.set_title(format!("intermediate title {number}"));
        }
        window.top.set_title("after configure".into());
        queue.roundtrip(&mut client).unwrap();
        steps.step(true);
        for _ in 0..100 {
            window.top.set_title("after configure".into());
        }
        queue.roundtrip(&mut client).unwrap();
        steps.step(true);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, renamed| {
        if !renamed {
            server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
            server.frame();
            return;
        }
        let windows = server.core.state().session().windows();
        let recording = server.trace.borrow().snapshot(server.core.state());
        let titles =
            recording.records.iter().filter(|record| matches!(&record.input, crate::trace::Input::Window(clamshell_state::WindowEvent::Title { .. }))).count();
        assert_eq!(titles, 1, "a title burst enters state and diagnostics once per frame");
        assert_eq!(windows[0].title, "after configure");
        assert_eq!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Bottom, "a title doesn't move the window");
    });
    assert_eq!(checked, 3);
}

/// What the client of [`an_unmapped_window_leaves_the_state_and_comes_back_on_top`] did.
#[derive(Debug)]
enum Mapping {
    Mapped,
    Unmapped,
    Remapped,
}

#[test]
fn an_unmapped_window_leaves_the_state_and_comes_back_on_top() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let mut client = Client::default();
        let window = open_toplevel(&globals, &mut queue, &mut client, "test", None);
        steps.step(Mapping::Mapped);
        window.unmap(&mut queue, &mut client);
        steps.step(Mapping::Unmapped);
        window.top.set_title("after remapping".into());
        window.remap(&mut queue, &mut client);
        steps.step(Mapping::Remapped);
    });
    let mut id = None;
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, step| {
        let windows = server.core.state().session().windows();
        match step {
            Mapping::Mapped => {
                id = Some(windows[0].id);
                server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
                server.frame();
            }
            Mapping::Unmapped => {
                assert!(windows.is_empty());
                assert!(!server.windows[0].mapped);
            }
            Mapping::Remapped => {
                assert_eq!(windows.len(), 1);
                assert_eq!(Some(windows[0].id), id);
                assert_eq!(windows[0].id, server.windows[0].id);
                assert!(server.windows[0].mapped);
                assert_eq!(windows[0].title, "after remapping");
                assert!(
                    matches!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Top(_)),
                    "this unowned client has no persistent bottom rule"
                );
            }
        }
    });
    assert_eq!(checked, 3);
}

#[test]
fn a_dialog_of_a_window_below_shows_over_it_there_at_its_fixed_size() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let qh = queue.handle();
        let mut client = Client::default();
        let window = open_toplevel(&globals, &mut queue, &mut client, "test", None);
        steps.step(None);
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let child_surface = compositor.create_surface(&qh, ());
        let child_xdg = shell.get_xdg_surface(&child_surface, &qh, ());
        let child = child_xdg.get_toplevel(&qh, ());
        child.set_parent(Some(&window.top));
        child.set_app_id("dialog".into());
        child.set_min_size(320, 240);
        child.set_max_size(320, 240);
        child_surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(Some(*client.sizes.last().unwrap()));
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, configured| {
        let Some(size) = configured else {
            server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
            server.frame();
            return;
        };
        let windows = server.core.state().session().windows();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1].parent, Some(windows[0].id));
        // A dialog of the window below shows there over it.
        let (parent, dialog) = (windows[0].id, windows[1].id);
        assert_eq!(windows[1].limits.fixed_size(), Some((320, 240)));
        assert!(matches!(server.core.state().session().place(dialog).unwrap(), WindowPlace::Bottom));
        assert_eq!(server.core.state().session().stack(true).map(|w| w.id).collect::<Vec<_>>(), [parent, dialog]);
        assert_eq!(size, (320, 240));
    });
    assert_eq!(checked, 2);
}

#[test]
fn a_destroyed_window_leaves_the_state_and_the_server() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, launched(), |globals, mut queue, steps| {
        let mut client = Client::default();
        let window = open_toplevel(&globals, &mut queue, &mut client, "test", None);
        steps.step(false);
        window.top.destroy();
        window.xdg.destroy();
        window.surface.destroy();
        window.buffer.destroy();
        queue.roundtrip(&mut client).unwrap();
        steps.step(true);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, destroyed| {
        assert_eq!(server.core.state().session().windows().is_empty(), destroyed);
        assert_eq!(server.windows.is_empty(), destroyed);
    });
    assert_eq!(checked, 2);
}

#[derive(Debug, PartialEq)]
enum TouchRecord {
    Down(i32, u32, (f64, f64)),
    Motion(i32, (f64, f64)),
    Up(i32),
    Frame,
    Cancel,
}
impl Dispatch<wl_touch::WlTouch, ()> for Client {
    fn event(state: &mut Self, _: &wl_touch::WlTouch, event: wl_touch::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let record = match event {
            wl_touch::Event::Down { id, surface, x, y, .. } => TouchRecord::Down(id, surface.id().protocol_id(), (x, y)),
            wl_touch::Event::Motion { id, x, y, .. } => TouchRecord::Motion(id, (x, y)),
            wl_touch::Event::Up { id, .. } => TouchRecord::Up(id),
            wl_touch::Event::Frame => TouchRecord::Frame,
            wl_touch::Event::Cancel => TouchRecord::Cancel,
            _ => return,
        };
        state.touches.push(record);
    }
}
delegate_noop!(Client: ignore wl_seat::WlSeat);

#[test]
fn touch_contacts_keep_independent_surfaces_and_native_coordinates() {
    use smithay::backend::input::InputTime;
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _touch = seat.get_touch(&qh, ());
        let mut client = Client::default();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 320 * 240 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 320 * 240 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 320, 240, 320 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let mut windows = Vec::new();
        for name in ["first", "second"] {
            let surface = compositor.create_surface(&qh, ());
            let xdg = shell.get_xdg_surface(&surface, &qh, ());
            let top = xdg.get_toplevel(&qh, ());
            top.set_app_id(name.into());
            top.set_min_size(320, 240);
            top.set_max_size(320, 240);
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, 320, 240);
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            windows.push((surface, xdg, top));
        }
        steps.step(());
        queue.roundtrip(&mut client).unwrap();
        let first = windows[0].0.id().protocol_id();
        let second = windows[1].0.id().protocol_id();
        // Each finger keeps its own surface even while the other screen has a contact. Motion
        // beyond a screen boundary stays in the first surface's original coordinate transform.
        assert_eq!(
            client.touches,
            vec![
                TouchRecord::Down(0, first, (100.0, 80.0)),
                TouchRecord::Down(1, second, (120.0, 90.0)),
                TouchRecord::Frame,
                TouchRecord::Motion(0, (1000.0, 80.0)),
                TouchRecord::Up(0),
                TouchRecord::Frame,
                TouchRecord::Motion(1, (120.0, 90.0)),
                TouchRecord::Cancel,
                TouchRecord::Down(4, first, (128.0, 64.0)),
                TouchRecord::Up(4),
                TouchRecord::Frame,
                TouchRecord::Down(5, first, (128.0, 64.0)),
                TouchRecord::Frame,
                TouchRecord::Motion(5, (128.0, 64.0)),
                TouchRecord::Cancel,
            ]
        );
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ()| {
        let first = server.windows[0].id;
        let second = server.windows[1].id;
        server.core.handle(Event::HardwareKey(HardwareKey::WindowDown));
        server.core.handle(clamshell_state::WindowEvent::Activate(first).into());
        server.frame();
        let top = server.geometry(first).unwrap();
        let bottom = server.geometry(second).unwrap();
        let time = InputTime::from_millis(10);
        server.touch_down(Some(0).into(), top.origin + smithay::utils::Point::from((100.0, 80.0)), time);
        server.touch_down(Some(1).into(), bottom.origin + smithay::utils::Point::from((120.0, 90.0)), time);
        server.touch_frame();
        server.touch_motion(Some(0).into(), top.origin + smithay::utils::Point::from((1000.0, 80.0)), time);
        server.touch_up(Some(0).into(), time);
        server.touch_frame();
        // Black margins and the bottom bar must not leak touches into either client.
        server.touch_down(Some(2).into(), (0.0, 0.0).into(), time);
        server.touch_motion(Some(2).into(), top.origin + smithay::utils::Point::from((20.0, 20.0)), time);
        server.touch_up(Some(2).into(), time);
        server.touch_down(Some(3).into(), (server.screens.top_width as f64 + 10.0, 10.0).into(), time);
        server.touch_cancel();
        server.touch_up(Some(1).into(), time);
        server.touch_motion(Some(1).into(), bottom.origin, time);
        assert!(server.touch.contacts.iter().all(Option::is_none));
        server.resize((790, 135));
        let scaled = server.geometry(first).unwrap();
        assert!(scaled.scale.x < 1.0);
        let point = scaled.origin + smithay::utils::Point::from((128.0 * scaled.scale.x, 64.0 * scaled.scale.y));
        server.touch_down(Some(4).into(), point, time);
        server.touch_up(Some(4).into(), time);
        server.touch_frame();
        server.touch_down(Some(5).into(), point, time);
        server.touch_frame();
        server.core.handle(Event::Home);
        server.frame();
        assert!(server.touch.contacts.iter().all(Option::is_none), "hiding a touched client cancels the gesture");
    });
    assert_eq!(checked, 1);
}

#[test]
fn touching_a_home_arrow_switches_once_and_consumes_home_release() {
    use clamshell_state::{WindowEvent, session::windows::WindowFacts};
    use smithay::backend::input::InputTime;
    let (_display, mut server) = ServerBuilder::new().build();
    server.core.handle(Event::Start);
    // Two windows of one launch, the first in front.
    for id in [1, 2] {
        let unit = Some("clamshell-run-test-1-0.service".into());
        server.core.handle(WindowEvent::Opened(WindowFacts { id, app_id: "test".into(), unit, ..WindowFacts::default() }).into());
    }
    server.core.handle(WindowEvent::Activate(1).into());
    server.core.handle(Event::HomeHeld(true));
    assert!(server.core.state().session().has_other_top_window());
    // The arrows are drawn, as they are before they can be seen and touched.
    assert!(server.core.record(clamshell_ui::Layer::Overlay(clamshell_ui::Screen::Top), false).is_some());
    let time = InputTime::from_millis(10);
    let point = (server.screens.top_width as f64 - 20.0, server.screens.size.1 as f64 / 2.0).into();
    server.touch_down(Some(0).into(), point, time);
    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(2));
    server.touch_motion(Some(0).into(), point, time);
    server.touch_up(Some(0).into(), time);
    server.touch_up(Some(0).into(), time);
    server.touch_frame();
    assert!(!server.touch.pending);
    assert!(server.touch.contacts.iter().all(Option::is_none));
    server.core.handle(Event::HomeHeld(false));
    server.core.handle(Event::HomeTap);
    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(2));
}

#[test]
fn the_overview_has_its_windows_in_cells_and_a_touch_brings_one_in_front() {
    use clamshell_state::session::overview::OverviewEvent;
    use clamshell_state::{WindowEvent, session::windows::WindowFacts};
    use smithay::backend::input::InputTime;
    let (_display, mut server) = ServerBuilder::new().build();
    server.core.handle(Event::Start);
    for id in [1, 2] {
        let unit = Some("clamshell-run-test-1-0.service".into());
        server.core.handle(WindowEvent::Opened(WindowFacts { id, app_id: "test".into(), unit, ..WindowFacts::default() }).into());
    }
    server.core.handle(WindowEvent::Activate(2).into());
    assert_eq!(server.overview_cell(1), None);
    server.core.handle(Event::Overview(OverviewEvent::Toggle));
    // Side by side on the top screen.
    let (left, right) = (server.overview_cell(1).unwrap(), server.overview_cell(2).unwrap());
    assert!(left.loc.x > 0 && left.loc.x + left.size.w < right.loc.x && right.loc.x + right.size.w < server.screens.top_width);
    assert_eq!((left.loc.y, left.size), (right.loc.y, right.size));
    // The cells are drawn, as they are before they can be seen and touched.
    assert!(server.core.record(clamshell_ui::Layer::Overlay(clamshell_ui::Screen::Top), false).is_some());
    let time = InputTime::from_millis(10);
    let point = (f64::from(left.loc.x + left.size.w / 2), f64::from(left.loc.y + left.size.h / 2)).into();
    server.touch_down(Some(0).into(), point, time);
    server.touch_up(Some(0).into(), time);
    server.touch_frame();
    let session = server.core.state().session();
    assert_eq!((session.top_window().map(|w| w.id), session.overview()), (Some(1), None));
    assert_eq!(server.overview_cell(2), None);
}

#[test]
fn touching_a_notification_taps_it_and_its_close_mark_dismisses_it() {
    use clamshell_state::{Notification, NotificationEvent, NotificationId, Urgency};
    use smithay::backend::input::InputTime;
    let display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540) };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    for id in [1, 2] {
        let n = Notification {
            id: NotificationId::Bus(id),
            app: "mail".into(),
            summary: format!("note {id}"),
            body: String::new(),
            urgency: Urgency::Normal,
            action: Some("default".into()),
            timeout_ms: 0,
        };
        server.core.handle(NotificationEvent::Notified(Box::new(n)).into());
    }
    // Drawn, as they are before they can be seen and touched.
    let bottom = clamshell_ui::Layer::Overlay(clamshell_ui::Screen::Bottom);
    assert!(server.core.record(bottom, false).is_some());
    let time = InputTime::from_millis(10);
    // The newest, 2, is the first row below the bar; its close mark is at the row's right end.
    let close = (f64::from(server.screens.top_width) + 600.0, 100.0).into();
    server.touch_down(Some(0).into(), close, time);
    server.touch_up(Some(0).into(), time);
    let held = |server: &Server| server.core.state().notifications().iter().map(|n| n.id).collect::<Vec<_>>();
    assert_eq!(held(&server), [NotificationId::Bus(1)], "the close mark dismissed 2");
    assert!(server.core.record(bottom, false).is_some());
    let row = (f64::from(server.screens.top_width) + 100.0, 100.0).into();
    server.touch_down(Some(0).into(), row, time);
    server.touch_up(Some(0).into(), time);
    assert!(held(&server).is_empty(), "a tap on the row dismissed 1");
    assert!(server.touch.contacts.iter().all(Option::is_none));
}

mod keyboard_tests;

mod dialog_tests;
mod idle_tests;

#[test]
fn a_protocol_error_disconnects_only_its_client() {
    client_error_is_isolated(false);
}

#[test]
fn a_rejected_dmabuf_disconnects_only_its_client() {
    client_error_is_isolated(true);
}

fn client_error_is_isolated(dmabuf_error: bool) {
    let (mut display, mut server) = ServerBuilder::new().build();
    use smithay::backend::allocator::{Format, Fourcc, Modifier};
    server.advertise_dmabuf_formats(
        [Format { code: Fourcc::Xrgb8888, modifier: Modifier::Linear }, Format { code: Fourcc::Abgr2101010, modifier: Modifier::Linear }],
        dmabuf_error.then_some(42),
    );
    server.core.handle(Event::Start);
    server.frame();
    let (bad_server, bad_client) = UnixStream::pair().unwrap();
    display.handle().insert_client(bad_server, Arc::new(ClientState::default())).unwrap();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("healthy".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(0);
        let bad = Connection::from_socket(bad_client).unwrap();
        let (globals, mut bad_queue) = registry_queue_init::<Client>(&bad).unwrap();
        let qh = bad_queue.handle();
        if dmabuf_error {
            use dmabuf_tests::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
            let manager: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ()).unwrap();
            let mut bad_state = Client::default();
            bad_queue.roundtrip(&mut bad_state).unwrap();
            assert_eq!(bad_state.dmabuf_formats, vec![(Fourcc::Xrgb8888 as u32, 0), (Fourcc::Abgr2101010 as u32, 0)]);
            let feedback_manager: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 4..=4, ()).unwrap();
            let feedback = feedback_manager.get_default_feedback(&qh, ());
            bad_queue.roundtrip(&mut bad_state).unwrap();
            assert!(bad_state.feedback_done);
            assert_eq!(bad_state.feedback_device, 42_u64.to_ne_bytes());
            let expected: Vec<_> = [Fourcc::Xrgb8888, Fourcc::Abgr2101010]
                .into_iter()
                .flat_map(|format| (format as u32).to_ne_bytes().into_iter().chain(0_u32.to_ne_bytes()).chain(0_u64.to_ne_bytes()))
                .collect();
            assert_eq!(bad_state.feedback_table, expected);
            feedback.destroy();
            let params = manager.create_params(&qh, ());
            let file = tempfile::tempfile().unwrap();
            file.set_len(4096).unwrap();
            params.add(file.as_fd(), 0, 0, 64, 0, 0);
            params.create(16, 16, Fourcc::Xrgb8888 as u32, zwp_linux_buffer_params_v1::Flags::empty());
            assert!(bad_queue.roundtrip(&mut bad_state).is_err(), "a rejected import must end only its connection");
        } else {
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
            let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
            let broken = compositor.create_surface(&qh, ());
            let role = shell.get_xdg_surface(&broken, &qh, ());
            let _one = role.get_toplevel(&qh, ());
            let _two = role.get_toplevel(&qh, ());
            assert!(bad_queue.roundtrip(&mut Client::default()).is_err(), "a duplicate role must end this connection");
        }
        top.set_title("still responsive".into());
        queue.roundtrip(&mut client).unwrap();
        steps.step(1);
    });
    let mut imports = 0;
    let turn = |server: &mut Server| {
        server.complete_dmabuf_imports(|_| {
            imports += 1;
            Err("test renderer cannot import this fd")
        });
        server.frame();
    };
    let checked = client.serve(&mut display, &mut server, turn, |_, server, phase| {
        assert_eq!(server.windows.len(), 1);
        assert!(server.windows[0].mapped);
        assert!(!server.exit);
        assert_eq!(server.core.state().session().windows()[0].app_id, "healthy");
        if phase == 1 {
            assert_eq!(server.core.state().session().windows()[0].title, "still responsive");
        }
    });
    assert_eq!(imports, usize::from(dmabuf_error), "the failure must reach the renderer boundary");
    assert_eq!(checked, 2);
}

#[test]
fn screenshots_are_consumed_by_the_host_instead_of_the_parent_capture_tool() {
    use std::{cell::RefCell, rc::Rc};
    let forwarded = Rc::new(RefCell::new(Vec::new()));
    let effects = forwarded.clone();
    let (_display, mut server) = ServerBuilder::new().on_effect(move |effect| effects.borrow_mut().push(effect.clone())).started();
    forwarded.borrow_mut().clear();
    server.core.handle(Event::HardwareKey(HardwareKey::Screenshot));
    server.core.handle(Event::HardwareKey(HardwareKey::Screenshot));
    server.frame();
    assert!(std::mem::take(&mut server.screenshot_requested));
    assert!(!std::mem::take(&mut server.screenshot_requested), "several requests before a frame need one capture");
    assert!(!forwarded.borrow().iter().any(|effect| matches!(effect, clamshell_state::Effect::Compositor(Compositor::Screenshot))));
    server.core.handle(Event::HardwareKey(HardwareKey::Screenshot));
    server.frame();
    assert!(server.screenshot_requested, "a later frame can capture again");
}

mod viewport_tests;

mod dmabuf_tests;

mod popup_tests;

mod cursor_tests;

mod activation_tests;

mod pointer_tests;

mod identify_tests;
mod pacing_tests;
mod presentation_tests;

mod x11_popup_tests;

#[test]
fn requested_preview_geometry_sets_the_initial_canvas_and_resize_ratio() {
    let (_display, mut server) = ServerBuilder::new().sizes((800, 600), (400, 600)).build();
    assert_eq!(server.screens.size, (1200, 600));
    assert_eq!(server.screens.top_width, 800);
    server.resize((900, 400));
    assert_eq!(server.screens.top_width, 600);
    assert_eq!(server.core.size(clamshell_ui::Screen::Bottom).0, (300, 400));
    server.resize((1, 1));
    assert_eq!(server.screens.size, (4, 4));
    assert_eq!(server.screens.top_width, 2);
}

/// Parallel process-spawn tests can retain CLOEXEC descriptors until their child reaches exec.
/// Wait for the kernel's hangup notification before asserting EOF, rather than racing that exec.
pub(crate) fn wait_closed(fd: &impl AsFd) {
    use rustix::event::{PollFd, PollFlags, Timespec};
    let mut fds = [PollFd::new(fd, PollFlags::HUP | PollFlags::ERR)];
    assert_eq!(rustix::event::poll(&mut fds, Some(&Timespec { tv_sec: 2, tv_nsec: 0 })).unwrap(), 1);
    assert!(fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR));
}

mod xwayland_shell {
    //! Xwayland pairs an X11 window with its surface by a serial given through `xwayland_shell_v1`.
    use super::*;
    use smithay::reexports::wayland_protocols::xwayland::shell::v1::client::{xwayland_shell_v1, xwayland_surface_v1};
    use smithay::reexports::wayland_server::Resource;

    delegate_noop!(Client: ignore xwayland_shell_v1::XwaylandShellV1);
    delegate_noop!(Client: ignore xwayland_surface_v1::XwaylandSurfaceV1);

    /// What a client did, step by step: the main loop checks the server after each.
    enum Step {
        /// Bound the shell, made a surface, gave it serial 7 and not yet committed.
        SerialSet,
        Committed,
        Destroyed,
    }

    #[test]
    fn a_committed_serial_pairs_its_surface_once_and_only_xwayland_sees_the_shell() {
        let (mut display, mut server) = ServerBuilder::new().build();
        // Another client doesn't see it.
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
        let other = std::thread::spawn(move || {
            let connection = Connection::from_socket(client_socket).unwrap();
            let (globals, _queue) = registry_queue_init::<Client>(&connection).unwrap();
            globals.contents().with_list(|list| list.iter().any(|global| global.interface == "xwayland_shell_v1"))
        });
        while !other.is_finished() {
            display.dispatch_clients(&mut server).unwrap();
            display.flush_clients().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!other.join().unwrap(), "only Xwayland sees xwayland_shell_v1");

        let client = spawn_client(&mut display, ClientState { xwayland: true, ..ClientState::default() }, |globals, mut queue, steps| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
            let shell: xwayland_shell_v1::XwaylandShellV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xwayland = shell.get_xwayland_surface(&surface, &qh, ());
            xwayland.set_serial(7, 0);
            let mut client = Client::default();
            queue.roundtrip(&mut client).unwrap();
            steps.step(Step::SerialSet);
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            steps.step(Step::Committed);
            // Another surface given a serial, then destroyed before its window was paired.
            let gone = compositor.create_surface(&qh, ());
            let gone_xwayland = shell.get_xwayland_surface(&gone, &qh, ());
            gone_xwayland.set_serial(8, 0);
            gone.commit();
            gone_xwayland.destroy();
            gone.destroy();
            queue.roundtrip(&mut client).unwrap();
            steps.step(Step::Destroyed);
        });
        let checked = client.serve(
            &mut display,
            &mut server,
            |_| {},
            |_, server, step| match step {
                Step::SerialSet => assert!(crate::x11::shell::take(server, 7).is_none(), "not paired before its commit"),
                Step::Committed => {
                    assert!(crate::x11::shell::take(server, 7).is_some_and(|surface| surface.is_alive()), "paired once committed");
                    assert!(crate::x11::shell::take(server, 7).is_none(), "and only once");
                }
                Step::Destroyed => {
                    assert!(crate::x11::shell::take(server, 8).is_none_or(|surface| !surface.is_alive()), "a destroyed surface isn't paired alive");
                }
            },
        );
        assert_eq!(checked, 3);
    }
}
