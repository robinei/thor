//! System metrics for the sysinfo tab, and a minute of history for its graphs. Everything is fixed-size,
//! so keeping and drawing them never allocates.

/// The most cores shown individually.
pub const MAX_CORES: usize = 16;
/// Samples kept per graph, one a second.
pub const HISTORY: usize = 60;

/// One reading of the system, made by a system thread once a second. `None` means not available on
/// this machine.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Metrics {
    /// Total CPU load, 0-1.
    pub cpu: Option<f32>,
    /// Per-core load, 0-1, for the first `core_count` cores.
    pub cores: [f32; MAX_CORES],
    pub core_count: u8,
    /// The fastest core's current frequency in MHz.
    pub cpu_mhz: Option<u32>,
    /// GPU load 0-1 and frequency.
    pub gpu: Option<f32>,
    pub gpu_mhz: Option<u32>,
    /// The hottest CPU and GPU sensor, in °C.
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    /// The fan's speed in rpm, 0 when stopped.
    pub fan_rpm: Option<u32>,
    /// Memory in use and in total, and swap in use, in bytes.
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    /// Battery power in watts, positive while discharging.
    pub power_w: Option<f32>,
    /// Minutes left at the current draw, only while discharging.
    pub minutes_left: Option<u32>,
    /// The frame rate of the window in front on the top screen, while it draws: from the compositor, or
    /// under sway from the game through MangoHud.
    pub fps: Option<f32>,
    pub frame_ms: Option<f32>,
}

/// The last `HISTORY` values of one series, oldest first when read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    values: [f32; HISTORY],
    next: usize,
    len: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Ring { values: [0.0; HISTORY], next: 0, len: 0 }
    }
}

impl Ring {
    pub fn push(&mut self, v: f32) {
        if let Some(slot) = self.values.get_mut(self.next) {
            *slot = v;
        }
        self.next = (self.next + 1) % HISTORY;
        self.len = (self.len + 1).min(HISTORY);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The values, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        let start = (self.next + HISTORY - self.len) % HISTORY;
        (0..self.len).filter_map(move |i| self.values.get((start + i) % HISTORY).copied())
    }

    pub fn max(&self) -> f32 {
        self.iter().fold(0.0, f32::max)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct History {
    pub cpu: Ring,
    pub gpu: Ring,
    pub mem: Ring,
    pub power: Ring,
    pub fps: Ring,
}

impl History {
    pub fn push(&mut self, m: &Metrics) {
        self.cpu.push(m.cpu.unwrap_or(0.0));
        self.gpu.push(m.gpu.unwrap_or(0.0));
        self.mem.push(if m.mem_total > 0 { m.mem_used as f32 / m.mem_total as f32 } else { 0.0 });
        self.power.push(m.power_w.unwrap_or(0.0).max(0.0));
        self.fps.push(m.fps.unwrap_or(0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_last_values_oldest_first() {
        let mut r = Ring::default();
        for i in 0..(HISTORY + 5) {
            r.push(i as f32);
        }
        assert_eq!(r.len(), HISTORY);
        assert_eq!(r.iter().next(), Some(5.0));
        assert_eq!(r.iter().last(), Some((HISTORY + 4) as f32));
    }
}
