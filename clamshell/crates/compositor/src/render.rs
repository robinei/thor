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

/// A layer of the UI as composed: drawn into a software surface of the layer's size when it may have
/// changed. The surface is made when the layer is composed and dropped a while after it last was (see
/// `crate::hold`): a screen showing ClamShell alone from its own buffers composes none of its layers.
pub(crate) struct UiFrame {
    surface: Option<SoftwareSurface>,
    held: u64,
    hold: crate::hold::Hold,
    /// The layer last drawn, for the log.
    layer: Option<Layer>,
}

impl UiFrame {
    pub fn new() -> Self {
        Self { surface: None, held: 0, hold: Default::default(), layer: None }
    }

    pub fn update(&mut self, core: &mut Core, layer: impl Into<Layer>, renderer: &mut GlesRenderer) -> Result<(), Error> {
        let layer = layer.into();
        let (size, _) = core.size(layer);
        if self.surface.as_ref().is_none_or(|surface| surface.size() != size) {
            self.surface = Some(SoftwareSurface::new(size)?);
            self.held = 0;
            log::info!("compositor: {layer:?} composed, {:.1} MB", megabytes(size));
        }
        self.hold.used(std::time::Instant::now());
        self.layer = Some(layer);
        let Some(surface) = &mut self.surface else { return Ok(()) };
        // Recorded when stale; drawn when that changed something, or when the layer was drawn elsewhere
        // meanwhile (into a screen's scanout buffers), which this texture catches up with.
        if core.stale(layer) || !surface.drawn() {
            core.record(layer, false);
        }
        if !core.behind(layer, self.held) && surface.drawn() {
            return Ok(());
        }
        let held = &mut self.held;
        surface.draw(renderer, |pixmap| {
            let (frame, area) = core.draw(layer, pixmap, *held);
            *held = frame;
            (!area.is_empty()).then_some(area)
        })
    }

    /// Drops its surface if the layer hasn't been composed for a while by `now`. Returns when it will be
    /// dropped otherwise.
    pub fn trim(&mut self, now: std::time::Instant, trims: &mut crate::hold::Trims) {
        if let Some(surface) = &self.surface
            && self.hold.expired(now)
        {
            let layer = self.layer.map_or_else(|| "a layer".to_owned(), |layer| format!("{layer:?}"));
            log::info!("compositor: {layer} no longer composed, {:.1} MB dropped", megabytes(surface.size()));
            self.surface = None;
            self.held = 0;
            trims.dropped();
        }
        trims.held(&self.hold);
    }
}

/// A software surface's size in megabytes, at `size`: its pixmap and its texture, four bytes a pixel each.
fn megabytes((w, h): (u32, u32)) -> f64 {
    f64::from(w) * f64::from(h) * 8.0 / 1_000_000.0
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
/// but for the window `skip`, which the screen shows from its own buffer (see `window_scanout`), and the
/// overlay layer and the cursor while they are on planes of their own (`planed`, see `plane_layers`). The
/// screen's UI layers are drawn first, the top screen's not while a window covers it: that stays stale in
/// the core, and is drawn where it changed once uncovered (the texture holds the last frame drawn).
/// Drawing may change the state, so the scene is placed again after.
#[allow(clippy::too_many_arguments)]
pub(crate) fn elements(
    server: &mut Server,
    frames: &mut UiFrames,
    cursor: &mut crate::cursor::Cursor,
    renderer: &mut GlesRenderer,
    out: &mut Vec<Element>,
    down: bool,
    skip: Option<i64>,
    planed: crate::plane_layers::Planed,
) -> Result<(), Error> {
    server.place();
    let [top, bottom, overlay, overlay_below] = frames;
    if down {
        bottom.update(&mut server.core, Screen::Bottom, renderer)?;
        if !planed.overlay {
            overlay_below.update(&mut server.core, Layer::Overlay(Screen::Bottom), renderer)?;
        }
    } else {
        if !server.scene.covers_top() {
            top.update(&mut server.core, Screen::Top, renderer)?;
        }
        if !planed.overlay {
            overlay.update(&mut server.core, Layer::Overlay(Screen::Top), renderer)?;
        }
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
            Some(Shown::Software { layer: Layer::Overlay(_), .. }) if planed.overlay => continue,
            Some(Shown::Software { layer, x, part }) => {
                if let Some(element) = frame(frames, layer).and_then(|frame| frame.surface.as_ref()?.element(renderer, x, part)) {
                    out.push(element);
                }
                continue;
            }
            Some(Shown::Cursor { at }) if !on_screen(at.x) || planed.cursor => continue,
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
