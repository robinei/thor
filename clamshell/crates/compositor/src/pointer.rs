//! Pointer capture belongs to a focused surface and ends when that window loses focus.

use smithay::{
    backend::input::InputTime,
    input::pointer::RelativeMotionEvent,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point},
    wayland::{
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
        relative_pointer::RelativePointerManagerState,
    },
};

use crate::{geometry::Geometry, server::Server};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureMode {
    #[default]
    Free,
    Locked,
    Confined,
}

#[derive(Clone)]
pub(crate) struct Capture {
    window: i64,
    surface: WlSurface,
    geometry: Geometry,
    origin: Point<f64, Logical>,
    mode: CaptureMode,
}

impl Server {
    pub(crate) fn enable_relative_pointer(&mut self) {
        if self.relative_pointer {
            return;
        }
        RelativePointerManagerState::new::<Self>(&self.dh);
        smithay::wayland::pointer_constraints::PointerConstraintsState::new::<Self>(&self.dh);
        self.relative_pointer = true;
    }

    pub(crate) fn release_capture(&mut self) {
        if let Some(capture) = self.capture.take()
            && let Some(pointer) = self.seat.get_pointer()
        {
            with_pointer_constraint(&capture.surface, &pointer, |constraint| {
                if let Some(constraint) = constraint {
                    constraint.deactivate();
                }
            });
        }
    }

    pub(crate) fn validate_capture(&mut self) {
        let valid = self.capture.as_ref().is_none_or(|capture| {
            self.host_focused
                && self.menu.is_none()
                && !self.has_x11_menu()
                && capture.surface.is_alive()
                && self.core.state().session().keyboard_focus() == Some(capture.window)
                && self.visible(capture.window)
                && self.geometry(capture.window) == Some(capture.geometry)
                && self.seat.get_pointer().is_some_and(|pointer| {
                    with_pointer_constraint(&capture.surface, &pointer, |constraint| {
                        constraint.is_some_and(|constraint| {
                            constraint.is_active()
                                && constraint
                                    .region()
                                    .is_none_or(|region| region.contains((capture.geometry.surface_coords(self.pointer) - capture.origin).to_i32_round()))
                        })
                    })
                })
        });
        if !valid {
            self.release_capture();
        }
    }

    pub(crate) fn pointer_locked(&self) -> bool {
        self.capture.as_ref().is_some_and(|capture| capture.mode == CaptureMode::Locked)
    }

    pub(crate) fn sync_pointer_capture<E: std::fmt::Display>(&mut self, mut apply: impl FnMut(CaptureMode) -> Result<(), E>) {
        self.validate_capture();
        let wanted = if self.relative_pointer && self.host_focused && self.menu.is_none() && !self.has_x11_menu() {
            self.capture.clone().or_else(|| {
                let hit = self.hit(self.pointer)?;
                if !hit.surface.is_alive() || self.core.state().session().keyboard_focus() != Some(hit.window) {
                    return None;
                }
                let pointer = self.seat.get_pointer()?;
                if pointer.current_focus().as_ref() != Some(&hit.surface) {
                    return None;
                }
                let mode = with_pointer_constraint(&hit.surface, &pointer, |constraint| {
                    let constraint = constraint?;
                    if !constraint.region().is_none_or(|region| region.contains((hit.location - hit.origin).to_i32_round())) {
                        return None;
                    }
                    Some(match &*constraint {
                        PointerConstraint::Locked(_) => CaptureMode::Locked,
                        PointerConstraint::Confined(_) => CaptureMode::Confined,
                    })
                })?;
                Some(Capture { window: hit.window, surface: hit.surface, geometry: self.geometry(hit.window)?, origin: hit.origin, mode })
            })
        } else {
            None
        };
        let mode = wanted.as_ref().map_or(CaptureMode::Free, |capture| capture.mode);
        if mode != self.host_capture {
            if let Err(error) = apply(mode) {
                if let Some(capture) = &wanted {
                    self.reject_client(&capture.surface, &format!("host cannot capture pointer: {error}"));
                } else {
                    log::error!("compositor: host pointer capture cannot be released: {error}");
                    self.exit = true;
                }
                self.release_capture();
                return;
            }
            self.host_capture = mode;
        }
        if self.capture.is_none()
            && let Some(capture) = wanted
        {
            if let Some(pointer) = self.seat.get_pointer() {
                with_pointer_constraint(&capture.surface, &pointer, |constraint| {
                    if let Some(constraint) = constraint {
                        constraint.activate();
                    }
                });
            }
            self.capture = Some(capture);
        }
    }

    pub(crate) fn constrain_pointer(&mut self, requested: Point<f64, Logical>) -> Point<f64, Logical> {
        self.validate_capture();
        let Some(capture) = &self.capture else {
            return requested;
        };
        if capture.mode == CaptureMode::Locked {
            return self.pointer;
        }
        let allowed = |point| {
            let Some(hit) = self.hit(point) else {
                return false;
            };
            if hit.surface != capture.surface {
                return false;
            }
            self.seat.get_pointer().is_some_and(|pointer| {
                with_pointer_constraint(&capture.surface, &pointer, |constraint| {
                    constraint.is_some_and(|constraint| constraint.region().is_none_or(|region| region.contains((hit.location - hit.origin).to_i32_round())))
                })
            })
        };
        if allowed(requested) {
            return requested;
        }
        // Preserve a permitted component when the other crosses a region boundary.
        let mut result = self.pointer;
        let horizontal = (requested.x, result.y).into();
        if allowed(horizontal) {
            result = horizontal;
        }
        let vertical = (result.x, requested.y).into();
        if allowed(vertical) {
            result = vertical;
        }
        result
    }

    /// Relative-only devices, including libinput mice, also move our cursor.
    pub(crate) fn pointer_relative(&mut self, delta: Point<f64, Logical>, unaccelerated: Point<f64, Logical>, time: InputTime) {
        self.relative_motion(delta, unaccelerated, time);
        if !self.pointer_locked() {
            self.pointer_absolute(self.pointer + delta, time);
        }
    }

    pub(crate) fn relative_motion(&mut self, delta: Point<f64, Logical>, unaccelerated: Point<f64, Logical>, time: InputTime) {
        self.validate_capture();
        self.idle.input(std::time::Instant::now());
        self.cursor_activity.motion(std::time::Instant::now());
        let target = self
            .capture
            .as_ref()
            .map(|capture| (capture.surface.clone(), capture.origin, capture.geometry))
            .or_else(|| self.hit(self.pointer).and_then(|hit| Some((hit.surface, hit.origin, self.geometry(hit.window)?))));
        let Some((surface, origin, geometry)) = target else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.current_focus().as_ref() != Some(&surface) {
            return;
        }
        pointer.relative_motion(
            self,
            Some((surface, origin)),
            &RelativeMotionEvent {
                delta: (delta.x / geometry.scale, delta.y / geometry.scale).into(),
                delta_unaccel: (unaccelerated.x / geometry.scale, unaccelerated.y / geometry.scale).into(),
                time,
            },
        );
        pointer.frame(self);
    }
}
