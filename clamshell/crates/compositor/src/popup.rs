//! Menus borrow their parent's focus; they never become state windows.

use smithay::{
    backend::input::InputTime,
    desktop::{PopupGrab, PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, find_popup_root_surface},
    input::{Seat, pointer::Focus},
    reexports::wayland_server::protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    utils::{SERIAL_COUNTER, Serial},
    wayland::shell::xdg::PopupSurface,
};

use crate::server::Server;

pub(crate) struct Menu {
    window: i64,
    grab: PopupGrab<Server>,
}

impl Server {
    pub fn grab_popup(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let kind = PopupKind::Xdg(surface.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            self.reject_client(surface.wl_surface(), "popup has no root");
            return;
        };
        let Some(window) = self.surface_id(&root) else {
            self.reject_client(surface.wl_surface(), "popup root is not a managed window");
            return;
        };
        if Seat::<Self>::from_resource(&seat).as_ref() != Some(&self.seat)
            || self.core.state().session().keyboard_focus() != Some(window)
            || !self.visible(window)
        {
            surface.send_popup_done();
            return;
        }
        let grab = match self.popups.grab_popup(root, kind, &self.seat, serial) {
            Ok(grab) => grab,
            Err(error) => {
                self.reject_client(surface.wl_surface(), &format!("cannot grab popup: {error}"));
                return;
            }
        };
        let previous = grab.previous_serial().unwrap_or(serial);
        let keyboard = self.seat.get_keyboard();
        let pointer = self.seat.get_pointer();
        if keyboard.as_ref().is_some_and(|keyboard| keyboard.is_grabbed() && !keyboard.has_grab(serial) && !keyboard.has_grab(previous))
            || pointer.as_ref().is_some_and(|pointer| pointer.is_grabbed() && !pointer.has_grab(serial) && !pointer.has_grab(previous))
        {
            let mut grab = grab;
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        // Replacing a parent pointer grab releases its matching keyboard grab. Install the
        // new keyboard grab afterwards so opening a submenu cannot release the new grab.
        if let Some(pointer) = pointer {
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        if let Some(keyboard) = keyboard {
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        self.menu = Some(Menu { window, grab });
    }

    pub fn dismiss_menu(&mut self) {
        let Some(mut menu) = self.menu.take() else { return };
        menu.grab.ungrab(PopupUngrabStrategy::All);
        let serial = menu.grab.serial();
        let previous = menu.grab.previous_serial().unwrap_or(serial);
        if let Some(keyboard) = self.seat.get_keyboard()
            && (keyboard.has_grab(serial) || keyboard.has_grab(previous))
        {
            keyboard.unset_grab(self);
        }
        if let Some(pointer) = self.seat.get_pointer()
            && (pointer.has_grab(serial) || pointer.has_grab(previous))
        {
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), InputTime::now());
        }
    }

    pub fn dismiss_menu_outside(&mut self, window: Option<i64>) {
        if self.menu.as_ref().is_some_and(|menu| Some(menu.window) != window) {
            self.dismiss_menu();
        }
    }

    pub fn menu_focus(&mut self, parent: Option<WlSurface>) -> Option<WlSurface> {
        if self.menu.as_ref().is_some_and(|menu| {
            menu.grab.has_ended()
                || self.core.state().session().keyboard_focus() != Some(menu.window)
                || !self.visible(menu.window)
                || self.geometry(menu.window).is_none()
        }) {
            self.dismiss_menu();
        }
        self.menu.as_ref().and_then(|menu| menu.grab.current_grab()).or(parent)
    }
}

impl Server {
    pub(crate) fn has_x11_menu(&self) -> bool {
        self.windows.iter().any(|window| window.window.x11_popups().iter().any(|popup| popup.input))
    }
    pub(crate) fn sync_x11_popups(&mut self) {
        let focus = self.core.state().session().keyboard_focus();
        for index in 0..self.windows.len() {
            let current = self.windows.get(index).and_then(|window| self.visible(window.id).then(|| self.geometry(window.id)).flatten());
            if let Some(window) = self.windows.get_mut(index) {
                window.window.dismiss_x11_popups(&mut self.x11.popup_dismiss, |popup| {
                    !smithay::reexports::wayland_server::Resource::is_alive(&popup.surface)
                        || current != Some(popup.geometry)
                        || (popup.input && focus != Some(window.id))
                });
            }
        }
    }
    pub(crate) fn dismiss_x11_menus_outside(&mut self, surface: Option<&WlSurface>) {
        for window in &mut self.windows {
            if !surface.is_some_and(|surface| window.window.popup_surface(surface)) {
                window.window.dismiss_x11_popups(&mut self.x11.popup_dismiss, |popup| popup.input);
            }
        }
    }
}
