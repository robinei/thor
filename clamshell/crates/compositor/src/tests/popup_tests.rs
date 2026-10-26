//! Real menus borrow focus and cannot keep it when their parent is hidden.

use super::*;
use smithay::reexports::wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
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

/// A client's window with the keyboard and the pointer, for menus over it.
struct MenuClient {
    globals: wayland_client::globals::GlobalList,
    seat: wl_seat::WlSeat,
    pointer: wl_pointer::WlPointer,
    window: Toplevel,
    menu_buffer: wl_buffer::WlBuffer,
    client: Client,
}

/// A menu the client opened (see [`MenuClient::open_menu`]).
struct Menu {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    popup: xdg_popup::XdgPopup,
    positioner: xdg_positioner::XdgPositioner,
}

impl MenuClient {
    /// Opens the window, and waits for the press the server gives it ([`Step::Shown`]).
    fn new(globals: wayland_client::globals::GlobalList, queue: &mut EventQueue<Client>, steps: &Steps<Step>) -> MenuClient {
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let _keyboard = seat.get_keyboard(&qh, ());
        let mut client = Client::default();
        let window = open_toplevel(&globals, queue, &mut client, "menu-test", Some((960, 540)));
        let menu_buffer = shm_buffer(&globals, &qh, (100, 80));
        steps.step(Step::Shown);
        queue.roundtrip(&mut client).unwrap();
        assert_ne!(client.pointer_serial, 0);
        MenuClient { globals, seat, pointer, window, menu_buffer, client }
    }

    /// Opens a 100×80 menu over `parent` at `anchor`, grabbing with the press's serial.
    fn open_menu(&mut self, queue: &mut EventQueue<Client>, parent: &xdg_surface::XdgSurface, anchor: (i32, i32)) -> Menu {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = self.globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = self.globals.bind(&qh, 1..=6, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let positioner = shell.create_positioner(&qh, ());
        positioner.set_size(100, 80);
        positioner.set_anchor_rect(anchor.0, anchor.1, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::BottomRight);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        let popup = xdg.get_popup(Some(parent), &positioner, &qh, ());
        popup.grab(&self.seat, self.client.pointer_serial);
        surface.commit();
        queue.roundtrip(&mut self.client).unwrap();
        surface.attach(Some(&self.menu_buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut self.client).unwrap();
        Menu { surface, xdg, popup, positioner }
    }
}

impl Menu {
    fn destroy(self, queue: &mut EventQueue<Client>, client: &mut Client) {
        self.popup.destroy();
        self.xdg.destroy();
        self.surface.destroy();
        self.positioner.destroy();
        queue.roundtrip(client).unwrap();
    }
}

/// What the client did, for the server to check.
#[derive(Debug, PartialEq)]
enum Step {
    /// Showed its window, which the server presses on.
    Shown,
    /// Locked the pointer.
    Locked,
    /// Opened a menu.
    Opened,
    /// Opened a submenu of it.
    Submenu,
    /// Destroyed the submenu.
    SubmenuGone,
    /// Unmapped the menu's window.
    ParentUnmapped,
    /// Destroyed the menu.
    Closed,
}

/// The server's part of each turn: a frame, and the pointer's capture granted to the host.
fn turn(server: &mut Server) {
    server.frame();
    server.sync_pointer_capture(|_| Ok::<_, &str>(()));
}

/// The window's root surface, and the seat's keyboard.
fn window(server: &Server) -> (WlSurface, smithay::input::keyboard::KeyboardHandle<Server>) {
    (server.windows[0].window.toplevel().unwrap().wl_surface().clone(), server.seat.get_keyboard().unwrap())
}

/// What every step checks, and then a step's own checks: at [`Step::Shown`] the window is given the
/// keyboard and pressed on, at [`Step::Opened`] the menu has the keyboard and pointer and the press ends.
fn check(server: &mut Server, step: &Step) {
    let id = server.windows[0].id;
    let (root, keyboard) = window(server);
    let pointer = server.seat.get_pointer().unwrap();
    assert_eq!(server.core.state().session().windows().len(), usize::from(*step != Step::ParentUnmapped), "menus never become state windows");
    match step {
        Step::Shown => {
            assert!(server.menu.is_none());
            server.core.handle(clamshell_state::WindowEvent::Activate(id).into());
            server.frame();
            pointer.motion(
                server,
                Some((root, (0.0, 0.0).into())),
                &MotionEvent { location: (100.0, 100.0).into(), serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() },
            );
            pointer.button(server, &ButtonEvent { button: 0x110, state: ButtonState::Pressed, serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() });
            pointer.frame(server);
        }
        Step::Opened => {
            assert!(server.menu.is_some());
            assert!(server.pointer.capture.is_none(), "opening a menu releases the game's pointer capture");
            assert!(keyboard.is_grabbed());
            assert_ne!(keyboard.current_focus(), Some(root.clone()));
            let hit = server.hit((111.0, 111.0).into()).unwrap();
            assert_eq!(Some(hit.surface), keyboard.current_focus());
            assert_eq!(hit.location - hit.origin, (10.0, 10.0).into());
            pointer.button(server, &ButtonEvent { button: 0x110, state: ButtonState::Released, serial: SERIAL_COUNTER.next_serial(), time: InputTime::now() });
        }
        _ => {}
    }
}

/// After the menu is destroyed: it is gone and its grab with it.
fn closed(server: &mut Server) {
    server.frame();
    assert!(server.menu.is_none());
    assert!(!window(server).1.is_grabbed());
}

#[test]
fn opening_a_menu_releases_the_pointer_capture_and_takes_the_keyboard() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let mut menus = MenuClient::new(globals, &mut queue, &steps);
        let constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 = menus.globals.bind(&qh, 1..=1, ()).unwrap();
        let lock = constraints.lock_pointer(&menus.window.surface, &menus.pointer, None, zwp_pointer_constraints_v1::Lifetime::Persistent, &qh, ());
        queue.roundtrip(&mut menus.client).unwrap();
        queue.roundtrip(&mut menus.client).unwrap();
        steps.step(Step::Locked);
        let parent = menus.window.xdg.clone();
        let _menu = menus.open_menu(&mut queue, &parent, (100, 100));
        steps.step(Step::Opened);
        lock.destroy();
    });
    let checked = client.serve(&mut display, &mut server, turn, |_, server, step| {
        check(server, &step);
        if step == Step::Locked {
            assert!(server.pointer_locked());
            assert_eq!(server.pointer.host_capture, crate::pointer::CaptureMode::Locked);
        }
        server.sync_pointer_capture(|_| Ok::<_, &str>(()));
    });
    assert_eq!(checked, 3);
}

#[test]
fn a_touch_outside_a_menu_dismisses_it_and_gives_back_the_keyboard() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut menus = MenuClient::new(globals, &mut queue, &steps);
        let parent = menus.window.xdg.clone();
        let menu = menus.open_menu(&mut queue, &parent, (100, 100));
        steps.step(Step::Opened);
        queue.roundtrip(&mut menus.client).unwrap();
        assert_eq!(menus.client.popup_done, 1);
        menu.destroy(&mut queue, &mut menus.client);
        steps.step(Step::Closed);
    });
    let checked = client.serve(&mut display, &mut server, turn, |_, server, step| {
        check(server, &step);
        let (root, keyboard) = window(server);
        match step {
            Step::Opened => {
                server.touch_down(TouchSlot::from(Some(0)), (1000.0, 20.0).into(), InputTime::now());
                server.touch_up(TouchSlot::from(Some(0)), InputTime::now());
                server.touch_frame();
                server.frame();
                assert!(server.menu.is_none());
                assert_eq!(keyboard.current_focus(), Some(root));
            }
            Step::Closed => {
                closed(server);
                assert_eq!(keyboard.current_focus(), Some(root));
            }
            _ => {}
        }
    });
    assert_eq!(checked, 3);
}

#[test]
fn home_dismisses_a_menu_and_takes_the_keyboard_from_its_window() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut menus = MenuClient::new(globals, &mut queue, &steps);
        let parent = menus.window.xdg.clone();
        let menu = menus.open_menu(&mut queue, &parent, (100, 100));
        steps.step(Step::Opened);
        queue.roundtrip(&mut menus.client).unwrap();
        assert_eq!(menus.client.popup_done, 1);
        menu.destroy(&mut queue, &mut menus.client);
        steps.step(Step::Closed);
    });
    let checked = client.serve(&mut display, &mut server, turn, |_, server, step| {
        check(server, &step);
        match step {
            Step::Opened => {
                server.core.handle(Event::Home);
                server.frame();
                assert!(server.menu.is_none());
                assert_eq!(window(server).1.current_focus(), None);
                assert!(server.core.state().session().top_window().is_none());
            }
            Step::Closed => closed(server),
            _ => {}
        }
    });
    assert_eq!(checked, 3);
}

#[test]
fn a_submenu_destroyed_leaves_its_parent_menu_the_grab() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut menus = MenuClient::new(globals, &mut queue, &steps);
        let parent = menus.window.xdg.clone();
        let menu = menus.open_menu(&mut queue, &parent, (100, 100));
        steps.step(Step::Opened);
        let submenu = menus.open_menu(&mut queue, &menu.xdg, (20, 20));
        steps.step(Step::Submenu);
        submenu.destroy(&mut queue, &mut menus.client);
        steps.step(Step::SubmenuGone);
        assert_eq!(menus.client.popup_done, 0);
        menu.destroy(&mut queue, &mut menus.client);
        steps.step(Step::Closed);
    });
    let checked = client.serve(&mut display, &mut server, turn, |_, server, step| {
        check(server, &step);
        let (root, keyboard) = window(server);
        match step {
            Step::Submenu => {
                assert!(server.menu.is_some());
                assert!(keyboard.is_grabbed());
                let hit = server.hit((132.0, 132.0).into()).unwrap();
                assert_eq!(Some(hit.surface), keyboard.current_focus());
                assert_eq!(hit.location - hit.origin, (10.0, 10.0).into());
                assert_eq!(smithay::desktop::PopupManager::popups_for_surface(&root).count(), 2);
            }
            Step::SubmenuGone => {
                assert!(server.menu.is_some(), "destroying a submenu keeps its parent grab");
                assert!(keyboard.is_grabbed());
                assert_eq!(Some(server.hit((111.0, 111.0).into()).unwrap().surface), keyboard.current_focus());
                assert_eq!(smithay::desktop::PopupManager::popups_for_surface(&root).count(), 1);
            }
            Step::Closed => {
                closed(server);
                assert_eq!(keyboard.current_focus(), Some(root));
            }
            _ => {}
        }
    });
    assert_eq!(checked, 5);
}

#[test]
fn unmapping_a_menus_window_dismisses_the_menu() {
    let (mut display, mut server) = ServerBuilder::new().relative_motion().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut menus = MenuClient::new(globals, &mut queue, &steps);
        let parent = menus.window.xdg.clone();
        let menu = menus.open_menu(&mut queue, &parent, (100, 100));
        steps.step(Step::Opened);
        menus.window.unmap(&mut queue, &mut menus.client);
        steps.step(Step::ParentUnmapped);
        queue.roundtrip(&mut menus.client).unwrap();
        assert_eq!(menus.client.popup_done, 1);
        menu.destroy(&mut queue, &mut menus.client);
        menus.window.remap(&mut queue, &mut menus.client);
        steps.step(Step::Closed);
    });
    let checked = client.serve(&mut display, &mut server, turn, |_, server, step| {
        check(server, &step);
        let (root, keyboard) = window(server);
        match step {
            Step::ParentUnmapped => {
                assert!(server.menu.is_none(), "unmapping the parent dismisses its menus");
                assert!(!keyboard.is_grabbed());
                assert_eq!(keyboard.current_focus(), None);
                assert!(!server.windows[0].mapped);
            }
            Step::Closed => {
                closed(server);
                assert_eq!(keyboard.current_focus(), Some(root), "the window shown again has the keyboard again");
            }
            _ => {}
        }
    });
    assert_eq!(checked, 4);
}
