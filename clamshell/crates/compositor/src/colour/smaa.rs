//! SMAA 1x at a game's own resolution, before its picture is scaled (`docs/picture.md`): edges detected on
//! luma, blending weights from the edges with SMAA's area and search tables, and each pixel blended with its
//! neighbours by them. SMAA is Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro and Diego
//! Gutierrez's, under the MIT licence; `smaa/` holds its shader, its tables as raw bytes and its licence,
//! from iryoku/smaa at 71c806a. Its shaders need OpenGL ES 3, so its passes are GL programs of their own,
//! run in Smithay's context between its frames; they put back the bindings they change.
//!
//! The passes read the game's buffer as it is, so nothing is copied: SMAA's language is given by our own
//! macros (its `SMAA_CUSTOM_SL`, in `shaders`), and the picture (`colorTex`) is declared with the external sampler of a
//! dmabuf the GPU can't render to, in a second variant of the passes that read it (by its declarations in
//! SMAA's text: GLSL ES has no token pasting to do it by name).

use clamshell_ui::Error;
use smithay::backend::{
    allocator::Fourcc,
    renderer::{
        ImportMem,
        gles::{GlesRenderer, GlesTexture, ffi},
    },
};

/// The area table: 160 by 560, two bytes a texel (red, green).
const AREA: &[u8] = include_bytes!("smaa/area.bin");
/// The search table: 64 by 16, a byte a texel.
const SEARCH: &[u8] = include_bytes!("smaa/search.bin");

/// A pass's program, its metrics uniform, and its samplers with their filtering (nearest or not) and
/// their texture's target.
struct Pass {
    program: u32,
    metrics: i32,
    samplers: Vec<(i32, bool, u32)>,
}

pub(crate) struct Smaa {
    edges: Pass,
    weights: Pass,
    blending: Pass,
    /// The passes that read the picture, for an external one: made the first time one comes.
    external: Option<(Pass, Pass)>,
    /// The edges' pass for linear light (scRGB's), measuring contrast in PQ's signal, for an ordinary
    /// picture and an external one: made the first time one comes.
    linear_edges: (Option<Pass>, Option<Pass>),
    area: GlesTexture,
    search: GlesTexture,
    framebuffer: u32,
}

impl Smaa {
    pub fn new(renderer: &mut GlesRenderer) -> Result<Smaa, Error> {
        // The tables as RGBA: the area's red and green, the search's in red.
        let area: Vec<u8> = AREA.as_chunks::<2>().0.iter().flat_map(|&[r, g]| [r, g, 0, 255]).collect();
        let search: Vec<u8> = SEARCH.iter().flat_map(|&r| [r, 0, 0, 255]).collect();
        let area = renderer.import_memory(&area, Fourcc::Abgr8888, (160, 560).into(), false)?;
        let search = renderer.import_memory(&search, Fourcc::Abgr8888, (64, 16).into(), false)?;
        let built = renderer.with_context(|gl| -> Result<_, String> {
            let (edges, blending) = picture_passes(gl, false)?;
            let weights = build(
                gl,
                super::shaders::SMAA_WEIGHTS,
                false,
                false,
                &[("edges_tex", false, ffi::TEXTURE_2D), ("area_tex", false, ffi::TEXTURE_2D), ("search_tex", true, ffi::TEXTURE_2D)],
            )?;
            Ok((edges, weights, blending, framebuffer(gl)))
        })?;
        let (edges, weights, blending, framebuffer) = built.map_err(|e| format!("smaa: {e}"))?;
        Ok(Smaa { edges, weights, blending, external: None, linear_edges: (None, None), area, search, framebuffer })
    }

    /// Anti-aliases `color` (an external texture if `external`) through `edges` and `blend`, all of `size`,
    /// in `color`'s orientation: into `out`, or with none, up to the blending weights, the final pass
    /// blending (`SMAA_BLEND_GLSL`).
    pub fn run(
        &mut self,
        renderer: &mut GlesRenderer,
        (color, external, linear): (&GlesTexture, bool, bool),
        [edges, blend]: [&GlesTexture; 2],
        out: Option<&GlesTexture>,
        size: (i32, i32),
    ) -> Result<(), Error> {
        if external && self.external.is_none() {
            let built = renderer.with_context(|gl| picture_passes(gl, true))?;
            self.external = Some(built.map_err(|e| format!("smaa: {e}"))?);
        }
        let linear_edges = super::chain::side(&mut self.linear_edges, external);
        if linear && linear_edges.is_none() {
            let built = renderer.with_context(|gl| build(gl, super::shaders::SMAA_EDGES, external, true, &[("color_tex", false, picture_target(external))]))?;
            *linear_edges = Some(built.map_err(|e| format!("smaa: {e}"))?);
        }
        let linear_edges = if external { &self.linear_edges.1 } else { &self.linear_edges.0 };
        let (edges_pass, blending_pass) = match (&self.external, external) {
            (Some((edges, blending)), true) => (edges, blending),
            _ => (&self.edges, &self.blending),
        };
        let edges_pass = match linear_edges {
            Some(pass) if linear => pass,
            _ => edges_pass,
        };
        let fb = self.framebuffer;
        renderer.with_context(|gl| {
            draw(gl, fb, edges_pass, edges.tex_id(), size, &[color.tex_id()]);
            draw(gl, fb, &self.weights, blend.tex_id(), size, &[edges.tex_id(), self.area.tex_id(), self.search.tex_id()]);
            if let Some(out) = out {
                draw(gl, fb, blending_pass, out.tex_id(), size, &[color.tex_id(), blend.tex_id()]);
            }
        })?;
        Ok(())
    }
}

/// The two passes that read the picture, edges and blending, for an ordinary texture or an external one.
fn picture_passes(gl: &ffi::Gles2, external: bool) -> Result<(Pass, Pass), String> {
    let color = picture_target(external);
    let edges = build(gl, super::shaders::SMAA_EDGES, external, false, &[("color_tex", false, color)])?;
    let blending = build(gl, super::shaders::SMAA_BLENDING, external, false, &[("color_tex", false, color), ("blend_tex", false, ffi::TEXTURE_2D)])?;
    Ok((edges, blending))
}

/// Draws `pass` into `target` (a texture of `size`), reading `inputs` on its samplers in order.
#[allow(unsafe_code)]
fn draw(gl: &ffi::Gles2, framebuffer: u32, pass: &Pass, target: u32, (w, h): (i32, i32), inputs: &[u32]) {
    // SAFETY: plain GL calls on Smithay's current context, with the names created here or its textures'.
    unsafe {
        gl.BindFramebuffer(ffi::FRAMEBUFFER, framebuffer);
        gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, target, 0);
        gl.Viewport(0, 0, w, h);
        gl.Disable(ffi::BLEND);
        gl.Disable(ffi::SCISSOR_TEST);
        gl.ClearColor(0.0, 0.0, 0.0, 0.0);
        gl.Clear(ffi::COLOR_BUFFER_BIT);
        gl.UseProgram(pass.program);
        gl.Uniform4f(pass.metrics, 1.0 / w as f32, 1.0 / h as f32, w as f32, h as f32);
        for (unit, (&texture, &(sampler, nearest, target))) in inputs.iter().zip(&pass.samplers).enumerate() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(target, texture);
            let filter = if nearest { ffi::NEAREST } else { ffi::LINEAR } as i32;
            gl.TexParameteri(target, ffi::TEXTURE_MIN_FILTER, filter);
            gl.TexParameteri(target, ffi::TEXTURE_MAG_FILTER, filter);
            gl.TexParameteri(target, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
            gl.TexParameteri(target, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
            gl.Uniform1i(sampler, unit as i32);
        }
        gl.DrawArrays(ffi::TRIANGLES, 0, 3);
        // Back as Smithay's drawing finds them: its frames set the rest (viewport, blending) themselves.
        for (unit, &(_, _, target)) in pass.samplers.iter().enumerate().take(inputs.len()).rev() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(target, 0);
        }
        gl.UseProgram(0);
        gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, 0, 0);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
        gl.Enable(ffi::BLEND);
    }
}

/// A framebuffer for the passes to draw through.
#[allow(unsafe_code)]
fn framebuffer(gl: &ffi::Gles2) -> u32 {
    let mut fb = 0;
    // SAFETY: GL on Smithay's current context.
    unsafe { gl.GenFramebuffers(1, &mut fb) };
    fb
}

/// The picture's texture target: external, or ordinary.
fn picture_target(external: bool) -> u32 {
    if external { ffi::TEXTURE_EXTERNAL_OES } else { ffi::TEXTURE_2D }
}

/// Compiles and links a pass from its vertex and fragment parts, for an external picture or not, finding
/// its uniforms: its samplers by name, with their filtering and target. With `linear` the picture's
/// samples, linear light, are taken to PQ's signal (only the edges' pass reads them, through
/// `SMAASamplePoint`).
#[allow(unsafe_code)]
fn build(gl: &ffi::Gles2, pass: (&str, &str), external: bool, linear: bool, samplers: &[(&str, bool, u32)]) -> Result<Pass, String> {
    let (vertex, fragment) = super::shaders::smaa(pass, external, linear);
    let vs = compile(gl, ffi::VERTEX_SHADER, &vertex)?;
    let fs = compile(gl, ffi::FRAGMENT_SHADER, &fragment)?;
    // SAFETY: GL on Smithay's current context, with the shaders compiled above.
    unsafe {
        let program = gl.CreateProgram();
        gl.AttachShader(program, vs);
        gl.AttachShader(program, fs);
        gl.LinkProgram(program);
        gl.DeleteShader(vs);
        gl.DeleteShader(fs);
        let mut linked = 0;
        gl.GetProgramiv(program, ffi::LINK_STATUS, &mut linked);
        if linked == 0 {
            let log = info_log(gl, program, true);
            gl.DeleteProgram(program);
            return Err(format!("link: {log}"));
        }
        let location = |name: &str| {
            let name = std::ffi::CString::new(name).unwrap_or_default();
            gl.GetUniformLocation(program, name.as_ptr())
        };
        let metrics = location("rt_metrics");
        let samplers = samplers.iter().map(|(name, nearest, target)| (location(name), *nearest, *target)).collect();
        Ok(Pass { program, metrics, samplers })
    }
}

#[allow(unsafe_code)]
fn compile(gl: &ffi::Gles2, kind: u32, source: &str) -> Result<u32, String> {
    let source = std::ffi::CString::new(source).map_err(|e| e.to_string())?;
    // SAFETY: GL on Smithay's current context; the source outlives the call.
    unsafe {
        let shader = gl.CreateShader(kind);
        gl.ShaderSource(shader, 1, &source.as_ptr(), std::ptr::null());
        gl.CompileShader(shader);
        let mut compiled = 0;
        gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &mut compiled);
        if compiled == 0 {
            let log = info_log(gl, shader, false);
            gl.DeleteShader(shader);
            let stage = if kind == ffi::VERTEX_SHADER { "vertex" } else { "fragment" };
            return Err(format!("{stage} shader: {log}"));
        }
        Ok(shader)
    }
}

/// A shader's or program's info log.
#[allow(unsafe_code)]
fn info_log(gl: &ffi::Gles2, name: u32, program: bool) -> String {
    let mut buffer = vec![0u8; 4096];
    let mut len = 0;
    // SAFETY: GL on Smithay's current context, into a buffer of the size given.
    unsafe {
        if program {
            gl.GetProgramInfoLog(name, buffer.len() as i32, &mut len, buffer.as_mut_ptr().cast());
        } else {
            gl.GetShaderInfoLog(name, buffer.len() as i32, &mut len, buffer.as_mut_ptr().cast());
        }
    }
    buffer.truncate(len.max(0) as usize);
    String::from_utf8_lossy(&buffer).into_owned()
}
