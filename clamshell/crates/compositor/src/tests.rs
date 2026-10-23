//! Real xdg-shell traffic through a socket pair, without a GPU or a host compositor.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

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
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};

use crate::server::{ClientState, Process, Server};

#[derive(Default)]
struct Client {
    sizes: Vec<(i32, i32)>,
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
    fn event(_: &mut Self, proxy: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
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
    Child((i32, i32)),
    Closed,
}

#[test]
fn xdg_clients_are_placed_before_configure_and_follow_state_without_a_gpu() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server =
        Server::new(State::new(Init::default()), config, display.handle(), || {}, |_| Process { unit: Some("test.service".into()), pids: vec![100] }, |_| {})
            .unwrap();
    server.core.handle(Event::Start);
    server.turn();
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
        report.send(Report::Mapped).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Below(*client.sizes.last().unwrap())).unwrap();
        steps.recv_timeout(Duration::from_secs(5)).unwrap();
        top.set_title("after configure".into());
        queue.roundtrip(&mut client).unwrap();
        report.send(Report::Renamed).unwrap();
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
        server.turn();
        display.flush_clients().unwrap();
        while let Ok(report) = reports.try_recv() {
            let windows = server.core.state().session().windows();
            match report {
                Report::Configured(size) => {
                    assert_eq!(size, (960, 540));
                    assert_eq!(windows.len(), 1);
                    assert_eq!(windows[0].title, "before configure");
                    assert_eq!(windows[0].unit.as_deref(), Some("test.service"));
                    assert!(matches!(windows[0].place(), WindowPlace::Top(_)));
                    assert!(!server.windows[0].mapped, "initial configure precedes the first buffer");
                    assert_eq!(server.core.state().session().top_window().map(|w| w.id), Some(windows[0].id));
                    next.send(()).unwrap();
                }
                Report::Mapped => {
                    assert!(server.windows[0].mapped);
                    server.core.handle(Event::Device(DeviceKey::WindowDown));
                    server.turn();
                    display.flush_clients().unwrap();
                    next.send(()).unwrap();
                }
                Report::Below(size) => {
                    assert_eq!(windows[0].place(), WindowPlace::Bottom);
                    assert_eq!(size, (server.area(true).size.w, server.area(true).size.h));
                    next.send(()).unwrap();
                }
                Report::Renamed => {
                    assert_eq!(windows[0].title, "after configure");
                    assert_eq!(windows[0].place(), WindowPlace::Bottom);
                    next.send(()).unwrap();
                }
                Report::Child(size) => {
                    assert_eq!(windows.len(), 2);
                    assert_eq!(windows[1].parent, Some(windows[0].id));
                    assert_eq!(windows[1].place(), WindowPlace::Bottom);
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
    assert_eq!(phases, 6);
}
