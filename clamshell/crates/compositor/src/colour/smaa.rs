//! SMAA 1x at a game's own resolution, before its picture is scaled (`docs/picture.md`): edges detected on
//! luma, blending weights from the edges with SMAA's area and search tables, and each pixel blended with its
//! neighbours by them. SMAA is Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro and Diego
//! Gutierrez's, under the MIT licence; `smaa/` holds its shader, its tables as raw bytes and its licence,
//! from iryoku/smaa at 71c806a. Its shaders need OpenGL ES 3, so its passes are GL programs of their own,
//! run in Smithay's context between its frames; they put back the bindings they change.

use clamshell_ui::Error;
use smithay::backend::{
    allocator::Fourcc,
    renderer::{
        ImportMem,
        gles::{GlesRenderer, GlesTexture, ffi},
    },
};

/// SMAA's shader, with its own GLSL macros (`SMAA_GLSL_3`).
const SHADER: &str = include_str!("smaa/SMAA.hlsl");
/// The area table: 160 by 560, two bytes a texel (red, green).
const AREA: &[u8] = include_bytes!("smaa/area.bin");
/// The search table: 64 by 16, a byte a texel.
const SEARCH: &[u8] = include_bytes!("smaa/search.bin");

/// What every pass's source starts with: ES 3, SMAA's GLSL, its high preset, the render target's metrics
/// as a uniform. `{stage}` leaves out the other stage's half of the shader.
const HEADER: &str = "#version 300 es
precision highp float;
precision highp int;
#define SMAA_GLSL_3
#define SMAA_PRESET_HIGH
#define SMAA_RT_METRICS rt_metrics
uniform vec4 rt_metrics;
";

/// A full-screen triangle from the vertex's index alone, its texture coordinates 0 to 1 over the target.
const TRIANGLE: &str = "vec2 corner() { return vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)); }
";

/// The three passes' vertex and fragment shaders' own parts, after the header and SMAA's shader.
const EDGES: (&str, &str) = (
    "out vec2 v_texcoord;
out vec4 v_offset[3];
void main() {
    v_texcoord = corner();
    vec4 offset[3];
    SMAAEdgeDetectionVS(v_texcoord, offset);
    v_offset = offset;
    gl_Position = vec4(v_texcoord * 2.0 - 1.0, 0.0, 1.0);
}",
    "uniform sampler2D color_tex;
in vec2 v_texcoord;
in vec4 v_offset[3];
out vec4 frag;
void main() {
    frag = vec4(SMAALumaEdgeDetectionPS(v_texcoord, v_offset, color_tex), 0.0, 0.0);
}",
);
const WEIGHTS: (&str, &str) = (
    "out vec2 v_texcoord;
out vec2 v_pixcoord;
out vec4 v_offset[3];
void main() {
    v_texcoord = corner();
    vec2 pixcoord;
    vec4 offset[3];
    SMAABlendingWeightCalculationVS(v_texcoord, pixcoord, offset);
    v_pixcoord = pixcoord;
    v_offset = offset;
    gl_Position = vec4(v_texcoord * 2.0 - 1.0, 0.0, 1.0);
}",
    "uniform sampler2D edges_tex;
uniform sampler2D area_tex;
uniform sampler2D search_tex;
in vec2 v_texcoord;
in vec2 v_pixcoord;
in vec4 v_offset[3];
out vec4 frag;
void main() {
    frag = SMAABlendingWeightCalculationPS(v_texcoord, v_pixcoord, v_offset, edges_tex, area_tex, search_tex, vec4(0.0));
}",
);
const BLENDING: (&str, &str) = (
    "out vec2 v_texcoord;
out vec4 v_offset;
void main() {
    v_texcoord = corner();
    vec4 offset;
    SMAANeighborhoodBlendingVS(v_texcoord, offset);
    v_offset = offset;
    gl_Position = vec4(v_texcoord * 2.0 - 1.0, 0.0, 1.0);
}",
    "uniform sampler2D color_tex;
uniform sampler2D blend_tex;
in vec2 v_texcoord;
in vec4 v_offset;
out vec4 frag;
void main() {
    frag = SMAANeighborhoodBlendingPS(v_texcoord, v_offset, color_tex, blend_tex);
}",
);

/// A pass's program, its metrics uniform, and its samplers by name with their filtering (nearest or not).
struct Pass {
    program: u32,
    metrics: i32,
    samplers: Vec<(i32, bool)>,
}

pub(crate) struct Smaa {
    edges: Pass,
    weights: Pass,
    blending: Pass,
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
            let edges = build(gl, EDGES, &[("color_tex", false)])?;
            let weights = build(gl, WEIGHTS, &[("edges_tex", false), ("area_tex", false), ("search_tex", true)])?;
            let blending = build(gl, BLENDING, &[("color_tex", false), ("blend_tex", false)])?;
            Ok((edges, weights, blending, framebuffer(gl)))
        })?;
        let (edges, weights, blending, framebuffer) = built.map_err(|e| format!("smaa: {e}"))?;
        Ok(Smaa { edges, weights, blending, area, search, framebuffer })
    }

    /// Anti-aliases `color` into `out` through `edges` and `blend`, all of `size`.
    pub fn run(
        &self,
        renderer: &mut GlesRenderer,
        color: &GlesTexture,
        edges: &GlesTexture,
        blend: &GlesTexture,
        out: &GlesTexture,
        size: (i32, i32),
    ) -> Result<(), Error> {
        let fb = self.framebuffer;
        renderer.with_context(|gl| {
            draw(gl, fb, &self.edges, edges.tex_id(), size, &[color.tex_id()]);
            draw(gl, fb, &self.weights, blend.tex_id(), size, &[edges.tex_id(), self.area.tex_id(), self.search.tex_id()]);
            draw(gl, fb, &self.blending, out.tex_id(), size, &[color.tex_id(), blend.tex_id()]);
        })?;
        Ok(())
    }
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
        for (unit, (&texture, &(sampler, nearest))) in inputs.iter().zip(&pass.samplers).enumerate() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, texture);
            let filter = if nearest { ffi::NEAREST } else { ffi::LINEAR } as i32;
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, filter);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, filter);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
            gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
            gl.Uniform1i(sampler, unit as i32);
        }
        gl.DrawArrays(ffi::TRIANGLES, 0, 3);
        // Back as Smithay's drawing finds them: its frames set the rest (viewport, blending) themselves.
        for unit in (0..inputs.len()).rev() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
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

/// Compiles and links a pass from its vertex and fragment parts, finding its uniforms.
#[allow(unsafe_code)]
fn build(gl: &ffi::Gles2, (vertex, fragment): (&str, &str), samplers: &[(&str, bool)]) -> Result<Pass, String> {
    let vs = compile(gl, ffi::VERTEX_SHADER, &format!("{HEADER}#define SMAA_INCLUDE_PS 0\n{SHADER}\n{TRIANGLE}{vertex}"))?;
    let fs = compile(gl, ffi::FRAGMENT_SHADER, &format!("{HEADER}#define SMAA_INCLUDE_VS 0\n{SHADER}\n{fragment}"))?;
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
        let samplers = samplers.iter().map(|(name, nearest)| (location(name), *nearest)).collect();
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
