//! The time tab (the clock in the bar opens it): the time zone in force, then the regions and UTC; a
//! region tapped shows its zones (a back arrow returns), the one in force marked by a dot as is the region
//! it is in. A zone tapped is set at once. Zones' names as tzdata has them, made readable: the region left
//! out, underscores as spaces ("Argentina / Buenos Aires").

use std::fmt::{self, Display, Formatter, Write};

use clamshell_core::{Id, ListId, TimeRow};

use crate::toolkit::Frame;
use crate::toolkit::format::Fmt;
use crate::toolkit::style::Tone;
use crate::toolkit::widgets::{self, EditCell, HeaderRight, ListArea, ListRow};

/// A row's height, of the area's height.
const ROW_H: f32 = 0.105;

pub(super) fn draw(f: &mut Frame) {
    let s = f.s;
    let zones = s.time_zones();
    let region = s.time_region();
    let title = match (region, zones.current.as_deref()) {
        (Some(region), _) => Fmt::<96>::of(format_args!("{region}")),
        (None, Some(current)) => Fmt::of(format_args!("Time zone: {}", Name(current))),
        (None, None) => Fmt::of(format_args!("Time zone")),
    };
    let area = widgets::header(f, region.map(|_| Id::TimeRegions), &title, false, HeaderRight::None);
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    if let Some(error) = &zones.error {
        list.hint(f, error);
    }
    let rows = list.scrolled(f, ListId::Time, s.list_len(ListId::Time));
    for i in rows.shown() {
        let Some(row) = s.time_row(i) else {
            continue;
        };
        let current = s.time_row_current(row);
        let label = match row {
            TimeRow::Region(region) => Fmt::<64>::of(format_args!("{region}")),
            TimeRow::Utc => Fmt::of(format_args!("UTC")),
            TimeRow::Zone(zone) => Fmt::of(format_args!("{}", City(zone))),
        };
        let item = ListRow { label: &label, tone: Tone::high_if(current), mark: current, note: "", edit: EditCell::None };
        widgets::list_row(f, ListId::Time.row(i as u16), rows.row(i), list.pad, list.size, item);
    }
    list.end(f, rows);
}

/// A zone's name, readable: "America/Argentina/Buenos_Aires" as "America / Argentina / Buenos Aires".
struct Name<'a>(&'a str);

impl Display for Name<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for c in self.0.chars() {
            match c {
                '_' => f.write_char(' ')?,
                '/' => f.write_str(" / ")?,
                c => f.write_char(c)?,
            }
        }
        Ok(())
    }
}

/// A zone's name without its region (the header names it).
struct City<'a>(&'a str);

impl Display for City<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let city = self.0.split_once('/').map_or(self.0, |(_, rest)| rest);
        Name(city).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_names_are_readable() {
        assert_eq!(format!("{}", Name("America/Argentina/Buenos_Aires")), "America / Argentina / Buenos Aires");
        assert_eq!(format!("{}", City("America/Argentina/Buenos_Aires")), "Argentina / Buenos Aires");
        assert_eq!(format!("{}", City("Europe/Oslo")), "Oslo");
    }
}
