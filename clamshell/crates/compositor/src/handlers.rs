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
        compositor::{self, CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface},
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
    fn destroyed(&mut self, surface: &WlSurface) {
        self.colour.surfaces.remove(&smithay::backend::renderer::element::Id::from_wayland_resource(surface));
    }
    fn commit(&mut self, surface: &WlSurface) {
        // Before the buffer handler takes it: a new buffer on a window's own surface is a frame.
        let new_buffer = compositor::with_states(surface, |states| {
            matches!(states.cached_state.get::<compositor::SurfaceAttributes>().current().buffer, Some(compositor::BufferAssignment::NewBuffer(_)))
        });
        // The area it covers, which decides which surface holds its window's picture (see
        // `Surface::picture`).
        let covers = || with_renderer_surface_state(surface, |state| state.buffer().and(state.view()).map(|view| view.dst)).flatten();
        let covered = covers();
        on_commit_buffer_handler::<Self>(surface);
        let picture_may_move = covers() != covered;
        self.commits = self.commits.wrapping_add(1);
        let id = smithay::backend::renderer::element::Id::from_wayland_resource(surface);
        let current = crate::colour::protocol::current(surface);
        let previous = match current.clone() {
            Some(colour) => self.colour.surfaces.insert(id, colour),
            None => self.colour.surfaces.remove(&id),
        };
        if previous != current {
            self.colour.revision = self.colour.revision.wrapping_add(1);
        }
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(client) = self.windows.iter_mut().find(|w| w.window.root().as_ref() == Some(&root)) {
                if new_buffer && root == *surface {
                    client.frames.committed();
                }
                client.window.on_commit();
                let mapped = with_renderer_surface_state(&root, |state| state.buffer().is_some()).unwrap_or(false);
                if client.mapped && !mapped && client.window.toplevel().is_some() {
                    client.mapped = false;
                    client.opened = false;
                    client.title_dirty = false;
                    self.core.handle(WindowEvent::Closed(client.id).into());
                    self.sync_windows();
                    return;
                }
                client.mapped = mapped;
            }
        }
        self.open_on_commit(surface);
        // A subsurface's colour may be its window's (see `Server::window_colour`): when it changes, or the
        // window's picture may have moved to or from it.
        if (previous != current || picture_may_move) && get_parent(surface).is_some() {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if self.windows.iter().any(|w| w.opened && w.window.root().as_ref() == Some(&root)) {
                self.open_on_commit(&root);
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
            && let Err(error) = popup.send_configure()
        {
            self.reject_client(surface, &format!("cannot configure popup: {error}"));
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
impl OutputHandler for Server {
    fn output_bound(&mut self, output: smithay::output::Output, wl_output: smithay::reexports::wayland_server::protocol::wl_output::WlOutput) {
        if output == self.outputs[0] {
            self.top_bound.push(wl_output.downgrade());
            self.tell_game_mode(&wl_output);
        }
    }
}
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
        if let Some(client) =
            self.windows.iter_mut().find(|client| client.opened && client.window.toplevel().is_some_and(|top| top.wl_surface() == surface.wl_surface()))
        {
            client.title_dirty = true;
        }
    }
    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.identity_changed(surface);
    }
    fn parent_changed(&mut self, surface: ToplevelSurface) {
        self.identity_changed(surface);
    }
    fn new_popup(&mut self, surface: PopupSurface, _: PositionerState) {
        let wl_surface = surface.wl_surface().clone();
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            self.reject_client(&wl_surface, &format!("cannot track popup: {error}"));
        }
    }
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        self.grab_popup(surface, seat, serial);
    }
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
            if client.window.root().as_ref() == Some(&surface) && !geometry.native.to_f64().contains(location) {
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
    fn touch_down_grab(&mut self, start: smithay::input::touch::GrabStartData<Self>) -> impl smithay::input::touch::TouchGrab<Self> {
        crate::touch::ContactGrab::new(start)
    }
    fn cursor_image(&mut self, _: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        self.cursor = image;
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        smithay::wayland::selection::primary_selection::set_primary_focus(&self.dh, seat, client);
    }
}
impl SelectionHandler for Server {
    type SelectionUserData = u64;
    fn new_selection(
        &mut self,
        target: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        self.x11_selections.changed(target, source);
    }
    fn send_selection(
        &mut self,
        target: smithay::wayland::selection::SelectionTarget,
        mime: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        origin: &u64,
    ) {
        self.x11_selections.read(target, mime, fd, *origin);
    }
}
impl DataDeviceHandler for Server {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device
    }
}
impl WaylandDndGrabHandler for Server {}

smithay::delegate_dispatch2!(Server);

impl smithay::wayland::selection::primary_selection::PrimarySelectionHandler for Server {
    fn primary_selection_state(&mut self) -> &mut smithay::wayland::selection::primary_selection::PrimarySelectionState {
        &mut self.primary_selection
    }
}
impl smithay::wayland::selection::ext_data_control::DataControlHandler for Server {
    fn data_control_state(&mut self) -> &mut smithay::wayland::selection::ext_data_control::DataControlState {
        &mut self.data_control
    }
}

// Cursor-shape dispatch also accepts tablet tools; this host advertises only pointer devices.
impl smithay::input::tablet::TabletSeatHandler for Server {
    type ToolFocus = WlSurface;
}
