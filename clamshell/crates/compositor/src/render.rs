//! The scene's elements for a host to compose (see `scene`): the cursor, the UI's screens and overlays
//! from their software surfaces, and the windows shown, each scaled and cropped to its place by its geometry. The UI
//! draws each layer into its surface's pixmap, kept between frames ([`UiFrame`]); only the rectangle that
//! changed is uploaded, and is the texture's damage.

use crate::scene::Shown;
use crate::server::Server;
use crate::software::SoftwareSurface;
use clamshell_ui::{Core, Error, Layer, Screen};
use smithay::{
    backend::renderer::{
        element::{
            Kind,
            solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement,
            texture::TextureRenderElement,
            utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
        },
        gles::{GlesRenderer, GlesTexture},
        utils::{CommitCounter, import_surface_tree},
    },
    desktop::PopupManager,
};

smithay::backend::renderer::element::render_elements! {
    pub(crate) Element<=GlesRenderer>;
    Surface=CropRenderElement<RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>>,
    Cursor=WaylandSurfaceRenderElement<GlesRenderer>,
    Ui=TextureRenderElement<GlesTexture>,
    Dim=SolidColorRenderElement,
}

/// A layer of the UI, drawn into a software surface of the layer's size when it may have changed.
pub(crate) struct UiFrame {
    surface: SoftwareSurface,
    held: u64,
}

impl UiFrame {
    pub fn new(size: (u32, u32)) -> Result<Self, Error> {
        Ok(Self { surface: SoftwareSurface::new(size)?, held: 0 })
    }

    pub fn update(&mut self, core: &mut Core, layer: impl Into<Layer>, renderer: &mut GlesRenderer) -> Result<(), Error> {
        let layer = layer.into();
        let (size, _) = core.size(layer);
        if self.surface.size() != size {
            *self = Self::new(size)?;
        }
        // Recorded when stale; drawn when that changed something, or when the layer was drawn elsewhere
        // meanwhile (into a screen's scanout buffers), which this texture catches up with.
        if core.stale(layer) || !self.surface.drawn() {
            core.record(layer, false);
        }
        if !core.behind(layer, self.held) && self.surface.drawn() {
            return Ok(());
        }
        let held = &mut self.held;
        self.surface.draw(renderer, |pixmap| {
            let (frame, area) = core.draw(layer, pixmap, *held);
            *held = frame;
            (!area.is_empty()).then_some(area)
        })
    }
}

/// The UI's frames: the top screen's, the second's, and their overlays'.
pub(crate) type UiFrames = [UiFrame; 4];

fn frame(frames: &UiFrames, layer: Layer) -> Option<&UiFrame> {
    frames.get(match layer {
        Layer::Screen(Screen::Top) => 0,
        Layer::Screen(Screen::Bottom) => 1,
        Layer::Overlay(Screen::Top) => 2,
        Layer::Overlay(Screen::Bottom) => 3,
    })
}

/// Screen `down`'s elements (the second's, or the top's), front first, as the scene lists what is shown,
/// but for the window `skip`, which the screen shows from its own buffer (see `window_scanout`). The
/// screen's UI layers are drawn first, the top screen's not while a window covers it: that stays stale in
/// the core, and is drawn where it changed once uncovered (the texture holds the last frame drawn).
/// Drawing may change the state, so the scene is placed again after.
pub(crate) fn elements(
    server: &mut Server,
    frames: &mut UiFrames,
    cursor: &mut crate::cursor::Cursor,
    renderer: &mut GlesRenderer,
    out: &mut Vec<Element>,
    down: bool,
    skip: Option<i64>,
) -> Result<(), Error> {
    server.place();
    let [top, bottom, overlay, overlay_below] = frames;
    if down {
        bottom.update(&mut server.core, Screen::Bottom, renderer)?;
        overlay_below.update(&mut server.core, Layer::Overlay(Screen::Bottom), renderer)?;
    } else {
        if !server.scene.covers_top() {
            top.update(&mut server.core, Screen::Top, renderer)?;
        }
        overlay.update(&mut server.core, Layer::Overlay(Screen::Top), renderer)?;
    }
    server.core.frames_done();
    let screen = if down { Screen::Bottom } else { Screen::Top };
    let top_width = server.screens.top_width;
    let on_screen = |x: f64| (x >= f64::from(top_width)) == down;
    server.place();
    out.clear();
    let mut dims = 0;
    for i in 0..server.scene.shown().len() {
        let placed = match server.scene.shown().get(i).copied() {
            Some(Shown::Software { layer, .. }) if !matches!(layer, Layer::Screen(s) | Layer::Overlay(s) if s == screen) => continue,
            Some(Shown::Software { layer, x, part }) => {
                if let Some(element) = frame(frames, layer).and_then(|frame| frame.surface.element(renderer, x, part)) {
                    out.push(element);
                }
                continue;
            }
            Some(Shown::Cursor { at }) if !on_screen(at.x) => continue,
            Some(Shown::Cursor { at }) => {
                cursor.elements(server, renderer, at, out)?;
                continue;
            }
            Some(Shown::Dim { area }) => {
                // Numbered across both screens, so that each keeps its id from frame to frame.
                let id = server.scene.dim_id(dims);
                dims += 1;
                if !on_screen(f64::from(area.loc.x)) {
                    continue;
                }
                out.push(
                    SolidColorRenderElement::new(id, area.to_physical(1), CommitCounter::default(), [0.0, 0.0, 0.0, clamshell_ui::DIM], Kind::Unspecified)
                        .into(),
                );
                continue;
            }
            Some(Shown::Window(placed)) if placed.below != down || skip == Some(placed.id) => continue,
            Some(Shown::Window(placed)) => placed,
            None => break,
        };
        let Some(client) = server.windows.iter().find(|w| w.id == placed.id) else { continue };
        let Some(surface) = client.window.root() else { continue };
        let surface = &surface;
        let imported = import_surface_tree(renderer, surface).and_then(|()| {
            for (popup, _) in PopupManager::popups_for_surface(surface) {
                import_surface_tree(renderer, popup.wl_surface())?;
            }
            for popup in client.window.x11_popups() {
                import_surface_tree(renderer, &popup.surface)?;
            }
            Ok(())
        });
        if let Err(error) = imported {
            server.reject_client(surface, &format!("cannot import window {} buffer: {error}", placed.id));
            continue;
        }
        let geometry = placed.geometry;
        client.window.for_each_element(renderer, |element| {
            let element = RescaleRenderElement::from_element(element, (0, 0).into(), geometry.scale);
            let element = RelocateRenderElement::from_element(element, (geometry.origin.x as i32, geometry.origin.y as i32), Relocate::Relative);
            if let Some(element) = CropRenderElement::from_element(element, 1.0, geometry.viewport.to_physical(1)) {
                out.push(element.into());
            }
        });
    }
    Ok(())
}
