//! FNV-1a, a small and stable hash used to make names and values from text: unit and file names,
//! colours, the screenshot test's file hashes, and the library's index. It is not for anything
//! adversarial.

/// FNV-1a, 32 bits.
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// FNV-1a, 64 bits.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = Fnv64::default();
    std::hash::Hasher::write(&mut h, bytes);
    h.0
}

/// FNV-1a, 64 bits, as a [`std::hash::Hasher`]. It goes byte by byte, so bytes written in pieces hash as
/// they would written at once; it is also a [`std::fmt::Write`], to hash text as `write!` makes it
/// without making it.
#[derive(Debug, Clone, Copy)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Fnv64 {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for Fnv64 {
    fn write(&mut self, bytes: &[u8]) {
        self.0 = bytes.iter().fold(self.0, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Write for Fnv64 {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        std::hash::Hasher::write(self, s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_values() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        let mut h = Fnv64::default();
        std::fmt::Write::write_fmt(&mut h, format_args!("{}{}", "a", 1)).unwrap();
        assert_eq!(std::hash::Hasher::finish(&h), fnv1a64(b"a1"), "in pieces as at once");
    }
}
