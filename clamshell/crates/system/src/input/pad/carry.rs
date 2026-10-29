//! Motion kept in fractions between steps: a slow stick or turn moves the pointer or wheel a fraction of a
//! unit each step, which would be lost if each step were rounded on its own.

/// High-resolution wheel units per notch, by the kernel's convention.
pub(crate) const NOTCH: i32 = 120;

/// A two-axis motion's fractions not sent yet.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Carry(f32, f32);

impl Carry {
    /// Adds `(x, y)` to what is kept.
    pub(crate) fn add(&mut self, x: f32, y: f32) {
        (self.0, self.1) = (self.0 + x, self.1 + y);
    }

    /// The whole units kept, which are taken; the fractions stay.
    pub(crate) fn take(&mut self) -> (i32, i32) {
        let (x, y) = (self.0.trunc(), self.1.trunc());
        (self.0, self.1) = (self.0 - x, self.1 - y);
        (x as i32, y as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_add_up_to_whole_units() {
        let mut c = Carry::default();
        c.add(0.4, -0.4);
        assert_eq!(c.take(), (0, 0));
        c.add(0.4, -0.4);
        c.add(0.4, -0.4);
        assert_eq!(c.take(), (1, -1), "1.2 and -1.2");
        c.add(0.9, -0.9);
        assert_eq!(c.take(), (1, -1), "the 0.2 kept");
    }
}
