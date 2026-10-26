//! The same cursor composition serves nested and later direct outputs.

use std::time::{Duration, Instant};

use clamshell_ui::Error;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Id, Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                texture::TextureRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
            utils::import_surface_tree,
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::Resource,
    utils::{Point, Transform},
    wayland::compositor::with_states,
};
use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

use crate::{render::Element, server::Server};

const HIDE_AFTER: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct Activity {
    last_motion: Option<Instant>,
}
impl Activity {
    pub fn motion(&mut self, now: Instant) {
        self.last_motion = Some(now);
    }
    pub fn hide(&mut self) {
        self.last_motion = None;
    }
    pub fn visible(&self, now: Instant) -> bool {
        self.last_motion.is_some_and(|last| now.saturating_duration_since(last) < HIDE_AFTER)
    }
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.last_motion.map(|last| last + HIDE_AFTER).filter(|deadline| *deadline > now)
    }
}

struct Frame {
    texture: GlesTexture,
    id: Id,
    hotspot: (f64, f64),
    delay: u32,
}

pub(crate) struct Cursor {
    theme: CursorTheme,
    size: u32,
    cached: Vec<(CursorIcon, Vec<Frame>)>,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            theme: CursorTheme::load(&std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into())),
            size: std::env::var("XCURSOR_SIZE").ok().and_then(|size| size.parse().ok()).unwrap_or(24).clamp(1, 256),
            cached: Vec::new(),
        }
    }

    pub fn elements(&mut self, server: &mut Server, renderer: &mut GlesRenderer, out: &mut Vec<Element>) -> Result<(), Error> {
        if !server.cursor_activity.visible(Instant::now()) {
            return Ok(());
        }
        let status = server.effective_cursor();
        match status {
            CursorImageStatus::Hidden => {}
            CursorImageStatus::Surface(surface) if surface.is_alive() => {
                if let Err(error) = import_surface_tree(renderer, &surface) {
                    server.reject_client(&surface, &format!("cannot import cursor buffer: {error}"));
                    server.cursor = CursorImageStatus::default_named();
                    return Ok(());
                }
                let hotspot = with_states(&surface, |states| {
                    states.data_map.get::<CursorImageSurfaceData>().and_then(|data| data.lock().ok()).map(|data| data.hotspot).unwrap_or_default()
                });
                let location = server.pointer - hotspot.to_f64();
                out.splice(
                    0..0,
                    render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlesRenderer>>(
                        renderer,
                        &surface,
                        location.to_physical(1.0).to_i32_round(),
                        1.0,
                        1.0,
                        Kind::Cursor,
                    )
                    .into_iter()
                    .map(Element::from),
                );
                let [top, bottom] = &server.outputs;
                let output = if server.pointer.x < server.top_width as f64 { top } else { bottom };
                smithay::desktop::utils::send_frames_surface_tree(&surface, output, server.started.elapsed(), Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
            CursorImageStatus::Named(icon) => self.named(icon, server, renderer, out)?,
            CursorImageStatus::Surface(_) => {
                server.cursor = CursorImageStatus::default_named();
                self.named(CursorIcon::Default, server, renderer, out)?;
            }
        }
        Ok(())
    }

    fn named(&mut self, icon: CursorIcon, server: &Server, renderer: &mut GlesRenderer, out: &mut Vec<Element>) -> Result<(), Error> {
        let index = if let Some(index) = self.cached.iter().position(|(cached, _)| *cached == icon) {
            index
        } else {
            let images = self
                .theme
                .load_icon(icon.name())
                .or_else(|| self.theme.load_icon("default"))
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|bytes| parse_xcursor(&bytes))
                .filter(|images| !images.is_empty());
            let images = images.unwrap_or_else(|| {
                log::warn!("compositor: no usable cursor theme image for {}, using the built-in pointer", icon.name());
                vec![fallback()]
            });
            let nearest = images.iter().min_by_key(|image| image.size.abs_diff(self.size)).map(|image| image.size);
            let mut frames = Vec::new();
            for image in images.into_iter().filter(|image| Some(image.size) == nearest) {
                let size = (i32::try_from(image.width)?, i32::try_from(image.height)?).into();
                let texture = renderer.import_memory(&image.pixels_rgba, Fourcc::Abgr8888, size, false)?;
                frames.push(Frame { id: Id::new(), texture, hotspot: (image.xhot as f64, image.yhot as f64), delay: image.delay });
            }
            let index = self.cached.len();
            self.cached.push((icon, frames));
            index
        };
        let Some((_, frames)) = self.cached.get(index) else { return Ok(()) };
        let frame = animation_frame(frames.iter().map(|frame| frame.delay), server.started.elapsed().as_millis());
        if let Some(frame) = frames.get(frame) {
            let location = server.pointer - Point::from(frame.hotspot);
            out.insert(
                0,
                TextureRenderElement::from_static_texture(
                    frame.id.clone(),
                    renderer.context_id(),
                    location.to_physical(1.0),
                    frame.texture.clone(),
                    1,
                    Transform::Normal,
                    None,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                )
                .into(),
            );
        }
        Ok(())
    }
}

fn animation_frame(delays: impl Iterator<Item = u32> + Clone, millis: u128) -> usize {
    let total: u128 = delays.clone().map(u128::from).sum();
    if total == 0 {
        return 0;
    }
    let mut elapsed = millis % total;
    for (index, delay) in delays.enumerate() {
        if elapsed < u128::from(delay) {
            return index;
        }
        elapsed -= u128::from(delay);
    }
    0
}

fn fallback() -> Image {
    let mut pixels = Vec::with_capacity(16 * 24 * 4);
    for y in 0..24 {
        for x in 0..16 {
            let inside = x <= y / 2 && y < 22;
            let edge = x == 0 || x == y / 2 || y == 21;
            let shade = if edge { 24 } else { 180 };
            let shade = if inside { shade } else { 0 };
            pixels.extend_from_slice(&[shade, shade, shade, if inside { 255 } else { 0 }]);
        }
    }
    Image { size: 24, width: 16, height: 24, xhot: 0, yhot: 0, delay: 0, pixels_rgba: pixels, pixels_argb: Vec::new() }
}

impl Server {
    pub(crate) fn effective_cursor(&self) -> CursorImageStatus {
        if self.pointer_locked() {
            return CursorImageStatus::Hidden;
        }
        let client = self.seat.get_pointer().is_some_and(|pointer| pointer.current_focus().is_some())
            && (self.hit(self.pointer).is_some() || self.pointer_window.is_some_and(|window| self.visible(window)));
        match &self.cursor {
            CursorImageStatus::Surface(surface) if !surface.is_alive() => CursorImageStatus::default_named(),
            image if client => image.clone(),
            _ => CursorImageStatus::default_named(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn motion_shows_the_cursor_and_touch_or_three_seconds_without_motion_hides_it() {
        let start = Instant::now();
        let mut activity = Activity::default();
        assert!(!activity.visible(start));
        assert_eq!(activity.deadline(start), None);
        activity.motion(start);
        assert!(activity.visible((start + HIDE_AFTER).checked_sub(Duration::from_nanos(1)).unwrap()));
        assert_eq!(activity.deadline(start), Some(start + HIDE_AFTER));
        assert!(!activity.visible(start + HIDE_AFTER));
        assert_eq!(activity.deadline(start + HIDE_AFTER), None);
        activity.motion(start + HIDE_AFTER);
        activity.motion(start + HIDE_AFTER + Duration::from_secs(1));
        assert!(activity.visible(start + HIDE_AFTER * 2));
        activity.hide();
        assert!(!activity.visible(start + HIDE_AFTER * 2));
        assert_eq!(activity.deadline(start + HIDE_AFTER * 2), None);
        activity.motion(start + HIDE_AFTER * 2);
        assert!(activity.visible(start + HIDE_AFTER * 2));
    }

    #[test]
    fn cursor_animation_wraps_and_zero_delay_images_do_not_stall_it() {
        let frame = |time| animation_frame([0, 20, 30].into_iter(), time);
        assert_eq!(frame(0), 1);
        assert_eq!(frame(19), 1);
        assert_eq!(frame(20), 2);
        assert_eq!(frame(49), 2);
        assert_eq!(frame(50), 1);
        assert_eq!(animation_frame([0, 0].into_iter(), u128::MAX), 0);
        assert_eq!(animation_frame(std::iter::empty(), 10), 0);
        let image = fallback();
        assert_eq!(image.pixels_rgba.len(), (image.width * image.height * 4) as usize);
    }
}
