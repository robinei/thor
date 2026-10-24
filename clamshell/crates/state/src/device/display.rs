//! Refresh choices announced by the output manager, at the current pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayModes {
    rates: [u16; 32],
    len: u8,
}
impl DisplayModes {
    /// Adds one announced refresh rate in millihertz. Duplicate rounded rates share a choice.
    /// Returns false only if a new valid rate could not fit.
    pub fn add(&mut self, millihertz: i32) -> bool {
        if !(1000..=1_000_000).contains(&millihertz) {
            return true;
        }
        let hz = ((millihertz + 500) / 1000) as u16;
        if self.rates().contains(&hz) {
            return true;
        }
        let Some(slot) = self.rates.get_mut(usize::from(self.len)) else {
            return false;
        };
        *slot = hz;
        self.len += 1;
        self.rates.get_mut(..usize::from(self.len)).into_iter().for_each(|rates| rates.sort_unstable());
        true
    }
    pub fn rates(&self) -> &[u16] {
        self.rates.get(..usize::from(self.len)).unwrap_or_default()
    }
    /// Zero restores the original mode; an absent saved rate starts again there.
    pub fn next(&self, current: u16) -> u16 {
        if current == 0 {
            return self.rates().first().copied().unwrap_or(0);
        }
        self.rates().iter().position(|rate| *rate == current).and_then(|index| self.rates().get(index + 1)).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_are_bounded_sorted_deduplicated_and_never_invented() {
        let mut modes = DisplayModes::default();
        assert_eq!(modes.next(0), 0);
        for rate in [120000, 59940, 60000, 90000, -1, 0] {
            assert!(modes.add(rate));
        }
        assert_eq!(modes.rates(), &[60, 90, 120]);
        assert_eq!([modes.next(0), modes.next(60), modes.next(90), modes.next(120), modes.next(75)], [60, 90, 120, 0, 0]);
        for rate in 1..=1000 {
            modes.add(rate * 1000);
        }
        assert_eq!(modes.rates().len(), 32);
        assert!(modes.rates().windows(2).all(|pair| matches!(pair, [a,b] if a < b)));
    }
}
