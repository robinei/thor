//! Text that drawing formats: [`Fmt`], a string on the stack (so a frame doesn't allocate, and any number
//! of them can exist at once), and the numbers, times and counts the views show, as `Display` values.

use std::fmt::{self, Display, Formatter, Write};

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

    /// `args` formatted (with `format_args!`).
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

/// A fraction (0-1) as a percentage, or a dash.
pub struct Percent(pub Option<f32>);

impl Display for Percent {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{:.0}%", v * 100.0),
            None => f.write_str("–"),
        }
    }
}

/// A share (0-1) as a percentage: one decimal below 10 %, so small ones don't all read 0 %.
pub struct Share(pub f32);

impl Display for Share {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let percent = self.0 * 100.0;
        if percent < 9.95 { write!(f, "{percent:.1}%") } else { write!(f, "{percent:.0}%") }
    }
}

/// A clock frequency in MHz: "3.7 GHz", "680 MHz", or "clock unknown".
pub struct Frequency(pub Option<u32>);

impl Display for Frequency {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(mhz) if mhz >= 1000 => write!(f, "{:.1} GHz", mhz as f32 / 1000.0),
            Some(mhz) => write!(f, "{mhz} MHz"),
            None => f.write_str("clock unknown"),
        }
    }
}

/// A temperature after a detail: " · 46°C", or nothing.
pub struct Temp(pub Option<f32>);

impl Display for Temp {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(t) => write!(f, " · {t:.0}°C"),
            None => Ok(()),
        }
    }
}

/// A level (0-100) with `suffix` after it, or "-" when unknown.
pub struct Level<'a>(pub Option<u8>, pub &'a str);

impl Display for Level<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{v}{}", self.1),
            None => f.write_str("-"),
        }
    }
}

/// How long a game was played, in seconds: "Played briefly", "Played 5 min", "Played 2 h 5 min".
pub struct PlayTime(pub u64);

impl Display for PlayTime {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match (self.0 / 3600, self.0 % 3600 / 60) {
            (0, 0) => f.write_str("Played briefly"),
            (0, m) => write!(f, "Played {m} min"),
            (h, m) => write!(f, "Played {h} h {m} min"),
        }
    }
}

/// How many days ago, after a detail: " · today", " · yesterday", " · 3 days ago".
pub struct DaysAgo(pub u64);

impl Display for DaysAgo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            0 => f.write_str(" · today"),
            1 => f.write_str(" · yesterday"),
            d => write!(f, " · {d} days ago"),
        }
    }
}

/// A file's size: "512 B", "3.4 kB", "1.2 GB" (powers of 1000).
pub struct Bytes(pub u64);

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let units = ["kB", "MB", "GB", "TB"];
        let mut value = self.0 as f64;
        if value < 1000.0 {
            return write!(f, "{} B", self.0);
        }
        for unit in units {
            value /= 1000.0;
            if value < 1000.0 || unit == "TB" {
                return if value < 10.0 { write!(f, "{value:.1} {unit}") } else { write!(f, "{value:.0} {unit}") };
            }
        }
        Ok(())
    }
}

/// How many environment variables: "No variables", "1 variable", "3 variables".
pub struct Variables(pub usize);

impl Display for Variables {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            0 => f.write_str("No variables"),
            1 => f.write_str("1 variable"),
            n => write!(f, "{n} variables"),
        }
    }
}

/// Minutes as hours and minutes: "3 h 10 min".
pub struct Duration(pub u32);

impl Display for Duration {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{} h {} min", self.0 / 60, self.0 % 60)
    }
}

/// Minutes as "2:05".
pub struct HoursMinutes(pub u32);

impl Display for HoursMinutes {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}:{:02}", self.0 / 60, self.0 % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(d: impl Display) -> String {
        d.to_string()
    }

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

    #[test]
    fn numbers() {
        assert_eq!(show(Percent(Some(0.426))), "43%");
        assert_eq!(show(Percent(None)), "–");
        assert_eq!(show(Share(0.05)), "5.0%");
        assert_eq!(show(Share(0.2)), "20%");
        assert_eq!(show(Frequency(Some(3700))), "3.7 GHz");
        assert_eq!(show(Frequency(Some(680))), "680 MHz");
        assert_eq!(show(Frequency(None)), "clock unknown");
        assert_eq!(show(Temp(Some(46.2))), " · 46°C");
        assert_eq!(show(Temp(None)), "");
        assert_eq!(show(Level(Some(40), " %")), "40 %");
        assert_eq!(show(Level(None, "")), "-");
    }

    #[test]
    fn times_and_counts() {
        assert_eq!(show(PlayTime(30)), "Played briefly");
        assert_eq!(show(PlayTime(300)), "Played 5 min");
        assert_eq!(show(PlayTime(7500)), "Played 2 h 5 min");
        assert_eq!(show(DaysAgo(0)), " · today");
        assert_eq!(show(DaysAgo(1)), " · yesterday");
        assert_eq!(show(DaysAgo(3)), " · 3 days ago");
        assert_eq!(show(Bytes(512)), "512 B");
        assert_eq!(show(Bytes(3_400)), "3.4 kB");
        assert_eq!(show(Bytes(1_234_000_000)), "1.2 GB");
        assert_eq!(show(Bytes(25_000_000)), "25 MB");
        assert_eq!(show(Variables(0)), "No variables");
        assert_eq!(show(Variables(1)), "1 variable");
        assert_eq!(show(Variables(2)), "2 variables");
        assert_eq!(show(Duration(190)), "3 h 10 min");
        assert_eq!(show(HoursMinutes(125)), "2:05");
    }
}
