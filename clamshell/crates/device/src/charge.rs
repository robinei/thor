//! Charging quirks and non-standard firmware attributes. Standard battery controls are discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Charge {
    /// Some firmware exposes an end threshold but never honours it.
    pub prefer_current: bool,
    pub switches: Option<Switches>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub charge_now: &'static str,
    pub limit80: &'static str,
}
impl Charge {
    /// SM8550 firmware ignored end-threshold requests in Thor trials with Kettle; charge current
    /// worked with Armada patch 0903. The OEM switches are exposed by linux-thor but remain untried.
    pub const fn thor() -> Self {
        Self { prefer_current: true, switches: Some(Switches { charge_now: "retroid_charge_now", limit80: "retroid_limit_capacity" }) }
    }
    pub const fn standard() -> Self {
        Self { prefer_current: false, switches: None }
    }
}
