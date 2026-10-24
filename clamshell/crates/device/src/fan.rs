//! Board fan policy. Actual hwmon devices and thermal readings are still discovered at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fan {
    pub hwmon_name: &'static str,
    pub auto: &'static [(u16, u8)],
    pub quiet: &'static [(u16, u8)],
    pub emergency_celsius: u16,
    pub safe_duty: u8,
    pub min_running: u8,
    pub off_below: u8,
    pub step_up: u8,
    pub step_down: u8,
}
impl Fan {
    pub const fn thor() -> Self {
        Self {
            hwmon_name: "pwmfan",
            auto: &[(55, 51), (72, 77), (78, 102), (84, 153), (90, 204), (96, 255)],
            quiet: &[(60, 51), (78, 77), (85, 102), (90, 153), (95, 204)],
            emergency_celsius: 95,
            safe_duty: 128,
            min_running: 51,
            off_below: 5,
            step_up: 36,
            step_down: 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_profiles_do_not_invent_a_curve_and_known_curves_are_ordered() {
        assert_eq!(crate::Profile::generic().fan, None);
        assert_eq!(crate::Profile::vm().fan, None);
        for points in [Fan::thor().auto, Fan::thor().quiet] {
            assert!(points.windows(2).all(|points| matches!(points, [(left,_),(right,_)] if left < right)));
            assert!(points.first().is_some_and(|(_, duty)| *duty >= Fan::thor().min_running));
        }
    }
}
