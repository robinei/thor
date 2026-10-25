//! Desktop input reaches either the existing UI targets or Smithay's seat.

use crate::server::Server;
use clamshell_state::{Event as StateEvent, WindowEvent, device::buttons::DeviceKey};
use clamshell_ui::{End, Screen, Source};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent, TouchEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};

/// How a host delivers input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Host {
    /// A desktop window: absolute motion places the free cursor, and relative motion only reaches
    /// clients. Otherwise (libinput's mice) relative motion moves the cursor.
    pub absolute_cursor: bool,
    /// Whether the host ends each touch frame; winit doesn't, so each of its touch events is one.
    pub touch_frames: bool,
}

pub(crate) struct Hit {
    pub window: i64,
    pub surface: WlSurface,
    pub location: Point<f64, Logical>,
    pub origin: Point<f64, Logical>,
}

impl Server {
    /// An input event from a host. `area` is where an absolute device (a touchscreen, a tablet, the
    /// nested window) lies in the scene.
    pub(crate) fn input<B: InputBackend>(&mut self, event: InputEvent<B>, host: Host, area: impl Fn(&B::Device) -> Rectangle<f64, Logical>) {
        self.idle.input(std::time::Instant::now());
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
                        // XF86Switch_VT_1 to _12: the keymap gives these for Ctrl+Alt+F1..F12.
                        if (0x1008_fe01..=0x1008_fe0c).contains(&sym.raw()) {
                            if down {
                                server.vt_switch = Some((sym.raw() - 0x1008_fe00) as i32);
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
            InputEvent::PointerMotion { event } => {
                self.validate_capture();
                let delta = PointerMotionEvent::<B>::delta(&event);
                let unaccelerated = PointerMotionEvent::<B>::delta_unaccel(&event);
                let time = Event::<B>::time(&event);
                // Winit can report absolute and relative events for one physical movement.
                // Its absolute event positions the free cursor; captured motion is relative-only.
                if self.capture.is_some() || !host.absolute_cursor {
                    self.pointer_relative(delta, unaccelerated, time);
                } else {
                    self.relative_motion(delta, unaccelerated, time);
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.validate_capture();
                if self.capture.is_none() {
                    let area = area(&event.device());
                    let at = event.position_transformed(area.size.to_i32_round());
                    self.pointer_absolute(area.loc + at, event.time());
                }
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
                        let hit = self.hit(self.pointer);
                        self.dismiss_menu_outside(hit.as_ref().map(|hit| hit.window));
                        self.dismiss_x11_menus_outside(hit.as_ref().map(|hit| &hit.surface));
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
                            if screen == Screen::Top {
                                self.core.handle(WindowEvent::KeyboardFocus(None).into());
                            }
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
            // Positions are within the device's area of the scene. Winit sends no frame events, so each
            // of its touch events ends a frame.
            InputEvent::TouchDown { event } => {
                let area = area(&event.device());
                self.touch_down(event.slot(), area.loc + event.position_transformed(area.size.to_i32_round()), event.time());
                if !host.touch_frames {
                    self.touch_frame();
                }
            }
            InputEvent::TouchMotion { event } => {
                let area = area(&event.device());
                self.touch_motion(event.slot(), area.loc + event.position_transformed(area.size.to_i32_round()), event.time());
                if !host.touch_frames {
                    self.touch_frame();
                }
            }
            InputEvent::TouchUp { event } => {
                self.touch_up(event.slot(), event.time());
                if !host.touch_frames {
                    self.touch_frame();
                }
            }
            InputEvent::TouchCancel { .. } => self.touch_cancel(),
            InputEvent::TouchFrame { .. } => self.touch_frame(),
            _ => {}
        }
    }

    pub(crate) fn pointer_absolute(&mut self, position: Point<f64, Logical>, time: smithay::backend::input::InputTime) {
        self.cursor_activity.motion(std::time::Instant::now());
        self.validate_capture();
        if self.pointer_locked() {
            return;
        }
        self.pointer = self.constrain_pointer(position);
        let (_, position) = self.ui_position();
        self.core.moved(Source::Mouse, position, time.millis());
        self.move_pointer(time);
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
