//! Desktop input reaches either the existing UI targets or Smithay's seat.

use crate::server::Server;
use clamshell_state::{Event as StateEvent, WindowEvent, device::buttons::DeviceKey};
use clamshell_ui::{End, Screen, Source};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

impl Server {
    pub fn input<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.input::<(), _>(self, event.key_code(), event.state(), SERIAL_COUNTER.next_serial(), event.time(), |server, _, key| {
                        let sym = key.modified_sym();
                        let down = event.state() == KeyState::Pressed;
                        let action = match sym.raw() {
                            0xffc9 => Some(StateEvent::Home),
                            0xffc8 => Some(StateEvent::Device(DeviceKey::WindowDown)),
                            0xffc7 => Some(StateEvent::Device(DeviceKey::WindowUp)),
                            _ => None,
                        };
                        if let Some(action) = action {
                            if down {
                                server.core.handle(action);
                            }
                            FilterResult::Intercept(())
                        } else if server.core.state().session().keyboard_focus().is_none() {
                            server.core.key(sym, down);
                            FilterResult::Intercept(())
                        } else {
                            FilterResult::Forward
                        }
                    });
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.pointer = event.position_transformed(self.size.into());
                let pos = self.pointer;
                let (screen, position) = self.ui_position();
                self.core.moved(Source::Mouse, position, event.time().millis());
                let _ = screen;
                if let Some(pointer) = self.seat.get_pointer() {
                    let focus = self.surface_under(pos);
                    pointer.motion(self, focus, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time: event.time() });
                    pointer.frame(self);
                }
            }
            InputEvent::PointerButton { event, .. } => {
                let focus = self.surface_under(self.pointer);
                if event.state() == ButtonState::Pressed {
                    match &focus {
                        Some((surface, _)) => {
                            if let Some(id) = self.surface_id(surface) {
                                self.core.handle(WindowEvent::Activate(id).into());
                            }
                        }
                        None => {
                            self.core.handle(WindowEvent::KeyboardFocus(None).into());
                            if event.button_code() == 0x110 {
                                let (screen, position) = self.ui_position();
                                self.core.press(Source::Mouse, screen, position, event.time().millis());
                            }
                        }
                    }
                    self.sync_windows();
                } else if event.button_code() == 0x110 {
                    let at = self.core.at(event.time().millis());
                    self.core.up(Source::Mouse, End::Release, at);
                }
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.button(
                        self,
                        &ButtonEvent { button: event.button_code(), state: event.state(), serial: SERIAL_COUNTER.next_serial(), time: event.time() },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerAxis { event, .. } => {
                let vertical = event.amount(Axis::Vertical).unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or_default() / 8.0);
                if self.surface_under(self.pointer).is_some() {
                    if let Some(pointer) = self.seat.get_pointer() {
                        let frame = AxisFrame::new(event.time()).source(event.source()).value(Axis::Vertical, vertical);
                        pointer.axis(self, frame);
                        pointer.frame(self);
                    }
                } else {
                    let (screen, position) = self.ui_position();
                    self.core.wheel(screen, position, vertical.signum() as i32);
                }
            }
            _ => {}
        }
    }

    fn ui_position(&self) -> (Screen, (f64, f64)) {
        if self.pointer.x < self.top_width as f64 {
            (Screen::Top, (self.pointer.x, self.pointer.y))
        } else {
            (Screen::Bottom, (self.pointer.x - self.top_width as f64, self.pointer.y))
        }
    }
}
