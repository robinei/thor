//! Real menus borrow focus and cannot keep it when their parent is hidden.

use super::*;
use smithay::{
    backend::input::{ButtonState, InputTime, TouchSlot},
    input::pointer::{ButtonEvent, MotionEvent},
    reexports::wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner},
    utils::SERIAL_COUNTER,
};
use wayland_client::protocol::wl_pointer;

delegate_noop!(Client: ignore xdg_positioner::XdgPositioner);
impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
    fn event(state: &mut Self, _: &xdg_popup::XdgPopup, event: xdg_popup::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_popup::Event::PopupDone = event {
            state.popup_done += 1;
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(state: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Enter { serial, .. } = event {
            state.cursor_enter = serial;
        }
        if let wl_pointer::Event::Button { serial, state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed), .. } = event {
            state.pointer_serial = serial;
        }
    }
}

#[test]
fn menus_keep_popup_focus_and_dismiss_on_outside_touch_home_or_client_destroy() {
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), || {}, |_| Process::default(), |_| {}).unwrap();
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
        let _pointer = seat.get_pointer(&qh, ());
        let _keyboard = seat.get_keyboard(&qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("menu-test".into());
        surface.commit();
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0xff; 960 * 540 * 4]).unwrap();
        let pool = shm.create_pool(file.as_fd(), 960 * 540 * 4, &qh, ());
        let buffer = pool.create_buffer(0, 960, 540, 960 * 4, wl_shm::Format::Abgr8888, &qh, ());
        let menu_buffer = pool.create_buffer(0, 100, 80, 400, wl_shm::Format::Abgr8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        for cycle in 0..4 {
            report.send((cycle, 0)).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
            queue.roundtrip(&mut client).unwrap();
            assert_ne!(client.pointer_serial, 0);
            let menu_surface = compositor.create_surface(&qh, ());
            let menu_xdg = shell.get_xdg_surface(&menu_surface, &qh, ());
            let positioner = shell.create_positioner(&qh, ());
            positioner.set_size(100, 80);
            positioner.set_anchor_rect(100, 100, 1, 1);
            positioner.set_anchor(xdg_positioner::Anchor::BottomRight);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
            let menu = menu_xdg.get_popup(Some(&xdg), &positioner, &qh, ());
            menu.grab(&seat, client.pointer_serial);
            menu_surface.commit();
            queue.roundtrip(&mut client).unwrap();
            menu_surface.attach(Some(&menu_buffer), 0, 0);
            menu_surface.commit();
            queue.roundtrip(&mut client).unwrap();
            report.send((cycle, 1)).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
            queue.roundtrip(&mut client).unwrap();
            assert_eq!(client.popup_done, (cycle + 1).min(2));
            if cycle == 2 {
                let child_surface = compositor.create_surface(&qh, ());
                let child_xdg = shell.get_xdg_surface(&child_surface, &qh, ());
                positioner.set_anchor_rect(20, 20, 1, 1);
                let child = child_xdg.get_popup(Some(&menu_xdg), &positioner, &qh, ());
                child.grab(&seat, client.pointer_serial);
                child_surface.commit();
                queue.roundtrip(&mut client).unwrap();
                child_surface.attach(Some(&menu_buffer), 0, 0);
                child_surface.commit();
                queue.roundtrip(&mut client).unwrap();
                report.send((cycle, 3)).unwrap();
                steps.recv_timeout(Duration::from_secs(5)).unwrap();
                child.destroy();
                child_xdg.destroy();
                child_surface.destroy();
                queue.roundtrip(&mut client).unwrap();
                report.send((cycle, 4)).unwrap();
                steps.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            if cycle == 3 {
                surface.attach(None, 0, 0);
                surface.commit();
                queue.roundtrip(&mut client).unwrap();
                report.send((cycle, 5)).unwrap();
                steps.recv_timeout(Duration::from_secs(5)).unwrap();
                queue.roundtrip(&mut client).unwrap();
                assert_eq!(client.popup_done, 3);
            }
            menu.destroy();
            menu_xdg.destroy();
            menu_surface.destroy();
            positioner.destroy();
            if cycle == 3 {
                top.set_app_id("menu-test".into());
                surface.commit();
                queue.roundtrip(&mut client).unwrap();
                surface.attach(Some(&buffer), 0, 0);
                surface.commit();
            }
            queue.roundtrip(&mut client).unwrap();
            report.send((cycle, 2)).unwrap();
            steps.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut phases = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        server.frame();
        display.flush_clients().unwrap();
        if let Ok((cycle, phase)) = reports.try_recv() {
            let id = server.windows[0].id;
            let root = server.windows[0].window.toplevel().unwrap().wl_surface().clone();
            let keyboard = server.seat.get_keyboard().unwrap();
            let pointer = server.seat.get_pointer().unwrap();
            assert_eq!(server.core.state().session().windows().len(), usize::from(phase != 5), "menus never become state windows");
            match phase {
                0 => {
                    assert!(server.menu.is_none());
                    server.core.handle(clamshell_state::WindowEvent::Activate(id).into());
                    server.frame();
                    pointer.motion(
                        &mut server,
                        Some((root, (0.0, 0.0).into())),
                        &MotionEvent { location: (100.0, 100.0).into(), serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() },
                    );
                    pointer.button(
                        &mut server,
                        &ButtonEvent { button: 0x110, state: ButtonState::Pressed, serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() },
                    );
                    pointer.frame(&mut server);
                }
                1 => {
                    assert!(server.menu.is_some());
                    assert!(keyboard.is_grabbed());
                    assert_ne!(keyboard.current_focus(), Some(root.clone()));
                    let hit = server.hit((111.0, 111.0).into()).unwrap();
                    assert_eq!(Some(hit.surface), keyboard.current_focus());
                    assert_eq!(hit.location - hit.origin, (10.0, 10.0).into());
                    pointer.button(
                        &mut server,
                        &ButtonEvent { button: 0x110, state: ButtonState::Released, serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() },
                    );
                    if cycle == 0 {
                        server.touch_down(TouchSlot::from(Some(0)), (1000.0, 20.0).into(), InputTime::now());
                        server.touch_up(TouchSlot::from(Some(0)), InputTime::now());
                        server.touch_frame();
                        server.frame();
                        assert!(server.menu.is_none());
                        assert_eq!(keyboard.current_focus(), Some(root));
                    } else if cycle == 1 {
                        server.core.handle(Event::Home);
                        server.frame();
                        assert!(server.menu.is_none());
                        assert_eq!(keyboard.current_focus(), None);
                        assert!(server.core.state().session().top_window().is_none());
                    }
                }
                2 => {
                    server.frame();
                    assert!(server.menu.is_none());
                    assert!(!keyboard.is_grabbed());
                    if cycle != 1 {
                        assert_eq!(keyboard.current_focus(), Some(root));
                    }
                }
                3 => {
                    assert!(server.menu.is_some());
                    assert!(keyboard.is_grabbed());
                    let hit = server.hit((132.0, 132.0).into()).unwrap();
                    assert_eq!(Some(hit.surface), keyboard.current_focus());
                    assert_eq!(hit.location - hit.origin, (10.0, 10.0).into());
                    assert_eq!(smithay::desktop::PopupManager::popups_for_surface(&root).count(), 2);
                }
                4 => {
                    assert!(server.menu.is_some(), "destroying a submenu keeps its parent grab");
                    assert!(keyboard.is_grabbed());
                    assert_eq!(Some(server.hit((111.0, 111.0).into()).unwrap().surface), keyboard.current_focus());
                    assert_eq!(smithay::desktop::PopupManager::popups_for_surface(&root).count(), 1);
                }
                5 => {
                    assert!(server.menu.is_none(), "unmapping the parent dismisses its menus");
                    assert!(!keyboard.is_grabbed());
                    assert_eq!(keyboard.current_focus(), None);
                    assert!(!server.windows[0].mapped);
                }
                _ => panic!("unexpected phase"),
            }
            phases += 1;
            next.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished());
    thread.join().unwrap();
    assert_eq!(phases, 15);
}
