//! Direct scanout, on with CLAMSHELL_SCANOUT=1: a fullscreen window shown by a display plane rather than
//! composed, its colour by the plane's colour pipeline (see `colour::scanout`). Best effort: it is written
//! before it could be tried on the Thor, whose display engine and Armada's colour patches decide what
//! passes (`docs/history/compositor-mvp.md`, phase 4's direct scanout).
//!
//! At start, what the display engine offers is logged ([`report`]): each plane either screen can
//! use, its properties (its type, rotations, colour pipelines) and formats with their modifiers; each
//! colour pipeline's operations; each screen's CRTC's colour properties. The plane colour pipeline
//! capability is set first, without which the kernel hides the pipelines.

use std::os::fd::AsFd;

use drm_ffi::{DRM_CLIENT_CAP_PLANE_COLOR_PIPELINE, DRM_MODE_OBJECT_COLOROP, DRM_MODE_OBJECT_CRTC, DRM_MODE_OBJECT_PLANE};
use smithay::backend::drm::DrmDevice;
use smithay::reexports::drm::control::crtc;

/// A property's flags (drm_mode.h's `DRM_MODE_PROP_*`).
const RANGE: u32 = 1 << 1;
const ENUM: u32 = 1 << 3;
const BLOB: u32 = 1 << 4;
const BITMASK: u32 = 1 << 5;
const OBJECT: u32 = 1 << 6;

/// Asks for the plane colour pipelines, and logs what the display engine offers to `screens`, by their
/// names.
pub(crate) fn report(device: &DrmDevice, screens: &[(&str, crtc::Handle)]) {
    let fd = device.as_fd();
    if let Err(e) = drm_ffi::set_capability(fd, u64::from(DRM_CLIENT_CAP_PLANE_COLOR_PIPELINE), true) {
        log::warn!("scanout: no plane colour pipelines (the capability: {e}): a scanned-out window would show uncorrected");
    }
    let mut seen = Vec::new();
    for (screen, crtc) in screens {
        let planes = match device.planes(crtc) {
            Ok(planes) => planes,
            Err(e) => {
                log::warn!("scanout: {screen}'s planes: {e}");
                continue;
            }
        };
        for plane in planes.primary.iter().chain(&planes.overlay).chain(&planes.cursor) {
            if seen.contains(&plane.handle) {
                continue;
            }
            seen.push(plane.handle);
            let users: Vec<&str> = screens
                .iter()
                .filter(|(_, c)| device.planes(c).is_ok_and(|p| p.primary.iter().chain(&p.overlay).chain(&p.cursor).any(|i| i.handle == plane.handle)))
                .map(|(name, _)| *name)
                .collect();
            let id = u32::from(plane.handle);
            let mut formats: Vec<(String, String)> = plane.formats.iter().map(|f| (f.code.to_string(), format!("{:?}", f.modifier))).collect();
            formats.sort();
            let mut by_code: Vec<(String, Vec<String>)> = Vec::new();
            for (code, modifier) in formats {
                match by_code.last_mut() {
                    Some((last, modifiers)) if *last == code => modifiers.push(modifier),
                    _ => by_code.push((code, vec![modifier])),
                }
            }
            let formats: Vec<String> = by_code.iter().map(|(code, modifiers)| format!("{code} [{}]", modifiers.join(", "))).collect();
            log::info!(
                "scanout: plane {id} ({:?}) for {users:?}: {}; formats {}",
                plane.type_,
                properties(device, id, DRM_MODE_OBJECT_PLANE).join(", "),
                formats.join(", ")
            );
            report_pipelines(device, id);
        }
    }
    for (screen, crtc) in screens {
        log::info!("scanout: {screen}'s CRTC {}: {}", u32::from(*crtc), properties(device, u32::from(*crtc), DRM_MODE_OBJECT_CRTC).join(", "));
    }
}

/// Logs each colour pipeline of plane `id`: its operations in order, from the first to the last.
fn report_pipelines(device: &DrmDevice, id: u32) {
    let Some((_, pipelines)) = values(device, id, DRM_MODE_OBJECT_PLANE).into_iter().find(|(name, _)| name == "COLOR_PIPELINE") else {
        return;
    };
    for first in pipelines {
        let mut chain = Vec::new();
        let mut at = first;
        // A bound, so that a cycle can't loop.
        while at != 0 && chain.len() < 16 {
            chain.push(format!("colorop {at}: {}", properties(device, at, DRM_MODE_OBJECT_COLOROP).join(", ")));
            at = values(device, at, DRM_MODE_OBJECT_COLOROP).into_iter().find(|(name, _)| name == "NEXT").and_then(|(_, v)| v.first().copied()).unwrap_or(0);
        }
        log::info!("scanout: plane {id}'s colour pipeline {first}: {}", chain.join(" -> "));
    }
}

/// The properties of object `id` of DRM type `kind`, each as `NAME=value`, an enum's or bitmask's by its
/// names, with the names it may have.
fn properties(device: &DrmDevice, id: u32, kind: u32) -> Vec<String> {
    let fd = device.as_fd();
    let (mut props, mut raw) = (Vec::new(), Vec::new());
    if let Err(e) = drm_ffi::mode::get_properties(fd, id, kind, Some(&mut props), Some(&mut raw)) {
        return vec![format!("(its properties: {e})")];
    }
    props
        .iter()
        .zip(&raw)
        .map(|(prop, value)| {
            let (mut values, mut enums) = (Vec::new(), Vec::new());
            let Ok(info) = drm_ffi::mode::get_property(fd, *prop, Some(&mut values), Some(&mut enums)) else {
                return format!("{prop}=?");
            };
            let name = c_name(&info.name);
            let names = || enums.iter().map(|e| (e.value, c_name(&e.name))).collect::<Vec<_>>();
            let shown = if info.flags & ENUM != 0 {
                let all = names();
                let current = all.iter().find(|(v, _)| v == value).map_or_else(|| value.to_string(), |(_, n)| n.clone());
                format!("{current} of {:?}", all.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>())
            } else if info.flags & BITMASK != 0 {
                let all = names();
                let set: Vec<&str> = all.iter().filter(|(bit, _)| value & (1 << bit) != 0).map(|(_, n)| n.as_str()).collect();
                format!("{set:?} of {:?}", all.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>())
            } else if info.flags & BLOB != 0 {
                format!("blob {value}")
            } else if info.flags & OBJECT != 0 {
                format!("object {value}")
            } else if info.flags & RANGE != 0 {
                format!("{value} in {values:?}")
            } else {
                value.to_string()
            };
            format!("{name}={shown}")
        })
        .collect()
}

/// The properties of object `id` of DRM type `kind` by name, each with its value; for an enum, the values
/// it may have instead (a plane's colour pipelines, by their first operation).
fn values(device: &DrmDevice, id: u32, kind: u32) -> Vec<(String, Vec<u32>)> {
    let fd = device.as_fd();
    let (mut props, mut raw) = (Vec::new(), Vec::new());
    if drm_ffi::mode::get_properties(fd, id, kind, Some(&mut props), Some(&mut raw)).is_err() {
        return Vec::new();
    }
    props
        .iter()
        .zip(&raw)
        .filter_map(|(prop, value)| {
            let (mut values, mut enums) = (Vec::new(), Vec::new());
            let info = drm_ffi::mode::get_property(fd, *prop, Some(&mut values), Some(&mut enums)).ok()?;
            let of = if info.flags & ENUM != 0 { enums.iter().map(|e| e.value as u32).filter(|v| *v != 0).collect() } else { vec![*value as u32] };
            Some((c_name(&info.name), of))
        })
        .collect()
}

/// The properties of object `id` of DRM type `kind`: each one's name, handle and value.
pub(crate) fn properties_of(device: &impl AsFd, id: u32, kind: u32) -> Vec<(String, smithay::reexports::drm::control::property::Handle, u64)> {
    let fd = device.as_fd();
    let (mut props, mut raw) = (Vec::new(), Vec::new());
    if let Err(e) = drm_ffi::mode::get_properties(fd, id, kind, Some(&mut props), Some(&mut raw)) {
        log::warn!("drm: object {id}'s properties: {e}");
        return Vec::new();
    }
    props
        .iter()
        .zip(&raw)
        .filter_map(|(prop, value)| {
            let info = drm_ffi::mode::get_property(fd, *prop, None, None).ok()?;
            let handle = smithay::reexports::drm::control::from_u32(*prop)?;
            Some((c_name(&info.name), handle, *value))
        })
        .collect()
}

fn c_name(name: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A property to set in a commit: of which object, which, to what.
pub(crate) type Property = (smithay::reexports::drm::control::RawResourceHandle, smithay::reexports::drm::control::property::Handle, u64);

/// Property blobs made for commits (colour LUTs and matrices), destroyed with it.
pub(crate) struct Blobs {
    device: smithay::backend::drm::DrmDeviceFd,
    ids: Vec<u32>,
}

impl Blobs {
    pub(crate) fn new(device: &smithay::backend::drm::DrmDeviceFd) -> Blobs {
        Blobs { device: device.clone(), ids: Vec::new() }
    }

    /// A blob of `data`; its id.
    pub(crate) fn create(&mut self, mut data: Vec<u8>) -> Result<u32, clamshell_ui::Error> {
        let id = drm_ffi::mode::create_property_blob(self.device.as_fd(), &mut data).map_err(|e| format!("a property blob: {e}"))?.blob_id;
        self.ids.push(id);
        Ok(id)
    }
}

impl Drop for Blobs {
    fn drop(&mut self) {
        use smithay::reexports::drm::control::Device;
        for id in &self.ids {
            if let Err(e) = self.device.destroy_property_blob(u64::from(*id)) {
                log::warn!("drm: destroying a property blob: {e}");
            }
        }
    }
}

/// The name of enum property `property`'s value `value`.
pub(crate) fn enum_name(device: &impl AsFd, property: smithay::reexports::drm::control::property::Handle, value: u64) -> Option<String> {
    let mut enums = Vec::new();
    drm_ffi::mode::get_property(device.as_fd(), property.into(), None, Some(&mut enums)).ok()?;
    enums.iter().find(|e| e.value == value).map(|e| c_name(&e.name))
}

/// The values enum property `property` may have, but 0 (a plane's colour pipelines: their first colorops).
pub(crate) fn enum_values(device: &impl AsFd, property: smithay::reexports::drm::control::property::Handle) -> Vec<u64> {
    let mut enums = Vec::new();
    if drm_ffi::mode::get_property(device.as_fd(), property.into(), None, Some(&mut enums)).is_err() {
        return Vec::new();
    }
    enums.iter().map(|e| e.value).filter(|v| *v != 0).collect()
}
