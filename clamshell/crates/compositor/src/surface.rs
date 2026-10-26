//! A window's surfaces, Wayland's (an xdg toplevel in Smithay's `Window`) or X11's (Xwayland's surface,
//! with its override-redirect popups), behind one type, so drawing, hit testing and frame callbacks treat
//! both alike and none of them needs the X11 connection.

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
    /// The surface holding the window's picture: the topmost surface covering at least half as much of the
    /// screen as the largest. Wine draws a game in a subsurface above its window's own buffer, which covers
    /// the screen while the game, at a mode of another shape, covers less; Xwayland, SDL and Wine show a
    /// mode a game picked as a buffer a viewport stretches, or crops. Smaller surfaces above, such as
    /// overlays, aren't the picture.
    pub fn picture_surface(&self) -> Option<WlSurface> {
        let mut surfaces: Vec<(WlSurface, i64)> = Vec::new();
        // Topmost first, as Smithay goes through them.
        self.with_surfaces(|surface, states| {
            if let Some(view) = Self::view(states) {
                surfaces.push((surface.clone(), i64::from(view.dst.w) * i64::from(view.dst.h)));
            }
        });
        let largest = surfaces.iter().map(|(_, area)| *area).max()?;
        surfaces.into_iter().find(|(_, area)| area * 2 >= largest).map(|(surface, _)| surface)
    }

    /// The window's picture: the size of the part of its buffer shown, and where it is shown in the
    /// window, in its root surface's coordinates (see [`Surface::picture_surface`]).
    pub fn picture(&self) -> Option<(smithay::utils::Size<i32, smithay::utils::Logical>, smithay::utils::Rectangle<i32, smithay::utils::Logical>)> {
        let surface = self.picture_surface()?;
        let view = smithay::wayland::compositor::with_states(&surface, Self::view)?;
        let shown = (view.src.size.w.round() as i32, view.src.size.h.round() as i32).into();
        // Its offset from its parent's, and theirs up to the root's.
        let mut at = view.offset;
        let mut parent = smithay::wayland::compositor::get_parent(&surface);
        while let Some(surface) = parent {
            at += smithay::wayland::compositor::with_states(&surface, Self::view).map(|view| view.offset).unwrap_or_default();
            parent = smithay::wayland::compositor::get_parent(&surface);
        }
        Some((shown, smithay::utils::Rectangle::new(at, view.dst)))
    }

    /// How a surface with a buffer shows it.
    fn view(states: &SurfaceData) -> Option<smithay::backend::renderer::utils::SurfaceView> {
        let state = states.data_map.get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()?;
        let state = state.lock().ok()?;
        state.buffer_size()?;
        state.view()
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
