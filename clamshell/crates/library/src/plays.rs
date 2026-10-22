//! A summary of an entry's play history. It is read and written by clamshell-system's `store`.

/// A game's plays: how many there were, their total length in seconds, and when the last one ended in
/// epoch seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Plays {
    pub count: u32,
    pub seconds: u64,
    pub last: u64,
}

impl Plays {
    pub fn add(&mut self, start: u64, duration: u64) {
        self.count += 1;
        self.seconds += duration;
        self.last = self.last.max(start + duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_add_up() {
        let mut p = Plays::default();
        p.add(1000, 60);
        p.add(500, 30);
        assert_eq!(p, Plays { count: 2, seconds: 90, last: 1060 });
    }
}
