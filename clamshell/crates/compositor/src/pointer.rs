//! Pointer capture belongs to a focused surface and ends when that window loses focus.

use smithay::{
    backend::input::InputTime,
    input::pointer::{CursorImageStatus, RelativeMotionEvent},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point},
    wayland::{
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
        relative_pointer::RelativePointerManagerState,
    },
};

use crate::{geometry::Geometry, server::Server};

/// The pointer: where it is, what its buttons are doing, the cursor it shows, and its capture by a
/// window.
pub(crate) struct Pointer {
    /// Where it is in the scene.
    pub location: Point<f64, Logical>,
    /// The window a button came down on, which gets the motion while any button is held.
    pub window: Option<i64>,
    /// The buttons held, in the order they came down.
    pub buttons: Vec<u32>,
    /// The left button came down on an overlay's target, which acted at once; its release is not passed on.
    pub overlay_pressed: bool,
    /// The image the client under it asked for.
    pub cursor: CursorImageStatus,
    /// When it last moved, for the cursor to hide after a while still.
    pub cursor_activity: crate::cursor::Activity,
    /// Whether clients are offered relative motion and pointer constraints (see
    /// [`Server::enable_relative_pointer`]).
    relative: bool,
    /// The window holding the pointer locked or confined, if one does.
    pub capture: Option<Capture>,
    /// The capture the host was last asked for.
    pub host_capture: CaptureMode,
    /// Whether the host has the input focus; a capture ends while it hasn't.
    pub host_focused: bool,
}

impl Pointer {
    pub fn new() -> Self {
        Self {
            location: (0.0, 0.0).into(),
            window: None,
            buttons: Vec::new(),
            overlay_pressed: false,
            cursor: CursorImageStatus::default_named(),
            cursor_activity: crate::cursor::Activity::default(),
            relative: false,
            capture: None,
            host_capture: CaptureMode::Free,
            host_focused: true,
        }
    }
}

/// How the pointer is held: free, locked in place (its motion reaching the window only as relative
/// motion), or confined to a region of the window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureMode {
    #[default]
    Free,
    Locked,
    Confined,
}

/// A window's lock or confinement in force: its surface, and the window's geometry and the surface's
/// origin when it began, which it ends on changing.
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
        if self.pointer.relative {
            return;
        }
        RelativePointerManagerState::new::<Self>(&self.dh);
        smithay::wayland::pointer_constraints::PointerConstraintsState::new::<Self>(&self.dh);
        self.pointer.relative = true;
    }

    pub(crate) fn release_capture(&mut self) {
        if let Some(capture) = self.pointer.capture.take()
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
        let valid = self.pointer.capture.as_ref().is_none_or(|capture| {
            self.pointer.host_focused
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
                                && constraint.region().is_none_or(|region| {
                                    region.contains((capture.geometry.surface_coords(self.pointer.location) - capture.origin).to_i32_round())
                                })
                        })
                    })
                })
        });
        if !valid {
            self.release_capture();
        }
    }

    pub(crate) fn pointer_locked(&self) -> bool {
        self.pointer.capture.as_ref().is_some_and(|capture| capture.mode == CaptureMode::Locked)
    }

    pub(crate) fn sync_pointer_capture<E: std::fmt::Display>(&mut self, mut apply: impl FnMut(CaptureMode) -> Result<(), E>) {
        self.validate_capture();
        let wanted = if self.pointer.relative && self.pointer.host_focused && self.menu.is_none() && !self.has_x11_menu() {
            self.pointer.capture.clone().or_else(|| {
                let hit = self.hit(self.pointer.location)?;
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
        if mode != self.pointer.host_capture {
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
            self.pointer.host_capture = mode;
        }
        if self.pointer.capture.is_none()
            && let Some(capture) = wanted
        {
            if let Some(pointer) = self.seat.get_pointer() {
                with_pointer_constraint(&capture.surface, &pointer, |constraint| {
                    if let Some(constraint) = constraint {
                        constraint.activate();
                    }
                });
            }
            self.pointer.capture = Some(capture);
        }
    }

    pub(crate) fn constrain_pointer(&mut self, requested: Point<f64, Logical>) -> Point<f64, Logical> {
        self.validate_capture();
        let Some(capture) = &self.pointer.capture else {
            return requested;
        };
        if capture.mode == CaptureMode::Locked {
            return self.pointer.location;
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
        let mut result = self.pointer.location;
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
            self.pointer_absolute(self.pointer.location + delta, time);
        }
    }

    pub(crate) fn relative_motion(&mut self, delta: Point<f64, Logical>, unaccelerated: Point<f64, Logical>, time: InputTime) {
        self.validate_capture();
        self.idle.input(std::time::Instant::now());
        self.pointer.cursor_activity.motion(std::time::Instant::now());
        let target = self
            .pointer
            .capture
            .as_ref()
            .map(|capture| (capture.surface.clone(), capture.origin, capture.geometry))
            .or_else(|| self.hit(self.pointer.location).and_then(|hit| Some((hit.surface, hit.origin, self.geometry(hit.window)?))));
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
                delta: (delta.x / geometry.scale.x, delta.y / geometry.scale.y).into(),
                delta_unaccel: (unaccelerated.x / geometry.scale.x, unaccelerated.y / geometry.scale.y).into(),
                time,
            },
        );
        pointer.frame(self);
    }
}
