//! Desktop input reaches either the existing UI targets or Smithay's seat.

use crate::server::Server;
use clamshell_state::{Event as StateEvent, WindowEvent, device::buttons::DeviceKey};
use clamshell_ui::{End, Screen, Source};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, TouchEvent,
    },
    backend::winit::WinitInput,
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

pub(crate) struct Hit {
    pub window: i64,
    pub surface: WlSurface,
    pub location: Point<f64, Logical>,
    pub origin: Point<f64, Logical>,
}

impl Server {
    pub fn input(&mut self, event: InputEvent<WinitInput>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.input::<(), _>(self, event.key_code(), event.state(), SERIAL_COUNTER.next_serial(), event.time(), |server, _, key| {
                        let sym = key.modified_sym();
                        let down = event.state() == KeyState::Pressed;
                        if sym.raw() == 0xffc9 {
                            if down != server.keyboard_home {
                                server.keyboard_home = down;
                                server.core.handle(StateEvent::HomeHeld(down));
                                if !down {
                                    server.core.handle(StateEvent::HomeTap);
                                }
                            }
                            return FilterResult::Intercept(());
                        }
                        let action = match sym.raw() {
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
                let (_, position) = self.ui_position();
                self.core.moved(Source::Mouse, position, event.time().millis());
                self.move_pointer(event.time());
            }
            InputEvent::PointerButton { event, .. } => {
                if event.button_code() == 0x110 && self.arrow_pressed {
                    if event.state() == ButtonState::Released {
                        self.arrow_pressed = false;
                    }
                    return;
                }
                if event.state() == ButtonState::Pressed {
                    if self.pointer_buttons.is_empty() {
                        let arrows = clamshell_ui::window_arrows::WindowArrows::new(self.core.state(), (self.top_width as u32, self.size.1 as u32));
                        if event.button_code() == 0x110
                            && let Some(action) = arrows.hit((self.pointer.x, self.pointer.y))
                        {
                            self.arrow_pressed = true;
                            self.core.handle(StateEvent::Device(action));
                            self.sync_windows();
                            return;
                        }
                        let hit = self.hit(self.pointer);
                        self.pointer_window = hit.as_ref().map(|hit| hit.window);
                        if let Some(hit) = hit {
                            self.core.handle(WindowEvent::Activate(hit.window).into());
                        } else if let Some((screen, position)) = self.ui_under() {
                            self.core.handle(WindowEvent::KeyboardFocus(None).into());
                            if event.button_code() == 0x110 {
                                self.core.press(Source::Mouse, screen, position, event.time().millis());
                            }
                        }
                    }
                    if !self.pointer_buttons.contains(&event.button_code()) {
                        self.pointer_buttons.push(event.button_code());
                    }
                    self.sync_windows();
                } else if event.button_code() == 0x110 {
                    let at = self.core.at(event.time().millis());
                    self.core.up(Source::Mouse, End::Release, at);
                }
                self.move_pointer(event.time());
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.button(
                        self,
                        &ButtonEvent { button: event.button_code(), state: event.state(), serial: SERIAL_COUNTER.next_serial(), time: event.time() },
                    );
                    pointer.frame(self);
                }
                if event.state() == ButtonState::Released {
                    self.pointer_buttons.retain(|code| *code != event.button_code());
                    if self.pointer_buttons.is_empty() {
                        self.pointer_window = None;
                    }
                }
            }

            InputEvent::PointerAxis { event, .. } => {
                let vertical = event.amount(Axis::Vertical).unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or_default() / 8.0);
                if self.hit(self.pointer).is_some() {
                    if let Some(pointer) = self.seat.get_pointer() {
                        let frame = AxisFrame::new(event.time()).source(event.source()).value(Axis::Vertical, vertical);
                        pointer.axis(self, frame);
                        pointer.frame(self);
                    }
                } else if let Some((screen, position)) = self.ui_under() {
                    self.core.wheel(screen, position, vertical.signum() as i32);
                }
            }
            // Winit gives physical window coordinates and no touch frame events. These are the
            // same coordinates as our nested framebuffer; finish each host event as one frame.
            InputEvent::TouchDown { event } => {
                self.touch_down(event.slot(), (event.x(), event.y()).into(), event.time());
                self.touch_frame();
            }
            InputEvent::TouchMotion { event } => {
                self.touch_motion(event.slot(), (event.x(), event.y()).into(), event.time());
                self.touch_frame();
            }
            InputEvent::TouchUp { event } => {
                self.touch_up(event.slot(), event.time());
                self.touch_frame();
            }
            InputEvent::TouchCancel { .. } => self.touch_cancel(),
            InputEvent::TouchFrame { .. } => self.touch_frame(),
            _ => {}
        }
    }

    fn move_pointer(&mut self, time: smithay::backend::input::InputTime) {
        let hit = self.hit(self.pointer);
        let location = self
            .pointer_window
            .filter(|_| !self.pointer_buttons.is_empty())
            .and_then(|id| self.geometry(id))
            .map(|geometry| geometry.surface_coords(self.pointer))
            .or_else(|| hit.as_ref().map(|hit| hit.location))
            .unwrap_or(self.pointer);
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(self, hit.map(|hit| (hit.surface, hit.origin)), &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time });
            pointer.frame(self);
        }
    }

    fn ui_under(&self) -> Option<(Screen, (f64, f64))> {
        self.ui_at(self.pointer)
    }

    pub(crate) fn ui_at(&self, point: Point<f64, Logical>) -> Option<(Screen, (f64, f64))> {
        let (screen, position) = self.screen_position(point);
        let session = self.core.state().session();
        match screen {
            Screen::Top if session.top_window().is_some() => None,
            Screen::Bottom if self.core.state().bottom().shows_windows(session) && point.y >= self.area(true).loc.y as f64 => None,
            _ => Some((screen, position)),
        }
    }

    fn ui_position(&self) -> (Screen, (f64, f64)) {
        self.screen_position(self.pointer)
    }

    fn screen_position(&self, point: Point<f64, Logical>) -> (Screen, (f64, f64)) {
        if point.x < self.top_width as f64 { (Screen::Top, (point.x, point.y)) } else { (Screen::Bottom, (point.x - self.top_width as f64, point.y)) }
    }
}
