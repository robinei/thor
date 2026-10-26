//! Text as other programs and formats escape it: percent-encoding for URLs and the trash's records,
//! and the octal and hex escapes in the kernel's tables and udev's names.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// The characters left as they are in one part of a URL: letters, digits and `-._~`.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
/// The same, plus `/` between the parts of a path.
const PATH: &AsciiSet = &COMPONENT.remove(b'/');

/// `text` percent-encoded as one part of a URL.
pub(crate) fn percent_encode(text: &str) -> String {
    utf8_percent_encode(text, COMPONENT).to_string()
}

/// A path percent-encoded, with its slashes kept.
pub(crate) fn percent_encode_path(text: &str) -> String {
    utf8_percent_encode(text, PATH).to_string()
}

/// Percent-encoding undone. A `%` not followed by two hex digits stays as it is.
pub(crate) fn percent_decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// Replaces escapes such as `\040` (in mounts) and `\x20` (in udev's names) with the characters they
/// stand for.
pub(crate) fn unescape(text: &str) -> String {
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
        assert_eq!(percent_encode("Nintendo - SNES/A&B.png"), "Nintendo%20-%20SNES%2FA%26B.png");
        assert_eq!(percent_encode_path("/a b/ø.txt"), "/a%20b/%C3%B8.txt");
        assert_eq!(percent_decode("A%20B%26C.png"), "A B&C.png");
        assert_eq!(percent_decode(&percent_encode_path("/a b/ø%.txt")), "/a b/ø%.txt");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn escapes() {
        assert_eq!(unescape(r"/run/media/u/My\040Card"), "/run/media/u/My Card");
        assert_eq!(unescape(r"My\x20Card"), "My Card");
        assert_eq!(unescape(r"a\b\0"), r"a\b\0", "not an escape");
    }
}
