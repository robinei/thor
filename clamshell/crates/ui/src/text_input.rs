//! Whether an app's text field wants typing, from the compositor's input-method protocol (sway relays
//! text-input-v3, which GTK, Qt, Firefox and foot use): ClamShell registers as the seat's input method
//! and only listens; the keys still go through the keyboard's virtual device. A field that is let go is
//! reported after a short wait, so moving between fields doesn't bounce the keyboard.

use std::time::Duration;

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
use clamshell_core::Event;

use crate::App;

/// How long a field must stay let go before the keyboard goes back.
const SETTLE: Duration = Duration::from_millis(400);

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
}

/// The objects' user data (they carry nothing).
pub(crate) struct Data;

impl Dispatch2<ZwpInputMethodManagerV2, App> for Data {
    fn event(&self, _: &mut App, _: &ZwpInputMethodManagerV2, _: zwp_input_method_manager_v2::Event, _: &Connection, _: &QueueHandle<App>) {}
}

impl Dispatch2<ZwpInputMethodV2, App> for Data {
    fn event(&self, app: &mut App, method: &ZwpInputMethodV2, event: zwp_input_method_v2::Event, _: &Connection, _: &QueueHandle<App>) {
        let t = &mut app.text_input;
        match event {
            zwp_input_method_v2::Event::Activate => t.pending = Some(true),
            zwp_input_method_v2::Event::Deactivate => t.pending = Some(false),
            zwp_input_method_v2::Event::Done => {
                if let Some(active) = t.pending.take() {
                    app.text_field(active);
                }
            }
            // Another input method has the seat (fcitx, ibus on a desktop): do without.
            // (A field it had is let go of: the keyboard goes back.)
            zwp_input_method_v2::Event::Unavailable => {
                log::info!("another input method is running: the keyboard won't come up for text fields");
                method.destroy();
                t.method = None;
                t.pending = None;
                if let Some(token) = t.settle.take() {
                    app.loop_handle.remove(token);
                }
                if std::mem::take(&mut app.text_input.active) {
                    app.handle(Event::TextInput(false));
                }
            }
            _ => {}
        }
    }
}

impl App {
    fn text_field(&mut self, active: bool) {
        if let Some(token) = self.text_input.settle.take() {
            self.loop_handle.remove(token);
        }
        if active == self.text_input.active {
            return;
        }
        if active {
            self.text_input.active = true;
            self.handle(Event::TextInput(true));
            return;
        }
        let timer = Timer::from_duration(SETTLE);
        let settled = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.text_input.settle = None;
            app.text_input.active = false;
            app.handle(Event::TextInput(false));
            TimeoutAction::Drop
        });
        match settled {
            Ok(token) => self.text_input.settle = Some(token),
            Err(e) => log::warn!("text input timer: {e}"),
        }
    }
}
