//! The colour pipeline, through which both hosts draw. Each screen has its own scene, composed on its own:
//! its surfaces decoded from their own colour into half floats kept between frames, with each game's curve
//! and clarity applied, and encoded once in the screen's colour when presented. Only what changed is drawn
//! into a scene, and a screen that changed nothing draws nothing. A scene's texture is made when its screen
//! composes through it and dropped a while after it last did (`crate::hold`): a screen showing a game
//! alone, or ClamShell alone from its own buffers, holds none. A screen whose whole frame is one surface
//! (a game alone on it) has it drawn straight onto the screen instead, decoded and encoded in one pass: the
//! scene, a full screen of half floats written and read again, is left out, and with nothing to blend the
//! picture is the same.

use crate::{render::Element, server::Server};
use clamshell_state::colour::{ColourDescription, ColourPrimaries, TransferFunction};
use clamshell_state::curve::{Curve, REFERENCE_NITS};
use clamshell_state::settings::AntiAliasing;
use clamshell_ui::Error;
use smithay::backend::renderer::sync::SyncPoint;
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
    /// The top screen's scene and the second's.
    scenes: [Scene; 2],
    /// The shaders decoding a surface into a scene, by its kind of colour (see [`program_colour`]) and the
    /// scene's primaries.
    shaders: Vec<((ColourDescription, ColourPrimaries), GlesTexProgram)>,
    /// The shaders decoding a surface and encoding it for a screen in one pass, by its kind of colour and the
    /// screen's.
    direct_shaders: Vec<((ColourDescription, ColourDescription), GlesTexProgram)>,
    revision: u64,
    /// The surfaces whose pictures take passes before the scene this frame, kept so that composing
    /// allocates nothing.
    chained: Vec<(Id, WlSurface, AntiAliasing, bool, bool)>,
    /// The histogram of the picture a curve's edit screen plots.
    histogram: super::histogram::Measure,
    /// What a screenshot is drawn with, kept between screenshots taken close together (see `crate::hold`).
    capture: Option<Capture>,
    capture_hold: crate::hold::Hold,
    /// A screenshot started, waiting for the GPU (see [`Pipeline::captured`]).
    reading: Option<CaptureReading>,
}

/// A scene as a screen other than its panel shows it (an external display mirroring it): encoded in that
/// screen's colour, at `dst` on it. Made when the screen comes, for the scene's primaries then.
pub(crate) struct Encoding {
    program: GlesTexProgram,
    dst: Rectangle<i32, Physical>,
}

/// A screenshot's texture, of the layout's size, and the shaders encoding each scene, by its primaries, in
/// sRGB for it.
struct Capture {
    texture: GlesTexture,
    size: (i32, i32),
    programs: [(ColourPrimaries, GlesTexProgram); 2],
}

/// A screen's scene: its surfaces decoded into half floats, kept between frames, of which only what changed
/// is drawn; its encoding; and, if its frame is one surface, that surface drawn straight onto the screen.
struct Scene {
    /// The screen's area in the layout of both screens; its elements are moved from there to the scene's
    /// origin.
    part: Rectangle<i32, Physical>,
    /// Its texture while it is used, and when it last was.
    texture: Option<GlesTexture>,
    hold: crate::hold::Hold,
    damage: OutputDamageTracker,
    first: bool,
    converted: Vec<Converted>,
    /// The screen's encoding, and its shader.
    output: (ColourDescription, GlesTexProgram),
    /// Its windows' surfaces with how the game running each is shown, this frame and the last: a change
    /// redraws the scene, since the surfaces themselves didn't.
    curves: Vec<(Id, Look)>,
    last_curves: Vec<(Id, Look)>,
    /// Its windows whose pictures take passes before the scene (SMAA, FXAA, FSR), and their textures,
    /// dropped a while after it last composed.
    chains: super::chain::Chains,
    chains_hold: crate::hold::Hold,
    /// The surface drawn straight onto the screen, and what of it was drawn last, to know when it changed.
    /// The element is let go at the end of the turn (see [`Pipeline::let_go`]).
    direct: Option<Converted>,
    direct_drawn: Option<(Id, CommitCounter)>,
    /// Its draws' uniforms' list (see [`Converted::uniforms`]).
    uniforms: Uniforms,
}

impl Scene {
    fn new(part: Rectangle<i32, Physical>, output: (ColourDescription, GlesTexProgram)) -> Scene {
        let size = (part.size.w.max(1), part.size.h.max(1));
        Scene {
            part,
            texture: None,
            hold: Default::default(),
            damage: OutputDamageTracker::new(size, 1.0, Transform::Normal),
            first: true,
            converted: Vec::new(),
            output,
            curves: Vec::new(),
            last_curves: Vec::new(),
            chains: Default::default(),
            chains_hold: Default::default(),
            direct: None,
            direct_drawn: None,
            uniforms: Default::default(),
        }
    }

    /// Draws all of it at its next compose.
    fn redraw(&mut self) {
        let size = (self.part.size.w.max(1), self.part.size.h.max(1));
        self.damage = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        self.first = true;
    }
}

/// A scene's size in megabytes, at `size` in half floats.
fn megabytes((w, h): (i32, i32)) -> f64 {
    f64::from(w.max(0)) * f64::from(h.max(0)) * 8.0 / 1_000_000.0
}

/// Each screen's area in the layout of both: the top screen's at the origin, the second's to its right.
fn parts(server: &Server) -> [Rectangle<i32, Physical>; 2] {
    let (width, height) = server.screens.size;
    let top = server.screens.top_width;
    [Rectangle::new((0, 0).into(), (top, height).into()), Rectangle::new((top, 0).into(), ((width - top).max(1), height).into())]
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
    /// Where its scene's origin is in the layout of both screens, which its geometry is taken from.
    offset: Point<i32, Physical>,
    shader: GlesTexProgram,
    /// For an SDR or HDR surface's shader: its curve's uniforms (see [`curve_uniforms`]) and its clarity's
    /// (see [`clarity_uniform`]).
    curve: Option<[[f32; 4]; 3]>,
    /// Its colour's uniforms (see [`decode_uniforms`]).
    decode: Decode,
    /// SMAA's blending weights, when its blending is done in this pass: bound to the second texture unit
    /// while the element is drawn, as Smithay's programs take one texture (`docs/picture.md`).
    smaa_blend: Option<GlesTexture>,
    /// The uniforms' list, its scene's, taken for a draw and given back after, so that drawing allocates
    /// nothing.
    uniforms: Uniforms,
}

/// A scene's list of a draw's uniforms, shared by its elements, which draw one at a time.
type Uniforms = std::rc::Rc<std::cell::RefCell<Vec<Uniform<'static>>>>;
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
        let geometry = self.inner.geometry(scale);
        Rectangle::new(geometry.loc - self.offset, geometry.size)
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale) - self.offset
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
        let mut uniforms = std::mem::take(&mut *self.uniforms.borrow_mut());
        uniforms.clear();
        if let Some([a, b, c]) = self.curve {
            // The weights' sampler is always on the second unit: left on the first, beside an external
            // texture, it would be a GL error at the draw.
            let second = smithay::backend::renderer::gles::UniformValue::_1i(1);
            uniforms.extend([Uniform::new("curve_a", a), Uniform::new("curve_b", b), Uniform::new("clarity", c), Uniform::new("smaa_blend_tex", second)]);
            self.decode.push(&mut uniforms);
        }
        if let Some(weights) = &self.smaa_blend {
            bind_second(frame, Some(weights.tex_id()))?;
        }
        frame.override_default_tex_program(self.shader.clone(), uniforms);
        let result = self.inner.draw(frame, src, dst, damage, opaque, cache);
        if let Some((_, uniforms)) = frame.take_tex_program_override() {
            *self.uniforms.borrow_mut() = uniforms;
        }
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
    /// Measures the histogram if it is time to, and returns one measured since the last call, for the state
    /// (see [`super::histogram`]). Once a turn, after the screens composed (which brings the window's
    /// texture up to date).
    pub(crate) fn take_histogram(&mut self, renderer: &mut GlesRenderer, server: &Server) -> Option<clamshell_state::colour::Histogram> {
        self.histogram.turn(renderer, server, std::time::Instant::now());
        self.histogram.measured.take()
    }

    /// The scenes of the screens laid out as `server` has them.
    pub(crate) fn new(renderer: &mut GlesRenderer, server: &Server) -> Result<Self, Error> {
        Self::with_parts(renderer, parts(server))
    }

    /// The scenes of screens at `parts` in the layout of both (see [`parts`]).
    fn with_parts(renderer: &mut GlesRenderer, [top, bottom]: [Rectangle<i32, Physical>; 2]) -> Result<Self, Error> {
        let output = ColourDescription::default();
        let program = encode_shader(renderer, output.primaries, output)?;
        Ok(Self {
            scenes: [Scene::new(top, (output, program.clone())), Scene::new(bottom, (output, program))],
            shaders: Vec::new(),
            direct_shaders: Vec::new(),
            revision: 0,
            chained: Vec::new(),
            histogram: Default::default(),
            capture: None,
            capture_hold: Default::default(),
            reading: None,
        })
    }

    /// Each screen's encoding: the top's, then the second's.
    pub(crate) fn set_outputs(&mut self, renderer: &mut GlesRenderer, outputs: [ColourDescription; 2]) -> Result<(), Error> {
        for (scene, output) in self.scenes.iter_mut().zip(outputs) {
            if scene.output.0 != output {
                // A scene is in its screen's primaries: with new ones, what it holds is wrong.
                if scene.output.0.primaries != output.primaries {
                    scene.redraw();
                }
                scene.output = (output, encode_shader(renderer, output.primaries, output)?);
            }
        }
        Ok(())
    }

    /// Screen `down`'s encoding (the second's, or the top's).
    pub(crate) fn output_description(&self, down: bool) -> ColourDescription {
        self.scene(down).output.0
    }

    fn scene(&self, down: bool) -> &Scene {
        let [top, bottom] = &self.scenes;
        if down { bottom } else { top }
    }

    /// Draws what changed in screen `down`'s scene, `elements` being the screen's (see
    /// `render::elements`), and refills `states` with their states, handing the last ones back to the
    /// scene's damage tracker to fill next. With `direct` (a window alone on the screen), a frame of one
    /// element has it kept to be drawn straight onto the screen when presented, rather than into the
    /// scene. Returns whether the screen's picture changed.
    pub(crate) fn compose(
        &mut self,
        server: &Server,
        renderer: &mut GlesRenderer,
        down: bool,
        elements: &mut Vec<Element>,
        states: &mut RenderElementStates,
        direct: bool,
    ) -> Result<bool, Error> {
        let [top_part, bottom_part] = parts(server);
        let part = if down { bottom_part } else { top_part };
        if self.revision != server.colour.revision {
            for scene in &mut self.scenes {
                scene.redraw();
            }
            self.revision = server.colour.revision;
        }
        let Pipeline { scenes, shaders, direct_shaders, chained, .. } = self;
        let [top, bottom] = scenes;
        let scene = if down { bottom } else { top };
        if scene.part != part {
            if scene.texture.is_some() {
                log::info!("colour: the {} screen's scene dropped for the screens' new layout", if down { "second" } else { "top" });
            }
            *scene = Scene::new(part, scene.output.clone());
        }
        scene.curves.clear();
        // The surface holding each window's picture, if it takes passes before the scene (see `super::chain`):
        // SMAA, and with FSR and sharpening, FSR's upscale.
        chained.clear();
        for placed in server.scene.windows().filter(|placed| placed.below == down) {
            let Some(client) = server.windows.iter().find(|client| client.id == placed.id) else { continue };
            let curve = client.shown.curve;
            // What the screen shows, as clients are told: nested, the host's real peak rather than the
            // PQ signal we send it.
            let headroom = server.colour.screen(placed.below).headroom() as f32;
            let mut clarity = client.shown.clarity;
            let hdr_peak = client.shown.hdr_peak;
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
                scene.curves.push((Id::from_wayland_resource(surface), Look { curve, headroom, clarity, buffer, fxaa_here, hdr_peak }));
            });
            // At Integer, nearest-neighbour: no FSR whatever Scaling says.
            let upscale = clarity.scaling == clamshell_state::settings::Scaling::Fsr && clarity.sharpen > 0 && !clarity.integer;
            // FXAA done in the final pass takes no passes before it.
            let aa = if fxaa_here { AntiAliasing::Off } else { clarity.anti_aliasing };
            if let (true, Some((surface, ..))) = (upscale || aa != AntiAliasing::Off, picture) {
                chained.push((Id::from_wayland_resource(&surface), surface, aa, upscale, smaa_fused));
            }
        }
        if scene.curves != scene.last_curves {
            scene.redraw();
            std::mem::swap(&mut scene.curves, &mut scene.last_curves);
        }
        // Drawn straight only while the whole frame is one element (the passes replace elements, not add
        // them). A scene drawn straight before is drawn whole again, the element having been left out of it.
        let direct = direct && elements.len() == 1;
        if scene.direct_drawn.is_some() && !direct {
            scene.redraw();
        }
        scene.chains_hold.used(std::time::Instant::now());
        let replaced = scene.chains.run(renderer, elements, |id| {
            chained.iter().find(|(chained, ..)| chained == id).map(|(id, surface, aa, upscale, fused)| {
                let hdr = server.colour.surfaces.get(id).is_some_and(|colour| colour.hdr());
                let linear = server.colour.surfaces.get(id).is_some_and(|colour| colour.transfer == TransferFunction::Linear);
                super::chain::Want { surface, smaa: *aa == AntiAliasing::Smaa, fxaa: *aa == AntiAliasing::Fxaa, upscale: *upscale, hdr, linear, fuse: *fused }
            })
        })?;
        scene.converted.clear();
        scene.direct = None;
        for inner in elements.drain(..) {
            let colour = server.colour.surfaces.get(inner.id()).map(|colour| **colour).unwrap_or_default();
            let look = scene.last_curves.iter().find(|(id, _)| id == inner.id()).map(|(_, look)| *look);
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
            let decode = decode_uniforms(colour)?;
            // Order: the program after its uniforms, which refuse what it couldn't draw (primaries without a
            // matrix to BT.2020).
            let kind = program_colour(colour);
            let program = if direct {
                let output = scene.output.0;
                cached(direct_shaders, (kind, output), || direct_shader(renderer, kind, output))?
            } else {
                let primaries = scene.output.0.primaries;
                cached(shaders, (kind, primaries), || decode_shader(renderer, kind, primaries))?
            };
            let smaa_blend = match replaced.iter().find(|(id, _)| id == inner.id()).map(|(_, chained)| chained) {
                Some(super::chain::Chained::Weights(weights)) => Some(weights.clone()),
                _ => None,
            };
            let converted = Converted { inner, offset: part.loc, shader: program, curve, decode, smaa_blend, uniforms: scene.uniforms.clone() };
            if direct {
                scene.direct = Some(converted);
            } else {
                scene.converted.push(converted);
            }
        }
        let mut changed = false;
        if direct {
            // The scene isn't drawn: its texture, if it has one, goes when its hold runs out.
            states.states.clear();
        } else {
            if scene.texture.is_none() {
                let size = (part.size.w.max(1), part.size.h.max(1));
                scene.texture = Some(renderer.create_buffer(Fourcc::Abgr16161616f, size.into())?);
                scene.redraw();
                log::info!("colour: the {} screen's scene made, {:.1} MB", if down { "second" } else { "top" }, megabytes(size));
            }
            scene.hold.used(std::time::Instant::now());
            if let Some(texture) = &mut scene.texture {
                let mut target = renderer.bind(texture)?;
                let result = scene.damage.render_output(renderer, &mut target, usize::from(!scene.first), &scene.converted, [0.0, 0.0, 0.0, 1.0])?;
                changed = result.damage.is_some_and(|damage| !damage.is_empty());
                let last = std::mem::replace(states, result.states);
                scene.damage.reuse_states(last);
            }
        }
        // Drawn straight, the screen changes whole when its element does, and the element is shown there.
        let now = scene.direct.as_ref().map(|element| (element.id().clone(), element.current_commit()));
        if let Some(element) = &scene.direct {
            changed |= now != scene.direct_drawn || scene.first;
            let size = element.geometry(Scale::from(1.0)).size;
            let visible_area = usize::try_from(size.w).unwrap_or(0) * usize::try_from(size.h).unwrap_or(0);
            let presentation_state = smithay::backend::renderer::element::RenderElementPresentationState::Rendering { reason: None };
            let state = smithay::backend::renderer::element::RenderElementState { visible_area, presentation_state, needs_capture: false };
            states.states.insert(element.id().clone(), state);
        }
        scene.direct_drawn = now;
        // Dropped now rather than at the next compose, so that the UI's next upload holds the only reference
        // to its damage and adds to it in place.
        scene.converted.clear();
        scene.first = false;
        Ok(changed)
    }

    /// Nested keeps both composed scenes displayed between redraws.
    #[cfg(feature = "nested")]
    pub(crate) fn keep_composed(&mut self, now: std::time::Instant) {
        for scene in &mut self.scenes {
            if scene.texture.is_some() {
                scene.hold.used(now);
            }
        }
    }

    #[cfg(feature = "nested")]
    pub(crate) fn histogram_due(&self, server: &Server, now: std::time::Instant) -> Option<std::time::Instant> {
        self.histogram.deadline(server.core.state().histogram_window().is_some(), now)
    }

    /// Drops the textures of scenes not composed through for a while by `now`, the textures of their
    /// picture passes, and the screenshot's (see `crate::hold`).
    pub(crate) fn trim(&mut self, now: std::time::Instant, trims: &mut crate::hold::Trims) {
        if self.capture.is_some() && self.capture_hold.expired(now) {
            log::info!("colour: the screenshot texture dropped");
            self.capture = None;
            trims.dropped();
        }
        trims.held(&self.capture_hold);
        for (scene, name) in self.scenes.iter_mut().zip(["top", "second"]) {
            if scene.texture.is_some() && scene.hold.expired(now) {
                log::info!("colour: the {name} screen's scene dropped, {:.1} MB", megabytes((scene.part.size.w, scene.part.size.h)));
                scene.texture = None;
                scene.first = true;
                trims.dropped();
            }
            trims.held(&scene.hold);
            if scene.chains_hold.expired(now) && scene.chains.drop_textures() {
                log::info!("colour: the {name} screen's picture passes' textures dropped");
                trims.dropped();
            }
            trims.held(&scene.chains_hold);
        }
    }

    /// Lets go of the surfaces drawn straight this turn, once presented or not: they hold their client's
    /// buffer, which it would otherwise not get back while the screen doesn't compose, and a reference to
    /// its damage, which its next commit would then copy. The next compose takes them again.
    pub(crate) fn let_go(&mut self) {
        for scene in &mut self.scenes {
            scene.direct = None;
        }
    }

    /// Draws every scene whole at its next compose, as after the screens were away.
    pub(crate) fn redraw_all(&mut self) {
        for scene in &mut self.scenes {
            scene.redraw();
        }
    }

    #[cfg(feature = "nested")]
    /// Both scenes side by side, each in its screen's encoding, as the nested host shows both screens in
    /// one window.
    pub(crate) fn present(&self, renderer: &mut GlesRenderer, target: &mut GlesTarget<'_>, size: (i32, i32)) -> Result<SyncPoint, Error> {
        let [top, bottom] = &self.scenes;
        self.present_with(renderer, target, size, [&top.output.1, &bottom.output.1])
    }

    /// Both scenes side by side into `target`, of the layout's `size`, the top's encoded by the first of
    /// `programs` and the second's by the other.
    fn present_with(
        &self,
        renderer: &mut GlesRenderer,
        target: &mut GlesTarget<'_>,
        size: (i32, i32),
        programs: [&GlesTexProgram; 2],
    ) -> Result<SyncPoint, Error> {
        let mut frame = renderer.render(target, size.into(), Transform::Flipped180)?;
        let rectangle = Rectangle::from_size(size.into());
        frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[rectangle])?;
        for (scene, program) in self.scenes.iter().zip(programs) {
            let Some(texture) = &scene.texture else { continue };
            let whole = Rectangle::from_size(texture.size()).to_f64();
            // The damage is relative to the destination: the second scene's part is off the origin.
            let all = Rectangle::from_size(scene.part.size);
            frame.render_texture_from_to(texture, whole, scene.part, &[all], &[], Transform::Normal, 1.0, Some(program), &[])?;
        }
        Ok(frame.finish()?)
    }

    /// Scene `down`'s encoding for a screen of `output` other than its panel, drawn at `dst` on it.
    pub(crate) fn encoding(
        &self,
        renderer: &mut GlesRenderer,
        down: bool,
        output: ColourDescription,
        dst: Rectangle<i32, Physical>,
    ) -> Result<Encoding, Error> {
        Ok(Encoding { program: encode_shader(renderer, self.scene(down).output.0.primaries, output)?, dst })
    }

    /// Draws screen `down`'s scene onto its framebuffer (the second, or the top) in its encoding, `size`
    /// before the output's `transform` (a panel mounted turned), returning when the GPU will be done with
    /// it. With `encoding`, onto another screen showing it (see [`Encoding`]), from the scene only.
    pub(crate) fn present_screen(
        &self,
        renderer: &mut GlesRenderer,
        target: &mut GlesTarget<'_>,
        down: bool,
        size: smithay::utils::Size<i32, Physical>,
        transform: Transform,
        encoding: Option<&Encoding>,
    ) -> Result<smithay::backend::renderer::sync::SyncPoint, Error> {
        let scene = self.scene(down);
        let mut frame = renderer.render(target, size, transform)?;
        let whole = Rectangle::from_size(transform.transform_size(size));
        frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[whole])?;
        let (program, dst) = encoding.map_or((&scene.output.1, whole), |encoding| (&encoding.program, encoding.dst));
        match &scene.direct {
            // The screen's one surface, decoded and encoded for it in one pass.
            Some(element) if encoding.is_none() => {
                let dst = element.geometry(Scale::from(1.0));
                element.draw(&mut frame, element.src(), dst, &[Rectangle::from_size(dst.size)], &[], None)?;
            }
            _ => {
                if let Some(texture) = &scene.texture {
                    let all = Rectangle::from_size(texture.size()).to_f64();
                    // The damage is relative to the destination, which bars put off the origin.
                    let damage = Rectangle::from_size(dst.size);
                    frame.render_texture_from_to(texture, all, dst, &[damage], &[], Transform::Normal, 1.0, Some(program), &[])?;
                }
            }
        }
        Ok(frame.finish()?)
    }

    /// Starts a screenshot: both scenes side by side in sRGB, `size` being the layout's, copied out once the
    /// GPU has drawn them, which [`Pipeline::captured`] then reads. What it is drawn with is kept for the next
    /// taken soon after.
    pub fn capture(&mut self, renderer: &mut GlesRenderer, size: (i32, i32)) -> Result<(), String> {
        if self.reading.is_some() {
            return Err("another screenshot is being taken".into());
        }
        let primaries = self.scenes.each_ref().map(|scene| scene.output.0.primaries);
        let fits = self.capture.as_ref().is_some_and(|capture| capture.size == size && capture.programs.each_ref().map(|(p, _)| *p) == primaries);
        if !fits {
            let srgb =
                |primaries, renderer: &mut GlesRenderer| encode_shader(renderer, primaries, ColourDescription::default()).map_err(|error| error.to_string());
            let [top, bottom] = primaries;
            let programs = [(top, srgb(top, renderer)?), (bottom, srgb(bottom, renderer)?)];
            let texture = renderer.create_buffer(Fourcc::Abgr8888, size.into()).map_err(|error| error.to_string())?;
            self.capture = Some(Capture { texture, size, programs });
        }
        self.capture_hold.used(std::time::Instant::now());
        let mut capture = self.capture.take().ok_or("no capture texture")?;
        let result = (|| {
            let mut target = renderer.bind(&mut capture.texture).map_err(|error| error.to_string())?;
            let [(_, top), (_, bottom)] = &capture.programs;
            drop(self.present_with(renderer, &mut target, size, [top, bottom]).map_err(|error| error.to_string())?);
            let mapping = smithay::backend::renderer::ExportMem::copy_framebuffer(renderer, &target, Rectangle::from_size(size.into()), Fourcc::Abgr8888)
                .map_err(|e| e.to_string())?;
            drop(target);
            let fence =
                super::histogram::fence(renderer).inspect_err(|error| log::info!("screenshot: no fence for its readback, so it is read at once: {error}")).ok();
            self.reading = Some(CaptureReading { mapping, fence, size });
            Ok(())
        })();
        self.capture = Some(capture);
        result
    }

    /// The screenshot started by [`Pipeline::capture`], once the GPU has drawn it, or at once with `wait`
    /// (a host that waits anyway). None while there is none, or it isn't done yet (see
    /// [`Pipeline::capture_due`]).
    pub fn captured(&mut self, renderer: &mut GlesRenderer, wait: bool) -> Option<Result<clamshell_state::ScreenshotPixels, String>> {
        let reading = self.reading.take()?;
        if !wait && !super::histogram::copied(renderer, reading.fence.as_ref(), "the screenshot") {
            self.reading = Some(reading);
            return None;
        }
        Some(read_back(renderer, &reading.mapping, reading.size))
    }

    /// When to look again whether a screenshot started is drawn: soon, while there is one.
    pub fn capture_due(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        self.reading.as_ref().map(|_| now + std::time::Duration::from_millis(2))
    }
}

/// A screenshot's readback, read once its fence says the GPU has done the copy.
struct CaptureReading {
    mapping: <GlesRenderer as smithay::backend::renderer::ExportMem>::TextureMapping,
    fence: Option<smithay::backend::egl::fence::EGLFence>,
    size: (i32, i32),
}

/// Readback is an SDR file export; clients and the composition target retain their native precision.
fn read_back(
    renderer: &mut GlesRenderer,
    mapping: &<GlesRenderer as smithay::backend::renderer::ExportMem>::TextureMapping,
    size: (i32, i32),
) -> Result<clamshell_state::ScreenshotPixels, String> {
    use smithay::backend::renderer::{ExportMem, TextureMapping};
    let width = u32::try_from(size.0).map_err(|e| e.to_string())?;
    let height = u32::try_from(size.1).map_err(|e| e.to_string())?;
    let stride = usize::try_from(width).ok().and_then(|w| w.checked_mul(4)).filter(|n| *n != 0).ok_or("invalid capture width")?;
    let flipped = mapping.flipped();
    let bytes = renderer.map_texture(mapping).map_err(|e| e.to_string())?;
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

/// The description a final pass's program is made for, standing for all with the same kind of transfer
/// function: what else a surface's description holds (its primaries, luminances, a power law's exponent),
/// which its client chooses freely, is given to the program as uniforms ([`decode_uniforms`]). So the
/// programs are a few, and no client's new description compiles one on the compositor's thread.
pub(crate) fn program_colour(colour: ColourDescription) -> ColourDescription {
    let transfer = match colour.transfer {
        TransferFunction::Gamma(_) => TransferFunction::Gamma(22000),
        transfer => transfer,
    };
    ColourDescription { primaries: ColourPrimaries::Bt2020, transfer, luminance: None, target: None }
}

/// A surface's colour as its final pass's uniforms: the matrix from its primaries to BT.2020 (its columns,
/// `decode_m0` to `decode_m2`), and `decode_k`: the scaling of its decoded light to units of its reference
/// white, its power law's exponent, and PQ's nits over its reference white (see `shaders::decoding`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Decode {
    columns: [[f32; 3]; 3],
    k: [f32; 4],
}
impl Decode {
    fn push(&self, uniforms: &mut Vec<Uniform<'static>>) {
        let [m0, m1, m2] = self.columns;
        uniforms.extend([Uniform::new("decode_m0", m0), Uniform::new("decode_m1", m1), Uniform::new("decode_m2", m2), Uniform::new("decode_k", self.k)]);
    }
}

/// The uniforms (see [`Decode`]) of a surface of `colour`; an error for primaries with no matrix to
/// BT.2020 (a white point other than D65).
pub(crate) fn decode_uniforms(colour: ColourDescription) -> Result<Decode, Error> {
    let [[a, b, c], [d, e, f], [g, h, i]] = super::math::matrix(colour.primaries, ColourPrimaries::Bt2020).ok_or("unsupported colour primary matrix")?;
    let luminances = colour.luminances();
    let reference = f64::from(luminances.reference.max(1));
    let scaling = if colour.transfer == TransferFunction::Pq { 1.0 } else { f64::from(luminances.max) / reference };
    let exponent = if let TransferFunction::Gamma(exponent) = colour.transfer { f64::from(exponent) / 10000.0 } else { 1.0 };
    let columns = [[a, d, g], [b, e, h], [c, f, i]].map(|column| column.map(|v| v as f32));
    Ok(Decode { columns, k: [scaling as f32, exponent as f32, (10000.0 / reference) as f32, 0.0] })
}

/// The shader drawing a surface of `colour` straight onto a screen of `output` (see [`Pipeline::compose`]).
fn direct_shader(renderer: &mut GlesRenderer, colour: ColourDescription, output: ColourDescription) -> Result<GlesTexProgram, Error> {
    final_shader(renderer, colour, |curved| super::shaders::direct(colour, output, curved))
}

/// The shader drawing a surface of `colour` into a scene in `scene`'s primaries.
fn decode_shader(renderer: &mut GlesRenderer, colour: ColourDescription, scene: ColourPrimaries) -> Result<GlesTexProgram, Error> {
    final_shader(renderer, colour, |curved| super::shaders::decode(colour, scene, curved))
}

/// A final pass's shader for a surface of `colour`, from its `source` given whether it takes a curve, and
/// then the curve's uniforms.
fn final_shader(renderer: &mut GlesRenderer, colour: ColourDescription, source: impl FnOnce(bool) -> Result<String, Error>) -> Result<GlesTexProgram, Error> {
    let curved = sdr(colour) || hdr(colour);
    let names = [
        UniformName::new("curve_a", UniformType::_4f),
        UniformName::new("curve_b", UniformType::_4f),
        UniformName::new("clarity", UniformType::_4f),
        UniformName::new("smaa_blend_tex", UniformType::_1i),
        UniformName::new("decode_m0", UniformType::_3f),
        UniformName::new("decode_m1", UniformType::_3f),
        UniformName::new("decode_m2", UniformType::_3f),
        UniformName::new("decode_k", UniformType::_4f),
    ];
    Ok(renderer.compile_custom_texture_shader(source(curved)?, if curved { &names } else { &[] })?)
}

/// How many final pass shaders are kept of each kind, the oldest dropped for a new one.
const SHADERS: usize = 32;

/// The shader for `key` in `cache`, made by `make` the first time.
fn cached<K: PartialEq>(cache: &mut Vec<(K, GlesTexProgram)>, key: K, make: impl FnOnce() -> Result<GlesTexProgram, Error>) -> Result<GlesTexProgram, Error> {
    if let Some((_, program)) = cache.iter().find(|(cached, _)| *cached == key) {
        return Ok(program.clone());
    }
    let program = make()?;
    if cache.len() == SHADERS {
        cache.remove(0);
    }
    cache.push((key, program.clone()));
    Ok(program)
}

/// The shader encoding a scene in `scene`'s primaries for a screen of `output`.
fn encode_shader(renderer: &mut GlesRenderer, scene: ColourPrimaries, output: ColourDescription) -> Result<GlesTexProgram, Error> {
    Ok(renderer.compile_custom_texture_shader(super::shaders::encode(scene, output)?, &[])?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    /// Descriptions differing in all a client chooses freely (luminances, primaries, a power law's exponent,
    /// target metadata) take one program per kind of transfer function: a client setting a new description
    /// at each commit compiles nothing.
    #[test]
    fn clients_descriptions_share_a_program_per_kind_of_transfer() {
        use clamshell_state::colour::{ColourLuminance, ColourTarget};
        let mut kinds = Vec::new();
        for i in 0..200u32 {
            for transfer in [TransferFunction::Srgb, TransferFunction::Gamma(18000 + i), TransferFunction::Linear, TransferFunction::Pq] {
                let primaries = if i % 2 == 0 {
                    ColourPrimaries::Custom([(640000 + i as i32, 330000), (300000, 600000), (150000, 60000), (312700, 329000)])
                } else {
                    ColourPrimaries::DisplayP3
                };
                let luminance = Some(ColourLuminance { min: i, max: 80 + i, reference: 80 + i / 2 });
                let target = Some(ColourTarget { max_cll: Some(100 + i), ..ColourTarget::default() });
                let kind = program_colour(ColourDescription { primaries, transfer, luminance, target });
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
        }
        assert_eq!(kinds.len(), 4);
    }

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
            // On opaque black, as a scene and a screen start.
            frame.clear([0.0, 0.0, 0.0, 1.0].into(), &[whole]).unwrap();
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

    /// A texture of `size` all white.
    fn white(renderer: &mut GlesRenderer, (w, h): (i32, i32)) -> GlesTexture {
        let picture = vec![255u8; usize::try_from(w * h * 4).unwrap()];
        renderer.import_memory(&picture, Fourcc::Abgr8888, (w, h).into(), false).unwrap()
    }

    /// Both scenes are drawn where they go side by side, the second's off the origin too (as the nested
    /// window and screenshots show them): each screen white, every pixel of the layout is.
    #[test]
    fn both_scenes_are_presented_side_by_side() {
        let mut renderer = renderer();
        let parts = [Rectangle::new((0, 0).into(), (8, 4).into()), Rectangle::new((8, 0).into(), (6, 4).into())];
        let mut pipeline = Pipeline::with_parts(&mut renderer, parts).unwrap();
        for (scene, part) in pipeline.scenes.iter_mut().zip(parts) {
            scene.texture = Some(white(&mut renderer, (part.size.w, part.size.h)));
        }
        let mut out: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (14, 4).into()).unwrap();
        {
            let mut target = renderer.bind(&mut out).unwrap();
            let [top, bottom] = &pipeline.scenes;
            drop(pipeline.present_with(&mut renderer, &mut target, (14, 4), [&top.output.1, &bottom.output.1]).unwrap());
        }
        let drawn = pixels(&mut renderer, &out);
        for (i, px) in drawn.as_chunks::<4>().0.iter().enumerate() {
            assert!(px[..3].iter().all(|c| *c >= 254), "pixel ({}, {}) is {px:?}, not white", i % 14, i / 14);
        }
    }

    /// A scene shown on another screen, fitted with bars, fills its place there whole and nothing else.
    #[test]
    fn a_fitted_encoding_fills_its_place() {
        let mut renderer = renderer();
        let parts = [Rectangle::new((0, 0).into(), (8, 4).into()), Rectangle::new((8, 0).into(), (6, 4).into())];
        let mut pipeline = Pipeline::with_parts(&mut renderer, parts).unwrap();
        pipeline.scenes[0].texture = Some(white(&mut renderer, (8, 4)));
        // As `drm::external::fit` places an 8x4 picture on a 10x8 screen.
        let dst = Rectangle::new((0, 1).into(), (10, 5).into());
        let encoding = pipeline.encoding(&mut renderer, false, ColourDescription::default(), dst).unwrap();
        let mut out: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (10, 8).into()).unwrap();
        {
            let mut target = renderer.bind(&mut out).unwrap();
            let sync = pipeline.present_screen(&mut renderer, &mut target, false, (10, 8).into(), Transform::Normal, Some(&encoding)).unwrap();
            sync.wait().unwrap();
        }
        let drawn = pixels(&mut renderer, &out);
        for (i, px) in drawn.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i % 10, i / 10);
            let inside = dst.contains((x as i32, y as i32));
            let want = if inside { 255 } else { 0 };
            assert!(px[..3].iter().all(|c| c.abs_diff(want) <= 1), "pixel ({x}, {y}) is {px:?}, inside {inside}");
        }
    }

    /// A surface drawn straight onto a screen, or through its scene, shows as the final pass would, as
    /// `scanout::Transform` works it out on the CPU, within a level: an sRGB surface on an unprofiled panel
    /// and on the Thor's top panel, and an HDR one (PQ) on the Thor's top panel. (A scene in BT.2020's
    /// primaries was a few levels off near zero: its half floats left a little of the channels the screen's
    /// primaries have none of, which the encoding's steep start made more.)
    #[test]
    fn a_surface_drawn_straight_is_the_final_pass() {
        let mut renderer = renderer();
        let picture: Vec<u8> = (0..256u32).flat_map(|i| [i as u8, (255 - i) as u8, (i * 7 % 256) as u8, 255]).collect();
        let texture = renderer.import_memory(&picture, Fourcc::Abgr8888, (256, 1).into(), false).unwrap();
        // Premultiplied: each alpha, a colour of each scaled by it.
        let picture: Vec<u8> = (0..256u32).flat_map(|a| [200 * a / 255, 90 * a / 255, (a * 7 % 256) * a / 255, a].map(|c| c as u8)).collect();
        let translucent = renderer.import_memory(&picture, Fourcc::Abgr8888, (256, 1).into(), false).unwrap();
        let picture: Vec<u8> = (0..256u32).flat_map(|i| [i as u8, (255 - i) as u8, (i * 7 % 256) as u8, 255]).collect();
        let srgb = ColourDescription::default();
        let pq = ColourDescription { primaries: ColourPrimaries::Bt2020, transfer: TransferFunction::Pq, ..ColourDescription::default() };
        let panel = ColourDescription::from(clamshell_device::colour::PanelColour::srgb());
        let thor = ColourDescription::from(clamshell_device::DeviceProfile::thor().top.colour);
        // What clients choose freely, which reaches the programs as uniforms (see `program_colour`): a power
        // law's exponent, the primaries, the luminances.
        let gamma = ColourDescription { primaries: ColourPrimaries::DisplayP3, transfer: TransferFunction::Gamma(26000), ..ColourDescription::default() };
        let luminance = clamshell_state::colour::ColourLuminance { min: 0, max: 120, reference: 100 };
        let brighter = ColourDescription { luminance: Some(luminance), ..ColourDescription::default() };
        let pq_bright = ColourDescription { luminance: Some(clamshell_state::colour::ColourLuminance { min: 50, max: 10000, reference: 300 }), ..pq };
        for (surface, output) in [(srgb, panel), (srgb, thor), (pq, thor), (gamma, thor), (brighter, panel), (pq_bright, thor)] {
            let curve = curve_uniforms(None, output.headroom() as f32, hdr_peak(surface, 0));
            let [a, b] = curve;
            let mut uniforms = vec![
                Uniform::new("curve_a", a),
                Uniform::new("curve_b", b),
                Uniform::new("clarity", [0.0f32; 4]),
                Uniform::new("smaa_blend_tex", smithay::backend::renderer::gles::UniformValue::_1i(1)),
            ];
            decode_uniforms(surface).unwrap().push(&mut uniforms);
            let transform = super::super::scanout::Transform::new(surface, curve, output).unwrap();
            // Straight onto the screen, and through its scene in half floats.
            let program = direct_shader(&mut renderer, program_colour(surface), output).unwrap();
            let straight = draw(&mut renderer, &texture, &program, &uniforms, Fourcc::Abgr8888);
            let decode = decode_shader(&mut renderer, program_colour(surface), output.primaries).unwrap();
            let encode = encode_shader(&mut renderer, output.primaries, output).unwrap();
            let scene = draw(&mut renderer, &texture, &decode, &uniforms, Fourcc::Abgr16161616f);
            let through = draw(&mut renderer, &scene, &encode, &[], Fourcc::Abgr8888);
            for (path, drawn) in [("straight", straight), ("through the scene", through)] {
                let drawn = pixels(&mut renderer, &drawn);
                for (i, px) in picture.as_chunks::<4>().0.iter().enumerate() {
                    let want = transform.apply([0, 1, 2].map(|c| f64::from(px[c]) / 255.0)).map(|c| c.clamp(0.0, 1.0) * 255.0);
                    for c in 0..3 {
                        let got = f64::from(drawn[i * 4 + c]);
                        assert!((got - want[c]).abs() <= 1.0, "{surface:?} on {output:?} {path}: pixel {i} channel {c} {got}, not {:.2}", want[c]);
                    }
                }
            }
            // Translucent, it is over black either way, composed in linear light: the same straight as
            // through the scene (which the opaque picture above pins to the CPU's).
            let straight = draw(&mut renderer, &translucent, &program, &uniforms, Fourcc::Abgr8888);
            let scene = draw(&mut renderer, &translucent, &decode, &uniforms, Fourcc::Abgr16161616f);
            let through = draw(&mut renderer, &scene, &encode, &[], Fourcc::Abgr8888);
            let (straight, through) = (pixels(&mut renderer, &straight), pixels(&mut renderer, &through));
            for (i, (a, b)) in straight.iter().zip(&through).enumerate() {
                assert!(a.abs_diff(*b) <= 1, "{surface:?} on {output:?} translucent: byte {i} straight {a}, through the scene {b}");
            }
        }
    }
}
