//! A surface drawn in software: a pixmap allocated once at its size and kept, and the texture it is shown
//! from, into which only the part drawn is uploaded. The UI's screens and overlays are drawn into these
//! (see `render::UiFrame`); anything else drawn on the CPU can be too, and placed in the scene as they are.

use clamshell_ui::{Bounds, Error};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::DamageBag;
use smithay::backend::renderer::{ImportMem, Renderer};
use smithay::utils::{Buffer, Rectangle, Transform};

use crate::render::Element;

pub(crate) struct SoftwareSurface {
    pixmap: tiny_skia::Pixmap,
    texture: Option<GlesTexture>,
    id: Id,
    damage: DamageBag<i32, Buffer>,
}

impl SoftwareSurface {
    pub fn new((w, h): (u32, u32)) -> Result<Self, Error> {
        Ok(Self {
            pixmap: tiny_skia::Pixmap::new(w, h).ok_or("compositor: software surface too large")?,
            texture: None,
            id: Id::new(),
            damage: DamageBag::default(),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Whether it has been drawn and uploaded at least once, so that it can be shown.
    pub fn drawn(&self) -> bool {
        self.texture.is_some()
    }

    /// Draws into the pixmap with `draw`, which returns the part it changed, if any; that part is uploaded
    /// and is the texture's damage. The first upload makes the texture, of all of it.
    pub fn draw(&mut self, renderer: &mut GlesRenderer, draw: impl FnOnce(&mut tiny_skia::Pixmap) -> Option<Bounds>) -> Result<(), Error> {
        let Some(bounds) = draw(&mut self.pixmap) else { return Ok(()) };
        let (w, h) = self.size();
        let damage = Rectangle::new((bounds.x0, bounds.y0).into(), (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0).into());
        match &self.texture {
            Some(texture) => renderer.update_memory(texture, self.pixmap.data(), damage)?,
            None => self.texture = Some(renderer.import_memory(self.pixmap.data(), Fourcc::Abgr8888, (w as i32, h as i32).into(), false)?),
        }
        self.damage.add([damage]);
        Ok(())
    }

    /// Its element at `x` in the scene: the whole of it, or only its `part`. None before it is drawn, or for
    /// a part outside it.
    pub fn element(&self, renderer: &GlesRenderer, x: i32, part: Option<Bounds>) -> Option<Element> {
        let (w, h) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let part = part.unwrap_or(Bounds::new(0, 0, w, h)).intersect(Bounds::new(0, 0, w, h));
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
