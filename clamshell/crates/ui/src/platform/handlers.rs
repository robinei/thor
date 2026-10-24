//! The Wayland protocol handlers: configures and frame callbacks for the views, and the seats with their
//! keyboard, pointer and touch, whose events are passed on to the core by the screen they are on.

use clamshell_state::Event;
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
use crate::Screen;
use crate::input::{End, Source};

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, factor: i32) {
        if let Some(screen) = self.screen_of(surface) {
            let (size, _) = self.core.size(screen);
            self.core.resize(screen, size, factor.max(1).unsigned_abs());
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(view) = self.view_mut(surface) {
            view.frame_pending = false;
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, output: &wl_output::WlOutput) {
        if self.screen_of(surface) == Some(Screen::Bottom) {
            self.bottom_output = Some(output.clone());
            self.update_bottom_density();
        }
    }

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, output: &wl_output::WlOutput) {
        if self.screen_of(surface) == Some(Screen::Bottom) && self.bottom_output.as_ref() == Some(output) {
            self.bottom_output = None;
            self.update_bottom_density();
        }
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window, configure: WindowConfigure, _: u32) {
        if let Some(view) = self.view_mut(window.wl_surface()) {
            let (w, h) = configure.new_size;
            let size = (w.map_or(view.default_size.0, |w| w.get()), h.map_or(view.default_size.1, |h| h.get()));
            view.configured = true;
            view.reconfigured = true;
            let screen = view.screen;
            self.configured(screen, size);
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
            let size = (if w == 0 { view.default_size.0 } else { w }, if h == 0 { view.default_size.1 } else { h });
            view.configured = true;
            view.reconfigured = true;
            let screen = view.screen;
            self.configured(screen, size);
        }
    }
}

impl App {
    /// The compositor gave `screen` its logical size.
    fn configured(&mut self, screen: crate::Screen, size: (u32, u32)) {
        let (_, scale) = self.core.size(screen);
        self.core.resize(screen, size, scale);
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.bottom_output.as_ref() == Some(&output) {
            self.update_bottom_density();
        }
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.bottom_output.as_ref() == Some(&output) {
            self.bottom_output = None;
            self.update_bottom_density();
        }
    }
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
                self.core.up(Source::Mouse, End::Cancel, std::time::Instant::now());
            }
            Capability::Touch => {
                if let Some(t) = self.touch.take() {
                    t.release();
                }
                self.core.cancel_touches();
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl App {
    /// Keys stand in for the pad on a desktop (see `BottomKind::desktop_keys`).
    fn key(&mut self, sym: Keysym, down: bool) {
        if self.bottom_kind.desktop_keys() {
            self.core.key(sym, down);
        }
    }
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}

    // No release comes once the focus has left, so a held Enter would go on repeating.
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32) {
        self.core.up(Source::Enter, End::Cancel, std::time::Instant::now());
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
                PointerEventKind::Press { button: BTN_RIGHT, .. } if self.screen_of(&event.surface) != Some(Screen::Top) => {
                    self.core.handle(Event::Paste);
                }
                PointerEventKind::Press { time, .. } => {
                    if let Some(screen) = self.screen_of(&event.surface) {
                        self.core.press(Source::Mouse, screen, event.position, time);
                    }
                }
                PointerEventKind::Motion { time } => self.core.moved(Source::Mouse, event.position, time),
                PointerEventKind::Release { time, .. } => {
                    let now = self.core.at(time);
                    self.core.up(Source::Mouse, End::Release, now);
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let notches = if vertical.discrete != 0 { vertical.discrete } else { (vertical.absolute / 10.0) as i32 };
                    if let Some(screen) = self.screen_of(&event.surface).filter(|_| notches != 0) {
                        self.core.wheel(screen, event.position, notches);
                    }
                }
                _ => {}
            }
        }
    }
}

impl TouchHandler for App {
    fn down(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, time: u32, surface: WlSurface, id: i32, position: (f64, f64)) {
        if let Some(screen) = self.screen_of(&surface) {
            self.core.press(Source::Touch(id), screen, position, time);
        }
    }

    fn up(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: u32, time: u32, id: i32) {
        let now = self.core.at(time);
        self.core.up(Source::Touch(id), End::Release, now);
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, time: u32, id: i32, position: (f64, f64)) {
        self.core.moved(Source::Touch(id), position, time);
    }

    fn shape(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64, _: f64) {}

    fn orientation(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _: i32, _: f64) {}

    fn cancel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch) {
        self.core.cancel_touches();
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
