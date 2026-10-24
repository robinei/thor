//! A sensor hub's board-specific service, timing and mounting; evdev motion is detected first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorHub {
    pub service: u32,
    pub rate_hz: u16,
    pub ticks_per_second: u64,
    pub axes: [[i8; 3]; 3],
}
impl SensorHub {
    pub const fn thor() -> Self {
        Self { service: 400, rate_hz: 200, ticks_per_second: 19_200_000, axes: [[0, -1, 0], [1, 0, 0], [0, 0, 1]] }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_physical_profile_claims_a_sensor_hub() {
        assert_eq!(crate::Profile::thor().sensor_hub, Some(SensorHub::thor()));
        assert_eq!(crate::Profile::vm().sensor_hub, None);
        assert_eq!(crate::Profile::generic().sensor_hub, None);
        let hub = SensorHub::thor();
        assert_eq!((hub.service, hub.rate_hz, hub.ticks_per_second), (400, 200, 19_200_000));
    }
}
