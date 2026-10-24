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
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{SurfaceData, get_parent},
        shell::xdg::ToplevelSurface,
    },
};
use std::time::Duration;

pub(crate) struct X11Popup {
    pub id: i64,
    pub surface: WlSurface,
    pub location: Point<i32, Logical>,
    pub geometry: crate::geometry::Geometry,
    pub input: bool,
}
pub(crate) struct X11Surface {
    pub root: WlSurface,
    pub popups: Vec<X11Popup>,
}
pub(crate) enum Surface {
    Wayland(Window),
    X11(X11Surface),
}
impl Surface {
    pub fn new_x11(root: WlSurface) -> Self {
        Self::X11(X11Surface { root, popups: Vec::new() })
    }
    pub fn toplevel(&self) -> Option<&ToplevelSurface> {
        match self {
            Self::Wayland(window) => window.toplevel(),
            Self::X11(_) => None,
        }
    }
    pub fn root(&self) -> Option<WlSurface> {
        match self {
            Self::Wayland(window) => window.toplevel().map(|t| t.wl_surface().clone()),
            Self::X11(window) => Some(window.root.clone()),
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
            Self::X11(window) => bbox_from_surface_tree(&window.root, (0, 0)),
        }
    }
    pub fn x11_popups(&self) -> &[X11Popup] {
        match self {
            Self::X11(window) => &window.popups,
            Self::Wayland(_) => &[],
        }
    }
    pub fn popup_surface(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        self.x11_popups().iter().any(|popup| popup.input && popup.surface == root)
    }
    pub fn contains_x11_target(&self, surface: &WlSurface) -> bool {
        let Self::X11(window) = self else { return true };
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        root == window.root || window.popups.iter().any(|popup| popup.surface == root)
    }
    pub fn dismiss_x11_popups(&mut self, dismissed: &mut Vec<i64>, mut remove: impl FnMut(&X11Popup) -> bool) {
        if let Self::X11(window) = self {
            window.popups.retain(|popup| {
                if remove(popup) {
                    dismissed.push(popup.id);
                    false
                } else {
                    true
                }
            });
        }
    }
    pub fn surface_under(&self, position: Point<f64, Logical>, types: WindowSurfaceType) -> Option<(WlSurface, Point<i32, Logical>)> {
        match self {
            Self::Wayland(window) => window.surface_under(position, types),
            Self::X11(window) => window
                .popups
                .iter()
                .rev()
                .filter(|popup| popup.input && popup.surface.is_alive())
                .find_map(|popup| under_from_surface_tree(&popup.surface, position, popup.location, types))
                .or_else(|| under_from_surface_tree(&window.root, position, (0, 0), types)),
        }
    }
    pub fn with_surfaces(&self, mut f: impl FnMut(&WlSurface, &SurfaceData)) {
        match self {
            Self::Wayland(window) => window.with_surfaces(f),
            Self::X11(window) => {
                with_surfaces_surface_tree(&window.root, &mut f);
                for popup in &window.popups {
                    with_surfaces_surface_tree(&popup.surface, &mut f);
                }
            }
        }
    }
    pub fn send_frame(&self, output: &Output, time: Duration, throttle: Option<Duration>, f: impl FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy) {
        match self {
            Self::Wayland(window) => window.send_frame(output, time, throttle, f),
            Self::X11(window) => {
                send_frames_surface_tree(&window.root, output, time, throttle, f);
                for popup in &window.popups {
                    send_frames_surface_tree(&popup.surface, output, time, throttle, f);
                }
            }
        }
    }
    pub fn for_each_element(&self, renderer: &mut GlesRenderer, mut f: impl FnMut(WaylandSurfaceRenderElement<GlesRenderer>)) {
        match self {
            Self::Wayland(window) => window.render_elements(renderer, (0, 0).into(), 1.0.into(), 1.0).into_iter().for_each(f),
            Self::X11(window) => {
                for popup in window.popups.iter().rev() {
                    render_elements_from_surface_tree(renderer, &popup.surface, (popup.location.x, popup.location.y), 1.0, 1.0, Kind::Unspecified)
                        .into_iter()
                        .for_each(&mut f);
                }
                render_elements_from_surface_tree(renderer, &window.root, (0, 0), 1.0, 1.0, Kind::Unspecified).into_iter().for_each(f);
            }
        }
    }
}
