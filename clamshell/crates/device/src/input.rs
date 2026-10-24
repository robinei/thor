//! Physical auxiliary controls become fixed logical roles before routing and keymaps see them.

/// Internal evdev codes for the logical Back and auxiliary-button roles.
pub const BACK_ROLE: u16 = 0x116;
pub const AUXILIARY_ROLE: u16 = 194;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryKey {
    pub device_name: &'static str,
    pub code: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub back_key: Option<u16>,
    pub auxiliary: Option<AuxiliaryKey>,
    pub letter_code_drivers: &'static [&'static str],
}
impl Controls {
    pub const fn thor() -> Self {
        Self { back_key: Some(0x116), auxiliary: Some(AuxiliaryKey { device_name: "gpio-keys-ayn", code: 194 }), letter_code_drivers: &["rsinput", "xpad"] }
    }
    pub const fn generic() -> Self {
        Self { back_key: Some(0x116), auxiliary: None, letter_code_drivers: &["xpad"] }
    }
    pub fn normalise_button(self, code: u16) -> u16 {
        if self.back_key == Some(code) { BACK_ROLE } else { code }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn button_conventions_are_facts_about_the_detected_driver() {
        assert!(Controls::thor().letter_code_drivers.contains(&"rsinput"));
        assert!(Controls::thor().letter_code_drivers.contains(&"xpad"));
        assert!(Controls::generic().letter_code_drivers.contains(&"xpad"));
        assert!(!Controls::generic().letter_code_drivers.contains(&"rsinput"));
        assert!(!Controls::thor().letter_code_drivers.contains(&"hid-generic"));
    }
    #[test]
    fn physical_back_codes_are_normalised_and_missing_auxiliary_controls_stay_absent() {
        let controls = Controls { back_key: Some(123), auxiliary: None, letter_code_drivers: &[] };
        assert_eq!(controls.normalise_button(123), BACK_ROLE);
        assert_eq!(controls.normalise_button(42), 42);
        assert_eq!(Controls::generic().auxiliary, None);
        assert_eq!(Controls::thor().auxiliary, Some(AuxiliaryKey { device_name: "gpio-keys-ayn", code: 194 }));
    }
}
