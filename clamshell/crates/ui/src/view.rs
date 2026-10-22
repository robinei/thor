//! A view: one screen's surface (an xdg window or a layer surface), its shm buffers, and drawing a
//! frame of it (see `render`) and handing it to the compositor.

use std::sync::Mutex;

use clamshell_core::State;
use clamshell_core::bottom::touch::TouchLayout;
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
use tiny_skia::PixmapMut;

use crate::gfx::images::Images;
use crate::gfx::{DrawCache, Rect};
use crate::input::focus::Drawn;
use crate::render::{self, Placement, Retained};
use crate::screens::{Screen, Screens};
use crate::toolkit::{FrameOut, Target};
use crate::{App, Error};

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
    /// What the last frame left: what can be tapped or focused in it.
    pub(crate) retained: Retained,
    /// The second screen's area below the bar in the last frame (logical pixels).
    pub(crate) body: Option<Rect>,
    /// shm buffers, reused once the compositor releases them (two in turn, a third if it holds both).
    buffers: Vec<Buffer>,
    /// A layer surface's place: behind the windows, or over them (see [`View::set_behind`]).
    behind: bool,
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
            retained: Retained::default(),
            body: None,
            buffers: Vec::new(),
            behind: false,
        }
    }

    pub(crate) fn window(compositor: &CompositorState, xdg: &XdgShell, qh: &QueueHandle<App>, screen: Screen, size: (u32, u32)) -> Self {
        let surface = compositor.create_surface(qh);
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, qh);
        let (title, app_id) = match screen {
            Screen::Top => ("clamshell: top", "clamshell.top"),
            Screen::Bottom => ("clamshell: bottom", "clamshell.bottom"),
        };
        window.set_title(title);
        window.set_app_id(app_id);
        window.set_min_size(Some((200, 150)));
        window.commit();
        View::new(screen, Shell::Window(window), size)
    }

    /// The second screen as a layer surface: over the whole `output`, or a panel of `corner` size in the
    /// bottom right corner.
    pub(crate) fn layer(
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<App>,
        output: Option<&wl_output::WlOutput>,
        corner: Option<(u32, u32)>,
    ) -> Self {
        let surface = compositor.create_surface(qh);
        // (Over the windows, a fullscreen one too: a window moved to the bottom screen is shown by putting
        // this behind it, see `set_behind`.)
        let layer = layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("clamshell-bottom"), output);
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

    /// The bottom screen's layer surface behind the windows (a window moved there is shown) or over
    /// them (ClamShell's view shown, also over a fullscreen window); a change is committed with the next
    /// frame. A window has no layer.
    pub(crate) fn set_behind(&mut self, behind: bool) {
        if let Shell::Layer(layer) = &self.shell
            && self.behind != behind
        {
            layer.set_layer(if behind { Layer::Background } else { Layer::Overlay });
            // (Now, not with the next frame: behind a window the surface gets no frame callbacks, so a
            // frame may never come to carry it.)
            layer.wl_surface().commit();
            self.behind = behind;
            self.dirty = true;
        }
    }

    /// Whether a frame is due: configured, changed, and the last frame shown.
    pub(crate) fn due(&self) -> bool {
        self.configured && self.dirty && !self.frame_pending
    }

    /// The buffer's size, its scale, and the burn-in drift now.
    fn placement(&self, unix_secs: u64) -> Placement {
        let scale = self.scale.max(1).unsigned_abs();
        Placement { size_px: (self.size.0 * scale, self.size.1 * scale), scale, drift: render::orbit(unix_secs) }
    }

    /// Draws a frame of `state` straight into a free shm buffer and commits it, asking for a frame
    /// callback. With every buffer still read by the compositor, nothing is drawn: the view stays dirty,
    /// and a buffer's release wakes the loop to draw again.
    ///
    /// The buffer is ABGR8888, tiny-skia's byte order (RGBA in memory, premultiplied), so it is drawn
    /// into as it is. The whole buffer is drawn and damaged: the burn-in orbit moves everything every
    /// minute anyway, and drawing whole frames keeps the views free of damage tracking.
    pub(crate) fn draw(
        &mut self,
        pool: &mut SlotPool,
        fonts: &mut DrawCache,
        state: &State,
        images: &mut Images,
        qh: &QueueHandle<App>,
    ) -> Result<Option<FrameOut>, Error> {
        let at = self.placement(state.unix_secs());
        let (w, h) = (i32::try_from(at.size_px.0)?, i32::try_from(at.size_px.1)?);
        let Some(index) = free_buffer(&mut self.buffers, pool, w, h)? else {
            return Ok(None);
        };
        let Some(buffer) = self.buffers.get(index) else {
            return Err("buffer index".into());
        };
        let Some(bytes) = buffer.canvas(pool) else {
            return Err("buffer busy".into());
        };
        let len = at.size_px.0 as usize * at.size_px.1 as usize * 4;
        let pixmap = bytes.get_mut(..len).and_then(|b| PixmapMut::from_bytes(b, at.size_px.0, at.size_px.1)).ok_or("buffer size")?;
        let rendered = render::render(pixmap, &mut self.retained, at, self.screen, state, fonts, images)?;
        let surface = self.shell.surface();
        surface.set_buffer_scale(self.scale.max(1));
        surface.damage_buffer(0, 0, w, h);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface)?;
        surface.commit();
        self.dirty = false;
        self.frame_pending = true;
        Ok(Some(rendered))
    }
}

/// A `w` x `h` buffer the compositor isn't reading: one kept, or a new one if fewer than three are kept;
/// `None` when all three are busy.
fn free_buffer(buffers: &mut Vec<Buffer>, pool: &mut SlotPool, w: i32, h: i32) -> Result<Option<usize>, Error> {
    buffers.retain(|b| b.height() == h && b.stride() == w * 4);
    if let Some(i) = buffers.iter().position(|b| b.canvas(pool).is_some()) {
        return Ok(Some(i));
    }
    if buffers.len() >= 3 {
        return Ok(None);
    }
    let (buffer, _) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Abgr8888)?;
    buffers.push(buffer);
    Ok(Some(buffers.len() - 1))
}

impl Drawn for Screens<Option<View>> {
    fn targets(&self, screen: Screen) -> &[Target] {
        self[screen].as_ref().map_or(&[], |v| &v.retained.targets)
    }

    fn size(&self, screen: Screen) -> (f32, f32) {
        self[screen].as_ref().map_or((1.0, 1.0), |v| (v.size.0 as f32, v.size.1 as f32))
    }
}

/// Tells the virtual touchpad the second screen's size (pixels).
pub(crate) fn publish(touch: &Mutex<TouchLayout>, screen: (f32, f32)) {
    if let Ok(mut layout) = touch.lock() {
        layout.screen = screen;
    }
}
