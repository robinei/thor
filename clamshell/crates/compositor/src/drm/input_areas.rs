//! Where each absolute input device lies in the scene, and ClamShell's own touchpad.

use super::*;

/// ClamShell's virtual touchpad (the touchpad tab, the keyboard while it points), as libinput names it.
pub(super) const TOUCHPAD: &str = "4660:22136:clamshell_touchpad";

/// On ClamShell's touchpad a tap clicks, two fingers tapped right-click, and a tap then a held finger
/// drags: libinput leaves tapping off by default.
pub(super) fn tap(device: &mut smithay::reexports::input::Device) {
    let mut set = true;
    for (what, done) in [("tap", device.config_tap_set_enabled(true)), ("tap and drag", device.config_tap_set_drag_enabled(true))] {
        if let Err(error) = done {
            log::warn!("drm: ClamShell's touchpad: {what}: {error:?}");
            set = false;
        }
    }
    if set {
        log::info!("drm: ClamShell's touchpad taps and drags");
    }
}

/// Where each absolute input device lies in the scene: on the screen the profile places it, else, for a
/// touchscreen, the profile's catch-all; anything else (a tablet nobody named) covers the top screen.
#[derive(Clone, Copy)]
pub(super) struct Areas {
    pub(super) profile: DeviceProfile,
    pub(super) top: Size<i32, Logical>,
    pub(super) bottom: Size<i32, Logical>,
}

impl Areas {
    pub(super) fn role(&self, device: &InputDevice) -> ScreenRole {
        let touch = device.has_capability(DeviceCapability::Touch.into());
        screen_of(self.profile.touchscreens, touch, |identifier| names(identifier, device))
    }

    /// The device's screen's area in the scene, and how its panel is turned.
    pub(super) fn of(&self, device: &InputDevice) -> (Rectangle<f64, Logical>, Transform) {
        let role = self.role(device);
        (area(role, self.top, self.bottom), transform(self.profile.screen(role).rotation))
    }
}

/// The screen of an absolute input device: the one of the profile's `touchscreens` that names it (by
/// `named`, see [`names`]), else, for a touchscreen (`touch`), the catch-all's, else the top screen.
pub(super) fn screen_of(touchscreens: &[Touchscreen], touch: bool, named: impl Fn(&str) -> bool) -> ScreenRole {
    let by_name = touchscreens.iter().find(|t| matches!(t.selector, TouchSelector::Named(n) if named(n)));
    by_name.or_else(|| touch.then(|| touchscreens.iter().find(|t| t.selector == TouchSelector::All)).flatten()).map_or(ScreenRole::Top, |t| t.screen)
}

/// Screen `role`'s area in the scene, the screens being `top` and `bottom` in size: the top screen's at
/// the origin, the bottom one's to its right.
pub(super) fn area(role: ScreenRole, top: Size<i32, Logical>, bottom: Size<i32, Logical>) -> Rectangle<f64, Logical> {
    match role {
        ScreenRole::Top => Rectangle::from_size(top.to_f64()),
        ScreenRole::Bottom => Rectangle::new((f64::from(top.w), 0.0).into(), bottom.to_f64()),
    }
}

/// A device's identifier as the profile names it: vendor, product and name, with spaces as underscores.
pub(super) fn identifier(device: &InputDevice) -> String {
    format!("{}:{}:{}", device.id_vendor(), device.id_product(), device.name().replace(' ', "_"))
}

/// Whether `identifier` (as [`identifier`] makes it) names `device`, compared without allocating: this is
/// asked for every absolute input event.
pub(super) fn names(identifier: &str, device: &InputDevice) -> bool {
    names_ids(identifier, device.id_vendor(), device.id_product(), &device.name())
}

/// [`names`], for a device's vendor, product and name.
pub(super) fn names_ids(identifier: &str, vendor: u32, product: u32, name: &str) -> bool {
    let mut parts = identifier.splitn(3, ':');
    let number = |part: Option<&str>| part.and_then(|p| p.parse::<u32>().ok());
    let (ours, theirs) = ((number(parts.next()), number(parts.next())), (Some(vendor), Some(product)));
    let named = parts.next().unwrap_or("");
    ours == theirs && named.len() == name.len() && named.bytes().zip(name.bytes()).all(|(a, b)| a == b || (a == b'_' && b == b' '))
}
