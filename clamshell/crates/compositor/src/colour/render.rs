//! The colour pipeline, through which both hosts draw: every surface is decoded from its own colour into
//! one scene of half floats kept between frames, with its game's curve and clarity applied, and each
//! screen's part of the scene is encoded once in that screen's colour. Only what changed is drawn into the
//! scene. A screen whose whole frame is one surface (a game alone on it) has it drawn straight onto the
//! screen instead, decoded and encoded in one pass: the scene, a full screen of half floats written and
//! read again, is left out, and with nothing to blend the picture is the same.

use crate::{render::Element, server::Server};
use clamshell_state::colour::{ColourDescription, TransferFunction};
use clamshell_state::curve::{Curve, REFERENCE_NITS};
use clamshell_state::settings::AntiAliasing;
use clamshell_ui::Error;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
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
    /// The surfaces whose pictures take those passes this frame, kept so that composing allocates nothing.
    chained: Vec<(Id, WlSurface, AntiAliasing, bool, bool)>,
    /// The histogram of the picture a curve's edit screen plots.
    histogram: super::histogram::Measure,
    /// Each screen's surface drawn straight onto it (the top's, the second's), if its frame is one, and
    /// what of it was drawn last, to know when it changed.
    direct: [Option<Converted>; 2],
    direct_drawn: [Option<(Id, CommitCounter)>; 2],
    /// The shaders decoding a surface and encoding it for a screen in one pass, by the two colours.
    direct_shaders: Vec<((ColourDescription, ColourDescription), GlesTexProgram)>,
}

/// How a window's surface is shown: the HDR curve of the game running it (or none), the headroom of its
/// screen, its clarity settings, and the size of the surface's buffer, which sampling steps by.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    curve: Option<Curve>,
    headroom: f32,
    clarity: clamshell_state::presentation::Clarity,
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
            chained: Vec::new(),
            histogram: Default::default(),
            direct: [None, None],
            direct_drawn: [None, None],
            direct_shaders: Vec::new(),
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

    /// Screen `down`'s encoding (the second's, or the top's).
    pub(crate) fn output_description(&self, down: bool) -> ColourDescription {
        self.output(down).0
    }

    /// Screen `down`'s encoding (the second's, or the top's) and its shader.
    fn output(&self, down: bool) -> &(ColourDescription, GlesTexProgram) {
        let [top, bottom] = &self.outputs;
        if down { bottom } else { top }
    }

    /// Draws what changed in the scene, and refills `states` with the elements' states, handing the last
    /// ones back to the damage tracker to fill next. Each screen whose part is given in `direct` (one with a
    /// window alone on it) has its element kept to be drawn straight onto it when presented, if it is the
    /// only one there. Returns the part of the scene that changed, if anything did, a screen drawn so
    /// changing whole when its element did.
    pub(crate) fn compose(
        &mut self,
        server: &Server,
        renderer: &mut GlesRenderer,
        elements: &mut Vec<Element>,
        states: &mut RenderElementStates,
        direct: [Option<Rectangle<i32, Physical>>; 2],
    ) -> Result<Option<Rectangle<i32, Physical>>, Error> {
        if self.size != server.screens.size {
            let outputs = self.outputs.clone().map(|(description, _)| description);
            *self = Self::new(renderer, server.screens.size)?;
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
        self.chained.clear();
        for placed in server.scene.windows() {
            let Some(client) = server.windows.iter().find(|client| client.id == placed.id) else { continue };
            let curve = client.shown.curve;
            // What the screen shows, as clients are told: nested, the host's real peak rather than the
            // PQ signal we send it.
            let headroom = server.colour.screen(placed.below).headroom() as f32;
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
            let one_to_one = placed.geometry.one_to_one() && picture.as_ref().is_some_and(|(_, unscaled)| *unscaled);
            // FSR shown one to one would upscale nothing: sampled as it is, its sharpening CAS's.
            if one_to_one && clarity.scaling == clamshell_state::settings::Scaling::Fsr {
                clarity.scaling = clamshell_state::settings::Scaling::Bilinear;
            }
            // Anti-aliasing at the texel's centre, as the window is shown one to one or nearest and nothing is
            // sharpened after it, is done in the final pass: FXAA all of it, SMAA its blending.
            let at_centre = (one_to_one || clarity.integer) && clarity.sharpen == 0;
            let fxaa_here = clarity.anti_aliasing == AntiAliasing::Fxaa && at_centre;
            let smaa_fused = clarity.anti_aliasing == AntiAliasing::Smaa && at_centre;
            client.window.with_surfaces(|surface, states| {
                let renderer_state = states.data_map.get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>();
                let buffer = renderer_state.and_then(|state| state.lock().ok()?.buffer_size()).map_or((0, 0), |size| (size.w, size.h));
                self.curves.push((Id::from_wayland_resource(surface), Look { curve, headroom, clarity, buffer, fxaa_here, hdr_peak }));
            });
            // At Integer, nearest-neighbour: no FSR whatever Scaling says.
            let upscale = clarity.scaling == clamshell_state::settings::Scaling::Fsr && clarity.sharpen > 0 && !clarity.integer;
            // FXAA done in the final pass takes no passes before it.
            let aa = if fxaa_here { AntiAliasing::Off } else { clarity.anti_aliasing };
            if let (true, Some((surface, ..))) = (upscale || aa != AntiAliasing::Off, picture) {
                self.chained.push((Id::from_wayland_resource(&surface), surface, aa, upscale, smaa_fused));
            }
        }
        if self.curves != self.last_curves {
            self.damage = OutputDamageTracker::new(self.size, 1.0, Transform::Normal);
            self.first = true;
            std::mem::swap(&mut self.curves, &mut self.last_curves);
        }
        let chained = &self.chained;
        let replaced = self.chains.run(renderer, elements, |id| {
            chained.iter().find(|(chained, ..)| chained == id).map(|(id, surface, aa, upscale, fused)| {
                let hdr = server.colour.surfaces.get(id).is_some_and(|colour| colour.hdr());
                let linear = server.colour.surfaces.get(id).is_some_and(|colour| colour.transfer == TransferFunction::Linear);
                super::chain::Want { surface, smaa: *aa == AntiAliasing::Smaa, fxaa: *aa == AntiAliasing::Fxaa, upscale: *upscale, hdr, linear, fuse: *fused }
            })
        })?;
        self.converted.clear();
        // A screen is drawn straight only while its whole frame is one element. One that no longer is has
        // the scene drawn whole, its part having been left out.
        let one = |part: Rectangle<i32, Physical>| elements.iter().filter(|e| e.geometry(Scale::from(1.0)).overlaps(part)).count() == 1;
        let direct = direct.map(|part| part.filter(|part| one(*part)));
        for (was, now) in self.direct_drawn.iter().zip(&direct) {
            if was.is_some() && now.is_none() {
                self.first = true;
            }
        }
        self.direct = [None, None];
        for inner in elements.drain(..) {
            let colour = server.colour.surfaces.get(inner.id()).map(|colour| **colour).unwrap_or_default();
            let look = self.last_curves.iter().find(|(id, _)| id == inner.id()).map(|(_, look)| *look);
            let (curve, headroom) = look.as_ref().map_or((None, 1.0), |look| (look.curve.as_ref(), look.headroom));
            let peak = hdr_peak(colour, look.map_or(0, |look| look.hdr_peak));
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
            let geometry = inner.geometry(Scale::from(1.0));
            let screen = direct.iter().position(|part| part.is_some_and(|part| geometry.overlaps(part)));
            let program = match screen {
                Some(i) => {
                    let output = self.outputs.get(i).map_or_else(ColourDescription::default, |(output, _)| *output);
                    match self.direct_shaders.iter().find(|(colours, _)| *colours == (colour, output)) {
                        Some((_, program)) => program.clone(),
                        None => {
                            let program = direct_shader(renderer, colour, output)?;
                            if self.direct_shaders.len() == 32 {
                                self.direct_shaders.remove(0);
                            }
                            self.direct_shaders.push(((colour, output), program.clone()));
                            program
                        }
                    }
                }
                None => match self.shaders.iter().find(|(description, _)| *description == colour) {
                    Some((_, program)) => program.clone(),
                    None => {
                        let program = shader(renderer, colour, true)?;
                        if self.shaders.len() == 32 {
                            self.shaders.remove(0);
                        }
                        self.shaders.push((colour, program.clone()));
                        program
                    }
                },
            };
            let smaa_blend = match replaced.iter().find(|(id, _)| id == inner.id()).map(|(_, chained)| chained) {
                Some(super::chain::Chained::Weights(weights)) => Some(weights.clone()),
                _ => None,
            };
            let converted = Converted { inner, shader: program, curve, smaa_blend };
            match screen.and_then(|i| self.direct.get_mut(i)) {
                Some(slot) => *slot = Some(converted),
                None => self.converted.push(converted),
            }
        }
        let mut target = renderer.bind(&mut self.texture)?;
        let result = self.damage.render_output(renderer, &mut target, usize::from(!self.first), &self.converted, [0.0, 0.0, 0.0, 1.0])?;
        let mut changed = result.damage.and_then(|d| d.iter().copied().reduce(|a, b| a.merge(b)));
        let last = std::mem::replace(states, result.states);
        self.damage.reuse_states(last);
        // A screen drawn straight changes whole when its element does, and the element is shown there.
        for ((element, drawn), part) in self.direct.iter().zip(&mut self.direct_drawn).zip(direct) {
            let now = element.as_ref().map(|element| (element.id().clone(), element.current_commit()));
            if let (Some(element), Some(part)) = (element, part) {
                if now != *drawn || self.first {
                    changed = Some(changed.map_or(part, |changed| changed.merge(part)));
                }
                let size = element.geometry(Scale::from(1.0)).size;
                let visible_area = usize::try_from(size.w).unwrap_or(0) * usize::try_from(size.h).unwrap_or(0);
                let presentation_state = smithay::backend::renderer::element::RenderElementPresentationState::Rendering { reason: None };
                let state = smithay::backend::renderer::element::RenderElementState { visible_area, presentation_state, needs_capture: false };
                states.states.insert(element.id().clone(), state);
            }
            *drawn = now;
        }
        // Dropped now rather than at the next compose, so that the UI's next upload holds the only reference
        // to its damage and adds to it in place.
        self.converted.clear();
        self.first = false;
        Ok(changed)
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
        match self.direct.get(usize::from(down)).and_then(Option::as_ref) {
            // The screen's one surface, decoded and encoded for it in one pass.
            Some(element) => {
                let geometry = element.geometry(Scale::from(1.0));
                let dst = Rectangle::new((geometry.loc.x - part.loc.x, geometry.loc.y - part.loc.y).into(), geometry.size);
                element.draw(&mut frame, element.src(), dst, &[Rectangle::from_size(dst.size)], &[], None)?;
            }
            None => {
                frame.render_texture_from_to(&self.texture, part.to_f64(), whole, &[whole], &[], Transform::Normal, 1.0, Some(&self.output(down).1), &[])?;
            }
        }
        Ok(frame.finish()?)
    }

    /// The whole scene in sRGB, for a screenshot.
    pub fn capture(&self, renderer: &mut GlesRenderer) -> Result<clamshell_state::ScreenshotPixels, String> {
        let program = shader(renderer, ColourDescription::default(), false).map_err(|error| error.to_string())?;
        let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, self.size.into()).map_err(|error| error.to_string())?;
        let mut target = renderer.bind(&mut texture).map_err(|error| error.to_string())?;
        self.present_with(renderer, &mut target, &program).map_err(|error| error.to_string())?;
        read_back(renderer, &target, self.size)
    }
}

/// Readback is an SDR file export; clients and the composition target retain their native precision.
fn read_back(
    renderer: &mut GlesRenderer,
    target: &smithay::backend::renderer::gles::GlesTarget<'_>,
    size: (i32, i32),
) -> Result<clamshell_state::ScreenshotPixels, String> {
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
    Ok(clamshell_state::ScreenshotPixels { size: (width, height), rgba })
}

/// Whether a surface of `description` is SDR: an sRGB or power-law signal, the curve applied to its signal
/// (`Curve::map_on`).
pub(crate) fn sdr(description: ColourDescription) -> bool {
    matches!(description.transfer, TransferFunction::Srgb | TransferFunction::Gamma(_))
}

/// Whether a surface of `description` is HDR: PQ or linear light (scRGB's), the curve applied to its light
/// and its highlights compressed into the screen (`curve::map_hdr`).
pub(crate) fn hdr(description: ColourDescription) -> bool {
    description.hdr()
}

/// A window's clarity as the input shaders' uniform: one texel of its buffer across and down, the
/// sharpening's strength (0 to 1), and how it is sampled (0 bilinear, 1 nearest, 2 FSR's upscale; 3, from
/// the chain, FSR's sharpening of what it upscaled, see [`super::chain`]; 4, `fxaa_here`, FXAA at the
/// texel's centre; 5, from the chain, SMAA's blending at the texel's centre).
/// Nothing without a buffer's size.
fn clarity_uniform(clarity: clamshell_state::presentation::Clarity, (w, h): (i32, i32), fxaa_here: bool) -> [f32; 4] {
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

/// An HDR surface's peak, in units of its white: `set` for its program (in nits; 0 for none), else what it
/// declares or is assumed. None for an SDR surface.
pub(crate) fn hdr_peak(colour: ColourDescription, set: u16) -> Option<f32> {
    hdr(colour).then(|| match set {
        0 => colour.peak() as f32,
        set => f32::from(set) / colour.luminances().reference.max(1) as f32,
    })
}

/// A tone curve as the input shaders' uniforms: black point, contrast, where highlights start, and for an
/// SDR surface their peak (in units of white: the curve's share of the screen's room above white), for an
/// HDR one its own peak (`hdr_peak`, in units of its white); then white over the reference, saturation,
/// the screen's headroom, and whether a curve is on.
/// Without one, an SDR surface is only decoded and an HDR one compressed into the screen.
pub(crate) fn curve_uniforms(curve: Option<&Curve>, headroom: f32, hdr_peak: Option<f32>) -> [[f32; 4]; 2] {
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

/// The shader drawing a surface of `colour` straight onto a screen of `output` (see [`Pipeline::compose`]).
fn direct_shader(renderer: &mut GlesRenderer, colour: ColourDescription, output: ColourDescription) -> Result<GlesTexProgram, Error> {
    let curved = sdr(colour) || hdr(colour);
    let source = super::shaders::direct(colour, output, curved)?;
    let names = [
        UniformName::new("curve_a", UniformType::_4f),
        UniformName::new("curve_b", UniformType::_4f),
        UniformName::new("clarity", UniformType::_4f),
        UniformName::new("smaa_blend_tex", UniformType::_1i),
    ];
    Ok(renderer.compile_custom_texture_shader(source, if curved { &names } else { &[] })?)
}

fn shader(renderer: &mut GlesRenderer, description: ColourDescription, input: bool) -> Result<GlesTexProgram, Error> {
    let curved = input && (sdr(description) || hdr(description));
    let source = super::shaders::surface(description, input, curved)?;
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

    /// The SDR input shader's arithmetic for an sRGB surface, as `shaders` writes it in GLSL.
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
    /// white), as `shaders` writes it in GLSL.
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
    /// right and below it, at `sharpness` (0 to 1), as `shaders` writes it in GLSL.
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
    /// edges), at input position `pp` (texels, 0 at the first texel's centre), as `shaders` writes it in GLSL.
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
    /// right and below it, at strength `s` (0 to 1), as `shaders` writes it in GLSL.
    fn rcas(e: f64, [b, d, f, h]: [f64; 4], s: f64) -> f64 {
        let (mn4, mx4) = (b.min(d).min(f).min(h), b.max(d).max(f).max(h));
        let hit_min = mn4.min(e) / (4.0 * mx4).max(0.000001);
        let hit_max = (1.0 - mx4.max(e)) / (4.0 * mn4 - 4.0).min(-0.000001);
        let lobe = (-hit_min).max(hit_max).clamp(-0.1875, 0.0) * (-2.0 * (1.0 - s)).exp2();
        (lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0)
    }

    /// FXAA of a one-channel picture (`at` a texel's value, between texels bilinear, clamped at the edges)
    /// at texel `(x, y)`, as `shaders` writes it in GLSL.
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
        use clamshell_state::presentation::Clarity;
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

#[cfg(test)]
mod direct_tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, unsafe_code)]
    use super::*;
    use clamshell_state::colour::ColourPrimaries;
    use smithay::backend::renderer::{ExportMem, ImportMem};

    /// A GLES renderer on the machine's render node.
    fn renderer() -> GlesRenderer {
        let file = std::fs::OpenOptions::new().read(true).write(true).open("/dev/dri/renderD128").unwrap();
        let gbm = smithay::backend::allocator::gbm::GbmDevice::new(smithay::utils::DeviceFd::from(std::os::fd::OwnedFd::from(file))).unwrap();
        // SAFETY: the GBM device lives as long as the display, which the context and renderer keep.
        let display = unsafe { smithay::backend::egl::EGLDisplay::new(gbm) }.unwrap();
        let context = smithay::backend::egl::EGLContext::new(&display).unwrap();
        // SAFETY: the context was made on this thread, for this renderer alone.
        unsafe { GlesRenderer::new(context) }.unwrap()
    }

    /// `texture` drawn whole by `program` with `uniforms` into a new texture of `format`.
    fn draw(renderer: &mut GlesRenderer, texture: &GlesTexture, program: &GlesTexProgram, uniforms: &[Uniform<'_>], format: Fourcc) -> GlesTexture {
        let size = texture.size();
        let mut out: GlesTexture = renderer.create_buffer(format, size).unwrap();
        {
            let mut target = renderer.bind(&mut out).unwrap();
            let mut frame = renderer.render(&mut target, (size.w, size.h).into(), Transform::Normal).unwrap();
            let whole = Rectangle::<i32, Physical>::from_size((size.w, size.h).into());
            frame
                .render_texture_from_to(texture, Rectangle::from_size(size).to_f64(), whole, &[whole], &[], Transform::Normal, 1.0, Some(program), uniforms)
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        out
    }

    fn pixels(renderer: &mut GlesRenderer, texture: &GlesTexture) -> Vec<u8> {
        let mapping = renderer.copy_texture(texture, Rectangle::from_size(texture.size()), Fourcc::Abgr8888).unwrap();
        renderer.map_texture(&mapping).unwrap().to_vec()
    }

    /// A surface drawn straight onto a screen shows as the final pass would, as `scanout::Transform` works it
    /// out on the CPU, within a level: an sRGB surface on an unprofiled panel and on the Thor's top panel,
    /// and an HDR one (PQ) on the Thor's top panel. (Through the scene it is further off near black: the
    /// scene's half floats, in BT.2020's primaries, leave a little of the other channels where the screen's
    /// primaries have none, which the encoding's steep start makes up to a few levels.)
    #[test]
    fn a_surface_drawn_straight_is_the_final_pass() {
        let mut renderer = renderer();
        let picture: Vec<u8> = (0..256u32).flat_map(|i| [i as u8, (255 - i) as u8, (i * 7 % 256) as u8, 255]).collect();
        let texture = renderer.import_memory(&picture, Fourcc::Abgr8888, (256, 1).into(), false).unwrap();
        let srgb = ColourDescription::default();
        let pq = ColourDescription { primaries: ColourPrimaries::Bt2020, transfer: TransferFunction::Pq, ..ColourDescription::default() };
        let panel = ColourDescription::from(clamshell_device::colour::PanelColour::srgb());
        let thor = ColourDescription::from(clamshell_device::DeviceProfile::thor().top.colour);
        for (surface, output) in [(srgb, panel), (srgb, thor), (pq, thor)] {
            let curve = curve_uniforms(None, output.headroom() as f32, hdr_peak(surface, 0));
            let [a, b] = curve;
            let uniforms = [
                Uniform::new("curve_a", a),
                Uniform::new("curve_b", b),
                Uniform::new("clarity", [0.0f32; 4]),
                Uniform::new("smaa_blend_tex", smithay::backend::renderer::gles::UniformValue::_1i(1)),
            ];
            let program = direct_shader(&mut renderer, surface, output).unwrap();
            let drawn = draw(&mut renderer, &texture, &program, &uniforms, Fourcc::Abgr8888);
            let drawn = pixels(&mut renderer, &drawn);
            let transform = super::super::scanout::Transform::new(surface, curve, output).unwrap();
            for (i, px) in picture.as_chunks::<4>().0.iter().enumerate() {
                let want = transform.apply([0, 1, 2].map(|c| f64::from(px[c]) / 255.0)).map(|c| c.clamp(0.0, 1.0) * 255.0);
                for c in 0..3 {
                    let got = f64::from(drawn[i * 4 + c]);
                    assert!((got - want[c]).abs() <= 1.0, "{surface:?} on {output:?}: pixel {i} channel {c} {got}, not {:.2}", want[c]);
                }
            }
        }
    }
}
