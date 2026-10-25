//! The passes a window's picture takes before the scene when its clarity settings need them
//! (`docs/picture.md`): FSR's upscale (EASU) into a texture of the size the window is shown at, so that the
//! final pass can sharpen what it upscaled with FSR's RCAS. Each window's texture is kept between frames and
//! drawn again only when its buffer changes. The window's element is then this texture, with the surface's
//! own id, so that its colour, its curve and its presentation are found as before.

use std::collections::HashMap;

use clamshell_ui::Error;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Frame, Offscreen, Renderer,
            element::{Element as ElementTrait, Id, Kind, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType},
            utils::{CommitCounter, DamageBag},
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, Physical, Rectangle, Scale, Size, Transform},
};

use crate::render::Element;

/// A window's texture after its passes, at the size it is shown, and the buffer commit it was drawn from.
struct Chain {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    commit: Option<CommitCounter>,
    damage: DamageBag<i32, Buffer>,
}

#[derive(Default)]
pub(crate) struct Chains {
    chains: HashMap<Id, Chain>,
    easu: Option<GlesTexProgram>,
}

/// What a window's surface element needs for its passes: the surface, whose buffer's texture they read, and
/// the texel size of that buffer.
pub(crate) struct Want<'a> {
    pub surface: &'a WlSurface,
    pub texel: [f32; 2],
}

impl Chains {
    /// Runs the passes of every surface element that `wants` them, replacing its element in `elements` with
    /// the texture they give, and returns the ids replaced, each with the texel size of its texture (one
    /// pixel of the size it is shown). The textures of windows no longer wanting passes are dropped.
    pub fn run<'a>(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &mut [Element],
        wants: impl Fn(&Id) -> Option<Want<'a>>,
    ) -> Result<Vec<(Id, [f32; 2])>, Error> {
        let mut done = Vec::new();
        for element in elements.iter_mut() {
            let Some(want) = wants(element.id()) else { continue };
            let geometry = element.geometry(Scale::from(1.0));
            let Some(source) =
                smithay::backend::renderer::utils::with_renderer_surface_state(want.surface, |s| s.texture::<GlesTexture>(renderer.context_id()).cloned())
                    .flatten()
            else {
                continue;
            };
            if geometry.size.w <= 0 || geometry.size.h <= 0 {
                continue;
            }
            let id = element.id().clone();
            let commit = element.current_commit();
            let chain = match self.chains.remove(&id) {
                Some(chain) if chain.size == geometry.size => chain,
                _ => Chain {
                    texture: renderer.create_buffer(Fourcc::Abgr16161616f, (geometry.size.w, geometry.size.h).into())?,
                    size: geometry.size,
                    commit: None,
                    damage: DamageBag::default(),
                },
            };
            let mut chain = chain;
            if chain.commit != Some(commit) {
                let program = match &self.easu {
                    Some(program) => program.clone(),
                    None => {
                        let program = easu_program(renderer)?;
                        self.easu = Some(program.clone());
                        program
                    }
                };
                draw(renderer, &mut chain, &source, &program, want.texel)?;
                chain.commit = Some(commit);
                chain.damage.add([Rectangle::from_size((chain.size.w, chain.size.h).into())]);
            }
            *element = Element::Ui(TextureRenderElement::from_texture_with_damage(
                id.clone(),
                renderer.context_id(),
                geometry.loc.to_f64(),
                chain.texture.clone(),
                1,
                Transform::Normal,
                None,
                None,
                Some((chain.size.w, chain.size.h).into()),
                None,
                chain.damage.snapshot(),
                Kind::Unspecified,
            ));
            done.push((id.clone(), [1.0 / chain.size.w as f32, 1.0 / chain.size.h as f32]));
            self.chains.insert(id, chain);
        }
        self.chains.retain(|id, _| done.iter().any(|(kept, _)| kept == id));
        Ok(done)
    }
}

/// EASU from `source` (a texel of it `texel`) into the chain's texture, over all of it.
fn draw(renderer: &mut GlesRenderer, chain: &mut Chain, source: &GlesTexture, program: &GlesTexProgram, texel: [f32; 2]) -> Result<(), Error> {
    use smithay::backend::renderer::Texture;
    let size = chain.size;
    let mut target = renderer.bind(&mut chain.texture)?;
    let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
    let whole = Rectangle::from_size(size);
    frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[whole])?;
    let uniforms = [Uniform::new("clarity", [texel[0], texel[1], 0.0, 2.0])];
    frame.render_texture_from_to(
        source,
        Rectangle::from_size(source.size()).to_f64(),
        whole,
        &[whole],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &uniforms,
    )?;
    drop(frame.finish()?);
    Ok(())
}

/// The EASU pass's program: the picture upscaled, its signal as it is (decoding comes in the final pass).
fn easu_program(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, Error> {
    let source = format!(
        r#"#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
uniform vec4 clarity;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
{easu}
void main() {{
    gl_FragColor = easu() * alpha;
}}"#,
        easu = super::render::EASU_GLSL
    );
    Ok(renderer.compile_custom_texture_shader(source, &[UniformName::new("clarity", UniformType::_4f)])?)
}
