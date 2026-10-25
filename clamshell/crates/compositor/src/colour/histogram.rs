//! The histogram of a window's picture for its curve's edit screen (see
//! `clamshell_state::State::histogram_window`): the picture drawn small by a program that writes each
//! pixel's luminance as a level of the content's signal, read back and counted, a few times a second
//! while the screen is open.

use std::time::{Duration, Instant};

use clamshell_state::session::presentation::{HISTOGRAM_BINS, Histogram};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen, Renderer,
            element::Id,
            gles::{GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
            utils::RendererSurfaceStateUserData,
        },
    },
    utils::{Rectangle, Transform},
};

use crate::server::Server;

/// The size the picture is drawn at to be counted.
const SIZE: (i32, i32) = (96, 54);
/// How often it is measured.
const EVERY: Duration = Duration::from_millis(250);

/// Each pixel's luminance as a level, in the red channel, by the kind in `clarity.x`: SDR's signal
/// (sRGB's encoding, BT.709's luminance); PQ's, of its light (BT.2020's luminance) scaled by `clarity.y`
/// so that its white is at the reference; or for linear light (scRGB's, BT.709's), PQ's of it with 1.0
/// at the reference.
const MEASURE_GLSL: &str = "float srgb_decode(float e) { return e <= 0.04045 ? e / 12.92 : pow((e + 0.055) / 1.055, 2.4); }
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

#[derive(Default)]
pub(crate) struct Measure {
    program: Option<Result<GlesTexProgram, String>>,
    target: Option<GlesTexture>,
    last: Option<Instant>,
    /// Why measuring failed, logged once; it isn't tried again.
    failed: bool,
    /// The last histogram measured, for the state.
    pub measured: Option<Histogram>,
}

impl Measure {
    /// Measures the window the state asks for, if it is time to.
    pub fn turn(&mut self, renderer: &mut GlesRenderer, server: &Server, now: Instant) {
        if self.failed {
            return;
        }
        let Some(window) = server.core.state().histogram_window() else { return };
        if self.last.is_some_and(|last| now < last + EVERY) {
            return;
        }
        self.last = Some(now);
        if let Err(error) = self.measure(renderer, server, window) {
            log::warn!("compositor: the curve's histogram can't be measured, the plot shows none: {error}");
            self.failed = true;
        }
    }

    fn measure(&mut self, renderer: &mut GlesRenderer, server: &Server, window: i64) -> Result<(), String> {
        let Some(client) = server.windows.iter().find(|w| w.id == window && w.mapped) else { return Ok(()) };
        // The picture: the surface with the largest buffer (Wine draws a game in a subsurface).
        let context = renderer.context_id();
        let mut picture: Option<(GlesTexture, i64, Id)> = None;
        client.window.with_surfaces(|surface, states| {
            let state = states.data_map.get::<RendererSurfaceStateUserData>();
            let texture = state.and_then(|state| state.lock().ok()?.texture::<GlesTexture>(context.clone()).cloned());
            if let Some(texture) = texture {
                let size = smithay::backend::renderer::Texture::size(&texture);
                let area = i64::from(size.w) * i64::from(size.h);
                if picture.as_ref().is_none_or(|(_, largest, _)| area > *largest) {
                    picture = Some((texture, area, Id::from_wayland_resource(surface)));
                }
            }
        });
        let Some((source, _, id)) = picture else { return Ok(()) };
        let colour = server.colour.surfaces.get(&id).map(|colour| **colour).unwrap_or_default();
        let hdr = colour.hdr();
        let kind = match colour.transfer {
            _ if !hdr => 0.0,
            clamshell_state::session::presentation::TransferFunction::Pq => 1.0,
            _ => 2.0,
        };
        let to_reference = 203.0 / colour.luminances().reference.max(1) as f32;
        if self.program.is_none() {
            self.program = Some(super::chain::pass_program(renderer, "measure()", MEASURE_GLSL).map_err(|e| e.to_string()));
        }
        let program = match &self.program {
            Some(Ok(program)) => program.clone(),
            Some(Err(error)) => return Err(error.clone()),
            None => return Ok(()),
        };
        if self.target.is_none() {
            self.target = Some(renderer.create_buffer(Fourcc::Abgr8888, SIZE.into()).map_err(|e| e.to_string())?);
        }
        let Some(target) = self.target.as_mut() else { return Ok(()) };
        let uniform = [Uniform::new("clarity", [kind, to_reference, 0.0, 0.0])];
        super::chain::render(renderer, target, &source, Some((&program, uniform)), Transform::Normal).map_err(|e| e.to_string())?;
        let bound = renderer.bind(target).map_err(|e| e.to_string())?;
        let mapping = renderer.copy_framebuffer(&bound, Rectangle::from_size(SIZE.into()), Fourcc::Abgr8888).map_err(|e| e.to_string())?;
        drop(bound);
        let bytes = renderer.map_texture(&mapping).map_err(|e| e.to_string())?;
        let levels: Vec<u8> = bytes.chunks_exact(4).map(|pixel| pixel.first().copied().unwrap_or(0)).collect();
        let (x0, y0, x1, y1) = picture_within(&levels, SIZE.0 as usize, SIZE.1 as usize);
        let width = SIZE.0 as usize;
        let inside = (y0..y1).flat_map(|y| (x0..x1).map(move |x| y * width + x)).filter_map(|i| levels.get(i).copied());
        self.measured = Some(Histogram { window, hdr, bins: count(inside) });
        Ok(())
    }
}

/// The part of a `width` by `height` grid of levels that is picture, as columns `x0..x1` and rows `y0..y1`:
/// without the black bars a game draws at its edges (letterboxing, or 4:3 in a wider frame), whole rows
/// and columns of black, and the one row or column next to each, whose samples blend the bar with the
/// picture. A picture that is black all over is all picture.
fn picture_within(levels: &[u8], width: usize, height: usize) -> (usize, usize, usize, usize) {
    let black = |i: usize| levels.get(i).is_none_or(|level| *level == 0);
    let row = |y: usize| (0..width).all(|x| black(y * width + x));
    let column = |x: usize| (0..height).all(|y| black(y * width + x));
    let top = (0..height).take_while(|y| row(*y)).count();
    if top == height {
        return (0, 0, width, height);
    }
    let bottom = (0..height).rev().take_while(|y| row(*y)).count();
    let left = (0..width).take_while(|x| column(*x)).count();
    let right = (0..width).rev().take_while(|x| column(*x)).count();
    // The line next to a bar blends it with the picture.
    let beside = |bar: usize| if bar > 0 { bar + 1 } else { 0 };
    let (y0, y1) = (beside(top), height.saturating_sub(beside(bottom)));
    let (x0, x1) = (beside(left), width.saturating_sub(beside(right)));
    if y0 >= y1 || x0 >= x1 { (0, 0, width, height) } else { (x0, y0, x1, y1) }
}

/// The levels (0 to 255) counted into [`HISTOGRAM_BINS`] equal steps.
fn count(levels: impl Iterator<Item = u8>) -> [u16; HISTOGRAM_BINS] {
    let mut bins = [0u16; HISTOGRAM_BINS];
    for level in levels {
        if let Some(bin) = bins.get_mut(usize::from(level) * HISTOGRAM_BINS / 256) {
            *bin = bin.saturating_add(1);
        }
    }
    bins
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn levels_are_counted_into_equal_steps() {
        let bins = count([0, 3, 4, 255, 255, 128].into_iter());
        assert_eq!((bins[0], bins[1], bins[32], bins[HISTOGRAM_BINS - 1]), (2, 1, 1, 2));
        assert_eq!(bins.iter().map(|b| u32::from(*b)).sum::<u32>(), 6);
    }

    #[test]
    fn a_games_own_black_bars_and_the_lines_beside_them_are_left_out() {
        // 10 by 8: letterboxed by two black rows top and bottom, a blended row inside each, then picture.
        let (w, h) = (10, 8);
        let mut levels = vec![200u8; w * h];
        for y in [0, 1, 6, 7] {
            levels[y * w..(y + 1) * w].fill(0);
        }
        levels[2 * w..3 * w].fill(40);
        assert_eq!(picture_within(&levels, w, h), (0, 3, w, 5));
        // Pillarboxed: one black column each side.
        let mut levels = vec![120u8; w * h];
        for y in 0..h {
            levels[y * w] = 0;
            levels[y * w + w - 1] = 0;
        }
        assert_eq!(picture_within(&levels, w, h), (2, 0, w - 2, h));
        // A dark scene with some black pixels at its edge, but no whole black line: all of it.
        let mut levels = vec![30u8; w * h];
        levels[0] = 0;
        assert_eq!(picture_within(&levels, w, h), (0, 0, w, h));
        // Black all over: all of it.
        assert_eq!(picture_within(&vec![0u8; w * h], w, h), (0, 0, w, h));
    }
}
