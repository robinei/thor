//! The local time as the bar and the time tab show it: the zone looked up once a minute, not per frame.

use super::format::Fmt;

/// The local time, as the clocks show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalTime {
    pub year: i16,
    /// 1-12.
    pub month: i8,
    pub day: i8,
    /// 0 Monday - 6 Sunday.
    pub weekday: i8,
    pub hour: i8,
    pub minute: i8,
}

impl LocalTime {
    pub const MONTHS: [&'static str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    pub const WEEKDAYS: [&'static str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

    pub fn month_name(self) -> &'static str {
        Self::MONTHS.get(usize::try_from(self.month - 1).unwrap_or(0)).copied().unwrap_or("")
    }

    pub fn weekday_name(self) -> &'static str {
        Self::WEEKDAYS.get(usize::try_from(self.weekday).unwrap_or(0)).copied().unwrap_or("")
    }

    /// The hour on a 12-hour dial (12, 1-11) and whether it is after noon.
    pub fn twelve(self) -> (i8, bool) {
        let h = self.hour % 12;
        (if h == 0 { 12 } else { h }, self.hour >= 12)
    }

    /// "14:05", or on a 12-hour dial "2:05 PM".
    pub fn time(self, twelve_hour: bool) -> Fmt<8> {
        if twelve_hour {
            let (h, pm) = self.twelve();
            Fmt::of(format_args!("{h}:{:02} {}", self.minute, if pm { "PM" } else { "AM" }))
        } else {
            Fmt::of(format_args!("{:02}:{:02}", self.hour, self.minute))
        }
    }
}

/// Kept between frames: the local time, worked out once a minute (the time zone looked up then, not
/// every frame).
#[derive(Debug, Default)]
pub struct Scratch {
    local: Option<(u64, LocalTime)>,
    /// The clock's zone, by name, as last given.
    zone: Option<(String, jiff::tz::TimeZone)>,
}

impl Scratch {
    /// The local time at `unix_secs`: in `zone` (the time tab's reading, once there is one: the system's
    /// zone as it changes), else the system's zone as the program found it.
    pub(crate) fn local(&mut self, unix_secs: u64, zone: Option<&str>) -> LocalTime {
        let minute = unix_secs / 60;
        if self.zone.as_ref().map(|(name, _)| name.as_str()) != zone {
            let tz = zone.and_then(|z| jiff::tz::db().get(z).ok());
            self.zone = zone.map(|z| (z.to_owned(), tz.unwrap_or_else(jiff::tz::TimeZone::system)));
            self.local = None;
        }
        match self.local {
            Some((m, local)) if m == minute => local,
            _ => {
                let mut local = LocalTime::default();
                if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                    let tz = self.zone.as_ref().map_or_else(jiff::tz::TimeZone::system, |(_, tz)| tz.clone());
                    let z = t.to_zoned(tz);
                    local = LocalTime {
                        year: z.year(),
                        month: z.month(),
                        day: z.day(),
                        weekday: z.weekday().to_monday_zero_offset(),
                        hour: z.hour(),
                        minute: z.minute(),
                    };
                }
                self.local = Some((minute, local));
                local
            }
        }
    }
}
