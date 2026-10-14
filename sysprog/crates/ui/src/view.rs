//! A view: one screen's surface (an xdg window or a layer surface), its shm buffers, and drawing a
//! frame of it, moved within the burn-in margin; the second screen's frames also tell the touchpad mode
//! where things are.

use std::sync::Mutex;

use smithay_client_toolkit::{
    compositor::{CompositorState, FrameCallbackData},
    reexports::client::{
        QueueHandle,
        protocol::{wl_output, wl_shm, wl_surface::WlSurface},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface},
        xdg::{
            XdgShell,
            window::{Window, WindowDecorations},
        },
    },
    shm::slot::{Buffer, SlotPool},
};
use sysprog_core::touch::{Region, TouchLayout};
use sysprog_core::{Id, State};
use tiny_skia::Pixmap;

use crate::draw::{self, DrawCache, Rect};
use crate::views::{self, Scratch, Target};
use crate::{App, Error, Screen, images};

/// OLED burn-in protection: every screen is laid out inside a black margin of MARGIN buffer pixels on
/// each side, and drawn moved within it by an offset that drifts slowly (about a pixel a minute, along an
/// irregular path within ±MARGIN), so static elements like the bar's icons and the clock don't sit on the
/// same pixels for hours, and nothing ever reaches the edge or is cut off. Tap areas move with the
/// picture.
pub(crate) const MARGIN: u32 = 4;

pub(crate) fn orbit(unix_secs: u64) -> (i32, i32) {
    let minutes = (unix_secs / 60) as f32;
    let tau = std::f32::consts::TAU;
    let reach = MARGIN as f32;
    let dx = (reach * (minutes * tau / 17.0).sin()).round() as i32;
    let dy = (reach * (minutes * tau / 23.0).sin()).round() as i32;
    (dx, dy)
}

/// The logical size to lay a screen out in (inside the margin), and the buffer-pixel offset to draw it
/// at for `orbit`.
pub(crate) fn inset(size_px: (u32, u32), scale: u32, (dx, dy): (i32, i32)) -> ((f32, f32), (i32, i32)) {
    let scale = scale.max(1) as f32;
    let w = size_px.0.saturating_sub(2 * MARGIN) as f32 / scale;
    let h = size_px.1.saturating_sub(2 * MARGIN) as f32 / scale;
    ((w, h), (MARGIN as i32 + dx, MARGIN as i32 + dy))
}

pub(crate) enum Shell {
    Window(Window),
    Layer(LayerSurface),
}

impl Shell {
    pub(crate) fn surface(&self) -> &WlSurface {
        match self {
            Shell::Window(w) => w.wl_surface(),
            Shell::Layer(l) => l.wl_surface(),
        }
    }
}

pub(crate) struct View {
    pub(crate) screen: Screen,
    pub(crate) shell: Shell,
    /// Logical size, from the compositor's configure (or `default_size` if it leaves it to us).
    pub(crate) size: (u32, u32),
    pub(crate) default_size: (u32, u32),
    pub(crate) scale: i32,
    pub(crate) configured: bool,
    pub(crate) dirty: bool,
    /// A frame was committed and its callback hasn't come yet: wait, then draw the latest state once.
    pub(crate) frame_pending: bool,
    /// What the last frame drew that can be tapped or focused.
    pub(crate) targets: Vec<Target>,
    pub(crate) pixmap: Option<Pixmap>,
    /// shm buffers, reused once the compositor releases them (two in turn, a third if it holds both).
    pub(crate) buffers: Vec<Buffer>,
    pub(crate) scratch: Scratch,
}

impl View {
    fn new(screen: Screen, shell: Shell, default_size: (u32, u32)) -> Self {
        View {
            screen,
            shell,
            size: default_size,
            default_size,
            scale: 1,
            configured: false,
            dirty: true,
            frame_pending: false,
            targets: Vec::new(),
            pixmap: None,
            buffers: Vec::new(),
            scratch: Scratch::default(),
        }
    }

    pub(crate) fn window(
        compositor: &CompositorState,
        xdg: &XdgShell,
        qh: &QueueHandle<App>,
        screen: Screen,
        size: (u32, u32),
    ) -> Self {
        let surface = compositor.create_surface(qh);
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, qh);
        let (title, app_id) = match screen {
            Screen::Top => ("sysprog: top", "sysprog.top"),
            Screen::Bottom => ("sysprog: bottom", "sysprog.bottom"),
        };
        window.set_title(title);
        window.set_app_id(app_id);
        window.set_min_size(Some((200, 150)));
        window.commit();
        View::new(screen, Shell::Window(window), size)
    }

    pub(crate) fn layer(
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<App>,
        output: Option<&wl_output::WlOutput>,
        corner: Option<(u32, u32)>,
    ) -> Self {
        let surface = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(qh, surface, Layer::Top, Some("sysprog-bottom"), output);
        match corner {
            // The whole output (the device's second screen).
            None => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                layer.set_size(0, 0);
            }
            // A panel of this size in the bottom right corner (testing on a desktop).
            Some((w, h)) => {
                layer.set_anchor(Anchor::BOTTOM | Anchor::RIGHT);
                layer.set_size(w, h);
            }
        }
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        View::new(Screen::Bottom, Shell::Layer(layer), (1, 1))
    }

    pub(crate) fn draw(
        &mut self,
        pool: &mut SlotPool,
        fonts: &mut DrawCache,
        state: &State,
        touch: &Mutex<TouchLayout>,
        images: &mut images::Images,
        qh: &QueueHandle<App>,
    ) -> Result<Option<usize>, Error> {
        let scale = self.scale.max(1);
        let (lw, lh) = self.size;
        let scale_u = scale.unsigned_abs();
        let (w, h) = (lw * scale_u, lh * scale_u);
        if self.pixmap.as_ref().map(|p| (p.width(), p.height())) != Some((w, h)) {
            self.pixmap = Pixmap::new(w, h);
        }
        let Some(pixmap) = self.pixmap.as_mut() else {
            return Err(format!("can't allocate a {w}x{h} pixmap").into());
        };
        let ((cw, ch), offset) = inset((w, h), scale_u, orbit(state.unix_secs));
        let mut body = None;
        let mut rows = None;
        {
            let mut canvas = draw::Canvas::new(pixmap, scale as f32, fonts).with_offset(offset);
            let (w, h) = (cw, ch);
            match self.screen {
                Screen::Top => {
                    rows = Some(views::top(&mut canvas, state, w, h, images, &mut self.scratch, &mut self.targets));
                    images.end_frame();
                }
                Screen::Bottom => {
                    body = Some(views::bottom(&mut canvas, state, w, h, images, &mut self.scratch, &mut self.targets));
                }
            }
        }

        let (sx, sy) = (offset.0 as f32 / scale as f32, offset.1 as f32 / scale as f32);
        for target in &mut self.targets {
            target.rect.x += sx;
            target.rect.y += sy;
        }
        if let Some(body) = body {
            publish(touch, &self.targets, body, (sx, sy), scale as f32, (w as f32, h as f32));
        }

        let (wi, hi) = (i32::try_from(w)?, i32::try_from(h)?);
        // A buffer of this size the compositor isn't reading; a new one only if all are in use.
        self.buffers.retain(|b| b.height() == hi && b.stride() == wi * 4);
        let free = self.buffers.iter().position(|b| b.canvas(pool).is_some());
        let index = match free {
            Some(i) => i,
            None if self.buffers.len() < 3 => {
                let (buffer, _) = pool.create_buffer(wi, hi, wi * 4, wl_shm::Format::Argb8888)?;
                self.buffers.push(buffer);
                self.buffers.len() - 1
            }
            // All busy: keep the view dirty; a buffer release wakes the loop and it draws then.
            None => return Ok(rows),
        };
        let Some(buffer) = self.buffers.get(index) else {
            return Err("buffer index".into());
        };
        let Some(bytes) = buffer.canvas(pool) else {
            return Err("buffer busy".into());
        };
        // tiny-skia is RGBA in memory, ARGB8888 is BGRA (little-endian); both premultiplied. sway also
        // takes ABGR8888 (RGBA in memory), which would save this copy; KWin doesn't.
        for (dst, src) in bytes.chunks_exact_mut(4).zip(pixmap.data().chunks_exact(4)) {
            if let ([b, g, r, a], [sr, sg, sb, sa]) = (dst, src) {
                (*b, *g, *r, *a) = (*sb, *sg, *sr, *sa);
            }
        }

        let surface = self.shell.surface();
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, wi, hi);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface)?;
        surface.commit();
        self.dirty = false;
        self.frame_pending = true;
        Ok(rows)
    }
}

/// Tells the touchpad mode's input side where things are, in screen pixels: the area below the bar is
/// the touchpad; the bar's targets stay taps. No allocation once the region list has grown.
fn publish(touch: &Mutex<TouchLayout>, targets: &[Target], body: Rect, (sx, sy): (f32, f32), scale: f32, screen: (f32, f32)) {
    let Ok(mut layout) = touch.lock() else {
        return;
    };
    layout.screen = screen;
    layout.pad = ((body.x + sx) * scale, (body.y + sy) * scale, (body.x + sx + body.w) * scale, (body.y + sy + body.h) * scale);
    layout.regions.clear();
    for t in targets.iter().filter(|t| matches!(t.id, Id::Mode(_) | Id::PadMouse)) {
        let r = t.rect;
        layout.regions.push(Region { x0: r.x * scale, y0: r.y * scale, x1: (r.x + r.w) * scale, y1: (r.y + r.h) * scale, id: t.id.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_stays_in_the_margin_and_moves_slowly() {
        let mut last = orbit(0);
        for minute in 1..2000u64 {
            let (dx, dy) = orbit(minute * 60);
            assert!(dx.unsigned_abs() <= MARGIN && dy.unsigned_abs() <= MARGIN);
            assert!((dx - last.0).abs() <= 2 && (dy - last.1).abs() <= 2);
            last = (dx, dy);
        }
    }
}
