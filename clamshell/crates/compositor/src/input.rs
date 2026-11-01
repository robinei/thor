//! Input from a host (the desktop window's events, or libinput's on the device): keys, the pointer, touch.
//! A key goes to ClamShell's view while no window has the keyboard, else to the window. Ctrl+Alt+F1..F12
//! switch the VT whatever has the keyboard; on a desktop host, where its keyboard is all there is, F12
//! stands for the Home button and F11 and F10 move a window down to the second screen and back. (On the
//! device, Home is the controller's, and F10 to F12 reach the window like other keys.) A click goes to the window shown under it, or to the view's
//! target there. Touch continues in `touch`, the pointer's capture in `pointer`.

use crate::server::Server;
use clamshell_state::{Event as StateEvent, HardwareKey, WindowEvent};
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
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Transform},
};
use xkeysym::Keysym;

/// How a host delivers input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Host {
    /// A desktop window: absolute motion places the free cursor, and relative motion only reaches
    /// clients. Otherwise (libinput's mice) relative motion moves the cursor.
    pub absolute_cursor: bool,
    /// Whether the host ends each touch frame; winit doesn't, so each of its touch events is one.
    pub touch_frames: bool,
    /// A desktop's keyboard standing in for the device's buttons: F12 as Home, F11 and F10 a window down
    /// and up.
    pub desktop_keys: bool,
}

pub(crate) struct Hit {
    pub window: i64,
    pub surface: WlSurface,
    pub location: Point<f64, Logical>,
    pub origin: Point<f64, Logical>,
}

/// Where an absolute device's position lies in the scene. The device reports across its panel as the
/// panel is mounted (a portrait panel turned in the case, `turn` the output's transform), so the position
/// is turned back to the screen's upright area.
fn placed<B: InputBackend>(event: &impl AbsolutePositionEvent<B>, (area, turn): (Rectangle<f64, Logical>, Transform)) -> Point<f64, Logical> {
    let panel = turn.transform_size(area.size);
    upright(event.position_transformed(panel.to_i32_round()), area, turn)
}

/// A position across a panel turned `turn`, in the scene's upright `area`.
fn upright(on_panel: Point<f64, Logical>, area: Rectangle<f64, Logical>, turn: Transform) -> Point<f64, Logical> {
    area.loc + turn.invert().transform_point_in(on_panel, &turn.transform_size(area.size))
}

impl Server {
    /// The surface of the frontmost window shown at `pos` in the scene that input reaches, and where `pos`
    /// is on it.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        self.scene.windows().filter(|placed| placed.input).find_map(|placed| {
            let client = self.windows.iter().find(|w| w.id == placed.id)?;
            let geometry = self.geometry(placed.id)?;
            if !geometry.viewport.to_f64().contains(pos) {
                return None;
            }
            let location = geometry.surface_coords(pos);
            let (surface, origin) = client.window.surface_under(location, smithay::desktop::WindowSurfaceType::ALL)?;
            if client.window.root().as_ref() == Some(&surface) && !geometry.native.to_f64().contains(location) {
                return None;
            }
            Some(Hit { window: client.id, surface, location, origin: origin.to_f64() })
        })
    }

    /// An input event from a host. `area` is where an absolute device (a touchscreen, a tablet, the
    /// nested window) lies in the scene, and the turn of the panel it reports in (see [`placed`]).
    pub(crate) fn input<B: InputBackend>(&mut self, event: InputEvent<B>, host: Host, area: impl Fn(&B::Device) -> (Rectangle<f64, Logical>, Transform)) {
        self.idle.input(std::time::Instant::now());
        // Order: input is hit against the scene as the dispatch so far left it (a commit in it may have moved
        // a window), placed again if it changed.
        self.place();
        match event {
            InputEvent::Keyboard { event, .. } => self.key::<B>(&event, host),
            InputEvent::PointerMotion { event } => {
                self.validate_capture();
                let delta = PointerMotionEvent::<B>::delta(&event);
                let unaccelerated = PointerMotionEvent::<B>::delta_unaccel(&event);
                let time = Event::<B>::time(&event);
                // Winit can report absolute and relative events for one physical movement.
                // Its absolute event positions the free cursor; captured motion is relative-only.
                if self.pointer.capture.is_some() || !host.absolute_cursor {
                    self.pointer_relative(delta, unaccelerated, time);
                } else {
                    self.relative_motion(delta, unaccelerated, time);
                }
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.validate_capture();
                if self.pointer.capture.is_none() {
                    let at = placed(&event, area(&event.device()));
                    log::debug!("compositor: absolute motion to {at:?}");
                    self.pointer_absolute(at, event.time());
                }
            }
            InputEvent::PointerButton { event, .. } => self.button::<B>(&event),
            InputEvent::PointerAxis { event, .. } => self.axis::<B>(&event),
            // Positions are within the device's area of the scene. Winit sends no frame events, so each
            // of its touch events ends a frame.
            InputEvent::TouchDown { event } => {
                self.touch_down(event.slot(), placed(&event, area(&event.device())), event.time());
                if !host.touch_frames {
                    self.touch_frame();
                }
            }
            InputEvent::TouchMotion { event } => {
                self.touch_motion(event.slot(), placed(&event, area(&event.device())), event.time());
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

    /// A key from a keyboard, through the compositor's filter (see [`Self::input`]): Home's stand-in and the
    /// window keys on a desktop, VT switches, ClamShell's own keys when no window has the keyboard; the rest
    /// to the focused client.
    fn key<B: InputBackend>(&mut self, event: &B::KeyboardKeyEvent, host: Host) {
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.input::<(), _>(self, event.key_code(), event.state(), SERIAL_COUNTER.next_serial(), event.time(), |server, _, key| {
                let sym = key.modified_sym();
                let down = event.state() == KeyState::Pressed;
                // On a desktop, F12 stands for the Home button.
                if host.desktop_keys && sym == Keysym::F12 {
                    if down != server.keyboard_home {
                        server.keyboard_home = down;
                        server.core.handle(StateEvent::HomeHeld(down));
                        if !down {
                            server.core.handle(StateEvent::HomeTap);
                        }
                    }
                    return FilterResult::Intercept(());
                }
                // The keymap gives these for Ctrl+Alt+F1..F12.
                if (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw()).contains(&sym.raw()) {
                    if down {
                        server.vt_switch = Some((sym.raw() - Keysym::XF86_Switch_VT_1.raw() + 1) as i32);
                    }
                    return FilterResult::Intercept(());
                }
                let action = match sym {
                    Keysym::F11 if host.desktop_keys => Some(StateEvent::HardwareKey(HardwareKey::WindowDown)),
                    Keysym::F10 if host.desktop_keys => Some(StateEvent::HardwareKey(HardwareKey::WindowUp)),
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

    /// A pointer button: a press on ClamShell's overlay or second screen is ClamShell's, one on a window
    /// activates it first; the button then goes to the client under the pointer.
    fn button<B: InputBackend>(&mut self, event: &B::PointerButtonEvent) {
        log::debug!("compositor: button {:#x} {:?} at {:?}", event.button_code(), event.state(), self.pointer.location);
        if event.button_code() == 0x110 && self.pointer.overlay_pressed {
            if event.state() == ButtonState::Released {
                self.pointer.overlay_pressed = false;
            }
            return;
        }
        if event.state() == ButtonState::Pressed {
            if self.pointer.buttons.is_empty() {
                let hit = self.hit(self.pointer.location);
                self.dismiss_menu_outside(hit.as_ref().map(|hit| hit.window));
                self.dismiss_x11_menus_outside(hit.as_ref().map(|hit| &hit.surface));
                let (screen, position) = self.screen_position(self.pointer.location);
                if event.button_code() == 0x110 && self.core.press_overlay(screen, position) {
                    self.pointer.overlay_pressed = true;
                    // Order: what the press changed (the keyboard's focus) before its button reaches a client.
                    self.sync_windows();
                    return;
                }
                let hit = self.hit(self.pointer.location);
                self.pointer.window = hit.as_ref().map(|hit| hit.window);
                if let Some(hit) = hit {
                    self.core.handle(WindowEvent::Activate(hit.window).into());
                } else if let Some((screen, position)) = self.ui_under() {
                    if screen == Screen::Top {
                        self.core.handle(WindowEvent::KeyboardFocus(None).into());
                    }
                    if event.button_code() == 0x110 {
                        self.core.press(Source::Mouse, screen, position, event.time().millis());
                    } else if event.button_code() == 0x111 && screen == Screen::Bottom {
                        // A right click on the second screen pastes into the field open there, as
                        // Ctrl+V does.
                        self.core.handle(StateEvent::Paste);
                    }
                }
            }
            if !self.pointer.buttons.contains(&event.button_code()) {
                self.pointer.buttons.push(event.button_code());
            }
            // Order: the window pressed is activated and given the keyboard before the button reaches
            // it, so that it gets the press with its focus.
            self.sync_windows();
        } else if event.button_code() == 0x110 {
            let at = self.core.at(event.time().millis());
            self.core.up(Source::Mouse, End::Release, at);
        }
        self.move_pointer(event.time());
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.button(self, &ButtonEvent { button: event.button_code(), state: event.state(), serial: SERIAL_COUNTER.next_serial(), time: event.time() });
            pointer.frame(self);
        }
        if event.state() == ButtonState::Released {
            self.pointer.buttons.retain(|code| *code != event.button_code());
            if self.pointer.buttons.is_empty() {
                self.pointer.window = None;
            }
        }
    }

    /// A wheel or scroll: to the window under the pointer, both axes, else to ClamShell's view there.
    fn axis<B: InputBackend>(&mut self, event: &B::PointerAxisEvent) {
        let amount = |axis| event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or_default() / 8.0);
        let vertical = amount(Axis::Vertical);
        if self.hit(self.pointer.location).is_some() {
            if let Some(pointer) = self.seat.get_pointer() {
                // Both axes, with the wheel's steps where it has them.
                let mut frame = AxisFrame::new(event.time()).source(event.source());
                for axis in [Axis::Vertical, Axis::Horizontal] {
                    if event.amount(axis).is_some() || event.amount_v120(axis).is_some() {
                        frame = frame.value(axis, amount(axis));
                    }
                    if let Some(v120) = event.amount_v120(axis) {
                        frame = frame.v120(axis, v120 as i32);
                    }
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
        } else if let Some((screen, position)) = self.ui_under() {
            self.core.wheel(screen, position, vertical.signum() as i32);
        }
    }

    pub(crate) fn pointer_absolute(&mut self, position: Point<f64, Logical>, time: smithay::backend::input::InputTime) {
        self.pointer.cursor_activity.motion(std::time::Instant::now());
        self.validate_capture();
        if self.pointer_locked() {
            return;
        }
        self.pointer.location = self.constrain_pointer(self.screens.clamp_point(position));
        let (_, position) = self.ui_position();
        self.core.moved(Source::Mouse, position, time.millis());
        self.move_pointer(time);
    }

    fn move_pointer(&mut self, time: smithay::backend::input::InputTime) {
        let hit = self.hit(self.pointer.location);
        let location = self
            .pointer
            .window
            .filter(|_| !self.pointer.buttons.is_empty())
            .and_then(|id| self.geometry(id))
            .map(|geometry| geometry.surface_coords(self.pointer.location))
            .or_else(|| hit.as_ref().map(|hit| hit.location))
            .unwrap_or(self.pointer.location);
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(self, hit.map(|hit| (hit.surface, hit.origin)), &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time });
            pointer.frame(self);
        }
    }

    fn ui_under(&self) -> Option<(Screen, (f64, f64))> {
        self.ui_at(self.pointer.location)
    }

    /// ClamShell's view at `point`, and where on its screen, unless a window is drawn over it there: what
    /// is touched is what the scene shows (see [`crate::scene`]).
    pub(crate) fn ui_at(&self, point: Point<f64, Logical>) -> Option<(Screen, (f64, f64))> {
        let (screen, position) = self.screen_position(point);
        match screen {
            Screen::Top if self.scene.covers_top() => None,
            Screen::Bottom if self.scene.windows().any(|placed| placed.below) && point.y >= self.area(true).loc.y as f64 => None,
            _ => Some((screen, position)),
        }
    }

    fn ui_position(&self) -> (Screen, (f64, f64)) {
        self.screen_position(self.pointer.location)
    }

    /// The screen `point` is on, and where on it.
    pub(crate) fn screen_position(&self, point: Point<f64, Logical>) -> (Screen, (f64, f64)) {
        if point.x < self.screens.top_width as f64 {
            (Screen::Top, (point.x, point.y))
        } else {
            (Screen::Bottom, (point.x - self.screens.top_width as f64, point.y))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_panels_corners_reach_the_screens_corners() {
        // The bottom screen, 1240 x 1080 upright, beside a 1920 wide top one.
        let area = Rectangle::<f64, Logical>::new((1920.0, 0.0).into(), (1240.0, 1080.0).into());
        assert_eq!(upright((10.0, 20.0).into(), area, Transform::Normal), (1930.0, 20.0).into());
        // Its panel reports 1080 x 1240, portrait. Each panel corner lands on a screen corner, all four
        // of them, and the middle stays the middle.
        let corners = [(0.0, 0.0), (1080.0, 0.0), (0.0, 1240.0), (1080.0, 1240.0)];
        let mut reached: Vec<(f64, f64)> = corners.iter().map(|&(x, y)| upright((x, y).into(), area, Transform::_90)).map(|p| (p.x, p.y)).collect();
        reached.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        assert_eq!(reached, vec![(1920.0, 0.0), (1920.0, 1080.0), (3160.0, 0.0), (3160.0, 1080.0)]);
        assert_eq!(upright((540.0, 620.0).into(), area, Transform::_90), (2540.0, 540.0).into());
    }
}
