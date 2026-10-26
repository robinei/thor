//! Surfaces decode into a retained half-float scene before the host encoding is applied once.
use crate::{render::Element, server::Server};
use clamshell_state::colour::{ColourDescription, ColourPrimaries, TransferFunction};
use clamshell_state::curve::{Curve, REFERENCE_NITS};
use clamshell_ui::Error;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Frame, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
            element::{Element as ElementTrait, Id, Kind, RenderElement, RenderElementStates},
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTarget, GlesTexProgram, GlesTexture, Uniform, UniformName, UniformType},
            utils::{CommitCounter, DamageSet, OpaqueRegions},
        },
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Transform, user_data::UserDataMap},
};

pub(crate) struct Pipeline {
    size: (i32, i32),
    texture: GlesTexture,
    damage: OutputDamageTracker,
    shaders: Vec<(ColourDescription, GlesTexProgram)>,
    converted: Vec<Converted>,
    /// Each screen's encoding (the top's, the second's), and its shader.
    outputs: [(ColourDescription, GlesTexProgram); 2],
    revision: u64,
    first: bool,
    /// Each shown window's surfaces with how the game running it is shown, this frame and the last: a change
    /// redraws the scene, since the surfaces themselves didn't.
    curves: Vec<(Id, Look)>,
    last_curves: Vec<(Id, Look)>,
    /// The windows whose pictures take passes before the scene (FSR with sharpening), and their textures.
    chains: super::chain::Chains,
    /// The histogram of the picture a curve's edit screen plots.
    histogram: super::histogram::Measure,
}

/// How a window's surface is shown: the HDR curve of the game running it (or none), the headroom of its
/// screen, its clarity settings, and the size of the surface's buffer, which sampling steps by.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    curve: Option<Curve>,
    headroom: f32,
    clarity: clamshell_state::session::presentation::Clarity,
    buffer: (i32, i32),
    /// FXAA in the final pass, as the window is shown one to one and not sharpened after it: no texture
    /// between.
    fxaa_here: bool,
    /// The HDR content peak set for its program, in the content's nits; 0 takes what it declares.
    hdr_peak: u16,
}
struct Converted {
    inner: Element,
    shader: GlesTexProgram,
    /// For an SDR or HDR surface's shader: its curve's uniforms (see [`curve_uniforms`]) and its clarity's
    /// (see [`clarity_uniform`]).
    curve: Option<[[f32; 4]; 3]>,
    /// SMAA's blending weights, when its blending is done in this pass: bound to the second texture unit
    /// while the element is drawn, as Smithay's programs take one texture (`docs/picture.md`).
    smaa_blend: Option<GlesTexture>,
}
impl ElementTrait for Converted {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        // SMAA's blending reaches past what the client damaged, along the edges it found: all of it.
        if self.smaa_blend.is_some() && commit != Some(self.current_commit()) {
            return DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]);
        }
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}
impl RenderElement<GlesRenderer> for Converted {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        // The weights' sampler is always on the second unit: left on the first, beside an external
        // texture, it would be a GL error at the draw.
        let uniforms = self.curve.map_or_else(Vec::new, |[a, b, c]| {
            let second = smithay::backend::renderer::gles::UniformValue::_1i(1);
            vec![Uniform::new("curve_a", a), Uniform::new("curve_b", b), Uniform::new("clarity", c), Uniform::new("smaa_blend_tex", second)]
        });
        if let Some(weights) = &self.smaa_blend {
            bind_second(frame, Some(weights.tex_id()))?;
        }
        frame.override_default_tex_program(self.shader.clone(), uniforms);
        let result = self.inner.draw(frame, src, dst, damage, opaque, cache);
        frame.clear_tex_program_override();
        if self.smaa_blend.is_some() {
            bind_second(frame, None)?;
        }
        result
    }
}
/// Binds `texture` to the second texture unit for the draw that follows, or unbinds it, filtered linearly as
/// SMAA's blending samples it. Smithay's drawing binds its texture to the first unit and leaves the others.
#[allow(unsafe_code)]
fn bind_second(frame: &mut GlesFrame<'_, '_>, texture: Option<u32>) -> Result<(), GlesError> {
    use smithay::backend::renderer::gles::ffi;
    frame.with_context(|gl| {
        // SAFETY: GL on Smithay's current context, on a texture of ours; the first unit is made active
        // again, as Smithay expects it.
        unsafe {
            gl.ActiveTexture(ffi::TEXTURE1);
            gl.BindTexture(ffi::TEXTURE_2D, texture.unwrap_or(0));
            if texture.is_some() {
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
            }
            gl.ActiveTexture(ffi::TEXTURE0);
        }
    })
}

impl Pipeline {
    /// The histogram measured since the last call, for the state (see [`super::histogram`]).
    pub(crate) fn take_histogram(&mut self) -> Option<clamshell_state::colour::Histogram> {
        self.histogram.measured.take()
    }

    pub(crate) fn new(renderer: &mut GlesRenderer, size: (i32, i32)) -> Result<Self, Error> {
        let texture = renderer.create_buffer(Fourcc::Abgr16161616f, size.into())?;
        let output = ColourDescription::default();
        let output_shader = shader(renderer, output, false)?;
        Ok(Self {
            size,
            texture,
            damage: OutputDamageTracker::new(size, 1.0, Transform::Normal),
            shaders: Vec::new(),
            converted: Vec::new(),
            outputs: [(output, output_shader.clone()), (output, output_shader)],
            revision: 0,
            first: true,
            curves: Vec::new(),
            last_curves: Vec::new(),
            chains: Default::default(),
            histogram: Default::default(),
        })
    }
    /// Each screen's encoding: the top's, then the second's.
    pub(crate) fn set_outputs(&mut self, renderer: &mut GlesRenderer, outputs: [ColourDescription; 2]) -> Result<(), Error> {
        for ((current, program), output) in self.outputs.iter_mut().zip(outputs) {
            if *current != output {
                *program = shader(renderer, output, false)?;
                *current = output;
            }
        }
        Ok(())
    }

    /// Screen `down`'s encoding (the second's, or the top's) and its shader.
    fn output(&self, down: bool) -> &(ColourDescription, GlesTexProgram) {
        let [top, bottom] = &self.outputs;
        if down { bottom } else { top }
    }

    /// Draws what changed in the scene. Returns the elements' states, and the part of the scene that
    /// changed, if anything did.
    pub(crate) fn compose(
        &mut self,
        server: &Server,
        renderer: &mut GlesRenderer,
        elements: &mut Vec<Element>,
    ) -> Result<(RenderElementStates, Option<Rectangle<i32, Physical>>), Error> {
        if self.size != server.size {
            let outputs = self.outputs.clone().map(|(description, _)| description);
            *self = Self::new(renderer, server.size)?;
            self.set_outputs(renderer, outputs)?;
        }
        if self.revision != server.colour.revision {
            self.damage = OutputDamageTracker::new(self.size, 1.0, Transform::Normal);
            self.first = true;
            self.revision = server.colour.revision;
        }
        self.histogram.turn(renderer, server, std::time::Instant::now());
        self.curves.clear();
        // The surface holding each window's picture, if it takes passes before the scene (see `super::chain`): SMAA,
        // and with FSR and sharpening, FSR's upscale.
        let mut chained: Vec<(Id, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, clamshell_state::settings::AntiAliasing, bool, bool)> =
            Vec::new();
        let state = server.core.state();
        for client in server.windows.iter().filter(|client| client.mapped && server.visible(client.id)) {
            let curve = client.shown.curve;
            // What the screen shows, as clients are told: nested, the host's real peak rather than the
            // PQ signal we send it.
            let headroom = server.colour.screen(state.session().below(client.id)).headroom() as f32;
            let mut clarity = client.shown.clarity;
            let hdr_peak = client.shown.hdr_peak;
            // The game's picture: the window's surface with the largest buffer (Wine draws it in a subsurface
            // over the toplevel's), and whether that buffer is its surface's size (Xwayland and Wine show a
            // mode a game picked as a smaller buffer scaled by a viewport).
            // The picture's surface (see `Surface::picture_surface`), and whether it shows its buffer unscaled.
            let picture = client.window.picture_surface().map(|surface| {
                let unscaled = smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| {
                    state.buffer_size().zip(state.surface_size()).is_some_and(|(buffer, shown)| buffer.w == shown.w && buffer.h == shown.h)
                });
                (surface, unscaled.unwrap_or(false))
            });
            let one_to_one = server.geometry(client.id).is_some_and(|g| g.one_to_one()) && picture.as_ref().is_some_and(|(_, unscaled)| *unscaled);
            // FSR shown one to one would upscale nothing: sampled as it is, its sharpening CAS's.
            if one_to_one && clarity.scaling == clamshell_state::settings::Scaling::Fsr {
                clarity.scaling = clamshell_state::settings::Scaling::Bilinear;
            }
            // Anti-aliasing at the texel's centre, as the window is shown one to one or nearest and nothing is
            // sharpened after it, is done in the final pass: FXAA all of it, SMAA its blending.
            let at_centre = (one_to_one || clarity.integer) && clarity.sharpen == 0;
            let fxaa_here = clarity.anti_aliasing == clamshell_state::settings::AntiAliasing::Fxaa && at_centre;
            let smaa_fused = clarity.anti_aliasing == clamshell_state::settings::AntiAliasing::Smaa && at_centre;
            client.window.with_surfaces(|surface, states| {
                let renderer_state = states.data_map.get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>();
                let buffer = renderer_state.and_then(|state| state.lock().ok()?.buffer_size()).map_or((0, 0), |size| (size.w, size.h));
                self.curves.push((Id::from_wayland_resource(surface), Look { curve, headroom, clarity, buffer, fxaa_here, hdr_peak }));
            });
            // At Integer, nearest-neighbour: no FSR whatever Scaling says.
            let upscale = clarity.scaling == clamshell_state::settings::Scaling::Fsr && clarity.sharpen > 0 && !clarity.integer;
            // FXAA done in the final pass takes no passes before it.
            let aa = if fxaa_here { clamshell_state::settings::AntiAliasing::Off } else { clarity.anti_aliasing };
            if let (true, Some((surface, ..))) = (upscale || aa != clamshell_state::settings::AntiAliasing::Off, picture) {
                chained.push((Id::from_wayland_resource(&surface), surface, aa, upscale, smaa_fused));
            }
        }
        if self.curves != self.last_curves {
            self.damage = OutputDamageTracker::new(self.size, 1.0, Transform::Normal);
            self.first = true;
            std::mem::swap(&mut self.curves, &mut self.last_curves);
        }
        let replaced = self.chains.run(renderer, elements, |id| {
            chained.iter().find(|(chained, ..)| chained == id).map(|(id, surface, aa, upscale, fused)| {
                use clamshell_state::settings::AntiAliasing;
                let hdr = server.colour.surfaces.get(id).is_some_and(|colour| colour.hdr());
                let linear = server.colour.surfaces.get(id).is_some_and(|colour| colour.transfer == TransferFunction::Linear);
                super::chain::Want { surface, smaa: *aa == AntiAliasing::Smaa, fxaa: *aa == AntiAliasing::Fxaa, upscale: *upscale, hdr, linear, fuse: *fused }
            })
        })?;
        self.converted.clear();
        for inner in elements.drain(..) {
            let colour = server.colour.surfaces.get(inner.id()).map(|colour| **colour).unwrap_or_default();
            let look = self.last_curves.iter().find(|(id, _)| id == inner.id()).map(|(_, look)| *look);
            let (curve, headroom) = look.as_ref().map_or((None, 1.0), |look| (look.curve.as_ref(), look.headroom));
            // An HDR surface's peak: the one set for its program, else what it declares or is assumed.
            let peak = hdr(colour).then(|| match look.map_or(0, |look| look.hdr_peak) {
                0 => colour.peak() as f32,
                set => f32::from(set) / colour.luminances().reference.max(1) as f32,
            });
            let curve = (sdr(colour) || hdr(colour)).then(|| {
                let [a, b] = curve_uniforms(curve, headroom, peak);
                // A picture the chain upscaled is sharpened one texel to a pixel (RCAS); one whose SMAA weights
                // it drew is blended here.
                let clarity = match replaced.iter().find(|(id, _)| id == inner.id()).map(|(_, chained)| chained) {
                    Some(super::chain::Chained::Upscaled([x, y])) => [*x, *y, look.map_or(0.0, |look| f32::from(look.clarity.sharpen.min(100)) / 100.0), 3.0],
                    Some(super::chain::Chained::Weights(_)) => {
                        let [x, y, _, _] = look.map_or([0.0; 4], |look| clarity_uniform(look.clarity, look.buffer, false));
                        [x, y, 0.0, 5.0]
                    }
                    None => look.map_or([0.0; 4], |look| clarity_uniform(look.clarity, look.buffer, look.fxaa_here)),
                };
                [a, b, clarity]
            });
            let program = match self.shaders.iter().find(|(description, _)| *description == colour) {
                Some((_, program)) => program.clone(),
                None => {
                    let program = shader(renderer, colour, true)?;
                    if self.shaders.len() == 32 {
                        self.shaders.remove(0);
                    }
                    self.shaders.push((colour, program.clone()));
                    program
                }
            };
            let smaa_blend = match replaced.iter().find(|(id, _)| id == inner.id()).map(|(_, chained)| chained) {
                Some(super::chain::Chained::Weights(weights)) => Some(weights.clone()),
                _ => None,
            };
            self.converted.push(Converted { inner, shader: program, curve, smaa_blend });
        }
        let mut target = renderer.bind(&mut self.texture)?;
        let result = self.damage.render_output(renderer, &mut target, usize::from(!self.first), &self.converted, [0.0, 0.0, 0.0, 1.0])?;
        let changed = result.damage.and_then(|d| d.iter().copied().reduce(|a, b| a.merge(b)));
        let states = result.states;
        self.first = false;
        Ok((states, changed))
    }
    /// Draws the whole scene at the next compose, as after the screens were away.
    pub(crate) fn redraw_all(&mut self) {
        self.first = true;
    }
    /// The whole scene in the top screen's encoding, as the nested host shows both screens in one window.
    pub(crate) fn present(&self, renderer: &mut GlesRenderer, target: &mut GlesTarget<'_>) -> Result<(), Error> {
        self.present_with(renderer, target, &self.output(false).1)
    }
    fn present_with(&self, renderer: &mut GlesRenderer, target: &mut GlesTarget<'_>, program: &GlesTexProgram) -> Result<(), Error> {
        let mut frame = renderer.render(target, self.size.into(), Transform::Flipped180)?;
        let rectangle = Rectangle::from_size(self.size.into());
        frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[rectangle])?;
        frame.render_texture_from_to(
            &self.texture,
            Rectangle::from_size(self.texture.size()).to_f64(),
            rectangle,
            &[rectangle],
            &[],
            Transform::Normal,
            1.0,
            Some(program),
            &[],
        )?;
        drop(frame.finish()?);
        Ok(())
    }
    /// Draws `part` of the scene onto screen `down`'s framebuffer (the second, or the top) in its
    /// encoding, `size` before the output's `transform` (a panel mounted turned), returning when the GPU
    /// will be done with it.
    pub(crate) fn present_part(
        &self,
        renderer: &mut GlesRenderer,
        target: &mut GlesTarget<'_>,
        down: bool,
        part: Rectangle<i32, Buffer>,
        size: smithay::utils::Size<i32, Physical>,
        transform: Transform,
    ) -> Result<smithay::backend::renderer::sync::SyncPoint, Error> {
        let mut frame = renderer.render(target, size, transform)?;
        let whole = Rectangle::from_size(transform.transform_size(size));
        frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[whole])?;
        frame.render_texture_from_to(&self.texture, part.to_f64(), whole, &[whole], &[], Transform::Normal, 1.0, Some(&self.output(down).1), &[])?;
        Ok(frame.finish()?)
    }

    /// The whole scene in sRGB, for a screenshot.
    pub fn capture(&self, renderer: &mut GlesRenderer) -> Result<crate::Capture, String> {
        let program = shader(renderer, ColourDescription::default(), false).map_err(|error| error.to_string())?;
        let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, self.size.into()).map_err(|error| error.to_string())?;
        let mut target = renderer.bind(&mut texture).map_err(|error| error.to_string())?;
        self.present_with(renderer, &mut target, &program).map_err(|error| error.to_string())?;
        read_back(renderer, &target, self.size)
    }
}

/// Readback is an SDR file export; clients and the composition target retain their native precision.
fn read_back(renderer: &mut GlesRenderer, target: &smithay::backend::renderer::gles::GlesTarget<'_>, size: (i32, i32)) -> Result<crate::Capture, String> {
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

/// Whether a surface of `description` is SDR: an sRGB or power-law signal, the curve applied to its signal
/// (`Curve::map_on`).
fn sdr(description: ColourDescription) -> bool {
    matches!(description.transfer, TransferFunction::Srgb | TransferFunction::Gamma(_))
}

/// Whether a surface of `description` is HDR: PQ or linear light (scRGB's), the curve applied to its light
/// and its highlights compressed into the screen (`curve::map_hdr`).
fn hdr(description: ColourDescription) -> bool {
    description.hdr()
}

/// A window's clarity as the input shaders' uniform: one texel of its buffer across and down, the
/// sharpening's strength (0 to 1), and how it is sampled (0 bilinear, 1 nearest, 2 FSR's upscale; 3, from
/// the chain, FSR's sharpening of what it upscaled, see [`super::chain`]; 4, `fxaa_here`, FXAA at the
/// texel's centre; 5, from the chain, SMAA's blending at the texel's centre).
/// Nothing without a buffer's size.
fn clarity_uniform(clarity: clamshell_state::session::presentation::Clarity, (w, h): (i32, i32), fxaa_here: bool) -> [f32; 4] {
    use clamshell_state::settings::Scaling;
    if w <= 0 || h <= 0 {
        return [0.0; 4];
    }
    let sampling = match clarity.scaling {
        _ if fxaa_here => 4.0,
        _ if clarity.integer => 1.0,
        Scaling::Bilinear => 0.0,
        Scaling::Fsr => 2.0,
    };
    [1.0 / w as f32, 1.0 / h as f32, f32::from(clarity.sharpen.min(100)) / 100.0, sampling]
}

/// A tone curve as the input shaders' uniforms: black point, contrast, where highlights start, and for an
/// SDR surface their peak (in units of white: the curve's share of the screen's room above white), for an
/// HDR one its own peak (`hdr_peak`, in units of its white); then white over the reference, saturation,
/// the screen's headroom, and whether a curve is on.
/// Without one, an SDR surface is only decoded and an HDR one compressed into the screen.
fn curve_uniforms(curve: Option<&Curve>, headroom: f32, hdr_peak: Option<f32>) -> [[f32; 4]; 2] {
    let Some(c) = curve else {
        return [[0.0, 1.0, 1.0, hdr_peak.unwrap_or(1.0)], [1.0, 1.0, headroom, 0.0]];
    };
    let white = f32::from(c.white.max(1));
    let to_scene = (f64::from(white) / REFERENCE_NITS).min(f64::from(headroom));
    let peak = hdr_peak.unwrap_or(c.peak_in(f64::from(headroom) / to_scene) as f32);
    [
        [f32::from(c.black) / 1000.0, f32::from(c.contrast) / 100.0, f32::from(c.start) / 100.0, peak],
        [white / REFERENCE_NITS as f32, f32::from(c.saturation) / 100.0, headroom, 1.0],
    ]
}

/// FSR 1's upscale (EASU) of a picture, with the GLSL it needs, on the texture `tex` whose texel size is
/// `clarity.xy` (see [`clarity_uniform`]): the twelve texels around each pixel, weighed along the edge their
/// luma shows and kept within the four nearest (as `easu` in the tests). It works on the signal, before
/// decoding.
pub(super) const EASU_GLSL: &str = "vec4 texel(vec2 at) { return texture2D(tex, (at + 0.5) * clarity.xy); }
float luma(vec3 c) { c = perceive(c); return c.b * 0.5 + c.r * 0.5 + c.g; }
void easu_set(inout vec2 dir, inout float len, float w, float a, float b, float c, float d, float e) {
    float dc = d - c;
    float cb = c - b;
    float lx = 1.0 / max(max(abs(dc), abs(cb)), 0.000001);
    float dx = d - b;
    dir.x += dx * w;
    lx = clamp(abs(dx) * lx, 0.0, 1.0);
    len += lx * lx * w;
    float ec = e - c;
    float ca = c - a;
    float ly = 1.0 / max(max(abs(ec), abs(ca)), 0.000001);
    float dy = e - a;
    dir.y += dy * w;
    ly = clamp(abs(dy) * ly, 0.0, 1.0);
    len += ly * ly * w;
}
void easu_tap(inout vec3 ac, inout float aw, vec2 off, vec2 dir, vec2 len2, float lob, float clp, vec3 c) {
    vec2 v = vec2(off.x * dir.x + off.y * dir.y, off.x * -dir.y + off.y * dir.x) * len2;
    float d2 = min(v.x * v.x + v.y * v.y, clp);
    float wb = 0.4 * d2 - 1.0;
    float wa = lob * d2 - 1.0;
    wb *= wb;
    wa *= wa;
    float w = (1.5625 * wb - 0.5625) * wa;
    ac += c * w;
    aw += w;
}
vec4 easu() {
    vec2 pp = v_coords / clarity.xy - 0.5;
    vec2 fp = floor(pp);
    pp -= fp;
    vec3 b = texel(fp + vec2(0.0, -1.0)).rgb; vec3 c = texel(fp + vec2(1.0, -1.0)).rgb;
    vec3 e = texel(fp + vec2(-1.0, 0.0)).rgb; vec4 f4 = texel(fp); vec3 f = f4.rgb;
    vec3 g = texel(fp + vec2(1.0, 0.0)).rgb; vec3 h = texel(fp + vec2(2.0, 0.0)).rgb;
    vec3 i = texel(fp + vec2(-1.0, 1.0)).rgb; vec3 j = texel(fp + vec2(0.0, 1.0)).rgb;
    vec3 k = texel(fp + vec2(1.0, 1.0)).rgb; vec3 l = texel(fp + vec2(2.0, 1.0)).rgb;
    vec3 n = texel(fp + vec2(0.0, 2.0)).rgb; vec3 o = texel(fp + vec2(1.0, 2.0)).rgb;
    float bl = luma(b); float cl = luma(c); float el = luma(e); float fl = luma(f);
    float gl = luma(g); float hl = luma(h); float il = luma(i); float jl = luma(j);
    float kl = luma(k); float ll = luma(l); float nl = luma(n); float ol = luma(o);
    vec2 dir = vec2(0.0);
    float len = 0.0;
    easu_set(dir, len, (1.0 - pp.x) * (1.0 - pp.y), bl, el, fl, gl, jl);
    easu_set(dir, len, pp.x * (1.0 - pp.y), cl, fl, gl, hl, kl);
    easu_set(dir, len, (1.0 - pp.x) * pp.y, fl, il, jl, kl, nl);
    easu_set(dir, len, pp.x * pp.y, gl, jl, kl, ll, ol);
    float dr = dir.x * dir.x + dir.y * dir.y;
    bool zero = dr < 1.0 / 32768.0;
    dr = zero ? 1.0 : inversesqrt(dr);
    dir.x = zero ? 1.0 : dir.x;
    dir *= dr;
    len = len * 0.5;
    len *= len;
    float stretch = (dir.x * dir.x + dir.y * dir.y) / max(abs(dir.x), abs(dir.y));
    vec2 len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    float lob = 0.5 + (0.25 - 0.04 - 0.5) * len;
    float clp = 1.0 / lob;
    vec3 ac = vec3(0.0);
    float aw = 0.0;
    easu_tap(ac, aw, vec2(0.0, -1.0) - pp, dir, len2, lob, clp, b);
    easu_tap(ac, aw, vec2(1.0, -1.0) - pp, dir, len2, lob, clp, c);
    easu_tap(ac, aw, vec2(-1.0, 1.0) - pp, dir, len2, lob, clp, i);
    easu_tap(ac, aw, vec2(0.0, 1.0) - pp, dir, len2, lob, clp, j);
    easu_tap(ac, aw, vec2(0.0, 0.0) - pp, dir, len2, lob, clp, f);
    easu_tap(ac, aw, vec2(-1.0, 0.0) - pp, dir, len2, lob, clp, e);
    easu_tap(ac, aw, vec2(1.0, 1.0) - pp, dir, len2, lob, clp, k);
    easu_tap(ac, aw, vec2(2.0, 1.0) - pp, dir, len2, lob, clp, l);
    easu_tap(ac, aw, vec2(2.0, 0.0) - pp, dir, len2, lob, clp, h);
    easu_tap(ac, aw, vec2(1.0, 0.0) - pp, dir, len2, lob, clp, g);
    easu_tap(ac, aw, vec2(1.0, 2.0) - pp, dir, len2, lob, clp, o);
    easu_tap(ac, aw, vec2(0.0, 2.0) - pp, dir, len2, lob, clp, n);
    vec3 lo = min(min(f, g), min(j, k));
    vec3 hi = max(max(f, g), max(j, k));
    return vec4(clamp(ac / aw, lo, hi), f4.a);
}
";

/// SMAA's last pass, neighbourhood blending, done in the final pass (SMAA's `SMAANeighborhoodBlendingPS`,
/// in GLSL ES 1.00): at texel centre `uv` of the picture `tex`, its blending weights from `smaa_blend`
/// (the weights pass's texture, at the picture's size and orientation) choose the neighbour it is
/// blended with, by bilinear sampling between the two.
const SMAA_BLEND_GLSL: &str = "vec4 smaa_blend(vec2 uv) {
    vec2 px = clarity.xy;
    vec4 a;
    a.x = texture2D(smaa_blend_tex, uv + vec2(px.x, 0.0)).a;
    a.y = texture2D(smaa_blend_tex, uv + vec2(0.0, px.y)).g;
    a.wz = texture2D(smaa_blend_tex, uv).xz;
    if (dot(a, vec4(1.0)) < 0.00001) { return texture2D(tex, uv); }
    bool h = max(a.x, a.z) > max(a.y, a.w);
    vec4 offset = h ? vec4(a.x, 0.0, a.z, 0.0) : vec4(0.0, a.y, 0.0, a.w);
    vec2 w = h ? a.xz : a.yw;
    w /= dot(w, vec2(1.0));
    vec4 coord = offset * vec4(px, -px) + uv.xyxy;
    return w.x * texture2D(tex, coord.xy) + w.y * texture2D(tex, coord.zw);
}
";

/// FXAA of a picture, on the texture `tex` whose texel size is `clarity.xy`: the edge's direction from the
/// luma of the four diagonal neighbours, two samples along it blended, or four when that stays within the
/// neighbours' range (as `fxaa` in the tests). It works on the signal, at the game's resolution.
pub(super) const FXAA_GLSL: &str = "vec4 fxaa(vec2 uv) {
    vec2 px = clarity.xy;
    vec4 m4 = texture2D(tex, uv);
    vec3 nw = texture2D(tex, uv + vec2(-1.0, -1.0) * px).rgb;
    vec3 ne = texture2D(tex, uv + vec2(1.0, -1.0) * px).rgb;
    vec3 sw = texture2D(tex, uv + vec2(-1.0, 1.0) * px).rgb;
    vec3 se = texture2D(tex, uv + vec2(1.0, 1.0) * px).rgb;
    vec3 weights = vec3(0.299, 0.587, 0.114);
    float lnw = dot(perceive(nw), weights);
    float lne = dot(perceive(ne), weights);
    float lsw = dot(perceive(sw), weights);
    float lse = dot(perceive(se), weights);
    float lm = dot(perceive(m4.rgb), weights);
    float lmin = min(lm, min(min(lnw, lne), min(lsw, lse)));
    float lmax = max(lm, max(max(lnw, lne), max(lsw, lse)));
    vec2 dir = vec2(-((lnw + lne) - (lsw + lse)), (lnw + lsw) - (lne + lse));
    float reduce = max((lnw + lne + lsw + lse) * (0.25 * 0.125), 1.0 / 128.0);
    float rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * rcp, vec2(-8.0), vec2(8.0)) * px;
    vec3 a = 0.5 * (texture2D(tex, uv + dir * (1.0 / 3.0 - 0.5)).rgb + texture2D(tex, uv + dir * (2.0 / 3.0 - 0.5)).rgb);
    vec3 b = a * 0.5 + 0.25 * (texture2D(tex, uv - dir * 0.5).rgb + texture2D(tex, uv + dir * 0.5).rgb);
    float lb = dot(perceive(b), weights);
    return vec4((lb < lmin || lb > lmax) ? a : b, m4.a);
}
";

/// The signal the passes measure contrast in: for linear light (scRGB's, 1.0 at 80 nits, `LINEAR`), PQ's of
/// it, as a PQ picture's own signal is; for the others their signal as it is. Edges and sharpening are
/// judged on it; anti-aliasing still blends the light itself.
pub(super) const PERCEIVE_GLSL: &str = "vec3 perceive(vec3 c) {
    if (!LINEAR) { return c; }
    vec3 p = pow(max(c, vec3(0.0)) * 0.008, vec3(0.1593017578125));
    return pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), vec3(78.84375));
}
vec3 unperceive(vec3 e) {
    if (!LINEAR) { return e; }
    vec3 p = pow(clamp(e, 0.0, 1.0), vec3(1.0 / 78.84375));
    return pow(max(p - 0.8359375, vec3(0.0)) / (18.8515625 - 18.6875 * p), vec3(1.0 / 0.1593017578125)) * 125.0;
}
";

/// FSR 1's sharpening (RCAS) of what EASU upscaled, one texel to a pixel (`clarity.xy` its texel size,
/// `clarity.z` the strength, 0 to 1, as AMD's sharpness from 2 stops down to none): a negative lobe from the
/// four texels around, limited so that it never rings beyond them (as `rcas` in the tests).
const RCAS_GLSL: &str = "vec4 rcas() {
    vec4 e4 = texture2D(tex, v_coords);
    vec3 e = perceive(e4.rgb);
    vec3 b = perceive(texture2D(tex, v_coords - vec2(0.0, clarity.y)).rgb);
    vec3 h = perceive(texture2D(tex, v_coords + vec2(0.0, clarity.y)).rgb);
    vec3 d = perceive(texture2D(tex, v_coords - vec2(clarity.x, 0.0)).rgb);
    vec3 f = perceive(texture2D(tex, v_coords + vec2(clarity.x, 0.0)).rgb);
    vec3 mn4 = min(min(b, d), min(f, h));
    vec3 mx4 = max(max(b, d), max(f, h));
    vec3 hit_min = min(mn4, e) / max(4.0 * mx4, vec3(0.000001));
    vec3 hit_max = (1.0 - max(mx4, e)) / min(4.0 * mn4 - 4.0, vec3(-0.000001));
    vec3 lobes = max(-hit_min, hit_max);
    float lobe = max(-0.1875, min(max(max(lobes.r, lobes.g), lobes.b), 0.0)) * exp2(-2.0 * (1.0 - clarity.z));
    return vec4(unperceive((lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0)), e4.a);
}
";

/// A window's picture as the final pass samples it, before decoding (see [`clarity_uniform`]): RCAS on
/// EASU's texture, EASU itself, or nearest (snapped to the centre of a texel) or bilinear, sharpened then by
/// AMD's CAS on the four texels around, with the weight it adapts to their contrast, kept within the
/// signal's range, or for linear light within the brightest of them (as `cas` in the tests).
const FETCH_GLSL: &str = "vec4 fetch() {
    if (clarity.w > 3.5) {
        vec2 centre = (floor(v_coords / clarity.xy) + 0.5) * clarity.xy;
        return clarity.w > 4.5 ? smaa_blend(centre) : fxaa(centre);
    }
    if (clarity.w > 2.5) { return rcas(); }
    if (clarity.w > 1.5) { return easu(); }
    vec2 uv = v_coords;
    if (clarity.w > 0.5) { uv = (floor(uv / clarity.xy) + 0.5) * clarity.xy; }
    vec4 e = texture2D(tex, uv);
    if (clarity.z > 0.0) {
        vec3 c = perceive(e.rgb);
        vec3 b = perceive(texture2D(tex, uv - vec2(0.0, clarity.y)).rgb);
        vec3 h = perceive(texture2D(tex, uv + vec2(0.0, clarity.y)).rgb);
        vec3 d = perceive(texture2D(tex, uv - vec2(clarity.x, 0.0)).rgb);
        vec3 f = perceive(texture2D(tex, uv + vec2(clarity.x, 0.0)).rgb);
        vec3 mn = min(min(min(d, c), min(f, b)), h);
        vec3 mx = max(max(max(d, c), max(f, b)), h);
        vec3 amp = sqrt(clamp(min(mn, 2.0 - mx) / max(mx, vec3(0.00001)), 0.0, 1.0));
        vec3 w = amp * (-1.0 / mix(8.0, 5.0, clarity.z));
        e.rgb = unperceive(clamp(((b + d + f + h) * w + c) / (1.0 + 4.0 * w), vec3(0.0), max(mx, vec3(1.0))));
    }
    return e;
}";

fn shader(renderer: &mut GlesRenderer, description: ColourDescription, input: bool) -> Result<GlesTexProgram, Error> {
    let (from, to) = if input { (description.primaries, ColourPrimaries::Bt2020) } else { (ColourPrimaries::Bt2020, description.primaries) };
    let [[a, b, c], [d, e, f], [g, h, i]] = super::math::matrix(from, to).ok_or("unsupported colour primary matrix")?;
    super::math::encode(1.0, description).ok_or("unsupported output transfer")?;
    let reference = f64::from(description.luminances().reference);
    let expression = match (input, description.transfer) {
        (true, TransferFunction::Srgb) => "mix(v/12.92,pow(max((v+0.055)/1.055,vec3(0.0)),vec3(2.4)),step(vec3(0.04045),v))".into(),
        (false, TransferFunction::Srgb) => "mix(v*12.92,1.055*pow(max(v,vec3(0.0)),vec3(1.0/2.4))-0.055,step(vec3(0.0031308),v))".into(),
        (true, TransferFunction::Gamma(exponent)) => format!("pow(max(v,vec3(0.0)),vec3({:.12}))", f64::from(exponent) / 10000.0),
        (false, TransferFunction::Gamma(exponent)) => format!("pow(max(v,vec3(0.0)),vec3({:.12}))", 10000.0 / f64::from(exponent)),
        (_, TransferFunction::Linear) => "v".into(),
        (true, TransferFunction::Pq) => format!(
            "pow(max(pow(max(v,vec3(0.0)),vec3(32.0/2523.0))-3424.0/4096.0,vec3(0.0))/max(vec3(2413.0/128.0)-2392.0/128.0*pow(max(v,vec3(0.0)),vec3(32.0/2523.0)),vec3(0.000000001)),vec3(16384.0/2610.0))*(10000.0/{reference:.12})"
        ),
        (false, TransferFunction::Pq) => format!(
            "pow((vec3(3424.0/4096.0)+2413.0/128.0*pow(max(v,vec3(0.0))*({reference:.12}/10000.0),vec3(2610.0/16384.0)))/(vec3(1.0)+2392.0/128.0*pow(max(v,vec3(0.0))*({reference:.12}/10000.0),vec3(2610.0/16384.0))),vec3(2523.0/32.0))"
        ),
        _ => return Err("unsupported colour transfer".into()),
    };
    let scaling = if description.transfer == TransferFunction::Pq { 1.0 } else { f64::from(description.luminances().max) / reference };
    // An SDR surface's HDR curve, as clamshell_state::curve::Curve::map_on does it: on the signal, the
    // black point and contrast; on the light, highlights expanded on luminance within the screen's peak
    // (curve_b.z) and saturation around it.
    let curved = input && (sdr(description) || hdr(description));
    let conversion = if input && sdr(description) {
        format!(
            "bool on = curve_b.w > 0.5;
    if (on) {{ v = pow(clamp((v - curve_a.x) / max(1.0 - curve_a.x, 0.000001), 0.0, 1.0), vec3(curve_a.y)); }}
    v = convert(v) * {scaling:.12};
    if (on) {{
        float white = min(curve_b.x, curve_b.z);
        float p = max(min(curve_a.w, curve_b.z / white), 1.0);
        float y = dot(v, vec3(0.2126, 0.7152, 0.0722));
        float s = curve_a.z;
        float t = (y - s) / max(1.0 - s, 0.000001);
        float e = (y > s && s < 1.0) ? s + (1.0 - s) * t + (p - 1.0) * t * t : y;
        vec3 c = y > 0.0 ? v * (e / y) : vec3(0.0);
        v = max(vec3(e) + (c - vec3(e)) * curve_b.y, vec3(0.0)) * white;
    }}
    v = matrix * v;"
        )
    } else if input && hdr(description) {
        // An HDR surface, as clamshell_state::curve::map_hdr does it, in BT.2020: with a curve, the black
        // point and contrast on each channel's light below white, and the white; with or without one, the
        // highlights above the knee compressed into the screen's peak when its own peak (curve_a.w) is
        // beyond.
        format!(
            "v = convert(v) * {scaling:.12};
    v = matrix * v;
    bool on = curve_b.w > 0.5;
    float white = on ? min(curve_b.x, curve_b.z) : min(1.0, curve_b.z);
    if (on) {{
        vec3 g = pow(clamp(v, 0.0, 1.0), vec3(1.0 / 2.2));
        g = pow(pow(clamp((g - curve_a.x) / max(1.0 - curve_a.x, 0.000001), 0.0, 1.0), vec3(curve_a.y)), vec3(2.2));
        v = mix(g, v, step(vec3(1.0), v));
    }} else {{
        v = max(v, vec3(0.0));
    }}
    float y = dot(v, vec3(0.2627, 0.6780, 0.0593));
    float room = curve_b.z / white;
    float k = min(on ? curve_a.z : 1.0, 0.75 * room);
    float e = y;
    if (curve_a.w > room && y > k) {{
        float x = y - k;
        e = min(k + x / (1.0 + x * (1.0 / (room - k) - 1.0 / (curve_a.w - k))), room);
    }}
    vec3 c = y > 0.0 ? v * (e / y) : vec3(0.0);
    v = max(vec3(e) + (c - vec3(e)) * (on ? curve_b.y : 1.0), vec3(0.0)) * white;"
        )
    } else if input {
        format!("v = convert(v) * {scaling:.12}; v = matrix * v;")
    } else if description.transfer == TransferFunction::Pq {
        format!("v = matrix * v; v /= {scaling:.12}; v = convert(v);")
    } else {
        // Above the screen's peak, each pixel is brought down whole, keeping its hue.
        format!("v = matrix * v; v /= {scaling:.12}; float m = max(max(v.r, v.g), v.b); if (m > 1.0) {{ v /= m; }} v = convert(v);")
    };
    let uniforms = if curved { "uniform vec4 curve_a;\nuniform vec4 curve_b;\nuniform vec4 clarity;\nuniform sampler2D smaa_blend_tex;" } else { "" };
    let fetch = if curved {
        let linear = format!("const bool LINEAR = {};\n", description.transfer == TransferFunction::Linear);
        [linear.as_str(), PERCEIVE_GLSL, EASU_GLSL, FXAA_GLSL, SMAA_BLEND_GLSL, RCAS_GLSL, FETCH_GLSL].concat()
    } else {
        "vec4 fetch() { return texture2D(tex, v_coords); }".to_owned()
    };
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
{uniforms}
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
vec3 convert(vec3 v) {{ return {expression}; }}
{fetch}
void main() {{
    vec4 sample = fetch();
#if defined(NO_ALPHA)
    sample.a = 1.0;
#endif
    vec3 v = sample.a > 0.0 ? sample.rgb / sample.a : vec3(0.0);
    mat3 matrix = mat3({a:.12},{d:.12},{g:.12},{b:.12},{e:.12},{h:.12},{c:.12},{f:.12},{i:.12});
    {conversion}
    gl_FragColor = vec4(v * sample.a, sample.a) * alpha;
}}"#
    );
    let names = [
        UniformName::new("curve_a", UniformType::_4f),
        UniformName::new("curve_b", UniformType::_4f),
        UniformName::new("clarity", UniformType::_4f),
        UniformName::new("smaa_blend_tex", UniformType::_1i),
    ];
    Ok(renderer.compile_custom_texture_shader(source, if curved { &names } else { &[] })?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    /// The SDR input shader's arithmetic for an sRGB surface, as written in GLSL above.
    fn shaded(rgb: [f64; 3], [a, b]: [[f32; 4]; 2]) -> [f64; 3] {
        let (a, b) = (a.map(f64::from), b.map(f64::from));
        let decode = |e: f64| if e <= 0.04045 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) };
        let on = b[3] > 0.5;
        let mut v = rgb;
        if on {
            v = v.map(|e| ((e - a[0]) / (1.0 - a[0]).max(0.000001)).clamp(0.0, 1.0).powf(a[1]));
        }
        v = v.map(decode);
        if on {
            let white = b[0].min(b[2]);
            let p = a[3].min(b[2] / white).max(1.0);
            let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
            let (s, t) = (a[2], (y - a[2]) / (1.0 - a[2]).max(0.000001));
            let e = if y > s && s < 1.0 { s + (1.0 - s) * t + (p - 1.0) * t * t } else { y };
            let c = if y > 0.0 { v.map(|x| x * e / y) } else { [0.0; 3] };
            v = c.map(|x| (e + (x - e) * b[1]).max(0.0) * white);
        }
        v
    }

    /// The HDR input shader's arithmetic, on light already in BT.2020 (1 at the content's reference
    /// white), as written in GLSL above.
    fn shaded_hdr(light: [f64; 3], [a, b]: [[f32; 4]; 2]) -> [f64; 3] {
        let (a, b) = (a.map(f64::from), b.map(f64::from));
        let peak = a[3];
        let on = b[3] > 0.5;
        let white = if on { b[0].min(b[2]) } else { 1.0_f64.min(b[2]) };
        let v = light.map(|v| {
            if !on {
                return v.max(0.0);
            }
            if v >= 1.0 {
                return v;
            }
            let g = v.clamp(0.0, 1.0).powf(1.0 / 2.2);
            ((g - a[0]) / (1.0 - a[0]).max(0.000001)).clamp(0.0, 1.0).powf(a[1]).powf(2.2)
        });
        let y = 0.2627 * v[0] + 0.6780 * v[1] + 0.0593 * v[2];
        let room = b[2] / white;
        let k = (if on { a[2] } else { 1.0 }).min(0.75 * room);
        let mut e = y;
        if peak > room && y > k {
            let x = y - k;
            e = (k + x / (1.0 + x * (1.0 / (room - k) - 1.0 / (peak - k)))).min(room);
        }
        let c = if y > 0.0 { v.map(|x| x * e / y) } else { [0.0; 3] };
        c.map(|x| (e + (x - e) * if on { b[1] } else { 1.0 }).max(0.0) * white)
    }

    /// The input shader's sharpening (CAS) for one channel: the centre `e` and the texels above, left,
    /// right and below it, at `sharpness` (0 to 1), as written in GLSL above.
    fn cas(e: f64, [b, d, f, h]: [f64; 4], sharpness: f64) -> f64 {
        let mn = b.min(d).min(e).min(f).min(h);
        let mx = b.max(d).max(e).max(f).max(h);
        let amp = (mn.min(2.0 - mx) / mx.max(0.00001)).clamp(0.0, 1.0).sqrt();
        let w = amp * (-1.0 / (8.0 + (5.0 - 8.0) * sharpness));
        (((b + d + f + h) * w + e) / (1.0 + 4.0 * w)).clamp(0.0, mx.max(1.0))
    }

    #[test]
    fn sharpening_leaves_flat_areas_and_raises_edges_within_range() {
        // Flat: nothing changes.
        assert!((cas(0.4, [0.4; 4], 1.0) - 0.4).abs() < 1e-9);
        // An edge: the brighter side brighter, the darker side darker, more with more sharpness.
        let (light, dark) = ([0.8, 0.2, 0.8, 0.8], [0.2, 0.8, 0.2, 0.2]);
        assert!(cas(0.8, light, 0.5) > 0.8 && cas(0.8, light, 1.0) > cas(0.8, light, 0.5));
        assert!(cas(0.2, dark, 1.0) < 0.2);
        // Never below black nor above the signal's range (above the brightest texel, for linear light).
        for e in [0.0, 0.1, 0.5, 0.9, 1.0] {
            for n in [0.0, 0.3, 1.0] {
                let v = cas(e, [n, e, n, e], 1.0);
                assert!((0.0..=1.0 + 1e-9).contains(&v), "{e} {n}: {v}");
            }
        }
    }

    /// The input shader's FSR upscale (EASU) of a one-channel picture (`at` a texel's value, clamped at the
    /// edges), at input position `pp` (texels, 0 at the first texel's centre), as written in GLSL above.
    fn easu(at: &dyn Fn(i32, i32) -> f64, pp: (f64, f64)) -> f64 {
        let (fx, fy) = (pp.0.floor(), pp.1.floor());
        let (px, py) = (pp.0 - fx, pp.1 - fy);
        let t = |dx: i32, dy: i32| at(fx as i32 + dx, fy as i32 + dy);
        let [b, c, e, f, g, h, i, j, k, l, n, o] =
            [t(0, -1), t(1, -1), t(-1, 0), t(0, 0), t(1, 0), t(2, 0), t(-1, 1), t(0, 1), t(1, 1), t(2, 1), t(0, 2), t(1, 2)];
        // One channel: its luma is twice it, which changes no weight.
        let lu = |v: f64| v * 2.0;
        let (mut dir, mut len) = ((0.0, 0.0), 0.0);
        let mut set = |w: f64, a: f64, b: f64, c: f64, d: f64, e: f64| {
            let lx = 1.0 / (d - c).abs().max((c - b).abs()).max(0.000001);
            dir.0 += (d - b) * w;
            len += ((d - b).abs() * lx).clamp(0.0, 1.0).powi(2) * w;
            let ly = 1.0 / (e - c).abs().max((c - a).abs()).max(0.000001);
            dir.1 += (e - a) * w;
            len += ((e - a).abs() * ly).clamp(0.0, 1.0).powi(2) * w;
        };
        set((1.0 - px) * (1.0 - py), lu(b), lu(e), lu(f), lu(g), lu(j));
        set(px * (1.0 - py), lu(c), lu(f), lu(g), lu(h), lu(k));
        set((1.0 - px) * py, lu(f), lu(i), lu(j), lu(k), lu(n));
        set(px * py, lu(g), lu(j), lu(k), lu(l), lu(o));
        let dr = dir.0 * dir.0 + dir.1 * dir.1;
        let zero = dr < 1.0 / 32768.0;
        let dr = if zero { 1.0 } else { 1.0 / dr.sqrt() };
        if zero {
            dir.0 = 1.0;
        }
        let dir = (dir.0 * dr, dir.1 * dr);
        let len = (len * 0.5).powi(2);
        let stretch = (dir.0 * dir.0 + dir.1 * dir.1) / dir.0.abs().max(dir.1.abs());
        let len2 = (1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
        let lob = 0.5 + (0.25 - 0.04 - 0.5) * len;
        let clp = 1.0 / lob;
        let (mut ac, mut aw) = (0.0, 0.0);
        for (ox, oy, v) in [
            (0.0, -1.0, b),
            (1.0, -1.0, c),
            (-1.0, 1.0, i),
            (0.0, 1.0, j),
            (0.0, 0.0, f),
            (-1.0, 0.0, e),
            (1.0, 1.0, k),
            (2.0, 1.0, l),
            (2.0, 0.0, h),
            (1.0, 0.0, g),
            (1.0, 2.0, o),
            (0.0, 2.0, n),
        ] {
            let (x, y) = (ox - px, oy - py);
            let (vx, vy) = ((x * dir.0 + y * dir.1) * len2.0, (x * -dir.1 + y * dir.0) * len2.1);
            let d2 = (vx * vx + vy * vy).min(clp);
            let (wb, wa) = ((0.4 * d2 - 1.0).powi(2), (lob * d2 - 1.0).powi(2));
            let w = (1.5625 * wb - 0.5625) * wa;
            ac += v * w;
            aw += w;
        }
        (ac / aw).clamp(f.min(g).min(j).min(k), f.max(g).max(j).max(k))
    }

    /// The final pass's FSR sharpening (RCAS) for one channel: the centre `e` and the texels above, left,
    /// right and below it, at strength `s` (0 to 1), as written in GLSL above.
    fn rcas(e: f64, [b, d, f, h]: [f64; 4], s: f64) -> f64 {
        let (mn4, mx4) = (b.min(d).min(f).min(h), b.max(d).max(f).max(h));
        let hit_min = mn4.min(e) / (4.0 * mx4).max(0.000001);
        let hit_max = (1.0 - mx4.max(e)) / (4.0 * mn4 - 4.0).min(-0.000001);
        let lobe = (-hit_min).max(hit_max).clamp(-0.1875, 0.0) * (-2.0 * (1.0 - s)).exp2();
        (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0)
    }

    /// FXAA of a one-channel picture (`at` a texel's value, between texels bilinear, clamped at the edges)
    /// at texel `(x, y)`, as written in GLSL above.
    fn fxaa(at: &dyn Fn(i32, i32) -> f64, (x, y): (i32, i32)) -> f64 {
        let sample = |sx: f64, sy: f64| {
            let (fx, fy) = (sx.floor(), sy.floor());
            let (tx, ty) = (sx - fx, sy - fy);
            let t = |dx: i32, dy: i32| at(fx as i32 + dx, fy as i32 + dy);
            (t(0, 0) * (1.0 - tx) + t(1, 0) * tx) * (1.0 - ty) + (t(0, 1) * (1.0 - tx) + t(1, 1) * tx) * ty
        };
        // One channel: its luma is the value, as the weights sum to 1.
        let (m, nw, ne, sw, se) = (at(x, y), at(x - 1, y - 1), at(x + 1, y - 1), at(x - 1, y + 1), at(x + 1, y + 1));
        let lmin = m.min(nw).min(ne).min(sw).min(se);
        let lmax = m.max(nw).max(ne).max(sw).max(se);
        let dir = (-((nw + ne) - (sw + se)), (nw + sw) - (ne + se));
        let reduce = ((nw + ne + sw + se) * (0.25 * 0.125)).max(1.0 / 128.0);
        let rcp = 1.0 / (dir.0.abs().min(dir.1.abs()) + reduce);
        let dir = ((dir.0 * rcp).clamp(-8.0, 8.0), (dir.1 * rcp).clamp(-8.0, 8.0));
        let along = |k: f64| sample(f64::from(x) + dir.0 * k, f64::from(y) + dir.1 * k);
        let a = 0.5 * (along(1.0 / 3.0 - 0.5) + along(2.0 / 3.0 - 0.5));
        let b = a * 0.5 + 0.25 * (along(-0.5) + along(0.5));
        if b < lmin || b > lmax { a } else { b }
    }

    #[test]
    fn fxaa_leaves_flat_areas_and_softens_a_jagged_edge_within_its_range() {
        assert!((fxaa(&|_, _| 0.4, (5, 5)) - 0.4).abs() < 1e-9);
        // A diagonal staircase: dark below the line y = x, light above.
        let stair = |x: i32, y: i32| if y < x { 0.9 } else { 0.1 };
        let v = fxaa(&stair, (5, 5));
        assert!(v > 0.1 && v < 0.9, "a pixel on the edge gets an in-between tone: {v}");
        let inside = fxaa(&stair, (5, 9));
        assert!((inside - 0.1).abs() < 1e-9, "far from the edge it stays: {inside}");
    }

    #[test]
    fn fsrs_sharpening_leaves_flat_areas_and_sharpens_edges_without_ringing_past_them() {
        assert!((rcas(0.4, [0.4; 4], 1.0) - 0.4).abs() < 1e-9);
        let (light, dark) = (rcas(0.8, [0.8, 0.2, 0.8, 0.8], 1.0), rcas(0.2, [0.2, 0.8, 0.2, 0.2], 1.0));
        assert!(light > 0.8 && dark < 0.2, "{light} {dark}");
        assert!(rcas(0.8, [0.8, 0.2, 0.8, 0.8], 0.25) < light, "weaker at a lower strength");
        for e in [0.0, 0.2, 0.5, 1.0] {
            for n in [0.0, 0.5, 1.0] {
                let v = rcas(e, [n, e, n, e], 1.0);
                assert!((-1e-9..=1.0 + 1e-9).contains(&v), "{e} {n}: {v}");
            }
        }
    }

    #[test]
    fn fsrs_upscale_keeps_flat_areas_and_edges_sharper_than_bilinear() {
        // A flat picture stays flat.
        let flat = |_: i32, _: i32| 0.4;
        assert!((easu(&flat, (3.3, 2.7)) - 0.4).abs() < 1e-9);
        // A vertical edge between texels 4 and 5, upscaled two times: just left of it EASU stays nearer the
        // dark side than bilinear does, and never leaves the four nearest texels' range.
        let edge = |x: i32, _: i32| if x <= 4 { 0.1 } else { 0.9 };
        let pp = (4.25, 3.0);
        let bilinear = 0.1 + (0.9 - 0.1) * 0.25;
        let fsr = easu(&edge, pp);
        assert!(fsr < bilinear && (0.1..=0.9).contains(&fsr), "{fsr} vs {bilinear}");
        // On the texels themselves it gives them back.
        assert!((easu(&edge, (4.0, 3.0)) - 0.1).abs() < 1e-6 && (easu(&edge, (5.0, 3.0)) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn the_clarity_uniform_steps_by_a_texel_and_says_how_to_sample() {
        use clamshell_state::session::presentation::Clarity;
        use clamshell_state::settings::Scaling;
        let nearest = Clarity { scaling: Scaling::Fsr, integer: true, sharpen: 50, ..Clarity::default() };
        assert_eq!(clarity_uniform(nearest, (960, 540), false), [1.0 / 960.0, 1.0 / 540.0, 0.5, 1.0], "integer: nearest, whatever the scaling");
        assert_eq!(clarity_uniform(Clarity { scaling: Scaling::Fsr, ..Clarity::default() }, (960, 540), false)[3], 2.0);
        assert_eq!(clarity_uniform(Clarity::default(), (960, 540), false)[2..], [0.0, 0.0], "bilinear, unsharpened");
        assert_eq!(clarity_uniform(Clarity::default(), (960, 540), true)[3], 4.0, "FXAA one to one");
        assert_eq!(clarity_uniform(nearest, (0, 0), false), [0.0; 4], "no buffer: as it is");
    }

    fn close(want: [f64; 3], got: [f64; 3], what: &str) {
        for (w, g) in want.iter().zip(got) {
            assert!((w - g).abs() < 1e-5, "{what}: {want:?} != {got:?}");
        }
    }

    const CURVES: [Curve; 3] = [
        Curve { black: 0, contrast: 100, white: 203, start: 100, highlights: 0, saturation: 100 },
        Curve { black: 20, contrast: 110, white: 203, start: 60, highlights: 60, saturation: 105 },
        Curve { black: 150, contrast: 70, white: 400, start: 30, highlights: 100, saturation: 50 },
    ];
    /// An SDR screen, a 650-nit panel with SDR white at the reference, and room for anything.
    const HEADROOMS: [f64; 3] = [1.0, 650.0 / 203.0, 100.0];

    #[test]
    fn the_sdr_shaders_uniforms_give_the_states_curve_on_each_screen() {
        for curve in CURVES {
            for headroom in HEADROOMS {
                for rgb in [[0.0, 0.0, 0.0], [0.03, 0.03, 0.03], [0.5, 0.4, 0.3], [1.0, 1.0, 1.0], [1.0, 0.2, 0.0], [0.9, 0.95, 1.0]] {
                    let got = shaded(rgb, curve_uniforms(Some(&curve), headroom as f32, None));
                    close(curve.map_on(rgb, headroom), got, &format!("{curve:?} {headroom} {rgb:?}"));
                }
            }
        }
        // Without a curve the surface is only decoded.
        assert_eq!(shaded([0.5, 0.5, 0.5], curve_uniforms(None, 1.0, None))[0], shaded([0.5, 0.5, 0.5], curve_uniforms(Some(&CURVES[0]), 1.0, None))[0]);
    }

    #[test]
    fn the_hdr_shaders_uniforms_give_the_states_mapping_on_each_screen() {
        use clamshell_state::curve::map_hdr;
        let peak = 1000.0 / 203.0;
        for curve in CURVES.iter().map(Some).chain([None]) {
            for headroom in HEADROOMS {
                for light in [[0.0, 0.0, 0.0], [0.01, 0.01, 0.01], [0.5, 0.4, 0.3], [1.0, 1.0, 1.0], [3.0, 2.0, 1.0], [peak, peak, peak], [6.0, 0.5, 0.2]] {
                    let got = shaded_hdr(light, curve_uniforms(curve, headroom as f32, Some(peak as f32)));
                    close(map_hdr(curve, light, peak, headroom), got, &format!("{curve:?} {headroom} {light:?}"));
                }
            }
        }
    }
}
