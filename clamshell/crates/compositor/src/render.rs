//! The scene's elements for a host to compose: the UI's screens and overlays as textures, and the windows
//! shown, each scaled and cropped to its place by its geometry. The UI draws each screen into a pixmap kept
//! between frames ([`UiFrame`]); only the rectangle that changed is uploaded into its texture, and is the
//! texture's damage.

use crate::server::Server;
use clamshell_ui::{Core, Error, Layer, Screen};
use smithay::{
    backend::renderer::utils::import_surface_tree,
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Id, Kind,
                surface::WaylandSurfaceRenderElement,
                texture::TextureRenderElement,
                utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement},
            },
            gles::{GlesRenderer, GlesTexture},
            utils::DamageBag,
        },
    },
    desktop::PopupManager,
    utils::{Buffer, Rectangle, Transform},
};

smithay::backend::renderer::element::render_elements! {
    pub(crate) Element<=GlesRenderer>;
    Surface=CropRenderElement<RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>>,
    Cursor=WaylandSurfaceRenderElement<GlesRenderer>,
    Ui=TextureRenderElement<GlesTexture>,
}

pub(crate) struct UiFrame {
    pixmap: tiny_skia::Pixmap,
    held: u64,
    texture: Option<GlesTexture>,
    id: Id,
    damage: DamageBag<i32, Buffer>,
}

impl UiFrame {
    pub fn new(size: (u32, u32)) -> Result<Self, Error> {
        Ok(Self {
            pixmap: tiny_skia::Pixmap::new(size.0, size.1).ok_or("compositor: UI pixmap too large")?,
            held: 0,
            texture: None,
            id: Id::new(),
            damage: DamageBag::default(),
        })
    }

    pub fn update(&mut self, core: &mut Core, layer: impl Into<Layer>, renderer: &mut GlesRenderer) -> Result<(), Error> {
        let layer = layer.into();
        let (size, _) = core.size(layer);
        if self.pixmap.width() != size.0 || self.pixmap.height() != size.1 {
            *self = Self::new(size)?;
        }
        if !core.stale(layer) && self.texture.is_some() {
            return Ok(());
        }
        let Some(bounds) = core.record(layer, self.texture.is_none()) else { return Ok(()) };
        self.held = core.draw(layer, self.pixmap.as_mut(), self.held, bounds);
        self.upload(renderer, bounds)
    }

    fn upload(&mut self, renderer: &mut GlesRenderer, bounds: clamshell_ui::Bounds) -> Result<(), Error> {
        let size = (self.pixmap.width(), self.pixmap.height());
        let damage = Rectangle::new((bounds.x0, bounds.y0).into(), (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0).into());
        match &self.texture {
            Some(texture) => renderer.update_memory(texture, self.pixmap.data(), damage)?,
            None => self.texture = Some(renderer.import_memory(self.pixmap.data(), Fourcc::Abgr8888, (size.0 as i32, size.1 as i32).into(), false)?),
        }
        self.damage.add([damage]);
        Ok(())
    }

    /// The texture shown at `x` in the scene, the whole of it, or only its `part`.
    fn element(&self, renderer: &GlesRenderer, x: i32, part: Option<clamshell_ui::Bounds>) -> Option<Element> {
        let (w, h) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let part = part.unwrap_or(clamshell_ui::Bounds::new(0, 0, w, h)).intersect(clamshell_ui::Bounds::new(0, 0, w, h));
        if part.is_empty() {
            return None;
        }
        let shown = Rectangle::<i32, smithay::utils::Logical>::new((part.x0, part.y0).into(), (part.width(), part.height()).into());
        Some(
            TextureRenderElement::from_texture_with_damage(
                self.id.clone(),
                renderer.context_id(),
                ((x + part.x0) as f64, part.y0 as f64),
                self.texture.clone()?,
                1,
                Transform::Normal,
                None,
                Some(shown.to_f64()),
                Some(shown.size),
                None,
                self.damage.snapshot(),
                Kind::Unspecified,
            )
            .into(),
        )
    }
}

/// The scene's elements, front first: the screens' overlays (over the top screen's window the HUD and
/// the window arrows, over the second screen the notifications), the UI's screens, and the windows
/// shown. `frames` are the top screen's, the second's, and their overlays'.
pub(crate) fn elements(server: &mut Server, frames: &mut [UiFrame; 4], renderer: &mut GlesRenderer, out: &mut Vec<Element>) -> Result<(), Error> {
    let [top, bottom, overlay, overlay_below] = frames;
    // The top screen's view isn't shown while a window covers it, so it isn't drawn either: it stays stale
    // in the core, and is drawn where it changed once uncovered (the texture holds the last frame drawn).
    let state = server.core.state();
    let covered = server.windows.iter().any(|w| w.mapped && server.visible(w.id) && state.session().window(w.id).is_some() && !state.session().below(w.id));
    if !covered {
        top.update(&mut server.core, Screen::Top, renderer)?;
    }
    bottom.update(&mut server.core, Screen::Bottom, renderer)?;
    overlay.update(&mut server.core, Layer::Overlay(Screen::Top), renderer)?;
    overlay_below.update(&mut server.core, Layer::Overlay(Screen::Bottom), renderer)?;
    server.core.frames_done();
    out.clear();
    for (frame, screen, x) in [(&*overlay_below, Screen::Bottom, server.screens.top_width), (&*overlay, Screen::Top, 0)] {
        if let Some(area) = server.core.overlay_area(screen)
            && let Some(element) = frame.element(renderer, x, Some(area))
        {
            out.push(element);
        }
    }
    let state = server.core.state();
    let show_bottom = state.bottom().shows_windows(state.session());
    let above_windows = show_bottom.then(|| clamshell_ui::Bounds::new(0, 0, i32::MAX, server.area(true).loc.y.max(1)));
    if let Some(element) = bottom.element(renderer, server.screens.top_width, above_windows) {
        out.push(element);
    }
    if !covered && let Some(element) = top.element(renderer, 0, None) {
        out.push(element);
    }
    for client in server.windows.iter().filter(|w| w.mapped && server.visible(w.id)) {
        let Some(geometry) = server.geometry(client.id) else { continue };
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
            server.reject_client(surface, &format!("cannot import window {} buffer: {error}", client.id));
            continue;
        }
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
