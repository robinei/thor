//! The colour pipeline's GLSL: the final pass that decodes each surface into the scene and encodes the
//! scene for a screen ([`surface`]), the passes a picture takes before it ([`pass`], SMAA's [`smaa`]), and
//! the histogram's measure.

use clamshell_state::colour::{ColourDescription, ColourPrimaries, TransferFunction};
use clamshell_ui::Error;

/// The source of the final pass's shader for a surface of `description` (`input`), or of the encoding to a
/// screen of it: decoded, converted to the working space with its curve when `curved`, and encoded.
pub(super) fn surface(description: ColourDescription, input: bool, curved: bool) -> Result<String, Error> {
    if input { source(Some(description), None, curved) } else { source(None, Some(description), false) }
}

/// The source of the final pass's shader for a surface of `description` drawn straight onto a screen of
/// `output`: decoded, converted with its curve when `curved`, and encoded for the screen in the same pass,
/// with no scene between (a screen showing only it, see `render::Pipeline::compose`).
pub(super) fn direct(description: ColourDescription, output: ColourDescription, curved: bool) -> Result<String, Error> {
    source(Some(description), Some(output), curved)
}

/// A shader decoding a surface of `input` into the working space, encoding the working space for a screen
/// of `output`, or both in turn.
fn source(input: Option<ColourDescription>, output: Option<ColourDescription>, curved: bool) -> Result<String, Error> {
    let (decoding, decoded) = input.map(decoding).transpose()?.unwrap_or_default();
    let (encoding, encoded) = output.map(encoding).transpose()?.unwrap_or_default();
    let description = input.or(output).ok_or("a shader with neither side")?;
    let uniforms = if curved { "uniform vec4 curve_a;\nuniform vec4 curve_b;\nuniform vec4 clarity;\nuniform sampler2D smaa_blend_tex;" } else { "" };
    let fetch = if curved {
        let linear = format!("const bool LINEAR = {};\n", description.transfer == TransferFunction::Linear);
        [linear.as_str(), PERCEIVE, EASU, FXAA, SMAA_BLEND, RCAS, FETCH].concat()
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
{decoding}
{encoding}
{fetch}
void main() {{
    vec4 sample = fetch();
#if defined(NO_ALPHA)
    sample.a = 1.0;
#endif
    vec3 v = sample.a > 0.0 ? sample.rgb / sample.a : vec3(0.0);
    {decoded}
    {encoded}
    gl_FragColor = vec4(v * sample.a, sample.a) * alpha;
}}"#
    );
    Ok(source)
}

/// A transfer function, as GLSL on `v`: `description`'s decoding (`input`) or its encoding.
fn transfer(description: ColourDescription, input: bool) -> Result<String, Error> {
    let reference = f64::from(description.luminances().reference);
    Ok(match (input, description.transfer) {
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
    })
}

/// A matrix as a GLSL `mat3` constructor.
fn mat3([[a, b, c], [d, e, f], [g, h, i]]: [[f64; 3]; 3]) -> String {
    format!("mat3({a:.12},{d:.12},{g:.12},{b:.12},{e:.12},{h:.12},{c:.12},{f:.12},{i:.12})")
}

/// The decoding of a surface of `description` into the working space, with its curve: its GLSL functions
/// (`convert`, `matrix`) and the statements on `v`.
fn decoding(description: ColourDescription) -> Result<(String, String), Error> {
    let matrix = super::math::matrix(description.primaries, ColourPrimaries::Bt2020).ok_or("unsupported colour primary matrix")?;
    let expression = transfer(description, true)?;
    let reference = f64::from(description.luminances().reference);
    let scaling = if description.transfer == TransferFunction::Pq { 1.0 } else { f64::from(description.luminances().max) / reference };
    // An SDR surface's HDR curve, as clamshell_state::curve::Curve::map_on does it: on the signal, the
    // black point and contrast; on the light, highlights expanded on luminance within the screen's peak
    // (curve_b.z) and saturation around it.
    let conversion = if super::render::sdr(description) {
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
    } else if super::render::hdr(description) {
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
    } else {
        format!("v = convert(v) * {scaling:.12}; v = matrix * v;")
    };
    Ok((format!("vec3 convert(vec3 v) {{ return {expression}; }}\nconst mat3 matrix = {};", mat3(matrix)), conversion))
}

/// The encoding of the working space for a screen of `description`: its GLSL functions (`encode`,
/// `to_screen`) and the statements on `v`.
fn encoding(description: ColourDescription) -> Result<(String, String), Error> {
    let matrix = super::math::matrix(ColourPrimaries::Bt2020, description.primaries).ok_or("unsupported colour primary matrix")?;
    super::math::encode(1.0, description).ok_or("unsupported output transfer")?;
    let expression = transfer(description, false)?;
    let reference = f64::from(description.luminances().reference);
    let scaling = if description.transfer == TransferFunction::Pq { 1.0 } else { f64::from(description.luminances().max) / reference };
    let conversion = if description.transfer == TransferFunction::Pq {
        format!("v = to_screen * v; v /= {scaling:.12}; v = encode(v);")
    } else {
        // Above the screen's peak, each pixel is brought down whole, keeping its hue.
        format!("v = to_screen * v; v /= {scaling:.12}; float m = max(max(v.r, v.g), v.b); if (m > 1.0) {{ v /= m; }} v = encode(v);")
    };
    Ok((format!("vec3 encode(vec3 v) {{ return {expression}; }}\nconst mat3 to_screen = {};", mat3(matrix)), conversion))
}

/// FSR 1's upscale (EASU) of a picture, with the GLSL it needs, on the texture `tex` whose texel size is
/// `clarity.xy` (see `render::clarity_uniform`): the twelve texels around each pixel, weighed along the edge their
/// luma shows and kept within the four nearest (as `easu` in the tests). It works on the signal, before
/// decoding.
pub(super) const EASU: &str = "vec4 texel(vec2 at) { return texture2D(tex, (at + 0.5) * clarity.xy); }
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
const SMAA_BLEND: &str = "vec4 smaa_blend(vec2 uv) {
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
pub(super) const FXAA: &str = "vec4 fxaa(vec2 uv) {
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
pub(super) const PERCEIVE: &str = "vec3 perceive(vec3 c) {
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
const RCAS: &str = "vec4 rcas() {
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

/// A window's picture as the final pass samples it, before decoding (see `render::clarity_uniform`): RCAS on
/// EASU's texture, EASU itself, or nearest (snapped to the centre of a texel) or bilinear, sharpened then by
/// AMD's CAS on the four texels around, with the weight it adapts to their contrast, kept within the
/// signal's range, or for linear light within the brightest of them (as `cas` in the tests).
const FETCH: &str = "vec4 fetch() {
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

/// The source of a pass before the scene that draws `call`, a function of `glsl`, for a picture of linear
/// light or not (see [`PERCEIVE`]).
pub(super) fn pass(call: &str, glsl: &str, linear: bool) -> String {
    let perceive = PERCEIVE;
    format!(
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
const bool LINEAR = {linear};
{perceive}
{glsl}
void main() {{
    gl_FragColor = {call} * alpha;
}}"#
    )
}

/// Each pixel's luminance as a level, in the red channel, by the kind in `clarity.x`: SDR's signal
/// (sRGB's encoding, BT.709's luminance); PQ's, of its light (BT.2020's luminance) scaled by `clarity.y`
/// so that its white is at the reference; or for linear light (scRGB's, BT.709's), PQ's of it with 1.0
/// at the reference.
pub(super) const MEASURE: &str = "float srgb_decode(float e) { return e <= 0.04045 ? e / 12.92 : pow((e + 0.055) / 1.055, 2.4); }
float srgb_encode(float l) { return l <= 0.0031308 ? l * 12.92 : 1.055 * pow(l, 1.0 / 2.4) - 0.055; }
float pq_light(float e) {
    float p = pow(clamp(e, 0.0, 1.0), 32.0 / 2523.0);
    return pow(max(p - 0.8359375, 0.0) / (18.8515625 - 18.6875 * p), 16384.0 / 2610.0) * 10000.0;
}
float pq(float nits) {
    float p = pow(max(nits, 0.0) / 10000.0, 2610.0 / 16384.0);
    return pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), 2523.0 / 32.0);
}
vec4 measure() {
    vec3 c = texture2D(tex, v_coords).rgb;
    float level;
    if (clarity.x < 0.5) {
        float y = dot(vec3(srgb_decode(c.r), srgb_decode(c.g), srgb_decode(c.b)), vec3(0.2126, 0.7152, 0.0722));
        level = srgb_encode(clamp(y, 0.0, 1.0));
    } else if (clarity.x < 1.5) {
        float y = dot(vec3(pq_light(c.r), pq_light(c.g), pq_light(c.b)), vec3(0.2627, 0.6780, 0.0593));
        level = pq(y * clarity.y);
    } else {
        level = pq(dot(max(c, vec3(0.0)), vec3(0.2126, 0.7152, 0.0722)) * 203.0);
    }
    return vec4(level, 0.0, 0.0, 1.0);
}
";

/// SMAA's shader, with its own GLSL macros (`SMAA_GLSL_3`).
pub(super) const SMAA_SHADER: &str = include_str!("smaa/SMAA.hlsl");
/// What every pass's source starts with after its version (and for an external picture, the extension
/// for it): SMAA's high preset, the render target's metrics as a uniform, and SMAA's language as GLSL ES 3
/// by our macros. Its textures are `sampler2D`s, but the picture's, `SAMPLER_colorTex`.
const SMAA_HEADER: &str = "precision highp float;
precision highp int;
#define SMAA_PRESET_HIGH
#define SMAA_RT_METRICS rt_metrics
uniform vec4 rt_metrics;
#define SMAA_CUSTOM_SL
#define SMAATexture2D(tex) sampler2D tex
#define SMAATexturePass2D(tex) tex
#define SMAA_FLATTEN
#define SMAA_BRANCH
#define lerp(a, b, t) mix(a, b, t)
#define saturate(a) clamp(a, 0.0, 1.0)
#define mad(a, b, c) (a * b + c)
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define int2 ivec2
#define int3 ivec3
#define int4 ivec4
#define bool2 bvec2
#define bool3 bvec3
#define bool4 bvec4
";

/// SMAA's sampling as SMAA_GLSL_3 does it, for passes whose textures are all ordinary.
const SMAA_SAMPLING: &str = "#define SAMPLER_colorTex sampler2D
#define SMAASampleLevelZero(tex, coord) textureLod(tex, coord, 0.0)
#define SMAASampleLevelZeroPoint(tex, coord) textureLod(tex, coord, 0.0)
#define SMAASampleLevelZeroOffset(tex, coord, offset) textureLodOffset(tex, coord, 0.0, offset)
#define SMAASample(tex, coord) texture(tex, coord)
#define SMAASamplePoint(tex, coord) texture(tex, coord)
#define SMAASampleOffset(tex, coord, offset) texture(tex, coord, offset)
";

/// SMAA's sampling with the picture an external texture, which takes only `texture`: offsets in texels as
/// coordinates, and a level of detail that changes nothing (the textures have one level, sampled the same
/// way minified or magnified).
const SMAA_SAMPLING_EXTERNAL: &str = "#define SAMPLER_colorTex samplerExternalOES
#define SMAASampleLevelZero(tex, coord) texture(tex, coord)
#define SMAASampleLevelZeroPoint(tex, coord) texture(tex, coord)
#define SMAASampleLevelZeroOffset(tex, coord, offset) texture(tex, coord + vec2(offset) * SMAA_RT_METRICS.xy)
#define SMAASample(tex, coord) texture(tex, coord)
#define SMAASamplePoint(tex, coord) texture(tex, coord)
#define SMAASampleOffset(tex, coord, offset) texture(tex, coord + vec2(offset) * SMAA_RT_METRICS.xy)
";

/// Sampling the picture as PQ's signal of its linear light (scRGB's, 1.0 at 80 nits), as
/// [`PERCEIVE`] does, for the edges' pass on linear light.
const SMAA_PERCEIVE: &str = "vec4 perceive(vec4 c) {
    vec3 p = pow(max(c.rgb, vec3(0.0)) * 0.008, vec3(0.1593017578125));
    return vec4(pow((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p), vec3(78.84375)), c.a);
}
#undef SMAASamplePoint
#define SMAASamplePoint(tex, coord) perceive(texture(tex, coord))
";

/// A full-screen triangle from the vertex's index alone, its texture coordinates 0 to 1 over the target.
const SMAA_TRIANGLE: &str = "vec2 corner() { return vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)); }
";

/// The three passes' vertex and fragment shaders' own parts, after the header and SMAA's shader.
pub(super) const SMAA_EDGES: (&str, &str) = (
    "out vec2 v_texcoord;
out vec4 v_offset[3];
void main() {
    v_texcoord = corner();
    vec4 offset[3];
    SMAAEdgeDetectionVS(v_texcoord, offset);
    v_offset = offset;
    gl_Position = vec4(v_texcoord * 2.0 - 1.0, 0.0, 1.0);
}",
    "uniform SAMPLER_colorTex color_tex;
in vec2 v_texcoord;
in vec4 v_offset[3];
out vec4 frag;
void main() {
    frag = vec4(SMAALumaEdgeDetectionPS(v_texcoord, v_offset, color_tex), 0.0, 0.0);
}",
);
pub(super) const SMAA_WEIGHTS: (&str, &str) = (
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
pub(super) const SMAA_BLENDING: (&str, &str) = (
    "out vec2 v_texcoord;
out vec4 v_offset;
void main() {
    v_texcoord = corner();
    vec4 offset;
    SMAANeighborhoodBlendingVS(v_texcoord, offset);
    v_offset = offset;
    gl_Position = vec4(v_texcoord * 2.0 - 1.0, 0.0, 1.0);
}",
    "uniform SAMPLER_colorTex color_tex;
uniform sampler2D blend_tex;
in vec2 v_texcoord;
in vec4 v_offset;
out vec4 frag;
void main() {
    frag = SMAANeighborhoodBlendingPS(v_texcoord, v_offset, color_tex, blend_tex);
}",
);

/// A pass's vertex and fragment sources, from its own parts (`SMAA_EDGES`, ...), for an external picture
/// or not; with `linear` the picture's samples, linear light, taken to PQ's signal.
pub(super) fn smaa((vertex, fragment): (&str, &str), external: bool, linear: bool) -> (String, String) {
    let (version, sampling) = match external {
        true => ("#version 300 es\n#extension GL_OES_EGL_image_external_essl3 : require\n", SMAA_SAMPLING_EXTERNAL),
        false => ("#version 300 es\n", SMAA_SAMPLING),
    };
    let perceive = if linear { SMAA_PERCEIVE } else { "" };
    let start = format!("{version}{SMAA_HEADER}{sampling}{perceive}");
    // The picture's declarations with its own sampler type.
    let shader = SMAA_SHADER.replace("SMAATexture2D(colorTex)", "SAMPLER_colorTex colorTex");
    (format!("{start}#define SMAA_INCLUDE_PS 0\n{shader}\n{SMAA_TRIANGLE}{vertex}"), format!("{start}#define SMAA_INCLUDE_VS 0\n{shader}\n{fragment}"))
}
