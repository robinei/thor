//! The on-screen keyboard's keys, sent to the focused client: a character as text input when the client
//! has a text field active, otherwise as a key through the seat's keymap, with its modifiers, from a
//! source of its own, so no virtual input device (uinput) is needed. The state is told when a client's
//! text field becomes active or stops being (`Event::TextInput`).
//!
//! A key is held as on a real keyboard: down with its modifiers when the state sends it, up when the
//! state says it was let go, but no sooner than [`KEY_HELD_AT_LEAST`] after it went down.

use crate::server::Server;
use clamshell_state::{
    Event,
    bottom::keyboard::{Key, Named, Stroke, shifted},
};
use smithay::{
    backend::input::{InputTime, KeyState},
    input::keyboard::{FilterResult, Keycode, Keysym, keysyms, xkb},
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

use clamshell_state::bottom::keyboard::KEY_HELD_AT_LEAST;
use std::time::Instant;

/// An on-screen key held down, with the modifiers it was pressed with.
#[derive(Debug)]
pub(crate) struct HeldKey {
    code: Keycode,
    modifiers: [Option<Keycode>; 4],
    down: Instant,
    /// When it goes up, once the state said it was let go.
    up: Option<Instant>,
}

impl HeldKey {
    /// When the loop has to wake to let the key go.
    pub(crate) fn release_at(&self) -> Option<Instant> {
        self.up
    }
}

impl Server {
    /// A key of the on-screen keyboard goes down, and is held (see [`HeldKey`]). The same key again while
    /// held is the state repeating it, which the client does itself. Pressed again while its release waits,
    /// it goes up first.
    pub(crate) fn stroke(&mut self, stroke: &Stroke, now: Instant) {
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
        let Some(code) = self.code(stroke.key) else { return };
        let modifiers = [
            (stroke.ctrl, keysyms::KEY_Control_L),
            (stroke.alt, keysyms::KEY_Alt_L),
            (stroke.logo, keysyms::KEY_Super_L),
            (stroke.shift, keysyms::KEY_Shift_L),
        ]
        .map(|(on, sym)| on.then(|| keyboard.keycode_for_keysym(Keysym::new(sym))).flatten());
        if self.held_keys.iter().flatten().any(|held| held.code == code && held.up.is_none()) {
            return;
        }
        // The same key waiting to go up, or, with every slot taken, the one held longest.
        let slot = self
            .held_keys
            .iter()
            .position(|held| held.as_ref().is_some_and(|held| held.code == code))
            .or_else(|| self.held_keys.iter().position(Option::is_none));
        let slot = slot.unwrap_or_else(|| {
            let oldest = self.held_keys.iter().enumerate().filter_map(|(i, held)| held.as_ref().map(|held| (i, held.down))).min_by_key(|&(_, down)| down);
            oldest.map_or(0, |(i, _)| i)
        });
        self.let_go_of(slot);
        for code in modifiers.into_iter().flatten().chain([code]) {
            self.send_key(code, KeyState::Pressed);
        }
        if let Some(held) = self.held_keys.get_mut(slot) {
            *held = Some(HeldKey { code, modifiers, down: now, up: None });
        }
    }

    /// `key` was let go on the keyboard: it goes up now, or once it has been held [`KEY_HELD_AT_LEAST`].
    pub(crate) fn key_up(&mut self, key: Key, now: Instant) {
        let Some(code) = self.code(key) else { return };
        if let Some(held) = self.held_keys.iter_mut().flatten().find(|held| held.code == code && held.up.is_none()) {
            held.up = Some(now.max(held.down + KEY_HELD_AT_LEAST));
        }
        self.release_keys(now);
    }

    /// Lets go of the keys whose time to go up has come.
    pub(crate) fn release_keys(&mut self, now: Instant) {
        for slot in 0..self.held_keys.len() {
            if self.held_keys.get(slot).and_then(Option::as_ref).and_then(HeldKey::release_at).is_some_and(|at| at <= now) {
                self.let_go_of(slot);
            }
        }
    }

    /// The seat's key for `key`, if the keymap has one.
    fn code(&self, key: Key) -> Option<Keycode> {
        let sym = keysym(key)?;
        let code = self.seat.get_keyboard()?.keycode_for_keysym(sym);
        if code.is_none() {
            log::warn!("compositor: no key in the keymap for on-screen key {key:?}: not typed");
        }
        code
    }

    /// Sends the release of the key held in `slot`, then of its modifiers.
    fn let_go_of(&mut self, slot: usize) {
        let Some(held) = self.held_keys.get_mut(slot).and_then(Option::take) else { return };
        for code in [Some(held.code)].into_iter().chain(held.modifiers.into_iter().rev()).flatten() {
            self.send_key(code, KeyState::Released);
        }
    }

    /// One key event from the on-screen keyboard's source, past the compositor's filter.
    fn send_key(&mut self, code: Keycode, state: KeyState) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        keyboard.input_from_source::<(), _>(self.keyboard_source, self, code, state, SERIAL_COUNTER.next_serial(), InputTime::now(), |_, _, _| {
            FilterResult::Forward
        });
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
