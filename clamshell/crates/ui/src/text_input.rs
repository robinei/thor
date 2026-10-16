//! Whether an app's text field wants typing, from the compositor's input-method protocol (sway relays
//! text-input-v3, which GTK, Qt, Firefox and foot use): ClamShell registers as the seat's input method
//! and only listens; the keys still go through the keyboard's virtual device. A field that is let go is
//! reported after a short wait, so moving between fields doesn't bounce the keyboard.

use std::time::Duration;

use clamshell_core::Event;
use smithay_client_toolkit::{
    dispatch2::Dispatch2,
    reexports::{
        calloop::RegistrationToken,
        calloop::timer::{TimeoutAction, Timer},
        client::{Connection, QueueHandle, globals::GlobalList, protocol::wl_seat::WlSeat},
        protocols_misc::zwp_input_method_v2::client::{
            zwp_input_method_manager_v2::{self, ZwpInputMethodManagerV2},
            zwp_input_method_v2::{self, ZwpInputMethodV2},
        },
    },
};

use crate::App;

/// How long a field must stay let go before the keyboard goes back.
const SETTLE: Duration = Duration::from_millis(400);

/// What the input method said, once a batch of its events is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// A field wants typing (true) or was let go (false).
    Active(bool),
    /// Another input method has the seat: any field is let go at once.
    Gone,
}

/// The input method and what it last said.
#[derive(Default)]
pub(crate) struct TextInput {
    manager: Option<ZwpInputMethodManagerV2>,
    method: Option<ZwpInputMethodV2>,
    /// Activate or deactivate since the last `done`.
    pending: Option<bool>,
    /// What the state was last told.
    active: bool,
    settle: Option<RegistrationToken>,
}

impl TextInput {
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> TextInput {
        let manager = globals.bind::<ZwpInputMethodManagerV2, App, Data>(qh, 1..=1, Data).map_err(|e| log::info!("no input method protocol: {e}")).ok();
        TextInput { manager, ..TextInput::default() }
    }

    /// Becomes the input method of `seat` (the first one).
    pub(crate) fn seat(&mut self, seat: &WlSeat, qh: &QueueHandle<App>) {
        if let (Some(manager), None) = (&self.manager, &self.method) {
            self.method = Some(manager.get_input_method(seat, qh, Data));
        }
    }

    /// The input method's `event`; what it means once it is complete.
    fn event(&mut self, method: &ZwpInputMethodV2, event: zwp_input_method_v2::Event) -> Option<Field> {
        match event {
            zwp_input_method_v2::Event::Activate => self.pending = Some(true),
            zwp_input_method_v2::Event::Deactivate => self.pending = Some(false),
            zwp_input_method_v2::Event::Done => return self.pending.take().map(Field::Active),
            // Another input method has the seat (fcitx, ibus on a desktop): do without.
            zwp_input_method_v2::Event::Unavailable => {
                log::info!("another input method is running: the keyboard won't come up for text fields");
                method.destroy();
                self.method = None;
                self.pending = None;
                return Some(Field::Gone);
            }
            _ => {}
        }
        None
    }

    /// Records what the state is told; whether that is news.
    fn tell(&mut self, active: bool) -> bool {
        std::mem::replace(&mut self.active, active) != active
    }
}

/// The objects' user data (they carry nothing).
pub(crate) struct Data;

impl Dispatch2<ZwpInputMethodManagerV2, App> for Data {
    fn event(&self, _: &mut App, _: &ZwpInputMethodManagerV2, _: zwp_input_method_manager_v2::Event, _: &Connection, _: &QueueHandle<App>) {}
}

impl Dispatch2<ZwpInputMethodV2, App> for Data {
    fn event(&self, app: &mut App, method: &ZwpInputMethodV2, event: zwp_input_method_v2::Event, _: &Connection, _: &QueueHandle<App>) {
        if let Some(field) = app.text_input.event(method, event) {
            app.text_field(field);
        }
    }
}

impl App {
    /// Tells the state about a text field: at once when one wants typing (or the input method is gone),
    /// after `SETTLE` when it was let go.
    fn text_field(&mut self, field: Field) {
        if let Some(token) = self.text_input.settle.take() {
            self.loop_handle.remove(token);
        }
        match field {
            Field::Active(true) | Field::Gone => {
                let active = field == Field::Active(true);
                if self.text_input.tell(active) {
                    self.handle(Event::TextInput(active));
                }
            }
            Field::Active(false) if self.text_input.active => {
                let timer = Timer::from_duration(SETTLE);
                let settled = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                    app.text_input.settle = None;
                    if app.text_input.tell(false) {
                        app.handle(Event::TextInput(false));
                    }
                    TimeoutAction::Drop
                });
                match settled {
                    Ok(token) => self.text_input.settle = Some(token),
                    Err(e) => log::warn!("text input timer: {e}"),
                }
            }
            Field::Active(false) => {}
        }
    }
}
