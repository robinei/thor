//! `xwayland_shell_v1`, seen only by Xwayland: it gives a window's surface a serial, sent the window too
//! as `WL_SURFACE_SERIAL`, and the window manager pairs them by it. Unlike a surface's id, a serial is
//! never reused, so a window can't be paired with a later surface that took a destroyed one's id.
use crate::server::{ClientState, Server};
use smithay::{
    reexports::{
        wayland_protocols::xwayland::shell::v1::server::{
            xwayland_shell_v1::{self, XwaylandShellV1},
            xwayland_surface_v1::{self, XwaylandSurfaceV1},
        },
        wayland_server::{Client, DataInit, DisplayHandle, New, Resource, protocol::wl_surface::WlSurface},
    },
    wayland::{
        Dispatch2, GlobalDispatch2,
        compositor::{self, Cacheable},
    },
};

#[derive(Clone, Copy)]
pub(crate) struct Shell;
pub(crate) struct XwaylandSurface(WlSurface);

/// A serial set and not yet committed.
#[derive(Debug, Clone, Copy, Default)]
struct Serial(Option<u64>);
impl Cacheable for Serial {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        *self
    }
    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}

pub(crate) fn install(dh: &DisplayHandle) {
    dh.create_global::<Server, XwaylandShellV1, _>(1, Shell);
}

/// The surface Xwayland committed with this serial, taken.
pub(crate) fn take(server: &mut Server, serial: u64) -> Option<WlSurface> {
    let index = server.xwayland_serials.iter().position(|(s, _)| *s == serial)?;
    Some(server.xwayland_serials.swap_remove(index).1)
}

impl GlobalDispatch2<XwaylandShellV1, Server> for Shell {
    fn bind(&self, _: &mut Server, _: &DisplayHandle, _: &Client, object: New<XwaylandShellV1>, init: &mut DataInit<'_, Server>) {
        init.init(object, Shell);
    }
    fn can_view(&self, client: &Client) -> bool {
        client.get_data::<ClientState>().is_some_and(|state| state.xwayland)
    }
}
impl Dispatch2<XwaylandShellV1, Server> for Shell {
    fn request(
        &self,
        _: &mut Server,
        _: &Client,
        shell: &XwaylandShellV1,
        request: xwayland_shell_v1::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            xwayland_shell_v1::Request::GetXwaylandSurface { id, surface } => {
                if compositor::give_role(&surface, "xwayland_shell").is_err() {
                    shell.post_error(xwayland_shell_v1::Error::Role, "the surface already has a role");
                    return;
                }
                compositor::add_pre_commit_hook::<Server, _>(&surface, |server, _, surface| {
                    let Some(serial) = compositor::with_states(surface, |states| states.cached_state.get::<Serial>().pending().0.take()) else {
                        return;
                    };
                    // Surfaces destroyed before their window was paired go now.
                    server.xwayland_serials.retain(|(_, surface)| surface.is_alive());
                    server.xwayland_serials.push((serial, surface.clone()));
                });
                init.init(id, XwaylandSurface(surface));
            }
            xwayland_shell_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
impl Dispatch2<XwaylandSurfaceV1, Server> for XwaylandSurface {
    fn request(
        &self,
        _: &mut Server,
        _: &Client,
        object: &XwaylandSurfaceV1,
        request: xwayland_surface_v1::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Server>,
    ) {
        match request {
            xwayland_surface_v1::Request::SetSerial { serial_lo, serial_hi } => {
                let serial = u64::from(serial_lo) | (u64::from(serial_hi) << 32);
                if serial == 0 {
                    object.post_error(xwayland_surface_v1::Error::InvalidSerial, "a serial is never 0");
                    return;
                }
                compositor::with_states(&self.0, |states| states.cached_state.get::<Serial>().pending().0 = Some(serial));
            }
            xwayland_surface_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}
