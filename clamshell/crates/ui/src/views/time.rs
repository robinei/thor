//! The time tab, opened by the clock in the bar.
//!
//! The main page shows the clock large with the date under it, then rows for the time zone, whether the
//! network sets the clock, and 24 or 12 hours. Tapping the time zone lists the regions and UTC, then a
//! region's zones. A dot marks the zone in force and the region it is in. Tapping a zone sets it and
//! returns to the main page.
//!
//! While the clock is set by hand, the clock's place shows a field each for day, month, year, hour and
//! minute, with an arrow above and below to step it. Each step is set at once.
//!
//! Zone names are tzdata's, made readable: the region left out and underscores shown as spaces
//! ("Argentina / Buenos Aires").

use std::fmt::{self, Display, Formatter, Write};

use clamshell_core::{ClockField, Id, ListId, TimeId, TimePage, TimeRow};

use crate::gfx::icons::Icon;
use crate::gfx::{Column, Rect};
use crate::toolkit::format::Fmt;
use crate::toolkit::style::{self, Look, Text, Tone};
use crate::toolkit::widgets::{self, EditCell, HeaderRight, ListArea, ListRow};
use crate::toolkit::{Frame, LocalTime};

/// A row's height, as a fraction of the area's height.
const ROW_H: f32 = 0.105;
/// The large clock's text size, as a fraction of its area's height.
const CLOCK: f32 = 0.4;
/// The size of the steppers' values as a fraction of their area's height, and of their arrows as a
/// fraction of a column's height.
const STEP_VALUE: f32 = 0.2;
const STEP_ARROW: f32 = 0.28;
/// The gap between the date's three columns and the time's two, as a fraction of a column's width.
const STEP_GAP: f32 = 0.5;

pub(super) fn draw(f: &mut Frame) {
    match f.s.sys().time_page() {
        TimePage::Main => main(f),
        TimePage::Regions | TimePage::Region(_) => zones(f),
    }
}

/// The main page: the clock, or its fields while it is set by hand, then the rows.
fn main(f: &mut Frame) {
    let s = f.s;
    let body = f.body();
    let (pad, size) = (f.pad(), f.text(Text::Body));
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let row_h = body.h * ROW_H;
    let rows_h = 3.0 * row_h;
    let clock = Rect::new(area.x, area.y, area.w, area.h - rows_h - pad);
    let local = f.scratch.local(s.clock().unix(), s.sys().time_zones().current.as_deref());
    if s.sys().clock_by_hand() {
        steppers(f, clock, local);
    } else {
        large_clock(f, clock, local);
    }
    let mut col = Column::new(Rect::new(area.x, area.bottom() - rows_h, area.w, rows_h));
    let zones = s.sys().time_zones();
    let zone = col.take(row_h);
    let name = match zones.current.as_deref() {
        Some(current) => Fmt::<96>::of(format_args!("{}", Name(current))),
        None => Fmt::of(format_args!("Unknown")),
    };
    f.button(Id::Time(TimeId::Zone), zone, Look::ROW, |f| widgets::value_row(f, zone, size, "Time zone", &name, true));
    let ntp = col.take(row_h);
    match zones.ntp {
        Some(on) => {
            let label = match (on, zones.synced) {
                (true, true) => "Set automatically · in sync",
                (true, false) => "Set automatically · not in sync yet",
                (false, _) => "Set automatically",
            };
            widgets::switch_row(f, Id::Time(TimeId::Ntp), ntp, size, label, on);
        }
        None => widgets::value_row(f, ntp, size, "Set automatically", "Unavailable", false),
    }
    widgets::switch_row(f, Id::Time(TimeId::TwelveHour), col.take(row_h), size, "24-hour clock", !s.sys().twelve_hour());
    if let Some(error) = &zones.error {
        let line = Rect::new(clock.x, clock.bottom() - size * 1.4, clock.w, size * 1.4);
        widgets::centred_text(f, line, f.text(Text::Small), Tone::Accent, error);
    }
}

/// The date as the clock's line under the time says it: "Sunday 18 October 2026".
fn date(local: LocalTime) -> Fmt<48> {
    Fmt::of(format_args!("{} {} {} {}", local.weekday_name(), local.day, local.month_name(), local.year))
}

/// The time in large text, centred in `r`, with the date under it. On a 12-hour clock, AM or PM follows
/// in smaller text.
fn large_clock(f: &mut Frame, r: Rect, local: LocalTime) {
    let s = f.s;
    let big = r.h * CLOCK;
    let small = f.text(Text::Body);
    let (time, half) = if s.sys().twelve_hour() {
        let (h, pm) = local.twelve();
        (Fmt::<8>::of(format_args!("{h}:{:02}", local.minute)), Some(if pm { "PM" } else { "AM" }))
    } else {
        (Fmt::of(format_args!("{:02}:{:02}", local.hour, local.minute)), None)
    };
    let half_size = big * 0.35;
    let time_w = f.c.text_width(big, &time);
    let half_w = half.map_or(0.0, |h| f.c.text_width(half_size, h) + half_size * 0.4);
    let block_h = big * 1.25 + small * 1.6;
    let top = r.y + (r.h - block_h) / 2.0;
    let x = r.x + (r.w - time_w - half_w) / 2.0;
    f.c.text(x, top, big, Tone::High, &time);
    if let Some(h) = half {
        // Roughly on the time's baseline, with its top a little below the digits' top.
        f.c.text(x + time_w + half_size * 0.4, top + big * 0.55, half_size, Tone::Dim, h);
    }
    let line = Rect::new(r.x, top + big * 1.25, r.w, small * 1.6);
    widgets::centred_text(f, line, small, Tone::Dim, &date(local));
}

/// The date and time fields to set by hand, a column each with an up arrow, the value and a down arrow.
/// The date goes under them.
fn steppers(f: &mut Frame, r: Rect, local: LocalTime) {
    let s = f.s;
    let small = f.text(Text::Body);
    let fields = ClockField::ALL;
    let units = fields.len() as f32 + STEP_GAP;
    let col_w = (r.w / units).min(r.h * 0.9);
    let total = col_w * units;
    let block = Rect::new(r.x + (r.w - total) / 2.0, r.y, total, r.h - small * 1.8);
    let value_size = (block.h * STEP_VALUE).min(col_w * 0.42);
    let arrow = block.h * STEP_ARROW;
    let mut x = block.x;
    for field in fields {
        if field == ClockField::Hour {
            x += col_w * STEP_GAP;
        }
        let column = Rect::new(x, block.y, col_w, block.h);
        x += col_w;
        let value = match field {
            ClockField::Day => Fmt::<12>::of(format_args!("{}", local.day)),
            ClockField::Month => Fmt::of(format_args!("{}", local.month_name().get(..3).unwrap_or(""))),
            ClockField::Year => Fmt::of(format_args!("{}", local.year)),
            ClockField::Hour if s.sys().twelve_hour() => {
                let (h, pm) = local.twelve();
                Fmt::of(format_args!("{h} {}", if pm { "PM" } else { "AM" }))
            }
            ClockField::Hour => Fmt::of(format_args!("{:02}", local.hour)),
            ClockField::Minute => Fmt::of(format_args!("{:02}", local.minute)),
        };
        let up = Rect::new(column.x, column.y, column.w, arrow);
        let down = Rect::new(column.x, column.bottom() - arrow, column.w, arrow);
        for (rect, is_up) in [(up, true), (down, false)] {
            f.button(Id::Time(TimeId::Step(field, is_up)), rect, Look::ICON, |f| {
                let side = arrow * 0.45;
                f.c.icon(if is_up { Icon::ArrowUp } else { Icon::ArrowDown }, rect.square_center(side), Tone::Fn);
            });
        }
        let middle = Rect::new(column.x, up.bottom(), column.w, down.y - up.bottom());
        let fit = value_size.min(column.w * 0.9 / (f.c.text_width(value_size, &value) / value_size).max(1.0));
        f.c.text_ink_centred(middle, fit, Tone::High, &value);
    }
    let line = Rect::new(r.x, block.bottom() + small * 0.2, r.w, small * 1.6);
    widgets::centred_text(f, line, small, Tone::Dim, &date(local));
}

/// The zone pages: the regions and UTC, or a region's zones. The header's arrow goes back a page.
fn zones(f: &mut Frame) {
    let s = f.s;
    let zones = s.sys().time_zones();
    let region = s.sys().time_region();
    let title = match region {
        Some(region) => Fmt::<96>::of(format_args!("{region}")),
        None => Fmt::of(format_args!("Time zone")),
    };
    let area = widgets::header(f, Some(Id::Time(TimeId::Back)), &title, false, HeaderRight::None);
    let list = ListArea::new(f, area, f.body().h * ROW_H);
    if let Some(error) = &zones.error {
        list.hint(f, error);
    }
    let rows = list.scrolled(f, ListId::Time, s.lists().len(ListId::Time));
    for i in rows.shown() {
        let Some(row) = s.sys().time_row(i) else {
            continue;
        };
        let current = s.sys().time_row_current(row);
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

/// A zone's name made readable: "America/Argentina/Buenos_Aires" becomes "America / Argentina / Buenos
/// Aires".
pub(super) struct Name<'a>(pub(super) &'a str);

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
