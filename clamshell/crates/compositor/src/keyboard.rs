//! The on-screen keyboard's keys, sent to the focused client: a character as text input when the client
//! has a text field active, otherwise as a key through the seat's keymap, with its modifiers, from a
//! source of its own, so no virtual input device (uinput) is needed. The state is told when a client's
//! text field becomes active or stops being (`Event::TextInput`).

use crate::server::Server;
use clamshell_state::{
    Event,
    bottom::keyboard::{Key, Named, Stroke, shifted},
};
use smithay::{
    backend::input::{InputTime, KeyState},
    input::keyboard::{FilterResult, Keysym, keysyms, xkb},
    utils::SERIAL_COUNTER,
    wayland::text_input::{TextInputActivation, TextInputSeat},
};

impl TextInputActivation for Server {
    fn activated(
        &mut self,
        _: Option<(
            smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ContentHint,
            smithay::reexports::wayland_protocols::wp::text_input::zv3::server::zwp_text_input_v3::ContentPurpose,
        )>,
    ) {
        self.core.handle(Event::TextInput(true));
    }
    fn deactivated(&mut self) {
        self.core.handle(Event::TextInput(false));
    }
}

impl Server {
    pub(crate) fn stroke(&mut self, stroke: &Stroke) {
        let Some(keyboard) = self.seat.get_keyboard().filter(|keyboard| keyboard.is_focused()) else { return };
        if let Key::Char(character) = stroke.key
            && !stroke.ctrl
            && !stroke.alt
            && !stroke.logo
        {
            let character = if stroke.shift { shifted(character) } else { character };
            let mut buffer = [0; 4];
            let text = character.encode_utf8(&mut buffer);
            let handle = self.seat.text_input().clone();
            let mut committed = false;
            handle.with_active_text_input(|input, _| {
                input.commit_string(Some(text.to_owned()));
                committed = true;
            });
            if committed {
                handle.done(false);
                return;
            }
        }
        let Some(sym) = keysym(stroke.key) else { return };
        let Some(code) = keyboard.keycode_for_keysym(sym) else {
            log::warn!("compositor: no key in the keymap for on-screen stroke {:?}: not typed", stroke.key);
            return;
        };
        let modifiers = [
            (stroke.ctrl, keysyms::KEY_Control_L),
            (stroke.alt, keysyms::KEY_Alt_L),
            (stroke.logo, keysyms::KEY_Super_L),
            (stroke.shift, keysyms::KEY_Shift_L),
        ]
        .map(|(on, sym)| on.then(|| keyboard.keycode_for_keysym(Keysym::new(sym))).flatten());
        let time = InputTime::now();
        let mut send = |code, state| {
            keyboard.input_from_source::<(), _>(self.keyboard_source, self, code, state, SERIAL_COUNTER.next_serial(), time, |_, _, _| FilterResult::Forward);
        };
        for code in modifiers.into_iter().flatten() {
            send(code, KeyState::Pressed);
        }
        send(code, KeyState::Pressed);
        send(code, KeyState::Released);
        for code in modifiers.into_iter().rev().flatten() {
            send(code, KeyState::Released);
        }
    }
}

fn keysym(key: Key) -> Option<Keysym> {
    Some(Keysym::new(match key {
        Key::Char(character) => return Some(xkb::utf32_to_keysym(character as u32)),
        Key::Named(name) => match name {
            Named::Backspace => keysyms::KEY_BackSpace,
            Named::Tab => keysyms::KEY_Tab,
            Named::Enter => keysyms::KEY_Return,
            Named::Esc => keysyms::KEY_Escape,
            Named::Delete => keysyms::KEY_Delete,
            Named::Insert => keysyms::KEY_Insert,
            Named::Home => keysyms::KEY_Home,
            Named::End => keysyms::KEY_End,
            Named::PageUp => keysyms::KEY_Page_Up,
            Named::PageDown => keysyms::KEY_Page_Down,
            Named::Left => keysyms::KEY_Left,
            Named::Right => keysyms::KEY_Right,
            Named::Up => keysyms::KEY_Up,
            Named::Down => keysyms::KEY_Down,
            Named::F(n @ 1..=12) => keysyms::KEY_F1 + u32::from(n) - 1,
            Named::F(_) => return None,
        },
        Key::Mod(_) | Key::Caps => return None,
    }))
}
