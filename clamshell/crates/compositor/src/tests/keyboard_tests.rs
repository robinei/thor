//! Seat keys and text-input traffic from real clients, without uinput or a GPU.

use super::*;
use clamshell_state::{
    Id, PointerEvent, Tab,
    bottom::keyboard::{self, Key, Named, Stroke},
};
use smithay::{
    backend::input::{InputTime, KeyState},
    input::keyboard::{FilterResult, Keycode},
    reexports::wayland_protocols::wp::text_input::zv3::client::{zwp_text_input_manager_v3, zwp_text_input_v3},
    utils::SERIAL_COUNTER,
};
use wayland_client::{WEnum, protocol::wl_keyboard};

#[derive(Debug, PartialEq)]
pub(super) enum TextRecord {
    Enter,
    Leave,
    Commit(String),
    Done(u32),
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Enter { serial, .. } = event {
            state.keyboard_enter = serial;
        }
        if let wl_keyboard::Event::Key { key, state: value, .. } = event {
            state.keys.push((key, value == WEnum::Value(wl_keyboard::KeyState::Pressed)));
        }
    }
}
impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Client {
    fn event(state: &mut Self, _: &zwp_text_input_v3::ZwpTextInputV3, event: zwp_text_input_v3::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let record = match event {
            zwp_text_input_v3::Event::Enter { .. } => TextRecord::Enter,
            zwp_text_input_v3::Event::Leave { .. } => TextRecord::Leave,
            zwp_text_input_v3::Event::CommitString { text: Some(text) } => TextRecord::Commit(text),
            zwp_text_input_v3::Event::Done { serial } => TextRecord::Done(serial),
            _ => return,
        };
        state.text.push(record);
    }
}
delegate_noop!(Client: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);

fn stroke(key: Key) -> Stroke {
    Stroke { key, shift: false, ctrl: false, alt: false, logo: false }
}

#[test]
fn the_bottom_keyboard_commits_text_and_sends_shortcuts_without_stealing_focus() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let original_tab = server.core.state().bottom().tab();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let shell: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _keyboard = seat.get_keyboard(&qh, ());
        let manager: zwp_text_input_manager_v3::ZwpTextInputManagerV3 = globals.bind(&qh, 1..=1, ()).unwrap();
        let text = manager.get_text_input(&seat, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("typing".into());
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
        assert_eq!(client.text, vec![TextRecord::Enter]);
        client.text.clear();
        steps.step(0);
        queue.roundtrip(&mut client).unwrap();
        // The modifier order makes the shortcut apply only to its key. F12 injection reaches the
        // app rather than triggering nested Home. A physical Shift survives an OSK Shift stroke.
        assert_eq!(
            client.keys,
            vec![
                (30, true),
                (30, false),
                (29, true),
                (42, true),
                (20, true),
                (20, false),
                (42, false),
                (29, false),
                (88, true),
                (88, false),
                (42, true),
                (30, true),
                (30, false),
                (42, false)
            ]
        );
        client.keys.clear();
        text.enable();
        text.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(1);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.text, vec![TextRecord::Commit("!".into()), TextRecord::Done(1), TextRecord::Commit("a".into()), TextRecord::Done(1)]);
        assert_eq!(client.keys, vec![(29, true), (46, true), (46, false), (29, false), (14, true), (14, false)]);
        client.text.clear();
        client.keys.clear();
        text.disable();
        text.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(2);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.keys, vec![(48, true), (48, false)]);
        assert!(client.text.is_empty());
        client.keys.clear();
        text.enable();
        text.commit();
        queue.roundtrip(&mut client).unwrap();
        steps.step(3);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.text, vec![TextRecord::Leave]);
        assert!(client.keys.is_empty(), "an unfocused client receives no on-screen strokes");
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, phase| match phase {
        0 => {
            server.stroke(&stroke(Key::Char('a')));
            server.stroke(&Stroke { ctrl: true, shift: true, ..stroke(Key::Char('t')) });
            server.stroke(&stroke(Key::Named(Named::F(12))));
            let keyboard = server.seat.get_keyboard().unwrap();
            keyboard
                .input::<(), _>(server, Keycode::new(50), KeyState::Pressed, SERIAL_COUNTER.next_serial(), InputTime::now(), |_, _, _| FilterResult::Forward);
            server.stroke(&Stroke { shift: true, ..stroke(Key::Char('a')) });
            assert!(keyboard.modifier_state().shift, "OSK release must leave physical Shift held");
            keyboard
                .input::<(), _>(server, Keycode::new(50), KeyState::Released, SERIAL_COUNTER.next_serial(), InputTime::now(), |_, _, _| FilterResult::Forward);
            server.stroke(&stroke(Key::Named(Named::F(13))));
            assert!(!server.core.state().session().home_held());
        }
        1 => {
            assert_eq!(server.core.state().bottom().tab(), Tab::Keyboard);
            let focus = server.core.state().session().keyboard_focus();
            server.touch_down(Some(0).into(), (server.top_width as f64 + 10.0, 10.0).into(), InputTime::now());
            server.touch_cancel();
            assert_eq!(server.core.state().session().keyboard_focus(), focus);
            assert!(server.seat.get_keyboard().unwrap().is_focused());
            server.stroke(&Stroke { shift: true, ..stroke(Key::Char('1')) });
            let pos = keyboard::find(Key::Char('a')).unwrap();
            server.core.handle(PointerEvent::Activate(Id::Key(pos)).into());
            server.frame();
            server.stroke(&Stroke { ctrl: true, ..stroke(Key::Char('c')) });
            server.stroke(&stroke(Key::Named(Named::Backspace)));
        }
        2 => {
            assert_eq!(server.core.state().bottom().tab(), original_tab);
            server.stroke(&stroke(Key::Char('b')));
        }
        3 => {
            assert_eq!(server.core.state().bottom().tab(), Tab::Keyboard);
            server.core.handle(Event::Home);
            server.frame();
            assert_eq!(server.core.state().bottom().tab(), original_tab);
            server.stroke(&stroke(Key::Char('c')));
        }
        _ => panic!("unexpected phase"),
    });
    assert_eq!(checked, 4);
}
