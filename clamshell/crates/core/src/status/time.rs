//! The time tab. Its main page has:
//!
//! - the clock, with the time and the date;
//! - the time zone. A tap shows the regions (Europe, America, ...) and UTC, then a region's zones, with
//!   the one in force marked. A tapped zone is set at once, and the main page comes back;
//! - whether the network sets the clock (NTP);
//! - while it doesn't, the date and time to set by hand. Each press steps a field up or down, and it is
//!   set at once;
//! - whether ClamShell's clocks, in the bar and on this page, show 12 or 24 hours. This is ClamShell's
//!   own setting, and is saved.
//!
//! The system side, systemd's timedated, is read while the tab is open and after each change.

use crate::TimeId;
use crate::effect::{Dirty, Effect, Save};
use crate::kit::list::Scroll;
use crate::out::Out;
use crate::status::Sys;

/// The system's time zone and the ones to choose from, by tzdata name ("Europe/Oslo"), and whether the
/// network sets the clock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeZones {
    pub current: Option<String>,
    pub zones: Vec<String>,
    /// Whether the network sets the clock through NTP (`Some(true)`), or it is set by hand. `None` when
    /// the system has no way to sync, because there is no time sync service.
    pub ntp: Option<bool>,
    /// The clock is synchronised with the network.
    pub synced: bool,
    /// Why reading or setting failed, until a reading that works.
    pub error: Option<String>,
}

/// The regions the zones are chosen by. These are tzdata's first name parts, without the old and
/// administrative ones such as Etc, US and Canada. UTC is a row of its own after them.
pub const REGIONS: [&str; 10] = ["Africa", "America", "Antarctica", "Arctic", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"];

/// What the time tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimePage {
    #[default]
    Main,
    /// The regions to choose a zone from, and UTC.
    Regions,
    /// A region's zones, by its index in [`REGIONS`].
    Region(u8),
}

/// A field of the date and time, as set by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockField {
    Day,
    Month,
    Year,
    Hour,
    Minute,
}

impl ClockField {
    /// In the order the main page shows them, left to right.
    pub const ALL: [ClockField; 5] = [ClockField::Day, ClockField::Month, ClockField::Year, ClockField::Hour, ClockField::Minute];
}

/// A row of the time tab's list, which shows the regions or a region's zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRow<'a> {
    Region(&'a str),
    Utc,
    /// A zone of the region shown, by its full name.
    Zone(&'a str),
}

impl TimeZones {
    /// A region's zones, in tzdata's order.
    fn of<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.zones.iter().map(String::as_str).filter(move |z| in_region(z, region))
    }
}

/// Whether `zone` ("Europe/Oslo", "America/Argentina/Salta") is in `region`.
pub fn in_region(zone: &str, region: &str) -> bool {
    zone.strip_prefix(region).is_some_and(|rest| rest.starts_with('/'))
}

impl Sys {
    pub fn time_page(&self) -> TimePage {
        self.time_page
    }

    /// Back to the main page, as when the tab is left.
    pub(crate) fn reset_time_page(&mut self) {
        self.time_page = TimePage::Main;
    }

    /// Whether ClamShell's clocks show 12 hours rather than 24.
    pub fn twelve_hour(&self) -> bool {
        self.twelve_hour
    }

    /// Whether the date and time are set by hand now, rather than by the network.
    pub fn clock_by_hand(&self) -> bool {
        self.time.ntp != Some(true)
    }

    /// The region whose zones the tab shows, if one is open.
    pub fn time_region(&self) -> Option<&'static str> {
        match self.time_page {
            TimePage::Region(i) => REGIONS.get(usize::from(i)).copied(),
            _ => None,
        }
    }

    pub fn time_rows(&self) -> usize {
        match self.time_page {
            TimePage::Main => 0,
            TimePage::Regions => REGIONS.len() + 1,
            TimePage::Region(_) => self.time_region().map_or(0, |region| self.time.of(region).count()),
        }
    }

    pub fn time_row(&self, i: usize) -> Option<TimeRow<'_>> {
        match self.time_page {
            TimePage::Main => None,
            TimePage::Regions => REGIONS.get(i).map(|r| TimeRow::Region(r)).or((i == REGIONS.len()).then_some(TimeRow::Utc)),
            TimePage::Region(_) => self.time_region().and_then(|region| self.time.of(region).nth(i)).map(TimeRow::Zone),
        }
    }

    /// Whether a row is the zone in force, or the region it is in.
    pub fn time_row_current(&self, row: TimeRow) -> bool {
        let Some(current) = self.time.current.as_deref() else {
            return false;
        };
        match row {
            TimeRow::Region(region) => in_region(current, region),
            TimeRow::Utc => current == "UTC",
            TimeRow::Zone(zone) => current == zone,
        }
    }

    /// A tap on a row. A region opens, scrolled to the zone in force if it is there. UTC or a zone is
    /// set, and the main page comes back. `scroll` is the list's.
    pub(crate) fn on_time_row(&mut self, i: usize, scroll: Option<&mut Scroll>, out: &mut Out) {
        let region = match self.time_row(i) {
            Some(TimeRow::Region(_)) => u8::try_from(i).ok(),
            Some(TimeRow::Utc) => return self.set_zone("UTC", out),
            Some(TimeRow::Zone(zone)) => {
                let zone = zone.to_owned();
                return self.set_zone(&zone, out);
            }
            None => return,
        };
        let Some(region) = region else {
            return;
        };
        self.time_page = TimePage::Region(region);
        let current = self.time.current.as_deref();
        let at = self.time_region().and_then(|region| self.time.of(region).position(|z| Some(z) == current));
        self.show_time_row(at.unwrap_or(0), scroll);
    }

    fn set_zone(&mut self, zone: &str, out: &mut Out) {
        self.time_page = TimePage::Main;
        out.effects.push(Effect::SetTimeZone(zone.to_owned()));
    }

    /// A tap on one of the tab's own targets. `scroll` is the list's.
    pub(crate) fn on_time(&mut self, id: TimeId, scroll: Option<&mut Scroll>, out: &mut Out) {
        match id {
            TimeId::Back => self.time_back(scroll),
            TimeId::Zone => {
                // Show the regions, scrolled to the one the zone in force is in.
                self.time_page = TimePage::Regions;
                let current = self.time.current.as_deref();
                let at = REGIONS.iter().position(|r| current.is_some_and(|c| in_region(c, r)));
                self.show_time_row(at.unwrap_or(0), scroll);
            }
            TimeId::Ntp => out.effects.extend(self.time.ntp.map(|on| Effect::SetNtp(!on))),
            TimeId::TwelveHour => {
                self.twelve_hour = !self.twelve_hour;
                out.effects.push(Effect::Save(Save::TwelveHour(self.twelve_hour)));
                out.mark(Dirty::BOTH);
            }
            TimeId::Step(field, up) if self.clock_by_hand() => out.effects.push(Effect::StepClock(field, if up { 1 } else { -1 })),
            TimeId::Step(..) => {}
        }
        out.mark(Dirty::BOTTOM);
    }

    /// Goes back one page: from a region's zones to the regions, with that region shown, and from the
    /// regions to the main page.
    fn time_back(&mut self, scroll: Option<&mut Scroll>) {
        match self.time_page {
            TimePage::Main => {}
            TimePage::Regions => self.time_page = TimePage::Main,
            TimePage::Region(i) => {
                self.time_page = TimePage::Regions;
                self.show_time_row(usize::from(i), scroll);
            }
        }
    }

    fn show_time_row(&self, i: usize, scroll: Option<&mut Scroll>) {
        let len = self.time_rows();
        if let Some(scroll) = scroll {
            // Start from the top, since another region's scroll position means nothing here.
            scroll.scroll_by(-scroll.offset(), len);
            scroll.show(i, len);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::event::Reading;
    use crate::id::TimeId;
    use crate::nav::lists::ListId;
    use crate::{Event, Id, Init, State, Tab};

    fn state() -> State {
        let mut s = State::new(Init::default());
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Tab(Tab::Time))));
        let zones = ["Africa/Cairo", "America/Argentina/Salta", "America/New_York", "Europe/London", "Europe/Oslo", "UTC"];
        let time = TimeZones { current: Some("Europe/Oslo".to_owned()), zones: zones.map(str::to_owned).to_vec(), ntp: Some(true), synced: true, error: None };
        s.apply(Event::Reading(Reading::Time(time)));
        s
    }

    fn tap(s: &mut State, i: u16) -> Vec<Effect> {
        s.apply(Event::Pointer(crate::PointerEvent::Activate(ListId::Time.row(i)))).1
    }

    fn time(s: &mut State, id: TimeId) -> Vec<Effect> {
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Time(id)))).1
    }

    #[test]
    fn regions_then_their_zones_and_a_zone_tapped_is_set() {
        let mut s = state();
        assert_eq!((s.sys().time_page(), s.sys().time_rows()), (TimePage::Main, 0), "the main page first");
        time(&mut s, TimeId::Zone);
        assert_eq!(s.sys().time_rows(), REGIONS.len() + 1, "the regions and UTC");
        assert!(s.sys().time_row_current(TimeRow::Region("Europe")));
        assert!(!s.sys().time_row_current(TimeRow::Region("America")));
        assert!(tap(&mut s, 1).is_empty(), "America opens");
        assert_eq!(s.sys().time_region(), Some("America"));
        assert_eq!(
            (s.sys().time_row(0), s.sys().time_row(1), s.sys().time_row(2)),
            (Some(TimeRow::Zone("America/Argentina/Salta")), Some(TimeRow::Zone("America/New_York")), None)
        );
        time(&mut s, TimeId::Back);
        assert_eq!(s.sys().time_page(), TimePage::Regions, "back to the regions");
        tap(&mut s, 1);
        assert_eq!(tap(&mut s, 1), vec![Effect::SetTimeZone("America/New_York".to_owned())]);
        assert_eq!(s.sys().time_page(), TimePage::Main, "set: the main page again");
        time(&mut s, TimeId::Zone);
        assert_eq!(tap(&mut s, REGIONS.len() as u16), vec![Effect::SetTimeZone("UTC".to_owned())]);
    }

    #[test]
    fn the_clock_set_by_the_network_or_by_hand_and_twelve_hours_or_twenty_four() {
        let mut s = state();
        assert_eq!(time(&mut s, TimeId::Step(ClockField::Hour, true)), vec![], "the network sets it: no steps");
        assert_eq!(time(&mut s, TimeId::Ntp), vec![Effect::SetNtp(false)]);
        let mut by_hand = s.sys().time_zones().clone();
        by_hand.ntp = Some(false);
        s.apply(Event::Reading(Reading::Time(by_hand)));
        assert!(s.sys().clock_by_hand());
        assert_eq!(time(&mut s, TimeId::Step(ClockField::Month, false)), vec![Effect::StepClock(ClockField::Month, -1)]);
        assert!(!s.sys().twelve_hour());
        assert_eq!(time(&mut s, TimeId::TwelveHour), vec![Effect::Save(Save::TwelveHour(true))]);
        assert!(s.sys().twelve_hour());
    }

    #[test]
    fn the_tab_reads_the_time_zone_while_open() {
        let mut s = crate::testkit::state();
        let (_, effects) = s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Tab(Tab::Time))));
        assert_eq!(effects, vec![Effect::Watch(crate::Watch::Metrics, false), Effect::Watch(crate::Watch::Time, true)]);
    }
}
