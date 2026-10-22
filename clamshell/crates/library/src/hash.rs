//! Hashes, written out here rather than taken from crates, each for what other programs or formats name
//! by it. FNV-1a: a small, stable hash for names derived from text (unit and file names, colours, the
//! screenshot test's file hashes). CRC32: game files' checksums, as zip and No-Intro have them. MD5: mpv's
//! names for its watch_later files. None of them for anything adversarial.

/// FNV-1a, 32 bits.
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// FNV-1a, 64 bits.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// CRC32 (as zip and No-Intro have it).
pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| CRC32_TABLE.get(usize::from((c as u8) ^ b)).copied().unwrap_or(0) ^ (c >> 8))
}

/// CRC-32's table (the reflected polynomial 0xEDB88320), built at compile time.
// (Indexing in a const can't panic at run time: out of bounds, it fails the build.)
#[allow(clippy::indexing_slicing)]
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// MD5 (mpv names its watch_later files by it).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
        11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i: i32| (f64::from(i + 1).sin().abs() * 4_294_967_296.0) as u32).collect();
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_le_bytes());
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for chunk in message.chunks_exact(64) {
        let m: Vec<u32> = chunk.chunks_exact(4).filter_map(|w| <[u8; 4]>::try_from(w).ok()).map(u32::from_le_bytes).collect();
        let [mut a, mut b, mut c, mut d] = state;
        for (i, (&k, &shift)) in k.iter().zip(SHIFTS.iter()).enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let word = m.get(g).copied().unwrap_or(0);
            let rotated = a.wrapping_add(f).wrapping_add(k).wrapping_add(word).rotate_left(shift);
            (a, d, c) = (d, c, b);
            b = b.wrapping_add(rotated);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(state.iter().flat_map(|s| s.to_le_bytes())) {
        *o = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_values() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"The quick brown fox jumps over the lazy dog")), "9e107d9d372bb6826bd81d3542a419d6");
    }
}
