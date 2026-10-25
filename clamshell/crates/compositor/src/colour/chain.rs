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
    /// FXAA's picture at the buffer's size, the right way up.
    fxaa: Option<GlesTexture>,
    upscaled: Option<GlesTexture>,
    /// The size of the texture shown last: the upscaled one's, else SMAA's.
    size: Size<i32, Buffer>,
    commit: Option<CommitCounter>,
    damage: DamageBag<i32, Buffer>,
}

/// SMAA's textures for a window: the edges, the blending weights and the anti-aliased picture, all at the
/// buffer's size, the picture in half floats for HDR content and in bytes for SDR; and the buffer copied
/// out, for one that can't be read as it is (see [`Read`]).
struct SmaaTextures {
    edges: GlesTexture,
    blend: GlesTexture,
    out: GlesTexture,
    copy: Option<GlesTexture>,
    size: Size<i32, Buffer>,
    hdr: bool,
}

/// How SMAA reads a window's buffer: as it is, an ordinary texture or an external one (a dmabuf the GPU
/// can't render to, as Smithay tells them apart), or copied out first, as Smithay draws it, when neither
/// is sure (a buffer its client turned, or one neither in shared memory nor a dmabuf).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    Ordinary,
    External,
    Copy,
}

#[derive(Default)]
pub(crate) struct Chains {
    chains: HashMap<Id, Chain>,
    easu: Option<Result<GlesTexProgram, String>>,
    fxaa: Option<Result<GlesTexProgram, String>>,
    /// SMAA's programs once made, or why they couldn't be, which isn't tried again.
    smaa: Option<Result<super::smaa::Smaa, String>>,
    /// The windows whose passes failed, logged once.
    failed: std::collections::HashSet<Id>,
}

/// What a window's surface element needs for its passes: the surface, whose buffer's texture they read,
/// whether it is anti-aliased, whether it is upscaled to be sharpened, and whether its content is HDR.
pub(crate) struct Want<'a> {
    pub surface: &'a WlSurface,
    pub smaa: bool,
    pub fxaa: bool,
    pub upscale: bool,
    pub hdr: bool,
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
            let context = renderer.context_id();
            let formats = renderer.egl_context().dmabuf_render_formats();
            let source = smithay::backend::renderer::utils::with_renderer_surface_state(want.surface, |s| {
                Some((s.texture::<GlesTexture>(context)?.clone(), read(s, formats)))
            })
            .flatten();
            let Some((source, read)) = source.filter(|_| geometry.size.w > 0 && geometry.size.h > 0) else { continue };
            let id = element.id().clone();
            let commit = element.current_commit();
            let shown: Size<i32, Buffer> = (geometry.size.w, geometry.size.h).into();
            let mut chain =
                self.chains.remove(&id).unwrap_or(Chain { smaa: None, fxaa: None, upscaled: None, size: shown, commit: None, damage: DamageBag::default() });
            // SMAA's picture is in the buffer's own orientation: one upside down is turned when shown.
            let flipped = read != Read::Copy && source.is_y_inverted();
            // A window whose passes fail is shown without them, as its buffer is; the failure is logged
            // once, and again only after they have worked.
            if let Err(error) = self.prepare(renderer, &mut chain, &want, &source, read, flipped, shown, commit) {
                if self.failed.insert(id.clone()) {
                    log::warn!("compositor: a window's picture passes failed, shown without them: {error}");
                }
                continue;
            }
            self.failed.remove(&id);
            let (texture, transform) = match (&chain.upscaled, &chain.smaa, &chain.fxaa) {
                (Some(texture), _, _) => (texture.clone(), Transform::Normal),
                (None, Some(smaa), _) => (smaa.out.clone(), if flipped { Transform::Flipped180 } else { Transform::Normal }),
                (None, None, Some(fxaa)) => (fxaa.clone(), Transform::Normal),
                (None, None, None) => continue,
            };
            *element = Element::Ui(TextureRenderElement::from_texture_with_damage(
                id.clone(),
                renderer.context_id(),
                geometry.loc.to_f64(),
                texture,
                1,
                transform,
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

    /// Makes a window's textures as its settings and sizes need them now, and draws its passes when they
    /// changed or its buffer did.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        renderer: &mut GlesRenderer,
        chain: &mut Chain,
        want: &Want<'_>,
        source: &GlesTexture,
        read: Read,
        flipped: bool,
        shown: Size<i32, Buffer>,
        commit: CommitCounter,
    ) -> Result<(), Error> {
        let source_size = source.size();
        let smaa_now = chain.smaa.as_ref().map(|t| (t.size, t.hdr, t.copy.is_some()));
        if want.smaa != smaa_now.is_some() || smaa_now.is_some_and(|now| now != (source_size, want.hdr, read == Read::Copy)) {
            chain.smaa = None;
            chain.commit = None;
            chain.smaa = want.smaa.then(|| smaa_textures(renderer, source_size, want.hdr, read == Read::Copy)).transpose()?;
        }
        let picture = if want.hdr { Fourcc::Abgr16161616f } else { Fourcc::Abgr8888 };
        let fxaa_now = chain.fxaa.as_ref().map(|t| (t.size(), t.format()));
        if want.fxaa != fxaa_now.is_some() || fxaa_now.is_some_and(|now| now != (source_size, Some(picture))) {
            chain.fxaa = None;
            chain.commit = None;
            chain.fxaa = want.fxaa.then(|| renderer.create_buffer(picture, source_size)).transpose()?;
        }
        let upscaled_size = chain.upscaled.as_ref().map(|t| t.size());
        if want.upscale != upscaled_size.is_some() || upscaled_size.is_some_and(|size| size != shown) {
            chain.upscaled = None;
            chain.commit = None;
            chain.upscaled = want.upscale.then(|| renderer.create_buffer(Fourcc::Abgr16161616f, shown)).transpose()?;
        }
        if chain.commit != Some(commit) {
            chain.commit = None;
            self.draw(renderer, chain, source, read, flipped)?;
            chain.commit = Some(commit);
        }
        Ok(())
    }

    /// Draws a window's passes from its buffer's texture `source`, read as `read` (and upside down if
    /// `flipped`): SMAA, then the upscale, each if wanted.
    fn draw(&mut self, renderer: &mut GlesRenderer, chain: &mut Chain, source: &GlesTexture, read: Read, flipped: bool) -> Result<(), Error> {
        let mut input = source.clone();
        let mut input_turn = Transform::Normal;
        if let Some(t) = &mut chain.smaa {
            if self.smaa.is_none() {
                self.smaa = Some(super::smaa::Smaa::new(renderer).map_err(|e| e.to_string()));
            }
            let color = match &mut t.copy {
                Some(copy) => {
                    render(renderer, copy, source, None, Transform::Normal)?;
                    copy.clone()
                }
                None => source.clone(),
            };
            match &mut self.smaa {
                Some(Ok(smaa)) => smaa.run(renderer, (&color, read == Read::External), [&t.edges, &t.blend, &t.out], (t.size.w, t.size.h))?,
                Some(Err(error)) => return Err(error.clone().into()),
                None => {}
            }
            input = t.out.clone();
            if flipped {
                input_turn = Transform::Flipped180;
            }
        }
        if let Some(target) = &mut chain.fxaa {
            if self.fxaa.is_none() {
                self.fxaa = Some(pass_program(renderer, "fxaa", super::render::FXAA_GLSL).map_err(|e| e.to_string()));
            }
            let program = match &self.fxaa {
                Some(Ok(program)) => program.clone(),
                Some(Err(error)) => return Err(error.clone().into()),
                None => return Ok(()),
            };
            // Drawn as Smithay draws the buffer, so the right way up whatever the buffer is.
            let texel = [1.0 / source.size().w as f32, 1.0 / source.size().h as f32];
            render(renderer, target, source, Some((&program, [Uniform::new("clarity", [texel[0], texel[1], 0.0, 0.0])])), Transform::Normal)?;
            input = target.clone();
        }
        if let Some(target) = &mut chain.upscaled {
            if self.easu.is_none() {
                self.easu = Some(pass_program(renderer, "easu", super::render::EASU_GLSL).map_err(|e| e.to_string()));
            }
            let program = match &self.easu {
                Some(Ok(program)) => program.clone(),
                Some(Err(error)) => return Err(error.clone().into()),
                None => return Ok(()),
            };
            let texel = [1.0 / input.size().w as f32, 1.0 / input.size().h as f32];
            render(renderer, target, &input, Some((&program, [Uniform::new("clarity", [texel[0], texel[1], 0.0, 2.0])])), input_turn)?;
        }
        let anti_aliased = chain.smaa.as_ref().map(|t| t.size).or(chain.fxaa.as_ref().map(|t| t.size()));
        chain.size = chain.upscaled.as_ref().map(|t| t.size()).or(anti_aliased).unwrap_or(chain.size);
        chain.damage.add([Rectangle::from_size(chain.size)]);
        Ok(())
    }
}

/// SMAA's textures at `size`: the edges and weights in bytes, as SMAA's own are; the picture, and the copy
/// of the buffer if it takes one, in half floats for HDR content, as the scene is, else in bytes.
fn smaa_textures(renderer: &mut GlesRenderer, size: Size<i32, Buffer>, hdr: bool, copy: bool) -> Result<SmaaTextures, Error> {
    let picture = if hdr { Fourcc::Abgr16161616f } else { Fourcc::Abgr8888 };
    Ok(SmaaTextures {
        edges: renderer.create_buffer(Fourcc::Abgr8888, size)?,
        blend: renderer.create_buffer(Fourcc::Abgr8888, size)?,
        out: renderer.create_buffer(picture, size)?,
        copy: copy.then(|| renderer.create_buffer(picture, size)).transpose()?,
        size,
        hdr,
    })
}

/// How SMAA reads the buffer of a surface whose state is `state` (see [`Read`]), `formats` the dmabuf
/// formats the GPU renders to.
fn read(state: &smithay::backend::renderer::utils::RendererSurfaceState, formats: &smithay::backend::allocator::format::FormatSet) -> Read {
    use smithay::backend::renderer::{BufferType, buffer_type};
    if state.buffer_transform() != Transform::Normal {
        return Read::Copy;
    }
    let Some(buffer) = state.buffer() else { return Read::Copy };
    match buffer_type(buffer) {
        Some(BufferType::Shm) => Read::Ordinary,
        Some(BufferType::Dma) => match smithay::wayland::dmabuf::get_dmabuf(buffer) {
            Ok(dmabuf) if formats.contains(&smithay::backend::allocator::Buffer::format(dmabuf)) => Read::Ordinary,
            Ok(_) => Read::External,
            Err(_) => Read::Copy,
        },
        _ => Read::Copy,
    }
}

/// `source`, turned by `turn`, drawn over all of `target`, by `program` with its uniform, or as Smithay
/// draws a texture.
fn render(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    source: &GlesTexture,
    program: Option<(&GlesTexProgram, [Uniform<'static>; 1])>,
    turn: Transform,
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
    frame.render_texture_from_to(source, Rectangle::from_size(source.size()).to_f64(), whole, &[whole], &[], turn, 1.0, program, uniforms)?;
    drop(frame.finish()?);
    Ok(())
}

/// A pass's program: `glsl` defining the function `main` draws with (`easu` or `fxaa`), its signal as it is
/// (decoding comes in the final pass).
fn pass_program(renderer: &mut GlesRenderer, main: &str, glsl: &str) -> Result<GlesTexProgram, Error> {
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
{glsl}
void main() {{
    gl_FragColor = {main}() * alpha;
}}"#
    );
    Ok(renderer.compile_custom_texture_shader(source, &[UniformName::new("clarity", UniformType::_4f)])?)
}
