//! A window shown directly, with CLAMSHELL_SCANOUT=1: its buffer on the screen's primary plane rather than
//! composed, so that a fullscreen game costs the GPU nothing but its own drawing. Best effort: a screen
//! shows a window so while it is the only thing on it ([`Server::scanout_window`]) and its buffer is one
//! the plane takes as ClamShell would show it, and the display engine passes the commit tested first;
//! otherwise it is composed, and a window refused stays composed until its buffer changes.
//!
//! The window's colour is what the final pass would make of it (`colour::scanout`): none where its
//! signal is the screen's and no curve is on, else the plane's colour pipeline, a 1D LUT then a 3D LUT,
//! where it has one. Nothing of what the picture settings do before the final pass (anti-aliasing,
//! sharpening, FSR, nearest-neighbour scaling) is done on a plane: a window with any of them on is
//! composed.
//!
//! The window is told, by dmabuf feedback, which formats the plane takes while it is alone on the screen,
//! so that its next buffers may be ones the plane takes (on the Thor, the UBWC the display engine turns).

use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::Modifier;
use smithay::backend::allocator::dmabuf::{Dmabuf, WeakDmabuf};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::drm::gbm::{GbmFramebuffer, framebuffer_from_dmabuf};
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::utils::{Buffer, CommitCounter, with_renderer_surface_state};
use smithay::desktop::PopupManager;
use smithay::reexports::drm::control::{RawResourceHandle, framebuffer, plane, property};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle, Transform};
use smithay::wayland::dmabuf::{DmabufFeedback, SurfaceDmabufFeedbackState, get_dmabuf};

use clamshell_state::colour::ColourDescription;
use clamshell_ui::Error;

use crate::colour::render::{curve_uniforms, hdr_peak};
use crate::colour::scanout::{Transform as ColourTransform, blob};
use crate::scanout::{Blobs, Property, enum_name, properties_of};
use crate::scene::Shown;
use crate::server::Server;

/// A window a screen could show from its own buffer: which part of the buffer, where, and its colour.
pub(crate) struct Candidate {
    pub id: i64,
    pub surface: WlSurface,
    pub buffer: Buffer,
    pub dmabuf: Dmabuf,
    /// The part of the buffer shown, in its pixels.
    pub src: Rectangle<f64, smithay::utils::Buffer>,
    /// Where it is shown, in the screen's pixels the right way up.
    pub dst: Rectangle<i32, Logical>,
    /// What the plane makes of its colour, if anything.
    pub colour: Option<ColourKey>,
    pub commit: CommitCounter,
}

/// What decides a window's colour on a plane, compared to know when its LUTs are to be made again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColourKey {
    surface: ColourDescription,
    curve: [[f32; 4]; 2],
    output: ColourDescription,
}

/// What a test commit decides on: a refusal holds until one of them changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shape {
    id: i64,
    format: smithay::backend::allocator::Format,
    size: (i32, i32),
    src: Rectangle<f64, smithay::utils::Buffer>,
    dst: Rectangle<i32, Logical>,
    colour: Option<ColourKey>,
}

impl Candidate {
    pub(crate) fn shape(&self) -> Shape {
        let size = self.dmabuf.size();
        Shape { id: self.id, format: self.dmabuf.format(), size: (size.w, size.h), src: self.src, dst: self.dst, colour: self.colour }
    }
}

impl Server {
    /// The window screen `down` could show from its own buffer, `screen` being its area in the scene and
    /// `output` its encoding: none unless one window is all there is on the screen and covers it (see
    /// [`Server::lone_window`]), else that window and whether its buffer can be shown so, or why not.
    pub(crate) fn scanout_window(
        &self,
        down: bool,
        screen: Rectangle<i32, Logical>,
        output: ColourDescription,
    ) -> Option<(i64, Result<Candidate, &'static str>)> {
        let placed = self.lone_window(down, screen)?;
        Some((placed.id, self.candidate(placed.id, placed.geometry, down, screen, output)))
    }

    /// The window that is all there is on screen `down`, `screen` being its area in the scene, covering it
    /// and taking the input: no other window, dim layer, UI layer or cursor on it.
    pub(crate) fn lone_window(&self, down: bool, screen: Rectangle<i32, Logical>) -> Option<&crate::scene::Placed> {
        let ui = if down { clamshell_ui::Screen::Bottom } else { clamshell_ui::Screen::Top };
        let mut on = self.scene.shown().iter().filter(|shown| match shown {
            Shown::Window(placed) => placed.below == down,
            Shown::Software { layer, .. } => matches!(layer, clamshell_ui::Layer::Screen(s) | clamshell_ui::Layer::Overlay(s) if *s == ui),
            Shown::Dim { area } => area.overlaps(screen),
            Shown::Cursor { at } => screen.to_f64().contains(*at),
        });
        let Some(Shown::Window(placed)) = on.next() else { return None };
        (on.next().is_none() && placed.input && placed.geometry.viewport == screen).then_some(placed)
    }

    fn candidate(
        &self,
        id: i64,
        geometry: crate::geometry::Geometry,
        down: bool,
        screen: Rectangle<i32, Logical>,
        output: ColourDescription,
    ) -> Result<Candidate, &'static str> {
        let client = self.windows.iter().find(|client| client.id == id).ok_or("no such window")?;
        let root = client.window.root().ok_or("no surface")?;
        if PopupManager::popups_for_surface(&root).next().is_some() || !client.window.x11_popups().is_empty() {
            return Err("a popup on it");
        }
        let mut drawn = 0;
        client.window.with_surfaces(|_, states| drawn += usize::from(crate::surface::Surface::view(states).is_some()));
        if drawn != 1 {
            return Err("more than one of its surfaces drawn");
        }
        let surface = client.window.picture_surface().ok_or("no picture")?;
        let (_, place) = client.window.picture().ok_or("no picture")?;
        let (buffer, view, commit, transform, scale) = with_renderer_surface_state(&surface, |state| {
            (state.buffer().cloned(), state.view(), state.current_commit(), state.buffer_transform(), state.buffer_scale())
        })
        .ok_or("no buffer")?;
        let (buffer, view) = (buffer.ok_or("no buffer")?, view.ok_or("no buffer")?);
        if transform != Transform::Normal || scale != 1 {
            return Err("a buffer turned or scaled by its client");
        }
        let dmabuf = get_dmabuf(&buffer).map_err(|_| "a shared-memory buffer")?.clone();
        if dmabuf.y_inverted() {
            return Err("an upside-down buffer");
        }
        let at = |v: i32, scale: f64, origin: f64| (origin + f64::from(v) * scale).round() as i32;
        let (x0, y0) = (at(place.loc.x, geometry.scale.x, geometry.origin.x), at(place.loc.y, geometry.scale.y, geometry.origin.y));
        let (x1, y1) =
            (at(place.loc.x + place.size.w, geometry.scale.x, geometry.origin.x), at(place.loc.y + place.size.h, geometry.scale.y, geometry.origin.y));
        let dst = Rectangle::<i32, Logical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into());
        if !screen.contains_rect(dst) {
            return Err("partly off the screen");
        }
        let clarity = client.shown.clarity;
        let one_to_one = (view.src.size.w - f64::from(dst.size.w)).abs() < 0.5 && (view.src.size.h - f64::from(dst.size.h)).abs() < 0.5;
        if clarity.anti_aliasing != clamshell_state::settings::AntiAliasing::Off || clarity.sharpen > 0 || (clarity.integer && !one_to_one) {
            return Err("anti-aliasing, sharpening or nearest-neighbour scaling on");
        }
        if self.core.state().histogram_window() == Some(id) {
            return Err("its curve is being edited");
        }
        let colour = self.colour.surfaces.get(&Id::from_wayland_resource(&surface)).map(|colour| **colour).unwrap_or_default();
        let headroom = self.colour.screen(down).headroom() as f32;
        let curve = curve_uniforms(client.shown.curve.as_ref(), headroom, hdr_peak(colour, client.shown.hdr_peak));
        let plain = colour.matches_output(output) && curve[1][3] <= 0.5;
        let src = Rectangle::new((view.src.loc.x, view.src.loc.y).into(), (view.src.size.w, view.src.size.h).into());
        Ok(Candidate {
            id,
            surface,
            buffer,
            dmabuf,
            src,
            dst: Rectangle::new(dst.loc - screen.loc, dst.size),
            colour: (!plain).then_some(ColourKey { surface: colour, curve, output }),
            commit,
        })
    }

    /// Window `id`'s surfaces get `feedback` for their buffers (those that asked for theirs).
    pub(crate) fn dmabuf_feedback(&self, id: i64, feedback: &DmabufFeedback) {
        if let Some(client) = self.windows.iter().find(|client| client.id == id) {
            client.window.with_surfaces(|_, states| {
                if let Some(state) = SurfaceDmabufFeedbackState::from_states(states) {
                    state.set_feedback(feedback);
                }
            });
        }
    }
}

/// A screen's means to show a window from its own buffer: the framebuffers of the clients' buffers it
/// showed lately, and its primary plane's colour pipeline.
pub(crate) struct WindowScanout {
    device: DrmDeviceFd,
    gbm: GbmDevice<DrmDeviceFd>,
    /// The formats the plane takes.
    formats: FormatSet,
    pipeline: Option<PlanePipeline>,
    /// The window's feedback while it is alone on the screen, the plane's formats preferred: none without
    /// a render device to name.
    pub feedback: Option<DmabufFeedback>,
    /// The framebuffers of buffers shown, each with its buffer, kept while it lives: a game draws into a
    /// few in turn.
    framebuffers: Vec<(WeakDmabuf, GbmFramebuffer)>,
    /// The LUTs made last, for a window's colour.
    colour: Option<(ColourKey, Blobs, Vec<Property>)>,
    /// The pipeline turned off, for a window that needs none and the commits after a window.
    off: Vec<Property>,
    /// The last shape refused, until a window or its buffer changes it.
    pub refused: Option<Shape>,
}

/// How many framebuffers are kept: more buffers than a client draws into in turn.
const FRAMEBUFFERS: usize = 4;

impl WindowScanout {
    /// For a screen whose primary plane is `plane`, taking `formats`, with `feedback` for a window alone on
    /// it.
    pub(crate) fn new(
        device: &DrmDeviceFd,
        gbm: &GbmDevice<DrmDeviceFd>,
        plane: plane::Handle,
        formats: FormatSet,
        feedback: Option<DmabufFeedback>,
    ) -> WindowScanout {
        let pipeline = PlanePipeline::find(device, plane);
        let off = pipeline.as_ref().map(|p| vec![(RawResourceHandle::from(plane), p.property, 0)]).unwrap_or_default();
        WindowScanout {
            device: device.clone(),
            gbm: gbm.clone(),
            formats,
            pipeline,
            feedback,
            framebuffers: Vec::with_capacity(FRAMEBUFFERS),
            colour: None,
            off,
            refused: None,
        }
    }

    /// Whether the plane has a colour pipeline it can show a window's colour with.
    pub(crate) fn has_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    /// The framebuffer of `dmabuf`, made the first time and kept with it.
    pub(crate) fn framebuffer(&mut self, dmabuf: &Dmabuf) -> Result<framebuffer::Handle, Error> {
        self.framebuffers.retain(|(weak, _)| weak.upgrade().is_some());
        if let Some((_, fb)) = self.framebuffers.iter().find(|(weak, _)| weak.upgrade().as_ref() == Some(dmabuf)) {
            return Ok(*fb.as_ref());
        }
        let format = dmabuf.format();
        let code = format.code;
        let opaque = !self.formats.iter().any(|f| f.code == code);
        let taken = |code| self.formats.iter().any(|f| f.code == code && (f.modifier == format.modifier || f.modifier == Modifier::Invalid));
        if !taken(code) && !smithay::backend::allocator::format::get_opaque(code).is_some_and(taken) {
            return Err(format!("the plane doesn't take its format, {code} {:?}", format.modifier).into());
        }
        let fb = framebuffer_from_dmabuf(&self.device, &self.gbm, dmabuf, opaque, false).map_err(|e| format!("its framebuffer: {e}"))?;
        let handle = *fb.as_ref();
        if self.framebuffers.len() == FRAMEBUFFERS {
            self.framebuffers.remove(0);
        }
        self.framebuffers.push((dmabuf.weak(), fb));
        Ok(handle)
    }

    /// The plane's properties for a window's colour `key`: the pipeline off for none, else its LUTs, made
    /// when the key changes.
    pub(crate) fn colour(&mut self, key: Option<ColourKey>) -> Result<&[Property], Error> {
        let Some(key) = key else { return Ok(&self.off) };
        let pipeline = self.pipeline.as_ref().ok_or("the plane has no colour pipeline")?;
        if self.colour.as_ref().is_none_or(|(made, ..)| *made != key) {
            self.colour = None;
            let transform = ColourTransform::new(key.surface, key.curve, key.output).ok_or("a colour the final pass can't draw")?;
            let mut blobs = Blobs::new(&self.device);
            let lut1d = blobs.create(blob(&transform.lut1d(pipeline.lut1d.size)))?;
            let lut3d = blobs.create(blob(&transform.lut3d(pipeline.lut3d.size)))?;
            self.colour = Some((key, blobs, pipeline.properties(lut1d, lut3d)));
        }
        Ok(self.colour.as_ref().map_or(&[][..], |(_, _, set)| set))
    }

    /// The plane's properties once it no longer shows a window: its pipeline off.
    pub(crate) fn clear(&self) -> &[Property] {
        &self.off
    }
}

/// One of a pipeline's operations: its colorop, its BYPASS and DATA properties, and its size (a 1D LUT's
/// entries, a 3D LUT's nodes along an axis).
struct Op {
    id: RawResourceHandle,
    bypass: property::Handle,
    data: property::Handle,
    size: usize,
}

/// A plane's colour pipeline that can show a window's colour: one 1D LUT, then one 3D LUT, every other
/// operation bypassed.
struct PlanePipeline {
    plane: RawResourceHandle,
    /// COLOR_PIPELINE, and this pipeline's value of it (its first colorop).
    property: property::Handle,
    first: u64,
    lut1d: Op,
    lut3d: Op,
    /// The other operations' colorops and BYPASS properties.
    others: Vec<(RawResourceHandle, property::Handle)>,
}

impl PlanePipeline {
    /// The first of `plane`'s pipelines with a 1D LUT and then a 3D LUT and nothing else that can't be
    /// bypassed; none, logged, if it has none.
    fn find(device: &DrmDeviceFd, plane: plane::Handle) -> Option<PlanePipeline> {
        let id = u32::from(plane);
        let props = properties_of(device, id, drm_ffi::DRM_MODE_OBJECT_PLANE);
        let Some((_, property, _)) = props.iter().find(|(name, ..)| name == "COLOR_PIPELINE") else {
            log::info!("window scanout: plane {id} has no colour pipeline: only windows needing no colour change are shown from their own buffers");
            return None;
        };
        let firsts = crate::scanout::enum_values(device, *property);
        let found = firsts.iter().find_map(|first| Self::of(device, plane, *property, *first));
        if found.is_none() {
            log::info!("window scanout: none of plane {id}'s colour pipelines {firsts:?} is a 1D LUT then a 3D LUT");
        }
        found
    }

    fn of(device: &DrmDeviceFd, plane: plane::Handle, property: property::Handle, first: u64) -> Option<PlanePipeline> {
        let (mut lut1d, mut lut3d, mut others) = (None, None, Vec::new());
        let mut at = u32::try_from(first).ok()?;
        // A bound, so that a cycle can't loop.
        for _ in 0..16 {
            if at == 0 {
                break;
            }
            let props = properties_of(device, at, drm_ffi::DRM_MODE_OBJECT_COLOROP);
            let find = |name: &str| props.iter().find(|(n, ..)| n == name).map(|(_, handle, value)| (*handle, *value));
            let kind = find("TYPE").and_then(|(handle, value)| enum_name(device, handle, value))?;
            let id: RawResourceHandle = smithay::reexports::drm::control::from_u32(at)?;
            let bypass = find("BYPASS")?.0;
            match kind.as_str() {
                "1D LUT" if lut1d.is_none() && lut3d.is_none() => {
                    lut1d = Some(Op { id, bypass, data: find("DATA")?.0, size: usize::try_from(find("SIZE")?.1).ok()? });
                }
                "3D LUT" if lut1d.is_some() && lut3d.is_none() => {
                    lut3d = Some(Op { id, bypass, data: find("DATA")?.0, size: usize::try_from(find("SIZE")?.1).ok()? });
                }
                _ => others.push((id, bypass)),
            }
            at = find("NEXT").map_or(0, |(_, next)| next as u32);
        }
        Some(PlanePipeline { plane: plane.into(), property, first, lut1d: lut1d?, lut3d: lut3d?, others })
    }

    /// The properties that show a window through the pipeline with LUT blobs `lut1d` and `lut3d`.
    fn properties(&self, lut1d: u32, lut3d: u32) -> Vec<Property> {
        let mut set = vec![(self.plane, self.property, self.first)];
        for (op, blob) in [(&self.lut1d, lut1d), (&self.lut3d, lut3d)] {
            set.push((op.id, op.bypass, 0));
            set.push((op.id, op.data, u64::from(blob)));
        }
        set.extend(self.others.iter().map(|(id, bypass)| (*id, *bypass, 1)));
        set
    }
}
