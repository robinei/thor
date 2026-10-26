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

/// A client's window with the keyboard, and a text field (text-input).
struct Typing {
    text: zwp_text_input_v3::ZwpTextInputV3,
    _window: Toplevel,
    client: Client,
}

impl Typing {
    /// Opens the window, which the text field enters.
    fn new(globals: wayland_client::globals::GlobalList, queue: &mut EventQueue<Client>) -> Typing {
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _keyboard = seat.get_keyboard(&qh, ());
        let manager: zwp_text_input_manager_v3::ZwpTextInputManagerV3 = globals.bind(&qh, 1..=1, ()).unwrap();
        let text = manager.get_text_input(&seat, &qh, ());
        let mut client = Client::default();
        let window = open_toplevel(&globals, queue, &mut client, "typing", Some((960, 540)));
        assert_eq!(client.text, vec![TextRecord::Enter]);
        client.text.clear();
        Typing { text, _window: window, client }
    }

    /// Activates the text field, or (`active` false) deactivates it.
    fn text_field(&mut self, queue: &mut EventQueue<Client>, active: bool) {
        if active {
            self.text.enable();
        } else {
            self.text.disable();
        }
        self.text.commit();
        queue.roundtrip(&mut self.client).unwrap();
    }
}

#[test]
fn the_bottom_keyboards_strokes_reach_the_focused_client_as_keys() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut typing = Typing::new(globals, &mut queue);
        steps.step(());
        queue.roundtrip(&mut typing.client).unwrap();
        // The modifier order makes the shortcut apply only to its key. F12 injection reaches the
        // app rather than triggering nested Home. A physical Shift survives an OSK Shift stroke.
        assert_eq!(
            typing.client.keys,
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
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ()| {
        server.stroke(&stroke(Key::Char('a')));
        server.stroke(&Stroke { ctrl: true, shift: true, ..stroke(Key::Char('t')) });
        server.stroke(&stroke(Key::Named(Named::F(12))));
        let keyboard = server.seat.get_keyboard().unwrap();
        keyboard.input::<(), _>(server, Keycode::new(50), KeyState::Pressed, SERIAL_COUNTER.next_serial(), InputTime::now(), |_, _, _| FilterResult::Forward);
        server.stroke(&Stroke { shift: true, ..stroke(Key::Char('a')) });
        assert!(keyboard.modifier_state().shift, "OSK release must leave physical Shift held");
        keyboard.input::<(), _>(server, Keycode::new(50), KeyState::Released, SERIAL_COUNTER.next_serial(), InputTime::now(), |_, _, _| FilterResult::Forward);
        server.stroke(&stroke(Key::Named(Named::F(13))));
        assert!(!server.core.state().session().home_held());
    });
    assert_eq!(checked, 1);
}

#[test]
fn a_text_field_opens_the_keyboard_and_takes_its_characters_as_text() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut typing = Typing::new(globals, &mut queue);
        typing.text_field(&mut queue, true);
        steps.step(());
        queue.roundtrip(&mut typing.client).unwrap();
        assert_eq!(typing.client.text, vec![TextRecord::Commit("!".into()), TextRecord::Done(1), TextRecord::Commit("a".into()), TextRecord::Done(1)]);
        assert_eq!(typing.client.keys, vec![(29, true), (46, true), (46, false), (29, false), (14, true), (14, false)]);
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ()| {
        assert_eq!(server.core.state().bottom().tab(), Tab::Keyboard);
        let focus = server.core.state().session().keyboard_focus();
        server.touch_down(Some(0).into(), (server.screens.top_width as f64 + 10.0, 10.0).into(), InputTime::now());
        server.touch_cancel();
        assert_eq!(server.core.state().session().keyboard_focus(), focus);
        assert!(server.seat.get_keyboard().unwrap().is_focused());
        server.stroke(&Stroke { shift: true, ..stroke(Key::Char('1')) });
        let pos = keyboard::find(Key::Char('a')).unwrap();
        server.core.handle(PointerEvent::Activate(Id::Key(pos)).into());
        server.frame();
        server.stroke(&Stroke { ctrl: true, ..stroke(Key::Char('c')) });
        server.stroke(&stroke(Key::Named(Named::Backspace)));
    });
    assert_eq!(checked, 1);
}

#[test]
fn a_text_field_deactivated_closes_the_keyboard_and_characters_are_keys_again() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let original_tab = server.core.state().bottom().tab();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut typing = Typing::new(globals, &mut queue);
        typing.text_field(&mut queue, true);
        steps.step(false);
        typing.text_field(&mut queue, false);
        steps.step(true);
        queue.roundtrip(&mut typing.client).unwrap();
        assert_eq!(typing.client.keys, vec![(48, true), (48, false)]);
        assert!(typing.client.text.is_empty());
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, deactivated| {
        if !deactivated {
            assert_eq!(server.core.state().bottom().tab(), Tab::Keyboard);
            return;
        }
        assert_eq!(server.core.state().bottom().tab(), original_tab);
        server.stroke(&stroke(Key::Char('b')));
    });
    assert_eq!(checked, 2);
}

#[test]
fn home_takes_the_keyboard_from_a_text_field_and_closes_the_keyboard() {
    let (mut display, mut server) = ServerBuilder::new().started();
    let original_tab = server.core.state().bottom().tab();
    let client = spawn_client(&mut display, ClientState::default(), move |globals, mut queue, steps| {
        let mut typing = Typing::new(globals, &mut queue);
        typing.text_field(&mut queue, true);
        steps.step(());
        queue.roundtrip(&mut typing.client).unwrap();
        assert_eq!(typing.client.text, vec![TextRecord::Leave]);
        assert!(typing.client.keys.is_empty(), "an unfocused client receives no on-screen strokes");
    });
    let checked = client.serve(&mut display, &mut server, Server::frame, |_, server, ()| {
        assert_eq!(server.core.state().bottom().tab(), Tab::Keyboard);
        server.core.handle(Event::Home);
        server.frame();
        assert_eq!(server.core.state().bottom().tab(), original_tab);
        server.stroke(&stroke(Key::Char('c')));
    });
    assert_eq!(checked, 1);
}
