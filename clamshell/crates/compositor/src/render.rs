//! UI pixmaps are retained; only the changed rectangle is uploaded into each texture.

use crate::server::Server;
use clamshell_ui::{Core, Error, Screen};
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

    pub fn update(&mut self, core: &mut Core, screen: Screen, renderer: &mut GlesRenderer) -> Result<(), Error> {
        let (size, _) = core.size(screen);
        if self.pixmap.width() != size.0 || self.pixmap.height() != size.1 {
            *self = Self::new(size)?;
        }
        if !core.stale(screen) && self.texture.is_some() {
            return Ok(());
        }
        let Some(bounds) = core.record(screen, self.texture.is_none()) else { return Ok(()) };
        self.held = core.draw(screen, self.pixmap.as_mut(), self.held, bounds);
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

    fn element(&self, renderer: &GlesRenderer, x: i32, height: Option<i32>) -> Option<Element> {
        let height = height.unwrap_or(self.pixmap.height() as i32).clamp(1, self.pixmap.height() as i32);
        let size = (self.pixmap.width() as i32, height).into();
        Some(
            TextureRenderElement::from_texture_with_damage(
                self.id.clone(),
                renderer.context_id(),
                (x as f64, 0.0),
                self.texture.clone()?,
                1,
                Transform::Normal,
                None,
                Some(Rectangle::from_size(size).to_f64()),
                Some(size),
                None,
                self.damage.snapshot(),
                Kind::Unspecified,
            )
            .into(),
        )
    }
}

/// What the compositor draws over the windows on the top screen: the window arrows while Home is held,
/// and the HUD.
pub(crate) struct Overlays {
    arrows: UiFrame,
    last_arrows: Option<clamshell_ui::window_arrows::WindowArrows>,
    hud: UiFrame,
    last_hud: Option<clamshell_ui::hud::HudLine>,
    hud_cache: clamshell_ui::DrawCache,
}
impl Overlays {
    pub fn new(size: (u32, u32)) -> Result<Self, Error> {
        Ok(Self {
            arrows: UiFrame::new(size)?,
            last_arrows: None,
            hud: UiFrame::new(clamshell_ui::hud::HudLine::size(size.1))?,
            last_hud: None,
            hud_cache: clamshell_ui::DrawCache::load(),
        })
    }
    fn update(&mut self, server: &Server, renderer: &mut GlesRenderer, out: &mut Vec<Element>) -> Result<(), Error> {
        let size = (server.top_width as u32, server.size.1 as u32);
        let arrows = clamshell_ui::window_arrows::WindowArrows::new(server.core.state(), size);
        if self.arrows.pixmap.width() != size.0 || self.arrows.pixmap.height() != size.1 {
            *self = Self::new(size)?;
        }
        if self.last_arrows != Some(arrows) {
            if arrows.visible {
                arrows.draw(self.arrows.pixmap.as_mut());
                self.arrows.upload(renderer, clamshell_ui::Bounds::of_size(size))?;
            }
            self.last_arrows = Some(arrows);
        }
        if arrows.visible
            && let Some(element) = self.arrows.element(renderer, 0, None)
        {
            out.push(element);
        }
        let hud = clamshell_ui::hud::HudLine::of(server.core.state(), size.1);
        if self.last_hud != hud {
            if let Some(hud) = &hud {
                hud.draw(self.hud.pixmap.as_mut(), &mut self.hud_cache);
                let (w, h) = (self.hud.pixmap.width(), self.hud.pixmap.height());
                self.hud.upload(renderer, clamshell_ui::Bounds::of_size((w, h)))?;
            }
            self.last_hud = hud;
        }
        if self.last_hud.is_some()
            && let Some(element) = self.hud.element(renderer, 0, None)
        {
            out.push(element);
        }
        Ok(())
    }
}

pub(crate) fn elements(
    server: &mut Server,
    frames: &mut [UiFrame; 2],
    overlays: &mut Overlays,
    renderer: &mut GlesRenderer,
    out: &mut Vec<Element>,
) -> Result<(), Error> {
    let [top, bottom] = frames;
    // The top screen's view isn't shown while a window covers it, so it isn't drawn either: it stays stale
    // in the core, and is drawn where it changed once uncovered (the texture holds the last frame drawn).
    let state = server.core.state();
    let covered = server.windows.iter().any(|w| w.mapped && server.visible(w.id) && state.session().window(w.id).is_some_and(|w| !w.down));
    if !covered {
        top.update(&mut server.core, Screen::Top, renderer)?;
    }
    bottom.update(&mut server.core, Screen::Bottom, renderer)?;
    server.core.frames_done();
    out.clear();
    overlays.update(server, renderer, out)?;
    let state = server.core.state();
    let show_bottom = state.bottom().shows_windows(state.session());
    if let Some(element) = bottom.element(renderer, server.top_width, show_bottom.then(|| server.area(true).loc.y)) {
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

/// Readback is an SDR file export; clients and the composition target retain their native precision.
pub(crate) fn capture(
    renderer: &mut GlesRenderer,
    target: &smithay::backend::renderer::gles::GlesTarget<'_>,
    size: (i32, i32),
) -> Result<crate::Capture, String> {
    use smithay::backend::renderer::{ExportMem, TextureMapping};
    let width = u32::try_from(size.0).map_err(|e| e.to_string())?;
    let height = u32::try_from(size.1).map_err(|e| e.to_string())?;
    let stride = usize::try_from(width).ok().and_then(|w| w.checked_mul(4)).filter(|n| *n != 0).ok_or("invalid capture width")?;
    let mapping = renderer.copy_framebuffer(target, Rectangle::from_size(size.into()), Fourcc::Abgr8888).map_err(|e| e.to_string())?;
    let flipped = mapping.flipped();
    let bytes = renderer.map_texture(&mapping).map_err(|e| e.to_string())?;
    let mut rgba = Vec::with_capacity(bytes.len());
    if flipped {
        for row in bytes.chunks_exact(stride).rev() {
            rgba.extend_from_slice(row);
        }
    } else {
        rgba.extend_from_slice(bytes);
    }
    Ok(crate::Capture { size: (width, height), rgba })
}
