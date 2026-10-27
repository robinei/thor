//! Smithay's handlers for the protocols without a module of their own: surfaces and their commits (a
//! window's frames, its mapping and unmapping, its colour), shm and buffers, outputs bound, xdg-shell's
//! toplevels and popups, the seat (focus, the cursor's image) and the selections. A window's lifecycle event
//! is applied to the state first, and where the window goes is read from the state after.

use std::sync::OnceLock;

use clamshell_state::WindowEvent;
use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    desktop::PopupKind,
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

use crate::clients::ClientState;
use crate::server::Server;

impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor
    }
    /// A commit with a new dmabuf waits until the buffer is ready: its explicit sync acquire point is
    /// signalled, or its implicit fence when it has none. Composed earlier, the GPU would wait for the
    /// client's rendering in the compositor's frame, and a late frame would hold up the screen instead of
    /// the screen showing the previous one.
    fn new_surface(&mut self, surface: &WlSurface) {
        compositor::add_pre_commit_hook::<Self, _>(surface, |server, _dh, surface| {
            let (acquire, dmabuf) = compositor::with_states(surface, |states| {
                let acquire = states.cached_state.get::<smithay::wayland::drm_syncobj::DrmSyncobjCachedState>().pending().acquire_point.clone();
                let dmabuf = states.cached_state.get::<compositor::SurfaceAttributes>().pending().buffer.as_ref().and_then(|buffer| match buffer {
                    compositor::BufferAssignment::NewBuffer(buffer) => smithay::wayland::dmabuf::get_dmabuf(buffer).cloned().ok(),
                    compositor::BufferAssignment::Removed => None,
                });
                (acquire, dmabuf)
            });
            let (Some(dmabuf), Some(handle), Some(client)) = (dmabuf, server.loop_handle.clone(), surface.client()) else { return };
            let woken = move |server: &mut Server| {
                let dh = server.dh.clone();
                server.client_compositor_state(&client).blocker_cleared(server, &dh);
            };
            if let Some(acquire) = acquire {
                match acquire.generate_blocker() {
                    Ok((blocker, source)) => {
                        let woken = woken.clone();
                        match handle.insert_source(source, move |_, _, server| {
                            woken(server);
                            Ok(())
                        }) {
                            Ok(_) => return compositor::add_blocker(surface, blocker),
                            Err(error) => log::warn!("compositor: a buffer's acquire point can't be waited on, its fence is: {}", error.error),
                        }
                    }
                    Err(error) => log::warn!("compositor: a buffer's acquire point can't be waited on, its fence is: {error}"),
                }
            }
            // (A buffer already ready has nothing to wait for.)
            if let Ok((blocker, source)) = dmabuf.generate_blocker(smithay::reexports::calloop::Interest::READ) {
                match handle.insert_source(source, move |_, _, server| {
                    woken(server);
                    Ok(())
                }) {
                    Ok(_) => compositor::add_blocker(surface, blocker),
                    Err(error) => log::warn!("compositor: a buffer's fence can't be waited on, it is shown as it comes: {}", error.error),
                }
            }
        });
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
        self.pacing.commits = self.pacing.commits.wrapping_add(1);
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
            let client = self.windows.iter_mut().find(|w| w.window.root().as_ref() == Some(&root));
            match &client {
                Some(client) => {
                    let below = self.core.state().session().below(client.id);
                    if let Some(screen) = self.pacing.committed.screens.get_mut(usize::from(below)) {
                        *screen = true;
                    }
                }
                None => self.pacing.committed.elsewhere = true,
            }
            if let Some(client) = client {
                // A frame is a new buffer on the window's root, or on a subsurface drawn apart from it (not
                // synchronised: here only those come), as Wine's Wayland driver and pattern-game draw a
                // game's picture.
                if new_buffer {
                    client.frames.committed();
                }
                client.window.on_commit();
                let mapped = with_renderer_surface_state(&root, |state| state.buffer().is_some()).unwrap_or(false);
                if client.mapped && !mapped && client.window.toplevel().is_some() {
                    client.mapped = false;
                    client.opened = false;
                    client.title_dirty = false;
                    log::info!("compositor: window {} unmaps", client.id);
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
        // A window's size or picture may have changed: a subsurface's commit doesn't sync the windows.
        self.place();
    }
}
impl BufferHandler for Server {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}
impl ShmHandler for Server {
    fn shm_state(&self) -> &ShmState {
        &self.protocols.shm
    }
}
impl OutputHandler for Server {
    fn output_bound(&mut self, output: smithay::output::Output, wl_output: smithay::reexports::wayland_server::protocol::wl_output::WlOutput) {
        if output == self.screens.outputs[0] {
            self.screens.top_bound.push(wl_output.downgrade());
            self.tell_game_mode(&wl_output);
        }
    }
}
impl PointerConstraintsHandler for Server {}
impl smithay::wayland::fractional_scale::FractionalScaleHandler for Server {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        smithay::wayland::compositor::with_states(&surface, |states| {
            smithay::wayland::fractional_scale::with_fractional_scale(states, |scale| scale.set_preferred_scale(1.0));
        });
    }
}

impl XdgShellHandler for Server {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.shell
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        log::debug!("compositor: a toplevel on {:?}", surface.wl_surface().id());
        self.add_toplevel(surface);
    }
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.surface_id(surface.wl_surface()) {
            log::info!("compositor: window {id} closes: its toplevel on {:?} is gone", surface.wl_surface().id());
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
impl smithay::wayland::shell::xdg::dialog::XdgDialogHandler for Server {
    fn dialog_hint_changed(&mut self, toplevel: ToplevelSurface, _: smithay::wayland::shell::xdg::dialog::ToplevelDialogHint) {
        self.identity_changed(toplevel);
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
}

impl SeatHandler for Server {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.protocols.seats
    }
    fn touch_down_grab(&mut self, start: smithay::input::touch::GrabStartData<Self>) -> impl smithay::input::touch::TouchGrab<Self> {
        crate::touch::ContactGrab::new(start)
    }
    fn cursor_image(&mut self, _: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        self.pointer.cursor = image;
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
        self.x11.selections.changed(target, source);
    }
    fn send_selection(
        &mut self,
        target: smithay::wayland::selection::SelectionTarget,
        mime: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        origin: &u64,
    ) {
        self.x11.selections.read(target, mime, fd, *origin);
    }
}
impl DataDeviceHandler for Server {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.protocols.data_device
    }
}
impl WaylandDndGrabHandler for Server {}

impl smithay::wayland::drm_syncobj::DrmSyncobjHandler for Server {
    fn drm_syncobj_state(&mut self) -> Option<&mut smithay::wayland::drm_syncobj::DrmSyncobjState> {
        self.protocols.syncobj.as_mut()
    }
}

smithay::delegate_dispatch2!(Server);

impl smithay::wayland::selection::primary_selection::PrimarySelectionHandler for Server {
    fn primary_selection_state(&mut self) -> &mut smithay::wayland::selection::primary_selection::PrimarySelectionState {
        &mut self.protocols.primary_selection
    }
}
impl smithay::wayland::selection::ext_data_control::DataControlHandler for Server {
    fn data_control_state(&mut self) -> &mut smithay::wayland::selection::ext_data_control::DataControlState {
        &mut self.protocols.data_control
    }
}

// Cursor-shape dispatch also accepts tablet tools; this host advertises only pointer devices.
impl smithay::input::tablet::TabletSeatHandler for Server {
    type ToolFocus = WlSurface;
}
