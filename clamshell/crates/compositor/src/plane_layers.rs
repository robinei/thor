//! A screen's overlay layer (the HUD, notices, the window arrows) and the cursor on display planes of their
//! own, with CLAMSHELL_SCANOUT=1: blended over whatever the screen's primary plane shows (ClamShell's own
//! buffer, a window's, or a frame composed) by the display engine, so that neither has the screen composed
//! nor redrawn when it changes. The overlay is drawn by its painter into buffers of its own, turned to the
//! panel's mounting, as `ui_scanout` draws a screen; the cursor's image is copied into a small square buffer
//! when it changes, and moving it only moves its plane.
//!
//! While a layer has its plane it isn't composed at all: the scene leaves it out, and a screen showing a
//! window alone with the HUD or the cursor over it still shows the window directly. A cursor the CPU can't
//! copy (see `cursor::Cursor::shape`) or too large for its plane is composed. The planes are the display
//! engine's overlay and cursor planes for the screen's CRTC, each used by one screen, in a format with
//! alpha. The layers aren't colour corrected on their planes but by the CRTC's stages where those are on
//! (ClamShell's own buffers shown): grey text on black and the cursor show the same either way.
//!
//! A commit the display engine refuses with them switches them off for the screen, logged: they are
//! composed from then on. A virtual machine's GPU shows its cursor plane only to a client that gives the
//! cursor's hotspot with it (`DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`, for the host to draw its own pointer
//! there): ClamShell does, so its cursor is on a plane there too.

use smithay::backend::drm::{DrmDevice, DrmDeviceFd, PlaneConfig, PlaneState};
use smithay::reexports::drm::control::{RawResourceHandle, crtc, framebuffer, plane, property};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Transform};

use clamshell_ui::{Bounds, Error, Layer, Screen};

use crate::cursor::{Cursor, Shape};
use crate::scanout::Property;
use crate::ui_scanout::{BLENDED, LayerBuffers};

/// Whether each layer goes on its plane this turn.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Planed {
    pub overlay: bool,
    pub cursor: bool,
}

/// Where a plane shows: which framebuffer, which part of it, where on the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    fb: framebuffer::Handle,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
}

/// One of the display engine's planes: where it showed in the last commit, and where in the next.
struct Plane {
    handle: plane::Handle,
    /// Its zpos set above the primary plane's, where its property allows.
    zpos: Option<Property>,
    /// Its hotspot properties, a virtual machine's cursor plane's.
    hotspot: Option<(property::Handle, property::Handle)>,
    /// The properties of its next commit: its zpos and hotspot.
    properties: Vec<Property>,
    shown: Option<Placement>,
    wanted: Option<Placement>,
    /// Whether the next commit shows a buffer newly drawn, so that the other is drawn after it.
    drew: bool,
}

impl Plane {
    /// Its next commit's properties with the hotspot at `(x, y)` in its buffer, where it has one.
    fn set_hotspot(&mut self, (x, y): (i32, i32)) {
        self.properties.clear();
        self.properties.extend(self.zpos);
        if let Some((hx, hy)) = self.hotspot {
            let object = RawResourceHandle::from(self.handle);
            // Signed 32-bit values, as the property's range has them.
            self.properties.push((object, hx, u64::from(x as u32)));
            self.properties.push((object, hy, u64::from(y as u32)));
        }
    }

    /// Its state in the next commit: shown where wanted, or off if it showed before; none if neither.
    fn state(&self) -> Option<PlaneState<'_>> {
        let config = self.wanted.map(|placement| PlaneConfig {
            src: placement.src,
            dst: placement.dst,
            transform: Transform::Normal,
            alpha: 1.0,
            damage_clips: None,
            fb: placement.fb,
            fence: None,
            properties: &self.properties,
        });
        (config.is_some() || self.shown.is_some()).then_some(PlaneState { handle: self.handle, config })
    }
}

/// A screen's overlay and cursor planes.
pub(crate) struct PlaneLayers {
    overlay: Option<(Plane, LayerBuffers)>,
    cursor: Option<CursorPlane>,
    /// Whether each goes on its plane this turn (see [`PlaneLayers::decide`]).
    pub planed: Planed,
    /// The cursor's shape decided this turn, and the overlay's area, as last placed.
    shape: Option<Shape>,
    area: Option<Bounds>,
    /// The display engine refused a commit with them: off from now on.
    failed: bool,
    /// The screen's height, the screen's way up, and whether its panel is turned a quarter.
    height: i32,
    quarter: bool,
}

struct CursorPlane {
    plane: Plane,
    buffers: LayerBuffers,
    /// The square buffer's side.
    side: u32,
    /// The shape the buffer shown holds.
    shown: Option<Shape>,
}

/// Asks for a virtual machine's cursor planes, which its GPU hides from a client that gives no hotspot:
/// before Smithay opens the device, which knows only the planes there are then. The kernel takes it from an
/// atomic client only, so atomic is asked for first (Smithay asks for it again).
pub(crate) fn ask_for_cursor_planes(device: &DrmDeviceFd) {
    let fd = std::os::fd::AsFd::as_fd(device);
    let asked = drm_ffi::set_capability(fd, u64::from(drm_ffi::DRM_CLIENT_CAP_ATOMIC), true)
        .and_then(|_| drm_ffi::set_capability(fd, u64::from(drm_ffi::DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT), true));
    if let Err(error) = asked {
        log::debug!("plane layers: no cursor hotspots ({error}): not a virtual machine's GPU");
    }
}

impl PlaneLayers {
    /// The planes for the screen on `crtc`, of `size` the screen's way up (`turned` a quarter clockwise as
    /// its panel is mounted), from those not `taken` by another screen, which are then taken; logged.
    pub(crate) fn new(drm: &DrmDevice, crtc: crtc::Handle, size: (u32, u32), quarter: bool, taken: &mut Vec<plane::Handle>, role: &str) -> PlaneLayers {
        let device = drm.device_fd();
        let planes = match drm.planes(&crtc) {
            Ok(planes) => planes,
            Err(error) => {
                log::warn!("plane layers: the {role} screen's planes: {error}");
                return PlaneLayers::off(size, quarter);
            }
        };
        let primary_zpos = planes.primary.first().and_then(|plane| plane.zpos);
        let free = |info: &&smithay::backend::drm::PlaneInfo, taken: &[plane::Handle]| !taken.contains(&info.handle);
        let mut overlay = None;
        for info in planes.overlay.iter().filter(|info| free(info, taken)) {
            match LayerBuffers::new(device, &info.formats, size, quarter, BLENDED) {
                Ok(buffers) => {
                    taken.push(info.handle);
                    overlay = Some((plane(device, info, primary_zpos, 1), buffers));
                    break;
                }
                Err(error) => log::info!("plane layers: plane {:?} for the {role} screen's overlay: {error}", info.handle),
            }
        }
        use smithay::reexports::drm::Device as _;
        let cap = |c| drm.get_driver_capability(c).ok().and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0).unwrap_or(64);
        let side = cap(smithay::reexports::drm::DriverCapability::CursorWidth).max(cap(smithay::reexports::drm::DriverCapability::CursorHeight));
        let mut cursor = None;
        for info in planes.cursor.iter().chain(&planes.overlay).filter(|info| free(info, taken)) {
            match LayerBuffers::new(device, &info.formats, (side, side), quarter, BLENDED) {
                Ok(buffers) => {
                    taken.push(info.handle);
                    cursor = Some(CursorPlane { plane: plane(device, info, primary_zpos, 2), buffers, side, shown: None });
                    break;
                }
                Err(error) => log::info!("plane layers: plane {:?} for the {role} screen's cursor: {error}", info.handle),
            }
        }
        let name = |plane: Option<&Plane>| plane.map_or_else(|| "none (composed)".to_owned(), |plane| format!("plane {:?}", plane.handle));
        log::info!(
            "plane layers: the {role} screen's overlay on {}, its cursor on {}",
            name(overlay.as_ref().map(|(plane, _)| plane)),
            name(cursor.as_ref().map(|cursor| &cursor.plane))
        );
        PlaneLayers { overlay, cursor, ..PlaneLayers::off(size, quarter) }
    }

    fn off((_, height): (u32, u32), quarter: bool) -> PlaneLayers {
        PlaneLayers {
            overlay: None,
            cursor: None,
            planed: Planed::default(),
            shape: None,
            area: None,
            failed: false,
            height: i32::try_from(height).unwrap_or(0),
            quarter,
        }
    }

    /// Decides which layers go on their planes this turn: the overlay while it shows anything, the cursor,
    /// with its hotspot `at` on this screen whose top left is `origin` in the scene, while it shows and its
    /// plane can take it. Returns whether what they show changed, for the screen to commit.
    pub(crate) fn decide(
        &mut self,
        server: &mut crate::server::Server,
        screen: Screen,
        cursor: &mut Cursor,
        renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
        at: Option<Point<f64, Logical>>,
        origin: Point<i32, Logical>,
    ) -> bool {
        let was = self.planed;
        let area = server.core.overlay_area(screen);
        self.planed.overlay = !self.failed && self.overlay.is_some() && area.is_some();
        let overlay_changed = self.planed.overlay
            && (area != self.area || self.overlay.as_ref().is_some_and(|(_, buffers)| buffers.behind(&server.core, Layer::Overlay(screen))));
        let shape = at.and_then(|at| cursor.shape(server, renderer, at)).map(|shape| shape.from(origin));
        let fits = |side: u32| shape.as_ref().is_some_and(|shape| shape.size.0 <= side && shape.size.1 <= side);
        self.planed.cursor = !self.failed && self.cursor.as_ref().is_some_and(|cursor| fits(cursor.side));
        let cursor_changed = self.planed.cursor && shape != self.cursor.as_ref().and_then(|cursor| cursor.shown.clone());
        self.shape = shape.filter(|_| self.planed.cursor);
        self.planed != was || overlay_changed || cursor_changed
    }

    /// Readies the planes for the next commit: the overlay drawn into its next buffer if behind, the cursor
    /// copied into its next if its image changed, each placed; those not planed this turn (the screen
    /// composes them) off.
    pub(crate) fn prepare(&mut self, server: &mut crate::server::Server, screen: Screen, cursor: &Cursor) {
        let (height, quarter) = (self.height, self.quarter);
        let area = server.core.overlay_area(screen);
        if let Some((plane, buffers)) = &mut self.overlay {
            plane.drew = false;
            plane.wanted = None;
            if self.planed.overlay
                && let Some(area) = area
            {
                let fb = match buffers.draw(&mut server.core, Layer::Overlay(screen)) {
                    Some(fb) => {
                        server.core.frames_done();
                        plane.drew = true;
                        Some(fb)
                    }
                    None => plane.shown.map(|shown| shown.fb),
                };
                if let Some(fb) = fb {
                    let rect = turned(Rectangle::new((area.x0, area.y0).into(), (area.x1 - area.x0, area.y1 - area.y0).into()), height, quarter);
                    let src = Rectangle::new((f64::from(rect.loc.x), f64::from(rect.loc.y)).into(), (f64::from(rect.size.w), f64::from(rect.size.h)).into());
                    plane.wanted = Some(Placement { fb, src, dst: rect });
                }
                self.area = Some(area);
            }
        }
        if let Some(plane) = &mut self.cursor {
            plane.plane.drew = false;
            plane.plane.wanted = None;
            if self.planed.cursor
                && let Some(shape) = &self.shape
            {
                let same_image = plane.shown.as_ref().is_some_and(|shown| shown.same_image(shape));
                let side = plane.side;
                let fb = match plane.plane.shown {
                    Some(shown) if same_image => Some(shown.fb),
                    _ => {
                        plane.plane.drew = true;
                        plane.buffers.write(|bytes, stride, quarter, bgra| cursor.copy(server, shape, bytes, stride, quarter, bgra, side))
                    }
                };
                if let Some(fb) = fb {
                    let square = i32::try_from(side).unwrap_or(0);
                    let dst = turned(Rectangle::new((shape.at.x, shape.at.y).into(), (square, square).into()), height, quarter);
                    let src = Rectangle::from_size((f64::from(side), f64::from(side)).into());
                    plane.plane.wanted = Some(Placement { fb, src, dst });
                    // The hotspot in the buffer, turned as the image is.
                    let (x, y) = shape.hotspot;
                    let hotspot = if quarter { (square - 1 - y, x) } else { (x, y) };
                    plane.plane.set_hotspot(hotspot);
                }
            }
        }
    }

    /// The planes' states for the next commit.
    pub(crate) fn states(&self) -> impl Iterator<Item = PlaneState<'_>> + Clone {
        let overlay = self.overlay.as_ref().and_then(|(plane, _)| plane.state());
        let cursor = self.cursor.as_ref().and_then(|cursor| cursor.plane.state());
        overlay.into_iter().chain(cursor)
    }

    /// The commit with them went through: they show as placed.
    pub(crate) fn committed(&mut self) {
        if let Some((plane, buffers)) = &mut self.overlay {
            if plane.drew {
                buffers.committed();
            }
            plane.shown = plane.wanted;
        }
        if let Some(cursor) = &mut self.cursor {
            if cursor.plane.drew {
                cursor.buffers.committed();
            }
            cursor.plane.shown = cursor.plane.wanted;
            cursor.shown = cursor.plane.wanted.and(self.shape.clone());
        }
    }

    /// The display engine refused a commit with them: off from now on, logged; the screen composes them.
    pub(crate) fn fail(&mut self, error: &Error, role: &str) {
        if !self.failed {
            log::warn!("plane layers: the {role} screen's commit with its overlay and cursor planes failed ({error}): they are composed from now on");
        }
        self.failed = true;
        self.planed = Planed::default();
    }

    /// Their buffers hold nothing any more (the screen was away): drawn whole next, and shown as if never.
    pub(crate) fn reset(&mut self) {
        if let Some((plane, buffers)) = &mut self.overlay {
            buffers.reset();
            plane.shown = None;
        }
        if let Some(cursor) = &mut self.cursor {
            cursor.plane.shown = None;
            cursor.shown = None;
        }
    }
}

/// A plane for a layer, its zpos the primary plane's (`primary`) and `above` it, where its property allows.
fn plane(device: &DrmDeviceFd, info: &smithay::backend::drm::PlaneInfo, primary: Option<i32>, above: i32) -> Plane {
    let props = crate::scanout::properties_of(device, u32::from(info.handle), drm_ffi::DRM_MODE_OBJECT_PLANE);
    let find = |name: &str| props.iter().find(|(n, ..)| n == name).map(|(_, handle, _)| *handle);
    let object = RawResourceHandle::from(info.handle);
    let zpos = match (primary, info.zpos, find("zpos")) {
        (Some(primary), Some(current), Some(handle)) if current <= primary => Some((object, handle, u64::try_from(primary + above).unwrap_or(0))),
        _ => None,
    };
    let hotspot = find("HOTSPOT_X").zip(find("HOTSPOT_Y"));
    Plane { handle: info.handle, zpos, hotspot, properties: zpos.into_iter().collect(), shown: None, wanted: None, drew: false }
}

/// `rect`, the screen's way up on a screen `height` high, as the panel takes it: turned a quarter clockwise
/// if `quarter` (a screen pixel (x, y) in buffer column height - 1 - y, row x, as `clamshell_ui::Pixels`).
fn turned(rect: Rectangle<i32, Logical>, height: i32, quarter: bool) -> Rectangle<i32, Physical> {
    let (x, y, w, h) = (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);
    if quarter { Rectangle::new((height - (y + h), x).into(), (h, w).into()) } else { Rectangle::new((x, y).into(), (w, h).into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_turned_a_quarter_is_where_the_painter_puts_it() {
        let rect = Rectangle::<i32, Logical>::new((10, 20).into(), (30, 5).into());
        assert_eq!(turned(rect, 100, false), Rectangle::new((10, 20).into(), (30, 5).into()));
        // Rows 20..25 of a screen 100 high are buffer columns 75..80; columns 10..40 are rows 10..40.
        assert_eq!(turned(rect, 100, true), Rectangle::new((75, 10).into(), (5, 30).into()));
    }
}
