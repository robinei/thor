//! The Wayland protocol handlers: configures and frame callbacks for the views, and the seats with their
//! keyboard, pointer and touch, whose events are passed on to the input side.

use clamshell_core::{Event, ShelfEvent};
use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::client::{
        Connection, QueueHandle,
        protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface::WlSurface, wl_touch},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        touch::TouchHandler,
    },
    shell::{
        WaylandSurface,
        wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        xdg::window::{Window, WindowConfigure, WindowHandler},
    },
    shm::{Shm, ShmHandler},
};

use super::App;
use crate::input::{End, Source};

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, factor: i32) {
        if let Some(view) = self.view_mut(surface) {
            view.scale = factor.max(1);
            view.stale = true;
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(view) = self.view_mut(surface) {
            view.frame_pending = false;
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window, configure: WindowConfigure, _: u32) {
        if let Some(view) = self.view_mut(window.wl_surface()) {
            let (w, h) = configure.new_size;
            view.size = (w.map_or(view.default_size.0, |w| w.get()), h.map_or(view.default_size.1, |h| h.get()));
            view.configured = true;
            view.reconfigured = true;
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        log::warn!("the compositor closed the bottom layer surface");
        self.exit = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        if let Some(view) = self.view_mut(layer.wl_surface()) {
            let (w, h) = configure.new_size;
            view.size = (if w == 0 { view.default_size.0 } else { w }, if h == 0 { view.default_size.1 } else { h });
            view.configured = true;
            view.reconfigured = true;
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.text_input.seat(&seat, qh);
        self.display.seat(&seat, qh);
    }

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).map_err(|e| log::warn!("keyboard: {e}")).ok();
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = self.seat_state.get_pointer(qh, &seat).map_err(|e| log::warn!("pointer: {e}")).ok();
                self.cursor = self.pointer.as_ref().zip(self.cursor_shapes.as_ref()).map(|(p, shapes)| shapes.get_shape_device(p, qh));
            }
            Capability::Touch if self.touch.is_none() => {
                self.touch = self.seat_state.get_touch(qh, &seat).map_err(|e| log::warn!("touch: {e}")).ok();
            }
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            Capability::Pointer => {
                if let Some(c) = self.cursor.take() {
                    c.destroy();
                }
                if let Some(p) = self.pointer.take() {
                    p.release();
                }
                self.up(Source::Mouse, End::Cancel, std::time::Instant::now());
            }
            Capability::Touch => {
                if let Some(t) = self.touch.take() {
                    t.release();
                }
                self.cancel_touches();
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}

    // No release comes once the focus has left, so a held Enter would go on repeating.
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32) {
        self.up(Source::Enter, End::Cancel, std::time::Instant::now());
    }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event.keysym, true);
    }

    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if matches!(event.keysym, Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down) {
            self.key(event.keysym, true);
        }
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event.keysym, false);
    }

    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: RawModifiers, _: u32) {}
}

/// The mouse's right button (evdev's code).
const BTN_RIGHT: u32 = 0x111;

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for event in events {
            match event.kind {
                // Show the arrow over our surfaces.
                PointerEventKind::Enter { serial } => {
                    if let Some(cursor) = &self.cursor {
                        cursor.set_shape(
                            serial,
                            smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape::Default,
                        );
                    }
                }
                // A right click on the second screen pastes into the field open there, as Ctrl+V does.
                PointerEventKind::Press { button: BTN_RIGHT, .. } if self.views.top.as_ref().is_none_or(|v| v.shell.surface() != &event.surface) => {
                    self.handle(Event::Paste);
                }
                PointerEventKind::Press { time, .. } => self.press_at(Source::Mouse, &event.surface, event.position, time),
                PointerEventKind::Motion { time } => self.moved(Source::Mouse, event.position, time),
                PointerEventKind::Release { time, .. } => {
                    let now = self.at(time);
                    self.up(Source::Mouse, End::Release, now);
                }
                // The wheel over the main screen scrolls the shelf's list by a game per notch. Over a list
                // on the second screen, it scrolls by a row per notch.
                PointerEventKind::Axis { vertical, .. } => {
                    let over_top = self.views.top.as_ref().is_some_and(|v| v.shell.surface() == &event.surface);
                    let notches = if vertical.discrete != 0 { vertical.discrete } else { (vertical.absolute / 10.0) as i32 };
                    if notches == 0 {
                        continue;
                    }
                    if over_top {
                        self.handle(Event::Shelf(ShelfEvent::ListStep(notches)));
                    } else {
                        self.wheel(&event.surface, event.position, notches);
                    }
                }
                _ => {}
            }
        }
    }
}

impl TouchHandler for App {
    fn down(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, time: u32, surface: WlSurface, id: i32, position: (f64, f64)) {
        self.press_at(Source::Touch(id), &surface, position, time);
    }

    fn up(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, time: u32, id: i32) {
        let now = self.at(time);
        self.up(Source::Touch(id), End::Release, now);
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, time: u32, id: i32, position: (f64, f64)) {
        self.moved(Source::Touch(id), position, time);
    }

    fn shape(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64, _: f64) {}

    fn orientation(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64) {}

    fn cancel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch) {
        self.cancel_touches();
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
