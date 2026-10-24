//! Requested preview and deterministic fixture geometry, never a substitute for detected output modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preview {
    pub top: (u32, u32),
    pub bottom: (u32, u32),
}
impl Preview {
    /// The reference two-screen canvas, also used for unknown desktop hardware.
    pub const fn reference() -> Self {
        Self { top: (1920, 1080), bottom: (1240, 1080) }
    }
    pub const fn half(self) -> Self {
        Self { top: (self.top.0 / 2, self.top.1 / 2), bottom: (self.bottom.0 / 2, self.bottom.1 / 2) }
    }
}
