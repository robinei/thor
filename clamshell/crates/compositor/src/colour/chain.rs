//! The passes a window's picture takes before the scene when its clarity settings need them
//! (`docs/picture.md`): SMAA at the game's resolution ([`super::smaa`]), then with FSR and sharpening, FSR's
//! upscale (EASU) into a texture of the size the window is shown at, so that the final pass can sharpen
//! what it upscaled with FSR's RCAS. Each window's textures are kept between frames and drawn again only
//! when its buffer changes. The window's element is then the last of them, with the surface's own id, so
//! that its colour, its curve and its presentation are found as before; without an upscale here the final
//! pass scales it as it would the buffer.

use std::collections::HashMap;

use clamshell_ui::Error;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Frame, Offscreen, Renderer, Texture,
            element::{Element as ElementTrait, Id, Kind, texture::TextureRenderElement},
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType},
            utils::{CommitCounter, DamageBag},
        },
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Buffer, Rectangle, Scale, Size, Transform},
};

use crate::render::Element;

/// A window's textures: SMAA's at the game's resolution if it anti-aliases, the upscaled one at the size
/// it is shown if it sharpens FSR's upscale, and the buffer commit they were drawn from.
struct Chain {
    smaa: Option<SmaaTextures>,
    upscaled: Option<GlesTexture>,
    /// The size of the texture shown last: the upscaled one's, else SMAA's.
    size: Size<i32, Buffer>,
    commit: Option<CommitCounter>,
    damage: DamageBag<i32, Buffer>,
}

/// SMAA's textures for a window: the picture copied out of its buffer, the edges, the blending weights,
/// and the anti-aliased picture, all at the buffer's size.
struct SmaaTextures {
    color: GlesTexture,
    edges: GlesTexture,
    blend: GlesTexture,
    out: GlesTexture,
    size: Size<i32, Buffer>,
}

#[derive(Default)]
pub(crate) struct Chains {
    chains: HashMap<Id, Chain>,
    easu: Option<GlesTexProgram>,
    smaa: Option<super::smaa::Smaa>,
}

/// What a window's surface element needs for its passes: the surface, whose buffer's texture they read,
/// whether it is anti-aliased, and whether it is upscaled to be sharpened.
pub(crate) struct Want<'a> {
    pub surface: &'a WlSurface,
    pub smaa: bool,
    pub upscale: bool,
}

impl Chains {
    /// Runs the passes of every surface element that `wants` them, replacing its element in `elements` with
    /// the texture they give, and returns the ids of those upscaled here, each with the texel size of its
    /// texture (one pixel of the size it is shown). The textures of windows no longer wanting passes are
    /// dropped.
    pub fn run<'a>(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &mut [Element],
        wants: impl Fn(&Id) -> Option<Want<'a>>,
    ) -> Result<Vec<(Id, [f32; 2])>, Error> {
        let mut kept = Vec::new();
        let mut upscaled = Vec::new();
        for element in elements.iter_mut() {
            let Some(want) = wants(element.id()) else { continue };
            let geometry = element.geometry(Scale::from(1.0));
            let source =
                smithay::backend::renderer::utils::with_renderer_surface_state(want.surface, |s| s.texture::<GlesTexture>(renderer.context_id()).cloned())
                    .flatten();
            let Some(source) = source.filter(|_| geometry.size.w > 0 && geometry.size.h > 0) else { continue };
            let id = element.id().clone();
            let commit = element.current_commit();
            let shown: Size<i32, Buffer> = (geometry.size.w, geometry.size.h).into();
            let mut chain = self.chains.remove(&id).unwrap_or(Chain { smaa: None, upscaled: None, size: shown, commit: None, damage: DamageBag::default() });
            let source_size = source.size();
            // The textures as the settings and sizes need them now; any that change are drawn again.
            if want.smaa != chain.smaa.is_some() || chain.smaa.as_ref().is_some_and(|t| t.size != source_size) {
                chain.smaa = want.smaa.then(|| smaa_textures(renderer, source_size)).transpose()?;
                chain.commit = None;
            }
            let upscaled_size = chain.upscaled.as_ref().map(|t| t.size());
            if want.upscale != upscaled_size.is_some() || upscaled_size.is_some_and(|size| size != shown) {
                chain.upscaled = want.upscale.then(|| renderer.create_buffer(Fourcc::Abgr16161616f, shown)).transpose()?;
                chain.commit = None;
            }
            if chain.commit != Some(commit) {
                self.draw(renderer, &mut chain, &source)?;
                chain.commit = Some(commit);
            }
            let texture = match (&chain.upscaled, &chain.smaa) {
                (Some(texture), _) => texture.clone(),
                (None, Some(smaa)) => smaa.out.clone(),
                (None, None) => continue,
            };
            *element = Element::Ui(TextureRenderElement::from_texture_with_damage(
                id.clone(),
                renderer.context_id(),
                geometry.loc.to_f64(),
                texture,
                1,
                Transform::Normal,
                None,
                None,
                Some((geometry.size.w, geometry.size.h).into()),
                None,
                chain.damage.snapshot(),
                Kind::Unspecified,
            ));
            if chain.upscaled.is_some() {
                upscaled.push((id.clone(), [1.0 / shown.w as f32, 1.0 / shown.h as f32]));
            }
            kept.push(id.clone());
            self.chains.insert(id, chain);
        }
        self.chains.retain(|id, _| kept.contains(id));
        Ok(upscaled)
    }

    /// Draws a window's passes from its buffer's texture `source`: SMAA, then the upscale, each if wanted.
    fn draw(&mut self, renderer: &mut GlesRenderer, chain: &mut Chain, source: &GlesTexture) -> Result<(), Error> {
        let mut input = source.clone();
        if let Some(t) = &mut chain.smaa {
            if self.smaa.is_none() {
                self.smaa = Some(super::smaa::Smaa::new(renderer)?);
            }
            // SMAA reads an ordinary texture: the buffer's copied out first, as Smithay draws it (its own
            // program takes external and turned buffers).
            copy(renderer, &mut t.color, source)?;
            if let Some(smaa) = &self.smaa {
                smaa.run(renderer, &t.color, &t.edges, &t.blend, &t.out, (t.size.w, t.size.h))?;
            }
            input = t.out.clone();
        }
        if let Some(target) = &mut chain.upscaled {
            let program = match &self.easu {
                Some(program) => program.clone(),
                None => {
                    let program = easu_program(renderer)?;
                    self.easu = Some(program.clone());
                    program
                }
            };
            let texel = [1.0 / input.size().w as f32, 1.0 / input.size().h as f32];
            render(renderer, target, &input, Some((&program, [Uniform::new("clarity", [texel[0], texel[1], 0.0, 2.0])])))?;
        }
        chain.size = chain.upscaled.as_ref().map_or_else(|| chain.smaa.as_ref().map_or(chain.size, |t| t.size), |t| t.size());
        chain.damage.add([Rectangle::from_size(chain.size)]);
        Ok(())
    }
}

/// SMAA's textures at `size`: the copied picture and the result in half floats, as the scene is, the edges
/// and the weights in bytes, as SMAA's own are.
fn smaa_textures(renderer: &mut GlesRenderer, size: Size<i32, Buffer>) -> Result<SmaaTextures, Error> {
    Ok(SmaaTextures {
        color: renderer.create_buffer(Fourcc::Abgr16161616f, size)?,
        edges: renderer.create_buffer(Fourcc::Abgr8888, size)?,
        blend: renderer.create_buffer(Fourcc::Abgr8888, size)?,
        out: renderer.create_buffer(Fourcc::Abgr16161616f, size)?,
        size,
    })
}

/// `source` copied into `target` as Smithay draws a texture.
fn copy(renderer: &mut GlesRenderer, target: &mut GlesTexture, source: &GlesTexture) -> Result<(), Error> {
    render(renderer, target, source, None)
}

/// `source` drawn over all of `target`, by `program` with its uniform, or as Smithay draws a texture.
fn render(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    source: &GlesTexture,
    program: Option<(&GlesTexProgram, [Uniform<'static>; 1])>,
) -> Result<(), Error> {
    let size = target.size();
    let size = (size.w, size.h).into();
    let mut bound = renderer.bind(target)?;
    let mut frame = renderer.render(&mut bound, size, Transform::Normal)?;
    let whole = Rectangle::from_size(size);
    frame.clear([0.0, 0.0, 0.0, 0.0].into(), &[whole])?;
    let (program, uniforms) = match &program {
        Some((program, uniforms)) => (Some(*program), uniforms.as_slice()),
        None => (None, &[][..]),
    };
    frame.render_texture_from_to(source, Rectangle::from_size(source.size()).to_f64(), whole, &[whole], &[], Transform::Normal, 1.0, program, uniforms)?;
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
