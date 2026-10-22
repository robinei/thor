//! The time tab. Its main page: the clock (the time, the date), the time zone (a tap shows the regions,
//! Europe, America..., and UTC; then a region's zones, the one in force marked; a zone tapped is set at
//! once, and the main page is back), whether the network sets the clock (NTP), and while it doesn't, the
//! date and time to set by hand: each field a step up or down at a press, set at once. And whether
//! ClamShell's clocks (the bar's, this page's) show 12 hours or 24, ClamShell's own (saved).
//!
//! The system's side (systemd's timedated) is read while the tab is open and after each change.

use crate::State;
use crate::effect::{Dirty, Effect, Save};
use crate::nav::lists::ListId;

/// The time zone as the system has it, and the ones to choose from (tzdata's names, "Europe/Oslo"); and
/// whether the network sets the clock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeZones {
    pub current: Option<String>,
    pub zones: Vec<String>,
    /// The network (NTP) sets the clock (`Some(true)`), or it is set by hand; none: the system has no
    /// way to (no time sync service).
    pub ntp: Option<bool>,
    /// The clock is synchronised with the network.
    pub synced: bool,
    /// Why reading or setting failed, until a reading that works.
    pub error: Option<String>,
}

/// The regions the zones are chosen by (tzdata's first parts, without the old and the administrative
/// ones: Etc, US, Canada...); UTC is a row of its own after them.
pub const REGIONS: [&str; 10] = ["Africa", "America", "Antarctica", "Arctic", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"];

/// What the time tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimePage {
    #[default]
    Main,
    /// The regions to choose a zone from, and UTC.
    Regions,
    /// A region's zones (in [`REGIONS`]).
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
    /// As the main page shows them, left to right.
    pub const ALL: [ClockField; 5] = [ClockField::Day, ClockField::Month, ClockField::Year, ClockField::Hour, ClockField::Minute];
}

/// The time tab's targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeId {
    /// The header's way back: from a region's zones to the regions, from those to the main page.
    Back,
    /// The time zone's row: the regions.
    Zone,
    /// Whether the network sets the clock.
    Ntp,
    /// 12 hours or 24.
    TwelveHour,
    /// A field one up (`true`) or down, while the clock is set by hand.
    Step(ClockField, bool),
}

/// A row of the time tab's list (the regions, a region's zones).
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

impl State {
    pub fn time_zones(&self) -> &TimeZones {
        &self.sys.time
    }

    pub fn time_page(&self) -> TimePage {
        self.bottom.time_page
    }

    /// Whether ClamShell's clocks show 12 hours (else 24).
    pub fn twelve_hour(&self) -> bool {
        self.twelve_hour
    }

    /// Whether the date and time are set by hand now (the network doesn't).
    pub fn clock_by_hand(&self) -> bool {
        self.sys.time.ntp != Some(true)
    }

    /// The region whose zones the tab shows, if one is open.
    pub fn time_region(&self) -> Option<&'static str> {
        match self.bottom.time_page {
            TimePage::Region(i) => REGIONS.get(usize::from(i)).copied(),
            _ => None,
        }
    }

    pub(crate) fn time_rows(&self) -> usize {
        match self.bottom.time_page {
            TimePage::Main => 0,
            TimePage::Regions => REGIONS.len() + 1,
            TimePage::Region(_) => self.time_region().map_or(0, |region| self.sys.time.of(region).count()),
        }
    }

    pub fn time_row(&self, i: usize) -> Option<TimeRow<'_>> {
        match self.bottom.time_page {
            TimePage::Main => None,
            TimePage::Regions => REGIONS.get(i).map(|r| TimeRow::Region(r)).or((i == REGIONS.len()).then_some(TimeRow::Utc)),
            TimePage::Region(_) => self.time_region().and_then(|region| self.sys.time.of(region).nth(i)).map(TimeRow::Zone),
        }
    }

    /// Whether a row is the zone in force, or the region it is in.
    pub fn time_row_current(&self, row: TimeRow) -> bool {
        let Some(current) = self.sys.time.current.as_deref() else {
            return false;
        };
        match row {
            TimeRow::Region(region) => in_region(current, region),
            TimeRow::Utc => current == "UTC",
            TimeRow::Zone(zone) => current == zone,
        }
    }

    /// A tap on a row: a region opens (scrolled to the zone in force if it is there); UTC or a zone is
    /// set, and the main page is back.
    pub(crate) fn on_time_row(&mut self, i: usize) -> Vec<Effect> {
        let region = match self.time_row(i) {
            Some(TimeRow::Region(_)) => u8::try_from(i).ok(),
            Some(TimeRow::Utc) => return self.set_zone("UTC"),
            Some(TimeRow::Zone(zone)) => {
                let zone = zone.to_owned();
                return self.set_zone(&zone);
            }
            None => return vec![],
        };
        let Some(region) = region else {
            return vec![];
        };
        self.bottom.time_page = TimePage::Region(region);
        let current = self.sys.time.current.as_deref();
        let at = self.time_region().and_then(|region| self.sys.time.of(region).position(|z| Some(z) == current));
        self.show_time_row(at.unwrap_or(0));
        vec![]
    }

    fn set_zone(&mut self, zone: &str) -> Vec<Effect> {
        self.bottom.time_page = TimePage::Main;
        vec![Effect::SetTimeZone(zone.to_owned())]
    }

    /// A tap on one of the tab's own targets.
    pub(crate) fn on_time(&mut self, id: TimeId) -> (Dirty, Vec<Effect>) {
        let effects = match id {
            TimeId::Back => {
                self.time_back();
                vec![]
            }
            TimeId::Zone => {
                // (The regions, scrolled to the one the zone in force is in.)
                self.bottom.time_page = TimePage::Regions;
                let current = self.sys.time.current.as_deref();
                let at = REGIONS.iter().position(|r| current.is_some_and(|c| in_region(c, r)));
                self.show_time_row(at.unwrap_or(0));
                vec![]
            }
            TimeId::Ntp => self.sys.time.ntp.map(|on| Effect::SetNtp(!on)).into_iter().collect(),
            TimeId::TwelveHour => {
                self.twelve_hour = !self.twelve_hour;
                return (Dirty::BOTH, vec![Effect::Save(Save::TwelveHour(self.twelve_hour))]);
            }
            TimeId::Step(field, up) if self.clock_by_hand() => vec![Effect::StepClock(field, if up { 1 } else { -1 })],
            TimeId::Step(..) => vec![],
        };
        (Dirty::BOTTOM, effects)
    }

    /// Back one page: from a region's zones to the regions (that region shown), from those to the main
    /// page.
    pub(crate) fn time_back(&mut self) {
        match self.bottom.time_page {
            TimePage::Main => {}
            TimePage::Regions => self.bottom.time_page = TimePage::Main,
            TimePage::Region(i) => {
                self.bottom.time_page = TimePage::Regions;
                self.show_time_row(usize::from(i));
            }
        }
    }

    fn show_time_row(&mut self, i: usize) {
        let len = self.time_rows();
        if let Some(scroll) = self.scroll_mut(ListId::Time) {
            // (From the top: another region's place means nothing here.)
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
    use crate::{Event, Id, Init, Tab};

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
        assert_eq!((s.time_page(), s.time_rows()), (TimePage::Main, 0), "the main page first");
        time(&mut s, TimeId::Zone);
        assert_eq!(s.time_rows(), REGIONS.len() + 1, "the regions and UTC");
        assert!(s.time_row_current(TimeRow::Region("Europe")));
        assert!(!s.time_row_current(TimeRow::Region("America")));
        assert!(tap(&mut s, 1).is_empty(), "America opens");
        assert_eq!(s.time_region(), Some("America"));
        assert_eq!(
            (s.time_row(0), s.time_row(1), s.time_row(2)),
            (Some(TimeRow::Zone("America/Argentina/Salta")), Some(TimeRow::Zone("America/New_York")), None)
        );
        time(&mut s, TimeId::Back);
        assert_eq!(s.time_page(), TimePage::Regions, "back to the regions");
        tap(&mut s, 1);
        assert_eq!(tap(&mut s, 1), vec![Effect::SetTimeZone("America/New_York".to_owned())]);
        assert_eq!(s.time_page(), TimePage::Main, "set: the main page again");
        time(&mut s, TimeId::Zone);
        assert_eq!(tap(&mut s, REGIONS.len() as u16), vec![Effect::SetTimeZone("UTC".to_owned())]);
    }

    #[test]
    fn the_clock_set_by_the_network_or_by_hand_and_twelve_hours_or_twenty_four() {
        let mut s = state();
        assert_eq!(time(&mut s, TimeId::Step(ClockField::Hour, true)), vec![], "the network sets it: no steps");
        assert_eq!(time(&mut s, TimeId::Ntp), vec![Effect::SetNtp(false)]);
        let mut by_hand = s.time_zones().clone();
        by_hand.ntp = Some(false);
        s.apply(Event::Reading(Reading::Time(by_hand)));
        assert!(s.clock_by_hand());
        assert_eq!(time(&mut s, TimeId::Step(ClockField::Month, false)), vec![Effect::StepClock(ClockField::Month, -1)]);
        assert!(!s.twelve_hour());
        assert_eq!(time(&mut s, TimeId::TwelveHour), vec![Effect::Save(Save::TwelveHour(true))]);
        assert!(s.twelve_hour());
    }

    #[test]
    fn the_tab_reads_the_time_zone_while_open() {
        let mut s = crate::testkit::state();
        let (_, effects) = s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Tab(Tab::Time))));
        assert_eq!(effects, vec![Effect::Watch(crate::Watch::Metrics, false), Effect::Watch(crate::Watch::Time, true)]);
    }
}
