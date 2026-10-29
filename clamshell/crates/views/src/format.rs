//! The words the views show for numbers, times and counts, as `Display` values to format into a
//! [`Fmt`](clamshell_ui::toolkit::format::Fmt) without allocating.

use std::fmt::{self, Display, Formatter};

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

/// A share (0-1) as a percentage. Below 10 % it has one decimal, so small shares don't all read 0 %.
pub struct Share(pub f32);

impl Display for Share {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let percent = self.0 * 100.0;
        if percent < 9.95 { write!(f, "{percent:.1}%") } else { write!(f, "{percent:.0}%") }
    }
}

/// A share that may be unknown because it couldn't be read. An unknown share is shown as a dash.
pub struct MaybeShare(pub Option<f32>);

impl Display for MaybeShare {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(share) => Share(share).fmt(f),
            None => f.write_str("–"),
        }
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

/// A fan's speed after a detail: " · fan 3200 rpm", " · fan off", or nothing.
pub struct Fan(pub Option<u32>);

impl Display for Fan {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0 {
            Some(0) => f.write_str(" · fan off"),
            Some(rpm) => write!(f, " · fan {rpm} rpm"),
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
        assert_eq!(show(Fan(Some(3200))), " · fan 3200 rpm");
        assert_eq!(show(Fan(Some(0))), " · fan off");
        assert_eq!(show(Fan(None)), "");
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
