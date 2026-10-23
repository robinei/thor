//! UI pixmaps are retained; only the changed rectangle is uploaded into each texture.

use crate::server::Server;
use clamshell_ui::{Core, Error, Screen};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{AsRenderElements, Id, Kind, surface::WaylandSurfaceRenderElement, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexture},
            utils::DamageBag,
        },
    },
    utils::{Buffer, Rectangle, Transform},
};

smithay::backend::renderer::element::render_elements! {
    pub(crate) Element<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
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

pub(crate) fn elements(server: &mut Server, frames: &mut [UiFrame; 2], renderer: &mut GlesRenderer, out: &mut Vec<Element>) -> Result<(), Error> {
    let [top, bottom] = frames;
    top.update(&mut server.core, Screen::Top, renderer)?;
    bottom.update(&mut server.core, Screen::Bottom, renderer)?;
    server.core.frames_done();
    out.clear();
    let state = server.core.state();
    let show_bottom = state.bottom().shows_windows(state.session());
    if let Some(element) = bottom.element(renderer, server.top_width, show_bottom.then(|| server.area(true).loc.y)) {
        out.push(element);
    }
    if state.session().top_window().is_none()
        && let Some(element) = top.element(renderer, 0, None)
    {
        out.push(element);
    }
    for client in server.windows.iter().filter(|w| w.mapped && server.visible(w.id)) {
        let down = state.session().window(client.id).is_some_and(|w| w.down);
        let area = server.area(down);
        out.extend(
            client
                .window
                .render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, area.loc.to_physical(1), 1.0.into(), 1.0)
                .into_iter()
                .map(Element::from),
        );
    }
    Ok(())
}
