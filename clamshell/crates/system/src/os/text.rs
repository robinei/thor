//! Text as other programs and formats escape it: percent-encoding (URLs, the trash's records) and the
//! octal and hex escapes of the kernel's tables and udev's names.

/// `text` percent-encoded: everything but ASCII letters, digits, `-._~` and the bytes in `keep` (`/` for
/// a path).
pub fn percent_encode(text: &str, keep: &[u8]) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encoding undone (a `%` not followed by two hex digits is kept as it is).
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(h)) => {
                out.push(h);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `\040` (mounts) and `\x20` (udev's names) as the characters they stand for.
pub fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let octal = bytes.get(i + 1..i + 4).filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        let hex = bytes
            .get(i + 1..i + 4)
            .filter(|d| d.first() == Some(&b'x'))
            .and_then(|d| std::str::from_utf8(d.get(1..)?).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, octal, hex) {
            (b'\\', Some(d), _) => {
                out.push(d.iter().fold(0u8, |n, c| n.wrapping_mul(8).wrapping_add(c - b'0')));
                i += 4;
            }
            (b'\\', None, Some(h)) => {
                out.push(h);
                i += 4;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("Nintendo - SNES/A&B.png", b""), "Nintendo%20-%20SNES%2FA%26B.png");
        assert_eq!(percent_encode("/a b/ø.txt", b"/"), "/a%20b/%C3%B8.txt");
        assert_eq!(percent_decode("A%20B%26C.png"), "A B&C.png");
        assert_eq!(percent_decode(&percent_encode("/a b/ø%.txt", b"/")), "/a b/ø%.txt");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn escapes() {
        assert_eq!(unescape(r"/run/media/u/My\040Card"), "/run/media/u/My Card");
        assert_eq!(unescape(r"My\x20Card"), "My Card");
        assert_eq!(unescape(r"a\b\0"), r"a\b\0", "not an escape");
    }
}
