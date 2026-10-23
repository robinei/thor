//! A view is one screen's surface, either an xdg window or a layer surface, with its shm buffers. When
//! a frame is due it has the core record the screen, and if it changed, draws what changed into a free
//! buffer and hands it to the compositor.

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

use super::App;
use crate::Error;
use crate::core::Core;
use crate::screens::Screen;

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
    /// The logical size when the compositor's configure leaves the size to us.
    pub(crate) default_size: (u32, u32),
    pub(crate) configured: bool,
    /// The compositor configured it, and waits for a frame in answer even if nothing changed.
    pub(crate) reconfigured: bool,
    /// Whether it can be seen. A hidden view is not recorded until it can be seen again.
    pub(crate) visible: bool,
    /// A frame was committed and its callback hasn't come yet. Until it comes, wait; then draw the latest
    /// state once.
    pub(crate) frame_pending: bool,
    /// shm buffers, reused once the compositor releases them, each with the frame it holds (0 for none).
    /// Two are used in turn, and a third is added if the compositor holds both.
    buffers: Vec<(Buffer, u64)>,
    /// Whether a layer surface is behind the windows rather than over them (see [`View::set_behind`]).
    behind: bool,
}

impl View {
    fn new(screen: Screen, shell: Shell, default_size: (u32, u32)) -> Self {
        View { screen, shell, default_size, configured: false, reconfigured: false, visible: true, frame_pending: false, buffers: Vec::new(), behind: false }
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

    /// The second screen as a layer surface, either over the whole `output` or as a panel of `corner` size
    /// in the bottom right corner.
    pub(crate) fn layer(
        compositor: &CompositorState,
        layer_shell: &LayerShell,
        qh: &QueueHandle<App>,
        output: Option<&wl_output::WlOutput>,
        corner: Option<(u32, u32)>,
    ) -> Self {
        let surface = compositor.create_surface(qh);
        // The surface goes over the windows, including a fullscreen one. A window moved to the bottom
        // screen is shown by putting this behind it (see `set_behind`).
        let layer = layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("clamshell-bottom"), output);
        match corner {
            // The whole output, which is the device's second screen.
            None => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                layer.set_size(0, 0);
            }
            // A panel of this size in the bottom right corner, for testing on a desktop.
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

    /// Puts the bottom screen's layer surface behind the windows, so a window moved there is shown, or
    /// over them, so ClamShell's view is shown even over a fullscreen window. A window has no layer, so
    /// this does nothing for it.
    pub(crate) fn set_behind(&mut self, behind: bool) {
        if let Shell::Layer(layer) = &self.shell
            && self.behind != behind
        {
            layer.set_layer(if behind { Layer::Background } else { Layer::Overlay });
            // Commit now rather than with the next frame. Behind a window the surface gets no frame
            // callbacks, so a frame may never come to carry the change.
            layer.wl_surface().commit();
            self.behind = behind;
        }
    }

    /// Whether a frame is due: the view is configured and can be seen, its screen may have changed
    /// (`stale`) or the compositor waits for a frame, and its last frame has been shown.
    pub(crate) fn due(&self, stale: bool) -> bool {
        self.configured && self.visible && (stale || self.reconfigured) && !self.frame_pending
    }

    /// Has the core record a frame of the screen and compare it with the last one drawn. If it changed,
    /// what changed is drawn straight into a free shm buffer, which is committed with a frame callback
    /// asked for. If the compositor is still reading every buffer, nothing is recorded. The view then
    /// stays due, and a buffer's release wakes the loop to draw again. Returns whether it recorded.
    ///
    /// The buffer is ABGR8888, which is tiny-skia's byte order (premultiplied RGBA in memory), so it is
    /// drawn into directly. A buffer drawn a few frames ago also gets what changed since then. The
    /// compositor is told the rectangle that changed since the last frame.
    pub(crate) fn draw(&mut self, core: &mut Core, pool: &mut SlotPool, qh: &QueueHandle<App>) -> Result<bool, Error> {
        let (size, scale) = core.size(self.screen);
        let size_px = (size.0 * scale, size.1 * scale);
        let (w, h) = (i32::try_from(size_px.0)?, i32::try_from(size_px.1)?);
        let Some(index) = free_buffer(&mut self.buffers, pool, w, h)? else {
            return Ok(false);
        };
        // A configure is answered with a whole frame.
        let Some(damage) = core.record(self.screen, self.reconfigured) else {
            return Ok(true);
        };
        let Some((buffer, held)) = self.buffers.get_mut(index) else {
            return Err("buffer index".into());
        };
        let Some(bytes) = buffer.canvas(pool) else {
            return Err("buffer busy".into());
        };
        let len = size_px.0 as usize * size_px.1 as usize * 4;
        let pixmap = bytes.get_mut(..len).and_then(|b| PixmapMut::from_bytes(b, size_px.0, size_px.1)).ok_or("buffer size")?;
        *held = core.draw(self.screen, pixmap, *held, damage);
        let surface = self.shell.surface();
        surface.set_buffer_scale(i32::try_from(scale)?);
        surface.damage_buffer(damage.x0, damage.y0, damage.width(), damage.height());
        surface.frame(qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface)?;
        surface.commit();
        self.reconfigured = false;
        self.frame_pending = true;
        Ok(true)
    }
}

/// A `w` x `h` buffer the compositor isn't reading. It is a kept one, or a new one if fewer than three
/// are kept. Returns `None` when all three are busy.
fn free_buffer(buffers: &mut Vec<(Buffer, u64)>, pool: &mut SlotPool, w: i32, h: i32) -> Result<Option<usize>, Error> {
    buffers.retain(|(b, _)| b.height() == h && b.stride() == w * 4);
    if let Some(i) = buffers.iter().position(|(b, _)| b.canvas(pool).is_some()) {
        return Ok(Some(i));
    }
    if buffers.len() >= 3 {
        return Ok(None);
    }
    let (buffer, _) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Abgr8888)?;
    buffers.push((buffer, 0));
    Ok(Some(buffers.len() - 1))
}
