//! Touch: each finger goes to what it came down on, a window's surface or ClamShell's view on that screen,
//! until it lifts, with the window's geometry as it was then, so a finger on each screen acts on its own.
//! Smithay's down grab would send every finger to the first finger's surface, so a grab of our own keeps
//! them apart.

use crate::{geometry::Geometry, server::Server};
use clamshell_state::WindowEvent;
use clamshell_ui::{End, Screen, Source};
use smithay::{
    backend::input::{InputTime, TouchSlot},
    input::touch::{DownEvent, GrabStartData, MotionEvent, OrientationEvent, ShapeEvent, TouchGrab, TouchInnerHandle, UpEvent},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

#[derive(Clone)]
pub(crate) enum Target {
    Client { window: i64, surface: WlSurface, geometry: Geometry, origin: Point<f64, Logical> },
    Ui(Screen),
    Ignored,
}
/// The touches in progress.
pub(crate) struct Touches {
    /// Each finger down, in a slot of its own.
    pub contacts: [Option<Contact>; 32],
    /// Whether clients were sent touch events since the last frame event.
    pub pending: bool,
}

impl Touches {
    pub fn new() -> Self {
        Self { contacts: std::array::from_fn(|_| None), pending: false }
    }
}

/// A finger down: its slot, what it came down on, and where it is in the scene.
pub(crate) struct Contact {
    slot: TouchSlot,
    target: Target,
    location: Point<f64, Logical>,
}

impl Server {
    pub(crate) fn touch_down(&mut self, slot: TouchSlot, position: Point<f64, Logical>, time: InputTime) {
        self.pointer.cursor_activity.hide();
        self.idle.input(std::time::Instant::now());
        if self.touch.contacts.iter().flatten().any(|contact| contact.slot == slot) {
            self.touch_cancel();
        }
        let Some(index) = self.touch.contacts.iter().position(Option::is_none) else {
            log::debug!("compositor: too many simultaneous touches");
            return;
        };
        let hit = self.hit(position);
        self.dismiss_menu_outside(hit.as_ref().map(|hit| hit.window));
        self.dismiss_x11_menus_outside(hit.as_ref().map(|hit| &hit.surface));
        let (screen, local) = self.screen_position(position);
        let target = if self.core.press_overlay(screen, local) {
            Target::Ignored
        } else if let Some(hit) = self.hit(position) {
            if let Some(geometry) = self.geometry(hit.window) {
                self.core.handle(WindowEvent::Activate(hit.window).into());
                let location = hit.location - hit.origin;
                if let Some(touch) = self.seat.get_touch() {
                    self.touch.pending = true;
                    touch.down(self, Some((hit.surface.clone(), (0.0, 0.0).into())), &DownEvent { slot, location, serial: SERIAL_COUNTER.next_serial(), time });
                }
                Target::Client { window: hit.window, surface: hit.surface, geometry, origin: hit.origin }
            } else {
                Target::Ignored
            }
        } else if let Some((screen, local)) = self.ui_at(position) {
            if screen == Screen::Top {
                self.core.handle(WindowEvent::KeyboardFocus(None).into());
            }
            self.core.press(Source::Touch(slot.into()), screen, local, time.millis());
            Target::Ui(screen)
        } else {
            Target::Ignored
        };
        if let Some(contact) = self.touch.contacts.get_mut(index) {
            *contact = Some(Contact { slot, target, location: position });
        }
        self.sync_windows();
    }
    pub(crate) fn touch_motion(&mut self, slot: TouchSlot, position: Point<f64, Logical>, time: InputTime) {
        self.pointer.cursor_activity.hide();
        self.idle.input(std::time::Instant::now());
        let Some(contact) = self.touch.contacts.iter_mut().flatten().find(|contact| contact.slot == slot) else { return };
        contact.location = position;
        let target = contact.target.clone();
        match target {
            Target::Client { surface, geometry, origin, .. } => {
                if let Some(touch) = self.seat.get_touch() {
                    self.touch.pending = true;
                    touch.motion(self, Some((surface, (0.0, 0.0).into())), &MotionEvent { slot, location: geometry.surface_coords(position) - origin, time });
                }
            }
            Target::Ui(screen) => {
                let x = if screen == Screen::Bottom { position.x - self.screens.top_width as f64 } else { position.x };
                self.core.moved(Source::Touch(slot.into()), (x, position.y), time.millis());
            }
            Target::Ignored => {}
        }
    }
    pub(crate) fn touch_up(&mut self, slot: TouchSlot, time: InputTime) {
        self.pointer.cursor_activity.hide();
        self.idle.input(std::time::Instant::now());
        let Some(contact) = self.touch.contacts.iter_mut().find(|contact| contact.as_ref().is_some_and(|contact| contact.slot == slot)).and_then(Option::take)
        else {
            return;
        };
        match contact.target {
            Target::Client { .. } => {
                if let Some(touch) = self.seat.get_touch() {
                    self.touch.pending = true;
                    touch.up(self, &UpEvent { slot, serial: SERIAL_COUNTER.next_serial(), time });
                }
            }
            Target::Ui(_) => {
                let at = self.core.at(time.millis());
                self.core.up(Source::Touch(slot.into()), End::Release, at);
            }
            Target::Ignored => {}
        }
    }
    pub(crate) fn touch_frame(&mut self) {
        if std::mem::take(&mut self.touch.pending)
            && let Some(touch) = self.seat.get_touch()
        {
            touch.frame(self);
        }
    }
    pub(crate) fn cancel_hidden_touches(&mut self) {
        let hidden = self.touch.contacts.iter().flatten().any(|contact| match &contact.target {
            Target::Client { window, surface, .. } => {
                !self.visible(*window) || !self.windows.iter().any(|client| client.id == *window && client.mapped && client.window.contains_x11_target(surface))
            }
            _ => false,
        });
        if hidden {
            self.touch_cancel();
        }
    }

    pub(crate) fn touch_cancel(&mut self) {
        self.core.cancel_touches();
        if let Some(touch) = self.seat.get_touch() {
            // This Smithay revision cancels only slots pending since the last frame. Repeat each
            // active position before cancelling so stationary contacts are cancelled too.
            for index in 0..self.touch.contacts.len() {
                let contact = self.touch.contacts.get_mut(index).and_then(Option::take);
                if let Some(Contact { slot, target: Target::Client { surface, geometry, origin, .. }, location }) = contact {
                    self.touch.pending = true;
                    touch.motion(
                        self,
                        Some((surface, (0.0, 0.0).into())),
                        &MotionEvent { slot, location: geometry.surface_coords(location) - origin, time: InputTime::now() },
                    );
                }
            }
            touch.cancel(self);
        }
        self.touch.contacts.fill_with(|| None);
        self.touch.pending = false;
    }
}

// Smithay's standard down grab sends all fingers to the first surface. Each screen needs its own
// contacts; Smithay's inner handle already retains the surface independently for every slot.
pub(crate) struct ContactGrab {
    start: GrabStartData<Server>,
    count: usize,
}
impl ContactGrab {
    pub fn new(start: GrabStartData<Server>) -> Self {
        Self { start, count: 1 }
    }
}
impl TouchGrab<Server> for ContactGrab {
    fn down(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>, focus: Option<(WlSurface, Point<f64, Logical>)>, event: &DownEvent) {
        handle.down(data, focus, event);
        self.count += 1;
    }
    fn up(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>, event: &UpEvent) {
        handle.up(data, event);
        self.count = self.count.saturating_sub(1);
        if self.count == 0 {
            handle.unset_grab(self, data);
        }
    }
    fn motion(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>, focus: Option<(WlSurface, Point<f64, Logical>)>, event: &MotionEvent) {
        handle.motion(data, focus, event);
    }
    fn frame(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>) {
        handle.frame(data);
    }
    fn cancel(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>) {
        handle.cancel(data);
        handle.unset_grab(self, data);
    }
    fn shape(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>, event: &ShapeEvent) {
        handle.shape(data, event);
    }
    fn orientation(&mut self, data: &mut Server, handle: &mut TouchInnerHandle<'_, Server>, event: &OrientationEvent) {
        handle.orientation(data, event);
    }
    fn start_data(&self) -> &GrabStartData<Server> {
        &self.start
    }
    fn unset(&mut self, _: &mut Server) {}
}
