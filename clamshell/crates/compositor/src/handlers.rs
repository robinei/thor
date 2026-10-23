//! Wayland protocol callbacks. Placement is read only after the lifecycle event has settled.

use std::sync::OnceLock;

use clamshell_state::WindowEvent;
use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    desktop::{PopupKind, WindowSurfaceType},
    input::{Seat, SeatHandler, SeatState},
    reexports::wayland_server::{
        Client, Resource,
        protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface},
        output::OutputHandler,
        pointer_constraints::PointerConstraintsHandler,
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus},
        },
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shm::{ShmHandler, ShmState},
    },
};

use crate::server::{ClientState, Server};

impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        static MISSING: OnceLock<CompositorClientState> = OnceLock::new();
        match client.get_data::<ClientState>() {
            Some(data) => &data.compositor,
            None => {
                log::warn!("compositor: client missing protocol state");
                MISSING.get_or_init(CompositorClientState::default)
            }
        }
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(client) = self.windows.iter_mut().find(|w| w.window.toplevel().is_some_and(|t| t.wl_surface() == &root)) {
                client.window.on_commit();
                let mapped = with_renderer_surface_state(&root, |state| state.buffer().is_some()).unwrap_or(false);
                if client.mapped && !mapped {
                    client.mapped = false;
                    client.opened = false;
                    self.core.handle(WindowEvent::Closed(client.id).into());
                    self.sync_windows();
                    return;
                }
                client.mapped = mapped;
            }
        }
        self.open_on_commit(surface);
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(error) = popup.send_configure()
        {
            log::debug!("compositor: configure popup: {error}");
        }
    }
}
impl BufferHandler for Server {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}
impl ShmHandler for Server {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl OutputHandler for Server {}
impl PointerConstraintsHandler for Server {}

impl XdgShellHandler for Server {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.shell
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_toplevel(surface);
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.surface_id(surface.wl_surface()) {
            self.core.handle(WindowEvent::Closed(id).into());
            self.windows.retain(|w| w.id != id);
            self.sync_windows();
        }
    }
    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.surface_id(surface.wl_surface())
            && self.core.state().session().window(id).is_some()
        {
            let title = self.facts(&surface, id).title;
            self.core.handle(WindowEvent::Title { window: id, title }.into());
        }
    }
    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.identity_changed(surface);
    }
    fn parent_changed(&mut self, surface: ToplevelSurface) {
        self.identity_changed(surface);
    }
    fn new_popup(&mut self, surface: PopupSurface, _: PositionerState) {
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            log::debug!("compositor: popup: {error}");
        }
    }
    fn grab(&mut self, _: PopupSurface, _: WlSeat, _: Serial) {}
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|pending| {
            pending.geometry = positioner.get_geometry();
            pending.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl Server {
    fn identity_changed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.surface_id(surface.wl_surface())
            && self.core.state().session().window(id).is_some()
        {
            self.core.handle(WindowEvent::Changed(self.facts(&surface, id)).into());
        }
    }

    pub fn hit(&self, pos: smithay::utils::Point<f64, smithay::utils::Logical>) -> Option<crate::input::Hit> {
        self.windows.iter().rev().filter(|w| w.mapped && self.visible(w.id)).find_map(|client| {
            let geometry = self.geometry(client.id)?;
            if !geometry.viewport.to_f64().contains(pos) {
                return None;
            }
            let location = geometry.surface_coords(pos);
            let (surface, origin) = client.window.surface_under(location, WindowSurfaceType::ALL)?;
            if client.window.toplevel().is_some_and(|top| top.wl_surface() == &surface) && !geometry.native.to_f64().contains(location) {
                return None;
            }
            Some(crate::input::Hit { window: client.id, surface, location, origin: origin.to_f64() })
        })
    }
}

impl SeatHandler for Server {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seats
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: smithay::input::pointer::CursorImageStatus) {}
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        set_data_device_focus(&self.dh, seat, focused.and_then(|s| self.dh.get_client(s.id()).ok()));
    }
}
impl SelectionHandler for Server {
    type SelectionUserData = ();
}
impl DataDeviceHandler for Server {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device
    }
}
impl WaylandDndGrabHandler for Server {}

smithay::delegate_dispatch2!(Server);
