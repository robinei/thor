//! Real xdg-shell traffic through a socket pair, without a GPU or a host compositor.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use clamshell_state::Compositor;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use clamshell_state::{Event, Init, State, device::buttons::DeviceKey, session::windows::WindowPlace};
use clamshell_ui::Config;
use smithay::reexports::{
    wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
    wayland_server::Display,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_touch},
};

use crate::clients::{ClientState, Process};
use crate::server::Server;

#[derive(Default)]
struct Client {
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

#[derive(Debug)]
enum Report {
    Configured((i32, i32)),
    Mapped,
    Below((i32, i32)),
    Renamed,
    Unmapped,
    Remapped,
    Child((i32, i32)),
    Closed,
}

#[test]
fn xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu() {
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(
        State::new(Init::default()),
        config,
        display.handle(),
        false,
        || {},
        |_| Process { unit: Some("test.service".into()), pids: vec![100], steam_app: None },
        |_| {},
    )
    .unwrap();
    server.core.handle(Event::Start);
    server.frame();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("test".into());
        top.set_title("before configure".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Configured(*client.sizes.last().unwrap())).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();

        let (width, height) = *client.sizes.last().unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; (width * height * 4) as usize]).unwrap();
        let pool = shm.create_pool(file.as_fd(), width * height * 4, &qh, ());
        let buffer = pool.create_buffer(0, width, height, width * 4, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        client.ignore_configures = true;
        report.send(Report::Mapped).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Below(*client.sizes.last().unwrap())).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        for number in 0..100 {
            top.set_title(format!("intermediate title {number}"));
        }
        top.set_title("after configure".into());
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Renamed).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..100 {
            top.set_title("after configure".into());
        }
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Renamed).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();

        client.ignore_configures = false;
        surface.attach(None, 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Unmapped).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        // Unmapping resets xdg properties; clients must set them again before remapping.
        top.set_app_id("test".into());
        top.set_title("after configure".into());
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Remapped).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();

        let child_surface = compositor.create_surface(&qh, ());
        let child_xdg = shell.get_xdg_surface(&child_surface, &qh, ());
        let child = child_xdg.get_toplevel(&qh, ());
        child.set_parent(Some(&top));
        child.set_app_id("dialog".into());
        child.set_min_size(320, 240);
        child.set_max_size(320, 240);
        child_surface.commit();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Child(*client.sizes.last().unwrap())).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        child.destroy();
        child_xdg.destroy();
        child_surface.destroy();
        top.destroy();
        xdg.destroy();
        surface.destroy();
        buffer.destroy();
        pool.destroy();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Closed).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        while let Ok(report) = reports.try_recv() {
            let windows = server.core.state().session().windows();
            match report {
                Report::Configured(size) => {
                    assert_eq!(size, (960, 540));
                    assert_eq!(windows.len(), 1);
                    assert_eq!(windows[0].title, "before configure");
                    assert_eq!(windows[0].unit.as_deref(), Some("test.service"));
                    assert!(matches!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Top(_)));
                    assert!(!server.windows[0].mapped, "initial configure precedes the first buffer");
                    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(windows[0].id));
                    next.send(()).unwrap();
                }
                Report::Mapped => {
                    use clamshell_state::session::presentation::{WindowLayout, WindowRole};
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
                    server.core.handle(Event::Device(DeviceKey::WindowDown));
                    server.frame();
                    display.flush_clients().unwrap();
                    next.send(()).unwrap();
                }
                Report::Below(size) => {
                    // The client keeps its original buffer and stops acknowledging configure. It
                    // cannot hold up placement; rendering and input use a fitted transform at once.
                    let geometry = server.geometry(windows[0].id).unwrap();
                    assert!(geometry.scale.x < 1.0);
                    assert_eq!(geometry.native.size, (960, 540).into());
                    assert_eq!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Bottom);
                    assert_eq!(size, (server.area(true).size.w, server.area(true).size.h));
                    next.send(()).unwrap();
                }
                Report::Renamed => {
                    let recording = server.trace.borrow().snapshot(server.core.state());
                    let titles = recording
                        .records
                        .iter()
                        .filter(|record| matches!(&record.input, crate::trace::Input::Window(clamshell_state::WindowEvent::Title { .. })))
                        .count();
                    assert_eq!(titles, 1, "a title burst enters state and diagnostics once per frame");
                    assert_eq!(windows[0].title, "after configure");
                    assert_eq!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Bottom);
                    next.send(()).unwrap();
                }
                Report::Unmapped => {
                    assert!(windows.is_empty());
                    assert!(!server.windows[0].mapped);
                    next.send(()).unwrap();
                }
                Report::Remapped => {
                    assert_eq!(windows.len(), 1);
                    assert_eq!(windows[0].id, server.windows[0].id);
                    assert!(server.windows[0].mapped);
                    assert_eq!(windows[0].title, "after configure");
                    assert!(
                        matches!(server.core.state().session().place(windows[0].id).unwrap(), WindowPlace::Top(_)),
                        "this unowned client has no persistent bottom rule"
                    );
                    server.core.handle(Event::Device(DeviceKey::WindowDown));
                    server.frame();
                    next.send(()).unwrap();
                }
                Report::Child(size) => {
                    assert_eq!(windows.len(), 2);
                    assert_eq!(windows[1].parent, Some(windows[0].id));
                    assert_eq!(server.core.state().session().place(windows[1].id).unwrap(), WindowPlace::Bottom);
                    assert_eq!(size, (320, 240));
                    assert_eq!(windows[1].limits.fixed_size(), Some((320, 240)));
                    next.send(()).unwrap();
                }
                Report::Closed => {
                    assert!(windows.is_empty());
                    assert!(server.windows.is_empty());
                }
            }
            phases += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished(), "client did not complete the protocol exchange");
    thread.join().unwrap();
    // The final report can arrive just as the client thread exits.
    if let Ok(Report::Closed) = reports.try_recv() {
        assert!(server.core.state().session().windows().is_empty());
        assert!(server.windows.is_empty());
        phases += 1;
    }
    assert_eq!(phases, 9);
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
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    server.core.handle(Event::Start);
    server.frame();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
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
        report.send(()).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
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
    let until = Instant::now() + Duration::from_secs(15);
    let mut checked = false;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        if reports.try_recv().is_ok() {
            let first = server.windows[0].id;
            let second = server.windows[1].id;
            server.core.handle(Event::Device(DeviceKey::WindowDown));
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
            server.touch_down(Some(3).into(), (server.top_width as f64 + 10.0, 10.0).into(), time);
            server.touch_cancel();
            server.touch_up(Some(1).into(), time);
            server.touch_motion(Some(1).into(), bottom.origin, time);
            assert!(server.touches.iter().all(Option::is_none));
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
            assert!(server.touches.iter().all(Option::is_none), "hiding a touched client cancels the gesture");
            display.flush_clients().unwrap();
            next.send(()).unwrap();
            checked = true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(checked);
    assert!(thread.is_finished());
    thread.join().unwrap();
}

#[test]
fn touching_a_home_arrow_switches_once_and_consumes_home_release() {
    use clamshell_state::{WindowEvent, session::windows::WindowFacts};
    use smithay::backend::input::InputTime;
    let display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
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
    assert!(server.core.record(clamshell_ui::Layer::Overlay, false).is_some());
    let time = InputTime::from_millis(10);
    let point = (server.top_width as f64 - 20.0, server.size.1 as f64 / 2.0).into();
    server.touch_down(Some(0).into(), point, time);
    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(2));
    server.touch_motion(Some(0).into(), point, time);
    server.touch_up(Some(0).into(), time);
    server.touch_up(Some(0).into(), time);
    server.touch_frame();
    assert!(!server.touch_pending);
    assert!(server.touches.iter().all(Option::is_none));
    server.core.handle(Event::HomeHeld(false));
    server.core.handle(Event::HomeTap);
    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(2));
}

#[path = "keyboard_tests.rs"]
mod keyboard_tests;

#[path = "dialog_tests.rs"]
mod dialog_tests;
#[path = "idle_tests.rs"]
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
    let mut display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    use smithay::backend::allocator::{Format, Fourcc, Modifier};
    server.advertise_dmabuf_formats(
        [Format { code: Fourcc::Xrgb8888, modifier: Modifier::Linear }, Format { code: Fourcc::Abgr2101010, modifier: Modifier::Linear }],
        dmabuf_error.then_some(42),
    );
    server.core.handle(Event::Start);
    server.frame();
    let (good_server, good_client) = UnixStream::pair().unwrap();
    let (bad_server, bad_client) = UnixStream::pair().unwrap();
    for stream in [good_server, bad_server] {
        display.handle().insert_client(stream, Arc::new(ClientState::default())).unwrap();
    }
    let (report, reports) = mpsc::channel();
    let (next, steps) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(good_client).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
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
        report.send(0).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
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
        report.send(1).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    let mut imports = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.complete_dmabuf_imports(|_| {
            imports += 1;
            Err("test renderer cannot import this fd")
        });
        server.frame();
        display.flush_clients().unwrap();
        if let Ok(phase) = reports.try_recv() {
            assert_eq!(server.windows.len(), 1);
            assert!(server.windows[0].mapped);
            assert!(!server.exit);
            assert_eq!(server.core.state().session().windows()[0].app_id, "healthy");
            if phase == 1 {
                assert_eq!(server.core.state().session().windows()[0].title, "still responsive");
            }
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(imports, usize::from(dmabuf_error), "the failure must reach the renderer boundary");
    assert_eq!(phases, 2);
    assert!(thread.is_finished());
    thread.join().unwrap();
}

#[test]
fn screenshots_are_consumed_by_the_host_instead_of_the_parent_capture_tool() {
    use std::{cell::RefCell, rc::Rc};
    let display = Display::<Server>::new().unwrap();
    let forwarded = Rc::new(RefCell::new(Vec::new()));
    let effects = forwarded.clone();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let mut server = Server::new(
        State::new(Init::default()),
        config,
        display.handle(),
        false,
        || {},
        |_| Process::default(),
        move |effect| effects.borrow_mut().push(effect.clone()),
    )
    .unwrap();
    server.core.handle(Event::Start);
    server.frame();
    forwarded.borrow_mut().clear();
    server.core.handle(Event::Device(DeviceKey::Screenshot));
    server.core.handle(Event::Device(DeviceKey::Screenshot));
    server.frame();
    assert!(std::mem::take(&mut server.screenshot_requested));
    assert!(!std::mem::take(&mut server.screenshot_requested), "several requests before a frame need one capture");
    assert!(!forwarded.borrow().iter().any(|effect| matches!(effect, clamshell_state::Effect::Compositor(Compositor::Screenshot))));
    server.core.handle(Event::Device(DeviceKey::Screenshot));
    server.frame();
    assert!(server.screenshot_requested, "a later frame can capture again");
}

#[path = "viewport_tests.rs"]
mod viewport_tests;

#[path = "dmabuf_tests.rs"]
mod dmabuf_tests;

#[path = "popup_tests.rs"]
mod popup_tests;

#[path = "cursor_tests.rs"]
mod cursor_tests;

#[path = "activation_tests.rs"]
mod activation_tests;

#[path = "pointer_tests.rs"]
mod pointer_tests;

#[path = "pacing_tests.rs"]
mod pacing_tests;
#[path = "presentation_tests.rs"]
mod presentation_tests;

#[path = "x11_popup_tests.rs"]
mod x11_popup_tests;

#[test]
fn requested_preview_geometry_sets_the_initial_canvas_and_resize_ratio() {
    let display = Display::<Server>::new().unwrap();
    let config = Config { top_size: (800, 600), bottom_size: (400, 600), touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    assert_eq!(server.size, (1200, 600));
    assert_eq!(server.top_width, 800);
    server.resize((900, 400));
    assert_eq!(server.top_width, 600);
    assert_eq!(server.core.size(clamshell_ui::Screen::Bottom).0, (300, 400));
    server.resize((1, 1));
    assert_eq!(server.size, (4, 4));
    assert_eq!(server.top_width, 2);
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
        let mut display = Display::<Server>::new().unwrap();
        let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
        let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
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

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        display.handle().insert_client(server_socket, Arc::new(ClientState { xwayland: true, ..ClientState::default() })).unwrap();
        let (report, reports) = mpsc::channel();
        let (next, steps) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let connection = Connection::from_socket(client_socket).unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
            let shell: xwayland_shell_v1::XwaylandShellV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xwayland = shell.get_xwayland_surface(&surface, &qh, ());
            xwayland.set_serial(7, 0);
            let mut client = Client::default();
            queue.roundtrip(&mut client).unwrap();
            report.send(Step::SerialSet).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
            surface.commit();
            queue.roundtrip(&mut client).unwrap();
            report.send(Step::Committed).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
            // Another surface given a serial, then destroyed before its window was paired.
            let gone = compositor.create_surface(&qh, ());
            let gone_xwayland = shell.get_xwayland_surface(&gone, &qh, ());
            gone_xwayland.set_serial(8, 0);
            gone.commit();
            gone_xwayland.destroy();
            gone.destroy();
            queue.roundtrip(&mut client).unwrap();
            report.send(Step::Destroyed).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let until = Instant::now() + Duration::from_secs(15);
        let mut steps_seen = 0;
        while !thread.is_finished() && Instant::now() < until {
            display.dispatch_clients(&mut server).unwrap();
            display.flush_clients().unwrap();
            if let Ok(step) = reports.try_recv() {
                match step {
                    Step::SerialSet => assert!(crate::x11::shell::take(&mut server, 7).is_none(), "not paired before its commit"),
                    Step::Committed => {
                        assert!(crate::x11::shell::take(&mut server, 7).is_some_and(|surface| surface.is_alive()), "paired once committed");
                        assert!(crate::x11::shell::take(&mut server, 7).is_none(), "and only once");
                    }
                    Step::Destroyed => {
                        assert!(crate::x11::shell::take(&mut server, 8).is_none_or(|surface| !surface.is_alive()), "a destroyed surface isn't paired alive");
                    }
                }
                steps_seen += 1;
                next.send(()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(steps_seen, 3);
        thread.join().unwrap();
    }
}
