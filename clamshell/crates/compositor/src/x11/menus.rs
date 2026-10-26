//! X11 menus and tooltips: override-redirect windows shown as popups of the managed window they belong to
//! (by `WM_TRANSIENT_FOR`, or the focused window of the same process), within its presentation, and
//! dismissed with it. They never become windows of the state's.

use super::*;
use crate::surface::X11Popup;

impl Wm {
    pub(super) fn popup_owner(&self, menu: &Window, focused: Option<i64>, hovered: Option<i64>) -> Option<i64> {
        let mut transient = menu.properties.transient;
        for _ in 0..self.windows.len() {
            let Some(xid) = transient else { break };
            let parent = self.windows.iter().find(|w| w.xid == xid)?;
            if !parent.override_redirect {
                return menu.popup_parent.map_or(Some(parent.id), |owner| (owner == parent.id).then_some(owner));
            }
            if !parent.wants_map {
                return None;
            }
            transient = parent.properties.transient;
        }
        if transient.is_some() {
            return None;
        }
        if let Some(parent) = menu.popup_parent {
            return Some(parent);
        }
        let target = if menu.properties.tooltip { hovered.or(focused) } else { focused }?;
        self.windows.iter().find(|w| w.id == target && !w.override_redirect && w.pid.is_some() && w.pid == menu.pid).map(|w| w.id)
    }
    pub(super) fn settle_menus(&mut self, wire: &mut Transport, server: &mut Server) -> io::Result<()> {
        server.sync_x11_popups();
        for id in server.x11.popup_dismiss.drain(..) {
            if let Some(menu) = self.windows.iter_mut().find(|w| w.id == id && w.override_redirect && w.wants_map) {
                wire.request(UnmapWindowRequest { window: menu.xid }, 0, false)?;
                menu.wants_map = false;
            }
        }
        let focused = server.core.state().session().keyboard_focus();
        let hovered = server.hit(server.pointer.location).map(|hit| hit.window);
        for index in 0..self.windows.len() {
            let Some(menu) = self.windows.get(index) else { continue };
            if !menu.override_redirect || !menu.wants_map || menu.outstanding != 0 || menu.input_only {
                continue;
            }
            let owner = self.popup_owner(menu, focused, hovered);
            let parent = owner.and_then(|id| self.windows.iter().find(|w| w.id == id && w.opened));
            let valid = parent.is_some_and(|parent| server.visible(parent.id) && (menu.properties.tooltip || focused == Some(parent.id)));
            if !valid {
                let id = menu.id;
                wire.request(UnmapWindowRequest { window: menu.xid }, 0, false)?;
                for window in &mut server.windows {
                    window.window.dismiss_x11_popups(&mut server.x11.popup_dismiss, |popup| popup.id == id);
                }
                if let Some(menu) = self.windows.get_mut(index) {
                    menu.wants_map = false;
                }
                continue;
            }
            let Some(parent) = parent else { continue };
            let Some(geometry) = server.geometry(parent.id) else { continue };
            let Some(configured) = parent.configured else { continue };
            let (id, location) = (menu.id, menu.position - configured.loc);
            let Some(surface) = menu.surface.clone() else { continue };
            if !with_renderer_surface_state(&surface, |state| state.buffer().is_some()).unwrap_or(false) {
                continue;
            }
            if let Some(parent) = server.windows.iter_mut().find(|window| window.id == parent.id)
                && let Surface::X11(window) = &mut parent.window
            {
                if let Some(popup) = window.popups.iter_mut().find(|popup| popup.id == id) {
                    popup.location = location;
                    popup.input = !menu.properties.tooltip;
                } else {
                    window.popups.push(X11Popup { id, surface, location, geometry, input: !menu.properties.tooltip });
                }
            }
            if let Some(menu) = self.windows.get_mut(index) {
                menu.popup_parent = owner;
            }
        }
        Ok(())
    }
}
