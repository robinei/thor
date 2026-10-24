//! Wayland surface operations shared by xdg-shell and X11, without an X11 connection in drawing.
use smithay::{
    backend::renderer::{
        element::{
            AsRenderElements, Kind,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
        gles::GlesRenderer,
    },
    desktop::{
        Window, WindowSurfaceType,
        utils::{bbox_from_surface_tree, send_frames_surface_tree, under_from_surface_tree, with_surfaces_surface_tree},
    },
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::{compositor::SurfaceData, shell::xdg::ToplevelSurface},
};
use std::time::Duration;

pub(crate) enum Surface {
    Wayland(Window),
    X11(WlSurface),
}
impl Surface {
    pub fn toplevel(&self) -> Option<&ToplevelSurface> {
        match self {
            Self::Wayland(window) => window.toplevel(),
            Self::X11(_) => None,
        }
    }
    pub fn root(&self) -> Option<WlSurface> {
        match self {
            Self::Wayland(window) => window.toplevel().map(|t| t.wl_surface().clone()),
            Self::X11(surface) => Some(surface.clone()),
        }
    }
    pub fn on_commit(&self) {
        if let Self::Wayland(window) = self {
            window.on_commit();
        }
    }
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        match self {
            Self::Wayland(window) => window.geometry(),
            Self::X11(surface) => bbox_from_surface_tree(surface, (0, 0)),
        }
    }
    pub fn surface_under(&self, position: Point<f64, Logical>, types: WindowSurfaceType) -> Option<(WlSurface, Point<i32, Logical>)> {
        match self {
            Self::Wayland(window) => window.surface_under(position, types),
            Self::X11(surface) => under_from_surface_tree(surface, position, (0, 0), types),
        }
    }
    pub fn with_surfaces(&self, mut f: impl FnMut(&WlSurface, &SurfaceData)) {
        match self {
            Self::Wayland(window) => window.with_surfaces(f),
            Self::X11(surface) => with_surfaces_surface_tree(surface, &mut f),
        }
    }
    pub fn send_frame(&self, output: &Output, time: Duration, throttle: Option<Duration>, f: impl FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy) {
        match self {
            Self::Wayland(window) => window.send_frame(output, time, throttle, f),
            Self::X11(surface) => send_frames_surface_tree(surface, output, time, throttle, f),
        }
    }
    pub fn render_elements(&self, renderer: &mut GlesRenderer) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
        match self {
            Self::Wayland(window) => window.render_elements(renderer, (0, 0).into(), 1.0.into(), 1.0),
            Self::X11(surface) => render_elements_from_surface_tree(renderer, surface, (0, 0), 1.0, 1.0, Kind::Unspecified),
        }
    }
}
