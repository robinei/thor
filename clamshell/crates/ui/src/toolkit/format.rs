//! Text formatted for drawing. [`Fmt`] is a string on the stack, so a frame doesn't allocate and any
//! number of them can exist at once.

use std::fmt::{self, Formatter, Write};

/// Up to `N` bytes of text, on the stack. Writing past the end cuts it at the last whole character.
#[derive(Clone, Copy)]
pub struct Fmt<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Fmt<N> {
    pub fn new() -> Self {
        Fmt { buf: [0; N], len: 0 }
    }

    /// `args` formatted, as made by `format_args!`.
    pub fn of(args: fmt::Arguments) -> Self {
        let mut f = Fmt::new();
        let _ = f.write_fmt(args);
        f
    }

    pub fn as_str(&self) -> &str {
        // Only whole characters are ever copied in, so this is always valid UTF-8.
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl<const N: usize> Default for Fmt<N> {
    fn default() -> Self {
        Fmt::new()
    }
}

impl<const N: usize> Write for Fmt<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = N - self.len;
        let mut take = s.len().min(room);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.len..self.len + take), s.as_bytes().get(..take)) {
            dst.copy_from_slice(src);
            self.len += take;
        }
        Ok(())
    }
}

impl<const N: usize> std::ops::Deref for Fmt<N> {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize> fmt::Debug for Fmt<N> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_cuts_at_a_whole_character() {
        let f = Fmt::<4>::of(format_args!("ab{}", "°cd"));
        assert_eq!(&*f, "ab°");
        let f = Fmt::<3>::of(format_args!("ab{}", "°cd"));
        assert_eq!(&*f, "ab");
        let f = Fmt::<4>::of(format_args!("{}", 12345));
        assert_eq!(&*f, "1234");
        assert_eq!(&*Fmt::<8>::new(), "");
    }
}
