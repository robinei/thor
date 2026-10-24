//! Surfaces decode into a retained half-float scene before the host encoding is applied once.
use crate::{render::Element, server::Server};
use clamshell_state::curve::{Curve, REFERENCE_NITS};
use clamshell_state::session::presentation::{ColourDescription, ColourPrimaries, TransferFunction};
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
    output: ColourDescription,
    output_shader: GlesTexProgram,
    revision: u64,
    first: bool,
    /// Each window's surfaces with the HDR curve of the game running it, this frame and the last: a
    /// curve that changes redraws the scene, since the surfaces themselves didn't.
    curves: Vec<(Id, Curve)>,
    last_curves: Vec<(Id, Curve)>,
}
struct Converted {
    inner: Element,
    shader: GlesTexProgram,
    /// For an SDR surface's shader: its curve's uniforms (see [`curve_uniforms`]), off without one.
    curve: Option<[[f32; 4]; 2]>,
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
        let uniforms = self.curve.map_or_else(Vec::new, |[a, b]| vec![Uniform::new("curve_a", a), Uniform::new("curve_b", b)]);
        frame.override_default_tex_program(self.shader.clone(), uniforms);
        let result = self.inner.draw(frame, src, dst, damage, opaque, cache);
        frame.clear_tex_program_override();
        result
    }
}
impl Pipeline {
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
            output,
            output_shader,
            revision: 0,
            first: true,
            curves: Vec::new(),
            last_curves: Vec::new(),
        })
    }
    pub(crate) fn set_output(&mut self, renderer: &mut GlesRenderer, output: ColourDescription) -> Result<(), Error> {
        if self.output != output {
            self.output_shader = shader(renderer, output, false)?;
            self.output = output;
        }
        Ok(())
    }
    pub(crate) fn compose(&mut self, server: &Server, renderer: &mut GlesRenderer, elements: &mut Vec<Element>) -> Result<RenderElementStates, Error> {
        if self.size != server.size {
            let output = self.output;
            *self = Self::new(renderer, server.size)?;
            self.set_output(renderer, output)?;
        }
        if self.revision != server.colour.revision {
            self.damage = OutputDamageTracker::new(self.size, 1.0, Transform::Normal);
            self.first = true;
            self.revision = server.colour.revision;
        }
        self.curves.clear();
        for client in server.windows.iter().filter(|client| client.mapped && server.visible(client.id)) {
            if let Some(curve) = server.core.state().window_curve(client.id) {
                client.window.with_surfaces(|surface, _| self.curves.push((Id::from_wayland_resource(surface), curve)));
            }
        }
        if self.curves != self.last_curves {
            self.damage = OutputDamageTracker::new(self.size, 1.0, Transform::Normal);
            self.first = true;
            std::mem::swap(&mut self.curves, &mut self.last_curves);
        }
        self.converted.clear();
        for inner in elements.drain(..) {
            let colour = server.colour.surfaces.get(inner.id()).map(|colour| **colour).unwrap_or_default();
            let curve = sdr(colour).then(|| curve_uniforms(self.last_curves.iter().find(|(id, _)| id == inner.id()).map(|(_, c)| c)));
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
            self.converted.push(Converted { inner, shader: program, curve });
        }
        let mut target = renderer.bind(&mut self.texture)?;
        let states = self.damage.render_output(renderer, &mut target, usize::from(!self.first), &self.converted, [0.0, 0.0, 0.0, 1.0])?.states;
        self.first = false;
        Ok(states)
    }
    pub(crate) fn present(&self, renderer: &mut GlesRenderer, target: &mut GlesTarget<'_>) -> Result<(), Error> {
        self.present_with(renderer, target, &self.output_shader)
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
    pub fn capture(&self, renderer: &mut GlesRenderer) -> Result<crate::Capture, String> {
        let program = shader(renderer, ColourDescription::default(), false).map_err(|error| error.to_string())?;
        let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, self.size.into()).map_err(|error| error.to_string())?;
        let mut target = renderer.bind(&mut texture).map_err(|error| error.to_string())?;
        self.present_with(renderer, &mut target, &program).map_err(|error| error.to_string())?;
        crate::render::capture(renderer, &target, self.size)
    }
}
/// Whether a surface of `description` is SDR, which an HDR curve applies to: an sRGB or power-law
/// signal. PQ, HLG and linear light (scRGB's) are left as they are.
fn sdr(description: ColourDescription) -> bool {
    matches!(description.transfer, TransferFunction::Srgb | TransferFunction::Gamma(_))
}

/// An HDR curve as the SDR input shader's uniforms: black point, contrast, where highlights start and their
/// peak (in units of white); then white over the reference, saturation, and whether a curve is on. Without
/// one, the shader leaves the surface as it is.
fn curve_uniforms(curve: Option<&Curve>) -> [[f32; 4]; 2] {
    let Some(c) = curve else {
        return [[0.0, 1.0, 1.0, 1.0], [1.0, 1.0, 0.0, 0.0]];
    };
    let white = f32::from(c.white.max(1));
    [
        [f32::from(c.black) / 1000.0, f32::from(c.contrast) / 100.0, f32::from(c.start) / 100.0, f32::from(c.peak.max(c.white)) / white],
        [white / REFERENCE_NITS as f32, f32::from(c.saturation) / 100.0, 0.0, 1.0],
    ]
}

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
    // An SDR surface's HDR curve, as clamshell_state::curve::Curve::map does it: on the signal, the black
    // point and contrast; on the light, highlights expanded on luminance and saturation around it.
    let curved = input && sdr(description);
    let conversion = if curved {
        format!(
            "bool on = curve_b.w > 0.5;
    if (on) {{ v = pow(clamp((v - curve_a.x) / max(1.0 - curve_a.x, 0.000001), 0.0, 1.0), vec3(curve_a.y)); }}
    v = convert(v) * {scaling:.12};
    if (on) {{
        float y = dot(v, vec3(0.2126, 0.7152, 0.0722));
        float s = curve_a.z;
        float t = (y - s) / max(1.0 - s, 0.000001);
        float e = (y > s && s < 1.0) ? s + (1.0 - s) * t + (curve_a.w - 1.0) * t * t : y;
        vec3 c = y > 0.0 ? v * (e / y) : vec3(0.0);
        v = max(vec3(e) + (c - vec3(e)) * curve_b.y, vec3(0.0)) * curve_b.x;
    }}
    v = matrix * v;"
        )
    } else if input {
        format!("v = convert(v) * {scaling:.12}; v = matrix * v;")
    } else {
        format!("v = matrix * v; v /= {scaling:.12}; v = convert(v);")
    };
    let uniforms = if curved { "uniform vec4 curve_a;\nuniform vec4 curve_b;" } else { "" };
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
void main() {{
    vec4 sample = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    sample.a = 1.0;
#endif
    vec3 v = sample.a > 0.0 ? sample.rgb / sample.a : vec3(0.0);
    mat3 matrix = mat3({a:.12},{d:.12},{g:.12},{b:.12},{e:.12},{h:.12},{c:.12},{f:.12},{i:.12});
    {conversion}
    gl_FragColor = vec4(v * sample.a, sample.a) * alpha;
}}"#
    );
    let names = [UniformName::new("curve_a", UniformType::_4f), UniformName::new("curve_b", UniformType::_4f)];
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
            let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
            let (s, t) = (a[2], (y - a[2]) / (1.0 - a[2]).max(0.000001));
            let e = if y > s && s < 1.0 { s + (1.0 - s) * t + (a[3] - 1.0) * t * t } else { y };
            let c = if y > 0.0 { v.map(|x| x * e / y) } else { [0.0; 3] };
            v = c.map(|x| (e + (x - e) * b[1]).max(0.0) * b[0]);
        }
        v
    }

    #[test]
    fn the_shaders_uniforms_give_the_states_curve() {
        let curves = [
            Curve::default(),
            Curve { black: 20, contrast: 110, white: 203, start: 60, peak: 650, saturation: 105 },
            Curve { black: 150, contrast: 70, white: 400, start: 30, peak: 1000, saturation: 50 },
        ];
        for curve in curves {
            for rgb in [[0.0, 0.0, 0.0], [0.03, 0.03, 0.03], [0.5, 0.4, 0.3], [1.0, 1.0, 1.0], [1.0, 0.2, 0.0], [0.9, 0.95, 1.0]] {
                let (want, got) = (curve.map(rgb), shaded(rgb, curve_uniforms(Some(&curve))));
                for (w, g) in want.iter().zip(got) {
                    assert!((w - g).abs() < 1e-5, "{curve:?} {rgb:?}: {want:?} != {got:?}");
                }
            }
        }
        // Without a curve the surface is only decoded.
        assert_eq!(shaded([0.5, 0.5, 0.5], curve_uniforms(None))[0], shaded([0.5, 0.5, 0.5], curve_uniforms(Some(&Curve::default())))[0]);
    }
}
