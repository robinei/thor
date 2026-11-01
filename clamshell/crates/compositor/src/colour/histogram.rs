//! The histogram of a window's picture for its curve's edit screen (see
//! `clamshell_state::State::histogram_window`): the picture drawn small by a program that writes each
//! pixel's luminance as a level of the content's signal, read back and counted, a few times a second
//! while the screen is open. The readback is read at a later turn, once a fence says the GPU has done
//! the copy.

use std::time::{Duration, Instant};

use clamshell_state::colour::{HISTOGRAM_BINS, Histogram};
use smithay::{
    backend::{
        allocator::Fourcc,
        egl::fence::EGLFence,
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
/// How long after measuring it failed it is tried again.
const RETRY: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct Measure {
    program: Option<GlesTexProgram>,
    target: Option<GlesTexture>,
    last: Option<Instant>,
    /// When measuring, having failed, is tried again; whether it failed since it last succeeded (logged at
    /// warn the first time).
    retry: Option<Instant>,
    failing: bool,
    /// The last histogram measured, for the state.
    pub measured: Option<Histogram>,
    /// A readback started and not yet read (see [`Reading`]).
    reading: Option<Reading>,
    /// Fences can't be made, logged once; a readback is then read at the next turn, which can wait for
    /// the GPU.
    no_fence: bool,
}

/// A readback of the picture drawn small, read once the GPU has done the copy, so that mapping it doesn't
/// wait on the GPU.
struct Reading {
    mapping: <GlesRenderer as ExportMem>::TextureMapping,
    /// Signalled once the copy is done; none if it couldn't be made.
    fence: Option<EGLFence>,
    window: i64,
    hdr: bool,
}

impl Measure {
    /// When the loop wakes for it: soon for a readback outstanding, else at the next sample while the
    /// editor asks for one, or once measuring is tried again after it failed.
    pub(crate) fn deadline(&self, wanted: bool, now: Instant) -> Option<Instant> {
        if self.reading.is_some() {
            Some(now + Duration::from_millis(2))
        } else if !wanted {
            None
        } else if let Some(retry) = self.retry {
            Some(retry)
        } else {
            Some(self.last.map_or(now, |last| last + EVERY))
        }
    }

    /// Measuring failed at `now` with `error`: tried again after [`RETRY`], the program made again. Warned
    /// of the first time, until it succeeds again.
    fn failed(&mut self, now: Instant, error: &str) {
        if std::mem::replace(&mut self.failing, true) {
            log::debug!("compositor: the curve's histogram can't be measured still: {error}");
        } else {
            log::warn!("compositor: the curve's histogram can't be measured, the plot shows none: {error}; tried again every {} s", RETRY.as_secs());
        }
        self.retry = Some(now + RETRY);
        self.program = None;
    }

    /// Measuring succeeded: logged if it had failed.
    fn measured(&mut self) {
        self.retry = None;
        if std::mem::take(&mut self.failing) {
            log::info!("compositor: the curve's histogram is measured again");
        }
    }

    /// Measures the window the state asks for, if it is time to.
    pub fn turn(&mut self, renderer: &mut GlesRenderer, server: &Server, now: Instant) {
        if self.retry.is_some_and(|retry| now < retry) {
            return;
        }
        if let Some(reading) = self.reading.take() {
            if !copied(renderer, reading.fence.as_ref(), "the curve's histogram") {
                self.reading = Some(reading);
                return;
            }
            match renderer.map_texture(&reading.mapping) {
                Ok(bytes) => self.measured = Some(histogram(bytes, reading.window, reading.hdr)),
                Err(error) => log::warn!("compositor: the curve's histogram: reading it back: {error}"),
            }
        }
        let Some(window) = server.core.state().histogram_window() else { return };
        if self.last.is_some_and(|last| now < last + EVERY) {
            return;
        }
        self.last = Some(now);
        match self.measure(renderer, server, window) {
            Ok(()) => self.measured(),
            Err(error) => self.failed(now, &error),
        }
    }

    fn measure(&mut self, renderer: &mut GlesRenderer, server: &Server, window: i64) -> Result<(), String> {
        let Some(client) = server.windows.iter().find(|w| w.id == window && w.mapped) else { return Ok(()) };
        // The picture's surface's texture (Wine draws a game in a subsurface).
        let Some(surface) = client.window.picture_surface() else { return Ok(()) };
        let context = renderer.context_id();
        let source = smithay::wayland::compositor::with_states(&surface, |states| {
            let state = states.data_map.get::<RendererSurfaceStateUserData>();
            state.and_then(|state| state.lock().ok()?.texture::<GlesTexture>(context).cloned())
        });
        let Some(source) = source else { return Ok(()) };
        let id = Id::from_wayland_resource(&surface);
        let colour = server.colour.surfaces.get(&id).map(|colour| **colour).unwrap_or_default();
        let hdr = colour.hdr();
        let kind = match colour.transfer {
            _ if !hdr => 0.0,
            clamshell_state::colour::TransferFunction::Pq => 1.0,
            _ => 2.0,
        };
        let to_reference = 203.0 / colour.luminances().reference.max(1) as f32;
        let program = match &self.program {
            Some(program) => program.clone(),
            None => {
                let program = super::chain::pass_program(renderer, "measure()", super::shaders::MEASURE, false).map_err(|e| e.to_string())?;
                self.program = Some(program.clone());
                program
            }
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
        let fence = fence(renderer).map_or_else(|error| self.without_fence(error), Some);
        self.reading = Some(Reading { mapping, fence, window, hdr });
        Ok(())
    }

    fn without_fence(&mut self, error: String) -> Option<EGLFence> {
        if !std::mem::replace(&mut self.no_fence, true) {
            log::warn!("compositor: the curve's histogram: no fence for its readback, so reading it may wait on the GPU: {error}");
        }
        None
    }
}

/// A fence after the renderer's commands so far, such as a readback's copy.
pub(super) fn fence(renderer: &mut GlesRenderer) -> Result<EGLFence, String> {
    let display = renderer.egl_context().display().clone();
    match renderer.with_context(|_| EGLFence::create(&display)) {
        Ok(Ok(fence)) => Ok(fence),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether the GPU has done the copy `fence` follows, checked without waiting; a copy without a fence is
/// taken as done. The check flushes the renderer's commands, so the fence comes without another frame.
/// `what` the copy is for, for the log.
pub(super) fn copied(renderer: &mut GlesRenderer, fence: Option<&EGLFence>, what: &str) -> bool {
    let Some(fence) = fence else { return true };
    match renderer.with_context(|_| fence.client_wait(Some(Duration::ZERO), true)) {
        Ok(Ok(signalled)) => signalled,
        Ok(Err(error)) => {
            log::warn!("compositor: {what}: checking its readback's fence: {error}; it is read now");
            true
        }
        Err(error) => {
            log::warn!("compositor: {what}: checking its readback's fence: {error}; it is read now");
            true
        }
    }
}

/// The histogram of a readback's `bytes` (RGBA, the level in red) of `window`'s picture.
fn histogram(bytes: &[u8], window: i64, hdr: bool) -> Histogram {
    let levels: Vec<u8> = bytes.chunks_exact(4).map(|pixel| pixel.first().copied().unwrap_or(0)).collect();
    let (x0, y0, x1, y1) = picture_within(&levels, SIZE.0 as usize, SIZE.1 as usize);
    let width = SIZE.0 as usize;
    let inside = (y0..y1).flat_map(|y| (x0..x1).map(move |x| y * width + x)).filter_map(|i| levels.get(i).copied());
    Histogram { window, hdr, bins: count(inside) }
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

    /// The loop wakes for the next sample while it is wanted; after a failure, to try again a while later,
    /// and on as before once that succeeds.
    #[test]
    fn sampling_has_a_deadline_only_while_wanted_and_is_tried_again_after_a_failure() {
        let now = Instant::now();
        let mut measure = Measure::default();
        assert_eq!(measure.deadline(false, now), None);
        assert_eq!(measure.deadline(true, now), Some(now));
        measure.last = Some(now);
        assert_eq!(measure.deadline(true, now), Some(now + EVERY));
        assert_eq!(measure.deadline(false, now), None);
        measure.failed(now, "a test");
        assert!(measure.failing);
        assert_eq!(measure.deadline(true, now), Some(now + RETRY), "tried again, not given up");
        assert_eq!(measure.deadline(false, now), None);
        let later = now + RETRY;
        measure.failed(later, "a test");
        assert_eq!(measure.deadline(true, later), Some(later + RETRY));
        measure.last = Some(later + RETRY);
        measure.measured();
        assert!(!measure.failing);
        assert_eq!(measure.deadline(true, later + RETRY), Some(later + RETRY + EVERY), "on as before");
    }

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
